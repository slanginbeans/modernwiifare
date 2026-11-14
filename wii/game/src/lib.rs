//! `mwf_game.dol`: load `<app folder>/maps/<map>.wpk` (e.g. `sd:/apps/mwf_game/maps/`) and play it on the GameCube
//! pad.
//!
//! Controls (port 1), walking: main stick move, C-stick look, A jump, B crouch (held), Z sprint, L ADS.
//! D-pad up toggles free fly (main stick, C-stick, L/R triggers down/up, B fast). Y next spawn, START exit.
//! Packs without collision start in free fly.
//! If the pack is missing or broken the screen shows a solid colour: red = no SD / no file, magenta = bad pack,
//! orange = out of memory or textures. START exits.
#![no_std]

extern crate alloc;

mod arsenal;
mod devlog;
mod ffi;
mod front;
mod rt;
mod sound;
mod ui;
mod view;

use alloc::vec::Vec;

use dpvs_iw4::WalkScratch;
use game_core::arena::{self, Arena, Event};
use game_core::camera::Camera;
use game_core::collision::MapClip;
use game_core::fly::{self, PadInput};
use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::gun::{Gun, GunInput, SpreadInput};
use game_core::hud::{self, Rect, heapless_str::Str};
use game_core::life::{self, Life, MatchClock};
use game_core::look;
use game_core::player::{MovePad, Player};
use game_core::telemetry::{self, section};
use game_core::world::World;
use game_core::world::{FrameStats, VisData};

/// Portal-walk scratch for `World::draw` (single-threaded; only `mwf_game_main` touches it).
static mut WALK_SCRATCH: WalkScratch = WalkScratch::new();

/// `Gfx` over the C shim.
#[derive(Default)]
struct Shim {
    /// Microseconds the CPU waited for the GPU at the end of the last frame.
    gpu_wait_us: u32,
}

impl Gfx for Shim {
    fn tex_create(&mut self, d: &TexDesc) -> i32 {
        let desc = ffi::TexDesc {
            data: d.data,
            alpha: d.alpha,
            width: d.width,
            height: d.height,
            format: d.format,
            levels: d.levels,
            wrap: d.wrap,
            _pad: 0,
        };
        unsafe { ffi::mwf_tex_create(&desc) }
    }
    fn frame_begin(&mut self, view: &Mtx34, fovy: f32, znear: f32, zfar: f32, clear: u32) {
        unsafe { ffi::mwf_frame_begin(view, fovy, znear, zfar, clear) }
    }
    fn world_arrays(&mut self, v: *const u8, stride: u8) {
        unsafe { ffi::mwf_world_arrays(v, u32::from(stride)) }
    }
    fn world_state(&mut self, s: &DrawState) {
        let st = ffi::DrawState {
            tex: s.tex,
            lightmap: s.lightmap,
            blend: s.blend,
            alpha_ref: s.alpha_ref,
            cull: s.cull,
            _pad: 0,
            tint: s.tint,
        };
        unsafe { ffi::mwf_world_state(&st) }
    }
    fn pos_matrix(&mut self, m: &Mtx34) {
        unsafe { ffi::mwf_pos_matrix(m) }
    }
    fn call_dl(&mut self, dl: &[u8]) {
        unsafe { ffi::mwf_call_dl(dl.as_ptr(), dl.len() as u32) }
    }
    fn sky_face(&mut self, rot: &Mtx34, corners: &[[f32; 5]; 4], tex: i32) {
        unsafe { ffi::mwf_sky_face(rot, corners, tex) }
    }
    fn hud_rects(&mut self, rects: &[game_core::hud::Rect], rgba: u32) {
        unsafe { ffi::mwf_hud_rects(rects.as_ptr(), rects.len() as u32, rgba) }
    }
    fn frame_end(&mut self) {
        self.gpu_wait_us = unsafe { ffi::mwf_frame_end() };
    }
}

/// Frame timing for the overlay, averaged over half a second.
#[derive(Default)]
struct Timing {
    window_start: u32,
    frames: u32,
    sum_frame: u32,
    sum_cpu: u32,
    sum_gpu: u32,
    /// Last window's averages in microseconds, and frames per second.
    fps: u32,
    frame_us: u32,
    cpu_us: u32,
    gpu_us: u32,
}

impl Timing {
    fn add(&mut self, now: u32, frame_us: u32, cpu_us: u32, gpu_us: u32) {
        self.frames += 1;
        self.sum_frame += frame_us;
        self.sum_cpu += cpu_us;
        self.sum_gpu += gpu_us;
        let span = now.wrapping_sub(self.window_start);
        if span >= 500_000 {
            self.fps = (self.frames as u64 * 1_000_000 / span as u64) as u32;
            self.frame_us = self.sum_frame / self.frames;
            self.cpu_us = self.sum_cpu / self.frames;
            self.gpu_us = self.sum_gpu / self.frames;
            *self = Timing {
                window_start: now,
                fps: self.fps,
                frame_us: self.frame_us,
                cpu_us: self.cpu_us,
                gpu_us: self.gpu_us,
                ..Timing::default()
            };
        }
    }
}

/// The debug overlay: timings, what was drawn, where the player is.
fn overlay(gfx: &mut Shim, rects: &mut Vec<Rect>, t: &Timing, s: &FrameStats, eye: [f32; 3]) {
    let mut line = Str::default();
    let ms = |line: &mut Str, us: u32| hud::fixed1(line, us as f32 / 1000.0);
    line.push_str("FPS ");
    line.num(t.fps);
    line.push_str("  FRAME ");
    ms(&mut line, t.frame_us);
    line.push_str("  CPU ");
    ms(&mut line, t.cpu_us);
    line.push_str("  GPU WAIT ");
    ms(&mut line, t.gpu_us);
    line.push_str(" MS\nTRIS ");
    line.num(s.triangles);
    line.push_str(" (MODELS ");
    line.num(s.model_triangles);
    line.push_str(")  DRAWS ");
    line.num(s.batches);
    line.push_str("  STATE ");
    line.num(s.state_changes);
    line.push_str("  CELLS ");
    line.num(s.cells_visible);
    line.push_str(if s.in_cell { "" } else { " (OUT)" });
    line.push_str("\nMODELS ");
    line.num(s.instances);
    line.push_str("  CULLED ");
    line.num(s.instances_culled + s.culled);
    line.push_str("\nPOS ");
    for v in eye {
        if v < 0.0 {
            line.push('-');
        }
        line.num(libm::fabsf(v) as u32);
        line.push(' ');
    }
    rects.clear();
    let w = hud::text(rects, 24, 24, 2, line.as_str());
    let bg = [[20, 20, w + 8, 4 * 12 + 4]];
    gfx.hud_rects(&bg, 0x0000_00a0);
    gfx.hud_rects(rects, 0xffff_60ff);
}

/// Screen colour for the last file error: red = no SD card or no such file, orange = out of memory, magenta = other.
fn file_error_colour() -> u32 {
    match unsafe { ffi::mwf_file_error() } {
        1 | 2 => 0xc02020ff,
        3 => 0xe08020ff,
        _ => 0xc020c0ff,
    }
}

/// Show a solid colour until START (back to the menu) or RESET (quit); no console while GX owns the screen.
fn fail(colour: u32) -> Exit {
    devlog::event(
        ffi::ms(),
        "load_fail",
        format_args!("colour={colour:08x} file_error={}", unsafe {
            ffi::mwf_file_error()
        }),
    );
    devlog::end(ffi::ms());
    let cam = Camera::new([0.0; 3], 0.0, 0.0);
    loop {
        unsafe {
            ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, colour);
            ffi::mwf_frame_end();
        }
        let pad = ffi::pad0();
        if ffi::should_exit() {
            return Exit::Quit;
        }
        if pad.down & ffi::BTN_START != 0 {
            return Exit::Menu;
        }
    }
}

fn spawn_player(world: &World<'_>, clip: &MapClip<'_>, i: usize) -> Player {
    match world.spawns.get(i % world.spawns.len().max(1)) {
        Some(s) => Player::spawn(clip, s.origin, s.yaw),
        None => Player::spawn(clip, [0.0, 0.0, 200.0], 0.0),
    }
}

fn spawn_camera(world: &World<'_>, i: usize) -> Camera {
    match world.spawns.get(i % world.spawns.len().max(1)) {
        Some(s) => Camera::new([s.origin[0], s.origin[1], s.origin[2] + 60.0], s.yaw, 0.0),
        None => Camera::new([0.0, 0.0, 200.0], 0.0, 0.0),
    }
}

/// Damage vignette, death screen, match clock and deaths.
fn draw_life_hud(gfx: &mut Shim, rects: &mut Vec<Rect>, life: &Life, clock: &MatchClock, now: u32) {
    let hurt = life.hurt(now);
    if hurt > 0.01 {
        let a = (hurt * 200.0) as u32;
        let edge = 48;
        rects.clear();
        rects.extend_from_slice(&[
            [0, 0, 640, edge],
            [0, 480 - edge, 640, edge],
            [0, edge, edge, 480 - 2 * edge],
            [640 - edge, edge, edge, 480 - 2 * edge],
        ]);
        gfx.hud_rects(rects, 0xa0000000 | a);
    }
    if !life.alive() {
        gfx.hud_rects(&[[0, 0, 640, 480]], 0x20000080);
        let mut line = Str::default();
        line.push_str("YOU DIED\nRESPAWN IN ");
        line.num(life.respawn_in(now).div_ceil(1000));
        rects.clear();
        hud::text(rects, 236, 200, 6, line.as_str());
        gfx.hud_rects(rects, 0xf0e8d0ff);
    }
    let left = clock.remaining_ms(now) / 1000;
    let mut line = Str::default();
    line.num(left / 60);
    line.push(':');
    if left % 60 < 10 {
        line.push('0');
    }
    line.num(left % 60);
    line.push_str("   DEATHS ");
    line.num(life.deaths);
    rects.clear();
    hud::text(rects, 24, 452, 2, line.as_str());
    gfx.hud_rects(rects, 0xd0d8e0c0);
}

/// Crosshair, ammo, weapon name and bullet impacts (markers that fade over 400 ms).
fn draw_gun_hud(
    gfx: &mut Shim,
    rects: &mut Vec<Rect>,
    g: &Gun,
    cam: &Camera,
    impacts: &[(Option<[f32; 3]>, u32)],
    now: u32,
) {
    // Crosshair: four ticks at the spread cone's edge (as MW2's hip-fire crosshair opens while firing or moving).
    let half_w = libm::tanf(cam.fovy.to_radians() * 0.5) * cam.aspect;
    let gap = (libm::tanf(g.spread_degrees.to_radians()) / half_w * 320.0).clamp(3.0, 120.0) as i16;
    rects.clear();
    rects.extend_from_slice(&[
        [320 - gap - 8, 239, 8, 2],
        [320 + gap, 239, 8, 2],
        [319, 240 - gap - 8, 2, 8],
        [319, 240 + gap, 2, 8],
    ]);
    gfx.hud_rects(rects, 0xffffffc0);
    rects.clear();
    for &(at, t) in impacts {
        let Some(p) = at else { continue };
        let age = now.wrapping_sub(t);
        if age > 400 {
            continue;
        }
        if let Some([x, y]) = cam.project(p) {
            rects.push([x as i16 - 2, y as i16 - 2, 4, 4]);
        }
    }
    gfx.hud_rects(rects, 0xffd040e0);
    // Ammo and weapon, bottom right.
    let (clip, stock) = g.ammo();
    let mut line = Str::default();
    line.num(clip.max(0) as u32);
    line.push_str(" / ");
    line.num(stock.max(0) as u32);
    rects.clear();
    hud::text(rects, 520, 430, 4, line.as_str());
    gfx.hud_rects(rects, if clip > 0 { 0xf0f0f0e0 } else { 0xe04040e0 });
    rects.clear();
    let mut name = Str::default();
    name.push_str(g.name().trim_end_matches("_mp"));
    hud::text(rects, 520, 412, 2, name.as_str());
    gfx.hud_rects(rects, 0xc0c8d0c0);
}

/// A NUL-terminated path inside the app folder: `<app folder>/<rest>`.
pub(crate) struct AppPath([u8; 192]);

impl AppPath {
    pub(crate) fn new(rest: &str) -> Option<AppPath> {
        let dir = unsafe { core::ffi::CStr::from_ptr(ffi::mwf_app_dir()) }.to_bytes();
        let mut p = AppPath([0; 192]);
        let n = dir.len() + 1 + rest.len();
        if n + 1 > p.0.len() {
            return None;
        }
        p.0[..dir.len()].copy_from_slice(dir);
        p.0[dir.len()] = b'/';
        p.0[dir.len() + 1..n].copy_from_slice(rest.as_bytes());
        Some(p)
    }
    pub(crate) fn as_ptr(&self) -> *const core::ffi::c_char {
        self.0.as_ptr().cast()
    }
}

/// A whole pack file into MEM2 (flushed; GX and the DSP read it in place), checked; None when missing or bad.
pub(crate) fn load_pack(path: &AppPath) -> Option<&'static [u8]> {
    let mut head = [0u8; wpk::HEADER_LEN];
    let got = unsafe { ffi::mwf_file_read(path.as_ptr(), 0, head.len() as u32, head.as_mut_ptr()) };
    if got != head.len() as i32 {
        return None;
    }
    let len = u32::from_be_bytes([head[24], head[25], head[26], head[27]]) as usize;
    if len > 16 << 20 {
        return None;
    }
    let p = unsafe { ffi::mwf_file_load_range(path.as_ptr(), 0, len as u32) };
    if p.is_null() {
        return None;
    }
    let bytes: &'static [u8] = unsafe { core::slice::from_raw_parts(p, len) };
    wpk::Pack::parse(bytes, false).is_ok().then_some(bytes)
}

/// `<app folder>/common.wpk` (from `mwf-convert common`) in MEM2 for the whole run, or empty when it isn't there.
fn load_common() -> &'static [u8] {
    AppPath::new("common.wpk")
        .and_then(|p| load_pack(&p))
        .unwrap_or(&[])
}

fn weapon_section(common: &[u8]) -> &[u8] {
    wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::WEAPONS))
        .map_or(&[], |s| s.data)
}

/// Where `run_map` goes back to.
enum Exit {
    Menu,
    Quit,
}

/// The map packs on the SD card (file stems, sorted).
fn list_maps(buf: &mut [u8; 2048]) -> Vec<&str> {
    let Some(dir) = AppPath::new("maps") else {
        return Vec::new();
    };
    let n = unsafe {
        ffi::mwf_list_dir(
            dir.as_ptr(),
            c".wpk".as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    };
    let text = buf
        .iter()
        .position(|&b| b == 0)
        .map_or(&buf[..], |end| &buf[..end]);
    let mut maps: Vec<&str> = core::str::from_utf8(text)
        .unwrap_or("")
        .split('\n')
        .filter(|m| !m.is_empty())
        .take(n.max(0) as usize)
        .collect();
    maps.sort_unstable();
    maps
}

/// Load `<app folder>/maps/<name>.wpk` and play it until START (menu) or RESET (quit). Everything the map
/// allocates in MEM2 is released by the caller (`mwf_mem2_release`); the MEM1 parts drop here.
/// Bots in a free-for-all (MW2 FFA takes up to 18 players; the Wii's CPU budget starts at 5 bots, see
/// docs/perf.md).
const BOTS: usize = 5;

fn fighter_name(i: usize) -> Str {
    let mut s = Str::default();
    if i == 0 {
        s.push_str("YOU");
    } else {
        s.push_str("BOT ");
        s.num(i as u32);
    }
    s
}

/// Kills (yours and the leader's), the kill feed (4 s) and the hitmarker (150 ms).
fn draw_match_hud(
    gfx: &mut Shim,
    rects: &mut Vec<Rect>,
    a: &Arena,
    feed: &[(Option<(usize, usize)>, u32)],
    hitmarker_ms: Option<u32>,
    now: u32,
) {
    let mut line = Str::default();
    line.push_str("SCORE ");
    line.num(a.fighters[0].score);
    line.push_str("  LEADER ");
    let (who, k) = a.leader();
    line.push_str(fighter_name(who).as_str());
    line.push(' ');
    line.num(k);
    if a.rules.score_limit > 0 {
        line.push_str(" / ");
        line.num(a.rules.score_limit);
    }
    rects.clear();
    hud::text(rects, 360, 24, 2, line.as_str());
    gfx.hud_rects(rects, 0xf0e8d0e0);
    for (row, (e, t)) in feed.iter().enumerate() {
        let Some((killer, victim)) = e else {
            continue;
        };
        if now.wrapping_sub(*t) > 4000 {
            continue;
        }
        let mut l = Str::default();
        if killer == victim {
            l.push_str(fighter_name(*victim).as_str());
            l.push_str(" DIED");
        } else {
            l.push_str(fighter_name(*killer).as_str());
            l.push_str(" KILLED ");
            l.push_str(fighter_name(*victim).as_str());
        }
        rects.clear();
        hud::text(rects, 24, 300 + row as i16 * 14, 2, l.as_str());
        let mine = *killer == 0 || *victim == 0;
        gfx.hud_rects(rects, if mine { 0xffd040e0 } else { 0xc0c8d0c0 });
    }
    if hitmarker_ms.is_some_and(|t| now.wrapping_sub(t) < 150) {
        // MW2's hitmarker: four short diagonal ticks around the crosshair (drawn as small squares).
        rects.clear();
        for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            for k in 0..3i16 {
                rects.push([320 + dx * (6 + k * 2) - 1, 240 + dy * (6 + k * 2) - 1, 2, 2]);
            }
        }
        gfx.hud_rects(rects, 0xffffffff);
    }
}

/// MW2's HUD minimap (`game_core::minimap`) in the `minimap_fullscreen` menu's place: the map turned with your view,
/// your arrow, enemies that fired, and the UAV's contacts.
fn draw_minimap(
    world: &World<'_>,
    arena: Option<&Arena>,
    radar: &game_core::minimap::Radar,
    cam: &Camera,
    now: u32,
) {
    use game_core::minimap::{FIRING_PING_MS, Frame, RADAR_PING_MS, ping_alpha};
    use wpk::ui::od;
    let (Some((m, handle)), Some(u)) = (world.minimap.as_ref(), ui::get()) else {
        return;
    };
    let Some(screen) = u.pack.screen("minimap") else {
        return;
    };
    let Some(rect) = screen
        .owner_draw(od::MINIMAP_MAP)
        .map(|o| [o.x, o.y, o.w, o.h])
    else {
        return;
    };
    let Some(f) = Frame::new(m, rect, cam.eye, cam.yaw) else {
        return;
    };
    let verts = f.map_quad(0xffff_ffff);
    unsafe { ffi::mwf_ui_verts(*handle, verts.as_ptr(), 1) };
    let enemy = u.pack.icon("compassping_enemy");
    let firing = u.pack.icon("compassping_enemyfiring").or(enemy);
    let me = u.pack.icon("compassping_player");
    u.page(|ui, q| {
        // MW2's own images over the map (the scan lines, the frame).
        if let Some(s) = ui.screen("minimap") {
            q.extend_from_slice(&s.quads);
        }
        let alpha = |a: f32| 0xffff_ff00 | (a.clamp(0.0, 1.0) * 255.0) as u32;
        if let Some(a) = arena {
            for (i, fi) in a.fighters.iter().enumerate().skip(1) {
                if !fi.life.alive() {
                    continue;
                }
                let radar_hit = radar.contacts.get(i).copied().flatten().and_then(|(p, t)| {
                    ping_alpha(now.wrapping_sub(t), RADAR_PING_MS).map(|al| (p, al))
                });
                if let (Some((p, al)), Some(t)) = (radar_hit, enemy) {
                    q.push(Frame::icon(f.to_screen(p), Frame::ENEMY_SIZE, t, alpha(al)));
                }
                let fired = fi
                    .last_fire_ms
                    .and_then(|t| ping_alpha(now.wrapping_sub(t), FIRING_PING_MS));
                if let (Some(al), Some(t)) = (fired, firing) {
                    q.push(Frame::icon(
                        f.to_screen(fi.player.ps.origin),
                        Frame::ENEMY_SIZE,
                        t,
                        alpha(al),
                    ));
                }
            }
        }
        if let Some(t) = me {
            q.push(f.player_icon(t));
        }
    });
}

/// End of match: everyone's score, best first (fewer deaths break ties), until A (or B).
fn scoreboard(gfx: &mut Shim, input: &mut front::Input, a: &Arena) {
    let order = a.standings();
    let title = if order.first() == Some(&0) {
        "YOU WIN"
    } else {
        "MATCH OVER"
    };
    // Free-for-all has no assists without teams.
    let rows: Vec<front::ScoreRow> = order
        .iter()
        .map(|&i| front::ScoreRow {
            name: alloc::string::String::from(fighter_name(i).as_str()),
            cols: [
                a.fighters[i].score as i32,
                a.fighters[i].kills as i32,
                0,
                a.fighters[i].life.deaths as i32,
            ],
            you: i == 0,
        })
        .collect();
    if front::scoreboard(gfx, input, title, &rows) {
        return;
    }
    let lines: Vec<front::Line> = order
        .iter()
        .map(|&i| {
            let mut l = front::Line::new(fighter_name(i).as_str());
            let mut v = Str::default();
            v.num(a.fighters[i].score);
            v.push_str(" POINTS  ");
            v.num(a.fighters[i].kills);
            v.push_str(" KILLS  ");
            v.num(a.fighters[i].life.deaths);
            v.push_str(" DEATHS");
            l.value = Some(alloc::string::String::from(v.as_str()));
            l
        })
        .collect();
    let mut sel = order.iter().position(|&i| i == 0).unwrap_or(0);
    front::list(gfx, input, title, &lines, &mut sel, "A CONTINUE");
}

struct Loadouts<'a> {
    catalog: Option<&'a wpk::classes::Catalog>,
    classes: &'a [wpk::classes::Loadout],
    /// The class chosen before the match (index into `classes`).
    class: Option<usize>,
    /// Cursor of the class list (kept between menus).
    cursor: &'a mut usize,
}

fn run_map(
    gfx: &mut Shim,
    name: &str,
    scratch: &mut WalkScratch,
    common: &[u8],
    loadouts: Loadouts<'_>,
    input: &mut front::Input,
) -> Exit {
    let weapons = weapon_section(common);
    // How each first-person weapon moves (sway, bob ...; empty with an older pack).
    let fpvs = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::WEAPON_FPV))
        .map_or(&[][..], |s| s.data);
    // MW2's match rules from the user's scripts (stock values with an older pack).
    let rules = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::RULES))
        .and_then(|s| wpk::rules::Rules::decode(s.data))
        .unwrap_or_default();
    let mut file = [0u8; 96];
    let n = name.len() + 4;
    if n > file.len() {
        return fail(0xc02020ff);
    }
    file[..name.len()].copy_from_slice(name.as_bytes());
    file[name.len()..n].copy_from_slice(b".wpk");
    let Some(path) = core::str::from_utf8(&file[..n])
        .ok()
        .and_then(|f| AppPath::new(&["maps/", f].concat()))
    else {
        return fail(0xc02020ff);
    };
    unsafe { ffi::mwf_tex_reset() };
    let load_start = ffi::ms();
    devlog::start(
        load_start,
        format_args!(
            "build={} map={name} bots={BOTS} class={} mem1_free={}k mem2_free={}k widescreen={}",
            option_env!("MWF_BUILD_ID").unwrap_or("dev"),
            loadouts
                .class
                .and_then(|c| loadouts.classes.get(c))
                .map_or("none", |l| l.name.as_str())
                .replace(' ', "_"),
            unsafe { ffi::mwf_mem1_free_bytes() } / 1024,
            unsafe { ffi::mwf_mem2_free_bytes() } / 1024,
            unsafe { ffi::mwf_widescreen() } != 0,
        ),
    );
    let cpath = path.as_ptr();
    // Header and section table first, to plan the load.
    let mut head = [0u8; wpk::HEADER_LEN + 64 * wpk::SECTION_ENTRY_LEN];
    let got = unsafe { ffi::mwf_file_read(cpath, 0, head.len() as u32, head.as_mut_ptr()) };
    if got < wpk::HEADER_LEN as i32 {
        return fail(file_error_colour());
    }
    let Ok(table) = wpk::Pack::parse_prefix(&head[..got as usize]) else {
        return fail(0xc020c0ff);
    };
    let file_len = table.header.file_len as usize;
    let in_place = table.in_place_len();
    // GPU data (textures, vertices, display lists) stays in MEM2 until the caller releases it.
    let ptr = unsafe { ffi::mwf_file_load_range(cpath, 0, in_place as u32) };
    if ptr.is_null() {
        return fail(file_error_colour());
    }
    let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr, in_place) };
    // The tail (visibility, cell tables, collision) is decoded into MEM1 from a temporary buffer.
    let (vis, clip_data, mut nav) = {
        let mut tail: Vec<u8> = Vec::new();
        if tail.try_reserve_exact(file_len - in_place).is_err() {
            return fail(0xe08020ff);
        }
        tail.resize(file_len - in_place, 0);
        let n = unsafe {
            ffi::mwf_file_read(cpath, in_place as u32, tail.len() as u32, tail.as_mut_ptr())
        };
        if n != tail.len() as i32 {
            return fail(file_error_colour());
        }
        let sec = |t: wpk::Tag| {
            let (off, len, _) = table.section_location(t)?;
            tail.get(off.checked_sub(in_place)?..off - in_place + len)
        };
        let vis = VisData::decode(
            sec(wpk::tag::VIS),
            sec(wpk::tag::BATCH_CELLS),
            sec(wpk::tag::INSTANCE_CELLS),
        );
        let clip = sec(wpk::tag::CLIP).and_then(|d| wpk::clip::Clip::decode(d).ok());
        let nav = sec(wpk::tag::NAV)
            .and_then(wpk::nav::NavGraph::decode)
            .map(game_core::nav::Navigator::new);
        (vis, clip, nav)
    };
    let mut world = match World::load(bytes, vis.as_ref(), gfx) {
        Ok(w) => w,
        Err(game_core::world::LoadError::TooManyTextures) => return fail(0xe08020ff),
        Err(_) => return fail(0xc020c0ff),
    };
    let clip: Option<MapClip<'_>> = clip_data.as_ref().map(MapClip::new);
    devlog::event(
        ffi::ms(),
        "loaded",
        format_args!(
            "ms={} mem1_free={}k mem2_free={}k collision={} nav_points={}",
            ffi::ms().wrapping_sub(load_start),
            unsafe { ffi::mwf_mem1_free_bytes() } / 1024,
            unsafe { ffi::mwf_mem2_free_bytes() } / 1024,
            clip.is_some(),
            nav.as_ref().map_or(0, |n| n.graph.nodes.len()),
        ),
    );
    let aspect = if unsafe { ffi::mwf_widescreen() } != 0 {
        16.0 / 9.0
    } else {
        4.0 / 3.0
    };
    let mut spawn = 0usize;
    let mut cam = spawn_camera(&world, spawn);
    cam.aspect = aspect;
    let mut player = clip.as_ref().map(|c| spawn_player(&world, c, spawn));
    // The class's weapons (when common.wpk is on the SD card), first-person files loaded above the map's data.
    let common_pack = wpk::Pack::parse(common, false).ok();
    let kicks = common_pack
        .as_ref()
        .and_then(|p| p.section(wpk::tag::KICK))
        .map_or(&[][..], |s| s.data);
    let moves = common_pack
        .as_ref()
        .and_then(|p| p.section(wpk::tag::WEAPON_MOVE))
        .map_or(&[][..], |s| s.data);
    let aims = common_pack
        .as_ref()
        .and_then(|p| p.section(wpk::tag::WEAPON_AIM))
        .map_or(&[][..], |s| s.data);
    // The map load reset every texture: the menu textures again (the pause menu uses them), below the arsenal's mark.
    if let Some(u) = ui::get() {
        u.create_textures(gfx);
    }
    let mut arsenal = arsenal::Arsenal::new();
    // A class change applies at the next spawn, as in MW2.
    let mut class = loadouts.class;
    // Arm for a spawn; returns the class's perks (with their Pro perks: everything is unlocked).
    let equip = |arsenal: &mut arsenal::Arsenal, gfx: &mut Shim, class: Option<usize>| {
        let l = class.and_then(|c| loadouts.classes.get(c));
        arsenal.equip(
            gfx,
            &arsenal::WeaponTables {
                weapons,
                kicks,
                moves,
                aims,
            },
            loadouts.catalog,
            l,
        );
        match (loadouts.catalog, l) {
            (Some(c), Some(l)) => game_core::perks::of_loadout(c, l),
            _ => game_core::perks::PerkSet::default(),
        }
    };
    let mut perk_set = equip(&mut arsenal, gfx, class);
    // Free-for-all against bots (MW2's FFA spawns when the map has them).
    let spawn_list: Vec<([f32; 3], f32)> = {
        let dm: Vec<_> = world
            .spawns
            .iter()
            .filter(|s| s.kind == wpk::records::spawn_kind::DM)
            .map(|s| (s.origin, s.yaw))
            .collect();
        if dm.is_empty() {
            world.spawns.iter().map(|s| (s.origin, s.yaw)).collect()
        } else {
            dm
        }
    };
    // Bot i carries the primary of MW2's default class i (Grenadier, First Recon ...), else the fallback.
    let bot_gun = |i: usize| -> Option<Gun> {
        let names = loadouts
            .catalog
            .and_then(|c| {
                let l = c
                    .defaults
                    .get((i + c.defaults.len().max(1) - 1) % c.defaults.len().max(1))?;
                let mut n = c.weapon_names(l.primary, l.primary_attachments);
                n.push(alloc::format!(
                    "{}_mp",
                    c.weapons.get(usize::from(l.primary))?.name
                ));
                Some(n)
            })
            .unwrap_or_else(|| alloc::vec![alloc::string::String::from("m4_mp")]);
        names.iter().find_map(|n| {
            let (index, rec) = wpk::weapon::find_indexed(weapons, n)?;
            let mut g = Gun::with_index(&rec, index)?;
            g.set_moves(wpk::records::get(moves, index as usize));
            g.set_aim(wpk::records::get(aims, index as usize));
            Some(g)
        })
    };
    let mut arena = match (&clip, &player) {
        (Some(c), Some(p)) if !spawn_list.is_empty() => {
            Some(Arena::new(c, &spawn_list, BOTS, p.clone(), rules, |i| {
                if i == 0 { None } else { bot_gun(i) }
            }))
        }
        _ => None,
    };
    // Bots call in a UAV at its killstreakTable cost.
    if let (Some(a), Some(c)) = (arena.as_mut(), loadouts.catalog) {
        a.bot_uav_kills = c
            .killstreaks
            .iter()
            .find(|k| k.name == "uav")
            .map_or(0, |k| u32::from(k.kind));
    }
    let mut events: Vec<Event> = Vec::with_capacity(32);
    let mut feed: [(Option<(usize, usize)>, u32); 4] = [(None, 0); 4];
    let mut hitmarker_ms: Option<u32> = None;
    let mut boxes: Vec<[f32; 6]> = Vec::with_capacity(BOTS * 10);
    let mut box_colours: Vec<u32> = Vec::with_capacity(BOTS * 10);
    let mut impacts: [(Option<[f32; 3]>, u32); 8] = [(None, 0); 8];
    let mut last_shot_ms: Option<u32> = None;
    let mut next_impact = 0usize;
    let mut hud_rects: Vec<Rect> = Vec::with_capacity(512);
    let mut life = Life::with_rules(&rules);
    let clock = MatchClock::with_limit(ffi::ms(), rules.time_limit_ms);
    // Below the lowest spawn point by this much is out of the world.
    let floor_z = world
        .spawns
        .iter()
        .map(|s| s.origin[2])
        .fold(f32::MAX, f32::min)
        - life::OUT_OF_WORLD_DEPTH;
    let mut flying = player.is_none();
    let mut last = ffi::ms();
    let mut show_overlay = true;
    // Killstreaks: kills since your last death, the rewards earned and not yet used (newest last), the UAV's radar.
    let mut streak = 0u32;
    let mut rewards: Vec<alloc::string::String> = Vec::new();
    let mut reward_note: Option<(&'static str, u32)> = None;
    let mut radar = game_core::minimap::Radar::default();
    let mut radar_at = last;
    let mut timing = Timing::default();
    let mut rects: Vec<Rect> = Vec::with_capacity(4096);
    let mut frame_start = ffi::us();
    let mut last_mem_s = u32::MAX;
    // Gamepad look (IW4L): the sticks' learned range, the look's state between steps, settings.
    let mut sticks = look::StickRange::default();
    let mut look_state = look::LookState::default();
    let look_settings = look::LookSettings::default();
    let mut aim_targets: Vec<look::AimTarget> = Vec::with_capacity(BOTS);
    // The current field of view over the hip one (look speed and aim assist scale with the zoom).
    let mut hip_fov_scale = 1.0f32;
    // Fixed-step clock for the player: milliseconds × 3 not yet simulated, and steps taken.
    let mut tick_acc3 = 0u32;
    // Largest stick and trigger values seen (calibration: IW4 needs 105 of 127 forward to sprint).
    let mut stick_max = [0i8; 4];
    let mut trigger_max = [0u8; 2];
    let stick_event = |stick_max: &[i8; 4], trigger_max: &[u8; 2], now: u32| {
        devlog::event(
            now,
            "stick",
            format_args!(
                "sx={} sy={} cx={} cy={} l={} r={}",
                stick_max[0],
                stick_max[1],
                stick_max[2],
                stick_max[3],
                trigger_max[0],
                trigger_max[1]
            ),
        );
    };
    let mut sim_ticks = 0u64;
    let lap = |mark: &mut u32| {
        let t = ffi::us();
        let d = t.wrapping_sub(*mark);
        *mark = t;
        d
    };
    loop {
        let mut sections = [0u32; section::COUNT];
        let mut mark = ffi::us();
        let now = ffi::ms();
        let msec = now.wrapping_sub(last).clamp(1, 100);
        last = now;
        let dt = msec as f32 / 1000.0;
        let pad = ffi::pad0();
        if pad.down & ffi::BTN_DOWN != 0 {
            show_overlay = !show_overlay;
        }
        if ffi::should_exit() {
            return Exit::Quit;
        }
        if pad.down & ffi::BTN_START != 0 {
            unsafe { ffi::mwf_snd_stop_all() };
            devlog::event(now, "pause", format_args!(""));
            stick_event(&stick_max, &trigger_max, now);
            devlog::flush(now);
            let mode = if rules.score_limit > 0 {
                alloc::format!("FREE-FOR-ALL: FIRST TO {} POINTS", rules.score_limit)
            } else {
                alloc::string::String::from("FREE-FOR-ALL")
            };
            match front::pause(
                gfx,
                input,
                name,
                &mode,
                loadouts.catalog,
                (!loadouts.classes.is_empty()).then_some(loadouts.classes),
                loadouts.cursor,
            ) {
                front::Pause::EndGame => {
                    devlog::event(ffi::ms(), "end_game", format_args!(""));
                    devlog::end(ffi::ms());
                    return Exit::Menu;
                }
                front::Pause::ChangeClass(c) => {
                    devlog::event(now, "class_change", format_args!("class={c}"));
                    class = Some(c);
                }
                front::Pause::Resume => {}
            }
            // The match was stopped: no time passed.
            last = ffi::ms();
            frame_start = ffi::us();
            continue;
        }
        // Y: the other weapon (MW2's console layout).
        if pad.down & ffi::BTN_Y != 0 && !flying && life.alive() && arsenal.switch() {
            devlog::event(
                now,
                "switch_request",
                format_args!("from={}", arsenal.active),
            );
        }
        // D-pad left: call in the newest killstreak reward (MW2's console uses the D-pad for it too).
        if pad.down & ffi::BTN_LEFT != 0
            && life.alive()
            && let Some(r) = rewards.pop()
        {
            if r == "uav" {
                radar.activate(now, rules.uav_time_ms);
                reward_note = Some(("UAV ONLINE", now));
            } else {
                reward_note = Some(("THAT KILLSTREAK IS NOT IN THIS BUILD YET", now));
            }
            devlog::event(now, "killstreak", format_args!("used={r}"));
        }
        if pad.down & ffi::BTN_UP != 0 && clip.is_some() {
            flying = !flying;
            if !flying {
                // Drop the player where the camera is, facing the same way.
                if let Some(c) = &clip {
                    let mut p =
                        Player::spawn(c, [cam.eye[0], cam.eye[1], cam.eye[2] - 60.0], cam.yaw);
                    p.ps.viewangles[0] = cam.pitch;
                    player = Some(p);
                }
            }
        }
        for (m, v) in stick_max.iter_mut().zip([pad.sx, pad.sy, pad.cx, pad.cy]) {
            *m = (*m).max(v.saturating_abs());
        }
        trigger_max[0] = trigger_max[0].max(pad.l);
        trigger_max[1] = trigger_max[1].max(pad.r);
        // Player steps due this frame (60 Hz, at most 6 = 100 ms).
        tick_acc3 += msec * 3;
        let player_ticks = (tick_acc3 / 50).min(6);
        tick_acc3 -= player_ticks * 50;
        tick_acc3 = tick_acc3.min(50);
        // Sticks: learned range, IW4L's radial dead zone; the main stick as IW4 move amounts.
        let main_stick = sticks.normalize(0, pad.sx, pad.sy);
        let c_stick = sticks.normalize(1, pad.cx, pad.cy);
        let (forward, right) = look::to_move(main_stick);
        let ads_held = pad.held & ffi::BTN_L != 0 || pad.l > 128;
        // Aim-assist targets: living bots in clear view of your eye (their hull box and chest).
        aim_targets.clear();
        if let (Some(a), Some(p), Some(c)) = (arena.as_ref(), player.as_ref(), clip.as_ref())
            && !flying
            && life.alive()
        {
            let eye = p.eye();
            for (i, f) in a.fighters.iter().enumerate().skip(1) {
                if !f.life.alive() {
                    continue;
                }
                let (feet, chest) = arena::feet_and_aim(&f.player);
                let height = if f.player.crouched() { 50.0 } else { 70.0 };
                let t = c.trace(eye, chest, [0.0; 3], [0.0; 3], game_core::gun::MASK_SHOT);
                if t.fraction < 1.0 {
                    continue;
                }
                aim_targets.push(look::AimTarget {
                    key: i as u64,
                    mins: [feet[0] - 15.0, feet[1] - 15.0, feet[2]],
                    maxs: [feet[0] + 15.0, feet[1] + 15.0, feet[2] + height],
                    aim: chest,
                    velocity: f.player.ps.velocity,
                });
            }
        }
        sections[section::INPUT] = lap(&mut mark);
        match (&mut player, &clip) {
            (Some(p), Some(c)) if !flying && life.alive() => {
                // Fixed 60 Hz for the player, like IW4 at 60 fps (usercmds of 16 / 17 ms): the fire rate and
                // movement don't depend on the frame rate. A slow frame runs several steps.
                for _ in 0..player_ticks {
                    if !life.alive() {
                        break;
                    }
                    let tick_ms = ((sim_ticks + 1) * 50 / 3 - sim_ticks * 50 / 3) as i32;
                    sim_ticks += 1;
                    let weapon_move = arsenal.gun().map(Gun::move_context).unwrap_or_default();
                    let (pitch, yaw) = p.view();
                    let view = look::AimView {
                        eye: p.eye(),
                        angles: [pitch, yaw],
                        velocity: p.ps.velocity,
                        ads_lerp: p.ps.f_weapon_pos_frac,
                        fov_scale: hip_fov_scale,
                        ranges: arsenal
                            .gun()
                            .map_or(look::AimAssistRanges::NONE, Gun::aim_ranges),
                        dt: tick_ms as f32 / 1000.0,
                    };
                    let look_delta = look::look_frame(
                        &mut look_state,
                        &look_settings,
                        c_stick,
                        &view,
                        &aim_targets,
                        ads_held,
                    );
                    // The class's perks and move speed (MW2: the primary's class, Lightweight), every step since a
                    // spawn resets the player state.
                    p.ps.perks = perk_set.bits;
                    p.ps.move_speed_scale_multiplier = {
                        let class = arsenal.guns[0].as_ref().and_then(|g| {
                            game_core::perks::speed_class(
                                g.facts().weap_class,
                                g.name().starts_with("riotshield"),
                            )
                        });
                        rules.move_speed(class, perk_set.lightweight)
                    };
                    if let Some(a) = arena.as_mut() {
                        a.fighters[0].bullet_scale = perk_set.bullet_scale();
                    }
                    // Other players block you (IW4 player clipping).
                    match arena.as_ref() {
                        Some(a) => c.set_bodies(a.bodies_except(0)),
                        None => c.set_bodies(core::iter::empty()),
                    }
                    let fall = p.update(
                        c,
                        &MovePad {
                            forward,
                            right,
                            look: look_delta,
                            jump: pad.held & ffi::BTN_A != 0,
                            crouch: pad.held & ffi::BTN_B != 0,
                            sprint: pad.held & ffi::BTN_Z != 0,
                            ads: ads_held,
                            fire: pad.held & ffi::BTN_R != 0 || pad.r > 100,
                            reload: pad.held & ffi::BTN_X != 0,
                        },
                        &weapon_move,
                        tick_ms,
                    );
                    cam = p.camera(aspect);
                    // ADS zoom (IW4L `calc_fov_from_ads` with the weapon's zoom FOV and window).
                    let fov = arsenal.gun().map_or(look::HIP_FOV, |g| {
                        look::view_fov(p.ps.f_weapon_pos_frac, ads_held, g.aim(), true)
                    });
                    hip_fov_scale = fov / look::HIP_FOV;
                    cam.fovy = look::vertical_fov(fov);
                    // Recoil kicks the view (IW4: kick angles on top of the view; shots follow the kicked view).
                    if let Some(g) = arsenal.gun() {
                        cam.pitch += g.kick_angles[0];
                        cam.yaw += g.kick_angles[1];
                    }
                    if fall > 0 {
                        life.damage(fall, now);
                    }
                    if p.ps.origin[2] < floor_z {
                        life.damage(life.max_health, now);
                    }
                    if let Some(a) = arena.as_mut() {
                        a.fighters[0].player = p.clone();
                        a.fighters[0].life = life;
                    }
                    sections[section::PLAYER] += lap(&mut mark);
                    let a = arsenal.active;
                    let switch_to = arsenal.switch_target();
                    if let (Some(g), true) = (arsenal.guns[a].as_mut(), life.alive()) {
                        g.update_spread(
                            tick_ms,
                            &SpreadInput {
                                ps: &p.ps,
                                cmd_angles: p.cmd_angles,
                                old_cmd_angles: p.old_cmd_angles,
                                forwardmove: p.cmd_move.0,
                                rightmove: p.cmd_move.1,
                            },
                        );
                        let (event, shot) = g.update(
                            c,
                            tick_ms,
                            &GunInput {
                                fire: pad.held & ffi::BTN_R != 0 || pad.r > 100,
                                reload: pad.held & ffi::BTN_X != 0,
                                // D-pad right: melee (MW2's console melee is a stick click; the pad has none).
                                melee: pad.held & ffi::BTN_RIGHT != 0,
                                pm_flags: p.ps.pm_flags,
                                perks: p.ps.perks,
                                switch_to,
                            },
                            cam.eye,
                            cam.forward(),
                            cam.right(),
                            cam.up(),
                        );
                        // The weapon's state goes into the player's (pmove reads it next step).
                        g.sync_player(&mut p.ps);
                        if event == Some(game_core::gun::WeaponTickEvent::MeleeFired)
                            && let Some(a) = arena.as_mut()
                        {
                            let hit = a.melee(
                                c,
                                0,
                                cam.eye,
                                [cam.forward(), cam.right(), cam.up()],
                                g.facts().melee_damage,
                                now,
                                &mut events,
                            );
                            devlog::event(now, "melee", format_args!("hit={}", u8::from(hit)));
                        }
                        if let Some(s) = shot.as_ref() {
                            last_shot_ms = Some(now);
                            if let Some(a) = arena.as_mut() {
                                a.fighters[0].last_fire_ms = Some(now);
                                let facts = *g.facts();
                                a.bullet(0, s, &facts, now, &mut events);
                                for pellet in g.extra_pellets() {
                                    a.bullet(0, pellet, &facts, now, &mut events);
                                }
                            }
                        }
                        // A raise that ends a switch is the incoming weapon's (`finish_switch` plays it).
                        let raise_of_other = switch_to.is_some()
                            && event == Some(game_core::gun::WeaponTickEvent::RaiseStarted);
                        if g.swung
                            && let Some(v) = arsenal.views[a].as_mut()
                            && let Some(&s) = v
                                .sounds
                                .events
                                .get(wpk::sound::event::MELEE_SWIPE)
                                .filter(|&&s| s != wpk::sound::NONE)
                        {
                            v.play(s);
                        }
                        if let (Some(e), Some(v), false) =
                            (event.as_ref(), arsenal.views[a].as_mut(), raise_of_other)
                            && let Some(s) =
                                game_core::weapon_sound::for_event(&v.sounds, e, g.ammo().0)
                        {
                            v.play(s);
                        }
                        if let Some(s) = shot.filter(|s| s.hit) {
                            impacts[next_impact] = (Some(s.end), now);
                            next_impact = (next_impact + 1) % impacts.len();
                        }
                    }
                    // A finished drop hands over to the other weapon, which raises.
                    let before = arsenal.active;
                    arsenal.finish_switch();
                    if arsenal.active != before {
                        devlog::event(now, "switch", format_args!("to={}", arsenal.active));
                    }
                    sections[section::GUN] += lap(&mut mark);
                }
            }
            (Some(_), Some(c)) if !flying => {
                // Dead: the view stays where the player fell until the respawn.
                if life.ready_to_respawn(now) {
                    spawn += 1;
                    // MW2's spawn weighting, as for the bots.
                    player = Some(match arena.as_mut() {
                        Some(a) if !spawn_list.is_empty() => {
                            let (o, yaw) = a.spawn_point(c, &spawn_list, 0);
                            Player::spawn(c, o, yaw)
                        }
                        _ => spawn_player(&world, c, spawn),
                    });
                    perk_set = equip(&mut arsenal, gfx, class);
                    life.respawn();
                }
            }
            _ => fly::update(
                &mut cam,
                &PadInput {
                    sx: pad.sx,
                    sy: pad.sy,
                    cx: pad.cx,
                    cy: pad.cy,
                    l: pad.l,
                    r: pad.r,
                    fast: pad.held & ffi::BTN_B != 0,
                },
                dt,
            ),
        }
        let _ = lap(&mut mark);
        if let (Some(a), Some(c)) = (arena.as_mut(), clip.as_ref()) {
            a.human_present = !flying && player.is_some();
            a.fighters[0].life = life;
            a.update_bots(
                c,
                nav.as_mut(),
                &spawn_list,
                now,
                msec as i32,
                bot_gun,
                &mut events,
            );
            a.regen(now);
            life = a.fighters[0].life;
            // Hits slow you (IW4 damage timer, set on the arena's copy of you).
            if let Some(p) = player.as_mut() {
                let ps = &a.fighters[0].player.ps;
                p.ps.damage_timer = ps.damage_timer;
                p.ps.damage_duration = ps.damage_duration;
                p.ps.flinch_yaw_anim = ps.flinch_yaw_anim;
            }
        }
        for e in events.drain(..) {
            match e {
                Event::Hit {
                    attacker,
                    victim,
                    damage,
                    location,
                } => {
                    if attacker == 0 {
                        hitmarker_ms = Some(now);
                    }
                    if attacker == 0 || victim == 0 {
                        devlog::event(
                            now,
                            "hit",
                            format_args!("by={attacker} on={victim} dmg={damage} loc={location}"),
                        );
                    }
                }
                Event::Kill { attacker, victim } => {
                    feed.rotate_right(1);
                    feed[0] = (Some((attacker, victim)), now);
                    devlog::event(now, "kill", format_args!("by={attacker} on={victim}"));
                    if victim == 0 {
                        // MW2: a death resets the streak; rewards already earned are kept.
                        streak = 0;
                    } else if attacker == 0 {
                        streak += 1;
                        // The class's killstreaks (their costs from killstreakTable) reached by this kill.
                        if let (Some(c), Some(l)) = (
                            loadouts.catalog,
                            class.and_then(|i| loadouts.classes.get(i)),
                        ) {
                            for &k in &l.killstreaks {
                                if let Some(item) = c.killstreaks.get(usize::from(k))
                                    && u32::from(item.kind) == streak
                                {
                                    rewards.push(item.name.clone());
                                    reward_note = Some(("KILLSTREAK READY: D-PAD LEFT", now));
                                    devlog::event(
                                        now,
                                        "killstreak",
                                        format_args!("earned={}", item.name),
                                    );
                                }
                            }
                        }
                    }
                    if victim == 0 {
                        stick_event(&stick_max, &trigger_max, now);
                        // A natural break: write the log while you wait to respawn.
                        devlog::flush(now);
                    }
                }
                _ => {}
            }
        }
        sections[section::BOTS] = lap(&mut mark);
        let stats = world.draw(gfx, &cam, scratch);
        sections[section::WORLD] = lap(&mut mark);
        // Bots: placeholder bodies, one shaded box per hit volume, until the character models are converted.
        if let Some(a) = arena.as_ref() {
            boxes.clear();
            box_colours.clear();
            for f in a.fighters[1..].iter().filter(|f| f.life.alive()) {
                for h in game_core::damage::boxes(f.draw_origin(now), f.player.crouched()) {
                    boxes.push([
                        h.mins[0], h.mins[1], h.mins[2], h.maxs[0], h.maxs[1], h.maxs[2],
                    ]);
                    box_colours.push(if h.loc <= game_core::damage::hitloc::NECK {
                        0xd8b090ff
                    } else {
                        0x6a7050ff
                    });
                }
            }
            unsafe {
                ffi::mwf_draw_boxes(
                    &cam.view(),
                    boxes.as_ptr(),
                    box_colours.as_ptr(),
                    boxes.len() as u32,
                )
            };
        }
        sections[section::BODIES] = lap(&mut mark);
        if !flying
            && life.alive()
            && let (Some(v), Some(g), Some(p)) = (
                arsenal.views[arsenal.active].as_mut(),
                arsenal.guns[arsenal.active].as_ref(),
                player.as_ref(),
            )
        {
            let fpv = wpk::records::get::<wpk::weapon::FpvRecord>(fpvs, g.weapon_index() as usize)
                .unwrap_or_default();
            v.draw(gfx, g, &p.ps, &fpv, dt, now);
            v.play_notes();
            // Muzzle flash for 50 ms after a shot: a placeholder additive-looking glow at the gun's tag_flash until
            // MW2's view flash effect is converted.
            if last_shot_ms.is_some_and(|t| now.wrapping_sub(t) < 50)
                && let Some([x, y]) = v.muzzle_screen(aspect)
            {
                let (x, y) = (x as i16, y as i16);
                gfx.hud_rects(
                    &[[x - 22, y - 8, 44, 16], [x - 8, y - 22, 16, 44]],
                    0xffb0_4070,
                );
                gfx.hud_rects(&[[x - 8, y - 8, 16, 16]], 0xffff_e0d0);
            }
        }
        sections[section::VIEW] = lap(&mut mark);
        if !flying {
            if let Some(g) = arsenal.gun().filter(|_| life.alive()) {
                draw_gun_hud(gfx, &mut hud_rects, g, &cam, &impacts, now);
            }
            draw_life_hud(gfx, &mut hud_rects, &life, &clock, now);
            if let Some(a) = arena.as_ref() {
                draw_match_hud(gfx, &mut hud_rects, a, &feed, hitmarker_ms, now);
            }
            // The UAV's sweep, then the minimap.
            if let (Some((m, _)), Some(a)) = (world.minimap.as_ref(), arena.as_ref()) {
                let enemies: Vec<(usize, [f32; 3])> = a
                    .fighters
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter(|(_, f)| f.life.alive())
                    .map(|(i, f)| (i, f.player.ps.origin))
                    .collect();
                radar.sweep(m, radar_at, now, &enemies);
            }
            radar_at = now;
            draw_minimap(&world, arena.as_ref(), &radar, &cam, now);
            if let Some((text, at)) = reward_note.filter(|(_, at)| now.wrapping_sub(*at) < 3000) {
                let _ = at;
                hud_rects.clear();
                hud::text(&mut hud_rects, 200, 360, 2, text);
                gfx.hud_rects(&hud_rects, 0xffd040e0);
            }
            if let Some(r) = rewards.last() {
                let mut line = Str::default();
                line.push_str("D-PAD LEFT: ");
                line.push_str(&r.to_ascii_uppercase());
                hud_rects.clear();
                hud::text(&mut hud_rects, 430, 430, 2, line.as_str());
                gfx.hud_rects(&hud_rects, 0xf0e8d0e0);
            }
        }
        if let Some(a) = arena.as_ref()
            && (a.won() || clock.over(now))
        {
            unsafe { ffi::mwf_snd_stop_all() };
            stick_event(&stick_max, &trigger_max, now);
            devlog::event(
                now,
                "match_end",
                format_args!(
                    "kills={} score={} deaths={} leader={} leader_score={}",
                    a.fighters[0].kills,
                    a.fighters[0].score,
                    life.deaths,
                    a.leader().0,
                    a.leader().1
                ),
            );
            devlog::end(now);
            scoreboard(gfx, input, a);
            return Exit::Menu;
        }
        if show_overlay {
            overlay(gfx, &mut rects, &timing, &stats, cam.eye);
        }
        sections[section::HUD] = lap(&mut mark);
        let cpu_us = ffi::us().wrapping_sub(frame_start);
        gfx.frame_end();
        let end = ffi::us();
        timing.add(end, end.wrapping_sub(frame_start), cpu_us, gfx.gpu_wait_us);
        // Memory every 5 s is enough (mallinfo walks the heap).
        let (mem1_free, mem2_free) = if now / 5000 != last_mem_s {
            last_mem_s = now / 5000;
            unsafe { (ffi::mwf_mem1_free_bytes(), ffi::mwf_mem2_free_bytes()) }
        } else {
            (0, 0)
        };
        devlog::frame(
            ffi::ms(),
            &telemetry::Frame {
                frame_us: end.wrapping_sub(frame_start),
                cpu_us,
                gpu_us: gfx.gpu_wait_us,
                sections,
                triangles: stats.triangles,
                draws: stats.batches,
                mem1_free,
                mem2_free,
            },
        );
        frame_start = end;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mwf_game_main() -> i32 {
    unsafe { ffi::mwf_gx_init(0x202020ff) };
    unsafe { ffi::mwf_snd_init() };
    let mut gfx = Shim::default();
    // The portal walk's scratch (~0.2 MiB) is static, never on the stack.
    let scratch: &'static mut WalkScratch = unsafe { &mut *core::ptr::addr_of_mut!(WALK_SCRATCH) };
    // Weapons, classes and other shared data (optional): <app folder>/common.wpk, in MEM2 for the whole run.
    let common = load_common();
    // MW2's menu fonts and screens (from common.wpk), when the pack has them.
    ui::init(common, &mut gfx);
    let catalog = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::CLASSES))
        .and_then(|s| wpk::classes::Catalog::decode(s.data).ok());
    let mut custom = catalog
        .as_ref()
        .map(front::load_classes)
        .unwrap_or_default();
    let mut input = front::Input::default();
    let (mut main_sel, mut map_sel, mut class_sel, mut edit_sel) = (0usize, 0usize, 0usize, 0usize);
    loop {
        match front::main_menu(&mut gfx, &mut input, &mut main_sel, catalog.is_some()) {
            front::Main::Quit => return 0,
            front::Main::CreateAClass => {
                let Some(cat) = catalog.as_ref() else {
                    continue;
                };
                while let Some(i) = front::pick_class(
                    &mut gfx,
                    &mut input,
                    Some(cat),
                    "CREATE A CLASS",
                    &custom,
                    &mut edit_sel,
                ) {
                    front::edit_class(&mut gfx, &mut input, cat, &mut custom[i]);
                    front::save_classes(cat, &custom);
                }
            }
            front::Main::Play => {
                let mut names = [0u8; 2048];
                let maps = list_maps(&mut names);
                while let Some(i) = front::pick_map(&mut gfx, &mut input, &maps, &mut map_sel) {
                    // Custom classes first, then MW2's defaults.
                    let classes: Vec<wpk::classes::Loadout> = catalog
                        .as_ref()
                        .map(|c| custom.iter().chain(&c.defaults).cloned().collect())
                        .unwrap_or_default();
                    let class = if classes.is_empty() {
                        None
                    } else {
                        match front::pick_class(
                            &mut gfx,
                            &mut input,
                            catalog.as_ref(),
                            "CHOOSE CLASS",
                            &classes,
                            &mut class_sel,
                        ) {
                            Some(c) => Some(c),
                            None => continue,
                        }
                    };
                    let mark = unsafe { ffi::mwf_mem2_mark() };
                    let exit = run_map(
                        &mut gfx,
                        maps[i],
                        scratch,
                        common,
                        Loadouts {
                            catalog: catalog.as_ref(),
                            classes: &classes,
                            class,
                            cursor: &mut class_sel,
                        },
                        &mut input,
                    );
                    unsafe { ffi::mwf_snd_stop_all() };
                    unsafe { ffi::mwf_mem2_release(mark) };
                    if let Exit::Quit = exit {
                        return 0;
                    }
                    break;
                }
            }
        }
    }
}

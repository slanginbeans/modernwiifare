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
mod autoplay;
mod character;
mod devlog;
mod effects;
mod ffi;
mod front;
mod objects;
mod rt;
mod sound;
mod ui;
mod view;

use alloc::vec::Vec;

use dpvs_iw4::WalkScratch;
use game_core::arena::{self, Arena, Event};
use game_core::camera::Camera;
use game_core::collision::MapClip;
use game_core::controls::{self, act};
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
    fn fog(&mut self, fog: Option<(f32, f32, u32)>) {
        let (s, e, c) = fog.unwrap_or((0.0, 1.0, 0));
        unsafe { ffi::mwf_fog(i32::from(fog.is_some()), s, e, c) }
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
    fn world_tint(&mut self, s: &DrawState) {
        unsafe { ffi::mwf_world_tint(s.tint) }
    }
    fn pos_matrix(&mut self, m: &Mtx34) {
        unsafe { ffi::mwf_pos_matrix(m) }
    }
    fn world_desc(&mut self, flags: u8) {
        unsafe { ffi::mwf_world_desc(flags) }
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
    fn sprites(
        &mut self,
        view: &Mtx34,
        tex: i32,
        blend: u8,
        verts: &[game_core::gfx::SpriteVert],
        world: bool,
    ) {
        unsafe {
            ffi::mwf_sprites(
                view,
                tex,
                u32::from(blend),
                verts.as_ptr(),
                (verts.len() / 4) as u32,
                i32::from(world),
            )
        }
    }
    fn frame_end(&mut self) {
        self.gpu_wait_us = unsafe { ffi::mwf_frame_end() };
        autoplay::after_frame();
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
    line.push_str("+");
    line.num(s.tint_changes);
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
        let (pad, x) = ffi::pads();
        if ffi::should_exit() {
            return Exit::Quit;
        }
        if pad.down & ffi::BTN_START != 0
            || (x.connected && x.buttons & game_core::controls::x360::START != 0)
        {
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
fn draw_life_hud(
    gfx: &mut Shim,
    rects: &mut Vec<Rect>,
    life: &Life,
    clock: &MatchClock,
    now: u32,
    text: bool,
) {
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
    }
    if !text {
        return;
    }
    if !life.alive() {
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
    text: bool,
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
    if !text {
        return;
    }
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
    // Grenades left (D-pad up throws).
    let grenades: i32 = g.offhand_ammo().map(|(_, n)| n.max(0)).sum();
    if g.offhand_ammo().next().is_some() {
        let mut line = Str::default();
        line.push_str("GRENADE ");
        line.num(grenades as u32);
        rects.clear();
        hud::text(rects, 520, 400, 2, line.as_str());
        gfx.hud_rects(rects, if grenades > 0 { 0xc0c8d0c0 } else { 0x606870c0 });
    }
}

/// Microseconds (wrapping), for timing inside `game_core`.
fn clock_us() -> u32 {
    unsafe { ffi::mwf_us() }
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

/// The largest pack `load_pack` takes (a sanity check on the header's length; `common.wpk` is about 17 MiB with
/// the menus, sounds and weapons, and stays in MEM2 for the whole run).
const PACK_MAX: usize = 24 << 20;

/// A whole pack file into MEM2 (flushed; GX and the DSP read it in place), checked; None when missing or bad.
pub(crate) fn load_pack(path: &AppPath) -> Option<&'static [u8]> {
    let mut head = [0u8; wpk::HEADER_LEN];
    let got = unsafe { ffi::mwf_file_read(path.as_ptr(), 0, head.len() as u32, head.as_mut_ptr()) };
    if got != head.len() as i32 {
        return None;
    }
    let len = u32::from_be_bytes([head[24], head[25], head[26], head[27]]) as usize;
    if len > PACK_MAX {
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
    // A nuke's countdown (MW2 shows it as the game timer).
    if let Some(n) = a.nuke.filter(|n| !n.done) {
        line.push_str("NUKE ");
        line.num(n.detonate_ms.saturating_sub(now).div_ceil(1000));
        line.push_str("  ");
    }
    if a.mode.teams() {
        // Your team first, as MW2's team score bar.
        let mine = usize::from(a.fighters[0].team);
        let theirs = 3 - mine;
        line.push_str("YOUR TEAM ");
        line.num(a.team_score[mine]);
        line.push_str("  ENEMY ");
        line.num(a.team_score[theirs]);
        // Domination: each flag's holder (yours, the enemy's, nobody's) and the one you are taking.
        if let Some(d) = a.dom.as_ref() {
            for f in &d.flags {
                line.push_str("  ");
                let mut l = [0u8; 4];
                line.push_str(f.letter().encode_utf8(&mut l));
                line.push_str(match f.owner {
                    0 => ":--",
                    t if usize::from(t) == mine => ":US",
                    _ => ":EN",
                });
            }
            let you = a.fighters[0].player.ps.origin;
            if let Some(f) = d
                .flags
                .iter()
                .find(|f| usize::from(f.claim) == mine && f.contains(you))
            {
                line.push_str("  SECURING ");
                let mut l = [0u8; 4];
                line.push_str(f.letter().encode_utf8(&mut l));
                line.push_str(" ");
                line.num((f.progress_ms * 100.0 / a.rules.dom_use_ms.max(1) as f32) as u32);
                line.push_str("%");
            }
        }
    } else {
        line.push_str("SCORE ");
        line.num(a.fighters[0].score);
        line.push_str("  LEADER ");
        let (who, k) = a.leader();
        line.push_str(fighter_name(who).as_str());
        line.push(' ');
        line.num(k);
    }
    if a.limits.score_limit > 0 {
        line.push_str(" / ");
        line.num(a.limits.score_limit);
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
    blasts: &[(Option<[f32; 3]>, u32)],
    now: u32,
    safe_area: f32,
) {
    use game_core::minimap::{FIRING_PING_MS, Frame, RADAR_PING_MS, ping_alpha};
    use wpk::ui::od;
    let (Some((m, handle)), Some(u)) = (world.minimap.as_ref(), ui::get()) else {
        return;
    };
    let Some(screen) = u.pack.screen("minimap") else {
        return;
    };
    // MW2's minimap sits top left in the safe area (baked at the full screen's corner): moved in by the margin.
    let (ox, oy) = (320.0 * (1.0 - safe_area), 240.0 * (1.0 - safe_area));
    let Some(rect) = screen
        .owner_draw(od::MINIMAP_MAP)
        .map(|o| [o.x + ox, o.y + oy, o.w, o.h])
    else {
        return;
    };
    let Some(f) = Frame::new(m, rect, cam.eye, cam.yaw) else {
        return;
    };
    // An enemy Counter-UAV jams your radar: a dimmed map and no pings but you.
    let jammed = arena.is_some_and(|a| a.radar_blocked(0, now));
    let verts = f.map_quad(if jammed { 0x5050_50ff } else { 0xffff_ffff });
    unsafe { ffi::mwf_ui_verts(*handle, verts.as_ptr(), 1) };
    let enemy = u.pack.icon("compassping_enemy");
    let firing = u.pack.icon("compassping_enemyfiring").or(enemy);
    let me = u.pack.icon("compassping_player");
    let friendly = u.pack.icon("compassping_friendly_mp");
    let grenade = u.pack.icon("compassping_grenade");
    let explosion = u.pack.icon("compassping_explosion");
    u.page(|ui, q| {
        // MW2's own images over the map (the scan lines, the frame).
        if let Some(s) = ui.screen("minimap") {
            q.extend(s.quads.iter().map(|q| wpk::ui::Quad {
                x: q.x + ox,
                y: q.y + oy,
                ..*q
            }));
        }
        let first = q.len();
        let alpha = |a: f32| 0xffff_ff00 | (a.clamp(0.0, 1.0) * 255.0) as u32;
        if let Some(a) = arena.filter(|_| !jammed) {
            for (i, fi) in a.fighters.iter().enumerate().skip(1) {
                if !fi.life.alive() {
                    continue;
                }
                // Teammates always show (MW2's friendly arrows).
                if !a.enemies(0, i) {
                    if let Some(t) = friendly {
                        q.push(Frame::icon(
                            f.to_screen(fi.player.ps.origin),
                            Frame::ENEMY_SIZE,
                            t,
                            0xffff_ffff,
                        ));
                    }
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
        // Grenades in the air or on the ground, and recent explosions (MW2's compass pings).
        if let Some(a) = arena.filter(|_| !jammed) {
            for g in &a.grenades {
                if let Some(t) = grenade {
                    q.push(Frame::icon(
                        f.to_screen(g.origin),
                        Frame::ENEMY_SIZE,
                        t,
                        0xffff_ffff,
                    ));
                }
            }
        }
        for &(o, t0) in blasts {
            if let (Some(o), Some(t)) = (o, explosion)
                && let Some(al) = ping_alpha(now.wrapping_sub(t0), FIRING_PING_MS)
            {
                q.push(Frame::icon(f.to_screen(o), Frame::ENEMY_SIZE, t, alpha(al)));
            }
        }
        // Pings beyond the map's frame aren't shown (MW2 clips them to the compass).
        let inside = |p: &wpk::ui::Quad| {
            let (cx, cy) = (p.x + p.w * 0.5, p.y + p.h * 0.5);
            cx >= rect[0] && cx <= rect[0] + rect[2] && cy >= rect[1] && cy <= rect[1] + rect[3]
        };
        let mut k = first;
        while k < q.len() {
            if inside(&q[k]) {
                k += 1;
            } else {
                q.swap_remove(k);
            }
        }
        if let Some(t) = me {
            q.push(f.player_icon(t));
        }
    });
}

/// End of match: everyone's score, best first (fewer deaths break ties), until A (or B).
fn scoreboard(gfx: &mut Shim, input: &mut front::Input, a: &Arena) {
    let mut order = a.standings();
    let title = match a.mode {
        arena::Mode::Ffa if order.first() == Some(&0) => "YOU WIN",
        arena::Mode::Ffa => "MATCH OVER",
        arena::Mode::Tdm | arena::Mode::Dom => {
            // Your team first.
            order.sort_by_key(|&i| a.fighters[i].team != a.fighters[0].team);
            match a.leading_team() {
                Some(t) if t == a.fighters[0].team => "VICTORY",
                Some(_) => "DEFEAT",
                None => "DRAW",
            }
        }
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
    front::list(
        gfx,
        input,
        &front::Page::on("main"),
        title,
        &lines,
        &mut sel,
        "A CONTINUE",
    );
}

struct Loadouts<'a> {
    catalog: Option<&'a wpk::classes::Catalog>,
    classes: &'a [wpk::classes::Loadout],
    /// The class chosen before the match (index into `classes`).
    class: Option<usize>,
    /// Cursor of the class list (kept between menus).
    cursor: &'a mut usize,
    /// Ask for the class as the match starts (MW2's class menu on joining).
    ask: bool,
}

/// A debug bullet path: (start, end) and when it was fired.
type Trail = (Option<([f32; 3], [f32; 3])>, u32);

/// Load `<app folder>/maps/<name>.wpk` and play it until START (menu) or RESET (quit). Everything the map
/// allocates in MEM2 is released by the caller (`mwf_mem2_release`); the MEM1 parts drop here.
#[allow(clippy::too_many_arguments)]
fn run_map(
    gfx: &mut Shim,
    name: &str,
    scratch: &mut WalkScratch,
    common: &'static [u8],
    loadouts: Loadouts<'_>,
    setup: (arena::Mode, arena::Limits),
    input: &mut front::Input,
    debug: &mut game_core::debug::DebugSettings,
    options: &mut game_core::options::Options,
) -> Exit {
    let bots = usize::from(debug.bots);
    let weapons = weapon_section(common);
    // Thrown weapons' numbers (fuse, blast, bounce; empty with an older pack).
    let equips = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::WEAPON_EQUIP))
        .map_or(&[][..], |s| s.data);
    // Bullet penetration (empty / None with an older pack).
    let pens = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::WEAPON_PEN))
        .map_or(&[][..], |s| s.data);
    let pen_table = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::PEN_TABLE))
        .filter(|s| s.data.len() >= 4 * 31 * 4)
        .map(|s| {
            let f = |k: usize| {
                f32::from_be_bytes([
                    s.data[k * 4],
                    s.data[k * 4 + 1],
                    s.data[k * 4 + 2],
                    s.data[k * 4 + 3],
                ])
            };
            weapon_iw4::PenetrationDepthTable::from_depths(core::array::from_fn(|t| {
                core::array::from_fn(|s| f(t * 31 + s))
            }))
        });
    // Launched projectiles' impact, guidance and lock (empty with an older pack).
    let missiles = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::WEAPON_MISSILE))
        .map_or(&[][..], |s| s.data);
    // Smoke's sight-blocking elements (empty with an older pack).
    let blockers = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::WEAPON_BLOCKERS))
        .map_or(&[][..], |s| s.data);
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
            "build={} map={name} bots={bots} class={} mem1_free={}k mem2_free={}k widescreen={}",
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
    // Game-mode objectives (Domination flags; none with an older pack).
    let objectives: Vec<wpk::records::ObjectiveRecord> = wpk::Pack::parse_prefix(bytes)
        .ok()
        .and_then(|p| p.section(wpk::tag::OBJECTIVES))
        .map_or_else(Vec::new, |s| wpk::records::iter(s.data).collect());
    let mut world = match World::load(bytes, vis.as_ref(), gfx) {
        Ok(w) => w,
        Err(game_core::world::LoadError::TooManyTextures) => return fail(0xe08020ff),
        Err(_) => return fail(0xc020c0ff),
    };
    // The players' bodies (none with an older pack: placeholder boxes then).
    let mut characters = character::Characters::load(bytes, common, |i| world.texture(i), gfx);
    // MW2's effects (none with an older pack: placeholder flashes then).
    let mut effects = effects::Effects::load(common, gfx);
    // The map's destructible objects and the effects they play (none with an older pack).
    if let Some(fx) = effects.as_mut() {
        fx.load_map(bytes, gfx);
    }
    let (props, map_objects) = match objects::Props::load(bytes, gfx) {
        Some((p, o)) => (Some(p), Some(o)),
        None => (None, None),
    };
    let mut poses: Vec<character::Pose> = Vec::with_capacity(bots);
    let clip: Option<MapClip<'_>> = clip_data.as_ref().map(MapClip::new);
    devlog::event(
        ffi::ms(),
        "loaded",
        format_args!(
            "ms={} mem1_free={}k mem2_free={}k collision={} nav_points={} characters={} objects={} objectives={}",
            ffi::ms().wrapping_sub(load_start),
            unsafe { ffi::mwf_mem1_free_bytes() } / 1024,
            unsafe { ffi::mwf_mem2_free_bytes() } / 1024,
            clip.is_some(),
            nav.as_ref().map_or(0, |n| n.graph.nodes.len()),
            characters.is_some(),
            map_objects.as_ref().map_or(0, |o| o.len()),
            objectives.len(),
        ),
    );
    let aspect = if unsafe { ffi::mwf_widescreen() } != 0 {
        16.0 / 9.0
    } else {
        4.0 / 3.0
    };
    let mut spawn = 0usize;
    let mut cam = spawn_camera(&world, spawn);
    // The view's feel on the eye (bob, landing dip, hit kick, scope sway: `game_core::view_kick`).
    let mut view_kick = game_core::view_kick::ViewKick::default();
    let mut landings_logged = 0u32;
    let mut was_mantling = false;
    let mut was_holding_breath = false;
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
    // The class's loadout; the debug menu's primary weapon replaces the class's (no attachments).
    let loadout_of = |class: Option<usize>, primary: u8| {
        let mut l = class.and_then(|c| loadouts.classes.get(c)).cloned();
        if primary != wpk::classes::NONE
            && let Some(l) = l.as_mut()
        {
            l.primary = primary;
            l.primary_attachments = [wpk::classes::NONE; 2];
        }
        l
    };
    // Arm for a spawn; returns the class's perks (with their Pro perks: everything is unlocked).
    let equip =
        |arsenal: &mut arsenal::Arsenal, gfx: &mut Shim, class: Option<usize>, primary: u8| {
            let l = loadout_of(class, primary);
            let l = l.as_ref();
            arsenal.equip(
                gfx,
                &arsenal::WeaponTables {
                    weapons,
                    kicks,
                    moves,
                    aims,
                    equips,
                    pens,
                    pen_table,
                },
                loadouts.catalog,
                l,
            );
            match (loadouts.catalog, l) {
                (Some(c), Some(l)) => game_core::perks::of_loadout(c, l),
                _ => game_core::perks::PerkSet::default(),
            }
        };
    let mut perk_set = equip(&mut arsenal, gfx, class, debug.primary);
    // The class menu at the first frame (`Loadouts::ask`).
    let mut ask_class = loadouts.ask && !loadouts.classes.is_empty();
    // Debug: every killstreak reward at once.
    let all_rewards = || -> Vec<alloc::string::String> {
        loadouts
            .catalog
            .map(|c| c.killstreaks.iter().map(|k| k.name.clone()).collect())
            .unwrap_or_default()
    };
    // The mode's spawns (free-for-all or team deathmatch spawns when the map has them).
    let spawn_kind = match setup.0 {
        arena::Mode::Ffa => wpk::records::spawn_kind::DM,
        // Domination spawns on the team deathmatch spawns (not MW2's flag-weighted `mp_dom_spawn` choice).
        arena::Mode::Tdm | arena::Mode::Dom => wpk::records::spawn_kind::TDM,
    };
    let spawn_list: Vec<([f32; 3], f32)> = {
        let dm: Vec<_> = world
            .spawns
            .iter()
            .filter(|s| s.kind == spawn_kind)
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
            // Bots carry MW2's frag.
            let tables = arsenal::WeaponTables {
                weapons,
                kicks,
                moves,
                aims,
                equips,
                pens,
                pen_table,
            };
            g.set_penetration(arsenal::penetration(&tables, index));
            if let Some(row) = arsenal::offhand_row(&tables, "frag_grenade_mp") {
                g.set_offhands(&[row], arsenal::OFFHAND_CLASS_FRAG, 2);
            }
            Some(g)
        })
    };
    let mut arena = match (&clip, &player) {
        (Some(c), Some(p)) if !spawn_list.is_empty() => {
            Some(Arena::new(c, &spawn_list, bots, p.clone(), rules, |i| {
                if i == 0 { None } else { bot_gun(i) }
            }))
        }
        _ => None,
    };
    if let Some(a) = arena.as_mut() {
        if let Some(o) = map_objects {
            a.objects = o;
        }
        a.equips = equips;
        a.missiles = missiles;
        a.blockers = blockers;
        // Time the bots (telemetry `bai` / `bmv` / `bgun`).
        a.clock = Some(clock_us);
    }
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
    let mut boxes: Vec<[f32; 6]> = Vec::with_capacity(bots * 10 + 64);
    let mut box_colours: Vec<u32> = Vec::with_capacity(bots * 10 + 64);
    let mut impacts: [(Option<[f32; 3]>, u32); 8] = [(None, 0); 8];
    // World sounds (explosions, grenade bounces, throws) from the common pack's bank.
    let mut world_sounds = sound::Sounds::load(common);
    // Footsteps by gait, listener and surface (`STEP`; empty with an older pack).
    let footsteps: Vec<u16> = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::FOOTSTEPS))
        .map_or_else(Vec::new, |s| {
            s.data
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect()
        });
    let equip_sound = |weapon: u32, k: usize| {
        wpk::records::get::<wpk::weapon::EquipRecord>(equips, weapon as usize)
            .map_or(wpk::sound::NONE, |e| e.sounds[k])
    };
    // Recent explosions (where, when) for their flash.
    let mut blasts: [(Option<[f32; 3]>, u32); 4] = [(None, 0); 4];
    let mut next_blast = 0usize;
    let mut last_shot_ms: Option<u32> = None;
    // The Javelin's lock on a point (`_javelin.gsc`, `game_core::lock`).
    let mut javelin = game_core::lock::JavelinLock::default();
    let mut next_impact = 0usize;
    let mut hud_rects: Vec<Rect> = Vec::with_capacity(512);
    let mut life = Life::with_rules(&rules);
    let mut clock = MatchClock::with_limit(ffi::ms(), setup.1.time_limit_ms);
    if let Some(a) = arena.as_mut() {
        a.set_mode(setup.0, setup.1);
        a.set_bot_skill(debug.skill());
        if setup.0 == arena::Mode::Dom {
            a.dom = game_core::dom::Domination::from_objectives(&objectives);
        }
    }
    // Below the lowest spawn point by this much is out of the world.
    let floor_z = world
        .spawns
        .iter()
        .map(|s| s.origin[2])
        .fold(f32::MAX, f32::min)
        - life::OUT_OF_WORLD_DEPTH;
    let mut flying = player.is_none();
    let mut last = ffi::ms();
    let mut show_overlay = debug.perf_overlay;
    // Debug: where the last teleports went, your recent bullet paths (start, end, when).
    let (mut tp_spawn, mut tp_objective) = (0usize, 0usize);
    let mut shot_trails: [Trail; 6] = [(None, 0); 6];
    let mut next_trail = 0usize;
    // Killstreaks: kills since your last death, the rewards earned and not yet used (newest last), the UAV's radar.
    let mut streak = 0u32;
    let mut rewards: Vec<alloc::string::String> = if debug.all_killstreaks {
        all_rewards()
    } else {
        Vec::new()
    };
    let mut reward_note: Option<(&'static str, u32)> = None;
    // The HUD's score popup: your score last frame, the points just scored and when.
    let mut last_score = 0u32;
    let mut popup: Option<(u32, u32)> = None;
    // Weapon names by index (the HUD's grenade icons).
    let weapon_names: Vec<alloc::string::String> =
        wpk::records::iter::<wpk::weapon::WeaponRecord>(weapons)
            .map(|r| alloc::string::String::from(r.name()))
            .collect();
    let mut radar = game_core::minimap::Radar::default();
    let mut radar_at = last;
    let mut timing = Timing::default();
    // The debug menu's AUTO draw distance (follows the frame time).
    let mut auto_distance = game_core::auto_distance::AutoDistance::default();
    let mut rects: Vec<Rect> = Vec::with_capacity(4096);
    let mut frame_start = ffi::us();
    let mut last_mem_s = u32::MAX;
    // Gamepad look (IW4L): the sticks' learned range, the look's state between steps, settings.
    let mut sticks = look::StickRange::default();
    let mut look_state = look::LookState::default();
    let mut look_settings = options.look();
    // The pads (GameCube, USB Xbox 360) to actions, with the button layout (Default / Tactical: `controls`).
    let tactical =
        |o: &game_core::options::Options| o.layout == game_core::options::layout::TACTICAL;
    let mut mapper = controls::Mapper::default();
    // Diagnostics: the sprint button last frame (a press that doesn't start a sprint is logged with why), and the
    // sound voices taken over so far (logged once a second when it grows).
    let mut sprint_held_before = false;
    let mut steals_logged = 0u32;
    let mut aim_targets: Vec<look::AimTarget> = Vec::with_capacity(bots);
    // The current field of view over the hip one (look speed and aim assist scale with the zoom).
    let mut hip_fov_scale = 1.0f32;
    // When X started being held over a knife you can pick up (IW4L: the use button held 250 ms takes the item).
    let mut use_hold_ms: Option<u32> = None;
    let mut stance_button = game_core::player::StanceButton::default();
    // X double tap for C4 (`watchC4AltDetonate`): when X went down, and when a short press ended.
    let mut x_down_ms: Option<u32> = None;
    let mut x_short_release_ms: Option<u32> = None;
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
        let (gc_pad, x360_pad) = ffi::pads();
        let ctl = mapper.map(&gc_pad, &x360_pad, tactical(options));
        let mut sprint_check = ctl.held(act::SPRINT) && !sprint_held_before;
        sprint_held_before = ctl.held(act::SPRINT);
        let steals = unsafe { ffi::mwf_snd_steals() };
        if steals != steals_logged && now / 1000 != last.wrapping_sub(msec) / 1000 {
            devlog::event(
                now,
                "sound_voices_full",
                format_args!("taken_over={}", steals.wrapping_sub(steals_logged)),
            );
            steals_logged = steals;
        }
        // The debug settings, every frame (the pause menu's debug page can change them).
        if let Some(a) = arena.as_mut() {
            a.god_mode = debug.god_mode;
            a.bots_passive = debug.bots_passive;
            a.bots_frozen = debug.bots_frozen;
        }
        world.draw_models = debug.world_models;
        world.draw_distance = match debug.draw_distance_units() {
            game_core::debug::AUTO => auto_distance.distance,
            d => f32::from(d),
        };
        world.lod_scale = debug.lod_scale();
        world.map_fog = debug.map_fog;
        for g in arsenal.guns.iter_mut().flatten() {
            g.kick_scale = debug.kick_scale();
        }
        sound::MUTED.store(!debug.sound, core::sync::atomic::Ordering::Relaxed);
        if debug.infinite_ammo
            && let Some(g) = arsenal.guns[arsenal.active].as_mut()
        {
            g.refill_ammo();
        }
        // Z + D-pad down: the performance overlay (D-pad down alone is the tactical grenade).
        if ctl.pressed(act::OVERLAY) {
            show_overlay = !show_overlay;
        }
        if ffi::should_exit() {
            return Exit::Quit;
        }
        if ctl.pressed(act::PAUSE) {
            unsafe { ffi::mwf_snd_stop_all() };
            devlog::event(now, "pause", format_args!(""));
            stick_event(&stick_max, &trigger_max, now);
            devlog::flush(now);
            let kind = match setup.0 {
                arena::Mode::Ffa => "FREE-FOR-ALL",
                arena::Mode::Tdm => "TEAM DEATHMATCH",
                arena::Mode::Dom => "DOMINATION",
            };
            let mode = if setup.1.score_limit > 0 {
                alloc::format!("{kind}: FIRST TO {} POINTS", setup.1.score_limit)
            } else {
                alloc::string::String::from(kind)
            };
            // The frozen game behind the menu.
            front::grab_screen();
            // Options go back to the START menu, as in MW2; the other choices close it.
            let mut pause_sel = 0usize;
            let choice = loop {
                let c = front::pause(
                    gfx,
                    input,
                    name,
                    &mode,
                    loadouts.catalog,
                    (!loadouts.classes.is_empty()).then_some(loadouts.classes),
                    loadouts.cursor,
                    &mut pause_sel,
                );
                if let front::Pause::Options = c {
                    front::options_menu(gfx, input, options, true);
                    front::save_options(options);
                    look_settings = options.look();
                    continue;
                }
                break c;
            };
            match choice {
                front::Pause::EndGame => {
                    devlog::event(ffi::ms(), "end_game", format_args!(""));
                    devlog::end(ffi::ms());
                    return Exit::Menu;
                }
                front::Pause::ChangeClass(c) => {
                    devlog::event(now, "class_change", format_args!("class={c}"));
                    class = Some(c);
                }
                front::Pause::Options => {}
                front::Pause::Debug => {
                    use game_core::debug::Action;
                    let cat = loadouts.catalog;
                    let weapon_name = |i: u8| {
                        cat.and_then(|c| c.weapons.get(usize::from(i)))
                            .map(|w| w.name.to_ascii_uppercase())
                    };
                    let count = cat.map_or(0, |c| c.weapons.len().min(254) as u8);
                    let mut sel = 0usize;
                    let action =
                        front::debug_menu(gfx, input, debug, &weapon_name, count, true, &mut sel);
                    front::save_debug(debug);
                    show_overlay = debug.perf_overlay;
                    let now = ffi::ms();
                    let yaw_of = |p: &Player| p.view().1;
                    match (action, clip.as_ref()) {
                        (Some(Action::TeleportSpawn), Some(c)) if !spawn_list.is_empty() => {
                            tp_spawn = (tp_spawn + 1) % spawn_list.len();
                            let (o, yaw) = spawn_list[tp_spawn];
                            player = Some(Player::spawn(c, o, yaw));
                        }
                        (Some(Action::TeleportObjective), Some(c)) if !objectives.is_empty() => {
                            tp_objective = (tp_objective + 1) % objectives.len();
                            let o = objectives[tp_objective].origin;
                            let yaw = player.as_ref().map_or(0.0, yaw_of);
                            player = Some(Player::spawn(c, [o[0] + 48.0, o[1], o[2] + 8.0], yaw));
                        }
                        (Some(Action::BotInFront), Some(c)) => {
                            if let (Some(a), Some(p)) = (arena.as_mut(), player.as_ref()) {
                                let yaw = yaw_of(p);
                                let o = p.ps.origin;
                                let at = [
                                    o[0] + libm::cosf(yaw.to_radians()) * 150.0,
                                    o[1] + libm::sinf(yaw.to_radians()) * 150.0,
                                    o[2] + 8.0,
                                ];
                                if let Some(f) =
                                    a.fighters.iter_mut().skip(1).find(|f| f.life.alive())
                                {
                                    f.player = Player::spawn(c, at, yaw + 180.0);
                                }
                            }
                        }
                        (Some(Action::KillBots), _) => {
                            if let Some(a) = arena.as_mut() {
                                for f in a.fighters.iter_mut().skip(1) {
                                    if f.life.alive() {
                                        f.life.damage(f.life.max_health * 10, now);
                                    }
                                }
                            }
                        }
                        (Some(Action::CallUav), _) => radar.activate(now, rules.uav_time_ms),
                        (Some(Action::CallCounterUav), _) => {
                            if let Some(a) = arena.as_mut() {
                                a.fighters[0].counter_uav_until =
                                    Some(now.wrapping_add(rules.uav_time_ms));
                            }
                        }
                        (Some(Action::CallEmp), _) => {
                            if let Some(a) = arena.as_mut() {
                                a.fighters[0].emp_until = Some(now.wrapping_add(rules.emp_ms));
                            }
                        }
                        (Some(Action::CallNuke), _) => {
                            if let Some(a) = arena.as_mut()
                                && a.nuke.is_none()
                            {
                                a.nuke = Some(arena::Nuke {
                                    owner: 0,
                                    detonate_ms: now.wrapping_add(rules.nuke_timer_ms),
                                    done: false,
                                });
                            }
                        }
                        (Some(Action::RefillRewards), _) => rewards = all_rewards(),
                        (Some(Action::ResetClock), _) => {
                            clock = MatchClock::with_limit(now, setup.1.time_limit_ms);
                        }
                        (Some(Action::WriteLog), _) => devlog::flush(now),
                        _ => {}
                    }
                    if let Some(a) = action {
                        devlog::event(now, "debug", format_args!("action={a:?}"));
                    }
                }
                front::Pause::Resume => {}
            }
            // The match was stopped: no time passed.
            last = ffi::ms();
            frame_start = ffi::us();
            continue;
        }
        // Y: the other weapon (MW2's console layout).
        if ctl.pressed(act::SWITCH) && !flying && life.alive() && arsenal.switch() {
            devlog::event(
                now,
                "switch_request",
                format_args!("from={}", arsenal.active),
            );
        }
        // D-pad left (360; Z + D-pad left on the GameCube pad): the weapon's underbarrel launcher or shotgun and back
        // (MW2's action slot 3).
        if ctl.pressed(act::ALT) && !flying && life.alive() && arsenal.alternate() {
            devlog::event(now, "alt_request", format_args!("from={}", arsenal.active));
        }
        // Call in the newest killstreak reward (D-pad right on the 360 pad, as MW2's action slot 4; D-pad left on the
        // GameCube pad).
        if ctl.pressed(act::KILLSTREAK)
            && life.alive()
            && let Some(r) = rewards.pop()
        {
            if r == "uav" {
                radar.activate(now, rules.uav_time_ms);
                reward_note = Some(("UAV ONLINE", now));
            } else if r == "counter_uav"
                && let Some(a) = arena.as_mut()
            {
                // `_uav.gsc`: up as long as a UAV, jamming the enemies' radar.
                a.fighters[0].counter_uav_until = Some(now.wrapping_add(rules.uav_time_ms));
                reward_note = Some(("COUNTER-UAV ONLINE", now));
            } else if r == "emp"
                && let Some(a) = arena.as_mut()
            {
                // `_emp.gsc`: the enemies' radar is jammed for the EMP's time.
                a.fighters[0].emp_until = Some(now.wrapping_add(rules.emp_ms));
                reward_note = Some(("EMP DEPLOYED", now));
            } else if r == "nuke"
                && let Some(a) = arena.as_mut()
                && a.nuke.is_none()
            {
                a.nuke = Some(arena::Nuke {
                    owner: 0,
                    detonate_ms: now.wrapping_add(rules.nuke_timer_ms),
                    done: false,
                });
                reward_note = Some(("TACTICAL NUKE INCOMING", now));
            } else {
                reward_note = Some(("THAT KILLSTREAK IS NOT IN THIS BUILD YET", now));
            }
            devlog::event(now, "killstreak", format_args!("used={r}"));
        }
        // Free fly (Z + D-pad up on the GameCube pad, BACK + D-pad up on the 360 pad).
        if ctl.pressed(act::FLY) && clip.is_some() {
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
        for (m, v) in
            stick_max
                .iter_mut()
                .zip([ctl.stick_l.0, ctl.stick_l.1, ctl.stick_r.0, ctl.stick_r.1])
        {
            *m = (*m).max(v.saturating_abs());
        }
        trigger_max[0] = trigger_max[0].max(ctl.triggers.0);
        trigger_max[1] = trigger_max[1].max(ctl.triggers.1);
        // Player steps due this frame (60 Hz, at most 6 = 100 ms).
        tick_acc3 += msec * 3;
        let player_ticks = (tick_acc3 / 50).min(6);
        tick_acc3 -= player_ticks * 50;
        tick_acc3 = tick_acc3.min(50);
        // Sticks: learned range, IW4L's radial dead zone; the main stick as IW4 move amounts.
        let (main_stick, c_stick) = {
            let (m, c) = (
                sticks.normalize(0, ctl.stick_l.0, ctl.stick_l.1),
                sticks.normalize(1, ctl.stick_r.0, ctl.stick_r.1),
            );
            // Southpaw: move with the C-stick, look with the main stick.
            if options.southpaw { (c, m) } else { (m, c) }
        };
        let (forward, right) = look::to_move(main_stick);
        let ads_held = ctl.held(act::ADS);
        // A thrown knife in reach: X (MW2's use / reload button) picks it up instead of reloading.
        let knife_near = match (arena.as_ref(), clip.as_ref()) {
            (Some(a), Some(c)) if life.alive() && !flying => a.knife_in_reach(c, 0, |w| {
                arsenal.gun().is_some_and(|g| g.can_pick_up_offhand(w))
            }),
            _ => None,
        };
        let x_held = ctl.held(act::USE);
        let reload_held = x_held && knife_near.is_none();
        // C4: X pressed, let go within 0.5 s, pressed again within 0.5 s detonates it.
        match (x_held, x_down_ms) {
            (true, None) => {
                if x_short_release_ms.is_some_and(|r| now.wrapping_sub(r) < 500)
                    && let Some(a) = arena.as_mut()
                    && life.alive()
                    && a.detonate_c4(0, now)
                {
                    devlog::event(now, "c4", format_args!("detonate"));
                }
                x_short_release_ms = None;
                x_down_ms = Some(now);
            }
            (false, Some(d)) => {
                x_short_release_ms = (now.wrapping_sub(d) < 500).then_some(now);
                x_down_ms = None;
            }
            _ => {}
        }
        match (knife_near, x_held) {
            (Some(k), true) => {
                let since = *use_hold_ms.get_or_insert(now);
                if now.wrapping_sub(since) >= 250
                    && let Some(a) = arena.as_mut()
                {
                    let w = a.grenades[k].weapon;
                    if let Some(g) = arsenal.guns[arsenal.active].as_mut()
                        && g.pick_up_offhand(w)
                    {
                        a.grenades.swap_remove(k);
                        devlog::event(now, "pickup", format_args!("weapon={w}"));
                    }
                    use_hold_ms = None;
                }
            }
            _ => use_hold_ms = None,
        }
        // Aim-assist targets: living bots in clear view of your eye (their hull box and chest).
        aim_targets.clear();
        if let (Some(a), Some(p), Some(c)) = (arena.as_ref(), player.as_ref(), clip.as_ref())
            && !flying
            && life.alive()
        {
            let eye = p.eye();
            let fwd = cam.forward();
            for (i, f) in a.fighters.iter().enumerate().skip(1) {
                // Aim assist helps against enemies only.
                if !f.life.alive() || !a.enemies(0, i) {
                    continue;
                }
                let (feet, chest) = arena::feet_and_aim(&f.player);
                // Its regions are near the screen's centre (at most 160 of 320 half-width pixels: about 18° off the
                // view at the hip FOV), so an enemy over 45° off the view (last frame's) can't be a target: no sight
                // trace for it (they cost up to milliseconds each through dense collision).
                let d: [f32; 3] = core::array::from_fn(|k| chest[k] - eye[k]);
                let len = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                if len > 48.0 && d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2] < len * 0.707 {
                    continue;
                }
                let height = f.player.hull_height();
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
        // A test script's `warp`: the player to a spot (`game_core::autoplay`).
        if let (Some(w), Some(c)) = (autoplay::take_warp(), clip.as_ref()) {
            let o = [w[0] as f32, w[1] as f32, w[2] as f32];
            player = Some(Player::spawn(c, o, w[3] as f32));
            view_kick.reset();
            if let Some(p) = player.as_ref() {
                devlog::event(
                    now,
                    "warp",
                    format_args!(
                        "to={:.1},{:.1},{:.1}",
                        p.ps.origin[0], p.ps.origin[1], p.ps.origin[2]
                    ),
                );
            }
        }
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
                    let mut look_delta = look::look_frame(
                        &mut look_state,
                        &look_settings,
                        c_stick,
                        &view,
                        &aim_targets,
                        ads_held,
                    );
                    // A shock's hold on the view (the stun's `.shock` file: slower, capped turning).
                    if let Some(s) = arena.as_ref().and_then(|a| a.fighters[0].shock)
                        && s.remaining(now) > 0
                    {
                        look_delta = look::shock_look(
                            look_delta,
                            view.dt,
                            &s.fx(&rules).look,
                            now.wrapping_sub(s.start_ms),
                            s.duration_ms,
                        );
                    }
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
                        a.fighters[0].bullet_scale = perk_set.bullet_scale(&rules);
                        a.fighters[0].explosive_scale = perk_set.explosive_scale(&rules);
                    }
                    // A stun's shock slows you (IW4 `PMF_SHELLSHOCKED`, when its `.shock` file says so).
                    if let Some(a) = arena.as_mut() {
                        if a.shock_slows(0, now) {
                            p.ps.pm_flags |= playerstate_iw4::pm_flags::SHELLSHOCKED;
                        } else {
                            p.ps.pm_flags &= !playerstate_iw4::pm_flags::SHELLSHOCKED;
                        }
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
                            jump: ctl.held(act::JUMP),
                            // B: tap to crouch, hold to go prone (MW2's console stance button).
                            stance: stance_button.update(
                                ctl.held(act::STANCE),
                                !p.on_ground(),
                                now as i32,
                            ),
                            sprint: ctl.held(act::SPRINT),
                            ads: ads_held,
                            fire: ctl.held(act::FIRE),
                            reload: reload_held,
                            // Z / left stick click (MW2's `+breath_sprint`): steadies a scope while aimed.
                            breath: ctl.held(act::BREATH),
                        },
                        &weapon_move,
                        tick_ms,
                    );
                    if core::mem::take(&mut sprint_check)
                        && p.ps.pm_flags & playerstate_iw4::pm_flags::SPRINTING == 0
                    {
                        // Why a sprint press did nothing (IW4L `update_sprint`): forward under 105, the weapon
                        // in a state that blocks it (melee 13-15, offhand 16-21), the button needing a release,
                        // or the meter (start / end / length vs the player's clock).
                        devlog::event(
                            now,
                            "sprint_refused",
                            format_args!(
                                "fwd={forward} ws={} upreq={} pmf={:#x} ground={} start={} end={} len={} t={}",
                                p.ps.weaponstate_primary,
                                p.ps.sprint_button_up_required,
                                p.ps.pm_flags,
                                p.on_ground(),
                                p.ps.last_sprint_start,
                                p.ps.last_sprint_end,
                                p.ps.sprint_start_max_length,
                                p.ps.command_time,
                            ),
                        );
                    }
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
                    if fall > 0 && !debug.god_mode {
                        life.damage(fall, now);
                        // A fall kicks the view as damage from nowhere (IW4 `P_DamageFeedback`, the world).
                        view_kick.hit(&p.ps, None, fall, now);
                    }
                    // The view's feel (IW4L `weapon_iw4::view_bob`): its angles go on the view shots follow too (the
                    // scope's sway and the hit kick move your aim); the eye's bob and landing dip only move the
                    // drawn camera, after the gun has fired from the eye.
                    let view_weapon = arsenal.gun().map(|g| {
                        let fpv = wpk::records::get::<wpk::weapon::FpvRecord>(
                            fpvs,
                            g.weapon_index() as usize,
                        )
                        .unwrap_or_default();
                        game_core::view_kick::ViewWeapon {
                            overlay_reticle: g.facts().overlay_reticle,
                            aim_down_sight: g.facts().aim_down_sight,
                            ads_bob_factor: fpv.ads_bob_factor,
                            ads_view_bob_mult: fpv.ads_view_bob_mult,
                            idle: weapon_iw4::WeaponIdleInputs {
                                ads_idle_amount: fpv.idle[0],
                                hip_idle_amount: fpv.idle[1],
                                ads_idle_speed: fpv.idle[2],
                                hip_idle_speed: fpv.idle[3],
                                idle_crouch_factor: fpv.idle[4],
                                idle_prone_factor: fpv.idle[5],
                            },
                        }
                    });
                    let view_pose =
                        view_kick.frame(&p.ps, view_weapon.as_ref(), now, tick_ms as f32 / 1000.0);
                    cam.pitch += view_pose.angles[0];
                    cam.yaw += view_pose.angles[1];
                    cam.roll = view_pose.angles[2];
                    // A warp or spawn resets the count: only a new landing logs.
                    if view_kick.landings != landings_logged {
                        let landed = view_kick.landings > landings_logged;
                        landings_logged = view_kick.landings;
                        if landed {
                            devlog::event(
                                now,
                                "land_dip",
                                format_args!(
                                    "dip={} z={:.1}",
                                    view_kick.land_view_dip, p.ps.origin[2]
                                ),
                            );
                        }
                    }
                    // Holding breath, for the log (IW4 `WEAPON_FLAG_HOLD_BREATH`).
                    let holding = p.ps.weap_flags & playerstate_iw4::weap_flags::HOLD_BREATH != 0;
                    if holding != was_holding_breath {
                        was_holding_breath = holding;
                        devlog::event(
                            now,
                            if holding {
                                "breath_hold"
                            } else {
                                "breath_release"
                            },
                            format_args!(
                                "scale={:.2} timer={}",
                                p.ps.hold_breath_scale, p.ps.hold_breath_timer
                            ),
                        );
                    }
                    // Mantles, for the log (IW4 `PMF_MANTLE`).
                    let mantling = p.ps.pm_flags & playerstate_iw4::pm_flags::MANTLE != 0;
                    if mantling != was_mantling {
                        was_mantling = mantling;
                        devlog::event(
                            now,
                            if mantling {
                                "mantle_start"
                            } else {
                                "mantle_end"
                            },
                            format_args!(
                                "at={:.1},{:.1},{:.1} anim={}",
                                p.ps.origin[0],
                                p.ps.origin[1],
                                p.ps.origin[2],
                                p.ps.mantle_trans_index
                            ),
                        );
                    }
                    if p.ps.origin[2] < floor_z && !debug.god_mode {
                        life.damage(life.max_health, now);
                    }
                    if let Some(a) = arena.as_mut() {
                        a.fighters[0].player = p.clone();
                        a.fighters[0].life = life;
                        // Your riot shield: in front when it is out, else on your back when you carry it.
                        let shield =
                            |g: &Gun| game_core::shield::is_shield(g.name(), g.facts().weap_type);
                        a.fighters[0].shield = if arsenal.gun().is_some_and(shield) {
                            game_core::shield::Carry::Front
                        } else if arsenal.guns[..2].iter().flatten().any(shield) {
                            game_core::shield::Carry::Back
                        } else {
                            game_core::shield::Carry::None
                        };
                    }
                    sections[section::PLAYER] += lap(&mut mark);
                    let a = arsenal.active;
                    let switch_to = arsenal.switch_target();
                    // Melee lunges at the enemy in reach (IW4L's charge target, sent with the melee press).
                    let melee_charge = match (arena.as_ref(), ctl.held(act::MELEE)) {
                        (Some(ar), true) => ar.melee_charge_target(c, 0),
                        _ => None,
                    };
                    // The Javelin's lock (a point on the world ahead); a weapon that needs a lock fires only on one
                    // (IW4L `require_lock_to_fire` with `WeaponLock::can_fire`).
                    let missile = arsenal.gun().and_then(|g| {
                        wpk::records::get::<wpk::weapon::MissileRecord>(
                            missiles,
                            g.weapon_index() as usize,
                        )
                    });
                    let javelin_out =
                        missile.is_some_and(|m| m.guidance == game_core::grenade::GUIDANCE_JAVELIN);
                    if let Some(e) = javelin.update(
                        now,
                        javelin_out && life.alive(),
                        p.ps.f_weapon_pos_frac,
                        cam.eye,
                        cam.forward(),
                        p.ps.origin,
                        |from, to| {
                            let t =
                                c.trace(from, to, [0.0; 3], [0.0; 3], game_core::gun::MASK_SHOT);
                            // Not the sky or a default surface (the script's `EyeTraceForward`).
                            (t.fraction < 1.0 && (t.surface_flags >> 20) & 0x1f != 0)
                                .then_some(t.endpos)
                        },
                    ) {
                        devlog::event(now, "javelin", format_args!("{e:?}"));
                    }
                    let lock_ok =
                        !missile.is_some_and(|m| m.require_lock) || javelin.locked().is_some();
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
                                fire: ctl.held(act::FIRE) && lock_ok,
                                // Akimbo fires its left gun with the aim trigger.
                                ads: ads_held,
                                reload: reload_held,
                                // Melee: the right stick click (360, Default) or D-pad right (GameCube).
                                melee: ctl.held(act::MELEE),
                                // Lethal and tactical grenades, held to cook (RB / LB; D-pad up / down).
                                frag: ctl.held(act::FRAG),
                                smoke: ctl.held(act::TACTICAL),
                                pm_flags: p.ps.pm_flags,
                                perks: p.ps.perks,
                                switch_to,
                                melee_charge,
                                charge_state: (
                                    p.ps.melee_charge_yaw,
                                    p.ps.melee_charge_dist,
                                    p.ps.melee_charge_time,
                                ),
                                pm_type: p.ps.pm_type,
                                in_air: !p.on_ground(),
                            },
                            cam.eye,
                            cam.forward(),
                            cam.right(),
                            cam.up(),
                        );
                        // The weapon's state goes into the player's (pmove reads it next step).
                        g.sync_player(&mut p.ps);
                        if g.akimbo()
                            && matches!(
                                event,
                                Some(game_core::gun::WeaponTickEvent::ShotAccepted { .. })
                            )
                        {
                            devlog::event(
                                now,
                                "akimbo_fire",
                                format_args!(
                                    "right={} left={} stock={}",
                                    g.ammo().0,
                                    g.left_clip().unwrap_or(0),
                                    g.ammo().1
                                ),
                            );
                        }
                        // A launcher's projectile (rockets, grenade launchers), steered to the Javelin's lock.
                        if let Some(l) = g.take_launch()
                            && let Some(a) = arena.as_mut()
                        {
                            let target = javelin_out.then(|| javelin.locked()).flatten();
                            let ok =
                                a.launch(0, &l, g.facts().weap_class, p.ps.velocity, target, now);
                            a.fighters[0].last_fire_ms = Some(now);
                            last_shot_ms = Some(now);
                            devlog::event(
                                now,
                                "launch",
                                format_args!(
                                    "weapon={} ok={} locked={}",
                                    l.weapon,
                                    u8::from(ok),
                                    u8::from(target.is_some())
                                ),
                            );
                            if let (Some(fx), Some(v)) =
                                (effects.as_mut(), arsenal.views[arsenal.active % 2].as_ref())
                                && let Some((o, f)) = v.muzzle_eye()
                            {
                                let e = fx.weapon(g.weapon_index(), effects::VIEW_FLASH);
                                fx.play_view(e, o, f, now);
                            }
                        }
                        if let Some(game_core::gun::WeaponTickEvent::OffhandUsed {
                            weapon,
                            remaining_fuse_ms,
                        }) = event
                            && let (Some(a), Some(e)) = (
                                arena.as_mut(),
                                wpk::records::get::<wpk::weapon::EquipRecord>(
                                    equips,
                                    weapon as usize,
                                ),
                            )
                        {
                            a.add_grenade(
                                game_core::grenade::Grenade::throw(
                                    0,
                                    weapon,
                                    e,
                                    cam.eye,
                                    cam.forward(),
                                    p.ps.velocity,
                                    remaining_fuse_ms,
                                    now,
                                ),
                                now,
                            );
                            devlog::event(now, "grenade", format_args!("weapon={weapon}"));
                            if let Some(w) = world_sounds.as_mut() {
                                w.play(equip_sound(weapon, wpk::weapon::equip_sound::THROW));
                            }
                        }
                        if let Some(game_core::gun::WeaponTickEvent::OffhandPrepare { weapon }) =
                            event
                            && let Some(w) = world_sounds.as_mut()
                        {
                            w.play(equip_sound(weapon, wpk::weapon::equip_sound::PULLBACK));
                        }
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
                            // The first-person muzzle flash, in the gun's own (eye) space.
                            if let (Some(fx), Some(v)) =
                                (effects.as_mut(), arsenal.views[arsenal.active % 2].as_ref())
                                && let Some((o, f)) = v.muzzle_eye()
                            {
                                let e = fx.weapon(g.weapon_index(), effects::VIEW_FLASH);
                                fx.play_view(e, o, f, now);
                            }
                            shot_trails[next_trail] = (Some((s.start, s.pen_end)), now);
                            next_trail = (next_trail + 1) % shot_trails.len();
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
                            && let Some(v) = arsenal.views[a % 2].as_mut()
                            && let Some(&s) = v
                                .sounds
                                .events
                                .get(wpk::sound::event::MELEE_SWIPE)
                                .filter(|&&s| s != wpk::sound::NONE)
                        {
                            v.play(s);
                        }
                        if let (Some(e), Some(v), false) = (
                            event.as_ref(),
                            arsenal.views[a % 2].as_mut(),
                            raise_of_other,
                        ) && let Some(s) =
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
                    for k in 0..3 {
                        cam.eye[k] += view_pose.origin[k];
                    }
                }
            }
            (Some(_), Some(c)) if !flying => {
                // Dead: the view stays where the player fell until the respawn. A second in (the gun's last sounds
                // have played from its file), the next life's weapon files stream in, a slice a frame.
                if life.respawn_in(now) <= game_core::life::RESPAWN_DELAY_MS.saturating_sub(1000) {
                    let l = loadout_of(class, debug.primary);
                    arsenal.prefetch(gfx, loadouts.catalog, l.as_ref());
                }
                if life.ready_to_respawn(now) {
                    spawn += 1;
                    // MW2's spawn weighting, as for the bots.
                    // Your C4 and claymores from the last life go (`_weapons.gsc` on spawn).
                    if let Some(a) = arena.as_mut() {
                        a.clear_charges(0);
                    }
                    player = Some(match arena.as_mut() {
                        Some(a) if !spawn_list.is_empty() => {
                            let (o, yaw) = a.spawn_point(c, &spawn_list, 0);
                            Player::spawn(c, o, yaw)
                        }
                        _ => spawn_player(&world, c, spawn),
                    });
                    perk_set = equip(&mut arsenal, gfx, class, debug.primary);
                    if debug.all_killstreaks {
                        rewards = all_rewards();
                    }
                    life.respawn();
                    stance_button.reset();
                    view_kick.reset();
                }
            }
            _ => fly::update(
                &mut cam,
                &PadInput {
                    sx: ctl.stick_l.0,
                    sy: ctl.stick_l.1,
                    cx: ctl.stick_r.0,
                    cy: ctl.stick_r.1,
                    l: ctl.triggers.0,
                    r: ctl.triggers.1,
                    fast: ctl.held(act::FLY_FAST),
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
            a.update_grenades(c, dt, now, &mut events);
            a.update_objects(c, now, &mut events);
            a.update_objectives(dt * 1000.0, now, &mut events);
            a.footsteps(c, &mut events);
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
                Event::Footstep {
                    fighter,
                    origin,
                    gait,
                    surface,
                } => {
                    // Yours as you hear them (the `_plr` set), others' where they are.
                    let you = usize::from(fighter == 0);
                    let alias = footsteps
                        .get(wpk::sound::step_index(gait, you, surface))
                        .copied()
                        .unwrap_or(wpk::sound::NONE);
                    if let Some(w) = world_sounds.as_mut() {
                        if you == 1 {
                            w.play(alias);
                        } else {
                            w.play_at(alias, origin, cam.eye, cam.right());
                        }
                    }
                }
                Event::Hit {
                    attacker,
                    victim,
                    damage,
                    location,
                } => {
                    if attacker == 0 {
                        hitmarker_ms = Some(now);
                    }
                    // Your view kicks away from the hit (IW4 `P_DamageFeedback`: towards the attacker; your own
                    // grenade has no direction).
                    if victim == 0
                        && let Some(p) = player.as_ref()
                    {
                        let from = arena
                            .as_ref()
                            .filter(|_| attacker != 0)
                            .map(|a| a.fighters[attacker].player.ps.origin);
                        view_kick.hit(&p.ps, from, damage, now);
                    }
                    if attacker == 0 || victim == 0 {
                        devlog::event(
                            now,
                            "hit",
                            format_args!("by={attacker} on={victim} dmg={damage} loc={location}"),
                        );
                    }
                }
                Event::Impact {
                    origin,
                    normal,
                    surface,
                    flesh,
                    impact_type,
                } => {
                    if surface == game_core::shield::SURFACE {
                        devlog::event(now, "shield_block", format_args!(""));
                    }
                    if let Some(fx) = effects.as_mut() {
                        let e = fx.impact(impact_type, surface, flesh);
                        fx.play(e, origin, normal, now);
                    }
                }
                Event::ObjectFx {
                    effect,
                    origin,
                    forward,
                } => {
                    if let Some(fx) = effects.as_mut() {
                        fx.play_map(effect, origin, forward, now);
                    }
                }
                Event::ObjectExploded { origin } => {
                    // MW2's object sounds aren't packed yet: the frag's explosion stands in.
                    if let (Some(w), Some((frag, _))) = (
                        world_sounds.as_mut(),
                        wpk::weapon::find_indexed(weapons, "frag_grenade_mp"),
                    ) {
                        w.play_at(
                            equip_sound(frag, wpk::weapon::equip_sound::EXPLODE),
                            origin,
                            cam.eye,
                            cam.right(),
                        );
                    }
                    if effects.is_none() {
                        blasts[next_blast] = (Some(origin), now);
                        next_blast = (next_blast + 1) % blasts.len();
                    }
                    devlog::event(now, "object exploded", format_args!(""));
                }
                Event::BotFired { bot } => {
                    if let (Some(fx), Some(ch), Some(a)) =
                        (effects.as_mut(), characters.as_ref(), arena.as_ref())
                        && let Some((o, f)) = ch.muzzle(bot)
                        && let Some(g) = a.fighters.get(bot).and_then(|b| b.gun.as_ref())
                    {
                        let e = fx.weapon(g.weapon_index(), effects::WORLD_FLASH);
                        fx.play(e, o, f, now);
                    }
                }
                Event::GrenadeBounce { origin, weapon } => {
                    if let Some(w) = world_sounds.as_mut() {
                        w.play_at(
                            equip_sound(weapon, wpk::weapon::equip_sound::BOUNCE),
                            origin,
                            cam.eye,
                            cam.right(),
                        );
                    }
                }
                Event::Explosion {
                    origin,
                    weapon,
                    surface,
                } => {
                    if let Some(w) = world_sounds.as_mut() {
                        // Launchers (their own explosion sounds aren't packed) use the frag's.
                        let own = equip_sound(weapon, wpk::weapon::equip_sound::EXPLODE);
                        let alias =
                            match (own, wpk::weapon::find_indexed(weapons, "frag_grenade_mp")) {
                                (wpk::sound::NONE, Some((frag, _))) => {
                                    equip_sound(frag, wpk::weapon::equip_sound::EXPLODE)
                                }
                                _ => own,
                            };
                        w.play_at(alias, origin, cam.eye, cam.right());
                    }
                    // MW2's explosion effect (the weapon's own, else the impact table's for its impact type and
                    // the surface), or the placeholder flash without one.
                    let impact_type =
                        wpk::records::get::<wpk::weapon::WeaponRecord>(weapons, weapon as usize)
                            .map_or(0, |r| r.combat.impact_type);
                    let effect = effects.as_ref().map_or(wpk::records::NONE, |fx| {
                        match fx.weapon(weapon, effects::EXPLOSION) {
                            wpk::records::NONE => fx.impact(impact_type, surface, false),
                            e => e,
                        }
                    });
                    match effects.as_mut() {
                        Some(fx) if effect != wpk::records::NONE => {
                            fx.play(effect, origin, [0.0, 0.0, 1.0], now);
                        }
                        _ => {
                            blasts[next_blast] = (Some(origin), now);
                            next_blast = (next_blast + 1) % blasts.len();
                        }
                    }
                    devlog::event(now, "explosion", format_args!(""));
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
        if let Some(a) = arena.as_ref() {
            sections[section::BOT_THINK] = a.bot_cost.think_us;
            sections[section::BOT_MOVE] = a.bot_cost.move_us;
            sections[section::BOT_GUN] = a.bot_cost.gun_us;
        }
        let stats = world.draw(gfx, &cam, scratch);
        // Destructible objects, as part of the world.
        if let (Some(p), Some(a)) = (props.as_ref(), arena.as_ref()) {
            p.draw(gfx, &cam.view(), cam.eye, cam.forward(), &a.objects);
        }
        sections[section::WORLD] = lap(&mut mark);
        // The other players' bodies (timed with the placeholder boxes as `bod`).
        if let (Some(ch), Some(a), true) = (characters.as_mut(), arena.as_ref(), debug.characters) {
            poses.clear();
            // Living players, and the dead until they respawn (their death animations).
            for (i, f) in a.fighters.iter().enumerate().skip(1) {
                let team = match f.team {
                    arena::TEAM_AXIS => 1,
                    arena::TEAM_ALLIES => 0,
                    _ => i % 2,
                };
                poses.push(character::Pose::of(f, i, team, now));
            }
            ch.draw(gfx, &cam.view(), &poses, now);
        }
        // Bots: placeholder bodies, one shaded box per hit volume, until the character models are converted.
        if let Some(a) = arena.as_ref() {
            boxes.clear();
            box_colours.clear();
            // Grenades: small dark boxes until the projectile models are converted; explosions: a short bright
            // flash growing from the blast point (placeholder for MW2's explosion effect).
            for g in &a.grenades {
                let o = g.origin;
                boxes.push([
                    o[0] - 2.0,
                    o[1] - 2.0,
                    o[2] - 2.0,
                    o[0] + 2.0,
                    o[1] + 2.0,
                    o[2] + 2.0,
                ]);
                box_colours.push(0x303828ff);
            }
            for (o, t) in blasts.iter().filter_map(|(o, t)| o.map(|o| (o, *t))) {
                let age = now.wrapping_sub(t);
                if age < 250 {
                    let r = 20.0 + age as f32 * 0.4;
                    boxes.push([
                        o[0] - r,
                        o[1] - r,
                        o[2] - r * 0.5,
                        o[0] + r,
                        o[1] + r,
                        o[2] + r,
                    ]);
                    box_colours.push(0xffc060ff);
                }
            }
            for (i, f) in a
                .fighters
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(_, f)| f.life.alive() && (characters.is_none() || !debug.characters))
            {
                let ally = !a.enemies(0, i);
                for h in game_core::damage::boxes(f.draw_origin(now), f.player.hull_height()) {
                    boxes.push([
                        h.mins[0], h.mins[1], h.mins[2], h.maxs[0], h.maxs[1], h.maxs[2],
                    ]);
                    box_colours.push(match (ally, h.loc <= game_core::damage::hitloc::NECK) {
                        (true, true) => 0xb0c0d8ff,
                        (true, false) => 0x46607cff,
                        (false, true) => 0xd8b090ff,
                        (false, false) => 0x6a7050ff,
                    });
                }
            }
            // Smoke: each particle a see-through grey box of its size and alpha (placeholder for MW2's smoke
            // sprites; a third of the particle's alpha, as the boxes overlap heavily).
            for p in &a.smoke.particles {
                if let Some((r, alpha)) = p.state(now) {
                    let o = p.origin;
                    let h = r * 0.8;
                    boxes.push([o[0] - h, o[1] - h, o[2], o[0] + h, o[1] + h, o[2] + 2.0 * h]);
                    box_colours.push(0xa8a8a000 | u32::from(alpha / 3).min(254));
                }
            }
            // Debug overlays (see-through boxes): hit boxes, bot goals, objectives, your recent bullet paths.
            if debug.show_hitboxes {
                for (_, f) in a
                    .fighters
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter(|(_, f)| f.life.alive())
                {
                    for h in game_core::damage::boxes(f.draw_origin(now), f.player.hull_height()) {
                        boxes.push([
                            h.mins[0], h.mins[1], h.mins[2], h.maxs[0], h.maxs[1], h.maxs[2],
                        ]);
                        box_colours.push(if h.loc <= game_core::damage::hitloc::NECK {
                            0xff404070
                        } else {
                            0x40a0ff50
                        });
                    }
                }
            }
            if debug.show_bot_goals {
                for f in a.fighters.iter().skip(1).filter(|f| f.life.alive()) {
                    if let Some(g) = f.brain.as_ref().and_then(|b| b.goal()) {
                        boxes.push([
                            g[0] - 4.0,
                            g[1] - 4.0,
                            g[2],
                            g[0] + 4.0,
                            g[1] + 4.0,
                            g[2] + 96.0,
                        ]);
                        box_colours.push(0xffe040c0);
                    }
                }
            }
            if debug.show_objectives {
                for o in &objectives {
                    let owner = a
                        .dom
                        .as_ref()
                        .and_then(|d| d.flags.iter().find(|f| f.label == o.label))
                        .map_or(0, |f| f.owner);
                    let r = o.radius;
                    boxes.push([
                        o.origin[0] - r,
                        o.origin[1] - r,
                        o.origin[2],
                        o.origin[0] + r,
                        o.origin[1] + r,
                        o.origin[2] + o.height,
                    ]);
                    box_colours.push(match owner {
                        arena::TEAM_ALLIES => 0x4080ff40,
                        arena::TEAM_AXIS => 0xff404040,
                        _ => 0xe0e0e040,
                    });
                }
            }
            if debug.show_shots {
                for (s, t) in shot_trails.iter().filter_map(|(s, t)| s.map(|s| (s, *t))) {
                    if now.wrapping_sub(t) > 3000 {
                        continue;
                    }
                    let (a0, b0) = s;
                    let d: [f32; 3] = core::array::from_fn(|k| b0[k] - a0[k]);
                    let len = libm::sqrtf(d.iter().map(|x| x * x).sum());
                    let n = ((len / 32.0) as usize).clamp(1, 96);
                    for k in 1..=n {
                        let f = k as f32 / n as f32;
                        let p: [f32; 3] = core::array::from_fn(|j| a0[j] + d[j] * f);
                        boxes.push([
                            p[0] - 1.0,
                            p[1] - 1.0,
                            p[2] - 1.0,
                            p[0] + 1.0,
                            p[1] + 1.0,
                            p[2] + 1.0,
                        ]);
                        box_colours.push(0x40fff0c0);
                    }
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
        // MW2's effects in the world (impacts, explosions, the others' muzzle flashes).
        if let Some(fx) = effects.as_mut() {
            fx.draw_world(gfx, &cam, now);
        }
        sections[section::BODIES] = lap(&mut mark);
        if !flying
            && life.alive()
            && let (Some(v), Some(g), Some(p)) = (
                arsenal.views[arsenal.view_slot()].as_mut(),
                arsenal.guns[arsenal.active].as_ref(),
                player.as_ref(),
            )
        {
            let fpv = wpk::records::get::<wpk::weapon::FpvRecord>(fpvs, g.weapon_index() as usize)
                .unwrap_or_default();
            v.draw(gfx, g, &p.ps, &fpv, dt, now);
            v.play_notes();
            // The first-person muzzle flash: MW2's view flash effect; without effects, a placeholder glow for
            // 50 ms after a shot.
            if let Some(fx) = effects.as_mut() {
                fx.draw_view(gfx, now);
            } else if last_shot_ms.is_some_and(|t| now.wrapping_sub(t) < 50)
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
            // MW2's HUD (its fonts and images) when the common pack has the UI; else the plain one.
            let mw2 = ui::get().is_some() && arena.is_some();
            if let Some(g) = arsenal.gun().filter(|_| life.alive()) {
                draw_gun_hud(gfx, &mut hud_rects, g, &cam, &impacts, now, !mw2);
            }
            draw_life_hud(gfx, &mut hud_rects, &life, &clock, now, !mw2);
            if let Some(a) = arena.as_ref() {
                // Points just scored pop up at the crosshair.
                let score = a.fighters[0].score;
                if score > last_score {
                    popup = Some((score - last_score, now));
                }
                last_score = score;
                if !mw2 {
                    draw_match_hud(gfx, &mut hud_rects, a, &feed, hitmarker_ms, now);
                }
            }
            if let (true, Some(u), Some(a)) = (mw2, ui::get(), arena.as_ref()) {
                let mut objective_line = Str::default();
                // The score bar: yours (your team's) and the best other player's (the enemy team's).
                let score_pair = if a.mode.teams() {
                    let mine = usize::from(a.fighters[0].team);
                    (a.team_score[mine] as i32, a.team_score[3 - mine] as i32)
                } else {
                    let others: Vec<i32> = a.fighters[1..].iter().map(|f| f.score as i32).collect();
                    (
                        a.fighters[0].score as i32,
                        hud_iw4::ffa_scorebar_lead(&others),
                    )
                };
                if a.mode.teams() {
                    let mine = usize::from(a.fighters[0].team);
                    if let Some(d) = a.dom.as_ref() {
                        for f in &d.flags {
                            let mut l = [0u8; 4];
                            objective_line.push_str(f.letter().encode_utf8(&mut l));
                            objective_line.push_str(match f.owner {
                                0 => " NEUTRAL  ",
                                t if usize::from(t) == mine => " OURS  ",
                                _ => " THEIRS  ",
                            });
                        }
                        let you = a.fighters[0].player.ps.origin;
                        if let Some(f) = d
                            .flags
                            .iter()
                            .find(|f| usize::from(f.claim) == mine && f.contains(you))
                        {
                            objective_line.push_str("SECURING ");
                            objective_line.num(
                                (f.progress_ms * 100.0 / a.rules.dom_use_ms.max(1) as f32) as u32,
                            );
                            objective_line.push('%');
                        }
                    }
                }
                let mode_name = front::mode_name(a.mode).to_ascii_uppercase();
                let mut feed_lines: [Str; 4] = Default::default();
                let mut feed_mine = [false; 4];
                let mut n_feed = 0;
                for (e, t) in &feed {
                    let Some((killer, victim)) = e else { continue };
                    if now.wrapping_sub(*t) > 4000 {
                        continue;
                    }
                    let l = &mut feed_lines[n_feed];
                    if killer == victim {
                        l.push_str(fighter_name(*victim).as_str());
                        l.push_str(" DIED");
                    } else {
                        l.push_str(fighter_name(*killer).as_str());
                        l.push_str(" KILLED ");
                        l.push_str(fighter_name(*victim).as_str());
                    }
                    feed_mine[n_feed] = *killer == 0 || *victim == 0;
                    n_feed += 1;
                }
                let feed_refs: Vec<(&str, bool)> = (0..n_feed)
                    .map(|k| (feed_lines[k].as_str(), feed_mine[k]))
                    .collect();
                let gun = arsenal.gun().filter(|_| life.alive());
                let mut display = Str::default();
                let weapon = gun.map(|g| {
                    let base = g.name().split('_').next().unwrap_or("");
                    match loadouts
                        .catalog
                        .and_then(|c| c.weapons.iter().find(|w| w.name == base))
                    {
                        Some(w) => display.push_str(&w.display),
                        None => display.push_str(&base.to_ascii_uppercase()),
                    }
                    let (clip, stock) = g.ammo();
                    (
                        g.name(),
                        clip,
                        stock,
                        g.facts().clip_size,
                        g.facts().weap_class,
                    )
                });
                let mut grenades: [Option<(&str, i32)>; 2] = [None; 2];
                if let Some(g) = gun {
                    for (k, (w, n)) in g.offhand_ammo().take(2).enumerate() {
                        grenades[k] = weapon_names.get(w as usize).map(|name| (name.as_str(), n));
                    }
                }
                let state = game_core::hud_mw2::State {
                    now,
                    match_ms: clock.elapsed_ms(now),
                    safe_area: options.safe_area(),
                    weapon: weapon.map(|(name, clip, stock, clip_size, weap_class)| {
                        game_core::hud_mw2::Weapon {
                            name,
                            display: display.as_str(),
                            clip,
                            stock,
                            clip_size,
                            weap_class,
                        }
                    }),
                    grenades,
                    score: (score_pair.0, score_pair.1, a.limits.score_limit),
                    teams: a.mode.teams(),
                    mode_name: &mode_name,
                    objectives: objective_line.as_str(),
                    clock_ms: clock.left_ms(now),
                    feed: &feed_refs,
                    hitmarker: hitmarker_ms.is_some_and(|t| now.wrapping_sub(t) < 150),
                    popup,
                    reward: rewards.last().map(|r| r.as_str()),
                    notice: reward_note,
                    respawn_in: (!life.alive()).then(|| life.respawn_in(now).div_ceil(1000)),
                    nuke_in: a
                        .nuke
                        .filter(|n| !n.done)
                        .map(|n| n.detonate_ms.saturating_sub(now).div_ceil(1000)),
                };
                u.page(|ui, q| game_core::hud_mw2::layout(ui, &state, q));
            } // The UAV's sweep, then the minimap.
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
            draw_minimap(
                &world,
                arena.as_ref(),
                &radar,
                &cam,
                &blasts,
                now,
                options.safe_area(),
            );
            // A flashbang's or stun's white-out (IW4L `shellshock_flash_blend` with the shock's fade times).
            if let Some(a) = arena.as_ref()
                && let Some(s) = a.fighters[0].shock
            {
                let fx = s.fx(&a.rules);
                if fx.screen_type == hud_iw4::SCREEN_BLEND_FLASHED
                    && let Some((white, _)) = hud_iw4::shellshock_flash_blend(
                        s.remaining(now) as i32,
                        fx.white_fade_ms,
                        fx.shot_fade_ms,
                    )
                {
                    let alpha = (white.clamp(0.0, 1.0) * 255.0) as u32;
                    gfx.hud_rects(&[[0, 0, 640, 480]], 0xffff_ff00 | alpha);
                }
            }
            if let Some((text, at)) =
                reward_note.filter(|(_, at)| now.wrapping_sub(*at) < 3000 && !mw2)
            {
                let _ = at;
                hud_rects.clear();
                hud::text(&mut hud_rects, 200, 360, 2, text);
                gfx.hud_rects(&hud_rects, 0xffd040e0);
            }
            if let Some(r) = rewards.last().filter(|_| !mw2) {
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
        // MW2 asks for a class as you join the match: its class menu over the first frame (B keeps the last one).
        if ask_class {
            ask_class = false;
            unsafe { ffi::mwf_snd_stop_all() };
            front::grab_screen();
            if let Some(c) = front::pick_class(
                gfx,
                input,
                loadouts.catalog,
                &front::choose_class_title(),
                loadouts.classes,
                loadouts.cursor,
                Some(&front::grabbed),
            ) {
                devlog::event(ffi::ms(), "class_change", format_args!("class={c}"));
                class = Some(c);
                perk_set = equip(&mut arsenal, gfx, class, debug.primary);
            }
            // The match waited: no time passed.
            last = ffi::ms();
            frame_start = ffi::us();
            continue;
        }
        let end = ffi::us();
        timing.add(end, end.wrapping_sub(frame_start), cpu_us, gfx.gpu_wait_us);
        auto_distance.update(
            end.wrapping_sub(frame_start),
            gfx.gpu_wait_us,
            sections[section::WORLD],
        );
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
    // A test script, when `autoplay.txt` is in the app folder.
    autoplay::init();
    // MW2's menu sounds (from common.wpk).
    front::init_menu_sounds(common);
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
    // The player's killstreaks (one set for every class, as in MW2).
    let mut streaks = catalog
        .as_ref()
        .map_or([wpk::classes::NONE; 3], front::load_streaks);
    let mut input = front::Input::default();
    let (mut main_sel, mut map_sel, mut class_sel, mut edit_sel) = (0usize, 0usize, 0usize, 0usize);
    // The match setup, kept between matches; the rules from the user's scripts.
    let rules = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::RULES))
        .and_then(|s| wpk::rules::Rules::decode(s.data))
        .unwrap_or_default();
    let mut mode = arena::Mode::Ffa;
    let mut limits = arena::Limits::of(&rules, mode);
    // Debug settings (`debug.dat`), kept between runs.
    let mut debug = front::load_debug();
    sound::MUTED.store(!debug.sound, core::sync::atomic::Ordering::Relaxed);
    let mut debug_sel = 0usize;
    // The player's options (`options.dat`).
    let mut options = front::load_options();
    loop {
        match front::main_menu(&mut gfx, &mut input, &mut main_sel, catalog.is_some()) {
            front::Main::Quit => return 0,
            front::Main::Options => {
                front::options_menu(&mut gfx, &mut input, &mut options, false);
                front::save_options(&options);
            }
            front::Main::Debug => {
                let cat = catalog.as_ref();
                let weapon_name = |i: u8| {
                    cat.and_then(|c| c.weapons.get(usize::from(i)))
                        .map(|w| w.name.to_ascii_uppercase())
                };
                let count = cat.map_or(0, |c| c.weapons.len().min(254) as u8);
                // No sandbox actions outside a match: the menu only returns on B.
                front::debug_menu(
                    &mut gfx,
                    &mut input,
                    &mut debug,
                    &weapon_name,
                    count,
                    false,
                    &mut debug_sel,
                );
                front::save_debug(&debug);
                sound::MUTED.store(!debug.sound, core::sync::atomic::Ordering::Relaxed);
            }
            front::Main::CreateAClass => {
                if let Some(cat) = catalog.as_ref() {
                    front::create_a_class(&mut gfx, &mut input, cat, &mut custom, &mut edit_sel);
                }
            }
            // The Xbox 360's private match: the lobby (start, game setup, Create-a-Class), the match with the class
            // last chosen, then back to the lobby.
            front::Main::PrivateMatch => {
                let mut names = [0u8; 2048];
                let maps = list_maps(&mut names);
                let mut lobby_sel = 0usize;
                loop {
                    let players = |m: arena::Mode, bots: usize| -> Vec<front::LobbyRow> {
                        (0..=bots)
                            .map(|i| {
                                let ally = i == 0 || i % 2 == 0;
                                let (info, rgba) = match m {
                                    arena::Mode::Ffa if i == 0 => ("", 0xffd8_40ff),
                                    arena::Mode::Ffa => ("BOT", 0xffff_ffff),
                                    _ if ally => ("ALLIES", 0x9cc8_ffff),
                                    _ => ("OPFOR", 0xff9c_8cff),
                                };
                                front::LobbyRow {
                                    name: alloc::string::String::from(fighter_name(i).as_str()),
                                    info: alloc::string::String::from(info),
                                    rgba,
                                }
                            })
                            .collect()
                    };
                    let mut setup = front::Setup {
                        maps: &maps,
                        map: &mut map_sel,
                        mode: &mut mode,
                        limits: &mut limits,
                        rules: &rules,
                        debug: &mut debug,
                    };
                    match front::lobby(&mut gfx, &mut input, &mut setup, &players, &mut lobby_sel) {
                        front::Lobby::Back => break,
                        front::Lobby::CreateAClass => {
                            if let Some(cat) = catalog.as_ref() {
                                front::create_a_class(
                                    &mut gfx,
                                    &mut input,
                                    cat,
                                    &mut custom,
                                    &mut edit_sel,
                                );
                            }
                            continue;
                        }
                        front::Lobby::Playercard => {
                            if let Some(cat) = catalog.as_ref() {
                                front::playercard(&mut gfx, &mut input, cat, &mut streaks);
                            }
                            continue;
                        }
                        front::Lobby::Start => {}
                    }
                    // The bots' count and skill are set in Game Rules.
                    front::save_debug(&debug);
                    // Custom classes first, then MW2's defaults, every one with the player's killstreaks; the match
                    // asks for the class first, on the one last chosen.
                    let classes: Vec<wpk::classes::Loadout> = catalog
                        .as_ref()
                        .map(|c| {
                            custom
                                .iter()
                                .chain(&c.defaults)
                                .map(|l| wpk::classes::Loadout {
                                    killstreaks: streaks,
                                    ..l.clone()
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let class = (!classes.is_empty()).then(|| class_sel.min(classes.len() - 1));
                    let Some(&map) = maps.get(map_sel) else {
                        continue;
                    };
                    let mark = unsafe { ffi::mwf_mem2_mark() };
                    let exit = run_map(
                        &mut gfx,
                        map,
                        scratch,
                        common,
                        Loadouts {
                            catalog: catalog.as_ref(),
                            classes: &classes,
                            class,
                            cursor: &mut class_sel,
                            ask: true,
                        },
                        (mode, limits),
                        &mut input,
                        &mut debug,
                        &mut options,
                    );
                    unsafe { ffi::mwf_snd_stop_all() };
                    unsafe { ffi::mwf_mem2_release(mark) };
                    if let Exit::Quit = exit {
                        return 0;
                    }
                    input.settle();
                }
            }
        }
    }
}

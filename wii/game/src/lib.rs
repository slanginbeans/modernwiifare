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
mod ffi;
mod front;
mod rt;
mod sound;
mod view;

use alloc::vec::Vec;

use dpvs_iw4::WalkScratch;
use game_core::camera::Camera;
use game_core::collision::MapClip;
use game_core::fly::{self, PadInput};
use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::gun::{Gun, GunInput, SpreadInput};
use game_core::hud::{self, Rect, heapless_str::Str};
use game_core::life::{self, Life, MatchClock};
use game_core::player::{MovePad, Player};
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
/// The classes a match can spawn with.
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
    let (vis, clip_data) = {
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
        (vis, clip)
    };
    let mut world = match World::load(bytes, vis.as_ref(), gfx) {
        Ok(w) => w,
        Err(game_core::world::LoadError::TooManyTextures) => return fail(0xe08020ff),
        Err(_) => return fail(0xc020c0ff),
    };
    let clip: Option<MapClip<'_>> = clip_data.as_ref().map(MapClip::new);
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
    let kicks = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::KICK))
        .map_or(&[][..], |s| s.data);
    let mut arsenal = arsenal::Arsenal::new();
    // A class change applies at the next spawn, as in MW2.
    let mut class = loadouts.class;
    let equip = |arsenal: &mut arsenal::Arsenal, gfx: &mut Shim, class: Option<usize>| {
        let l = class.and_then(|c| loadouts.classes.get(c));
        arsenal.equip(gfx, weapons, kicks, loadouts.catalog, l);
    };
    equip(&mut arsenal, gfx, class);
    let mut impacts: [(Option<[f32; 3]>, u32); 8] = [(None, 0); 8];
    let mut last_shot_ms: Option<u32> = None;
    let mut next_impact = 0usize;
    let mut hud_rects: Vec<Rect> = Vec::with_capacity(512);
    let mut life = Life::default();
    let clock = MatchClock::start(ffi::ms());
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
    let mut timing = Timing::default();
    let mut rects: Vec<Rect> = Vec::with_capacity(4096);
    let mut frame_start = ffi::us();
    loop {
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
            match front::pause(
                gfx,
                input,
                (!loadouts.classes.is_empty()).then_some(loadouts.classes),
                loadouts.cursor,
            ) {
                front::Pause::EndGame => return Exit::Menu,
                front::Pause::ChangeClass(c) => class = Some(c),
                front::Pause::Resume => {}
            }
            // The match was stopped: no time passed.
            last = ffi::ms();
            frame_start = ffi::us();
            continue;
        }
        // Y: the other weapon (MW2's console layout).
        if pad.down & ffi::BTN_Y != 0 && !flying && life.alive() {
            arsenal.switch();
        }
        // D-pad left: next spawn point (debug), with the class's weapons.
        if pad.down & ffi::BTN_LEFT != 0 {
            spawn += 1;
            cam = spawn_camera(&world, spawn);
            cam.aspect = aspect;
            if let Some(c) = &clip {
                player = Some(spawn_player(&world, c, spawn));
            }
            equip(&mut arsenal, gfx, class);
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
        match (&mut player, &clip) {
            (Some(p), Some(c)) if !flying && life.alive() => {
                let fall = p.update(
                    c,
                    &MovePad {
                        sx: pad.sx,
                        sy: pad.sy,
                        cx: pad.cx,
                        cy: pad.cy,
                        jump: pad.held & ffi::BTN_A != 0,
                        crouch: pad.held & ffi::BTN_B != 0,
                        sprint: pad.held & ffi::BTN_Z != 0,
                        ads: pad.held & ffi::BTN_L != 0 || pad.l > 128,
                    },
                    msec as i32,
                );
                cam = p.camera(aspect);
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
                let a = arsenal.active;
                if let (Some(g), true) = (arsenal.guns[a].as_mut(), life.alive()) {
                    g.update_spread(
                        msec as i32,
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
                        msec as i32,
                        &GunInput {
                            fire: pad.held & ffi::BTN_R != 0 || pad.r > 100,
                            reload: pad.held & ffi::BTN_X != 0,
                            pm_flags: p.ps.pm_flags,
                        },
                        cam.eye,
                        cam.forward(),
                        cam.right(),
                        cam.up(),
                    );
                    if shot.is_some() {
                        last_shot_ms = Some(now);
                    }
                    if let (Some(e), Some(v)) = (event.as_ref(), arsenal.views[a].as_mut())
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
            }
            (Some(_), Some(c)) if !flying => {
                // Dead: the view stays where the player fell until the respawn.
                if life.ready_to_respawn(now) {
                    spawn += 1;
                    player = Some(spawn_player(&world, c, spawn));
                    equip(&mut arsenal, gfx, class);
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
        let stats = world.draw(gfx, &cam, scratch);
        if !flying
            && life.alive()
            && let (Some(v), Some(g)) = (
                arsenal.views[arsenal.active].as_mut(),
                arsenal.guns[arsenal.active].as_ref(),
            )
        {
            let (slot, restart) = g.anim();
            v.draw(gfx, slot, restart, now, g.gun_kick());
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
        if !flying {
            if let Some(g) = arsenal.gun().filter(|_| life.alive()) {
                draw_gun_hud(gfx, &mut hud_rects, g, &cam, &impacts, now);
            }
            draw_life_hud(gfx, &mut hud_rects, &life, &clock, now);
        }
        if show_overlay {
            overlay(gfx, &mut rects, &timing, &stats, cam.eye);
        }
        let cpu_us = ffi::us().wrapping_sub(frame_start);
        gfx.frame_end();
        let end = ffi::us();
        timing.add(end, end.wrapping_sub(frame_start), cpu_us, gfx.gpu_wait_us);
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

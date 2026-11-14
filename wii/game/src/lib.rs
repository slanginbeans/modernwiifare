//! `mwf_game.dol`: load `sd:/modernwiifare/maps/<map>.wpk` and walk it with IW4L movement on the GameCube pad.
//!
//! Controls (port 1), walking: main stick move, C-stick look, A jump, B crouch (held), Z sprint, L ADS.
//! D-pad up toggles free fly (main stick, C-stick, L/R triggers down/up, B fast). Y next spawn, START exit.
//! Packs without collision start in free fly.
//! If the pack is missing or broken the screen shows a solid colour: red = no SD / no file, magenta = bad pack,
//! orange = out of memory or textures. START exits.
#![no_std]

extern crate alloc;

mod ffi;
mod rt;

use alloc::boxed::Box;

use alloc::vec::Vec;

use dpvs_iw4::WalkScratch;
use game_core::camera::Camera;
use game_core::collision::MapClip;
use game_core::fly::{self, PadInput};
use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::hud::{self, Rect, heapless_str::Str};
use game_core::player::{MovePad, Player};
use game_core::world::FrameStats;
use game_core::world::World;

const MAP_PATH: &core::ffi::CStr = c"sd:/modernwiifare/maps/mp_rust.wpk";

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
    fn world_arrays(&mut self, v: *const u8) {
        unsafe { ffi::mwf_world_arrays(v) }
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

/// Show a solid colour until START or RESET (no console while GX owns the screen).
fn fail(colour: u32) -> i32 {
    let cam = Camera::new([0.0; 3], 0.0, 0.0);
    loop {
        unsafe {
            ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, colour);
            ffi::mwf_frame_end();
        }
        let pad = ffi::pad0();
        if pad.down & ffi::BTN_START != 0 || ffi::should_exit() {
            return 1;
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

#[unsafe(no_mangle)]
pub extern "C" fn mwf_game_main() -> i32 {
    unsafe { ffi::mwf_gx_init(0x202020ff) };
    let mut size = 0u32;
    let ptr = unsafe { ffi::mwf_file_load(MAP_PATH.as_ptr(), &mut size) };
    if ptr.is_null() {
        return fail(0xc02020ff);
    }
    // The pack stays in MEM2 for the program's lifetime.
    let bytes: &'static [u8] = unsafe { core::slice::from_raw_parts(ptr, size as usize) };
    let mut gfx = Shim::default();
    // Visibility lives as long as the world; the portal walk's scratch (~0.2 MiB) is static, never on the stack.
    let vis = World::decode_vis(bytes);
    let scratch: &'static mut WalkScratch = unsafe { &mut *core::ptr::addr_of_mut!(WALK_SCRATCH) };
    let mut world = match World::load(bytes, vis.as_ref(), &mut gfx) {
        Ok(w) => w,
        Err(game_core::world::LoadError::TooManyTextures) => return fail(0xe08020ff),
        Err(_) => return fail(0xc020c0ff),
    };
    // Collision: decoded once into the MEM1 heap (traces are hot; MEM1 is the fast memory) and kept for the
    // program's lifetime.
    let clip: Option<MapClip<'static>> = wpk::Pack::parse(bytes, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::CLIP))
        .and_then(|s| wpk::clip::Clip::decode(s.data).ok())
        .map(|c| MapClip::new(Box::leak(Box::new(c))));
    let aspect = if unsafe { ffi::mwf_widescreen() } != 0 {
        16.0 / 9.0
    } else {
        4.0 / 3.0
    };
    let mut spawn = 0usize;
    let mut cam = spawn_camera(&world, spawn);
    cam.aspect = aspect;
    let mut player = clip.as_ref().map(|c| spawn_player(&world, c, spawn));
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
        if pad.down & ffi::BTN_START != 0 || ffi::should_exit() {
            return 0;
        }
        if pad.down & ffi::BTN_Y != 0 {
            spawn += 1;
            cam = spawn_camera(&world, spawn);
            cam.aspect = aspect;
            if let Some(c) = &clip {
                player = Some(spawn_player(&world, c, spawn));
            }
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
            (Some(p), Some(c)) if !flying => {
                p.update(
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
        let stats = world.draw(&mut gfx, &cam, scratch);
        if show_overlay {
            overlay(&mut gfx, &mut rects, &timing, &stats, cam.eye);
        }
        let cpu_us = ffi::us().wrapping_sub(frame_start);
        gfx.frame_end();
        let end = ffi::us();
        timing.add(end, end.wrapping_sub(frame_start), cpu_us, gfx.gpu_wait_us);
        frame_start = end;
    }
}

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

use game_core::camera::Camera;
use game_core::collision::MapClip;
use game_core::fly::{self, PadInput};
use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::player::{MovePad, Player};
use game_core::world::World;

const MAP_PATH: &core::ffi::CStr = c"sd:/modernwiifare/maps/mp_rust.wpk";

/// `Gfx` over the C shim.
struct Shim;

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
    fn frame_end(&mut self) {
        unsafe { ffi::mwf_frame_end() }
    }
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
    let mut gfx = Shim;
    let mut world = match World::load(bytes, &mut gfx) {
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
    loop {
        let now = ffi::ms();
        let msec = now.wrapping_sub(last).clamp(1, 100);
        last = now;
        let dt = msec as f32 / 1000.0;
        let pad = ffi::pad0();
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
        world.draw(&mut gfx, &cam);
    }
}

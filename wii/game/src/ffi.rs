//! Declarations matching `wii/platform/shim.h` (the parts the game uses). Keep the two in sync.

use core::ffi::c_char;

// The shim's `MWF_BTN_*` GameCube bits (one definition, in `game_core::controls::gc`).
pub use game_core::controls::gc::{
    A as BTN_A, B as BTN_B, DOWN as BTN_DOWN, START as BTN_START, UP as BTN_UP,
};

/// `mwf_ui_quad` (shim.h).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UiQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub s0: f32,
    pub t0: f32,
    pub s1: f32,
    pub t1: f32,
    pub rgba: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Pad {
    pub held: u16,
    pub down: u16,
    pub sx: i8,
    pub sy: i8,
    pub cx: i8,
    pub cy: i8,
    pub l: u8,
    pub r: u8,
    pub connected: u8,
    pub _pad: u8,
}

/// The shim's `mwf_x360`: an Xbox 360 wired pad over USB.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct X360 {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub connected: u8,
    pub _pad: [u8; 3],
}

const _: () = assert!(core::mem::size_of::<X360>() == 16);

#[repr(C)]
pub struct TexDesc {
    pub data: *const u8,
    pub alpha: *const u8,
    pub width: u16,
    pub height: u16,
    pub format: u8,
    pub levels: u8,
    pub wrap: u8,
    pub _pad: u8,
}

#[repr(C)]
pub struct DrawState {
    pub tex: i32,
    pub lightmap: i32,
    pub blend: u8,
    pub alpha_ref: u8,
    pub cull: u8,
    pub _pad: u8,
    pub tint: u32,
}

const _: () = assert!(core::mem::size_of::<Pad>() == 12);
const _: () = assert!(core::mem::size_of::<TexDesc>() == 16);
const _: () = assert!(core::mem::size_of::<DrawState>() == 16);

type Mtx34 = [[f32; 4]; 3];

unsafe extern "C" {
    pub fn mwf_should_exit() -> i32;
    pub fn mwf_ms() -> u32;
    pub fn mwf_us() -> u32;
    pub fn mwf_input_scan();
    pub fn mwf_pad_read(chan: i32, out: *mut Pad);
    pub fn mwf_x360_scan();
    pub fn mwf_x360_read(out: *mut X360);
    pub fn mwf_panic(msg: *const u8, len: usize) -> !;
    pub fn mwf_widescreen() -> i32;

    pub fn mwf_gx_init(clear_rgba: u32);

    pub fn mwf_file_load_range(path: *const c_char, offset: u32, len: u32) -> *const u8;
    pub fn mwf_file_read(path: *const c_char, offset: u32, len: u32, dst: *mut u8) -> i32;
    pub fn mwf_file_write(path: *const c_char, data: *const u8, len: u32) -> i32;
    pub fn mwf_file_append(path: *const c_char, data: *const u8, len: u32) -> i32;
    pub fn mwf_mkdir(path: *const c_char) -> i32;
    pub fn mwf_mem1_free_bytes() -> u32;
    pub fn mwf_mem2_free_bytes() -> u32;
    pub fn mwf_file_error() -> i32;
    pub fn mwf_app_dir() -> *const c_char;
    pub fn mwf_mem2_alloc(size: u32, align: u32) -> *mut u8;
    pub fn mwf_flush(p: *const u8, len: u32);
    pub fn mwf_ui_quads(tex: i32, q: *const UiQuad, count: u32);
    /// Quads with a texture coordinate per corner (4 vertices each).
    pub fn mwf_ui_verts(tex: i32, v: *const game_core::minimap::Vert, quads: u32);
    pub fn mwf_draw_boxes(
        view: &[[f32; 4]; 3],
        boxes: *const [f32; 6],
        rgba: *const u32,
        count: u32,
    );
    pub fn mwf_snd_init();
    pub fn mwf_snd_play(pcm: *const u8, bytes: u32, rate: u32, vol_l: u8, vol_r: u8) -> i32;
    pub fn mwf_snd_play_adpcm(clip: *const u8, bytes: u32, rate: u32, vol_l: u8, vol_r: u8) -> i32;
    pub fn mwf_snd_stop_all();
    pub fn mwf_snd_steals() -> u32;
    pub fn mwf_view_begin(fovy_deg: f32, znear: f32, zfar: f32);
    pub fn mwf_world_view();
    pub fn mwf_world_tint(rgba: u32);
    pub fn mwf_view_arrays_indexed(positions: *const u8, attributes: *const u8);
    pub fn mwf_pos_matrix_slot(m: *const Mtx34, slot: u32);
    pub fn mwf_fog(on: i32, start: f32, end: f32, rgba: u32);
    /// Effect sprites, 4 vertices a quad (`mwf_sprite_vert`).
    pub fn mwf_sprites(
        view: *const Mtx34,
        tex: i32,
        blend: u32,
        v: *const game_core::gfx::SpriteVert,
        quads: u32,
        world: i32,
    );
    pub fn mwf_view_arrays(positions: *const u8, attributes: *const u8);
    pub fn mwf_mem2_mark() -> u32;
    pub fn mwf_mem2_release(mark: u32);
    pub fn mwf_list_dir(dir: *const c_char, suffix: *const c_char, out: *mut u8, cap: u32) -> i32;
    pub fn mwf_tex_reset();
    pub fn mwf_tex_mark() -> i32;
    pub fn mwf_tex_release(mark: i32);
    pub fn mwf_tex_create(desc: *const TexDesc) -> i32;
    pub fn mwf_frame_begin(
        view: *const Mtx34,
        fovy_deg: f32,
        znear: f32,
        zfar: f32,
        clear_rgba: u32,
    );
    pub fn mwf_world_arrays(vertices: *const u8, stride: u32);
    pub fn mwf_world_state(s: *const DrawState);
    pub fn mwf_pos_matrix(m: *const Mtx34);
    pub fn mwf_world_desc(flags: u8);
    pub fn mwf_call_dl(dl: *const u8, len: u32);
    pub fn mwf_sky_face(view_rot: *const Mtx34, corners: *const [[f32; 5]; 4], tex: i32);
    pub fn mwf_hud_rects(rects: *const [i16; 4], count: u32, rgba: u32);
    pub fn mwf_frame_end() -> u32;
}

pub fn ms() -> u32 {
    unsafe { mwf_ms() }
}

pub fn us() -> u32 {
    unsafe { mwf_us() }
}

pub fn should_exit() -> bool {
    unsafe { mwf_should_exit() != 0 }
}

/// Scan the pads (once a frame): GameCube port 1 and the USB Xbox 360 pad.
pub fn pads() -> (game_core::controls::GcPad, game_core::controls::X360Pad) {
    let (mut p, mut x) = (Pad::default(), X360::default());
    unsafe {
        mwf_input_scan();
        mwf_pad_read(0, &mut p);
        mwf_x360_scan();
        mwf_x360_read(&mut x);
    }
    (
        game_core::controls::GcPad {
            held: p.held,
            down: p.down,
            sx: p.sx,
            sy: p.sy,
            cx: p.cx,
            cy: p.cy,
            l: p.l,
            r: p.r,
            connected: p.connected != 0,
        },
        game_core::controls::X360Pad {
            buttons: x.buttons,
            lt: x.lt,
            rt: x.rt,
            lx: x.lx,
            ly: x.ly,
            rx: x.rx,
            ry: x.ry,
            connected: x.connected != 0,
        },
    )
}

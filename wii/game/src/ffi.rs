//! Declarations matching `wii/platform/shim.h` (the parts the game uses). Keep the two in sync.

use core::ffi::c_char;

pub const BTN_LEFT: u16 = 0x0001;
pub const BTN_DOWN: u16 = 0x0004;
pub const BTN_UP: u16 = 0x0008;
pub const BTN_Z: u16 = 0x0010;
pub const BTN_L: u16 = 0x0040;
pub const BTN_A: u16 = 0x0100;
pub const BTN_X: u16 = 0x0400;
pub const BTN_R: u16 = 0x0020;
pub const BTN_B: u16 = 0x0200;
pub const BTN_Y: u16 = 0x0800;
pub const BTN_START: u16 = 0x1000;

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
    pub fn mwf_panic(msg: *const u8, len: usize) -> !;
    pub fn mwf_widescreen() -> i32;

    pub fn mwf_gx_init(clear_rgba: u32);

    pub fn mwf_file_load_range(path: *const c_char, offset: u32, len: u32) -> *const u8;
    pub fn mwf_file_read(path: *const c_char, offset: u32, len: u32, dst: *mut u8) -> i32;
    pub fn mwf_file_write(path: *const c_char, data: *const u8, len: u32) -> i32;
    pub fn mwf_file_error() -> i32;
    pub fn mwf_app_dir() -> *const c_char;
    pub fn mwf_mem2_alloc(size: u32, align: u32) -> *mut u8;
    pub fn mwf_flush(p: *const u8, len: u32);
    pub fn mwf_snd_init();
    pub fn mwf_snd_play(pcm: *const u8, bytes: u32, rate: u32, vol_l: u8, vol_r: u8) -> i32;
    pub fn mwf_snd_stop_all();
    pub fn mwf_view_begin(fovy_deg: f32, znear: f32, zfar: f32);
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

/// Scan the ports and return port 1.
pub fn pad0() -> Pad {
    let mut p = Pad::default();
    unsafe {
        mwf_input_scan();
        mwf_pad_read(0, &mut p);
    }
    p
}

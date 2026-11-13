//! Declarations matching `wii/platform/shim.h` (the parts the game uses). Keep the two in sync.

use core::ffi::c_char;

pub const BTN_UP: u16 = 0x0008;
pub const BTN_Z: u16 = 0x0010;
pub const BTN_L: u16 = 0x0040;
pub const BTN_A: u16 = 0x0100;
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
}

const _: () = assert!(core::mem::size_of::<Pad>() == 12);
const _: () = assert!(core::mem::size_of::<TexDesc>() == 16);
const _: () = assert!(core::mem::size_of::<DrawState>() == 12);

type Mtx34 = [[f32; 4]; 3];

unsafe extern "C" {
    pub fn mwf_should_exit() -> i32;
    pub fn mwf_ms() -> u32;
    pub fn mwf_input_scan();
    pub fn mwf_pad_read(chan: i32, out: *mut Pad);
    pub fn mwf_panic(msg: *const u8, len: usize) -> !;
    pub fn mwf_widescreen() -> i32;

    pub fn mwf_gx_init(clear_rgba: u32);

    pub fn mwf_file_load(path: *const c_char, size_out: *mut u32) -> *const u8;
    pub fn mwf_tex_create(desc: *const TexDesc) -> i32;
    pub fn mwf_frame_begin(
        view: *const Mtx34,
        fovy_deg: f32,
        znear: f32,
        zfar: f32,
        clear_rgba: u32,
    );
    pub fn mwf_world_arrays(vertices: *const u8);
    pub fn mwf_world_state(s: *const DrawState);
    pub fn mwf_call_dl(dl: *const u8, len: u32);
    pub fn mwf_sky_face(view_rot: *const Mtx34, corners: *const [[f32; 5]; 4], tex: i32);
    pub fn mwf_frame_end();
}

pub fn ms() -> u32 {
    unsafe { mwf_ms() }
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

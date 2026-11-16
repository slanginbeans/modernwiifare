//! Declarations matching `wii/platform/shim.h`. Keep the two in sync.

pub const BTN_LEFT: u16 = 0x0001;
pub const BTN_RIGHT: u16 = 0x0002;
pub const BTN_DOWN: u16 = 0x0004;
pub const BTN_UP: u16 = 0x0008;
pub const BTN_Z: u16 = 0x0010;
pub const BTN_A: u16 = 0x0100;
pub const BTN_B: u16 = 0x0200;
pub const BTN_X: u16 = 0x0400;
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
#[derive(Clone, Copy, Default)]
pub struct Mem {
    pub arena1_lo: u32,
    pub arena1_hi: u32,
    pub arena2_lo: u32,
    pub arena2_hi: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vtx {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rgba: u32,
}

const _: () = assert!(core::mem::size_of::<Pad>() == 12);
const _: () = assert!(core::mem::size_of::<Mem>() == 16);
const _: () = assert!(core::mem::size_of::<Vtx>() == 16);

/// `mwf_gpu_metric`: one GPU counter set's last reading (`shim.h`).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct GpuMetric {
    pub c0: u32,
    pub c1: u32,
    pub frames: u32,
    pub us: u32,
    pub taken: u32,
}

/// `MWF_GPU_METRIC_SETS`.
pub const GPU_METRIC_SETS: usize = 4;

unsafe extern "C" {
    pub fn mwf_should_exit() -> i32;
    pub fn mwf_ms() -> u32;
    pub fn mwf_vsync();
    pub fn mwf_print(s: *const u8, len: usize);
    pub fn mwf_mem_info(out: *mut Mem);
    pub fn mwf_input_scan();
    pub fn mwf_pad_read(chan: i32, out: *mut Pad);
    pub fn mwf_pad_rumble(chan: i32, on: i32);
    pub fn mwf_ffi_echo(a: f32, b: f64, c: i32) -> f64;
    pub fn mwf_panic(msg: *const u8, len: usize) -> !;

    pub fn mwf_gx_init(clear_rgba: u32);
    pub fn mwf_gx_draw(mtx: *const [[f32; 4]; 3], v: *const Vtx, n: u32);
    pub fn mwf_gx_end_frame();
    pub fn mwf_gpu_wait_us() -> u32;
    pub fn mwf_set_frame_interval(retraces: u32);
    pub fn mwf_pace_wait_us() -> u32;
    pub fn mwf_gpu_metrics(out: *mut GpuMetric);
    pub fn mwf_gpu_idle();
    pub fn mwf_world_tint(rgba: u32);
    pub fn mwf_view_arrays_indexed(positions: *const u8, attributes: *const u8);
    pub fn mwf_pos_matrix_slot(m: *const [[f32; 4]; 3], slot: u32);
    pub fn mwf_ps_concat_each(
        a: *const [[f32; 4]; 3],
        b: *const [[f32; 4]; 3],
        out: *mut [[f32; 4]; 3],
        n: u32,
    );
    pub fn mwf_ps_skin(
        verts: *const u8,
        n: u32,
        skin: *const [[f32; 4]; 3],
        bones: u32,
        out: *mut [f32; 3],
        outs: u32,
    );
    pub fn mwf_ps_load_slots(
        eye: *const [[f32; 4]; 3],
        skin: *const [[f32; 4]; 3],
        bones: u32,
        keys: *const u16,
        n: u32,
        loaded: *mut u16,
    );
    pub fn mwf_fog(on: i32, start: f32, end: f32, rgba: u32);
    /// Effect sprites (`mwf_sprite_vert`: x, y, z, rgba, s, t), 4 vertices a quad.
    pub fn mwf_sprites(
        view: *const [[f32; 4]; 3],
        tex: i32,
        blend: u32,
        v: *const u8,
        quads: u32,
        world: i32,
    );
    /// Decals on the world (bullet marks), like `mwf_sprites` in the world, depth-biased towards the eye.
    pub fn mwf_decals(view: *const [[f32; 4]; 3], tex: i32, blend: u32, v: *const u8, quads: u32);

    pub fn mwf_net_init(ip: *mut u8, mask: *mut u8, gw: *mut u8) -> i32;
    pub fn mwf_udp_open(port: u16, broadcast_opt_result: *mut i32, bind_result: *mut i32) -> i32;
    pub fn mwf_udp_send(s: i32, ip: u32, port: u16, buf: *const u8, len: u32) -> i32;
    pub fn mwf_udp_recv(
        s: i32,
        buf: *mut u8,
        cap: u32,
        from_ip: *mut u32,
        from_port: *mut u16,
    ) -> i32;
}

// Safe wrappers for the calls with no preconditions.

pub fn ms() -> u32 {
    unsafe { mwf_ms() }
}

pub fn vsync() {
    unsafe { mwf_vsync() }
}

pub fn should_exit() -> bool {
    unsafe { mwf_should_exit() != 0 }
}

/// Scan all GameCube ports and return port 0.
pub fn pad0() -> Pad {
    let mut p = Pad::default();
    unsafe {
        mwf_input_scan();
        mwf_pad_read(0, &mut p);
    }
    p
}

pub fn mem_info() -> Mem {
    let mut m = Mem::default();
    unsafe { mwf_mem_info(&mut m) };
    m
}

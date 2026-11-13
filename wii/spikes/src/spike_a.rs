//! Spike A: does Rust on the Wii work end to end?
//!
//! 1. Self-tests on the text console: endianness, heap, u64 maths (compiler-builtins), f64 maths,
//!    hard-float argument passing across the C ABI, core::fmt.
//! 2. A GX scene: a spinning tetrahedron (4 triangles, per-vertex colour). The GameCube main stick
//!    rotates it, the C-stick moves it, A changes the palette, START or RESET exits.
//!    The clear colour is dark green if every self-test passed, dark red otherwise.

use alloc::{boxed::Box, format, vec::Vec};

use crate::ffi::{self, Vtx};
use crate::say;

struct Check {
    name: &'static str,
    ok: bool,
}

fn self_tests() -> Vec<Check> {
    let mut out = Vec::new();
    let mut check = |name: &'static str, ok: bool| out.push(Check { name, ok });

    check(
        "big-endian target",
        u32::from_ne_bytes([1, 2, 3, 4]) == 0x0102_0304,
    );

    let v: Vec<u32> = (0..100_000u32).collect();
    let sum: u64 = v.iter().map(|&x| x as u64).sum();
    check("heap: 100k-element Vec", sum == 4_999_950_000);
    let b = Box::new([7u8; 4096]);
    check("heap: 4 KB Box", b.iter().all(|&x| x == 7));

    let big: u64 = core::hint::black_box(1u64 << 40);
    check(
        "u64 divide (compiler-builtins)",
        big / 3 == 366_503_875_925 && big % 7 == 2,
    );

    let r2 = libm::sqrt(core::hint::black_box(2.0f64));
    check("f64 maths (libm)", (r2 * r2 - 2.0).abs() < 1e-12);
    let s = libm::sinf(core::hint::black_box(core::f32::consts::FRAC_PI_2));
    check("f32 maths (libm)", (s - 1.0).abs() < 1e-6);

    // Float args in FPRs and an int in a GPR, across the Rust (SysV) -> C (EABI) boundary.
    let echoed = unsafe { ffi::mwf_ffi_echo(1.5, 2.25, -4) };
    check("hard-float FFI args", echoed == -0.25);

    check(
        "core::fmt + alloc::format!",
        format!("{:>6.2}|{:#x}", core::hint::black_box(1.255f64), 255) == "  1.25|0xff",
    );

    out
}

fn rgba(r: u8, g: u8, b: u8) -> u32 {
    u32::from_be_bytes([r, g, b, 0xff])
}

const PALETTES: [[u32; 4]; 3] = [
    [0xE0_40_40_FF, 0x40_E0_40_FF, 0x40_60_E0_FF, 0xF0_F0_F0_FF],
    [0xF0_A0_20_FF, 0x20_A0_F0_FF, 0xA0_20_F0_FF, 0x20_20_20_FF],
    [0x80_80_80_FF, 0xC0_C0_C0_FF, 0x40_40_40_FF, 0xFF_FF_FF_FF],
];

/// Tetrahedron as 4 triangles (12 vertices) with the given corner colours.
fn tetra(pal: &[u32; 4]) -> [Vtx; 12] {
    let p = [
        (1.0, 1.0, 1.0),
        (-1.0, -1.0, 1.0),
        (-1.0, 1.0, -1.0),
        (1.0, -1.0, -1.0),
    ];
    let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];
    let mut out = [Vtx {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        rgba: 0,
    }; 12];
    for (f, face) in faces.iter().enumerate() {
        for (k, &i) in face.iter().enumerate() {
            let (x, y, z) = p[i];
            out[f * 3 + k] = Vtx {
                x,
                y,
                z,
                rgba: pal[i],
            };
        }
    }
    out
}

/// Model-view matrix (libogc `Mtx` layout: 3 rows x 4 cols, column vectors, translation in col 3).
fn model_view(yaw: f32, pitch: f32, tx: f32, ty: f32, dist: f32) -> [[f32; 4]; 3] {
    let (sy, cy) = (libm::sinf(yaw), libm::cosf(yaw));
    let (sp, cp) = (libm::sinf(pitch), libm::cosf(pitch));
    // R = Ry(yaw) * Rx(pitch)
    [
        [cy, sy * sp, sy * cp, tx],
        [0.0, cp, -sp, ty],
        [-sy, cy * sp, cy * cp, -dist],
    ]
}

/// Map a GameCube stick axis to -1..1 with a small deadzone.
fn axis(v: i8) -> f32 {
    let v = v as f32 / 80.0;
    if v.abs() < 0.12 {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

pub fn run() -> i32 {
    say!(
        "\x1b[2;0HModern Wiifare - Spike A: Rust toolchain + GX   build {}\n\n",
        crate::rt::BUILD_ID
    );
    let checks = self_tests();
    let all_ok = checks.iter().all(|c| c.ok);
    for c in &checks {
        say!("  [{}] {}\n", if c.ok { " OK " } else { "FAIL" }, c.name);
    }
    let m = ffi::mem_info();
    say!(
        "\n  MEM1 arena free: {} KB   MEM2 arena free: {} KB\n",
        m.arena1_hi.wrapping_sub(m.arena1_lo) / 1024,
        m.arena2_hi.wrapping_sub(m.arena2_lo) / 1024
    );
    say!(
        "\n  {}\n\n  Press A (or wait 10 s) to start the GX test.  START/RESET exits.\n",
        if all_ok {
            "ALL SELF-TESTS PASSED"
        } else {
            "SOME SELF-TESTS FAILED - photo this screen"
        }
    );

    let start = ffi::ms();
    loop {
        ffi::vsync();
        let pad = ffi::pad0();
        if pad.down & ffi::BTN_START != 0 || ffi::should_exit() {
            return 0;
        }
        if pad.down & ffi::BTN_A != 0 || ffi::ms().wrapping_sub(start) > 10_000 {
            break;
        }
    }

    unsafe {
        ffi::mwf_gx_init(if all_ok {
            rgba(0x10, 0x30, 0x18)
        } else {
            rgba(0x40, 0x10, 0x10)
        })
    };

    let (mut yaw, mut pitch, mut tx, mut ty) = (0.0f32, 0.4f32, 0.0f32, 0.0f32);
    let mut pal = 0usize;
    let mut verts = tetra(&PALETTES[pal]);
    let mut last = ffi::ms();
    loop {
        let pad = ffi::pad0();
        if pad.down & ffi::BTN_START != 0 || ffi::should_exit() {
            return 0;
        }
        if pad.down & ffi::BTN_A != 0 {
            pal = (pal + 1) % PALETTES.len();
            verts = tetra(&PALETTES[pal]);
        }
        let now = ffi::ms();
        let dt = (now.wrapping_sub(last).min(100)) as f32 / 1000.0;
        last = now;

        // Idle spin, plus stick control; C-stick pans.
        yaw += dt * (0.6 + 3.0 * axis(pad.sx));
        pitch += dt * 3.0 * axis(pad.sy);
        tx = (tx + dt * 2.0 * axis(pad.cx)).clamp(-2.5, 2.5);
        ty = (ty + dt * 2.0 * axis(pad.cy)).clamp(-1.8, 1.8);

        let mv = model_view(yaw, pitch, tx, ty, 5.0);
        unsafe {
            ffi::mwf_gx_draw(&mv, verts.as_ptr(), verts.len() as u32);
            ffi::mwf_gx_end_frame();
        }
    }
}

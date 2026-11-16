//! The bodies' skinning kernels on Broadway's paired-single unit (`platform/shim_ps.c`), with the same inputs as the
//! scalar reference in `game_core::skin_kernels`. [`self_check`] runs both once at load and logs how far apart
//! they are (`ps_check`), so a broken kernel shows in the first log.

use alloc::vec::Vec;

use game_core::gfx::Mtx34;
use game_core::skin_kernels::{self as sk, SkinVert};

use crate::ffi;

/// `out[i] = a[i] × b[i]` for the shorter of the two.
pub fn concat_each(a: &[Mtx34], b: &[Mtx34], out: &mut Vec<Mtx34>) {
    let n = a.len().min(b.len());
    out.clear();
    out.resize(n, [[0.0; 4]; 3]);
    unsafe { ffi::mwf_ps_concat_each(a.as_ptr(), b.as_ptr(), out.as_mut_ptr(), n as u32) };
}

/// Each vertex skinned into `out[v.out]` (`skin_kernels::skin`).
pub fn skin(verts: &[SkinVert], skin: &[Mtx34], out: &mut [[f32; 3]]) {
    unsafe {
        ffi::mwf_ps_skin(
            verts.as_ptr().cast(),
            verts.len() as u32,
            skin.as_ptr(),
            skin.len() as u32,
            out.as_mut_ptr(),
            out.len() as u32,
        )
    };
}

/// Load a draw's position matrices: slot `i` gets `eye × skin[keys[i]]` (`eye` alone for a vertex the CPU skinned,
/// `viewmodel::BLENDED`). `loaded` remembers each slot's key (start a model with [`UNLOADED`]).
pub fn load_slots(eye: &Mtx34, skin: &[Mtx34], keys: &[u16], loaded: &mut [u16; 10]) {
    unsafe {
        ffi::mwf_ps_load_slots(
            eye,
            skin.as_ptr(),
            skin.len() as u32,
            keys.as_ptr(),
            keys.len().min(10) as u32,
            loaded.as_mut_ptr(),
        )
    };
}

/// No slot loaded yet (a key no draw uses).
pub const UNLOADED: [u16; 10] = [u16::MAX - 1; 10];

/// The paired-single kernels against the scalar ones on `skin` matrices and `verts` (a model's own): the largest
/// difference in a matrix entry and in a skinned position.
pub fn self_check(world: &[Mtx34], inv_bind: &[Mtx34], verts: &[SkinVert]) -> (f32, f32) {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    concat_each(world, inv_bind, &mut a);
    sk::concat_each(world, inv_bind, &mut b);
    let mut dm = 0.0f32;
    for (x, y) in a.iter().zip(&b) {
        for r in 0..3 {
            for c in 0..4 {
                dm = dm.max((x[r][c] - y[r][c]).abs());
            }
        }
    }
    let outs = verts
        .iter()
        .map(|v| usize::from(v.out) + 1)
        .max()
        .unwrap_or(0);
    let (mut p, mut q) = (
        alloc::vec![[0.0f32; 3]; outs],
        alloc::vec![[0.0f32; 3]; outs],
    );
    skin(verts, &b, &mut p);
    sk::skin(verts, &b, &mut q);
    let mut dp = 0.0f32;
    for (x, y) in p.iter().zip(&q) {
        for k in 0..3 {
            dp = dp.max((x[k] - y[k]).abs());
        }
    }
    (dm, dp)
}

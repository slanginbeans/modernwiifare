//! The float kernels a body's skinning spends its time in, in the shape the Wii's paired-single versions take
//! (`wii/platform/shim_ps.c`: libogc's `ps_guMtxConcat` / `ps_guVecMultiply`, Broadway's two-wide float maths).
//! These are the scalar reference: the host runs them and their tests, and the Wii checks its own against them once
//! at load (`game::ps`). The inputs are laid out for C (`#[repr(C)]`, matrices as `[[f32; 4]; 3]` rows).

use alloc::vec::Vec;

use crate::gfx::Mtx34;
use crate::viewmodel::{apply, mul};

/// A vertex the CPU skins (more than one bone): its bind position, weights already scaled to sum to ~1 (unused ones
/// 0) and bones. `out` is where its skinned position goes in the position array.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinVert {
    pub pos: [f32; 3],
    pub weights: [f32; 4],
    pub bones: [u16; 4],
    pub out: u16,
    pub count: u16,
}

impl SkinVert {
    /// From a model vertex (weights out of 255, as `viewmodel::skin_into` reads them), written to `out`.
    pub fn of(v: &wpk::view::SkinVertex, out: u16) -> SkinVert {
        const W: f32 = 1.0 / 255.0;
        let mut s = SkinVert {
            pos: v.pos,
            out,
            ..SkinVert::default()
        };
        for k in 0..4 {
            if v.weights[k] != 0 {
                let c = usize::from(s.count);
                s.weights[c] = f32::from(v.weights[k]) * W;
                s.bones[c] = u16::from(v.bones[k]);
                s.count += 1;
            }
        }
        s
    }
}

/// `out[i] = a[i] × b[i]` (skin matrices: posed bones × inverse binds).
pub fn concat_each(a: &[Mtx34], b: &[Mtx34], out: &mut Vec<Mtx34>) {
    out.clear();
    out.extend(a.iter().zip(b).map(|(x, y)| mul(x, y)));
}

/// A draw's position matrices (`shim_ps.c`'s `mwf_ps_load_slots` loads these into GX): slot `i` gets
/// `eye × skin[keys[i]]`, or `eye` alone for a key past `skin` (`viewmodel::BLENDED`: vertices the CPU skinned).
pub fn slot_matrices(eye: &Mtx34, skin: &[Mtx34], keys: &[u16]) -> Vec<Mtx34> {
    keys.iter()
        .map(|&k| skin.get(usize::from(k)).map_or(*eye, |s| mul(eye, s)))
        .collect()
}

/// Each vertex's weighted blend of its bones' `skin` matrices applied to its bind position, into `out[v.out]`.
/// Vertices whose bones or slot are out of range are left alone.
pub fn skin(verts: &[SkinVert], skin: &[Mtx34], out: &mut [[f32; 3]]) {
    for v in verts {
        let mut m = [[0.0f32; 4]; 3];
        let mut ok = true;
        for k in 0..usize::from(v.count.min(4)) {
            let Some(s) = skin.get(usize::from(v.bones[k])) else {
                ok = false;
                break;
            };
            let w = v.weights[k];
            for r in 0..3 {
                for c in 0..4 {
                    m[r][c] += s[r][c] * w;
                }
            }
        }
        if let (true, Some(o)) = (ok, out.get_mut(usize::from(v.out))) {
            *o = apply(&m, v.pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewmodel::{IDENTITY, mtx};
    use alloc::vec;
    use wpk::view::SkinVertex;

    fn turn(deg: f32, t: [f32; 3]) -> Mtx34 {
        let h = deg.to_radians() * 0.5;
        mtx([0.0, 0.0, libm::sinf(h), libm::cosf(h)], t)
    }

    #[test]
    fn concat_matches_mul() {
        let a = vec![turn(30.0, [1.0, 2.0, 3.0]), turn(-75.0, [0.0, -4.0, 9.0])];
        let b = vec![turn(10.0, [5.0, 0.0, 0.0]), IDENTITY];
        let mut out = Vec::new();
        concat_each(&a, &b, &mut out);
        assert_eq!(out, vec![mul(&a[0], &b[0]), mul(&a[1], &b[1])]);
        let slots = slot_matrices(&a[0], &b, &[1, 0, crate::viewmodel::BLENDED]);
        assert_eq!(slots, vec![a[0], mul(&a[0], &b[0]), a[0]]);
    }

    /// The blended matrix gives what `viewmodel::skin_into` (each bone's position, then weighted) gives.
    #[test]
    fn skin_matches_skin_into() {
        let skin_m = vec![
            turn(0.0, [0.0; 3]),
            turn(40.0, [3.0, -1.0, 2.0]),
            turn(-90.0, [0.0, 7.0, -5.0]),
        ];
        let verts = [
            SkinVertex {
                pos: [10.0, -3.0, 40.0],
                bones: [1, 2, 0, 0],
                weights: [128, 127, 0, 0],
                ..SkinVertex::default()
            },
            SkinVertex {
                pos: [-6.0, 2.5, 12.0],
                bones: [0, 1, 2, 0],
                weights: [85, 85, 85, 0],
                ..SkinVertex::default()
            },
            SkinVertex {
                pos: [1.0, 1.0, 1.0],
                bones: [2, 0, 1, 0],
                weights: [200, 30, 25, 0],
                ..SkinVertex::default()
            },
        ];
        let vm = wpk::view::ViewModel {
            verts: verts.to_vec(),
            ..Default::default()
        };
        let mut want = vec![[0.0f32; 3]; verts.len()];
        crate::viewmodel::skin_into(&vm, &skin_m, &mut want);
        // Written in reverse order through `out`.
        let list: Vec<SkinVert> = verts
            .iter()
            .enumerate()
            .map(|(i, v)| SkinVert::of(v, (verts.len() - 1 - i) as u16))
            .collect();
        let mut got = vec![[0.0f32; 3]; verts.len()];
        skin(&list, &skin_m, &mut got);
        for i in 0..verts.len() {
            let g = got[verts.len() - 1 - i];
            for k in 0..3 {
                assert!((g[k] - want[i][k]).abs() < 1e-4, "{g:?} vs {:?}", want[i]);
            }
        }
        assert_eq!(list[1].count, 3);
        assert_eq!(list[1].bones[..3], [0, 1, 2]);
    }

    #[test]
    fn bad_bones_leave_the_vertex() {
        let v = SkinVert {
            pos: [1.0; 3],
            weights: [1.0, 0.0, 0.0, 0.0],
            bones: [9, 0, 0, 0],
            out: 0,
            count: 1,
        };
        let mut out = [[5.0f32; 3]];
        skin(&[v], &[IDENTITY], &mut out);
        assert_eq!(out, [[5.0; 3]]);
        let v = SkinVert {
            out: 3,
            bones: [0; 4],
            ..v
        };
        skin(&[v], &[IDENTITY], &mut out);
        assert_eq!(out, [[5.0; 3]]);
    }

    #[test]
    fn layout_is_what_c_reads() {
        assert_eq!(core::mem::size_of::<SkinVert>(), 40);
        assert_eq!(core::mem::size_of::<Mtx34>(), 48);
    }
}

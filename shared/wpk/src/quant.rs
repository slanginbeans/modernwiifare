//! Fixed-point ("compact") vertices: 18 bytes instead of the 28 of [`WorldVertex`].
//!
//! - Position: s16 × 3 at 1/8 unit (GX VAT fraction [`POS_FRAC`]), relative to an origin the renderer folds into the
//!   position matrix. Origins are multiples of [`ORIGIN_SNAP`] (itself a multiple of the step), so a vertex shared
//!   by two batches rounds to the same point in both: no cracks.
//! - Colour: RGBA8.
//! - UV: s16 × 2 after removing a whole-number offset (repeat-wrapped textures sample the same), with the fraction of
//!   the batch's vertex format: [`VTXFMT_UV12`] (±8, 1/4096), [`VTXFMT_UV10`] (±32, 1/1024), [`VTXFMT_UV8`] (±128,
//!   1/256). Clamped textures keep offset 0.
//! - Lightmap UV: u16 × 2, fraction 15 (as [`WorldVertex`]).
//!
//! A batch that does not fit (too large, or UVs out of range) stays in the float format ([`crate::records::WORLD_VTXFMT`]).

use alloc::vec::Vec;

use crate::be::{self, Push};
use crate::records::{Record, WorldVertex};

/// Position fraction bits (1/8 unit).
pub const POS_FRAC: u8 = 3;
/// Origins snap to multiples of this (units).
pub const ORIGIN_SNAP: f32 = 64.0;
/// GX vertex formats of compact vertices, by UV fraction bits.
pub const VTXFMT_UV12: u8 = 3;
pub const VTXFMT_UV10: u8 = 4;
pub const VTXFMT_UV8: u8 = 5;

/// UV fraction bits of a compact vertex format.
pub const fn uv_frac(vtxfmt: u8) -> u8 {
    match vtxfmt {
        VTXFMT_UV12 => 12,
        VTXFMT_UV10 => 10,
        _ => 8,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompactVertex {
    pub pos: [i16; 3],
    pub rgba: u32,
    pub uv: [i16; 2],
    pub lm: [u16; 2],
}

impl Record for CompactVertex {
    const LEN: usize = 18;
    fn write(&self, o: &mut Vec<u8>) {
        self.pos.iter().for_each(|&v| o.u16(v as u16));
        o.u32(self.rgba);
        self.uv.iter().for_each(|&v| o.u16(v as u16));
        self.lm.iter().for_each(|&v| o.u16(v));
    }
    fn read(b: &[u8]) -> Self {
        CompactVertex {
            pos: [
                be::u16(b, 0) as i16,
                be::u16(b, 2) as i16,
                be::u16(b, 4) as i16,
            ],
            rgba: be::u32(b, 6),
            uv: [be::u16(b, 10) as i16, be::u16(b, 12) as i16],
            lm: [be::u16(b, 14), be::u16(b, 16)],
        }
    }
}

/// How a batch's vertices are stored: its GX vertex format and, for compact vertices, the origin to add.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuantRecord {
    /// [`crate::records::WORLD_VTXFMT`] (float, `WVTX` / `MVTX`) or one of the compact formats (`WVTQ` / `MVTQ`).
    pub vtxfmt: u8,
    pub origin: [f32; 3],
}

impl QuantRecord {
    pub fn is_compact(&self) -> bool {
        self.vtxfmt != crate::records::WORLD_VTXFMT
    }
}

impl Record for QuantRecord {
    const LEN: usize = 16;
    fn write(&self, o: &mut Vec<u8>) {
        o.u8(self.vtxfmt);
        o.zeros(3);
        self.origin.iter().for_each(|&v| o.f32(v));
    }
    fn read(b: &[u8]) -> Self {
        QuantRecord {
            vtxfmt: b[0],
            origin: [be::f32(b, 4), be::f32(b, 8), be::f32(b, 12)],
        }
    }
}

/// The origin for vertices centred on `centre`.
pub fn snap_origin(centre: [f32; 3]) -> [f32; 3] {
    centre.map(|c| libm::roundf(c / ORIGIN_SNAP) * ORIGIN_SNAP)
}

/// A position on the 1/8-unit grid compact vertices use. Float vertices are snapped too, so a surface and a layer
/// drawn on it stay exactly coplanar whichever format each ends up in.
pub fn snap_position(p: [f32; 3]) -> [f32; 3] {
    let step = (1u32 << POS_FRAC) as f32;
    p.map(|c| libm::roundf(c * step) / step)
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        parent[i as usize] = parent[parent[i as usize] as usize];
        i = parent[i as usize];
    }
    i
}

/// Quantize `verts` (drawn by triangle list `tris`) around `origin`. Each connected piece gets its own whole-number
/// UV offset (repeat wrapping samples the same), so only a piece's own UV span limits the precision. `clamp_uv`: the
/// texture is clamped, so UVs may not be shifted. None when they don't fit (the caller keeps floats).
pub fn quantize(
    verts: &[WorldVertex],
    tris: &[u16],
    origin: [f32; 3],
    clamp_uv: bool,
) -> Option<(u8, Vec<CompactVertex>)> {
    if verts.is_empty() {
        return None;
    }
    let step = (1u32 << POS_FRAC) as f32;
    // Connected pieces (union-find over triangle edges).
    let mut parent: Vec<u32> = (0..verts.len() as u32).collect();
    for t in tris.chunks_exact(3) {
        let a = find(&mut parent, u32::from(t[0]));
        for &v in &t[1..] {
            let b = find(&mut parent, u32::from(v));
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let root: Vec<u32> = (0..verts.len() as u32)
        .map(|i| find(&mut parent, i))
        .collect();
    let mut lo = alloc::vec![[f32::MAX; 2]; verts.len()];
    let mut hi = alloc::vec![[f32::MIN; 2]; verts.len()];
    for (v, &r) in verts.iter().zip(&root) {
        for k in 0..2 {
            lo[r as usize][k] = lo[r as usize][k].min(v.uv[k]);
            hi[r as usize][k] = hi[r as usize][k].max(v.uv[k]);
        }
    }
    let shift_of = |r: u32| -> [f32; 2] {
        if clamp_uv {
            [0.0; 2]
        } else {
            core::array::from_fn(|k| libm::floorf((lo[r as usize][k] + hi[r as usize][k]) * 0.5))
        }
    };
    let reach = verts
        .iter()
        .zip(&root)
        .map(|(v, &r)| {
            let s = shift_of(r);
            (v.uv[0] - s[0]).abs().max((v.uv[1] - s[1]).abs())
        })
        .fold(0.0f32, f32::max);
    let vtxfmt = if reach < 7.99 {
        VTXFMT_UV12
    } else if reach < 31.99 {
        VTXFMT_UV10
    } else if reach < 127.99 {
        VTXFMT_UV8
    } else {
        return None;
    };
    let uv_scale = (1u32 << uv_frac(vtxfmt)) as f32;
    let fits = |x: f32| (-32768.0..=32767.0).contains(&x);
    let mut out = Vec::with_capacity(verts.len());
    for (v, &r) in verts.iter().zip(&root) {
        let shift = shift_of(r);
        let p: [f32; 3] = core::array::from_fn(|k| libm::roundf((v.pos[k] - origin[k]) * step));
        let t: [f32; 2] = core::array::from_fn(|k| libm::roundf((v.uv[k] - shift[k]) * uv_scale));
        if !p.iter().chain(&t).all(|&x| fits(x)) {
            return None;
        }
        out.push(CompactVertex {
            pos: p.map(|x| x as i16),
            rgba: v.rgba,
            uv: t.map(|x| x as i16),
            lm: v.lm,
        });
    }
    Some((vtxfmt, out))
}

/// Back to floats (the reference renderer; UVs come back shifted by the batch's whole-number offset).
pub fn dequantize(v: &CompactVertex, q: &QuantRecord) -> WorldVertex {
    let step = 1.0 / (1u32 << POS_FRAC) as f32;
    let uv_step = 1.0 / (1u32 << uv_frac(q.vtxfmt)) as f32;
    WorldVertex {
        pos: core::array::from_fn(|k| q.origin[k] + f32::from(v.pos[k]) * step),
        rgba: v.rgba,
        uv: v.uv.map(|t| f32::from(t) * uv_step),
        lm: v.lm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn vtx(p: [f32; 3], uv: [f32; 2]) -> WorldVertex {
        WorldVertex {
            pos: p,
            rgba: 0x11223344,
            uv,
            lm: [5, 6],
        }
    }

    #[test]
    fn shared_vertices_round_identically_in_any_batch() {
        let p = [1234.567, -987.06, 55.5];
        let a = snap_origin([1000.0, -1000.0, 0.0]);
        let b = snap_origin([1900.0, -400.0, 300.0]);
        let (_, qa) = quantize(&[vtx(p, [0.5, 0.5])], &[], a, false).unwrap();
        let (_, qb) = quantize(&[vtx(p, [0.5, 0.5])], &[], b, false).unwrap();
        let back = |q: &CompactVertex, o| {
            dequantize(
                q,
                &QuantRecord {
                    vtxfmt: VTXFMT_UV12,
                    origin: o,
                },
            )
            .pos
        };
        assert_eq!(back(&qa[0], a), back(&qb[0], b));
        assert!((back(&qa[0], a)[0] - p[0]).abs() <= 1.0 / 16.0);
    }

    #[test]
    fn separate_pieces_get_their_own_uv_offsets() {
        // Two triangles far apart in UV space: as one piece they'd need ±300, separately each fits in ±8.
        let v = vec![
            vtx([0.0; 3], [300.0, 0.0]),
            vtx([1.0, 0.0, 0.0], [300.5, 0.0]),
            vtx([0.0, 1.0, 0.0], [300.0, 0.5]),
            vtx([5.0, 0.0, 0.0], [-300.0, 0.0]),
            vtx([6.0, 0.0, 0.0], [-299.5, 0.0]),
            vtx([5.0, 1.0, 0.0], [-300.0, 0.5]),
        ];
        let (fmt, _) = quantize(&v, &[0, 1, 2, 3, 4, 5], [0.0; 3], false).unwrap();
        assert_eq!(fmt, VTXFMT_UV12);
        // Joined by a triangle they are one piece again and don't fit.
        assert!(quantize(&v, &[0, 1, 2, 3, 4, 5, 2, 3, 4], [0.0; 3], false).is_none());
        assert_eq!(snap_position([1.06, -2.0, 3.2]), [1.0, -2.0, 3.25]);
    }

    #[test]
    fn uv_ranges_pick_formats_and_round_trip() {
        let v = vec![vtx([0.0; 3], [10.25, 11.5]), vtx([1.0; 3], [12.75, 9.0])];
        let (fmt, q) = quantize(&v, &[], [0.0; 3], false).unwrap();
        assert_eq!(fmt, VTXFMT_UV12);
        let back = dequantize(
            &q[0],
            &QuantRecord {
                vtxfmt: fmt,
                origin: [0.0; 3],
            },
        );
        // Same texel modulo whole repeats.
        assert!(((back.uv[0] - 10.25).rem_euclid(1.0)).abs() < 1e-3);
        // One piece (a triangle) spanning ±100 needs the coarse format; ±300 doesn't fit.
        let wide = vec![
            vtx([0.0; 3], [-100.0, 0.0]),
            vtx([0.0; 3], [100.0, 0.0]),
            vtx([0.0; 3], [0.0, 1.0]),
        ];
        assert_eq!(
            quantize(&wide, &[0, 1, 2], [0.0; 3], false).unwrap().0,
            VTXFMT_UV8
        );
        let too_wide = vec![
            vtx([0.0; 3], [-300.0, 0.0]),
            vtx([0.0; 3], [300.0, 0.0]),
            vtx([0.0; 3], [0.0, 1.0]),
        ];
        assert!(quantize(&too_wide, &[0, 1, 2], [0.0; 3], false).is_none());
        // Positions beyond ±4096 units of the origin don't fit.
        assert!(quantize(&[vtx([5000.0, 0.0, 0.0], [0.0; 2])], &[], [0.0; 3], false).is_none());
        let mut b = Vec::new();
        q[0].write(&mut b);
        assert_eq!(b.len(), CompactVertex::LEN);
        assert_eq!(CompactVertex::read(&b), q[0]);
    }
}

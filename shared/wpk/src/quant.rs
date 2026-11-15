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

/// Display-list layout flags in [`QuantRecord::flags`] (0 in older packs: four u16 indices per vertex).
pub mod dl_flags {
    /// Indices are u8 (the batch has at most 256 vertices).
    pub const IDX8: u8 = 1;
    /// No colour index: every vertex colour is white, so the renderer uses a white colour register instead.
    pub const NO_COLOR: u8 = 2;
}

/// How a batch's vertices are stored: its GX vertex format and, for compact vertices, the origin to add.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuantRecord {
    /// [`crate::records::WORLD_VTXFMT`] (float, `WVTX` / `MVTX`) or one of the compact formats (`WVTQ` / `MVTQ`).
    pub vtxfmt: u8,
    /// [`dl_flags`]: how the batch's display list indexes its vertices.
    pub flags: u8,
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
        o.u8(self.flags);
        o.zeros(2);
        self.origin.iter().for_each(|&v| o.f32(v));
    }
    fn read(b: &[u8]) -> Self {
        QuantRecord {
            vtxfmt: b[0],
            flags: b[1],
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

/// Why a batch stays in floats after [`quantize_tiled`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatReason {
    /// A vertex is more than 4096 units from the batch origin.
    Position,
    /// A single triangle spans too many texture repeats for any compact UV format.
    TriangleUv,
    /// Splitting into UV tiles would need more than 65,536 vertices.
    TooManyVertices,
}

/// UV tile sizes [`quantize_tiled`] tries, in texture repeats (even whole numbers, so tile shifts are whole numbers).
/// Larger tiles duplicate fewer vertices; smaller ones leave room for wider triangles (a vertex sits at most half a
/// tile plus its triangle's span from the tile centre, which must stay under 128 repeats).
pub const UV_TILES: [f32; 3] = [96.0, 32.0, 8.0];

/// [`quantize`] for batches whose connected pieces span too many texture repeats (large tiled floors, roads, walls):
/// triangles are grouped into tiles ([`UV_TILES`]) by their UV centre, each tile gets its own whole-number UV
/// shift, and a vertex used by triangles in several tiles is duplicated per tile (same position; UVs differing by
/// whole numbers sample the same with repeat wrapping, so no seams). Returns the vertex format, the vertices and the
/// triangles re-indexed to them.
pub fn quantize_tiled(
    verts: &[WorldVertex],
    tris: &[u16],
    origin: [f32; 3],
) -> Result<(u8, Vec<CompactVertex>, Vec<u16>), FloatReason> {
    let mut last = Err(FloatReason::TriangleUv);
    for tile in UV_TILES {
        last = quantize_tiles_of(verts, tris, origin, tile);
        if last != Err(FloatReason::TriangleUv) {
            break;
        }
    }
    last
}

fn quantize_tiles_of(
    verts: &[WorldVertex],
    tris: &[u16],
    origin: [f32; 3],
    tile_size: f32,
) -> Result<(u8, Vec<CompactVertex>, Vec<u16>), FloatReason> {
    let step = (1u32 << POS_FRAC) as f32;
    let fits = |x: f32| (-32768.0..=32767.0).contains(&x);
    if verts
        .iter()
        .any(|v| !(0..3).all(|k| fits(libm::roundf((v.pos[k] - origin[k]) * step))))
    {
        return Err(FloatReason::Position);
    }
    // Tile of each triangle, and its shift (the tile's centre: a whole number).
    let mut map: alloc::collections::BTreeMap<(u16, i32, i32), u16> =
        alloc::collections::BTreeMap::new();
    let mut out_verts: Vec<(u16, [f32; 2])> = Vec::new();
    let mut out_tris = Vec::with_capacity(tris.len());
    for t in tris.chunks_exact(3) {
        let c: [f32; 2] = core::array::from_fn(|k| {
            t.iter().map(|&i| verts[usize::from(i)].uv[k]).sum::<f32>() / 3.0
        });
        let tile = c.map(|x| libm::floorf(x / tile_size) as i32);
        let shift = tile.map(|x| x as f32 * tile_size + tile_size * 0.5);
        for &i in t {
            let key = (i, tile[0], tile[1]);
            let n = match map.get(&key) {
                Some(&n) => n,
                None => {
                    if out_verts.len() > usize::from(u16::MAX) {
                        return Err(FloatReason::TooManyVertices);
                    }
                    let n = out_verts.len() as u16;
                    out_verts.push((i, shift));
                    map.insert(key, n);
                    n
                }
            };
            out_tris.push(n);
        }
    }
    let reach = out_verts
        .iter()
        .map(|&(i, s)| {
            let uv = verts[usize::from(i)].uv;
            (uv[0] - s[0]).abs().max((uv[1] - s[1]).abs())
        })
        .fold(0.0f32, f32::max);
    let vtxfmt = if reach < 7.99 {
        VTXFMT_UV12
    } else if reach < 31.99 {
        VTXFMT_UV10
    } else if reach < 127.99 {
        VTXFMT_UV8
    } else {
        return Err(FloatReason::TriangleUv);
    };
    let uv_scale = (1u32 << uv_frac(vtxfmt)) as f32;
    let mut out = Vec::with_capacity(out_verts.len());
    for &(i, shift) in &out_verts {
        let v = &verts[usize::from(i)];
        let p: [f32; 3] = core::array::from_fn(|k| libm::roundf((v.pos[k] - origin[k]) * step));
        let t: [f32; 2] = core::array::from_fn(|k| libm::roundf((v.uv[k] - shift[k]) * uv_scale));
        if !t.iter().all(|&x| fits(x)) {
            return Err(FloatReason::TriangleUv);
        }
        out.push(CompactVertex {
            pos: p.map(|x| x as i16),
            rgba: v.rgba,
            uv: t.map(|x| x as i16),
            lm: v.lm,
        });
    }
    Ok((vtxfmt, out, out_tris))
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
                    flags: 0,
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
                flags: 0,
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

    /// A flat floor of `n` × `n` quads, `size` units each, its texture repeating once per `per` units.
    fn floor(n: usize, size: f32, per: f32) -> (Vec<WorldVertex>, Vec<u16>) {
        let mut v = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let p = [x as f32 * size, y as f32 * size, 0.0];
                v.push(vtx(p, [p[0] / per, p[1] / per]));
            }
        }
        let mut t = Vec::new();
        let w = (n + 1) as u16;
        for y in 0..n as u16 {
            for x in 0..n as u16 {
                let (a, b, c, d) = (
                    y * w + x,
                    y * w + x + 1,
                    (y + 1) * w + x,
                    (y + 1) * w + x + 1,
                );
                t.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        (v, t)
    }

    #[test]
    fn tiled_quantize_fits_wide_floors_without_seams() {
        // 40 × 40 quads of 100 units, the texture repeating every 8 units: one connected piece spanning 500 repeats.
        let (v, t) = floor(40, 100.0, 8.0);
        let origin = snap_origin([2000.0, 2000.0, 0.0]);
        assert!(
            quantize(&v, &t, origin, false).is_none(),
            "the plain path must fail for this test"
        );
        let (fmt, q, tris) = quantize_tiled(&v, &t, origin).unwrap();
        assert_eq!(tris.len(), t.len());
        assert!(
            q.len() >= v.len() && q.len() < v.len() * 2,
            "{} vertices from {}",
            q.len(),
            v.len()
        );
        let rec = QuantRecord {
            vtxfmt: fmt,
            flags: 0,
            origin,
        };
        let step = 1.0 / f32::from(1u16 << uv_frac(fmt));
        for (old, new) in t.chunks_exact(3).zip(tris.chunks_exact(3)) {
            // Same corners, same positions; UVs shifted by one whole number per triangle (same texels).
            let mut shift: Option<[f32; 2]> = None;
            for (&a, &b) in old.iter().zip(new) {
                let (o, n) = (v[usize::from(a)], dequantize(&q[usize::from(b)], &rec));
                for k in 0..3 {
                    assert!((o.pos[k] - n.pos[k]).abs() <= 1.0 / 16.0);
                }
                let d = [o.uv[0] - n.uv[0], o.uv[1] - n.uv[1]];
                for dk in d {
                    assert!((dk - libm::roundf(dk)).abs() <= step, "uv off by {dk}");
                }
                let r = d.map(libm::roundf);
                assert_eq!(
                    *shift.get_or_insert(r),
                    r,
                    "a triangle's corners must share one shift"
                );
            }
        }
    }

    #[test]
    fn tiled_quantize_reports_why_it_cannot() {
        // A single triangle spanning 300 repeats.
        let v = vec![
            vtx([0.0; 3], [0.0, 0.0]),
            vtx([1.0, 0.0, 0.0], [300.0, 0.0]),
            vtx([0.0, 1.0, 0.0], [0.0, 1.0]),
        ];
        assert_eq!(
            quantize_tiled(&v, &[0, 1, 2], [0.0; 3]),
            Err(FloatReason::TriangleUv)
        );
        // A vertex 5000 units from the origin.
        let v = vec![
            vtx([5000.0, 0.0, 0.0], [0.0, 0.0]),
            vtx([0.0; 3], [0.0, 0.0]),
            vtx([1.0; 3], [0.0, 0.0]),
        ];
        assert_eq!(
            quantize_tiled(&v, &[0, 1, 2], [0.0; 3]),
            Err(FloatReason::Position)
        );
    }
}

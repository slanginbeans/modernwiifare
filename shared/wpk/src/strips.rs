//! Triangle strips for GX display lists.
//!
//! GX has no post-transform vertex cache: an indexed triangle list sends and transforms 3 vertices per triangle, a
//! strip about 1. [`stripify`] turns a triangle list into strips without changing a single triangle or its winding;
//! [`display_list`] writes them (strips of two or more triangles as `GX_TRIANGLESTRIP`, the rest as one
//! `GX_TRIANGLES`), in the world vertex layout ([`crate::records::WORLD_DL_ATTRS`] u16 indices per vertex).
//!
//! Strip winding: triangle k of a strip is (s[k], s[k+1], s[k+2]) for even k and (s[k+1], s[k], s[k+2]) for odd k,
//! the GX / D3D rule, so back-face culling sees the original winding.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::be::Push;
use crate::records::{WORLD_DL_ATTRS, WORLD_VTXFMT, gx_triangles};

/// `GX_TRIANGLESTRIP | fmt`.
pub const fn gx_triangle_strip(vtxfmt: u8) -> u8 {
    0x98 | (vtxfmt & 7)
}

/// Strips (vertex sequences of at least 4 vertices) and the triangles left over.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Strips {
    pub strips: Vec<Vec<u16>>,
    pub singles: Vec<[u16; 3]>,
}

impl Strips {
    /// Vertices the GPU will transform for these strips.
    pub fn vertex_count(&self) -> usize {
        self.strips.iter().map(Vec::len).sum::<usize>() + self.singles.len() * 3
    }
}

/// Greedy stripification. Degenerate triangles (repeated vertex) are kept as singles, unchanged.
pub fn stripify(tris: &[u16]) -> Strips {
    let n = tris.len() / 3;
    let tri = |t: usize| [tris[3 * t], tris[3 * t + 1], tris[3 * t + 2]];
    // Directed edge (a, b) -> triangles whose cyclic order contains a -> b.
    let mut edges: BTreeMap<(u16, u16), Vec<u32>> = BTreeMap::new();
    let mut degenerate = alloc::vec![false; n];
    for (t, deg) in degenerate.iter_mut().enumerate() {
        let [a, b, c] = tri(t);
        if a == b || b == c || a == c {
            *deg = true;
            continue;
        }
        for e in [(a, b), (b, c), (c, a)] {
            edges.entry(e).or_default().push(t as u32);
        }
    }
    let mut used = degenerate.clone();
    let third = |t: usize, a: u16, b: u16| -> u16 {
        let v = tri(t);
        v.into_iter().find(|&x| x != a && x != b).unwrap_or(v[0])
    };
    // Next unused triangle continuing the strip: for strip triangle index k with last two vertices (u, v), even k
    // needs directed edge u -> v, odd k needs v -> u.
    let next = |used: &[bool], u: u16, v: u16, k: usize| -> Option<(usize, u16)> {
        let e = if k.is_multiple_of(2) { (u, v) } else { (v, u) };
        edges
            .get(&e)?
            .iter()
            .map(|&t| t as usize)
            .find(|&t| !used[t])
            .map(|t| (t, third(t, u, v)))
    };
    // Walk a strip from triangle `t` started in rotation `r` without committing; returns its triangles.
    let walk = |used: &mut [bool], t: usize, r: usize| -> (Vec<u16>, Vec<usize>) {
        let v = tri(t);
        let mut s = alloc::vec![v[r], v[(r + 1) % 3], v[(r + 2) % 3]];
        let mut ts = alloc::vec![t];
        used[t] = true;
        while let Some((nt, w)) = next(used, s[s.len() - 2], s[s.len() - 1], s.len() - 2) {
            used[nt] = true;
            ts.push(nt);
            s.push(w);
        }
        for &x in &ts {
            used[x] = false;
        }
        (s, ts)
    };

    // Start strips at the triangles with the fewest neighbours (mesh borders and corners) first: strips then run
    // inward instead of stranding border triangles as singles.
    let neighbours = |t: usize| -> usize {
        let [a, b, c] = tri(t);
        [(b, a), (c, b), (a, c)]
            .iter()
            .map(|e| {
                edges
                    .get(e)
                    .map_or(0, |ts| ts.iter().filter(|&&o| o as usize != t).count())
            })
            .sum()
    };
    let mut starts: Vec<(usize, usize)> = (0..n).map(|t| (neighbours(t), t)).collect();
    starts.sort_unstable();

    let mut out = Strips::default();
    for t in starts.into_iter().map(|(_, t)| t) {
        if used[t] {
            if degenerate[t] {
                out.singles.push(tri(t));
                degenerate[t] = false;
            }
            continue;
        }
        // Try the three starting rotations, keep the longest.
        let best = (0..3)
            .map(|r| walk(&mut used, t, r))
            .max_by_key(|(_, ts)| ts.len())
            .unwrap_or_default();
        for &x in &best.1 {
            used[x] = true;
        }
        if best.1.len() >= 2 {
            out.strips.push(best.0);
        } else {
            out.singles.push(tri(t));
        }
    }
    out
}

/// The triangles a strip draws, each in its GX winding.
pub fn strip_triangles(s: &[u16]) -> impl Iterator<Item = [u16; 3]> + '_ {
    (0..s.len().saturating_sub(2)).map(move |k| {
        if k.is_multiple_of(2) {
            [s[k], s[k + 1], s[k + 2]]
        } else {
            [s[k + 1], s[k], s[k + 2]]
        }
    })
}

/// A world display list from a triangle list, as strips plus one triangle list. Padded with NOPs to 32 bytes.
pub fn display_list(tris: &[u16]) -> Vec<u8> {
    display_list_fmt(tris, WORLD_VTXFMT)
}

/// [`display_list`] in vertex format `vtxfmt`.
pub fn display_list_fmt(tris: &[u16], vtxfmt: u8) -> Vec<u8> {
    strips_display_list(&stripify(tris), vtxfmt)
}

/// How a display list indexes its vertices: `attrs` indexed attributes per vertex (each the same index; the first is
/// the position's), as u16 or, for batches of at most 256 vertices, u8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DlLayout {
    pub attrs: usize,
    pub idx8: bool,
}

impl DlLayout {
    /// The world layout of older packs: position, colour, UV and lightmap UV as u16 indices.
    pub const WORLD: DlLayout = DlLayout {
        attrs: WORLD_DL_ATTRS,
        idx8: false,
    };

    /// The world layout a batch's [`crate::quant::dl_flags`] describe.
    pub const fn from_flags(flags: u8) -> DlLayout {
        DlLayout {
            attrs: if flags & crate::quant::dl_flags::NO_COLOR != 0 {
                WORLD_DL_ATTRS - 1
            } else {
                WORLD_DL_ATTRS
            },
            idx8: flags & crate::quant::dl_flags::IDX8 != 0,
        }
    }

    /// Bytes per vertex in the display list.
    pub const fn vertex_bytes(self) -> usize {
        self.attrs * if self.idx8 { 1 } else { 2 }
    }
}

/// A display list of `st` (strips may hold repeated-vertex "swap" triangles: zero area, nothing drawn).
pub fn strips_display_list(st: &Strips, vtxfmt: u8) -> Vec<u8> {
    strips_display_list_layout(st, vtxfmt, DlLayout::WORLD)
}

/// [`strips_display_list`] for a vertex format with `attrs` indexed attributes (each the same u16 index).
pub fn strips_display_list_attrs(st: &Strips, vtxfmt: u8, attrs: usize) -> Vec<u8> {
    strips_display_list_layout(st, vtxfmt, DlLayout { attrs, idx8: false })
}

/// [`strips_display_list`] in `layout`. With u8 indices every index must be under 256.
pub fn strips_display_list_layout(st: &Strips, vtxfmt: u8, layout: DlLayout) -> Vec<u8> {
    let mut dl = Vec::new();
    let put = |dl: &mut Vec<u8>, op: u8, verts: &[u16]| {
        dl.u8(op);
        dl.u16(verts.len() as u16);
        for &i in verts {
            for _ in 0..layout.attrs {
                if layout.idx8 {
                    debug_assert!(i < 256, "u8 display-list index {i}");
                    dl.u8(i as u8);
                } else {
                    dl.u16(i);
                }
            }
        }
    };
    for s in &st.strips {
        // A strip longer than 65535 vertices restarts two vertices back, keeping its parity.
        let mut at = 0;
        while at + 2 < s.len() {
            let end = (at + 65534).min(s.len());
            put(&mut dl, gx_triangle_strip(vtxfmt), &s[at..end]);
            at = end - 2;
        }
    }
    let flat: Vec<u16> = st.singles.iter().flatten().copied().collect();
    // 65535 vertices is a whole number of triangles (21845).
    for chunk in flat.chunks(65535) {
        put(&mut dl, gx_triangles(vtxfmt), chunk);
    }
    while dl.len() % crate::ALIGN != 0 {
        dl.u8(0);
    }
    dl
}

/// Decode a world display list (lists and strips) to triangles in GX winding.
pub fn dl_triangles(dl: &[u8]) -> Vec<[u16; 3]> {
    dl_triangles_layout(dl, DlLayout::WORLD)
}

/// [`dl_triangles`] for a vertex format with `attrs` u16 indices per vertex (the first is the position's).
pub fn dl_triangles_attrs(dl: &[u8], attrs: usize) -> Vec<[u16; 3]> {
    dl_triangles_layout(dl, DlLayout { attrs, idx8: false })
}

/// [`dl_triangles`] in `layout`.
pub fn dl_triangles_layout(dl: &[u8], layout: DlLayout) -> Vec<[u16; 3]> {
    let vb = layout.vertex_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= dl.len() {
        let op = dl[i];
        if op == 0 {
            i += 1;
            continue;
        }
        let c = usize::from(u16::from_be_bytes([dl[i + 1], dl[i + 2]]));
        let verts: Vec<u16> = (0..c)
            .map(|v| {
                let at = i + 3 + v * vb;
                if layout.idx8 {
                    u16::from(dl[at])
                } else {
                    u16::from_be_bytes([dl[at], dl[at + 1]])
                }
            })
            .collect();
        if op & 0xF8 == 0x98 {
            out.extend(strip_triangles(&verts));
        } else {
            out.extend(verts.chunks_exact(3).map(|t| [t[0], t[1], t[2]]));
        }
        i += 3 + c * vb;
    }
    out
}

/// Same triangles with the same winding (any order, any rotation)? Triangles with a repeated vertex draw nothing
/// and are ignored on both sides (strips use them to turn).
pub fn same_triangles(
    a: impl Iterator<Item = [u16; 3]>,
    b: impl Iterator<Item = [u16; 3]>,
) -> bool {
    let canon = |it: &mut dyn Iterator<Item = [u16; 3]>| {
        let mut v: Vec<[u16; 3]> = it
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .map(|t| {
                let r = if t[0] <= t[1] && t[0] <= t[2] {
                    0
                } else if t[1] <= t[2] {
                    1
                } else {
                    2
                };
                [t[r], t[(r + 1) % 3], t[(r + 2) % 3]]
            })
            .collect();
        v.sort_unstable();
        v
    };
    let (mut a, mut b) = (a, b);
    canon(&mut a) == canon(&mut b)
}

/// Vertices a world display list sends to the GPU (what the vertex pipeline transforms).
pub fn dl_vertex_count(dl: &[u8]) -> usize {
    dl_vertex_count_layout(dl, DlLayout::WORLD)
}

/// [`dl_vertex_count`] in `layout`.
pub fn dl_vertex_count_layout(dl: &[u8], layout: DlLayout) -> usize {
    let (mut i, mut n) = (0, 0);
    while i + 3 <= dl.len() {
        if dl[i] == 0 {
            i += 1;
            continue;
        }
        let c = usize::from(u16::from_be_bytes([dl[i + 1], dl[i + 2]]));
        n += c;
        i += 3 + c * layout.vertex_bytes();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Every triangle with its winding, as a sorted list of canonical rotations.
    fn canonical(tris: impl Iterator<Item = [u16; 3]>) -> Vec<[u16; 3]> {
        let mut v: Vec<[u16; 3]> = tris
            .map(|t| {
                let r = (0..3).min_by_key(|&i| t[i]).unwrap();
                [t[r], t[(r + 1) % 3], t[(r + 2) % 3]]
            })
            .collect();
        v.sort();
        v
    }

    fn grid(w: u16, h: u16) -> Vec<u16> {
        let mut t = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = y * (w + 1) + x;
                let (a, b, c, d) = (i, i + 1, i + w + 1, i + w + 2);
                t.extend([a, c, b, b, c, d]);
            }
        }
        t
    }

    #[test]
    fn strips_keep_every_triangle_and_winding() {
        let tris = grid(16, 16);
        let st = stripify(&tris);
        let back = st
            .strips
            .iter()
            .flat_map(|s| strip_triangles(s).collect::<Vec<_>>())
            .chain(st.singles.iter().copied());
        assert_eq!(
            canonical(back),
            canonical(tris.chunks(3).map(|c| [c[0], c[1], c[2]]))
        );
        // A regular grid strips well: far fewer vertices than 3 per triangle.
        let n = tris.len() / 3;
        assert!(
            st.vertex_count() * 10 < n * 3 * 6,
            "{} vertices for {n} triangles",
            st.vertex_count()
        );
    }

    #[test]
    fn degenerate_and_isolated_triangles_survive() {
        let tris = vec![0, 1, 2, 5, 5, 6, 7, 8, 9];
        let st = stripify(&tris);
        assert!(st.strips.is_empty());
        assert_eq!(canonical(st.singles.iter().copied()).len(), 3);
    }

    #[test]
    fn display_list_has_strip_commands_and_alignment() {
        let dl = display_list(&grid(4, 1));
        assert_eq!(dl.len() % 32, 0);
        assert_eq!(dl[0], 0x98);
        assert_eq!(dl_vertex_count(&dl), stripify(&grid(4, 1)).vertex_count());
    }

    #[test]
    fn layouts_round_trip_and_shrink() {
        use crate::quant::dl_flags::{IDX8, NO_COLOR};
        // A 9 × 9 grid of quads (100 vertices): fits u8 indices.
        let mut tris = Vec::new();
        for y in 0..9u16 {
            for x in 0..9u16 {
                let (a, b, c, d) = (
                    y * 10 + x,
                    y * 10 + x + 1,
                    (y + 1) * 10 + x,
                    (y + 1) * 10 + x + 1,
                );
                tris.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        let st = stripify(&tris);
        let want = || tris.chunks_exact(3).map(|t| [t[0], t[1], t[2]]);
        let old = strips_display_list(&st, 3);
        assert_eq!(
            old,
            strips_display_list_layout(&st, 3, DlLayout::from_flags(0)),
            "flags 0 is the old layout"
        );
        for flags in [IDX8, NO_COLOR, IDX8 | NO_COLOR] {
            let layout = DlLayout::from_flags(flags);
            let dl = strips_display_list_layout(&st, 3, layout);
            assert!(
                same_triangles(dl_triangles_layout(&dl, layout).into_iter(), want()),
                "flags {flags}"
            );
            assert_eq!(dl_vertex_count_layout(&dl, layout), dl_vertex_count(&old));
            assert_eq!(dl.len() % crate::ALIGN, 0);
            assert!(
                dl.len() < old.len(),
                "flags {flags}: {} vs {}",
                dl.len(),
                old.len()
            );
        }
        assert_eq!(DlLayout::from_flags(IDX8).vertex_bytes(), 4);
        assert_eq!(DlLayout::from_flags(NO_COLOR).vertex_bytes(), 6);
        assert_eq!(DlLayout::from_flags(IDX8 | NO_COLOR).vertex_bytes(), 3);
        // Triangle lists (blended batches) too.
        let layout = DlLayout::from_flags(IDX8 | NO_COLOR);
        let list = crate::records::world_display_list_layout(&tris, 3, layout);
        assert!(same_triangles(
            dl_triangles_layout(&list, layout).into_iter(),
            want()
        ));
        assert_eq!(
            crate::records::world_display_list_fmt(&tris, 3),
            crate::records::world_display_list_layout(&tris, 3, DlLayout::WORLD)
        );
    }
}

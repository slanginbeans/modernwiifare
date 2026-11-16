//! World mesh simplification for the Wii's triangle budget (meshoptimizer, MIT, via the `meshopt` crate).
//!
//! Each opaque world batch is simplified on its own: vertices whose position other batches also use are locked, so
//! batches still meet without cracks (edges the batch alone owns may move); texture UVs, lightmap UVs and vertex colours are weighed
//! alongside positions, so textures and lighting don't swim; and no vertex moves further than `max_error` world
//! units from the original surface. Collision is never touched (it comes from the clip map, not these meshes).

use meshopt::{SimplifyOptions, SimplifyVertex, VertexDataAdapter};
use wpk::records::WorldVertex;

/// How far the converter simplifies the world's draw meshes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldQuality {
    /// Target triangles as a share of the original (1 = off).
    pub ratio: f32,
    /// The most a simplified surface may stray from the original, in world units (inches).
    pub max_error: f32,
}

impl Default for WorldQuality {
    /// Off until hardware numbers say how far to go.
    fn default() -> Self {
        WorldQuality {
            ratio: 1.0,
            max_error: 2.0,
        }
    }
}

/// Lightmap UV fixed point back to its [0, 2) value.
fn lm(v: u16) -> f32 {
    f32::from(v) / (1u32 << wpk::records::LM_FRAC) as f32
}

/// A position key for finding where batches meet (exact float bits: IW4 shares edge positions exactly).
pub fn position_key(p: [f32; 3]) -> [u32; 3] {
    p.map(f32::to_bits)
}

/// A small far-away part (a held weapon's attachment): `tris` (indices into `positions`) cut to about `target`
/// triangles by meshoptimizer's sloppy simplifier (topology not kept: it only has to read at a distance), then the
/// vertices still used, renumbered. Unchanged when `target` isn't smaller or the result would be empty.
pub fn far_part(positions: &[[f32; 3]], tris: &[u16], target: usize) -> (Vec<usize>, Vec<u16>) {
    let all = || ((0..positions.len()).collect(), tris.to_vec());
    if target * 3 >= tris.len() {
        return all();
    }
    let bytes: Vec<u8> = positions
        .iter()
        .flatten()
        .flat_map(|f| f.to_ne_bytes())
        .collect();
    let Ok(adapter) = VertexDataAdapter::new(&bytes, 12, 0) else {
        return all();
    };
    let indices: Vec<u32> = tris.iter().map(|&i| u32::from(i)).collect();
    let out = meshopt::simplify_sloppy(&indices, &adapter, target.max(1) * 3, 1.0, None);
    if out.is_empty() {
        return all();
    }
    let mut keep = vec![u16::MAX; positions.len()];
    let mut used = Vec::new();
    let tris = out
        .iter()
        .map(|&i| {
            let k = &mut keep[i as usize];
            if *k == u16::MAX {
                *k = used.len() as u16;
                used.push(i as usize);
            }
            *k
        })
        .collect();
    (used, tris)
}

/// `tris` (indices into `verts`) simplified by `q`; `shared(v)` says whether vertex `v`'s position is also used
/// by another batch (it stays). The same triangles when nothing can go.
pub fn batch(
    verts: &[WorldVertex],
    tris: &[u16],
    q: &WorldQuality,
    shared: impl Fn(&WorldVertex) -> bool,
) -> Vec<u16> {
    if q.ratio >= 1.0 || tris.len() < 3 * 16 {
        return tris.to_vec();
    }
    let positions: Vec<f32> = verts.iter().flat_map(|v| v.pos).collect();
    let bytes: Vec<u8> = positions.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let Ok(adapter) = VertexDataAdapter::new(&bytes, 12, 0) else {
        return tris.to_vec();
    };
    // Attributes: texture UV, lightmap UV, colour (0..1), weighted so a texel or lightmap texel of drift costs about
    // what an inch of position error does.
    const STRIDE: usize = 8;
    let attrs: Vec<f32> = verts
        .iter()
        .flat_map(|v| {
            let c = v.rgba.to_be_bytes();
            [
                v.uv[0],
                v.uv[1],
                lm(v.lm[0]),
                lm(v.lm[1]),
                f32::from(c[0]) / 255.0,
                f32::from(c[1]) / 255.0,
                f32::from(c[2]) / 255.0,
                f32::from(c[3]) / 255.0,
            ]
        })
        .collect();
    // Vertex colour also carries IW4's terrain blend weights: changing it shows as dark or light patches.
    let weights = [0.5, 0.5, 8.0, 8.0, 4.0, 4.0, 4.0, 4.0];
    let indices: Vec<u32> = tris.iter().map(|&i| u32::from(i)).collect();
    // Open edges of the batch (by position, so UV seams inside it don't count): an edge used by one triangle.
    let mut edges: std::collections::HashMap<([u32; 3], [u32; 3]), u32> =
        std::collections::HashMap::new();
    for t in tris.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (pa, pb) = (
                position_key(verts[usize::from(a)].pos),
                position_key(verts[usize::from(b)].pos),
            );
            *edges
                .entry(if pa < pb { (pa, pb) } else { (pb, pa) })
                .or_default() += 1;
        }
    }
    let mut on_edge: std::collections::HashSet<[u32; 3]> = std::collections::HashSet::new();
    for ((a, b), n) in &edges {
        if *n == 1 {
            on_edge.insert(*a);
            on_edge.insert(*b);
        }
    }
    // Locked: on an open edge where another batch meets this one.
    let locks: Vec<SimplifyVertex> = verts
        .iter()
        .map(|v| {
            if on_edge.contains(&position_key(v.pos)) && shared(v) {
                SimplifyVertex::Lock
            } else {
                SimplifyVertex::None
            }
        })
        .collect();
    let target = ((indices.len() as f32 * q.ratio) as usize / 3 * 3).max(3);
    let out = meshopt::simplify_with_attributes_and_locks(
        &indices,
        &adapter,
        &attrs,
        &weights,
        STRIDE * 4,
        &locks,
        target,
        q.max_error,
        SimplifyOptions::ErrorAbsolute,
        None,
    );
    if out.is_empty() || out.len() >= indices.len() {
        return tris.to_vec();
    }
    out.iter().map(|&i| i as u16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20 × 20 grid cut to ~40 triangles: fewer vertices, every index in range.
    #[test]
    fn far_parts_shrink_to_their_target() {
        let n = 21u16;
        let positions: Vec<[f32; 3]> = (0..n * n)
            .map(|i| [f32::from(i % n), f32::from(i / n), 0.0])
            .collect();
        let mut tris = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let a = y * n + x;
                tris.extend([a, a + 1, a + n, a + 1, a + n + 1, a + n]);
            }
        }
        let (used, out) = far_part(&positions, &tris, 40);
        assert!(!out.is_empty() && out.len() <= 3 * 60, "{}", out.len());
        assert!(used.len() < positions.len());
        assert!(out.iter().all(|&i| usize::from(i) < used.len()));
        // A target at or over what it has: unchanged.
        let (used, same) = far_part(&positions, &tris, 800);
        assert_eq!((used.len(), same), (positions.len(), tris));
    }

    /// A flat 10 × 10 quad grid (200 triangles) with planar UVs: simplified, it keeps its outline.
    #[test]
    fn a_flat_grid_simplifies_and_keeps_its_border() {
        let n = 11;
        let mut verts = Vec::new();
        for y in 0..n {
            for x in 0..n {
                verts.push(WorldVertex {
                    pos: [x as f32 * 16.0, y as f32 * 16.0, 0.0],
                    rgba: 0xffff_ffff,
                    uv: [x as f32 / 10.0, y as f32 / 10.0],
                    lm: [0, 0],
                });
            }
        }
        let mut tris = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let i = (y * n + x) as u16;
                let n = n as u16;
                tris.extend_from_slice(&[i, i + 1, i + n, i + 1, i + n + 1, i + n]);
            }
        }
        let q = WorldQuality {
            ratio: 0.25,
            max_error: 1.0,
        };
        // The outline is shared with neighbours (as a batch's edges are): it must stay.
        let edge = |v: &WorldVertex| {
            v.pos[0] == 0.0 || v.pos[1] == 0.0 || v.pos[0] == 160.0 || v.pos[1] == 160.0
        };
        let out = batch(&verts, &tris, &q, edge);
        assert!(
            out.len() < tris.len() / 2,
            "{} of {}",
            out.len(),
            tris.len()
        );
        // Every corner of the grid is still used (the border is locked).
        for corner in [0u16, 10, 110, 120] {
            assert!(out.contains(&corner), "corner {corner}");
        }
        // Off: unchanged.
        assert_eq!(batch(&verts, &tris, &WorldQuality::default(), edge), tris);
    }
}

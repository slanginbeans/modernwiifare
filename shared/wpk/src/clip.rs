//! The `CLIP` section: a map's collision in the shape IW4L's `clipmap_iw4` traces against.
//!
//! Layout: a table of [`ARRAYS`] entries (count u32, offset u32; offset from the section start), then each array,
//! 32-byte aligned, big-endian. Brush planes are pooled: a brush is a run of `planes` / `plane_flags`.
//! Nothing is simplified: playable collision must match the PC game exactly.

use alloc::vec::Vec;

use clipmap_iw4::{
    BrushView, ClipAabbNode, ClipBorder, ClipCmodel, ClipLeaf, ClipMeshRef, ClipMeshTables,
    ClipNode, ClipPartition,
};

use crate::be::{self, Push};

/// Number of arrays in the section table.
pub const ARRAYS: usize = 16;

/// One brush: a run of pooled planes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrushRec {
    pub first_plane: u32,
    pub plane_count: u16,
    pub glass_encoded: u16,
    pub contents: u32,
}

/// A whole map's collision.
#[derive(Clone, Debug, Default)]
pub struct Clip {
    pub planes: Vec<[f32; 4]>,
    pub plane_flags: Vec<u32>,
    pub brushes: Vec<BrushRec>,
    pub nodes: Vec<ClipNode>,
    pub leaves: Vec<ClipLeaf>,
    pub leafbrushes: Vec<u16>,
    pub mesh: ClipMeshTables,
    pub cmodels: Vec<ClipCmodel>,
}

/// A brush as `clipmap_iw4` sees it, borrowing the pooled planes.
#[derive(Clone, Copy, Debug)]
pub struct BrushSlice<'a> {
    pub planes: &'a [[f32; 4]],
    pub flags: &'a [u32],
    pub contents: u32,
    pub glass_encoded: u16,
}

impl BrushView for BrushSlice<'_> {
    fn planes(&self) -> &[[f32; 4]] {
        self.planes
    }
    fn contents(&self) -> u32 {
        self.contents
    }
    fn plane_surface_flags(&self) -> &[u32] {
        self.flags
    }
    fn glass_encoded(&self) -> u16 {
        self.glass_encoded
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipError {
    Truncated,
    /// A brush, leaf, node or partition points outside its table.
    BadIndex(&'static str),
}

impl Clip {
    /// Push a brush from its planes and per-plane surface flags.
    pub fn push_brush(
        &mut self,
        planes: &[[f32; 4]],
        flags: &[u32],
        contents: u32,
        glass_encoded: u16,
    ) {
        debug_assert_eq!(planes.len(), flags.len());
        self.brushes.push(BrushRec {
            first_plane: self.planes.len() as u32,
            plane_count: planes.len() as u16,
            glass_encoded,
            contents,
        });
        self.planes.extend_from_slice(planes);
        self.plane_flags.extend_from_slice(flags);
    }

    /// Brushes as `BrushView`s, in brush order (what `ClipMapRef::brushes` wants).
    pub fn brush_slices(&self) -> Vec<BrushSlice<'_>> {
        self.brushes
            .iter()
            .map(|b| {
                let r = b.first_plane as usize..b.first_plane as usize + b.plane_count as usize;
                BrushSlice {
                    planes: &self.planes[r.clone()],
                    flags: &self.plane_flags[r],
                    contents: b.contents,
                    glass_encoded: b.glass_encoded,
                }
            })
            .collect()
    }

    pub fn mesh_ref(&self) -> ClipMeshRef<'_> {
        self.mesh.as_ref()
    }

    /// Check every cross-reference, so traces never index out of range.
    pub fn validate(&self) -> Result<(), ClipError> {
        let planes = self.planes.len();
        if self.plane_flags.len() != planes {
            return Err(ClipError::BadIndex("plane flags"));
        }
        for b in &self.brushes {
            if b.first_plane as usize + b.plane_count as usize > planes {
                return Err(ClipError::BadIndex("brush planes"));
            }
        }
        let (nn, nl) = (self.nodes.len() as i64, self.leaves.len() as i64);
        for n in &self.nodes {
            for &c in &n.children {
                // >= 0: node index; < 0: leaf -1 - c.
                let ok = if c >= 0 {
                    (c as i64) < nn
                } else {
                    (-1 - c as i64) < nl
                };
                if !ok {
                    return Err(ClipError::BadIndex("node child"));
                }
            }
        }
        for l in &self.leaves {
            if l.first_brush as usize + l.num_brushes as usize > self.leafbrushes.len() {
                return Err(ClipError::BadIndex("leaf brushes"));
            }
        }
        if self
            .leafbrushes
            .iter()
            .any(|&b| b as usize >= self.brushes.len())
        {
            return Err(ClipError::BadIndex("leafbrush"));
        }
        let m = &self.mesh;
        let tris = m.tri_indices.len() / 3;
        if m.tri_indices.iter().any(|&i| i as usize >= m.verts.len()) {
            return Err(ClipError::BadIndex("mesh vertex"));
        }
        if m.tri_surface_flags.len() < tris || m.tri_content_flags.len() < tris {
            return Err(ClipError::BadIndex("mesh tri flags"));
        }
        if m.tri_edge_is_walkable.len() * 8 < tris * 3 {
            return Err(ClipError::BadIndex("mesh walkable bits"));
        }
        for p in &m.partitions {
            if p.first_tri < 0 || p.first_tri as usize + p.tri_count as usize > tris {
                return Err(ClipError::BadIndex("partition tris"));
            }
            if p.first_border as usize + p.border_count as usize > m.borders.len() {
                return Err(ClipError::BadIndex("partition borders"));
            }
        }
        if m.aabb_roots
            .iter()
            .any(|&r| r as usize >= m.aabb_trees.len())
        {
            return Err(ClipError::BadIndex("aabb root"));
        }
        for c in &self.cmodels {
            if c.num_brushes > 0
                && c.first_brush as usize + c.num_brushes as usize > self.leafbrushes.len()
            {
                return Err(ClipError::BadIndex("cmodel brushes"));
            }
        }
        Ok(())
    }

    /// Encode as one `CLIP` section.
    pub fn encode(&self) -> Vec<u8> {
        let m = &self.mesh;
        let mut arrays: Vec<(u32, Vec<u8>)> = Vec::with_capacity(ARRAYS);
        let mut put = |count: usize, f: &mut dyn FnMut(&mut Vec<u8>)| {
            let mut v = Vec::new();
            f(&mut v);
            arrays.push((count as u32, v));
        };
        put(self.planes.len(), &mut |o| {
            self.planes.iter().flatten().for_each(|&x| o.f32(x))
        });
        put(self.plane_flags.len(), &mut |o| {
            self.plane_flags.iter().for_each(|&x| o.u32(x))
        });
        put(self.brushes.len(), &mut |o| {
            for b in &self.brushes {
                o.u32(b.first_plane);
                o.u16(b.plane_count);
                o.u16(b.glass_encoded);
                o.u32(b.contents);
            }
        });
        put(self.nodes.len(), &mut |o| {
            for n in &self.nodes {
                n.plane.iter().for_each(|&x| o.f32(x));
                n.children.iter().for_each(|&c| o.u32(c as u32));
            }
        });
        put(self.leaves.len(), &mut |o| {
            for l in &self.leaves {
                o.u32(l.first_brush);
                o.u16(l.num_brushes);
                o.u16(l.first_coll_aabb_index);
                o.u16(l.coll_aabb_count);
                o.u16(0);
            }
        });
        put(self.leafbrushes.len(), &mut |o| {
            self.leafbrushes.iter().for_each(|&x| o.u16(x))
        });
        put(m.verts.len(), &mut |o| {
            m.verts.iter().flatten().for_each(|&x| o.f32(x))
        });
        put(m.tri_indices.len(), &mut |o| {
            m.tri_indices.iter().for_each(|&x| o.u16(x))
        });
        put(m.tri_edge_is_walkable.len(), &mut |o| {
            o.extend_from_slice(&m.tri_edge_is_walkable)
        });
        put(m.tri_surface_flags.len(), &mut |o| {
            m.tri_surface_flags.iter().for_each(|&x| o.u32(x))
        });
        put(m.tri_content_flags.len(), &mut |o| {
            m.tri_content_flags.iter().for_each(|&x| o.u32(x))
        });
        put(m.aabb_trees.len(), &mut |o| {
            for a in &m.aabb_trees {
                a.origin.iter().chain(&a.half_size).for_each(|&x| o.f32(x));
                o.u16(a.material_index);
                o.u16(a.child_count);
                o.u32(a.u as u32);
            }
        });
        put(m.partitions.len(), &mut |o| {
            for p in &m.partitions {
                o.u8(p.tri_count);
                o.u8(p.first_vert_segment);
                o.u8(p.border_count);
                o.u8(0);
                o.u32(p.first_tri as u32);
                o.u32(p.first_border);
            }
        });
        put(m.borders.len(), &mut |o| {
            for b in &m.borders {
                b.dist_eq.iter().for_each(|&x| o.f32(x));
                [b.z_base, b.z_slope, b.start, b.length]
                    .iter()
                    .for_each(|&x| o.f32(x));
            }
        });
        put(m.aabb_roots.len(), &mut |o| {
            m.aabb_roots.iter().for_each(|&x| o.u16(x))
        });
        put(self.cmodels.len(), &mut |o| {
            for c in &self.cmodels {
                c.mins.iter().chain(&c.maxs).for_each(|&x| o.f32(x));
                o.f32(c.radius);
                o.u32(c.first_brush);
                o.u16(c.num_brushes);
                o.u16(0);
            }
        });
        debug_assert_eq!(arrays.len(), ARRAYS);

        let mut out = Vec::new();
        out.zeros(align(ARRAYS * 8));
        for (i, (count, data)) in arrays.iter().enumerate() {
            let off = out.len();
            be::put_u32(&mut out, i * 8, *count);
            be::put_u32(&mut out, i * 8 + 4, off as u32);
            out.extend_from_slice(data);
            out.zeros(align(out.len()) - out.len());
        }
        out
    }

    /// Decode a `CLIP` section (copies into owned tables) and validate it.
    pub fn decode(b: &[u8]) -> Result<Clip, ClipError> {
        if b.len() < ARRAYS * 8 {
            return Err(ClipError::Truncated);
        }
        let array = |i: usize, stride: usize| -> Result<(usize, &[u8]), ClipError> {
            let n = be::u32(b, i * 8) as usize;
            let off = be::u32(b, i * 8 + 4) as usize;
            let end = n
                .checked_mul(stride)
                .and_then(|l| l.checked_add(off))
                .ok_or(ClipError::Truncated)?;
            b.get(off..end).map(|d| (n, d)).ok_or(ClipError::Truncated)
        };
        fn rows<T>(d: (usize, &[u8]), stride: usize, f: impl Fn(&[u8]) -> T) -> Vec<T> {
            d.1.chunks_exact(stride).take(d.0).map(f).collect()
        }
        let f3 = |r: &[u8], at: usize| [be::f32(r, at), be::f32(r, at + 4), be::f32(r, at + 8)];

        let planes = rows(array(0, 16)?, 16, |r| {
            [be::f32(r, 0), be::f32(r, 4), be::f32(r, 8), be::f32(r, 12)]
        });
        let plane_flags = rows(array(1, 4)?, 4, |r| be::u32(r, 0));
        let brushes = rows(array(2, 12)?, 12, |r| BrushRec {
            first_plane: be::u32(r, 0),
            plane_count: be::u16(r, 4),
            glass_encoded: be::u16(r, 6),
            contents: be::u32(r, 8),
        });
        let nodes = rows(array(3, 24)?, 24, |r| ClipNode {
            plane: [be::f32(r, 0), be::f32(r, 4), be::f32(r, 8), be::f32(r, 12)],
            children: [be::u32(r, 16) as i32, be::u32(r, 20) as i32],
        });
        let leaves = rows(array(4, 12)?, 12, |r| ClipLeaf {
            first_brush: be::u32(r, 0),
            num_brushes: be::u16(r, 4),
            first_coll_aabb_index: be::u16(r, 6),
            coll_aabb_count: be::u16(r, 8),
        });
        let leafbrushes = rows(array(5, 2)?, 2, |r| be::u16(r, 0));
        let verts = rows(array(6, 12)?, 12, |r| f3(r, 0));
        let tri_indices = rows(array(7, 2)?, 2, |r| be::u16(r, 0));
        let tri_edge_is_walkable = rows(array(8, 1)?, 1, |r| r[0]);
        let tri_surface_flags = rows(array(9, 4)?, 4, |r| be::u32(r, 0));
        let tri_content_flags = rows(array(10, 4)?, 4, |r| be::u32(r, 0));
        let aabb_trees = rows(array(11, 32)?, 32, |r| ClipAabbNode {
            origin: f3(r, 0),
            half_size: f3(r, 12),
            material_index: be::u16(r, 24),
            child_count: be::u16(r, 26),
            u: be::u32(r, 28) as i32,
        });
        let partitions = rows(array(12, 12)?, 12, |r| ClipPartition {
            tri_count: r[0],
            first_vert_segment: r[1],
            border_count: r[2],
            first_tri: be::u32(r, 4) as i32,
            first_border: be::u32(r, 8),
        });
        let borders = rows(array(13, 28)?, 28, |r| ClipBorder {
            dist_eq: f3(r, 0),
            z_base: be::f32(r, 12),
            z_slope: be::f32(r, 16),
            start: be::f32(r, 20),
            length: be::f32(r, 24),
        });
        let aabb_roots = rows(array(14, 2)?, 2, |r| be::u16(r, 0));
        let cmodels = rows(array(15, 36)?, 36, |r| ClipCmodel {
            mins: f3(r, 0),
            maxs: f3(r, 12),
            radius: be::f32(r, 24),
            first_brush: be::u32(r, 28),
            num_brushes: be::u16(r, 32),
        });
        let clip = Clip {
            planes,
            plane_flags,
            brushes,
            nodes,
            leaves,
            leafbrushes,
            mesh: ClipMeshTables {
                verts,
                tri_indices,
                tri_edge_is_walkable,
                tri_surface_flags,
                tri_content_flags,
                aabb_trees,
                partitions,
                borders,
                aabb_roots,
            },
            cmodels,
        };
        clip.validate()?;
        Ok(clip)
    }
}

fn align(n: usize) -> usize {
    n.div_ceil(crate::ALIGN) * crate::ALIGN
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb_planes(min: [f32; 3], max: [f32; 3]) -> [[f32; 4]; 6] {
        [
            [1.0, 0.0, 0.0, max[0]],
            [-1.0, 0.0, 0.0, -min[0]],
            [0.0, 1.0, 0.0, max[1]],
            [0.0, -1.0, 0.0, -min[1]],
            [0.0, 0.0, 1.0, max[2]],
            [0.0, 0.0, -1.0, -min[2]],
        ]
    }

    /// One floor brush in a single leaf, plus one mesh triangle.
    pub(crate) fn floor() -> Clip {
        let mut c = Clip::default();
        c.push_brush(
            &aabb_planes([-512.0, -512.0, -16.0], [512.0, 512.0, 0.0]),
            &[0; 6],
            1,
            0,
        );
        c.leaves.push(ClipLeaf {
            first_brush: 0,
            num_brushes: 1,
            ..ClipLeaf::default()
        });
        c.leafbrushes.push(0);
        c.mesh.verts = alloc::vec![[0.0, 0.0, 100.0], [10.0, 0.0, 100.0], [0.0, 10.0, 100.0]];
        c.mesh.tri_indices = alloc::vec![0, 1, 2];
        c.mesh.tri_edge_is_walkable = alloc::vec![7];
        c.mesh.tri_surface_flags = alloc::vec![0];
        c.mesh.tri_content_flags = alloc::vec![1];
        c.mesh.partitions.push(ClipPartition {
            tri_count: 1,
            ..ClipPartition::default()
        });
        c.mesh.aabb_trees.push(ClipAabbNode {
            half_size: [5.0, 5.0, 1.0],
            ..ClipAabbNode::default()
        });
        c.mesh.aabb_roots.push(0);
        c.cmodels.push(ClipCmodel {
            mins: [-1.0; 3],
            maxs: [1.0; 3],
            radius: 2.0,
            first_brush: 0,
            num_brushes: 1,
        });
        c
    }

    #[test]
    fn round_trip() {
        let c = floor();
        c.validate().unwrap();
        let bytes = c.encode();
        assert_eq!(bytes.len() % crate::ALIGN, 0);
        let d = Clip::decode(&bytes).unwrap();
        assert_eq!(d.planes, c.planes);
        assert_eq!(d.brushes, c.brushes);
        assert_eq!(d.leaves, c.leaves);
        assert_eq!(d.mesh, c.mesh);
        assert_eq!(d.cmodels.len(), 1);
        assert_eq!(d.cmodels[0].first_brush, 0);
        assert_eq!(d.brush_slices()[0].planes.len(), 6);
    }

    #[test]
    fn bad_index_is_rejected() {
        let mut c = floor();
        c.leafbrushes[0] = 9;
        assert_eq!(
            Clip::decode(&c.encode()).unwrap_err(),
            ClipError::BadIndex("leafbrush")
        );
        assert_eq!(Clip::decode(&[0; 8]).unwrap_err(), ClipError::Truncated);
    }
}

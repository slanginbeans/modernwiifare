//! Collision from a map zone's clipMap: brushes (6 axial planes from their bounds + side planes, with surface flags),
//! the BSP (nodes, leaves, leaf brushes), triangle meshes (partitions, AABB trees, borders) and brush models.
//!
//! Adapted from IW4L `crates/asset_world/src/clip_collision.rs` (Apache-2.0, rev `acc57a5`): the IW4 extraction is
//! carried here because `asset_world` depends on Bevy. Static-model collision and IW5/T5 support are left out.
//! The result feeds `clipmap_iw4` (traces) in the pack and on the Wii.

// Kept close to the upstream text so IW4L fixes can be carried over line by line.
#![allow(
    clippy::needless_range_loop,
    clippy::type_complexity,
    clippy::collapsible_if
)]

use asset_iw4::size as sz;
use fastfile_iw4::{ClipMapGeometry, Ptr, ZonePtr, ZoneStream};

#[derive(Clone, Debug)]
pub struct ClipBrush {
    pub planes: Vec<[f32; 4]>,
    pub contents: u32,

    pub plane_surface_flags: Vec<u32>,

    pub glass_encoded: u16,
}

pub use clipmap_iw4::{ClipLeaf as ClipBspLeaf, ClipNode as ClipBspNode};

#[derive(Clone, Copy, Debug)]
pub struct ClipCmodel {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub radius: f32,
    pub first_brush: u32,
    pub num_brushes: u16,
}

/// Kept as IW4L reads it; the pack stores only the resolved flags.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct ClipMapMaterial {
    pub name: String,
    pub source_surface_flags: u32,
    pub surface_flags: u32,
    pub content_flags: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ClipCollision {
    pub brushes: Vec<ClipBrush>,
    pub nodes: Vec<ClipBspNode>,
    pub leaves: Vec<ClipBspLeaf>,
    pub leafbrushes: Vec<u16>,
    pub mesh: std::sync::Arc<clipmap_iw4::ClipMeshTables>,
    pub tri_material_index: Vec<u16>,
    pub materials: Vec<ClipMapMaterial>,
    pub cmodels: Vec<ClipCmodel>,
}

impl clipmap_iw4::BrushView for ClipBrush {
    fn planes(&self) -> &[[f32; 4]] {
        &self.planes
    }
    fn contents(&self) -> u32 {
        self.contents
    }
    fn plane_surface_flags(&self) -> &[u32] {
        &self.plane_surface_flags
    }
    fn glass_encoded(&self) -> u16 {
        self.glass_encoded
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipCollisionError {
    MissingTables,
    Truncated,

    LeafbrushIndexOverflow,
}

impl core::fmt::Display for ClipCollisionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingTables => write!(f, "clipmap brush tables were not retained"),
            Self::Truncated => write!(f, "clipmap brush bytes truncated in arena"),
            Self::LeafbrushIndexOverflow => {
                write!(
                    f,
                    "leafbrush table length does not fit first_brush/num_brushes index types"
                )
            }
        }
    }
}

fn leafbrush_range(first_len: usize, after_len: usize) -> Result<(u32, u16), ClipCollisionError> {
    let first = u32::try_from(first_len).map_err(|_| ClipCollisionError::LeafbrushIndexOverflow)?;
    let count = after_len
        .checked_sub(first_len)
        .ok_or(ClipCollisionError::LeafbrushIndexOverflow)?;
    let num = u16::try_from(count).map_err(|_| ClipCollisionError::LeafbrushIndexOverflow)?;
    Ok((first, num))
}

pub fn gate_leafbrush_index_fits(leafbrush_count: usize) -> Result<(), ClipCollisionError> {
    if leafbrush_count > u32::MAX as usize {
        return Err(ClipCollisionError::LeafbrushIndexOverflow);
    }
    Ok(())
}

pub fn build_clip_collision(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
) -> Result<ClipCollision, ClipCollisionError> {
    let brushes_ptr = g.brushes.ok_or(ClipCollisionError::MissingTables)?;
    let bounds_ptr = g.brush_bounds.ok_or(ClipCollisionError::MissingTables)?;
    let contents_ptr = g.brush_contents.ok_or(ClipCollisionError::MissingTables)?;
    let sides_base = g.brush_sides;
    let materials = g.materials;

    let mut out = ClipCollision {
        brushes: Vec::with_capacity(g.brush_count),
        ..ClipCollision::default()
    };

    for i in 0..g.brush_count {
        let brush = brushes_ptr.at(i * s.layout(sz::CBRUSH, 48));
        let bound = bounds_ptr.at(i * 24);
        let contents = s
            .u32_at(contents_ptr, i * 4)
            .map_err(|_| ClipCollisionError::Truncated)?;

        let mid = [
            s.f32_at(bound, 0)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(bound, 4)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(bound, 8)
                .map_err(|_| ClipCollisionError::Truncated)?,
        ];
        let half = [
            s.f32_at(bound, 12)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(bound, 16)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(bound, 20)
                .map_err(|_| ClipCollisionError::Truncated)?,
        ];
        let mins = [mid[0] - half[0], mid[1] - half[1], mid[2] - half[2]];
        let maxs = [mid[0] + half[0], mid[1] + half[1], mid[2] + half[2]];

        let mut planes = Vec::with_capacity(6);
        planes.push([1.0, 0.0, 0.0, maxs[0]]);
        planes.push([-1.0, 0.0, 0.0, -mins[0]]);
        planes.push([0.0, 1.0, 0.0, maxs[1]]);
        planes.push([0.0, -1.0, 0.0, -mins[1]]);
        planes.push([0.0, 0.0, 1.0, maxs[2]]);
        planes.push([0.0, 0.0, -1.0, -mins[2]]);

        let mut axial_mat = [0u16; 6];
        for i in 0..6 {
            axial_mat[i] = s
                .u16_at(brush, s.layout(12, 24) + i * 2)
                .map_err(|_| ClipCollisionError::Truncated)?;
        }
        let mut plane_surface_flags = Vec::with_capacity(6);
        plane_surface_flags.extend_from_slice(&axial_plane_flags(axial_mat, |idx| {
            material_flags(s, materials, idx)
        }));

        let numsides = s
            .u16_at(brush, 0)
            .map_err(|_| ClipCollisionError::Truncated)? as usize;
        let glass_encoded = s
            .u16_at(brush, 2)
            .map_err(|_| ClipCollisionError::Truncated)?;
        if let (Some(sides_all), Some(first)) = (sides_base, side_first(s, brush, sides_base)?) {
            for j in 0..numsides {
                let side = sides_all.at((first + j) * s.layout(sz::CBRUSH_SIDE, 16));
                if let Some(plane) = read_side_plane(s, side)? {
                    planes.push(plane);
                    let mat = s
                        .u16_at(side, s.layout(4, 8))
                        .map_err(|_| ClipCollisionError::Truncated)?
                        as usize;
                    plane_surface_flags.push(material_flags(s, materials, mat));
                }
            }
        }

        debug_assert_eq!(planes.len(), plane_surface_flags.len());
        out.brushes.push(ClipBrush {
            planes,
            contents,
            plane_surface_flags,
            glass_encoded,
        });
    }

    extract_bsp_tables(s, g, &mut out)?;
    extract_mesh_tables(s, g, &mut out)?;
    extract_cmodels(s, g, &mut out)?;
    gate_leafbrush_index_fits(out.leafbrushes.len())?;

    Ok(out)
}

fn extract_mesh_tables(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
    out: &mut ClipCollision,
) -> Result<(), ClipCollisionError> {
    let mesh = std::sync::Arc::make_mut(&mut out.mesh);
    if (g.vert_count != 0 && g.verts.is_none())
        || (g.tri_count != 0 && g.tri_indices.is_none())
        || (g.material_count != 0 && g.materials.is_none())
    {
        return Err(ClipCollisionError::MissingTables);
    }
    let Some(verts_ptr) = g.verts else {
        return Ok(());
    };
    mesh.verts.reserve(g.vert_count);
    for i in 0..g.vert_count {
        let v = verts_ptr.at(i * 12);
        mesh.verts.push([
            s.f32_at(v, 0).map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(v, 4).map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(v, 8).map_err(|_| ClipCollisionError::Truncated)?,
        ]);
    }
    if let Some(idx_ptr) = g.tri_indices {
        let n = g.tri_count.saturating_mul(3);
        mesh.tri_indices.reserve(n);
        for i in 0..n {
            let id = s
                .u16_at(idx_ptr, i * 2)
                .map_err(|_| ClipCollisionError::Truncated)?;
            mesh.tri_indices.push(id);
        }
    }
    if let Some(walk_ptr) = g.tri_edge_is_walkable {
        let n = g.tri_count.saturating_mul(3).div_ceil(32) * 4;
        mesh.tri_edge_is_walkable.reserve(n);
        for i in 0..n {
            let b = s
                .u8_at(walk_ptr, i)
                .map_err(|_| ClipCollisionError::Truncated)?;
            mesh.tri_edge_is_walkable.push(b);
        }
    }
    extract_mesh_materials(s, g, out)?;
    extract_aabb_forest(s, g, out)?;
    Ok(())
}

fn extract_aabb_forest(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
    out: &mut ClipCollision,
) -> Result<(), ClipCollisionError> {
    let mesh = std::sync::Arc::make_mut(&mut out.mesh);
    if let Some(parts) = g.collision_partitions {
        mesh.partitions.reserve(g.partition_count);
        for i in 0..g.partition_count {
            let p = parts.at(i * s.layout(sz::COLLISION_PARTITION, 16));
            let tri_n = s.u8_at(p, 0).map_err(|_| ClipCollisionError::Truncated)?;
            let seg = s.u8_at(p, 2).map_err(|_| ClipCollisionError::Truncated)?;
            let first = s.i32_at(p, 4).map_err(|_| ClipCollisionError::Truncated)?;
            let border_n = s.u8_at(p, 1).map_err(|_| ClipCollisionError::Truncated)?;
            let first_border = mesh.borders.len() as u32;
            if border_n != 0
                && let ZonePtr::Offset(b) =
                    s.ptr_at(p, 8).map_err(|_| ClipCollisionError::Truncated)?
            {
                let b = s.resolve_alias(b);
                for k in 0..border_n as usize {
                    let f = |o: usize| {
                        s.f32_at(b, k * 28 + o)
                            .map_err(|_| ClipCollisionError::Truncated)
                    };
                    mesh.borders.push(clipmap_iw4::ClipBorder {
                        dist_eq: [f(0)?, f(4)?, f(8)?],
                        z_base: f(12)?,
                        z_slope: f(16)?,
                        start: f(20)?,
                        length: f(24)?,
                    });
                }
            }
            let border_count = (mesh.borders.len() as u32 - first_border) as u8;
            mesh.partitions.push(clipmap_iw4::ClipPartition {
                tri_count: tri_n,
                first_tri: first,
                first_vert_segment: seg,
                border_count,
                first_border,
            });
        }
    }
    if let Some(trees) = g.collision_aabb_trees {
        mesh.aabb_trees.reserve(g.aabb_tree_count);
        for i in 0..g.aabb_tree_count {
            let node = trees.at(i * sz::COLLISION_AABB_TREE);
            let origin = [
                s.f32_at(node, 0)
                    .map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(node, 4)
                    .map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(node, 8)
                    .map_err(|_| ClipCollisionError::Truncated)?,
            ];
            let material_index = s
                .u16_at(node, 12)
                .map_err(|_| ClipCollisionError::Truncated)?;
            let child_count = s
                .u16_at(node, 14)
                .map_err(|_| ClipCollisionError::Truncated)?;
            let half_size = [
                s.f32_at(node, 16)
                    .map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(node, 20)
                    .map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(node, 24)
                    .map_err(|_| ClipCollisionError::Truncated)?,
            ];
            let u = s
                .i32_at(node, 28)
                .map_err(|_| ClipCollisionError::Truncated)?;
            mesh.aabb_trees.push(clipmap_iw4::ClipAabbNode {
                origin,
                half_size,
                material_index,
                child_count,
                u,
            });
        }
    }
    mesh.aabb_roots = clipmap_iw4::aabb_forest_roots(&mesh.aabb_trees);
    Ok(())
}

fn extract_mesh_materials(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
    out: &mut ClipCollision,
) -> Result<(), ClipCollisionError> {
    let mesh = std::sync::Arc::make_mut(&mut out.mesh);
    out.materials = extract_clip_materials_iw4(s, g)?;
    if g.tri_count == 0 {
        return Ok(());
    }
    let mut material_sflags = vec![0u32; g.material_count];
    let mut material_cflags = vec![0u32; g.material_count];
    for (i, mat) in out.materials.iter().enumerate() {
        if let Some(slot) = material_sflags.get_mut(i) {
            *slot = mat.surface_flags;
        }
        if let Some(slot) = material_cflags.get_mut(i) {
            *slot = mat.content_flags;
        }
    }
    let (leaves, partitions) = collect_aabb_leaves_iw4(s, g)?;
    mesh.tri_surface_flags =
        clipmap_iw4::flatten_tri_surface_flags(&leaves, &partitions, &material_sflags, g.tri_count);
    mesh.tri_content_flags =
        clipmap_iw4::flatten_tri_surface_flags(&leaves, &partitions, &material_cflags, g.tri_count);
    out.tri_material_index =
        clipmap_iw4::flatten_tri_material_index(&leaves, &partitions, g.tri_count);
    Ok(())
}

fn extract_clip_materials_iw4(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
) -> Result<Vec<ClipMapMaterial>, ClipCollisionError> {
    let mut out = Vec::with_capacity(g.material_count);
    let Some(mats) = g.materials else {
        if g.material_count != 0 {
            return Err(ClipCollisionError::MissingTables);
        }
        return Ok(out);
    };
    for i in 0..g.material_count {
        let slot = mats.at(i * s.layout(sz::CLIP_MATERIAL, 16));
        let name = match s
            .ptr_at(slot, 0)
            .map_err(|_| ClipCollisionError::Truncated)?
        {
            ZonePtr::Offset(p) => s.cstr(s.resolve_alias(p)).unwrap_or("").to_owned(),
            _ => String::new(),
        };
        let surface_flags = s
            .u32_at(slot, s.layout(4, 8))
            .map_err(|_| ClipCollisionError::Truncated)?;
        let content_flags = s
            .u32_at(slot, s.layout(8, 12))
            .map_err(|_| ClipCollisionError::Truncated)?;
        out.push(ClipMapMaterial {
            name,
            source_surface_flags: surface_flags,
            surface_flags,
            content_flags,
        });
    }
    Ok(out)
}

fn collect_aabb_leaves_iw4(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
) -> Result<(Vec<(u16, i32)>, Vec<(u8, i32)>), ClipCollisionError> {
    let mut partitions = vec![(0u8, 0i32); g.partition_count];
    if let Some(parts) = g.collision_partitions {
        for (i, slot) in partitions.iter_mut().enumerate() {
            let p = parts.at(i * s.layout(sz::COLLISION_PARTITION, 16));
            let tri_n = s.u8_at(p, 0).map_err(|_| ClipCollisionError::Truncated)?;
            let first = s.i32_at(p, 4).map_err(|_| ClipCollisionError::Truncated)?;
            *slot = (tri_n, first);
        }
    }
    let mut leaves = Vec::new();
    if let Some(trees) = g.collision_aabb_trees {
        for i in 0..g.aabb_tree_count {
            let node = trees.at(i * sz::COLLISION_AABB_TREE);
            let child_count = s
                .u16_at(node, 14)
                .map_err(|_| ClipCollisionError::Truncated)?;
            if child_count != 0 {
                continue;
            }
            let mat = s
                .u16_at(node, 12)
                .map_err(|_| ClipCollisionError::Truncated)?;
            let part = s
                .i32_at(node, 28)
                .map_err(|_| ClipCollisionError::Truncated)?;
            leaves.push((mat, part));
        }
    }
    Ok((leaves, partitions))
}

fn extract_cmodels(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
    out: &mut ClipCollision,
) -> Result<(), ClipCollisionError> {
    let Some(cmodels_ptr) = g.cmodels else {
        return Ok(());
    };
    let lb_nodes = g.leafbrush_nodes;
    out.cmodels.reserve(g.cmodel_count);
    for i in 0..g.cmodel_count {
        let cm = cmodels_ptr.at(i * sz::CMODEL);
        let mins = [
            s.f32_at(cm, 0).map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(cm, 4).map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(cm, 8).map_err(|_| ClipCollisionError::Truncated)?,
        ];
        let maxs = [
            s.f32_at(cm, 12)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(cm, 16)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(cm, 20)
                .map_err(|_| ClipCollisionError::Truncated)?,
        ];
        let radius = s
            .f32_at(cm, 24)
            .map_err(|_| ClipCollisionError::Truncated)?;

        let lb_index = s
            .i32_at(cm, 64)
            .map_err(|_| ClipCollisionError::Truncated)?;
        let first_len = out.leafbrushes.len();
        if lb_index >= 0 {
            if let Some(lb_base) = lb_nodes {
                append_leafbrush_node(s, lb_base, lb_index as usize, &mut out.leafbrushes)?;
            }
        }
        let (first, num) = leafbrush_range(first_len, out.leafbrushes.len())?;
        out.cmodels.push(ClipCmodel {
            mins,
            maxs,
            radius,
            first_brush: first,
            num_brushes: num,
        });
    }
    Ok(())
}

fn extract_bsp_tables(
    s: &ZoneStream<'_>,
    g: ClipMapGeometry,
    out: &mut ClipCollision,
) -> Result<(), ClipCollisionError> {
    let Some(nodes_ptr) = g.nodes else {
        return Ok(());
    };
    let Some(leaves_ptr) = g.leaves else {
        return Ok(());
    };
    let Some(leafbrushes_ptr) = g.leafbrushes else {
        return Ok(());
    };

    out.nodes.reserve(g.node_count);
    for i in 0..g.node_count {
        let node = nodes_ptr.at(i * s.layout(sz::C_NODE, 16));
        let plane = match s
            .ptr_at(node, 0)
            .map_err(|_| ClipCollisionError::Truncated)?
        {
            ZonePtr::Offset(p) => [
                s.f32_at(p, 0).map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(p, 4).map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(p, 8).map_err(|_| ClipCollisionError::Truncated)?,
                s.f32_at(p, 12).map_err(|_| ClipCollisionError::Truncated)?,
            ],
            _ => [0.0, 0.0, 1.0, 0.0],
        };
        let c0 = s
            .i16_at(node, s.layout(4, 8))
            .map_err(|_| ClipCollisionError::Truncated)? as i32;
        let c1 = s
            .i16_at(node, s.layout(6, 10))
            .map_err(|_| ClipCollisionError::Truncated)? as i32;
        out.nodes.push(ClipBspNode {
            plane,
            children: [c0, c1],
        });
    }

    let lb_nodes = g.leafbrush_nodes;
    out.leaves.reserve(g.leaf_count);
    for i in 0..g.leaf_count {
        let leaf = leaves_ptr.at(i * sz::C_LEAF);
        let lb_index = s
            .i32_at(leaf, 36)
            .map_err(|_| ClipCollisionError::Truncated)?;
        let first_len = out.leafbrushes.len();
        if lb_index >= 0 {
            if let Some(lb_base) = lb_nodes {
                append_leafbrush_node(s, lb_base, lb_index as usize, &mut out.leafbrushes)?;
            }
        }
        let (first, num) = leafbrush_range(first_len, out.leafbrushes.len())?;
        let first_coll_aabb_index = s
            .u16_at(leaf, 0)
            .map_err(|_| ClipCollisionError::Truncated)?;
        let coll_aabb_count = s
            .u16_at(leaf, 2)
            .map_err(|_| ClipCollisionError::Truncated)?;
        out.leaves.push(ClipBspLeaf {
            first_brush: first,
            num_brushes: num,
            first_coll_aabb_index,
            coll_aabb_count,
        });
    }

    if out.leafbrushes.is_empty() && g.leafbrush_count > 0 {
        for i in 0..g.leafbrush_count {
            let id = s
                .u16_at(leafbrushes_ptr, i * 2)
                .map_err(|_| ClipCollisionError::Truncated)?;
            out.leafbrushes.push(id);
        }
    }

    Ok(())
}

const LEAFBRUSH_NODE_MAX_DEPTH: usize = 64;

fn append_leafbrush_node(
    s: &ZoneStream<'_>,
    lb_base: Ptr,
    index: usize,
    out: &mut Vec<u16>,
) -> Result<(), ClipCollisionError> {
    append_leafbrush_node_at(s, lb_base, index, 0, out)
}

fn append_leafbrush_node_at(
    s: &ZoneStream<'_>,
    lb_base: Ptr,
    index: usize,
    depth: usize,
    out: &mut Vec<u16>,
) -> Result<(), ClipCollisionError> {
    if depth > LEAFBRUSH_NODE_MAX_DEPTH {
        return Ok(());
    }
    let node = lb_base.at(index * s.layout(sz::C_LEAF_BRUSH_NODE, 24));
    let count = s
        .i16_at(node, 2)
        .map_err(|_| ClipCollisionError::Truncated)?;
    if count > 0 {
        if let ZonePtr::Offset(brushes) = s
            .ptr_at(node, 8)
            .map_err(|_| ClipCollisionError::Truncated)?
        {
            for j in 0..count as usize {
                out.push(
                    s.u16_at(brushes, j * 2)
                        .map_err(|_| ClipCollisionError::Truncated)?,
                );
            }
        }
        return Ok(());
    }

    let off0 = s
        .u16_at(node, 16)
        .map_err(|_| ClipCollisionError::Truncated)? as usize;
    let off1 = s
        .u16_at(node, 18)
        .map_err(|_| ClipCollisionError::Truncated)? as usize;
    for next in leafbrush_node_visits(index, count, off0, off1) {
        append_leafbrush_node_at(s, lb_base, next, depth + 1, out)?;
    }
    Ok(())
}

fn leafbrush_node_visits(index: usize, count: i16, off0: usize, off1: usize) -> Vec<usize> {
    let mut visits = Vec::with_capacity(3);
    if count < 0 {
        visits.push(index + 1);
    }

    if off0 != 0 {
        visits.push(index + off0);
    }
    if off1 != 0 {
        visits.push(index + off1);
    }
    visits
}

fn side_first(
    s: &ZoneStream<'_>,
    brush: Ptr,
    sides_all: Option<Ptr>,
) -> Result<Option<usize>, ClipCollisionError> {
    let Some(sides_all) = sides_all else {
        return Ok(None);
    };
    match s
        .ptr_at(brush, s.layout(4, 8))
        .map_err(|_| ClipCollisionError::Truncated)?
    {
        ZonePtr::Offset(p) if p.block == sides_all.block && p.offset >= sides_all.offset => Ok(
            Some((p.offset - sides_all.offset) as usize / s.layout(sz::CBRUSH_SIDE, 16)),
        ),
        _ => Ok(None),
    }
}

fn read_side_plane(s: &ZoneStream<'_>, side: Ptr) -> Result<Option<[f32; 4]>, ClipCollisionError> {
    match s
        .ptr_at(side, 0)
        .map_err(|_| ClipCollisionError::Truncated)?
    {
        ZonePtr::Offset(plane) => Ok(Some([
            s.f32_at(plane, 0)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(plane, 4)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(plane, 8)
                .map_err(|_| ClipCollisionError::Truncated)?,
            s.f32_at(plane, 12)
                .map_err(|_| ClipCollisionError::Truncated)?,
        ])),
        _ => Ok(None),
    }
}

fn material_flags(s: &ZoneStream<'_>, materials: Option<Ptr>, index: usize) -> u32 {
    let Some(base) = materials else {
        return 0;
    };
    s.u32_at(
        base.at(index * s.layout(sz::CLIP_MATERIAL, 16)),
        s.layout(4, 8),
    )
    .unwrap_or(0)
}

fn axial_plane_flags(axial_mat: [u16; 6], flag_of: impl Fn(usize) -> u32) -> [u32; 6] {
    [
        flag_of(axial_mat[3] as usize),
        flag_of(axial_mat[0] as usize),
        flag_of(axial_mat[4] as usize),
        flag_of(axial_mat[1] as usize),
        flag_of(axial_mat[5] as usize),
        flag_of(axial_mat[2] as usize),
    ]
}

/// Collision counts for the pack report (content-free).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ClipReport {
    pub brushes: usize,
    pub planes: usize,
    pub nodes: usize,
    pub leaves: usize,
    pub leafbrushes: usize,
    pub mesh_triangles: usize,
    pub brush_models: usize,
}

impl ClipReport {
    pub fn of(c: &wpk::clip::Clip) -> ClipReport {
        ClipReport {
            brushes: c.brushes.len(),
            planes: c.planes.len(),
            nodes: c.nodes.len(),
            leaves: c.leaves.len(),
            leafbrushes: c.leafbrushes.len(),
            mesh_triangles: c.mesh.tri_count(),
            brush_models: c.cmodels.len(),
        }
    }
}

/// Read the zone's clipMap (after a walk) into the pack's collision tables, unchanged.
pub fn read_clip(s: &ZoneStream<'_>) -> Result<wpk::clip::Clip, String> {
    let g = s.clip_map().ok_or("zone has no clipMap")?;
    let c = build_clip_collision(s, g).map_err(|e| e.to_string())?;
    Ok(to_pack(c))
}

fn to_pack(c: ClipCollision) -> wpk::clip::Clip {
    let mut out = wpk::clip::Clip::default();
    for b in &c.brushes {
        out.push_brush(
            &b.planes,
            &b.plane_surface_flags,
            b.contents,
            b.glass_encoded,
        );
    }
    out.nodes = c.nodes;
    out.leaves = c.leaves;
    out.leafbrushes = c.leafbrushes;
    out.mesh = std::sync::Arc::unwrap_or_clone(c.mesh);
    out.cmodels = c
        .cmodels
        .iter()
        .map(|m| clipmap_iw4::ClipCmodel {
            mins: m.mins,
            maxs: m.maxs,
            radius: m.radius,
            first_brush: m.first_brush,
            num_brushes: m.num_brushes,
        })
        .collect();
    out
}

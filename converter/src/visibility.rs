//! World visibility from a map zone's GfxWorld: the dpvs BSP (planes and nodes), cells and their portals, and which
//! cell each draw surface and static model belongs to (from each cell's AABB tree).
//!
//! Layouts follow IW4L `crates/asset_world/src/world_draw.rs` (`extract_dpvs`), Apache-2.0, rev `acc57a5`.

use asset_iw4::size as sz;
use fastfile_iw4::{ZonePtr, ZoneStream};
use wpk::vis::{ALWAYS, Cell, Plane, Portal, Vis};

#[derive(Clone, Debug, Default)]
pub struct WorldVis {
    pub vis: Vis,
    /// Cell of each GfxWorld surface (`ALWAYS` when no cell lists it).
    pub surface_cell: Vec<u16>,
    /// Cell of each static model (GfxWorld static model index).
    pub smodel_cell: Vec<u16>,
}

pub fn read(s: &ZoneStream<'_>) -> Result<WorldVis, String> {
    let g = s.gfx_world().ok_or("zone has no GfxWorld")?;
    let e = |e: fastfile_iw4::ZoneError| format!("{e:?}");
    let mut out = WorldVis {
        surface_cell: vec![ALWAYS; g.surface_count],
        smodel_cell: vec![ALWAYS; g.smodel_count],
        ..WorldVis::default()
    };
    let (Some(planes), Some(nodes), Some(cells)) = (g.planes, g.nodes, g.cells) else {
        return Ok(out);
    };
    for i in 0..g.plane_count {
        let p = planes.at(i * sz::CPLANE);
        out.vis.planes.push(Plane {
            normal: [
                s.f32_at(p, 0).map_err(e)?,
                s.f32_at(p, 4).map_err(e)?,
                s.f32_at(p, 8).map_err(e)?,
            ],
            dist: s.f32_at(p, 12).map_err(e)?,
            kind: s.u8_at(p, 16).map_err(e)?,
        });
    }
    for i in 0..g.node_count {
        out.vis.nodes.push(s.u16_at(nodes, i * 2).map_err(e)?);
    }

    for i in 0..g.cell_count {
        let cell = cells.at(i * s.layout(sz::GFX_CELL, 56));
        let f = |o| s.f32_at(cell, o).map_err(e);
        let first_portal = out.vis.portals.len() as u32;
        let portal_count = s.i32_at(cell, 24).map_err(e)?.max(0) as usize;
        if let ZonePtr::Offset(arr) = s.ptr_at(cell, s.layout(28, 32)).map_err(e)? {
            for j in 0..portal_count {
                let portal = arr.at(j * s.layout(sz::GFX_PORTAL, 80));
                let pf = |o| s.f32_at(portal, o).map_err(e);
                let plane = [
                    pf(s.layout(12, 24))?,
                    pf(s.layout(16, 28))?,
                    pf(s.layout(20, 32))?,
                    pf(s.layout(24, 36))?,
                ];
                let neighbor = s.u16_at(portal, s.layout(32, 48)).map_err(e)?;
                let vert_count = usize::from(s.u8_at(portal, s.layout(34, 50)).map_err(e)?);
                let first_vert = out.vis.verts.len() as u32;
                if let ZonePtr::Offset(verts) = s.ptr_at(portal, s.layout(28, 40)).map_err(e)? {
                    for k in 0..vert_count {
                        let v = verts.at(k * 12);
                        out.vis.verts.push([
                            s.f32_at(v, 0).map_err(e)?,
                            s.f32_at(v, 4).map_err(e)?,
                            s.f32_at(v, 8).map_err(e)?,
                        ]);
                    }
                }
                let h = s.layout(sz::GFX_PORTAL_HULL_AXIS, 52);
                let hull_axis = [
                    [pf(h)?, pf(h + 4)?, pf(h + 8)?],
                    [pf(h + 12)?, pf(h + 16)?, pf(h + 20)?],
                ];
                out.vis.portals.push(Portal {
                    plane,
                    neighbor,
                    first_vert,
                    vert_count: out.vis.verts.len() as u32 - first_vert,
                    hull_axis: Some(hull_axis),
                });
            }
        }
        out.vis.cells.push(Cell {
            mins: [f(0)?, f(4)?, f(8)?],
            maxs: [f(12)?, f(16)?, f(20)?],
            first_portal,
            portal_count: out.vis.portals.len() as u32 - first_portal,
        });
    }

    // Surfaces and static models of each cell: the root of the cell's AABB tree spans the cell's range in the sorted
    // surface index; every node lists the static models it holds.
    if let (Some(counts), Some(trees), Some(sorted)) =
        (g.aabb_tree_counts, g.aabb_trees, g.sorted_surf_index)
    {
        let sorted_len = g.static_surface_count + g.static_surface_count_no_decal;
        for cell in 0..g.cell_count {
            let count = s.i32_at(counts, cell * 4).map_err(e)?.max(0) as usize;
            let ZonePtr::Offset(tree) = s.ptr_at(trees, cell * s.pointer_bytes()).map_err(e)?
            else {
                continue;
            };
            if count == 0 {
                continue;
            }
            let start = usize::from(s.u16_at(tree, 28).map_err(e)?);
            let n = usize::from(s.u16_at(tree, 26).map_err(e)?);
            for k in start..(start + n).min(sorted_len) {
                let surf = usize::from(s.u16_at(sorted, k * 2).map_err(e)?);
                if let Some(c) = out.surface_cell.get_mut(surf)
                    && *c == ALWAYS
                {
                    *c = cell as u16;
                }
            }
            for j in 0..count {
                let node = tree.at(j * s.layout(sz::GFX_AABB_TREE, 56));
                let smodels = usize::from(s.u16_at(node, 34).map_err(e)?);
                if let ZonePtr::Offset(idx) = s.ptr_at(node, s.layout(36, 40)).map_err(e)? {
                    for k in 0..smodels {
                        let m = usize::from(s.u16_at(idx, k * 2).map_err(e)?);
                        if let Some(c) = out.smodel_cell.get_mut(m)
                            && *c == ALWAYS
                        {
                            *c = cell as u16;
                        }
                    }
                }
            }
        }
    }
    out.vis
        .validate()
        .map_err(|e| format!("visibility: {e:?}"))?;
    Ok(out)
}

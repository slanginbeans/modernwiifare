//! Static models of a map: the placed instances (`GfxWorld` static model draw instances), the LOD 0 surfaces of each
//! XModel they use (model-space packed vertices + triangle lists + material), and the light each instance is lit
//! with (sampled from the world's light grid at its lighting origin, the way IW4 lights static models).
//!
//! Instance and surface layouts follow IW4L (`asset_world/src/model_mesh.rs` `build_static_model_instances`,
//! `asset_model/src/model_skel.rs` XSurface reading), Apache-2.0, rev `acc57a5`.

use std::collections::{BTreeMap, HashMap};

use asset_iw4::size as sz;
use fastfile_iw4::{Ptr, ZonePtr, ZoneStream};
use wpk::records::WorldVertex;

use crate::light_grid::{OwnedLightGrid, mean_tile_rgba01, sample_light_grid};
use crate::material_links::Links;

/// One surface of a model's LOD 0.
#[derive(Clone, Debug)]
pub struct ModelSurface {
    /// Index into `Links::material_info`.
    pub material: Option<usize>,
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u16>,
}

/// One level of detail: its surfaces and IW4's switch distance (`lodDist`).
#[derive(Clone, Debug)]
pub struct ModelLod {
    pub dist: f32,
    pub surfaces: Vec<ModelSurface>,
}

#[derive(Clone, Debug)]
pub struct Instance {
    /// GfxWorld static model index (what visibility data refers to).
    pub index: usize,
    /// Index into `Links::models`.
    pub model: usize,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
    pub scale: f32,
    pub cull_dist: u16,
    /// Light multiplier in gamma space (1 = the texture as authored), per channel.
    pub light: [f32; 3],
}

#[derive(Clone, Debug, Default)]
pub struct StaticModels {
    pub instances: Vec<Instance>,
    /// The LODs of each model the instances use (from IW4's `lodStart`).
    pub meshes: BTreeMap<usize, Vec<ModelLod>>,
    /// Instances whose model slot did not resolve, models without readable LOD 0, unlit instances.
    pub unresolved: usize,
    pub unreadable: usize,
    /// Why models could not be read (reason -> instances).
    pub unreadable_why: BTreeMap<&'static str, usize>,
    pub unlit: usize,
}

const DRAW_INST: usize = sz::GFX_STATIC_MODEL_DRAW_INST;
const INST: usize = lighting_iw4::GFX_STATIC_MODEL_INST_SIZE;

pub(crate) fn half(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let man = f32::from(h & 0x3ff);
    sign * match exp {
        0 => man * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

pub(crate) fn target(s: &ZoneStream<'_>, p: Ptr, off: usize) -> Option<Ptr> {
    match s.ptr_at(p, off).ok()? {
        ZonePtr::Offset(t) => Some(s.resolve_alias(t)),
        _ => None,
    }
}

/// LOD 0 surfaces of model `m`, or None when they cannot be read.
/// XModelSurfs sets by name. A model whose LOD surfaces were loaded earlier by another model only carries the set's
/// name (IW4 shares them); IW4L resolves them the same way (`model_skel.rs`).
pub(crate) fn surface_sets(s: &ZoneStream<'_>, links: &Links) -> HashMap<String, Ptr> {
    let mut sets = HashMap::new();
    for g in &links.model_geometry {
        for lod in 0..4 {
            if let (Some(surfs), Some(name)) = (g.lod_xsurfaces[lod], g.lod_surface_names[lod])
                && let Ok(n) = s.cstr(s.resolve_alias(name))
            {
                sets.entry(n.trim_start_matches(',').to_owned())
                    .or_insert(surfs);
            }
        }
    }
    sets
}

/// Every LOD of model `m` from IW4's `lodStart`. The first must read; a later one that doesn't ends the list.
fn lods(
    s: &ZoneStream<'_>,
    links: &Links,
    sets: &HashMap<String, Ptr>,
    m: usize,
) -> Result<Vec<ModelLod>, &'static str> {
    let g = links.model_geometry.get(m).ok_or("no geometry")?;
    let start = usize::from(g.lod_start).min(3);
    let end = usize::from(g.num_lods).clamp(start + 1, 4);
    let mut out = Vec::new();
    for l in start..end {
        match lod(s, links, sets, m, l) {
            Ok(surfaces) => out.push(ModelLod {
                dist: g.lod_dist[l],
                surfaces,
            }),
            Err(e) if out.is_empty() => return Err(e),
            Err(_) => break,
        }
    }
    Ok(out)
}

fn lod(
    s: &ZoneStream<'_>,
    links: &Links,
    sets: &HashMap<String, Ptr>,
    m: usize,
    lod: usize,
) -> Result<Vec<ModelSurface>, &'static str> {
    let g = links.model_geometry.get(m).ok_or("no geometry")?;
    let surfaces = match g.lod_xsurfaces[lod] {
        Some(p) => p,
        None => {
            let name = g.lod_surface_names[lod].ok_or("no lod surfaces")?;
            let name = s
                .cstr(s.resolve_alias(name))
                .map_err(|_| "lod surface name")?;
            *sets
                .get(name.trim_start_matches(','))
                .ok_or("shared lod surfaces not found")?
        }
    };
    let count = usize::from(g.lod_numsurfs[lod]);
    let handle_base = usize::from(g.lod_surf_index[lod]);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let surf = surfaces.at(i * sz::XSURFACE);
        let vertex_count = usize::from(s.u16_at(surf, 2).map_err(|_| "surface header")?);
        let tri_count = usize::from(s.u16_at(surf, 4).map_err(|_| "surface header")?);
        let verts = target(s, surf, 28).ok_or("vertex pointer")?;
        let tris = target(s, surf, 12).ok_or("triangle pointer")?;
        let mut vertices = Vec::with_capacity(vertex_count);
        for v in 0..vertex_count {
            let p = verts.at(v * sz::GFX_PACKED_VERTEX);
            let f = |o| s.f32_at(p, o).map_err(|_| "vertex data");
            let bgra = s.u32_at(p, 16).map_err(|_| "vertex data")?.to_le_bytes();
            let tc = s.u32_at(p, 20).map_err(|_| "vertex data")?;
            vertices.push(WorldVertex {
                pos: [f(0)?, f(4)?, f(8)?],
                rgba: u32::from_be_bytes([bgra[2], bgra[1], bgra[0], bgra[3]]),
                uv: [half((tc >> 16) as u16), half(tc as u16)],
                lm: [0, 0],
            });
        }
        let mut indices = Vec::with_capacity(tri_count * 3);
        for t in 0..tri_count * 3 {
            let idx = s.u16_at(tris, t * 2).map_err(|_| "triangle data")?;
            if usize::from(idx) >= vertex_count {
                return Err("triangle index out of range");
            }
            indices.push(idx);
        }
        let slot = handle_base + i;
        let material = (slot < g.material_handle_count)
            .then(|| {
                g.material_handles
                    .and_then(|h| links.resolve_material(h.at(slot * s.pointer_bytes())))
            })
            .flatten();
        out.push(ModelSurface {
            material,
            vertices,
            indices,
        });
    }
    Ok(out)
}

/// Read the static models after a zone walk (the stream still holds its blocks).
pub fn read(s: &ZoneStream<'_>, links: &Links) -> Result<StaticModels, String> {
    let g = s.gfx_world().ok_or("zone has no GfxWorld")?;
    let mut out = StaticModels::default();
    let (Some(draw_insts), Some(insts)) = (g.smodel_draw_insts, g.smodel_insts) else {
        return Ok(out);
    };
    let grid = OwnedLightGrid::from_stream(s, g.light_grid);
    let sets = surface_sets(s, links);
    let e = |e: fastfile_iw4::ZoneError| format!("{e:?}");
    for i in 0..g.smodel_count {
        let d = draw_insts.at(i * DRAW_INST);
        let Some(model) = links.resolve_model(d.at(0x34)) else {
            out.unresolved += 1;
            continue;
        };
        if let std::collections::btree_map::Entry::Vacant(e) = out.meshes.entry(model) {
            match lods(s, links, &sets, model) {
                Ok(m) => {
                    e.insert(m);
                }
                Err(why) => {
                    out.unreadable += 1;
                    *out.unreadable_why.entry(why).or_default() += 1;
                    continue;
                }
            }
        }
        let mut axis = [[0.0; 3]; 3];
        for (r, row) in axis.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = s.f32_at(d, 12 + (r * 3 + c) * 4).map_err(e)?;
            }
        }
        let origin = [
            s.f32_at(d, 0).map_err(e)?,
            s.f32_at(d, 4).map_err(e)?,
            s.f32_at(d, 8).map_err(e)?,
        ];
        let lighting_origin = s
            .slice_at(insts.at(i * INST), 0, INST)
            .ok()
            .and_then(lighting_iw4::lighting_origin_from_inst_bytes)
            .unwrap_or(origin);
        let light = match grid
            .as_ref()
            .map(|g| sample_light_grid(&g.view(), lighting_origin))
        {
            Some(Ok(sample)) => {
                // Linear light for a white surface, back to gamma space (IW4 squares the albedo, see lit_albedo).
                let lin =
                    lighting_iw4::decode_model_lighting_sample(mean_tile_rgba01(&sample.tile));
                lin.map(|c| c.max(0.0).sqrt())
            }
            _ => {
                out.unlit += 1;
                [1.0; 3]
            }
        };
        out.instances.push(Instance {
            index: i,
            model,
            origin,
            axis,
            scale: s.f32_at(d, 0x30).map_err(e)?,
            cull_dist: s.u16_at(d, 0x38).map_err(e)?,
            light,
        });
    }
    Ok(out)
}

impl Instance {
    /// Model-to-world matrix rows: world = origin + scale · (x·axis[0] + y·axis[1] + z·axis[2]).
    pub fn matrix(&self) -> [[f32; 4]; 3] {
        let a = &self.axis;
        let s = self.scale;
        std::array::from_fn(|r| [a[0][r] * s, a[1][r] * s, a[2][r] * s, self.origin[r]])
    }

    /// Light as RGBA8 at half intensity (the GPU doubles it).
    pub fn light_rgba(&self) -> u32 {
        let c = self
            .light
            .map(|v| ((v * 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u32);
        (c[0] << 24) | (c[1] << 16) | (c[2] << 8) | 0xff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0), 0.0);
    }

    #[test]
    fn instance_matrix_places_and_scales() {
        let i = Instance {
            index: 0,
            model: 0,
            origin: [100.0, 200.0, 300.0],
            // Rotated 90° about Z: model +X points along world +Y.
            axis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 2.0,
            cull_dist: 0,
            light: [1.0, 0.5, 2.0],
        };
        let m = i.matrix();
        let p = [1.0, 0.0, 0.0];
        let w: [f32; 3] =
            std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]);
        assert_eq!(w, [100.0, 202.0, 300.0]);
        assert_eq!(i.light_rgba(), 0x8040_ffff);
    }
}

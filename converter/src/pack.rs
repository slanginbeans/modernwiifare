//! `mwf-convert pack`: one map zone to a `.wpk` the Wii loads (see `shared/wpk`).
//!
//! The world is read from the zone's GfxWorld (layouts in `docs/iw4l-world.md`) and written for GX:
//! - **Vertices**: the GfxWorld vertex buffer as is, re-encoded big-endian into the 28-byte interleaved layout
//!   (`wpk::records::WorldVertex`).
//! - **Batches**: surfaces grouped by vertex group (a span under 65,536 vertices, so u16 indices reach every vertex),
//!   a 1024-unit grid cell (so batches can be frustum-culled), material and lightmap. Each batch is one display list.
//!   Sky-material surfaces are dropped: the skybox (six faces) replaces them.
//! - **Lightmaps**: IW4 lights a surface by `(page0 + page1)²` in linear space (`docs/iw4l-world.md`). GX blends in gamma
//!   space, where that is `page0 + page1`; it is stored halved and the TEV scales it by 2.
//! - **Textures**: the `.gxt` files `mwf-convert textures` wrote, found through its `index.json`.
//! - **Spawns**: player spawn entities from the map's entity string.
//!
//! Not yet: static models, collision, visibility cells (frustum culling of batches only).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use fastfile_iw4::ZoneStream;
use gx_texture::image::Image;
use gx_texture::policy::{Role, convert};
use serde::Serialize;
use wpk::records::{
    self, BatchRecord, Blend, Cull, InstanceRecord, MaterialRecord, ModelRecord, NONE, Recipe,
    Skybox, SpawnRecord, TextureRecord, WorldVertex, Wrap, spawn_kind, spawn_team, texture_flags,
};

use crate::material_links::{BlendMode, Links};
use crate::zone_textures::{IndexEntry, ZoneImage};

const GFX_WORLD_VERTEX: usize = 44;
const GFX_SURFACE: usize = 24;
/// D3DFMT_A8R8G8B8: IW4 lightmap secondary images.
const D3D_A8R8G8B8: u32 = 21;
/// Grid cell size for splitting world batches (IW4 units; 1 unit = 1 inch).
pub const CELL: f32 = 1024.0;
/// IW4 sort keys of sky materials.
const SORT_SKY: u8 = 2;
const SORT_SKYBOX: u8 = 3;

/// One GfxSurface.
#[derive(Clone, Copy, Debug)]
struct Surface {
    material: Option<usize>,
    lightmap: u8,
    first_vertex: u32,
    vertex_count: u16,
    tri_count: u16,
    base_index: u32,
}

/// What the converter reads from the zone's GfxWorld and MapEnts.
pub struct WorldData {
    vertices: Vec<WorldVertex>,
    indices: Vec<u16>,
    surfaces: Vec<Surface>,
    /// Lightmap pages the world uses (surfaces index `0..lightmap_count`).
    lightmap_count: usize,
    entity_string: String,
    /// Collision, or why it could not be read.
    clip: Result<wpk::clip::Clip, String>,
    models: crate::static_models::StaticModels,
    vis: crate::visibility::WorldVis,
}

/// Read the world after a zone walk (the stream still holds its blocks).
pub fn read_world(s: &ZoneStream<'_>, links: &Links) -> Result<WorldData, String> {
    let g = s.gfx_world().ok_or("zone has no GfxWorld")?;
    let vbase = g.vertices.ok_or("GfxWorld without vertices")?;
    let mut vertices = Vec::with_capacity(g.vertex_count);
    for i in 0..g.vertex_count {
        let v = vbase.at(i * GFX_WORLD_VERTEX);
        let f = |o| s.f32_at(v, o).map_err(|e| format!("{e:?}"));
        let bgra = s.u32_at(v, 16).map_err(|e| format!("{e:?}"))?.to_le_bytes();
        vertices.push(WorldVertex {
            pos: [f(0)?, f(4)?, f(8)?],
            rgba: u32::from_be_bytes([bgra[2], bgra[1], bgra[0], bgra[3]]),
            uv: [f(20)?, f(24)?],
            lm: [WorldVertex::lm_fixed(f(28)?), WorldVertex::lm_fixed(f(32)?)],
        });
    }
    let ibase = g.indices.ok_or("GfxWorld without indices")?;
    let indices = (0..g.index_count)
        .map(|i| s.u16_at(ibase, i * 2).map_err(|e| format!("{e:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    let sbase = g.surfaces.ok_or("GfxWorld without surfaces")?;
    let mut surfaces = Vec::with_capacity(g.surface_count);
    for i in 0..g.surface_count {
        let p = sbase.at(i * GFX_SURFACE);
        let e = |e: fastfile_iw4::ZoneError| format!("{e:?}");
        surfaces.push(Surface {
            material: links.resolve_material(p.at(16)),
            lightmap: s.u8_at(p, 20).map_err(e)?,
            first_vertex: s.u32_at(p, 4).map_err(e)?,
            vertex_count: s.u16_at(p, 8).map_err(e)?,
            tri_count: s.u16_at(p, 10).map_err(e)?,
            base_index: s.u32_at(p, 12).map_err(e)?,
        });
    }
    let lightmap_count = g.lightmap_count;
    let entity_string = s
        .map_ents()
        .and_then(|m| {
            let p = m.entity_string?;
            s.slice_at(p, 0, m.entity_chars).ok()
        })
        .map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_owned())
        .unwrap_or_default();
    let clip = crate::collision::read_clip(s);
    let models = crate::static_models::read(s, links)?;
    let vis = crate::visibility::read(s)?;
    Ok(WorldData {
        vertices,
        indices,
        surfaces,
        lightmap_count,
        entity_string,
        clip,
        models,
        vis,
    })
}

/// Lightmap `i` as an RGBA8 image of `(page0 + page1) / 2`, from the zone image IW4 names `*lightmap<i>_secondary`.
/// (The GfxWorld lightmap array points at these images, which load earlier in the zone; IW4L records the pair only
/// when the image loads inline, so the converter finds them in its image list by IW4's fixed name.)
fn lightmap(images: &[ZoneImage], i: usize) -> Option<Image> {
    let name = format!("*lightmap{i}_secondary");
    let img = images.iter().find(|img| img.name == name)?;
    if img.d3d_format != D3D_A8R8G8B8 || img.height % 2 != 0 || img.payload.is_empty() {
        return None;
    }
    lightmap_from_secondary(img.width, img.height, &img.payload)
}

/// A secondary lightmap image (BGRA8, page0 stacked over page1) to one page of `(page0 + page1) / 2`.
fn lightmap_from_secondary(w: u32, h: u32, bgra: &[u8]) -> Option<Image> {
    let page = (w * h / 2) as usize;
    if bgra.len() < page * 2 * 4 {
        return None;
    }
    let px = |i: usize| [bgra[i * 4 + 2], bgra[i * 4 + 1], bgra[i * 4]];
    let out = (0..page)
        .map(|i| {
            let (a, b) = (px(i), px(page + i));
            let c = |k: usize| ((u16::from(a[k]) + u16::from(b[k])) / 2) as u8;
            [c(0), c(1), c(2), 255]
        })
        .collect();
    Some(Image {
        width: w,
        height: h / 2,
        pixels: out,
    })
}

/// A parsed map entity: key/value pairs.
pub fn parse_entities(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = Vec::new();
    let mut cur: Option<BTreeMap<String, String>> = None;
    let mut key: Option<String> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => cur = Some(BTreeMap::new()),
            '}' => {
                out.extend(cur.take());
                key = None;
            }
            '"' => {
                let s: String = chars.by_ref().take_while(|&c| c != '"').collect();
                match key.take() {
                    None => key = Some(s),
                    Some(k) => {
                        if let Some(e) = cur.as_mut() {
                            e.insert(k, s);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Player spawns from parsed entities: TDM (`mp_tdm_spawn*`) and free-for-all (`mp_dm_spawn*`).
pub fn spawns(entities: &[BTreeMap<String, String>]) -> Vec<SpawnRecord> {
    let mut out = Vec::new();
    for e in entities {
        let Some(class) = e.get("classname") else {
            continue;
        };
        let kind = if class.starts_with("mp_tdm_spawn") {
            spawn_kind::TDM
        } else if class.starts_with("mp_dm_spawn") {
            spawn_kind::DM
        } else {
            continue;
        };
        let num = |s: &str| -> Vec<f32> {
            s.split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect()
        };
        let origin = e.get("origin").map(|o| num(o)).unwrap_or_default();
        if origin.len() != 3 {
            continue;
        }
        let yaw = e
            .get("angles")
            .map(|a| num(a))
            .and_then(|a| a.get(1).copied())
            .unwrap_or(0.0);
        let team = if class.contains("allies") {
            spawn_team::ALLIES
        } else if class.contains("axis") {
            spawn_team::AXIS
        } else {
            spawn_team::ANY
        };
        out.push(SpawnRecord {
            origin: [origin[0], origin[1], origin[2]],
            yaw,
            kind,
            team,
            flags: u8::from(class.ends_with("_start")),
        });
    }
    out
}

/// What went into a pack (content-free: counts and bytes).
#[derive(Clone, Debug, Default, Serialize)]
pub struct PackReport {
    pub map: String,
    pub pack_bytes: u64,
    /// What the Wii keeps in MEM2 (everything before the sections decoded at load).
    pub mem2_bytes: u64,
    pub sections: BTreeMap<String, u64>,
    pub textures: usize,
    pub lightmaps: usize,
    pub materials: usize,
    /// Materials with an IW4 polygon offset (decals), by level 1..3.
    pub polygon_offset_materials: [usize; 3],
    pub world_vertices: usize,
    pub world_triangles: u64,
    /// Vertices the world's display lists send (strips: about one per triangle; lists: three).
    pub world_dl_vertices: u64,
    pub batches: usize,
    pub vertex_groups: usize,
    pub sky_surfaces_dropped: usize,
    pub surfaces_without_material: usize,
    pub materials_without_texture: usize,
    pub skybox: bool,
    pub spawns: usize,
    pub collision: crate::collision::ClipReport,
    pub static_models: StaticModelReport,
    pub visibility: VisReport,
    pub content_hash: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VisReport {
    pub cells: usize,
    pub portals: usize,
    /// World batches / static model instances that no cell lists (always drawn when in the frustum).
    pub batches_always: usize,
    pub instances_always: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StaticModelReport {
    pub instances: usize,
    pub models: usize,
    pub surfaces: usize,
    pub vertices: usize,
    /// LOD 0 triangles of the models (stored once); `lods` counts every LOD packed.
    pub triangles: u64,
    pub lods: usize,
    pub dl_vertices: u64,
    /// Triangles if every instance were drawn (what world-space baking would cost).
    pub placed_triangles: u64,
    pub unresolved: usize,
    pub unreadable: usize,
    pub unreadable_why: BTreeMap<String, usize>,
    pub unlit: usize,
    pub surfaces_without_material: usize,
}

/// Texture data accumulated into `TXDA`.
#[derive(Default)]
struct Textures {
    records: Vec<TextureRecord>,
    data: Vec<u8>,
    /// `.gxt` file name -> texture index.
    by_file: HashMap<String, u16>,
}

impl Textures {
    fn push_blob(&mut self, bytes: &[u8]) -> u32 {
        let off = self.data.len() as u32;
        self.data.extend_from_slice(bytes);
        let padded = (self.data.len() + 31) & !31;
        self.data.resize(padded, 0);
        off
    }

    fn add(&mut self, c: &gx_texture::Converted, wrap: Wrap) -> u16 {
        let data_off = self.push_blob(&c.data);
        let (flags, alpha_off, alpha_len) = match &c.alpha_plane {
            Some(p) => (
                texture_flags::ALPHA_PLANE,
                self.push_blob(p),
                p.len() as u32,
            ),
            None => (0, 0, 0),
        };
        self.records.push(TextureRecord {
            format: c.format.gx_id(),
            levels: c.levels as u8,
            flags,
            wrap: wrap as u8,
            width: c.width as u16,
            height: c.height as u16,
            data_off,
            data_len: c.data.len() as u32,
            alpha_off,
            alpha_len,
        });
        (self.records.len() - 1) as u16
    }

    /// Add a converted `.gxt` from the texture folder (once per file).
    fn add_file(&mut self, dir: &Path, file: &str, wrap: Wrap) -> Result<u16, String> {
        if let Some(&i) = self.by_file.get(file) {
            return Ok(i);
        }
        let bytes = std::fs::read(dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        let (h, data) =
            gx_texture::container::parse(&bytes).map_err(|e| format!("{file}: {e:?}"))?;
        let plane = gx_texture::container::parse_alpha_plane(&bytes)
            .map_err(|e| format!("{file}: {e:?}"))?;
        let c = gx_texture::Converted {
            format: h.format,
            width: u32::from(h.width),
            height: u32::from(h.height),
            levels: u32::from(h.levels),
            traits: gx_texture::policy::Traits {
                grey: false,
                alpha: gx_texture::policy::Alpha::Opaque,
            },
            data: data.to_vec(),
            alpha_plane: plane.map(|(_, p)| p.to_vec()),
        };
        let i = self.add(&c, wrap);
        self.by_file.insert(file.to_owned(), i);
        Ok(i)
    }
}

/// Build the pack bytes for one map. `index` / `tex_dir`: the textures `convert_zone` wrote for this zone.
pub fn build(
    map: &str,
    world: &WorldData,
    links: &Links,
    images: &[ZoneImage],
    index: &[IndexEntry],
    tex_dir: &Path,
) -> Result<(Vec<u8>, PackReport), String> {
    let mut report = PackReport {
        map: map.to_owned(),
        world_vertices: world.vertices.len(),
        ..PackReport::default()
    };
    // Image (load index) -> texture file(s) (cube faces in face order).
    let mut files_by_image: HashMap<usize, Vec<&IndexEntry>> = HashMap::new();
    for e in index {
        for &i in &e.images {
            files_by_image.entry(i).or_default().push(e);
        }
    }

    let mut tex = Textures::default();

    // Lightmaps first: lightmap index -> texture index.
    let mut lightmap_tex: Vec<u16> = Vec::new();
    for i in 0..world.lightmap_count {
        let t = match lightmap(images, i).as_ref() {
            Some(img) => {
                report.lightmaps += 1;
                tex.add(&convert(img, Role::Lightmap), Wrap::Clamp)
            }
            None => NONE,
        };
        lightmap_tex.push(t);
    }

    // Materials actually drawn by world surfaces.
    let mut material_index: HashMap<usize, u16> = HashMap::new();
    let mut materials: Vec<MaterialRecord> = Vec::new();
    let mut sky_image: Option<usize> = None;
    let mut is_sky = |m: usize| {
        let info = &links.material_info[m];
        let sky = info.sort_key == SORT_SKY || info.sort_key == SORT_SKYBOX;
        if sky && sky_image.is_none() {
            sky_image = links.colour_image(m);
        }
        sky
    };
    let mut material_for =
        |m: usize, tex: &mut Textures, report: &mut PackReport| -> Result<u16, String> {
            if let Some(&i) = material_index.get(&m) {
                return Ok(i);
            }
            let info = &links.material_info[m];
            let colour = match links
                .colour_image(m)
                .and_then(|img| files_by_image.get(&img))
            {
                Some(entries) => tex.add_file(tex_dir, &entries[0].file, Wrap::Repeat)?,
                None => {
                    report.materials_without_texture += 1;
                    NONE
                }
            };
            let (blend, alpha_ref) = match info.blend {
                BlendMode::Opaque => (Blend::Opaque, 0),
                BlendMode::AlphaTest(t) => (Blend::AlphaTest, t),
                BlendMode::Blend => (Blend::Blend, 0),
                BlendMode::Additive => (Blend::Additive, 0),
                BlendMode::Multiply => (Blend::Multiply, 0),
            };
            materials.push(MaterialRecord {
                recipe: Recipe::Lightmapped as u8,
                blend: blend as u8,
                alpha_ref,
                cull: match info.cull {
                    1 => Cull::Back,
                    2 => Cull::Front,
                    _ => Cull::None,
                } as u8,
                colour,
                sort: info.sort_key,
                // Layers drawn over coplanar geometry (blend, add, multiply: no depth write) get at least the smallest
                // depth pull, so they never lose the depth test to their own base surface (PC relies on identical
                // vertices giving identical depth; GX strips and the preview rotate triangles).
                flags: if matches!(blend, Blend::Opaque | Blend::AlphaTest) {
                    info.polygon_offset
                } else {
                    info.polygon_offset.max(1)
                } & records::material_flags::POLYGON_OFFSET,
            });
            let i = (materials.len() - 1) as u16;
            material_index.insert(m, i);
            Ok(i)
        };

    // Vertex groups: consecutive surfaces whose vertices span under 65,536.
    struct Draw {
        group_base: u32,
        blended: bool,
        indices: Vec<u16>,
        min: [f32; 3],
        max: [f32; 3],
    }
    let mut draws: BTreeMap<(u32, u16, i32, i32, u16, u16), Draw> = BTreeMap::new();
    let mut group_base = 0u32;
    let mut groups = 0usize;
    for (si, surf) in world.surfaces.iter().enumerate() {
        let Some(m) = surf.material else {
            report.surfaces_without_material += 1;
            continue;
        };
        if is_sky(m) {
            report.sky_surfaces_dropped += 1;
            continue;
        }
        let (lo, hi) = (
            surf.first_vertex,
            surf.first_vertex + u32::from(surf.vertex_count),
        );
        if groups == 0 || lo < group_base || hi - group_base > 65535 {
            group_base = lo;
            groups += 1;
        }
        let mat = material_for(m, &mut tex, &mut report)?;
        let lightmap = lightmap_tex
            .get(usize::from(surf.lightmap))
            .copied()
            .unwrap_or(NONE);
        // Cell of the surface's first vertex.
        let v0 = world
            .vertices
            .get(lo as usize)
            .ok_or("surface vertex out of range")?;
        // Batches split by IW4 visibility cell (`ALWAYS` when no cell lists the surface).
        let vis_cell = world
            .vis
            .surface_cell
            .get(si)
            .copied()
            .unwrap_or(wpk::vis::ALWAYS);
        // Within a cell, a 1024-unit grid keeps batches small enough to frustum-cull (IW4 cells are coarse: an open
        // map can be one cell).
        let cell = (
            vis_cell,
            (v0.pos[0] / CELL).floor() as i32,
            (v0.pos[1] / CELL).floor() as i32,
        );
        let d = draws
            .entry((group_base, cell.0, cell.1, cell.2, mat, lightmap))
            .or_insert(Draw {
                group_base,
                blended: matches!(links.material_info[m].blend, BlendMode::Blend),
                indices: Vec::new(),
                min: [f32::MAX; 3],
                max: [f32::MIN; 3],
            });
        for t in 0..usize::from(surf.tri_count) * 3 {
            let local = *world
                .indices
                .get(surf.base_index as usize + t)
                .ok_or("surface index out of range")?;
            let abs = surf.first_vertex + u32::from(local);
            let rel = abs
                .checked_sub(group_base)
                .filter(|&r| r < 65535)
                .ok_or("vertex outside its group")?;
            d.indices.push(rel as u16);
            let p = world.vertices[abs as usize].pos;
            for (k, &c) in p.iter().enumerate() {
                d.min[k] = d.min[k].min(c);
                d.max[k] = d.max[k].max(c);
            }
        }
    }
    report.vertex_groups = groups;

    let mut batches = Vec::with_capacity(draws.len());
    let mut dls = Vec::new();
    let mut batch_cells: Vec<u16> = Vec::with_capacity(draws.len());
    for (&(_, vis_cell, _, _, material, lightmap), d) in &draws {
        if d.indices.is_empty() {
            continue;
        }
        let dl = display_list(d.blended, &d.indices);
        if !wpk::strips::same_triangles(
            wpk::strips::dl_triangles(&dl).into_iter(),
            d.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]),
        ) {
            return Err("display list self-check: triangles changed".into());
        }
        let max_rel = *d.indices.iter().max().unwrap_or(&0) as u32;
        batch_cells.push(vis_cell);
        batches.push(BatchRecord {
            vertex_base: d.group_base,
            vertex_count: max_rel + 1,
            material,
            lightmap,
            dl_off: dls.len() as u32,
            dl_len: dl.len() as u32,
            tri_count: (d.indices.len() / 3) as u32,
            bounds_min: d.min,
            bounds_max: d.max,
        });
        report.world_triangles += (d.indices.len() / 3) as u64;
        report.world_dl_vertices += wpk::strips::dl_vertex_count(&dl) as u64;
        dls.extend_from_slice(&dl);
    }
    report.batches = batches.len();

    // Static models: model-space vertices and display lists per surface, placed by instance matrices.
    let sm = &world.models;
    let mut model_index: HashMap<usize, u16> = HashMap::new();
    let mut model_records: Vec<ModelRecord> = Vec::new();
    let mut model_surfaces: Vec<BatchRecord> = Vec::new();
    let mut model_vertices: Vec<WorldVertex> = Vec::new();
    let mut model_tris: HashMap<u16, u64> = HashMap::new();
    let mut r = StaticModelReport {
        unresolved: sm.unresolved,
        unreadable: sm.unreadable,
        unreadable_why: sm
            .unreadable_why
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect(),
        unlit: sm.unlit,
        ..StaticModelReport::default()
    };
    for (&m, lods) in &sm.meshes {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        let mut tris = 0u64;
        let mut rec_lods = [records::ModelLod::default(); 4];
        let mut lod_count = 0u8;
        for (li, lod) in lods.iter().take(4).enumerate() {
            let first_surface = model_surfaces.len() as u32;
            for surf in &lod.surfaces {
                let Some(mat) = surf.material else {
                    r.surfaces_without_material += 1;
                    continue;
                };
                if surf.indices.is_empty() {
                    continue;
                }
                let material = material_for(mat, &mut tex, &mut report)?;
                let (mut smin, mut smax) = ([f32::MAX; 3], [f32::MIN; 3]);
                for v in &surf.vertices {
                    for k in 0..3 {
                        smin[k] = smin[k].min(v.pos[k]);
                        smax[k] = smax[k].max(v.pos[k]);
                    }
                }
                let dl = display_list(
                    matches!(links.material_info[mat].blend, BlendMode::Blend),
                    &surf.indices,
                );
                if !wpk::strips::same_triangles(
                    wpk::strips::dl_triangles(&dl).into_iter(),
                    surf.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]),
                ) {
                    return Err("display list self-check: triangles changed".into());
                }
                r.dl_vertices += wpk::strips::dl_vertex_count(&dl) as u64;
                model_surfaces.push(BatchRecord {
                    vertex_base: model_vertices.len() as u32,
                    vertex_count: surf.vertices.len() as u32,
                    material,
                    lightmap: NONE,
                    dl_off: dls.len() as u32,
                    dl_len: dl.len() as u32,
                    tri_count: (surf.indices.len() / 3) as u32,
                    bounds_min: smin,
                    bounds_max: smax,
                });
                dls.extend_from_slice(&dl);
                model_vertices.extend_from_slice(&surf.vertices);
                if li == 0 {
                    tris += (surf.indices.len() / 3) as u64;
                    for k in 0..3 {
                        lo[k] = lo[k].min(smin[k]);
                        hi[k] = hi[k].max(smax[k]);
                    }
                }
            }
            let count = model_surfaces.len() as u32 - first_surface;
            if count == 0 && li == 0 {
                break;
            }
            rec_lods[li] = records::ModelLod {
                first_surface,
                surface_count: count as u16,
                dist: lod.dist,
            };
            lod_count = li as u8 + 1;
        }
        if lod_count == 0 {
            continue;
        }
        r.lods += usize::from(lod_count);
        let centre = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let radius = (0..3)
            .map(|k| (hi[k] - lo[k]) * 0.5)
            .map(|h| h * h)
            .sum::<f32>()
            .sqrt();
        let idx = model_records.len() as u16;
        model_records.push(ModelRecord {
            centre,
            radius,
            lod_count,
            lods: rec_lods,
        });
        model_index.insert(m, idx);
        model_tris.insert(idx, tris);
        r.triangles += tris;
    }
    let mut instances: Vec<InstanceRecord> = Vec::new();
    let mut instance_cells: Vec<u16> = Vec::new();
    for inst in &sm.instances {
        let Some(&model) = model_index.get(&inst.model) else {
            continue;
        };
        instance_cells.push(
            world
                .vis
                .smodel_cell
                .get(inst.index)
                .copied()
                .unwrap_or(wpk::vis::ALWAYS),
        );
        instances.push(InstanceRecord {
            model,
            cull_dist: inst.cull_dist,
            light: inst.light_rgba(),
            mtx: inst.matrix(),
        });
        r.placed_triangles += model_tris[&model];
    }
    r.instances = instances.len();
    r.models = model_records.len();
    r.surfaces = model_surfaces.len();
    r.vertices = model_vertices.len();
    report.static_models = r;

    // Skybox: the six faces converted from the sky material's cube image.
    let mut skybox = None;
    if let Some(entries) = sky_image.and_then(|img| files_by_image.get(&img)) {
        let mut faces = [NONE; 6];
        for e in entries {
            if let Some(i) = crate::pixel_decode::CUBE_FACES
                .iter()
                .position(|f| *f == e.face)
            {
                faces[i] = tex.add_file(tex_dir, &e.file, Wrap::Clamp)?;
            }
        }
        if faces.iter().all(|&f| f != NONE) {
            skybox = Some(Skybox { faces });
        }
    }
    report.skybox = skybox.is_some();

    let spawns = spawns(&parse_entities(&world.entity_string));
    report.spawns = spawns.len();
    report.textures = tex.records.len();
    report.materials = materials.len();
    for m in &materials {
        let level = m.flags & records::material_flags::POLYGON_OFFSET;
        if level > 0 {
            report.polygon_offset_materials[usize::from(level) - 1] += 1;
        }
    }

    let mut b = wpk::PackBuilder::new(map);
    b.section(
        wpk::tag::TEXTURES,
        tex.records.len() as u32,
        records::encode(&tex.records),
    );
    b.section(wpk::tag::TEXTURE_DATA, 0, tex.data);
    b.section(
        wpk::tag::MATERIALS,
        materials.len() as u32,
        records::encode(&materials),
    );
    b.section(
        wpk::tag::WORLD_VERTICES,
        world.vertices.len() as u32,
        records::encode(&world.vertices),
    );
    b.section(
        wpk::tag::WORLD_BATCHES,
        batches.len() as u32,
        records::encode(&batches),
    );
    b.section(wpk::tag::WORLD_DISPLAY_LISTS, 0, dls);
    b.section(
        wpk::tag::MODEL_VERTICES,
        model_vertices.len() as u32,
        records::encode(&model_vertices),
    );
    b.section(
        wpk::tag::MODEL_SURFACES,
        model_surfaces.len() as u32,
        records::encode(&model_surfaces),
    );
    b.section(
        wpk::tag::MODELS,
        model_records.len() as u32,
        records::encode(&model_records),
    );
    b.section(
        wpk::tag::MODEL_INSTANCES,
        instances.len() as u32,
        records::encode(&instances),
    );
    if let Some(sky) = skybox {
        b.section(wpk::tag::SKYBOX, 1, records::encode(&[sky]));
    }
    b.section(
        wpk::tag::SPAWNS,
        spawns.len() as u32,
        records::encode(&spawns),
    );
    let v = &world.vis;
    report.visibility = VisReport {
        cells: v.vis.cells.len(),
        portals: v.vis.portals.len(),
        batches_always: batch_cells
            .iter()
            .filter(|&&c| c == wpk::vis::ALWAYS)
            .count(),
        instances_always: instance_cells
            .iter()
            .filter(|&&c| c == wpk::vis::ALWAYS)
            .count(),
    };
    if !v.vis.cells.is_empty() {
        let u16s = |cells: &[u16]| {
            let mut o = Vec::with_capacity(cells.len() * 2);
            cells
                .iter()
                .for_each(|&c| o.extend_from_slice(&c.to_be_bytes()));
            o
        };
        b.section(wpk::tag::VIS, 1, v.vis.encode());
        b.section(
            wpk::tag::BATCH_CELLS,
            batch_cells.len() as u32,
            u16s(&batch_cells),
        );
        b.section(
            wpk::tag::INSTANCE_CELLS,
            instance_cells.len() as u32,
            u16s(&instance_cells),
        );
    }
    let clip = world.clip.as_ref().map_err(|e| format!("collision: {e}"))?;
    report.collision = crate::collision::ClipReport::of(clip);
    b.section(wpk::tag::CLIP, 1, clip.encode());
    let bytes = b.finish();
    let pack = wpk::Pack::parse(&bytes, true).map_err(|e| format!("pack self-check: {e:?}"))?;
    for t in [
        wpk::tag::TEXTURES,
        wpk::tag::TEXTURE_DATA,
        wpk::tag::MATERIALS,
        wpk::tag::WORLD_VERTICES,
        wpk::tag::WORLD_BATCHES,
        wpk::tag::WORLD_DISPLAY_LISTS,
        wpk::tag::SKYBOX,
        wpk::tag::SPAWNS,
        wpk::tag::CLIP,
        wpk::tag::MODEL_VERTICES,
        wpk::tag::MODEL_SURFACES,
        wpk::tag::MODELS,
        wpk::tag::MODEL_INSTANCES,
        wpk::tag::VIS,
        wpk::tag::BATCH_CELLS,
        wpk::tag::INSTANCE_CELLS,
    ] {
        if let Some(s) = pack.section(t) {
            report.sections.insert(
                String::from_utf8_lossy(&t).into_owned(),
                s.data.len() as u64,
            );
        }
    }
    report.pack_bytes = bytes.len() as u64;
    report.mem2_bytes = pack.in_place_len() as u64;
    report.content_hash = format!("{:016x}", pack.header.content_hash);
    Ok((bytes, report))
}

/// Strips for order-independent materials; alpha-blended ones keep their authored triangle order (it is their
/// layering order).
fn display_list(blended: bool, tris: &[u16]) -> Vec<u8> {
    if blended {
        records::world_display_list(tris)
    } else {
        wpk::strips::display_list(tris)
    }
}

/// MEM2 the pack may use on the Wii (~52 MiB usable, the rest for sound and other runtime data).
pub const MEM2_BUDGET: u64 = 44 << 20;

pub fn render_text(r: &PackReport) -> String {
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    let mut s = format!(
        "Modern Wiifare pack: {} ({:.2} MiB, hash {}; {:.2} MiB stays in MEM2{})\n",
        r.map,
        mib(r.pack_bytes),
        r.content_hash,
        mib(r.mem2_bytes),
        if r.mem2_bytes > MEM2_BUDGET {
            " - OVER the 44 MiB budget, may not load"
        } else {
            ""
        }
    );
    for (k, v) in &r.sections {
        s.push_str(&format!("  {k} {:>9.2} MiB\n", mib(*v)));
    }
    s.push_str(&format!(
        "textures {} (lightmaps {}), materials {} ({} without texture), world {} vertices, {} triangles ({} display-list vertices) in {} batches \
         ({} vertex groups), sky surfaces dropped {}, surfaces without material {}, skybox {}, spawns {}\n",
        r.textures,
        r.lightmaps,
        r.materials,
        r.materials_without_texture,
        r.world_vertices,
        r.world_triangles,
        r.world_dl_vertices,
        r.batches,
        r.vertex_groups,
        r.sky_surfaces_dropped,
        r.surfaces_without_material,
        r.skybox,
        r.spawns
    ));
    let m = &r.static_models;
    s.push_str(&format!(
        "static models: {} instances of {} models ({} surfaces, {} vertices, {} triangles; {} placed),          unresolved {}, unreadable {}, unlit {}, surfaces without material {}
",
        m.instances, m.models, m.surfaces, m.vertices, m.triangles, m.placed_triangles, m.unresolved,
        m.unreadable, m.unlit, m.surfaces_without_material
    ));
    let v = &r.visibility;
    s.push_str(&format!(
        "visibility: {} cells, {} portals; always drawn: {} batches, {} instances\n",
        v.cells, v.portals, v.batches_always, v.instances_always
    ));
    let c = &r.collision;
    s.push_str(&format!(
        "collision: {} brushes ({} planes), {} nodes, {} leaves, {} mesh triangles, {} brush models
",
        c.brushes, c.planes, c.nodes, c.leaves, c.mesh_triangles, c.brush_models
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_and_spawns() {
        let text = r#"{
"classname" "worldspawn"
}
{
"origin" "100 -200 32"
"angles" "0 90 0"
"classname" "mp_tdm_spawn_allies_start"
}
{
"origin" "5 6 7"
"classname" "mp_dm_spawn"
}
{
"origin" "1 2"
"classname" "mp_tdm_spawn"
}
"#;
        let ents = parse_entities(text);
        assert_eq!(ents.len(), 4);
        assert_eq!(ents[0]["classname"], "worldspawn");
        let s = spawns(&ents);
        assert_eq!(
            s.len(),
            2,
            "the entity with a 2-component origin is skipped"
        );
        assert_eq!(s[0].origin, [100.0, -200.0, 32.0]);
        assert_eq!(
            (s[0].yaw, s[0].kind, s[0].team, s[0].flags),
            (90.0, spawn_kind::TDM, spawn_team::ALLIES, 1)
        );
        assert_eq!(
            (s[1].kind, s[1].team, s[1].flags),
            (spawn_kind::DM, spawn_team::ANY, 0)
        );
    }

    #[test]
    fn lightmap_pages_are_averaged() {
        // 1x2 BGRA image: page0 pixel (B10,G20,R30), page1 pixel (B50,G60,R70).
        let bgra = [10u8, 20, 30, 255, 50, 60, 70, 255];
        let img = lightmap_from_secondary(1, 2, &bgra).unwrap();
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.pixels[0], [50, 40, 30, 255]);
        assert!(lightmap_from_secondary(2, 2, &bgra).is_none());
    }
}

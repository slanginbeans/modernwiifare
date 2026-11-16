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
pub(crate) const SORT_SKY: u8 = 2;
pub(crate) const SORT_SKYBOX: u8 = 3;

/// One GfxSurface.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Surface {
    pub(crate) material: Option<usize>,
    pub(crate) lightmap: u8,
    pub(crate) first_vertex: u32,
    pub(crate) vertex_count: u16,
    pub(crate) tri_count: u16,
    pub(crate) base_index: u32,
}

/// What the converter reads from the zone's GfxWorld and MapEnts.
pub struct WorldData {
    pub(crate) vertices: Vec<WorldVertex>,
    pub(crate) indices: Vec<u16>,
    pub(crate) surfaces: Vec<Surface>,
    /// Lightmap pages the world uses (surfaces index `0..lightmap_count`).
    pub(crate) lightmap_count: usize,
    pub(crate) entity_string: String,
    /// Collision, or why it could not be read.
    pub(crate) clip: Result<wpk::clip::Clip, String>,
    pub(crate) models: crate::static_models::StaticModels,
    vis: crate::visibility::WorldVis,
    /// The player characters, allies then axis (surfaces carry material indices until `build`), or why not.
    characters: Result<Vec<(wpk::view::ViewModel, Vec<String>)>, String>,
    /// The characters' per-bone hit boxes (`CHHB`), both teams.
    character_hitboxes: Vec<wpk::records::BoneHitRecord>,
    destructibles: crate::destructibles::Built,
}

/// What [`read_world`] builds the map's destructibles from: MW2's types (`destructibles::parse`) and the zone's
/// converted textures.
pub struct DestructibleInput<'a> {
    pub types: &'a HashMap<String, crate::destructibles::RawType>,
    pub index: &'a [crate::zone_textures::IndexEntry],
    pub tex_dir: &'a Path,
}

/// The LOD the characters are built from (their fourth, or the most detailed below it: a few thousand vertices, which
/// the Wii skins on the CPU for every player).
const CHARACTER_LOD: usize = 3;

/// Each team's character in the map zone: MW2's map zones load the allies' faction's models before the axis', each
/// faction's body followed by its heads. The allies' body is the first assault body (`mp_body_*assault*`); the axis'
/// the first assault body after it that shares none of its faction words; each team's head the first `head_` model
/// after its body.
fn team_models(links: &Links) -> Vec<(usize, Option<usize>)> {
    let names: Vec<&str> = links.models.iter().map(|m| m.name.as_str()).collect();
    let words = |n: &str| -> Vec<String> {
        n.trim_start_matches("mp_body_")
            .split('_')
            .filter(|w| !matches!(*w, "assault" | "a" | "b" | "c" | "d" | "e"))
            .map(str::to_owned)
            .collect()
    };
    let assault = |i: usize| names[i].starts_with("mp_body_") && names[i].contains("assault");
    let head_after = |b: usize| (b + 1..names.len()).find(|&i| names[i].starts_with("head_"));
    let Some(allies) = (0..names.len()).find(|&i| assault(i)) else {
        return Vec::new();
    };
    let aw = words(names[allies]);
    let axis = (allies + 1..names.len())
        .find(|&i| assault(i) && words(names[i]).iter().all(|w| !aw.contains(w)));
    let mut out = vec![(allies, head_after(allies))];
    if let Some(x) = axis {
        out.push((x, head_after(x)));
    }
    out
}

/// Read the world after a zone walk (the stream still holds its blocks).
pub fn read_world(
    s: &ZoneStream<'_>,
    links: &Links,
    dest: &DestructibleInput<'_>,
) -> Result<WorldData, String> {
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
    let characters = team_models(links)
        .into_iter()
        .map(|(body, head)| crate::viewmodel::build_character(s, links, body, head, CHARACTER_LOD))
        .collect();
    let character_hitboxes = team_models(links)
        .into_iter()
        .enumerate()
        .flat_map(|(t, (body, head))| {
            crate::viewmodel::character_hitboxes(s, links, body, head, t as u8)
        })
        .collect();
    let destructibles = crate::destructibles::build(
        s,
        links,
        &parse_entities(&entity_string),
        dest.types,
        dest.index,
        dest.tex_dir,
    );
    Ok(WorldData {
        vertices,
        indices,
        surfaces,
        lightmap_count,
        entity_string,
        clip,
        models,
        vis,
        characters,
        character_hitboxes,
        destructibles,
    })
}

/// Lightmap `i` as an RGBA8 image of `(page0 + page1) / 2`, from the zone image IW4 names `*lightmap<i>_secondary`.
/// (The GfxWorld lightmap array points at these images, which load earlier in the zone; IW4L records the pair only
/// when the image loads inline, so the converter finds them in its image list by IW4's fixed name.)
pub(crate) fn lightmap(images: &[ZoneImage], i: usize) -> Option<Image> {
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

/// The minimap's frame from the map's entities: MW2's `setupMiniMap` with the two `minimap_corner` entities and
/// worldspawn's `northyaw` (0 when absent); the visible range is the script's `compassmaxrange` or MW2's default.
pub fn minimap_frame(
    entities: &[BTreeMap<String, String>],
    max_range: Option<f32>,
    texture: u16,
) -> Option<records::Minimap> {
    let xy = |e: &BTreeMap<String, String>| -> Option<[f32; 2]> {
        let v: Vec<f32> = e
            .get("origin")?
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        (v.len() == 3).then(|| [v[0], v[1]])
    };
    let corners: Vec<[f32; 2]> = entities
        .iter()
        .filter(|e| e.get("targetname").map(String::as_str) == Some("minimap_corner"))
        .filter_map(xy)
        .collect();
    let [a, b] = corners[..] else {
        return None;
    };
    let north_yaw = entities
        .iter()
        .find(|e| e.get("classname").map(String::as_str) == Some("worldspawn"))
        .and_then(|w| w.get("northyaw"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0);
    let (northwest, southeast) =
        hud_iw4::setup_mini_map_frame(a, b, north_yaw, hud_iw4::REQUIRED_MAP_ASPECT_RATIO_DEFAULT);
    Some(records::Minimap {
        texture,
        northwest,
        southeast,
        north_yaw,
        max_range: max_range.unwrap_or(hud_iw4::COMPASS_MAX_RANGE_DEFAULT_MP),
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

/// Game-mode objectives from parsed entities: Domination flags (`dom.gsc` `domFlags`: the `flag_primary` and
/// `flag_secondary` triggers, labelled by `script_label` `_a`, `_b` ...; a trigger radius's `radius` / `height`).
pub fn objectives(entities: &[BTreeMap<String, String>]) -> Vec<wpk::records::ObjectiveRecord> {
    let num = |s: &str| -> Vec<f32> {
        s.split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect()
    };
    let mut out = Vec::new();
    for e in entities {
        let t = e.get("targetname").map(String::as_str);
        if !matches!(t, Some("flag_primary" | "flag_secondary")) {
            continue;
        }
        let origin = e.get("origin").map(|o| num(o)).unwrap_or_default();
        if origin.len() != 3 {
            continue;
        }
        let label = e
            .get("script_label")
            .and_then(|l| l.trim_start_matches('_').bytes().next())
            .map_or(0, |b| b.to_ascii_lowercase().saturating_sub(b'a'));
        let get = |k: &str| e.get(k).and_then(|v| v.trim().parse::<f32>().ok());
        let (Some(radius), Some(height)) = (get("radius"), get("height")) else {
            continue;
        };
        out.push(wpk::records::ObjectiveRecord {
            kind: wpk::records::objective_kind::DOM_FLAG,
            label,
            origin: [origin[0], origin[1], origin[2]],
            radius,
            height,
        });
    }
    out.sort_by_key(|o| o.label);
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
    /// Vertices of each team's player character.
    pub character_vertices: Vec<usize>,
    /// World triangles before simplification (`world_triangles` is what the pack draws).
    pub world_triangles_source: u64,
    pub textures: usize,
    pub lightmaps: usize,
    pub materials: usize,
    /// Materials with an IW4 polygon offset (decals), by level 1..3.
    pub polygon_offset_materials: [usize; 3],
    pub world_vertices: usize,
    /// World vertices merged into an identical one of their batch (welding: longer GX strips).
    pub world_vertices_welded: usize,
    pub world_triangles: u64,
    /// Vertices the world's display lists send (strips: about one per triangle; lists: three).
    pub world_dl_vertices: u64,
    /// Batches stored as fixed-point vertices (the rest stay float).
    pub world_compact_batches: usize,
    /// World batches and model surfaces made fixed-point by splitting into UV tiles (`--tiled-uv`).
    pub tiled_batches: usize,
    /// With `--tiled-uv`, batches still in floats, by why: a vertex beyond the origin's range, one triangle's UV
    /// span, or too many vertices after splitting.
    pub float_position: usize,
    pub float_triangle_uv: usize,
    pub float_vertices: usize,
    /// Display lists with u8 indices / without a colour index (`--small-dl`; world batches and model surfaces).
    pub dl_idx8_batches: usize,
    pub dl_no_color_batches: usize,
    pub batches: usize,
    pub vertex_groups: usize,
    pub sky_surfaces_dropped: usize,
    pub surfaces_without_material: usize,
    pub materials_without_texture: usize,
    pub skybox: bool,
    pub minimap: bool,
    pub spawns: usize,
    /// Destructible objects placed, their types, models and effects, and those left out.
    pub destructibles: usize,
    pub destructible_types: usize,
    pub destructible_models: usize,
    pub destructible_effects: usize,
    pub destructibles_skipped: usize,
    /// Bot walk graph: floor points and moves.
    pub nav_nodes: usize,
    pub nav_edges: usize,
    pub collision: crate::collision::ClipReport,
    /// `CLIP` by table, and what lossless repacking could save (`clip_report`; measuring only).
    pub collision_breakdown: crate::clip_report::ClipBreakdown,
    pub static_models: StaticModelReport,
    pub visibility: VisReport,
    /// Where `TXDA` goes: by use, format and size (`tex_report`).
    pub texture_breakdown: crate::tex_report::Breakdown,
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
    /// Vertices merged into an identical one of their surface.
    pub vertices_welded: usize,
    /// Surfaces stored as fixed-point vertices.
    pub compact_surfaces: usize,
    /// Triangles if every instance were drawn (what world-space baking would cost).
    pub placed_triangles: u64,
    pub unresolved: usize,
    pub unreadable: usize,
    pub unreadable_why: BTreeMap<String, usize>,
    pub unlit: usize,
    pub surfaces_without_material: usize,
    /// `--small-props`: models packed from their LOD 1, and the LOD 0 vertices / triangles left out.
    pub small_props: usize,
    pub small_prop_vertices_dropped: usize,
    pub small_prop_triangles_dropped: u64,
}

/// Texture data accumulated into `TXDA`.
#[derive(Default)]
pub(crate) struct Textures {
    pub(crate) records: Vec<TextureRecord>,
    pub(crate) data: Vec<u8>,
    /// `.gxt` file name -> texture index.
    by_file: HashMap<String, u16>,
    /// Keep only level 0 of each file (`--slim-views`: the first-person gun sits at the camera, so its smaller mips
    /// go unused; GX then samples it without mipmapping).
    pub(crate) base_level_only: bool,
    /// What the next textures added are for (the pack report's texture breakdown; "" = other).
    pub(crate) role: &'static str,
    /// The use of each record (`role` when it was added).
    pub(crate) roles: Vec<&'static str>,
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
        self.roles.push(self.role);
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

    /// A plain white texture (8 × 8 I8, made here: MW2's `white` material has no converted image), once.
    /// A texture converted here (not from a `.gxt` file).
    pub(crate) fn add_converted(&mut self, c: &gx_texture::Converted, wrap: Wrap) -> u16 {
        self.add(c, wrap)
    }

    pub(crate) fn add_white(&mut self) -> u16 {
        const KEY: &str = "<white>";
        if let Some(&i) = self.by_file.get(KEY) {
            return i;
        }
        let c = gx_texture::Converted {
            format: gx_texture::TexFormat::I8,
            width: 8,
            height: 8,
            levels: 1,
            traits: gx_texture::policy::Traits {
                grey: true,
                alpha: gx_texture::policy::Alpha::Opaque,
            },
            data: vec![0xff; 64],
            alpha_plane: None,
        };
        let i = self.add(&c, Wrap::Clamp);
        self.by_file.insert(KEY.to_owned(), i);
        i
    }

    /// [`Textures::add_file`] with its top mip levels left out until it is at most `max` texels across (as far as
    /// its mips go).
    pub(crate) fn add_file_capped(
        &mut self,
        dir: &Path,
        file: &str,
        wrap: Wrap,
        max: u32,
    ) -> Result<u16, String> {
        let key = format!("{file}@{max}");
        if let Some(&i) = self.by_file.get(&key) {
            return Ok(i);
        }
        let bytes = std::fs::read(dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        let (h, data) =
            gx_texture::container::parse(&bytes).map_err(|e| format!("{file}: {e:?}"))?;
        let plane = gx_texture::container::parse_alpha_plane(&bytes)
            .map_err(|e| format!("{file}: {e:?}"))?;
        let (w, ht, levels) = (u32::from(h.width), u32::from(h.height), u32::from(h.levels));
        // Without mips to drop (UI images), scale the top level down instead.
        if levels == 1 && (w > max || ht > max) {
            let s = (max as f32 / w.max(ht) as f32).min(1.0);
            let (nw, nh) = (
                ((w as f32 * s) as u32 / 8 * 8).max(8),
                ((ht as f32 * s) as u32 / 8 * 8).max(8),
            );
            // Full 32-bit colour goes to RGB5A3 on the way down (half the bytes, alpha kept).
            let format = if h.format == gx_texture::TexFormat::Rgba8 {
                gx_texture::TexFormat::Rgb5a3
            } else {
                h.format
            };
            let shrink = |fmt: gx_texture::TexFormat, to: gx_texture::TexFormat, d: &[u8]| {
                let img = gx_texture::decode(d, w, ht, fmt)?;
                Some(gx_texture::encode(
                    &gx_texture::image::resize(&img, nw, nh),
                    to,
                ))
            };
            let data =
                shrink(h.format, format, data).ok_or_else(|| format!("{file}: undecodable"))?;
            let alpha_plane = match plane {
                Some((ph, p)) => Some(
                    shrink(ph.format, ph.format, p)
                        .ok_or_else(|| format!("{file}: undecodable"))?,
                ),
                None => None,
            };
            let c = gx_texture::Converted {
                format,
                width: nw,
                height: nh,
                levels: 1,
                traits: gx_texture::policy::Traits {
                    grey: false,
                    alpha: gx_texture::policy::Alpha::Opaque,
                },
                data,
                alpha_plane,
            };
            let i = self.add(&c, wrap);
            self.by_file.insert(key, i);
            return Ok(i);
        }
        let mut k = 0;
        while k + 1 < levels && ((w >> k) > max || (ht >> k) > max) {
            k += 1;
        }
        let cut = |fmt: gx_texture::TexFormat, d: &[u8]| -> Vec<u8> {
            let skip = fmt.chain_size(w, ht, k);
            let keep = fmt.chain_size((w >> k).max(1), (ht >> k).max(1), levels - k);
            d.get(skip..skip + keep).unwrap_or(d).to_vec()
        };
        let c = gx_texture::Converted {
            format: h.format,
            width: (w >> k).max(1),
            height: (ht >> k).max(1),
            levels: levels - k,
            traits: gx_texture::policy::Traits {
                grey: false,
                alpha: gx_texture::policy::Alpha::Opaque,
            },
            data: cut(h.format, data),
            alpha_plane: plane.map(|(ph, p)| cut(ph.format, p)),
        };
        let i = self.add(&c, wrap);
        self.by_file.insert(key, i);
        Ok(i)
    }

    /// Add a converted `.gxt` from the texture folder (once per file).
    pub(crate) fn add_file(&mut self, dir: &Path, file: &str, wrap: Wrap) -> Result<u16, String> {
        if let Some(&i) = self.by_file.get(file) {
            return Ok(i);
        }
        let bytes = std::fs::read(dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        let (h, data) =
            gx_texture::container::parse(&bytes).map_err(|e| format!("{file}: {e:?}"))?;
        let plane = gx_texture::container::parse_alpha_plane(&bytes)
            .map_err(|e| format!("{file}: {e:?}"))?;
        let (w, ht) = (u32::from(h.width), u32::from(h.height));
        let base_only = self.base_level_only && h.levels > 1;
        let data = if base_only {
            &data[..h.format.level_size(w, ht).min(data.len())]
        } else {
            data
        };
        let plane = plane.map(|(a, p)| {
            if base_only {
                &p[..a.format.level_size(w, ht).min(p.len())]
            } else {
                p
            }
        });
        let c = gx_texture::Converted {
            format: h.format,
            width: w,
            height: ht,
            levels: if base_only { 1 } else { u32::from(h.levels) },
            traits: gx_texture::policy::Traits {
                grey: false,
                alpha: gx_texture::policy::Alpha::Opaque,
            },
            data: data.to_vec(),
            alpha_plane: plane.map(<[u8]>::to_vec),
        };
        let i = self.add(&c, wrap);
        self.by_file.insert(file.to_owned(), i);
        Ok(i)
    }
}

/// Pack options that change the output (each folded into the pack stamp by the caller).
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Batches whose UVs span too far for fixed-point vertices are split into UV tiles ([`wpk::quant::quantize_tiled`]).
    pub tiled_uv: bool,
    /// u8 display-list indices for batches of at most 256 vertices, no colour index where every vertex is white
    /// ([`wpk::quant::dl_flags`]).
    pub small_dl: bool,
    /// Small props start at their second level of detail ([`first_lod`]).
    pub small_props: bool,
}

/// With `--small-props`, models at most this big (bounding radius in units, at their largest placement scale) leave
/// out their most detailed level: about 80 cm across, so bottles, cans, debris, small boxes and fittings.
pub const SMALL_PROP_RADIUS: f32 = 32.0;

fn lod_triangles(l: &crate::static_models::ModelLod) -> usize {
    l.surfaces
        .iter()
        .filter(|s| s.material.is_some())
        .map(|s| s.indices.len() / 3)
        .sum()
}

/// The first level of detail to pack for a model: 1 when `small_props` is on, the model is small (its LOD 0 bounds,
/// times `max_scale`, within [`SMALL_PROP_RADIUS`]) and its LOD 1 has triangles, fewer than LOD 0. The packed LODs
/// keep IW4's switch distances, so LOD 1 then draws from the camera out to its own distance; nothing changes on the
/// Wii. Otherwise 0.
pub(crate) fn first_lod(
    lods: &[crate::static_models::ModelLod],
    max_scale: f32,
    small_props: bool,
) -> usize {
    if !small_props || lods.len() < 2 {
        return 0;
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in lods[0].surfaces.iter().flat_map(|s| &s.vertices) {
        for k in 0..3 {
            lo[k] = lo[k].min(v.pos[k]);
            hi[k] = hi[k].max(v.pos[k]);
        }
    }
    if lo[0] > hi[0] {
        return 0;
    }
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) * 0.5)
        .map(|h| h * h)
        .sum::<f32>()
        .sqrt()
        * max_scale.max(1.0);
    let (t0, t1) = (lod_triangles(&lods[0]), lod_triangles(&lods[1]));
    usize::from(radius <= SMALL_PROP_RADIUS && t1 > 0 && t1 < t0)
}

/// Build the pack bytes for one map. `index` / `tex_dir`: the textures `convert_zone` wrote for this zone.
#[allow(clippy::too_many_arguments)]
pub fn build(
    map: &str,
    world: &WorldData,
    links: &Links,
    images: &[ZoneImage],
    index: &[IndexEntry],
    tex_dir: &Path,
    stamp: &crate::stamp::Stamp,
    quality: &crate::simplify::WorldQuality,
    opts: &Options,
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
    tex.role = "lightmap";
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
                Some(entries) => {
                    tex.role = entries[0].role;
                    tex.add_file(tex_dir, &entries[0].file, Wrap::Repeat)?
                }
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

    // Where batches meet: how many batches use each vertex position (simplification keeps those vertices).
    let mut batches_at: HashMap<[u32; 3], (usize, u32)> = HashMap::new();
    if quality.ratio < 1.0 {
        for (k, d) in draws.values().enumerate().filter(|(_, d)| !d.blended) {
            for &rel in &d.indices {
                let p = world.vertices[(d.group_base + u32::from(rel)) as usize].pos;
                let e = batches_at
                    .entry(crate::simplify::position_key(p))
                    .or_insert((k, 0));
                if e.1 == 0 || e.0 != k {
                    e.1 += 1;
                    e.0 = k;
                }
            }
        }
    }
    let mut batches = Vec::with_capacity(draws.len());
    let mut dls = Vec::new();
    let mut batch_cells: Vec<u16> = Vec::with_capacity(draws.len());
    let mut batch_quant: Vec<wpk::quant::QuantRecord> = Vec::with_capacity(draws.len());
    let mut world_floats: Vec<WorldVertex> = Vec::new();
    let mut world_compact: Vec<wpk::quant::CompactVertex> = Vec::new();
    for (&(_, vis_cell, _, _, material, lightmap), d) in &draws {
        if d.indices.is_empty() {
            continue;
        }
        // Each batch gets its own vertices (only those it uses), renumbered from 0.
        let mut local = vec![u16::MAX; usize::from(*d.indices.iter().max().unwrap_or(&0)) + 1];
        let mut verts: Vec<WorldVertex> = Vec::new();
        let mut tris: Vec<u16> = Vec::with_capacity(d.indices.len());
        for &rel in &d.indices {
            let slot = &mut local[usize::from(rel)];
            if *slot == u16::MAX {
                *slot = verts.len() as u16;
                verts.push(world.vertices[(d.group_base + u32::from(rel)) as usize]);
            }
            tris.push(*slot);
        }
        // Vertices identical in every attribute become one, so strips can run across the seams IW4 leaves.
        report.world_vertices_welded += weld(&mut verts, &mut tris);
        // Simplified for the Wii's triangle budget (opaque batches; blended ones keep their authored order), then
        // only the vertices still used.
        report.world_triangles_source += (tris.len() / 3) as u64;
        if !d.blended && quality.ratio < 1.0 {
            let shared = |v: &WorldVertex| {
                batches_at
                    .get(&crate::simplify::position_key(v.pos))
                    .is_some_and(|e| e.1 > 1)
            };
            let simplified = crate::simplify::batch(&verts, &tris, quality, shared);
            let mut keep = vec![u16::MAX; verts.len()];
            let mut used: Vec<WorldVertex> = Vec::new();
            tris = simplified
                .iter()
                .map(|&i| {
                    let k = &mut keep[usize::from(i)];
                    if *k == u16::MAX {
                        *k = used.len() as u16;
                        used.push(verts[usize::from(i)]);
                    }
                    *k
                })
                .collect();
            verts = used;
        }
        let centre = std::array::from_fn(|k| (d.min[k] + d.max[k]) * 0.5);
        let (vertex_base, quant, vertex_count) = store_vertices(
            &verts,
            &mut tris,
            wpk::quant::snap_origin(centre),
            opts,
            &mut world_floats,
            &mut world_compact,
            &mut report,
        );
        if quant.is_compact() {
            report.world_compact_batches += 1;
        }
        let layout = wpk::strips::DlLayout::from_flags(quant.flags);
        let dl = display_list(d.blended, &tris, quant.vtxfmt, layout);
        if !wpk::strips::same_triangles(
            wpk::strips::dl_triangles_layout(&dl, layout).into_iter(),
            tris.chunks_exact(3).map(|t| [t[0], t[1], t[2]]),
        ) {
            return Err("display list self-check: triangles changed".into());
        }
        batch_cells.push(vis_cell);
        batch_quant.push(quant);
        batches.push(BatchRecord {
            vertex_base,
            vertex_count,
            material,
            lightmap,
            dl_off: dls.len() as u32,
            dl_len: dl.len() as u32,
            tri_count: (tris.len() / 3) as u32,
            bounds_min: d.min,
            bounds_max: d.max,
        });
        report.world_triangles += (tris.len() / 3) as u64;
        report.world_dl_vertices += wpk::strips::dl_vertex_count_layout(&dl, layout) as u64;
        dls.extend_from_slice(&dl);
    }
    report.batches = batches.len();

    // Static models: model-space vertices and display lists per surface, placed by instance matrices.
    let sm = &world.models;
    let mut model_index: HashMap<usize, u16> = HashMap::new();
    let mut model_records: Vec<ModelRecord> = Vec::new();
    let mut model_surfaces: Vec<BatchRecord> = Vec::new();
    let mut model_vertices: Vec<WorldVertex> = Vec::new();
    let mut model_compact: Vec<wpk::quant::CompactVertex> = Vec::new();
    let mut surface_quant: Vec<wpk::quant::QuantRecord> = Vec::new();
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
    // Each model's largest placement scale (a small model placed big is not small).
    let mut max_scale: HashMap<usize, f32> = HashMap::new();
    for inst in &sm.instances {
        let e = max_scale.entry(inst.model).or_insert(0.0);
        *e = e.max(inst.scale);
    }
    for (&m, all_lods) in &sm.meshes {
        let skip = first_lod(
            all_lods,
            max_scale.get(&m).copied().unwrap_or(1.0),
            opts.small_props,
        );
        if skip > 0 {
            r.small_props += 1;
            r.small_prop_vertices_dropped += all_lods[0]
                .surfaces
                .iter()
                .map(|s| s.vertices.len())
                .sum::<usize>();
            r.small_prop_triangles_dropped += lod_triangles(&all_lods[0]) as u64;
        }
        let lods = &all_lods[skip..];
        // One origin for every surface of every LOD of the model (no cracks between its surfaces).
        let (mut blo, mut bhi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in lods
            .iter()
            .take(1)
            .flat_map(|l| &l.surfaces)
            .flat_map(|s| &s.vertices)
        {
            for k in 0..3 {
                blo[k] = blo[k].min(v.pos[k]);
                bhi[k] = bhi[k].max(v.pos[k]);
            }
        }
        let model_origin = wpk::quant::snap_origin(std::array::from_fn(|k| {
            if blo[k] <= bhi[k] {
                (blo[k] + bhi[k]) * 0.5
            } else {
                0.0
            }
        }));
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
                let mut verts = surf.vertices.clone();
                let mut indices = surf.indices.clone();
                r.vertices_welded += weld(&mut verts, &mut indices);
                let (vertex_base, quant, vertex_count) = store_vertices(
                    &verts,
                    &mut indices,
                    model_origin,
                    opts,
                    &mut model_vertices,
                    &mut model_compact,
                    &mut report,
                );
                if quant.is_compact() {
                    r.compact_surfaces += 1;
                }
                let layout = wpk::strips::DlLayout::from_flags(quant.flags);
                let dl = display_list(
                    matches!(links.material_info[mat].blend, BlendMode::Blend),
                    &indices,
                    quant.vtxfmt,
                    layout,
                );
                if !wpk::strips::same_triangles(
                    wpk::strips::dl_triangles_layout(&dl, layout).into_iter(),
                    indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]),
                ) {
                    return Err("display list self-check: triangles changed".into());
                }
                r.dl_vertices += wpk::strips::dl_vertex_count_layout(&dl, layout) as u64;
                surface_quant.push(quant);
                model_surfaces.push(BatchRecord {
                    vertex_base,
                    vertex_count,
                    material,
                    lightmap: NONE,
                    dl_off: dls.len() as u32,
                    dl_len: dl.len() as u32,
                    tri_count: (surf.indices.len() / 3) as u32,
                    bounds_min: smin,
                    bounds_max: smax,
                });
                dls.extend_from_slice(&dl);
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
        tex.role = "sky";
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

    let entities = parse_entities(&world.entity_string);
    let spawns = spawns(&entities);
    report.spawns = spawns.len();
    // The HUD minimap: the material the map script passes to `setupMiniMap`, framed by its two `minimap_corner`
    // entities and the map's north (MW2 `_compass.gsc`).
    let minimap = links
        .material_info
        .iter()
        .position(|m| {
            m.name
                .as_deref()
                .is_some_and(|n| links.minimap_materials.contains(&n.to_ascii_lowercase()))
        })
        .and_then(|m| links.colour_image(m))
        .and_then(|img| files_by_image.get(&img))
        .and_then(|entries| entries.first())
        .map(|e| {
            tex.role = "minimap";
            tex.add_file(tex_dir, &e.file, Wrap::Clamp)
        })
        .transpose()?
        .and_then(|texture| minimap_frame(&entities, links.compass_max_range, texture));
    report.minimap = minimap.is_some();
    report.textures = tex.records.len();
    report.materials = materials.len();
    for m in &materials {
        let level = m.flags & records::material_flags::POLYGON_OFFSET;
        if level > 0 {
            report.polygon_offset_materials[usize::from(level) - 1] += 1;
        }
    }

    // Player characters: each surface's material to a texture of this pack.
    let mut character_teams = Vec::new();
    match &world.characters {
        Ok(chars) if !chars.is_empty() => {
            let mut teams = Vec::new();
            for (vm, names) in chars {
                let mut vm = vm.clone();
                for surf in &mut vm.surfaces {
                    let mat = surf.texture;
                    surf.texture = records::NONE;
                    if mat == records::NONE {
                        continue;
                    }
                    if let Some(e) = links
                        .colour_image(usize::from(mat))
                        .and_then(|img| files_by_image.get(&img))
                        .and_then(|v| v.first())
                    {
                        tex.role = "character";
                        surf.texture = tex.add_file(tex_dir, &e.file, records::Wrap::Repeat)?;
                    }
                }
                report.character_vertices.push(vm.verts.len());
                teams.push((vm, names.clone()));
            }
            println!(
                "player characters: {} teams, vertices {:?}",
                teams.len(),
                report.character_vertices
            );
            character_teams = teams;
        }
        Ok(_) => {}
        Err(e) => eprintln!("warning: no player characters: {e}"),
    }
    report.texture_breakdown = crate::tex_report::breakdown(&tex.records, &tex.roles);
    let mut b = wpk::PackBuilder::new(map);
    b.section(wpk::tag::STAMP, 0, stamp.encode());
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
        world_floats.len() as u32,
        records::encode(&world_floats),
    );
    b.section(
        wpk::tag::WORLD_VERTICES_Q,
        world_compact.len() as u32,
        records::encode(&world_compact),
    );
    b.section(
        wpk::tag::BATCH_QUANT,
        batch_quant.len() as u32,
        records::encode(&batch_quant),
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
        wpk::tag::MODEL_VERTICES_Q,
        model_compact.len() as u32,
        records::encode(&model_compact),
    );
    b.section(
        wpk::tag::SURFACE_QUANT,
        surface_quant.len() as u32,
        records::encode(&surface_quant),
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
    if let Some(m) = minimap {
        b.section(wpk::tag::MINIMAP, 1, records::encode(&[m]));
    }
    if let Some(f) = links.fog {
        b.section(wpk::tag::FOG, 1, records::encode(&[f]));
    }
    b.section(
        wpk::tag::SPAWNS,
        spawns.len() as u32,
        records::encode(&spawns),
    );
    if !character_teams.is_empty() {
        b.section(
            wpk::tag::CHARACTERS,
            character_teams.len() as u32,
            wpk::view::encode_characters(&character_teams),
        );
        if !world.character_hitboxes.is_empty() {
            println!("character hit boxes: {}", world.character_hitboxes.len());
            b.section(
                wpk::tag::CHARACTER_HITBOXES,
                world.character_hitboxes.len() as u32,
                records::encode(&world.character_hitboxes),
            );
        }
    }
    let objs = objectives(&entities);
    b.section(
        wpk::tag::OBJECTIVES,
        objs.len() as u32,
        records::encode(&objs),
    );
    // Destructible objects: models, textures, collision, types, the placed ones, and their effects (read in place,
    // so before the sections the Wii decodes at load: it keeps only what comes before those).
    let d = &world.destructibles;
    report.destructibles = d.instances.len();
    report.destructible_types = d.types.len();
    report.destructible_models = d.models.len();
    report.destructibles_skipped = d.skipped;
    report.destructible_effects = d.fx.len();
    if !d.instances.is_empty() {
        b.section(
            wpk::tag::DEST_MODELS,
            d.models.len() as u32,
            wpk::held::encode(&d.models),
        );
        b.section(
            wpk::tag::DEST_TEXTURES,
            d.tex.records.len() as u32,
            records::encode(&d.tex.records),
        );
        b.section(wpk::tag::DEST_TEXTURE_DATA, 0, d.tex.data.clone());
        b.section(
            wpk::tag::DEST_BOXES,
            d.boxes.len() as u32,
            wpk::destructible::encode_boxes(&d.boxes),
        );
        b.section(
            wpk::tag::DEST_TYPES,
            d.types.len() as u32,
            wpk::destructible::encode_types(&d.types),
        );
        b.section(
            wpk::tag::DEST_INSTANCES,
            d.instances.len() as u32,
            wpk::destructible::encode_instances(&d.instances),
        );
        b.section(wpk::tag::DEST_FX, d.fx.len() as u32, d.fx.encode());
        b.section(
            wpk::tag::DEST_FX_TEXTURES,
            d.fx_tex.records.len() as u32,
            records::encode(&d.fx_tex.records),
        );
        b.section(wpk::tag::DEST_FX_TEXTURE_DATA, 0, d.fx_tex.data.clone());
    }
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
    let clip_bytes = clip.encode();
    report.collision_breakdown = crate::clip_report::breakdown(clip, &clip_bytes);
    b.section(wpk::tag::CLIP, 1, clip_bytes);
    // The bots' walk graph, flooded out from the spawn points over the collision.
    let seeds: Vec<[f32; 3]> = spawns.iter().map(|s| s.origin).collect();
    let nav = game_core::nav::bake(&game_core::collision::MapClip::new(clip), &seeds);
    report.nav_nodes = nav.nodes.len();
    report.nav_edges = nav.edges.len();
    b.section(wpk::tag::NAV, nav.nodes.len() as u32, nav.encode());
    let bytes = b.finish();
    let pack = wpk::Pack::parse(&bytes, true).map_err(|e| format!("pack self-check: {e:?}"))?;
    // The Wii keeps only the part before the first decoded-at-load section in memory: everything read in place
    // must lie there.
    let in_place = pack.in_place_len();
    for (tag, _) in pack.section_sizes() {
        let at = pack.section_offset(tag).unwrap_or(0);
        if at >= in_place && !wpk::decoded_at_load().contains(&tag) {
            return Err(format!(
                "pack self-check: section {} lies past the in-place part",
                String::from_utf8_lossy(&tag)
            ));
        }
    }
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
        wpk::tag::WORLD_VERTICES_Q,
        wpk::tag::BATCH_QUANT,
        wpk::tag::MODEL_VERTICES_Q,
        wpk::tag::SURFACE_QUANT,
        wpk::tag::VIS,
        wpk::tag::BATCH_CELLS,
        wpk::tag::INSTANCE_CELLS,
        wpk::tag::NAV,
        wpk::tag::DEST_MODELS,
        wpk::tag::DEST_TEXTURES,
        wpk::tag::DEST_TEXTURE_DATA,
        wpk::tag::DEST_BOXES,
        wpk::tag::DEST_TYPES,
        wpk::tag::DEST_INSTANCES,
        wpk::tag::DEST_FX,
        wpk::tag::DEST_FX_TEXTURES,
        wpk::tag::DEST_FX_TEXTURE_DATA,
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

/// Merge vertices identical in every attribute (exact bits) and drop unused ones, keeping first-use order; returns
/// how many fewer vertices there are. The triangles are the same, so what is drawn does not change.
pub(crate) fn weld(verts: &mut Vec<WorldVertex>, tris: &mut [u16]) -> usize {
    let key = |v: &WorldVertex| {
        (
            v.pos.map(f32::to_bits),
            v.rgba,
            v.uv.map(f32::to_bits),
            v.lm,
        )
    };
    let mut seen: HashMap<_, u16> = HashMap::with_capacity(verts.len());
    let mut out: Vec<WorldVertex> = Vec::with_capacity(verts.len());
    let mut remap = vec![u16::MAX; verts.len()];
    for t in tris.iter_mut() {
        let i = usize::from(*t);
        if remap[i] == u16::MAX {
            let v = verts[i];
            remap[i] = *seen.entry(key(&v)).or_insert_with(|| {
                out.push(v);
                (out.len() - 1) as u16
            });
        }
        *t = remap[i];
    }
    let fewer = verts.len() - out.len();
    *verts = out;
    fewer
}

/// Strips for order-independent materials; alpha-blended ones keep their authored triangle order (it is their
/// layering order).
/// Opaque ones get the shorter of two strippings: ours (exact triangles, no turns) and meshoptimizer's (after its
/// vertex-cache order; it turns with repeated-vertex triangles, which cost a vertex each but keep strips going).
fn display_list(blended: bool, tris: &[u16], vtxfmt: u8, layout: wpk::strips::DlLayout) -> Vec<u8> {
    if blended {
        return records::world_display_list_layout(tris, vtxfmt, layout);
    }
    let ours =
        wpk::strips::strips_display_list_layout(&wpk::strips::stripify(tris), vtxfmt, layout);
    match meshopt_strips(tris) {
        Some(st) => {
            let theirs = wpk::strips::strips_display_list_layout(&st, vtxfmt, layout);
            if wpk::strips::dl_vertex_count_layout(&theirs, layout)
                < wpk::strips::dl_vertex_count_layout(&ours, layout)
            {
                theirs
            } else {
                ours
            }
        }
        None => ours,
    }
}

/// `tris` as meshoptimizer strips (cut at its restart index; a 3-vertex piece is a lone triangle).
fn meshopt_strips(tris: &[u16]) -> Option<wpk::strips::Strips> {
    let indices: Vec<u32> = tris.iter().map(|&i| u32::from(i)).collect();
    let vertex_count = usize::from(*tris.iter().max()?) + 1;
    let ordered = meshopt::optimize_vertex_cache(&indices, vertex_count);
    let strip = meshopt::stripify(&ordered, vertex_count, u32::MAX).ok()?;
    let mut st = wpk::strips::Strips::default();
    for piece in strip.split(|&i| i == u32::MAX) {
        let piece: Vec<u16> = piece.iter().map(|&i| i as u16).collect();
        match piece.len() {
            0..=2 => {}
            3 => st.singles.push([piece[0], piece[1], piece[2]]),
            _ => st.strips.push(piece),
        }
    }
    Some(st)
}

/// A batch's own vertices, fixed-point when they fit around `origin` (see `wpk::quant`), else floats. Appends them to
/// the matching array and returns (vertex base, how they are stored, vertex count). With `opts.tiled_uv`, a batch
/// whose UVs span too far is split into UV tiles: `tris` is then re-indexed to the (partly duplicated) vertices.
/// With `opts.small_dl`, the record's display-list flags say whether its list can use u8 indices / no colour index.
#[allow(clippy::too_many_arguments)]
fn store_vertices(
    verts: &[WorldVertex],
    tris: &mut Vec<u16>,
    origin: [f32; 3],
    opts: &Options,
    floats: &mut Vec<WorldVertex>,
    compact: &mut Vec<wpk::quant::CompactVertex>,
    report: &mut PackReport,
) -> (u32, wpk::quant::QuantRecord, u32) {
    use wpk::quant::{FloatReason, dl_flags};
    let no_quant = std::env::var_os("MWF_NO_QUANT").is_some();
    let mut quantized = if no_quant {
        None
    } else {
        wpk::quant::quantize(verts, tris, origin, false).map(|(f, q)| (f, q, None))
    };
    if quantized.is_none() && !no_quant && opts.tiled_uv {
        match wpk::quant::quantize_tiled(verts, tris, origin) {
            Ok((f, q, t)) => {
                report.tiled_batches += 1;
                quantized = Some((f, q, Some(t)));
            }
            Err(FloatReason::Position) => report.float_position += 1,
            Err(FloatReason::TriangleUv) => report.float_triangle_uv += 1,
            Err(FloatReason::TooManyVertices) => report.float_vertices += 1,
        }
    }
    let flags_for = |count: usize, white: bool, report: &mut PackReport| {
        let mut f = 0;
        if opts.small_dl {
            if count <= 256 {
                f |= dl_flags::IDX8;
                report.dl_idx8_batches += 1;
            }
            if white {
                f |= dl_flags::NO_COLOR;
                report.dl_no_color_batches += 1;
            }
        }
        f
    };
    let white = verts.iter().all(|v| v.rgba == 0xffff_ffff);
    match quantized {
        Some((vtxfmt, q, new_tris)) => {
            if let Some(t) = new_tris {
                *tris = t;
            }
            let base = compact.len() as u32;
            let count = q.len();
            compact.extend_from_slice(&q);
            let flags = flags_for(count, white, report);
            (
                base,
                wpk::quant::QuantRecord {
                    vtxfmt,
                    flags,
                    origin,
                },
                count as u32,
            )
        }
        None => {
            let base = floats.len() as u32;
            // Same 1/8-unit grid as compact vertices: coplanar layers stay coplanar across formats.
            floats.extend(verts.iter().map(|v| WorldVertex {
                pos: wpk::quant::snap_position(v.pos),
                ..*v
            }));
            let flags = flags_for(verts.len(), white, report);
            (
                base,
                wpk::quant::QuantRecord {
                    vtxfmt: records::WORLD_VTXFMT,
                    flags,
                    origin: [0.0; 3],
                },
                verts.len() as u32,
            )
        }
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
        "textures {} (lightmaps {}), materials {} ({} without texture), world {} vertices, {} triangles of {} before simplification ({} display-list vertices) in {} batches \
         ({} vertex groups), sky surfaces dropped {}, surfaces without material {}, skybox {}, spawns {}\n",
        r.textures,
        r.lightmaps,
        r.materials,
        r.materials_without_texture,
        r.world_vertices,
        r.world_triangles,
        r.world_triangles_source,
        r.world_dl_vertices,
        r.batches,
        r.vertex_groups,
        r.sky_surfaces_dropped,
        r.surfaces_without_material,
        r.skybox,
        r.spawns
    ));
    s.push_str(&format!(
        "bot walk graph: {} points, {} moves
",
        r.nav_nodes, r.nav_edges
    ));
    let m = &r.static_models;
    s.push_str(&format!(
        "static models: {} instances of {} models ({} surfaces, {} vertices, {} triangles; {} placed),          unresolved {}, unreadable {}, unlit {}, surfaces without material {}
",
        m.instances, m.models, m.surfaces, m.vertices, m.triangles, m.placed_triangles, m.unresolved,
        m.unreadable, m.unlit, m.surfaces_without_material
    ));
    if m.small_props > 0 {
        s.push_str(&format!(
            "small props from LOD 1: {} models ({} vertices, {} triangles of LOD 0 left out)\n",
            m.small_props, m.small_prop_vertices_dropped, m.small_prop_triangles_dropped
        ));
    }
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
    s.push_str(&crate::clip_report::render_text(&r.collision_breakdown));
    s.push_str(&crate::tex_report::render_text(&r.texture_breakdown));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slim_views_keep_only_the_base_level() {
        use gx_texture::policy::{Options, Role, convert_with};
        let dir = std::env::temp_dir().join(format!("mwf_base_level_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // An opaque texture and a smooth-alpha one (colour + I4 alpha plane), both with mips.
        let opaque = gx_texture::Image::from_fn(64, 32, |x, y| [x as u8 * 4, y as u8 * 8, 90, 255]);
        let smooth =
            gx_texture::Image::from_fn(64, 64, |x, y| [200, x as u8, y as u8, (x * 4) as u8]);
        for (name, img) in [("opaque.gxt", &opaque), ("smooth.gxt", &smooth)] {
            let c = convert_with(img, Role::Viewmodel, Options::default());
            assert!(c.levels > 1, "{name}: needs mips for this test");
            std::fs::write(dir.join(name), gx_texture::container::write(&c)).unwrap();
        }
        let mut full = Textures::default();
        let mut slim = Textures {
            base_level_only: true,
            ..Textures::default()
        };
        for name in ["opaque.gxt", "smooth.gxt"] {
            let a = full.add_file(&dir, name, Wrap::Repeat).unwrap() as usize;
            let b = slim.add_file(&dir, name, Wrap::Repeat).unwrap() as usize;
            let (fr, sr) = (full.records[a], slim.records[b]);
            assert!(fr.levels > 1 && sr.levels == 1, "{name}");
            assert_eq!(
                (sr.width, sr.height, sr.format),
                (fr.width, fr.height, fr.format)
            );
            let fmt = gx_texture::TexFormat::from_gx_id(sr.format).unwrap();
            assert_eq!(
                sr.data_len as usize,
                fmt.level_size(u32::from(sr.width), u32::from(sr.height))
            );
            assert!(sr.data_len < fr.data_len);
            // The level kept is the same bytes as the full file's level 0.
            let f0 = &full.data[fr.data_off as usize..][..sr.data_len as usize];
            let s0 = &slim.data[sr.data_off as usize..][..sr.data_len as usize];
            assert_eq!(f0, s0);
            if fr.alpha_len > 0 {
                assert!(
                    sr.alpha_len > 0 && sr.alpha_len < fr.alpha_len,
                    "{name}: alpha plane"
                );
                let i4 =
                    gx_texture::TexFormat::I4.level_size(u32::from(sr.width), u32::from(sr.height));
                assert_eq!(sr.alpha_len as usize, i4);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn weld_merges_identical_vertices_only() {
        let v = |x: f32, u: f32| WorldVertex {
            pos: [x, 0.0, 0.0],
            rgba: 0xffff_ffff,
            uv: [u, 0.0],
            lm: [0, 0],
        };
        // 0 and 2 are identical; 3 shares 0's position but not its UV (a seam that must stay); 4 is unused.
        let mut verts = vec![
            v(0.0, 0.0),
            v(1.0, 0.0),
            v(0.0, 0.0),
            v(0.0, 0.5),
            v(9.0, 0.0),
        ];
        let mut tris = vec![0, 1, 3, 2, 1, 3];
        let fewer = weld(&mut verts, &mut tris);
        assert_eq!(fewer, 2);
        assert_eq!(verts, [v(0.0, 0.0), v(1.0, 0.0), v(0.0, 0.5)]);
        assert_eq!(tris, [0, 1, 2, 0, 1, 2]);
    }

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

    #[test]
    fn small_props_start_at_lod_1() {
        use crate::static_models::{ModelLod, ModelSurface};
        // A cube of half-size `h` as `tris` triangles (positions only matter for the bounds).
        let lod = |h: f32, tris: usize, dist: f32| ModelLod {
            dist,
            surfaces: vec![ModelSurface {
                material: Some(0),
                vertices: [[-h, -h, -h], [h, h, h], [h, -h, h]]
                    .iter()
                    .map(|&pos| WorldVertex {
                        pos,
                        rgba: !0,
                        uv: [0.0; 2],
                        lm: [0; 2],
                    })
                    .collect(),
                indices: (0..tris).flat_map(|_| [0u16, 1, 2]).collect(),
            }],
        };
        let small = [lod(10.0, 200, 300.0), lod(10.0, 40, 1500.0)];
        assert_eq!(first_lod(&small, 1.0, false), 0);
        assert_eq!(first_lod(&small, 1.0, true), 1);
        // Placed three times bigger it is no longer small (radius 10·√3·3 ≈ 52).
        assert_eq!(first_lod(&small, 3.0, true), 0);
        // Big models, single-LOD models and LOD 1s that aren't lighter keep LOD 0.
        assert_eq!(
            first_lod(&[lod(40.0, 200, 300.0), lod(40.0, 40, 0.0)], 1.0, true),
            0
        );
        assert_eq!(first_lod(&small[..1], 1.0, true), 0);
        assert_eq!(
            first_lod(&[lod(10.0, 50, 300.0), lod(10.0, 50, 0.0)], 1.0, true),
            0
        );
        assert_eq!(
            first_lod(&[lod(10.0, 50, 300.0), lod(10.0, 0, 0.0)], 1.0, true),
            0
        );
        // The kept LODs still pick by IW4's distances: LOD 1 from the camera to its own switch distance.
        let rec = |lods: &[ModelLod]| records::ModelRecord {
            centre: [0.0; 3],
            radius: 1.0,
            lod_count: lods.len() as u8,
            lods: std::array::from_fn(|i| records::ModelLod {
                first_surface: i as u32,
                surface_count: 1,
                dist: lods.get(i).map_or(0.0, |l| l.dist),
            }),
        };
        let r = rec(&small[first_lod(&small, 1.0, true)..]);
        assert_eq!(r.lod_for(10.0).map(|l| l.first_surface), Some(0));
        assert_eq!(r.lod_for(1499.0).map(|l| l.first_surface), Some(0));
        assert!(r.lod_for(1600.0).is_none());
    }
}

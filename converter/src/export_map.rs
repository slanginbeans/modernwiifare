//! `mwf-convert export-map`: one map as plain files, for Slick Blasters' `tools/mw2/import_map.py`.
//!
//! Written to `<out>/<map>/` (the user's own data: never committed, like every `out/` file):
//! - `world.bin`: the world's draw surfaces and its static models, lit with the map's own light (lightmaps for the
//!   world, the light grid for models), simplified to `--max-tris` (see [`write_mesh`] for the layout).
//! - `models/<name>.bin`: each static model the map places, by itself in model space (LOD 0), so other tools can
//!   place it again (Slick Blasters' map props).
//! - `textures/<n>.png`: each material's colour texture, at most 256 px.
//! - `collision.json`: the map's collision as axis-aligned boxes (see [`collision_boxes`]).
//! - `map.json`: spawns, sun, the play area, materials, models and where the map places them.
//!
//! Units are IW4's (inches, Z up). Only opaque and alpha-tested surfaces are written: sky, decals and blended
//! effects are left out.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gx_texture::image::Image;
use serde::Serialize;
use wpk::clip::Clip;
use wpk::records::{LM_FRAC, WorldVertex, spawn_kind, spawn_team};

use crate::material_links::{BlendMode, Links};
use crate::pack::WorldData;
use crate::zone_textures::{IndexEntry, ZoneImage};

/// IW4 contents bits.
const CONTENTS_SOLID: u32 = 0x1;
const CONTENTS_GLASS: u32 = 0x10;
const CONTENTS_PLAYERCLIP: u32 = 0x1_0000;

/// Collision box kinds in `collision.json`.
pub const KIND_SOLID: u8 = 1;
/// Stops players only (shots and sight pass).
pub const KIND_CLIP: u8 = 2;

/// Default column size of the collision raster (inches): sloped brushes and the triangle mesh (terrain) become
/// columns (`--column`, meters).
const COLUMN: f32 = 12.0;
/// Below an up-facing triangle (above a down-facing one) the ground is solid this deep.
const THICK: f32 = 32.0;
/// Neighbouring columns merge into one box while their tops stay within this (inches; `--step`, meters).
const TOL_TOP: f32 = 6.0;
/// ...and their bottoms within this.
const TOL_BOTTOM: f32 = 24.0;
/// Coarser rasters `export-map` falls back on, in turn, while a map has more than `--max-boxes` boxes (inches:
/// column, step). Every step stays under the player's 0.45 m step-up.
const COARSER: [(f32, f32); 4] = [(16.0, 8.0), (24.0, 10.0), (32.0, 12.0), (48.0, 14.0)];
/// The game's box limit (Slick Blasters' `MAP_MAX_BOXES`), less room for the play area's edge walls.
const DEFAULT_MAX_BOXES: usize = 32_000;
/// Largest texture side written.
const TEX_MAX: u32 = 256;
/// Default triangle budget of `world.bin`.
const DEFAULT_MAX_TRIS: usize = 80_000;
/// Default margin around the spawns that makes the play area (inches; about 30 m).
const DEFAULT_MARGIN: f32 = 1200.0;
/// Grid cell the world is cut into before simplifying (inches).
const CELL: f32 = 2048.0;

#[derive(Serialize)]
struct MaterialOut {
    name: String,
    /// `textures/<n>.png`, when the material has a colour texture.
    #[serde(skip_serializing_if = "Option::is_none")]
    texture: Option<String>,
    /// "opaque" or "alphatest".
    blend: &'static str,
    /// Alpha test threshold (0..255) for "alphatest".
    alpha_ref: u8,
    /// The texture's mean colour (white without one).
    average: [u8; 3],
}

#[derive(Serialize)]
struct SpawnOut {
    origin: [f32; 3],
    /// Degrees, IW4 convention (0 = +X, counter-clockwise seen from above).
    yaw: f32,
    kind: &'static str,
    team: &'static str,
    start: bool,
}

#[derive(Serialize)]
struct ModelOut {
    name: String,
    file: String,
    triangles: usize,
    /// Model-space bounds of LOD 0 (inches, Z up).
    min: [f32; 3],
    max: [f32; 3],
}

#[derive(Serialize)]
struct InstanceOut {
    /// Index into `models`.
    model: usize,
    origin: [f32; 3],
    axis: [[f32; 3]; 3],
    scale: f32,
}

#[derive(Serialize)]
struct SunOut {
    /// Unit vector from the ground toward the sun (from worldspawn's `sundirection`).
    to_sun: [f32; 3],
    color: [f32; 3],
    light: f32,
}

#[derive(Serialize)]
struct WorldOut {
    file: &'static str,
    triangles: usize,
    source_triangles: usize,
    vertices: usize,
}

#[derive(Serialize)]
struct MapOut {
    comment: &'static str,
    map: String,
    units: &'static str,
    /// The area players can reach: the spawns plus a margin (x, y). Collision is written for it only.
    play_min: [f32; 2],
    play_max: [f32; 2],
    /// Bounds of everything drawn.
    world_min: [f32; 3],
    world_max: [f32; 3],
    sun: SunOut,
    worldspawn: BTreeMap<String, String>,
    spawns: Vec<SpawnOut>,
    materials: Vec<MaterialOut>,
    world: WorldOut,
    models: Vec<ModelOut>,
    instances: Vec<InstanceOut>,
    collision: &'static str,
}

#[derive(Serialize)]
struct CollisionOut {
    comment: &'static str,
    /// x0 y0 z0 x1 y1 z1 kind (1 solid, 2 players only), inches.
    boxes: Vec<[f32; 7]>,
    /// Boxes straight from axis-aligned brushes.
    brush_boxes: usize,
    /// Boxes merged from columns (sloped brushes, terrain).
    column_boxes: usize,
    sloped_brushes: usize,
    mesh_triangles: usize,
}

/// A mesh being written: vertices and, per material, triangles.
#[derive(Default)]
pub struct Mesh {
    pub verts: Vec<OutVertex>,
    pub batches: BTreeMap<u32, Vec<u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// Light at half intensity (128 = as authored, 255 = twice as bright), alpha 255.
    pub rgba: [u8; 4],
}

impl Mesh {
    fn triangles(&self) -> usize {
        self.batches.values().map(|b| b.len() / 3).sum()
    }

    /// Append `tris` (indices into `verts`) under `material`.
    fn add(&mut self, material: u32, verts: &[OutVertex], tris: &[u16]) {
        let base = self.verts.len() as u32;
        self.verts.extend_from_slice(verts);
        self.batches
            .entry(material)
            .or_default()
            .extend(tris.iter().map(|&i| base + u32::from(i)));
    }
}

/// `world.bin` / `models/*.bin`, little-endian:
/// `"MWFM"`, u32 version 1, u32 vertex count, u32 batch count; per batch u32 material (index into `map.json`
/// materials), u32 first index, u32 index count; the vertices (f32 pos[3], f32 uv[2], u8 rgba[4]: 24 bytes); the
/// indices (u32, triangles).
pub fn write_mesh(m: &Mesh) -> Vec<u8> {
    let mut o = Vec::with_capacity(16 + m.verts.len() * 24 + m.triangles() * 12);
    o.extend_from_slice(b"MWFM");
    for v in [1u32, m.verts.len() as u32, m.batches.len() as u32] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    let mut first = 0u32;
    for (&mat, idx) in &m.batches {
        for v in [mat, first, idx.len() as u32] {
            o.extend_from_slice(&v.to_le_bytes());
        }
        first += idx.len() as u32;
    }
    for v in &m.verts {
        for f in v.pos.iter().chain(&v.uv) {
            o.extend_from_slice(&f.to_le_bytes());
        }
        o.extend_from_slice(&v.rgba);
    }
    for idx in m.batches.values() {
        for i in idx {
            o.extend_from_slice(&i.to_le_bytes());
        }
    }
    o
}

/// The surfaces a map is drawn with: opaque or alpha-tested, not sky, not decals.
fn drawn(links: &Links, m: usize) -> Option<(&'static str, u8)> {
    let info = links.material_info.get(m)?;
    if info.sort_key == crate::pack::SORT_SKY
        || info.sort_key == crate::pack::SORT_SKYBOX
        || info.polygon_offset > 0
    {
        return None;
    }
    match info.blend {
        BlendMode::Opaque => Some(("opaque", 0)),
        BlendMode::AlphaTest(t) => Some(("alphatest", t)),
        _ => None,
    }
}

/// A `.gxt` the `textures` pass wrote, back to RGBA (its alpha plane folded in).
fn read_gxt(path: &Path) -> Option<Image> {
    let b = std::fs::read(path).ok()?;
    let (h, data) = gx_texture::container::parse(&b).ok()?;
    let mut img = gx_texture::codec::decode(data, h.width.into(), h.height.into(), h.format)?;
    if let Ok(Some((a, plane))) = gx_texture::container::parse_alpha_plane(&b)
        && let Some(alpha) =
            gx_texture::codec::decode(plane, a.width.into(), a.height.into(), a.format)
    {
        for (p, q) in img.pixels.iter_mut().zip(&alpha.pixels) {
            p[3] = q[0];
        }
    }
    Some(img)
}

/// Materials, numbered as they are first used, with their textures written as PNGs.
struct Materials<'a> {
    links: &'a Links,
    files_by_image: HashMap<usize, &'a IndexEntry>,
    tex_dir: &'a Path,
    out_dir: &'a Path,
    index: HashMap<usize, u32>,
    out: Vec<MaterialOut>,
    textures: HashMap<String, (String, [u8; 3])>,
}

impl Materials<'_> {
    fn get(&mut self, m: usize) -> Option<u32> {
        if let Some(&i) = self.index.get(&m) {
            return Some(i);
        }
        let (blend, alpha_ref) = drawn(self.links, m)?;
        let entry = self
            .links
            .colour_image(m)
            .and_then(|img| self.files_by_image.get(&img));
        let mut texture = None;
        let mut average = [255u8; 3];
        if let Some(e) = entry {
            if let Some((file, avg)) = self.textures.get(&e.file) {
                texture = Some(file.clone());
                average = *avg;
            } else if let Some(img) = read_gxt(&self.tex_dir.join(&e.file)) {
                let img = if img.width > TEX_MAX || img.height > TEX_MAX {
                    let (w, h) = gx_texture::image::fit_pow2(img.width, img.height, TEX_MAX);
                    gx_texture::image::resize(&img, w, h)
                } else {
                    img
                };
                let n = self.textures.len();
                let file = format!("textures/{n}.png");
                let mut sum = [0u64; 3];
                for p in &img.pixels {
                    for k in 0..3 {
                        sum[k] += u64::from(p[k]);
                    }
                }
                let cnt = img.pixels.len().max(1) as u64;
                average = sum.map(|s| (s / cnt) as u8);
                if crate::texture::write_png(&self.out_dir.join(&file), &img).is_ok() {
                    self.textures
                        .insert(e.file.clone(), (file.clone(), average));
                    texture = Some(file);
                }
            }
        }
        let i = self.out.len() as u32;
        self.out.push(MaterialOut {
            name: self.links.material_info[m]
                .name
                .clone()
                .unwrap_or_else(|| format!("material{m}")),
            texture,
            blend,
            alpha_ref,
            average,
        });
        self.index.insert(m, i);
        Some(i)
    }
}

/// Bilinear sample of a lightmap at a UV (0..1 across the page).
fn sample(img: &Image, u: f32, v: f32) -> [f32; 3] {
    let (w, h) = (img.width as f32, img.height as f32);
    let x = (u * w - 0.5).clamp(0.0, w - 1.0);
    let y = (v * h - 0.5).clamp(0.0, h - 1.0);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |x, y| img.get(x, y);
    std::array::from_fn(|k| {
        let a = f32::from(p(x0, y0)[k]) * (1.0 - fx) + f32::from(p(x1, y0)[k]) * fx;
        let b = f32::from(p(x0, y1)[k]) * (1.0 - fx) + f32::from(p(x1, y1)[k]) * fx;
        a * (1.0 - fy) + b * fy
    })
}

fn lm_uv(v: u16) -> f32 {
    f32::from(v) / (1u32 << LM_FRAC) as f32
}

/// Triangle lists by (material, cell): their vertices and indices.
type Groups = BTreeMap<(u32, i32, i32, i32), (Vec<WorldVertex>, Vec<u16>)>;

/// Simplify per (material, cell) group, keeping the vertices groups share, then append to `mesh`.
fn simplify_into(mesh: &mut Mesh, groups: Groups, ratio: f32) {
    let quality = crate::simplify::WorldQuality {
        ratio,
        max_error: 16.0,
    };
    let mut groups_at: HashMap<[u32; 3], (usize, u32)> = HashMap::new();
    if ratio < 1.0 {
        for (k, (verts, _)) in groups.values().enumerate() {
            for v in verts {
                let e = groups_at
                    .entry(crate::simplify::position_key(v.pos))
                    .or_insert((k, 0));
                if e.1 == 0 || e.0 != k {
                    e.1 += 1;
                    e.0 = k;
                }
            }
        }
    }
    for (&(mat, ..), (verts, tris)) in &groups {
        let mut verts = verts.clone();
        let mut tris = tris.clone();
        crate::pack::weld(&mut verts, &mut tris);
        if ratio < 1.0 {
            let shared = |v: &WorldVertex| {
                groups_at
                    .get(&crate::simplify::position_key(v.pos))
                    .is_some_and(|e| e.1 > 1)
            };
            tris = crate::simplify::batch(&verts, &tris, &quality, shared);
        }
        let out: Vec<OutVertex> = verts
            .iter()
            .map(|v| OutVertex {
                pos: v.pos,
                uv: v.uv,
                rgba: v.rgba.to_be_bytes(),
            })
            .collect();
        // Only the vertices the triangles still use.
        let mut keep = vec![u16::MAX; out.len()];
        let mut used = Vec::new();
        let tris: Vec<u16> = tris
            .iter()
            .map(|&i| {
                let k = &mut keep[usize::from(i)];
                if *k == u16::MAX {
                    *k = used.len() as u16;
                    used.push(out[usize::from(i)]);
                }
                *k
            })
            .collect();
        mesh.add(mat, &used, &tris);
    }
}

fn cell_of(p: [f32; 3]) -> (i32, i32, i32) {
    (
        (p[0] / CELL).floor() as i32,
        (p[1] / CELL).floor() as i32,
        (p[2] / CELL).floor() as i32,
    )
}

/// Push a triangle list into its (material, cell) group, renumbering vertices; starts a new group part when a group
/// would pass u16 indices.
fn push_group(groups: &mut Groups, key: (u32, i32, i32, i32), verts: &[WorldVertex]) {
    let mut key = key;
    loop {
        let g = groups.entry(key).or_default();
        if g.0.len() + verts.len() < 65_000 {
            let base = g.0.len() as u16;
            g.0.extend_from_slice(verts);
            g.1.extend((0..verts.len() as u16).map(|i| base + i));
            return;
        }
        key.3 += 1 << 16; // a fresh group in the same place
    }
}

/// The world's surfaces and static models as one lit mesh.
fn world_mesh(
    world: &WorldData,
    images: &[ZoneImage],
    mats: &mut Materials<'_>,
    max_tris: usize,
    ratio_override: Option<f32>,
) -> (Mesh, usize) {
    let lightmaps: Vec<Option<Image>> = (0..world.lightmap_count)
        .map(|i| crate::pack::lightmap(images, i))
        .collect();
    let mut groups = BTreeMap::new();
    let mut source = 0usize;
    for surf in &world.surfaces {
        let Some(mat) = surf.material.and_then(|m| mats.get(m)) else {
            continue;
        };
        let lm = lightmaps
            .get(usize::from(surf.lightmap))
            .and_then(Option::as_ref);
        let mut tri_verts = Vec::with_capacity(usize::from(surf.tri_count) * 3);
        let mut centre = [0.0f32; 3];
        for t in 0..usize::from(surf.tri_count) * 3 {
            let Some(&local) = world.indices.get(surf.base_index as usize + t) else {
                break;
            };
            let Some(v) = world
                .vertices
                .get((surf.first_vertex + u32::from(local)) as usize)
            else {
                break;
            };
            let c = v.rgba.to_be_bytes();
            let l = lm.map_or([128.0; 3], |img| {
                sample(img, lm_uv(v.lm[0]), lm_uv(v.lm[1]))
            });
            // half intensity: vertex colour x (page0 + page1) / 2
            let rgb: [u8; 3] =
                std::array::from_fn(|k| (f32::from(c[k]) * l[k] / 255.0).clamp(0.0, 255.0) as u8);
            let mut out = *v;
            out.rgba = u32::from_be_bytes([rgb[0], rgb[1], rgb[2], 255]);
            out.lm = [0, 0];
            for (c, p) in centre.iter_mut().zip(v.pos) {
                *c += p;
            }
            tri_verts.push(out);
        }
        tri_verts.truncate(tri_verts.len() / 3 * 3);
        if tri_verts.is_empty() {
            continue;
        }
        source += tri_verts.len() / 3;
        let centre = centre.map(|c| c / tri_verts.len() as f32);
        let (cx, cy, cz) = cell_of(centre);
        push_group(&mut groups, (mat, cx, cy, cz), &tri_verts);
    }
    // Static models: a lower level of detail when the budget is tight.
    let model_tris: usize = world
        .models
        .instances
        .iter()
        .filter_map(|i| world.models.meshes.get(&i.model)?.first())
        .map(|l| {
            l.surfaces
                .iter()
                .map(|s| s.indices.len() / 3)
                .sum::<usize>()
        })
        .sum();
    let total = source + model_tris;
    let ratio =
        ratio_override.unwrap_or_else(|| (max_tris as f32 / total.max(1) as f32).clamp(0.05, 1.0));
    let mut model_groups = BTreeMap::new();
    for inst in &world.models.instances {
        let Some(lods) = world.models.meshes.get(&inst.model) else {
            continue;
        };
        let lod0: usize = lods
            .first()
            .map_or(0, |l| l.surfaces.iter().map(|s| s.indices.len() / 3).sum());
        let want = (lod0 as f32 * ratio * 1.5) as usize;
        let Some(lod) = lods
            .iter()
            .find(|l| {
                l.surfaces
                    .iter()
                    .map(|s| s.indices.len() / 3)
                    .sum::<usize>()
                    <= want
            })
            .or(lods.last())
        else {
            continue;
        };
        let m = inst.matrix();
        let (cx, cy, cz) = cell_of(inst.origin);
        for s in &lod.surfaces {
            let Some(mat) = s.material.and_then(|m| mats.get(m)) else {
                continue;
            };
            let mut tri_verts = Vec::with_capacity(s.indices.len());
            for &i in &s.indices {
                let Some(v) = s.vertices.get(usize::from(i)) else {
                    break;
                };
                let p = v.pos;
                let pos = std::array::from_fn(|r| {
                    m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]
                });
                let c = v.rgba.to_be_bytes();
                let rgb: [u8; 3] = std::array::from_fn(|k| {
                    (f32::from(c[k]) * inst.light[k] * 0.5).clamp(0.0, 255.0) as u8
                });
                tri_verts.push(WorldVertex {
                    pos,
                    rgba: u32::from_be_bytes([rgb[0], rgb[1], rgb[2], 255]),
                    uv: v.uv,
                    lm: [0, 0],
                });
            }
            tri_verts.truncate(tri_verts.len() / 3 * 3);
            if !tri_verts.is_empty() {
                push_group(&mut model_groups, (mat, cx, cy, cz), &tri_verts);
            }
        }
    }
    let mut mesh = Mesh::default();
    simplify_into(&mut mesh, groups, ratio);
    let world_tris = mesh.triangles();
    simplify_into(&mut mesh, model_groups, ratio);
    println!(
        "world: {} of {} triangles; static models: {} of {} (ratio {ratio:.3})",
        world_tris,
        source,
        mesh.triangles() - world_tris,
        model_tris
    );
    (mesh, total)
}

/// One static model's LOD 0 by itself, in model space, lit as authored (half intensity 128).
fn model_mesh(lods: &[crate::static_models::ModelLod], mats: &mut Materials<'_>) -> Option<Mesh> {
    let lod = lods.first()?;
    let mut mesh = Mesh::default();
    for s in &lod.surfaces {
        let Some(mat) = s.material.and_then(|m| mats.get(m)) else {
            continue;
        };
        let verts: Vec<OutVertex> = s
            .vertices
            .iter()
            .map(|v| {
                let c = v.rgba.to_be_bytes();
                OutVertex {
                    pos: v.pos,
                    uv: v.uv,
                    rgba: [c[0] / 2, c[1] / 2, c[2] / 2, 255],
                }
            })
            .collect();
        mesh.add(mat, &verts, &s.indices);
    }
    (!mesh.batches.is_empty()).then_some(mesh)
}

// ---------------------------------------------------------------------------------------------------------------
// Collision as boxes
// ---------------------------------------------------------------------------------------------------------------

fn kind_of(contents: u32) -> Option<u8> {
    if contents & (CONTENTS_SOLID | CONTENTS_GLASS) != 0 {
        Some(KIND_SOLID)
    } else if contents & CONTENTS_PLAYERCLIP != 0 {
        Some(KIND_CLIP)
    } else {
        None
    }
}

/// Collision raster settings (inches).
#[derive(Clone, Copy, Debug)]
pub struct Raster {
    pub column: f32,
    pub step: f32,
}

impl Default for Raster {
    fn default() -> Self {
        Raster {
            column: COLUMN,
            step: TOL_TOP,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    lo: f32,
    hi: f32,
    kind: u8,
    /// How far the solid should reach below `lo` / above `hi` (a triangle's ground or ceiling), as long as it leaves
    /// room to stand where there was room (see [`Columns::normalize`]).
    down: f32,
    up: f32,
}

impl Span {
    fn solid(lo: f32, hi: f32, kind: u8) -> Span {
        Span {
            lo,
            hi,
            kind,
            down: 0.0,
            up: 0.0,
        }
    }
}

/// Room a standing player needs (inches): a surface thickened toward another never closes a gap taller than this.
const CLEAR: f32 = 76.0;

/// A raster of vertical columns over the play area, each with its solid spans.
struct Columns {
    column: f32,
    tol_top: f32,
    x0: f32,
    y0: f32,
    nx: usize,
    ny: usize,
    cols: Vec<Vec<Span>>,
}

impl Columns {
    fn new(min: [f32; 2], max: [f32; 2], r: Raster) -> Columns {
        let nx = ((max[0] - min[0]) / r.column).ceil().max(1.0) as usize;
        let ny = ((max[1] - min[1]) / r.column).ceil().max(1.0) as usize;
        Columns {
            column: r.column,
            tol_top: r.step,
            x0: min[0],
            y0: min[1],
            nx,
            ny,
            cols: vec![Vec::new(); nx * ny],
        }
    }

    fn index(&self, x: f32, y: f32) -> Option<usize> {
        let ix = ((x - self.x0) / self.column).floor();
        let iy = ((y - self.y0) / self.column).floor();
        (ix >= 0.0 && iy >= 0.0 && (ix as usize) < self.nx && (iy as usize) < self.ny)
            .then(|| iy as usize * self.nx + ix as usize)
    }

    /// Columns overlapping [min, max] (x, y) as index ranges.
    fn range(&self, min: [f32; 2], max: [f32; 2]) -> (usize, usize, usize, usize) {
        let col = self.column;
        let f = |v: f32, o: f32, n: usize| ((v - o) / col).floor().clamp(0.0, n as f32) as usize;
        let c = |v: f32, o: f32, n: usize| ((v - o) / col).ceil().clamp(0.0, n as f32) as usize;
        (
            f(min[0], self.x0, self.nx),
            c(max[0], self.x0, self.nx),
            f(min[1], self.y0, self.ny),
            c(max[1], self.y0, self.ny),
        )
    }

    /// Thicken the surfaces (into the space under ground, over a ceiling, but never so a gap a player could stand
    /// in closes below `CLEAR`), then sort each column's spans and join the overlapping ones of a kind.
    fn normalize(&mut self) {
        for col in &mut self.cols {
            let raw: Vec<(f32, f32)> = col.iter().map(|s| (s.lo, s.hi)).collect();
            for s in col.iter_mut() {
                if s.down > 0.0 {
                    let below = raw
                        .iter()
                        .filter(|r| r.1 < s.lo - 1.0)
                        .map(|r| r.1)
                        .fold(f32::MIN, f32::max);
                    let gap = s.lo - below;
                    let room = if gap >= CLEAR { gap - CLEAR } else { 0.0 };
                    s.lo -= s
                        .down
                        .min(room.max(if below == f32::MIN { s.down } else { 0.0 }));
                    s.down = 0.0;
                }
                if s.up > 0.0 {
                    let above = raw
                        .iter()
                        .filter(|r| r.0 > s.hi + 1.0)
                        .map(|r| r.0)
                        .fold(f32::MAX, f32::min);
                    let gap = above - s.hi;
                    let room = if gap >= CLEAR { gap - CLEAR } else { 0.0 };
                    s.hi +=
                        s.up.min(room.max(if above == f32::MAX { s.up } else { 0.0 }));
                    s.up = 0.0;
                }
            }
            if col.len() < 2 {
                continue;
            }
            col.sort_by(|a, b| (a.kind, a.lo).partial_cmp(&(b.kind, b.lo)).unwrap());
            let mut out: Vec<Span> = Vec::with_capacity(col.len());
            for s in col.drain(..) {
                match out.last_mut() {
                    Some(l) if l.kind == s.kind && s.lo <= l.hi + 1.0 => l.hi = l.hi.max(s.hi),
                    _ => out.push(s),
                }
            }
            *col = out;
        }
    }

    /// Greedy merge into boxes: runs along x, then whole rows along y, while tops and bottoms stay within the
    /// tolerances (a box takes the highest top and the lowest bottom).
    fn boxes(&self) -> Vec<[f32; 7]> {
        let tol = self.tol_top;
        let mut used: Vec<Vec<bool>> = self.cols.iter().map(|c| vec![false; c.len()]).collect();
        let fits = |b: &[f32; 4], s: &Span| {
            b[1].max(s.hi) - b[0].min(s.hi) <= tol && b[3].max(s.lo) - b[2].min(s.lo) <= TOL_BOTTOM
        };
        let grow = |b: &mut [f32; 4], s: &Span| {
            b[0] = b[0].min(s.hi);
            b[1] = b[1].max(s.hi);
            b[2] = b[2].min(s.lo);
            b[3] = b[3].max(s.lo);
        };
        let mut out = Vec::new();
        for iy in 0..self.ny {
            for ix in 0..self.nx {
                let i = iy * self.nx + ix;
                for k in 0..self.cols[i].len() {
                    if used[i][k] {
                        continue;
                    }
                    let s = self.cols[i][k];
                    used[i][k] = true;
                    // [top min, top max, bottom min, bottom max]
                    let mut b = [s.hi, s.hi, s.lo, s.lo];
                    let find = |b: &[f32; 4], j: usize, used: &Vec<Vec<bool>>| {
                        self.cols[j]
                            .iter()
                            .enumerate()
                            .position(|(n, t)| !used[j][n] && t.kind == s.kind && fits(b, t))
                    };
                    let mut x1 = ix + 1;
                    while x1 < self.nx {
                        let j = iy * self.nx + x1;
                        match find(&b, j, &used) {
                            Some(n) => {
                                grow(&mut b, &self.cols[j][n]);
                                used[j][n] = true;
                                x1 += 1;
                            }
                            None => break,
                        }
                    }
                    let mut y1 = iy + 1;
                    'rows: while y1 < self.ny {
                        let mut nb = b;
                        let mut picks = Vec::with_capacity(x1 - ix);
                        for x in ix..x1 {
                            let j = y1 * self.nx + x;
                            match find(&nb, j, &used) {
                                Some(n) => {
                                    grow(&mut nb, &self.cols[j][n]);
                                    picks.push((j, n));
                                }
                                None => break 'rows,
                            }
                        }
                        for (j, n) in picks {
                            used[j][n] = true;
                        }
                        b = nb;
                        y1 += 1;
                    }
                    out.push([
                        self.x0 + ix as f32 * self.column,
                        self.y0 + iy as f32 * self.column,
                        b[2],
                        self.x0 + x1 as f32 * self.column,
                        self.y0 + y1 as f32 * self.column,
                        b[1],
                        f32::from(s.kind),
                    ]);
                }
            }
        }
        out
    }
}

/// The brushes of the world model: those the BSP's leaves list (brush models such as doors have their own).
fn world_brushes(clip: &Clip) -> Vec<bool> {
    let mut on = vec![false; clip.brushes.len()];
    let mut mark = |leaf: usize| {
        if let Some(l) = clip.leaves.get(leaf) {
            let r = l.first_brush as usize..l.first_brush as usize + l.num_brushes as usize;
            for &b in clip.leafbrushes.get(r).unwrap_or(&[]) {
                if let Some(o) = on.get_mut(usize::from(b)) {
                    *o = true;
                }
            }
        }
    };
    if clip.nodes.is_empty() {
        (0..clip.leaves.len()).for_each(&mut mark);
        return on;
    }
    let mut seen = vec![false; clip.nodes.len()];
    let mut stack = vec![0usize];
    while let Some(n) = stack.pop() {
        if std::mem::replace(&mut seen[n], true) {
            continue;
        }
        for &c in &clip.nodes[n].children {
            if c >= 0 {
                if (c as usize) < clip.nodes.len() {
                    stack.push(c as usize);
                }
            } else {
                mark((-1 - c) as usize);
            }
        }
    }
    on
}

/// The vertical extent of a convex brush along the line at (x, y), or `None` where the line misses it.
fn brush_span(planes: &[[f32; 4]], x: f32, y: f32) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = (f32::MIN, f32::MAX);
    for p in planes {
        // inside: n . (x, y, z) <= d
        let r = p[3] - p[0] * x - p[1] * y;
        if p[2] > 1e-5 {
            hi = hi.min(r / p[2]);
        } else if p[2] < -1e-5 {
            lo = lo.max(r / p[2]);
        } else if r < 0.0 {
            return None;
        }
    }
    (lo < hi).then_some((lo, hi))
}

/// The map's collision inside the play area as axis-aligned boxes:
/// - an axis-aligned brush is its own box, exactly;
/// - a sloped brush, and the triangle mesh (terrain, curved patches), go through a raster of 8-inch columns: each
///   column gets its solid spans (a brush's, from its planes at the column's centre and corners; a triangle's from
///   points spread over it, down `THICK` below ground that faces up), and neighbouring columns of about the same
///   span merge into boxes. Slopes become steps of at most `TOL_TOP` plus the slope over one column.
fn collision_boxes(clip: &Clip, min: [f32; 2], max: [f32; 2], r: Raster) -> CollisionOut {
    let mut out = CollisionOut {
        comment: "Collision of a map from your own copy of the game, as boxes. Not for redistribution; never commit it.",
        boxes: Vec::new(),
        brush_boxes: 0,
        column_boxes: 0,
        sloped_brushes: 0,
        mesh_triangles: 0,
    };
    let mut cols = Columns::new(min, max, r);
    let world = world_brushes(clip);
    for (b, rec) in clip.brushes.iter().enumerate() {
        if !world[b] {
            continue;
        }
        let Some(kind) = kind_of(rec.contents) else {
            continue;
        };
        let range = rec.first_plane as usize..rec.first_plane as usize + rec.plane_count as usize;
        let Some(planes) = clip.planes.get(range) else {
            continue;
        };
        if planes.len() < 6 {
            continue;
        }
        let bmin = [-planes[1][3], -planes[3][3], -planes[5][3]];
        let bmax = [planes[0][3], planes[2][3], planes[4][3]];
        if bmax[0] <= min[0] || bmin[0] >= max[0] || bmax[1] <= min[1] || bmin[1] >= max[1] {
            continue;
        }
        if planes.len() == 6 {
            out.boxes.push([
                bmin[0].max(min[0]),
                bmin[1].max(min[1]),
                bmin[2],
                bmax[0].min(max[0]),
                bmax[1].min(max[1]),
                bmax[2],
                f32::from(kind),
            ]);
            out.brush_boxes += 1;
            continue;
        }
        out.sloped_brushes += 1;
        let (x0, x1, y0, y1) = cols.range([bmin[0], bmin[1]], [bmax[0], bmax[1]]);
        for iy in y0..y1 {
            for ix in x0..x1 {
                let cx = cols.x0 + (ix as f32 + 0.5) * r.column;
                let cy = cols.y0 + (iy as f32 + 0.5) * r.column;
                let h = r.column * 0.5 - 0.25;
                let mut span: Option<(f32, f32)> = None;
                for (dx, dy) in [(0.0, 0.0), (-h, -h), (h, -h), (-h, h), (h, h)] {
                    if let Some((lo, hi)) = brush_span(planes, cx + dx, cy + dy) {
                        span = Some(span.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
                    }
                }
                if let Some((lo, hi)) = span {
                    cols.cols[iy * cols.nx + ix].push(Span::solid(lo, hi, kind));
                }
            }
        }
    }
    // The triangle mesh.
    let mesh = &clip.mesh;
    let ntri = mesh.tri_indices.len() / 3;
    for t in 0..ntri {
        let Some(kind) = kind_of(
            mesh.tri_content_flags
                .get(t)
                .copied()
                .unwrap_or(CONTENTS_SOLID),
        ) else {
            continue;
        };
        let v = |k: usize| {
            mesh.verts
                .get(usize::from(mesh.tri_indices[t * 3 + k]))
                .copied()
        };
        let (Some(a), Some(b), Some(c)) = (v(0), v(1), v(2)) else {
            continue;
        };
        let lo_xy = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1])];
        let hi_xy = [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])];
        if hi_xy[0] < min[0] || lo_xy[0] > max[0] || hi_xy[1] < min[1] || lo_xy[1] > max[1] {
            continue;
        }
        out.mesh_triangles += 1;
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];
        let nlen = {
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                nz,
            ];
            (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6)
        };
        // IW4's collision triangles wind clockwise seen from their front: +1 faces up, -1 faces down
        let up = -nz / nlen;
        let len = |e: [f32; 3]| (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
        let longest = len(e1)
            .max(len(e2))
            .max(len([c[0] - b[0], c[1] - b[1], c[2] - b[2]]));
        let n = ((longest / (r.column * 0.5)).ceil() as usize).clamp(1, 1024);
        let mut per_col: HashMap<usize, (f32, f32)> = HashMap::new();
        for i in 0..=n {
            for j in 0..=n - i {
                let (s, r) = (i as f32 / n as f32, j as f32 / n as f32);
                let p: [f32; 3] = std::array::from_fn(|k| a[k] + e1[k] * s + e2[k] * r);
                if let Some(ci) = cols.index(p[0], p[1]) {
                    let e = per_col.entry(ci).or_insert((p[2], p[2]));
                    e.0 = e.0.min(p[2]);
                    e.1 = e.1.max(p[2]);
                }
            }
        }
        for (ci, (lo, hi)) in per_col {
            let mut span = Span::solid(lo, hi.max(lo + 1.0), kind);
            if up > 0.3 {
                span.down = THICK;
            } else if up < -0.3 {
                span.up = THICK;
            }
            cols.cols[ci].push(span);
        }
    }
    cols.normalize();
    let merged = cols.boxes();
    out.column_boxes = merged.len();
    out.boxes.extend(merged);
    out
}

// ---------------------------------------------------------------------------------------------------------------

fn sun(ws: &BTreeMap<String, String>) -> SunOut {
    let nums = |k: &str| -> Vec<f32> {
        ws.get(k)
            .map(|s| {
                s.split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let d = nums("sundirection");
    let (pitch, yaw) = if d.len() >= 2 {
        (d[0].to_radians(), d[1].to_radians())
    } else {
        ((-50f32).to_radians(), 30f32.to_radians())
    };
    // IW4 angles: forward = (cos p cos y, cos p sin y, -sin p)
    let to_sun = [
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        -pitch.sin(),
    ];
    let c = nums("suncolor");
    let color = if c.len() >= 3 {
        [c[0], c[1], c[2]]
    } else {
        [1.0, 1.0, 1.0]
    };
    let light = nums("sunlight").first().copied().unwrap_or(1.0);
    SunOut {
        to_sun,
        color,
        light,
    }
}

#[allow(clippy::too_many_arguments)]
fn export_one(
    a: &crate::MapArgs,
    out_root: &Path,
    max_tris: usize,
    ratio: Option<f32>,
    margin: f32,
    raster: Raster,
    raster_auto: bool,
    max_boxes: usize,
) -> Result<(), String> {
    let (image, label, _) = crate::open_map_zone(a).map_err(|_| format!("can't open {}", a.map))?;
    let tex_dir = PathBuf::from(format!("out/textures/{}", a.map));
    let (index, tables) = crate::convert_textures(a, &image, &label, &tex_dir, false)
        .map_err(|_| "texture conversion failed".to_owned())?;
    let (collector, world) =
        match crate::zone_textures::walk_with(&image, &tables, crate::pack::read_world) {
            Ok((c, Ok(w))) => (c, w),
            Ok((_, Err(e))) | Err(e) => return Err(format!("reading the world of {label}: {e}")),
        };
    let links = &collector.links;
    let dir = out_root.join(&a.map);
    for sub in ["textures", "models"] {
        std::fs::create_dir_all(dir.join(sub))
            .map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    let mut mats = Materials {
        links,
        files_by_image: index
            .iter()
            .flat_map(|e| e.images.iter().map(move |&i| (i, e)))
            .collect(),
        tex_dir: &tex_dir,
        out_dir: &dir,
        index: HashMap::new(),
        out: Vec::new(),
        textures: HashMap::new(),
    };

    let (mesh, source) = world_mesh(&world, &collector.images, &mut mats, max_tris, ratio);
    let world_bytes = write_mesh(&mesh);
    std::fs::write(dir.join("world.bin"), &world_bytes).map_err(|e| e.to_string())?;
    let mut world_min = [f32::MAX; 3];
    let mut world_max = [f32::MIN; 3];
    for v in &mesh.verts {
        for k in 0..3 {
            world_min[k] = world_min[k].min(v.pos[k]);
            world_max[k] = world_max[k].max(v.pos[k]);
        }
    }

    // Each placed model by itself.
    let mut models = Vec::new();
    let mut model_index: HashMap<usize, usize> = HashMap::new();
    let mut names_used: HashMap<String, usize> = HashMap::new();
    for (&m, lods) in &world.models.meshes {
        let Some(mm) = model_mesh(lods, &mut mats) else {
            continue;
        };
        let name = links
            .models
            .get(m)
            .map_or_else(|| format!("model{m}"), |i| i.name.clone());
        let mut stem = crate::export::file_stem(&name);
        let n = names_used.entry(stem.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            stem = format!("{stem}_{n}");
        }
        let file = format!("models/{stem}.bin");
        std::fs::write(dir.join(&file), write_mesh(&mm)).map_err(|e| e.to_string())?;
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in &mm.verts {
            for k in 0..3 {
                min[k] = min[k].min(v.pos[k]);
                max[k] = max[k].max(v.pos[k]);
            }
        }
        model_index.insert(m, models.len());
        models.push(ModelOut {
            name,
            file,
            triangles: mm.triangles(),
            min,
            max,
        });
    }
    let instances = world
        .models
        .instances
        .iter()
        .filter_map(|i| {
            Some(InstanceOut {
                model: *model_index.get(&i.model)?,
                origin: i.origin,
                axis: i.axis,
                scale: i.scale,
            })
        })
        .collect();

    let entities = crate::pack::parse_entities(&world.entity_string);
    let worldspawn = entities
        .iter()
        .find(|e| e.get("classname").map(String::as_str) == Some("worldspawn"))
        .cloned()
        .unwrap_or_default();
    let spawns: Vec<SpawnOut> = crate::pack::spawns(&entities)
        .iter()
        .map(|s| SpawnOut {
            origin: s.origin,
            yaw: s.yaw,
            kind: match s.kind {
                spawn_kind::TDM => "tdm",
                spawn_kind::DM => "dm",
                _ => "other",
            },
            team: match s.team {
                spawn_team::ALLIES => "allies",
                spawn_team::AXIS => "axis",
                _ => "any",
            },
            start: s.flags & 1 != 0,
        })
        .collect();
    if spawns.is_empty() {
        return Err("no spawn points in the map's entities".to_owned());
    }
    let mut play_min = [f32::MAX; 2];
    let mut play_max = [f32::MIN; 2];
    for s in &spawns {
        for k in 0..2 {
            play_min[k] = play_min[k].min(s.origin[k] - margin);
            play_max[k] = play_max[k].max(s.origin[k] + margin);
        }
    }

    let clip = world.clip.as_ref().map_err(|e| format!("collision: {e}"))?;
    // Coarser columns and steps until the map fits the game's box limit (unless they were given).
    let mut tries = vec![raster];
    if raster_auto {
        tries.extend(
            COARSER
                .iter()
                .map(|&(column, step)| Raster { column, step }),
        );
    }
    let mut collision = collision_boxes(clip, play_min, play_max, tries[0]);
    let mut used = tries[0];
    for &r in &tries[1..] {
        if collision.boxes.len() <= max_boxes {
            break;
        }
        collision = collision_boxes(clip, play_min, play_max, r);
        used = r;
    }
    if collision.boxes.len() > max_boxes {
        eprintln!(
            "warning: {} collision boxes, over {max_boxes} even with the coarsest columns: try a smaller --margin",
            collision.boxes.len()
        );
    }
    let coll_json = serde_json::to_vec(&collision).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("collision.json"), coll_json).map_err(|e| e.to_string())?;

    let map = MapOut {
        comment: "Written by mwf-convert export-map from your own copy of the game. Not for redistribution; never commit it.",
        map: a.map.clone(),
        units: "inches, Z up (IW4)",
        play_min,
        play_max,
        world_min,
        world_max,
        sun: sun(&worldspawn),
        worldspawn,
        spawns,
        materials: mats.out,
        world: WorldOut {
            file: "world.bin",
            triangles: mesh.triangles(),
            source_triangles: source,
            vertices: mesh.verts.len(),
        },
        models,
        instances,
        collision: "collision.json",
    };
    let json = serde_json::to_vec_pretty(&map).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("map.json"), json).map_err(|e| e.to_string())?;
    println!(
        "{}: {} of {} triangles, {} materials ({} textures), {} models, {} spawns, {} collision boxes \
         ({} from brushes, {} merged from {} sloped brushes and {} mesh triangles) -> {}",
        a.map,
        map.world.triangles,
        source,
        map.materials.len(),
        mats.textures.len(),
        map.models.len(),
        map.spawns.len(),
        collision.boxes.len(),
        collision.brush_boxes,
        collision.column_boxes,
        collision.sloped_brushes,
        collision.mesh_triangles,
        dir.display()
    );
    println!(
        "collision raster: {:.2} m columns, steps up to {:.2} m",
        used.column * 0.0254,
        used.step * 0.0254
    );
    Ok(())
}

pub fn run(args: &[String]) -> ExitCode {
    let mut out_dir = PathBuf::from("out/export/maps");
    let mut max_tris = DEFAULT_MAX_TRIS;
    let mut margin = DEFAULT_MARGIN;
    let mut ratio = None;
    let mut raster = Raster::default();
    let mut raster_auto = true;
    let mut max_boxes = DEFAULT_MAX_BOXES;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value =
            |it: &mut std::slice::Iter<'_, String>| -> Option<String> { it.next().cloned() };
        match arg.as_str() {
            "--out" | "--max-tris" | "--margin" | "--column" | "--step" | "--max-boxes" => {
                let Some(v) = value(&mut it) else {
                    eprintln!("{arg} needs a value");
                    return ExitCode::from(2);
                };
                match arg.as_str() {
                    "--out" => out_dir = v.into(),
                    "--max-tris" => max_tris = v.parse().unwrap_or(DEFAULT_MAX_TRIS),
                    "--max-boxes" => max_boxes = v.parse().unwrap_or(DEFAULT_MAX_BOXES),
                    "--column" => {
                        raster_auto = false;
                        raster.column = v
                            .parse::<f32>()
                            .map(|m| (m / 0.0254).max(2.0))
                            .unwrap_or(COLUMN)
                    }
                    "--step" => {
                        raster_auto = false;
                        raster.step = v
                            .parse::<f32>()
                            .map(|m| (m / 0.0254).max(0.0))
                            .unwrap_or(TOL_TOP)
                    }
                    _ => {
                        margin = v
                            .parse::<f32>()
                            .map(|m| m / 0.0254)
                            .unwrap_or(DEFAULT_MARGIN)
                    }
                }
            }
            "--simplify" => {
                let Some(v) = value(&mut it) else {
                    eprintln!("--simplify needs a value");
                    return ExitCode::from(2);
                };
                ratio = v.parse::<f32>().ok().map(|r| r.clamp(0.05, 1.0));
            }
            _ => rest.push(arg.clone()),
        }
    }
    let a = match crate::parse_map_args(&rest) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let maps: Vec<String> = if a.map == "all" {
        crate::BASE_MAPS.iter().map(|m| (*m).to_owned()).collect()
    } else {
        vec![a.map.clone()]
    };
    let mut failed = Vec::new();
    for m in &maps {
        let one = crate::MapArgs {
            map: m.clone(),
            zone: if maps.len() > 1 { None } else { a.zone.clone() },
            ..a.clone()
        };
        if let Err(e) = export_one(
            &one,
            &out_dir,
            max_tris,
            ratio,
            margin,
            raster,
            raster_auto,
            max_boxes,
        ) {
            eprintln!("{m}: {e}");
            failed.push(m.clone());
        }
    }
    println!("\nThis is your own game's data: keep it out of git and out of anything you share.");
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_layout() {
        let mut m = Mesh::default();
        let v = OutVertex {
            pos: [1.0, 2.0, 3.0],
            uv: [0.5, 0.25],
            rgba: [10, 20, 30, 255],
        };
        m.add(3, &[v, v, v], &[0, 1, 2]);
        let b = write_mesh(&m);
        assert_eq!(&b[0..4], b"MWFM");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(b[16..20].try_into().unwrap()), 3); // material
        assert_eq!(b.len(), 16 + 12 + 3 * 24 + 3 * 4);
        assert_eq!(&b[28 + 20..28 + 24], &[10, 20, 30, 255]);
    }

    #[test]
    fn sloped_brush_span() {
        // a wedge: the box 0..10 cubed, cut by the plane z <= x (normal (-1, 0, 1)/sqrt2, d 0)
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let planes = [
            [1.0, 0.0, 0.0, 10.0],
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 10.0],
            [0.0, 0.0, -1.0, 0.0],
            [-s, 0.0, s, 0.0],
        ];
        let (lo, hi) = brush_span(&planes, 4.0, 5.0).unwrap();
        assert_eq!(lo, 0.0);
        assert!((hi - 4.0).abs() < 1e-4);
        assert!(brush_span(&planes, 12.0, 5.0).is_none());
    }

    #[test]
    fn columns_merge_flat_ground_into_one_box() {
        let mut c = Columns::new(
            [0.0, 0.0],
            [80.0, 40.0],
            Raster {
                column: 8.0,
                step: 4.0,
            },
        );
        for col in &mut c.cols {
            col.push(Span::solid(-32.0, 0.0, KIND_SOLID));
        }
        c.normalize();
        let b = c.boxes();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0], [0.0, 0.0, -32.0, 80.0, 40.0, 0.0, 1.0]);
    }

    #[test]
    fn columns_keep_steps_apart() {
        // a step of 10 units halfway along x: two boxes
        let mut c = Columns::new(
            [0.0, 0.0],
            [80.0, 16.0],
            Raster {
                column: 8.0,
                step: 4.0,
            },
        );
        for (i, col) in c.cols.iter_mut().enumerate() {
            let x = i % 10;
            col.push(Span::solid(
                -32.0,
                if x < 5 { 0.0 } else { 10.0 },
                KIND_SOLID,
            ));
        }
        c.normalize();
        let mut b = c.boxes();
        b.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
        assert_eq!(b.len(), 2);
        assert_eq!(b[0][3], 40.0);
        assert_eq!(b[1][5], 10.0);
    }

    #[test]
    fn ground_thickens_but_keeps_headroom() {
        let r = Raster {
            column: 8.0,
            step: 4.0,
        };
        let mut c = Columns::new([0.0, 0.0], [8.0, 8.0], r);
        // ground at 0, and a roof whose top is at 100 with its underside at 90: 90 inches to stand in
        let mut ground = Span::solid(0.0, 0.0 + 1.0, 1);
        ground.down = THICK;
        let mut roof = Span::solid(90.0, 100.0, 1);
        roof.down = THICK;
        c.cols[0] = vec![ground, roof];
        c.normalize();
        let col = &c.cols[0];
        assert_eq!(
            col[0].lo, -THICK,
            "nothing under the ground: full thickness"
        );
        assert!(
            (col[1].lo - (1.0 + CLEAR)).abs() < 1e-3,
            "the roof stops where a player still fits: {:?}",
            col[1]
        );
    }

    #[test]
    fn overlapping_spans_join() {
        let mut c = Columns::new(
            [0.0, 0.0],
            [8.0, 8.0],
            Raster {
                column: 8.0,
                step: 4.0,
            },
        );
        c.cols[0] = vec![
            Span::solid(0.0, 10.0, 1),
            Span::solid(5.0, 20.0, 1),
            Span::solid(50.0, 60.0, 1),
        ];
        c.normalize();
        assert_eq!(c.cols[0].len(), 2);
        assert_eq!(c.cols[0][0].hi, 20.0);
    }
}

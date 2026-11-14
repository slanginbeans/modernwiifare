//! A loaded map pack and its per-frame draw.
//!
//! Order per frame (matching the converter's reference renderer): skybox, then visible opaque and alpha-tested
//! batches (grouped by material and lightmap to cut state changes), then blended / additive / multiply batches back
//! to front. Batches whose bounds are outside the view are skipped.
//!
//! Static models are drawn per instance: the instance's model-to-world matrix is folded into the position matrix,
//! the model's surfaces (model-space vertices, same vertex format and display lists as the world) are called, and the
//! instance's light-grid colour tints them. Instances past their draw distance or outside the view are skipped.

use alloc::vec::Vec;

use wpk::records::{
    self, BatchRecord, Blend, InstanceRecord, MaterialRecord, ModelRecord, NONE, Skybox,
    SpawnRecord, TextureRecord, material_flags, texture_flags,
};
use wpk::{Pack, ParseError, tag};

use dpvs_iw4::{
    CPlane, CellPortalGraph, DpvsPlanes, PortalView, VisBits, WalkScratch, dpvs_view_plane,
    visit_cells,
};
use wpk::quant::{CompactVertex, QuantRecord};
use wpk::records::Record;
use wpk::vis::{ALWAYS, Vis};

use crate::camera::{Camera, ZFAR, ZNEAR, dot, sub};
use crate::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use crate::sky;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    Pack(ParseError),
    MissingSection([u8; 4]),
    /// A record points outside its section.
    BadRecord([u8; 4], usize),
    /// The GPU texture table is full.
    TooManyTextures,
}

/// Per-frame counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub batches: u32,
    pub culled: u32,
    pub triangles: u32,
    /// Of `triangles`, those of static models.
    pub model_triangles: u32,
    pub state_changes: u32,
    pub instances: u32,
    pub instances_culled: u32,
    /// Visibility cells drawn this frame, and whether the camera was inside the cell BSP.
    pub cells_visible: u32,
    pub in_cell: bool,
}

/// Multiplies the distance used to pick a static model's LOD. 1.0 is the PC game; larger switches to lower detail
/// sooner and drops distant models sooner (a 60 fps lever, see `docs/perf.md`).
pub const LOD_SCALE: f32 = 1.0;

/// The most cells a map may have for the portal walk (`dpvs_iw4::WalkScratch` tracks 512 path cells).
pub const MAX_CELLS: usize = 512;

/// IW4 visibility in the shape `dpvs_iw4` walks: cell BSP planes and nodes, and each cell's portals.
struct VisRuntime<'a> {
    planes: Vec<CPlane>,
    nodes: Vec<u16>,
    views: Vec<PortalView<'a>>,
    /// Each cell's run in `views`.
    cell_ranges: Vec<(u32, u32)>,
    /// One bit per cell, rebuilt every frame.
    words: Vec<u32>,
}

impl<'a> VisRuntime<'a> {
    fn new(v: &'a Vis) -> Option<VisRuntime<'a>> {
        if v.cells.len() < 2 || v.cells.len() > MAX_CELLS {
            return None;
        }
        let views = v
            .portals
            .iter()
            .map(|p| PortalView {
                plane: p.plane,
                neighbor: p.neighbor,
                vertices: &v.verts[p.first_vert as usize..(p.first_vert + p.vert_count) as usize],
                hull_axis: p.hull_axis,
            })
            .collect();
        Some(VisRuntime {
            planes: v
                .planes
                .iter()
                .map(|p| CPlane {
                    normal: p.normal,
                    dist: p.dist,
                    r#type: p.kind,
                })
                .collect(),
            nodes: v.nodes.clone(),
            views,
            cell_ranges: v
                .cells
                .iter()
                .map(|c| (c.first_portal, c.portal_count))
                .collect(),
            words: alloc::vec![0; v.cells.len().div_ceil(32)],
        })
    }

    /// Mark the cells visible from `cam` (all of them when the eye is outside the cell BSP). Returns whether the eye
    /// was in a cell.
    fn update(&mut self, cam: &Camera, frustum: &[[f32; 4]; 6], scratch: &mut WalkScratch) -> bool {
        let cell_count = self.cell_ranges.len();
        let eye_cell = DpvsPlanes {
            planes: &self.planes,
            nodes: &self.nodes,
            cell_count: cell_count as u32,
        }
        .cell_for_point(cam.eye);
        let Some(cell) = eye_cell else {
            self.words.iter_mut().for_each(|w| *w = u32::MAX);
            return false;
        };
        let mut slices: [&[PortalView<'_>]; MAX_CELLS] = [&[]; MAX_CELLS];
        for (i, &(first, count)) in self.cell_ranges.iter().enumerate() {
            slices[i] = &self.views[first as usize..(first + count) as usize];
        }
        let graph = CellPortalGraph {
            portals: &slices[..cell_count],
        };
        let mut bits = VisBits::new(&mut self.words);
        visit_cells(
            &graph,
            cell,
            cam.eye,
            dpvs_view_plane(cam.forward(), cam.eye),
            frustum,
            0,
            &mut bits,
            scratch,
            None,
            None,
            None,
        );
        true
    }

    fn visible(&self, cell: u16) -> bool {
        cell == ALWAYS
            || self
                .words
                .get(usize::from(cell) / 32)
                .is_some_and(|w| w & (1 << (cell % 32)) != 0)
    }
}

pub struct World<'a> {
    pub name: &'a str,
    textures: Vec<i32>,
    materials: Vec<MaterialRecord>,
    batches: Vec<BatchRecord>,
    vertices: &'a [u8],
    dls: &'a [u8],
    sky: Option<[i32; 6]>,
    pub spawns: Vec<SpawnRecord>,
    model_vertices: &'a [u8],
    /// Compact vertex arrays and how each batch / model surface is stored (missing = float).
    vertices_q: &'a [u8],
    model_vertices_q: &'a [u8],
    batch_quant: Vec<QuantRecord>,
    surface_quant: Vec<QuantRecord>,
    model_surfaces: Vec<BatchRecord>,
    models: Vec<ModelRecord>,
    instances: Vec<InstanceRecord>,
    /// Per instance, fixed for the map: world-space centre, bounding radius and placement scale (computed at load,
    /// not every frame).
    instance_bounds: Vec<([f32; 3], f32, f32)>,
    vis: Option<VisRuntime<'a>>,
    /// Cell of each batch / instance (`ALWAYS` when none); empty when the pack has no visibility.
    batch_cells: Vec<u16>,
    instance_cells: Vec<u16>,
    /// Scratch for the frame's draw order: (class, sort key, draw). A draw below `MODEL_DRAW` is a world batch;
    /// otherwise it indexes `model_draws`.
    order: Vec<(u8, i32, u32)>,
    /// Scratch: (instance, model surface) of each visible model surface.
    model_draws: Vec<(u16, u32)>,
}

const MODEL_DRAW: u32 = 1 << 31;

/// `a * b` for affine 3x4 matrices (`b` applied first).
pub fn mul34(a: &Mtx34, b: &Mtx34) -> Mtx34 {
    core::array::from_fn(|r| {
        core::array::from_fn(|c| {
            let t = if c == 3 { a[r][3] } else { 0.0 };
            a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c] + t
        })
    })
}

fn place(m: &Mtx34, p: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

/// Largest axis scale of a placement matrix.
fn max_scale(m: &Mtx34) -> f32 {
    (0..3)
        .map(|c| libm::sqrtf(m[0][c] * m[0][c] + m[1][c] * m[1][c] + m[2][c] * m[2][c]))
        .fold(0.0, f32::max)
}

/// Background colour behind everything (when a map has no skybox).
pub const CLEAR: u32 = 0x4a5a6aff;

/// A map's visibility and the cell of each batch and instance: decoded at load from the `VISD`, `WBCL` and `SMCL`
/// sections (which need not stay in memory). The caller keeps it alive as long as the world.
#[derive(Clone, Debug, Default)]
pub struct VisData {
    pub vis: Vis,
    pub batch_cells: Vec<u16>,
    pub instance_cells: Vec<u16>,
}

impl VisData {
    /// From the three sections' bytes (any may be missing: no visibility then).
    pub fn decode(
        vis: Option<&[u8]>,
        batch_cells: Option<&[u8]>,
        instance_cells: Option<&[u8]>,
    ) -> Option<VisData> {
        let be16 = |d: Option<&[u8]>| -> Vec<u16> {
            d.unwrap_or(&[])
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect()
        };
        Some(VisData {
            vis: Vis::decode(vis?).ok()?,
            batch_cells: be16(batch_cells),
            instance_cells: be16(instance_cells),
        })
    }

    /// From a whole pack in memory (host tools and tests).
    pub fn from_pack(bytes: &[u8]) -> Option<VisData> {
        let pack = Pack::parse(bytes, false).ok()?;
        let sec = |t| pack.section(t).map(|s| s.data);
        VisData::decode(
            sec(tag::VIS),
            sec(tag::BATCH_CELLS),
            sec(tag::INSTANCE_CELLS),
        )
    }
}

impl<'a> World<'a> {
    /// Create the textures and tables of a pack held in memory. `bytes` may be the whole pack or just its in-place
    /// prefix (`Pack::in_place_len`); visibility comes in `vis` (see [`VisData`]).
    pub fn load(
        bytes: &'a [u8],
        vis: Option<&'a VisData>,
        gfx: &mut impl Gfx,
    ) -> Result<World<'a>, LoadError> {
        let pack = Pack::parse_prefix(bytes).map_err(LoadError::Pack)?;
        let need = |t: [u8; 4]| pack.section(t).ok_or(LoadError::MissingSection(t));
        let tex_sec = need(tag::TEXTURES)?;
        let txda = need(tag::TEXTURE_DATA)?.data;
        let mut textures = Vec::with_capacity(tex_sec.count as usize);
        for (i, r) in records::iter::<TextureRecord>(tex_sec.data).enumerate() {
            let data = txda
                .get(r.data_off as usize..(r.data_off + r.data_len) as usize)
                .ok_or(LoadError::BadRecord(tag::TEXTURES, i))?;
            let alpha = if r.flags & texture_flags::ALPHA_PLANE != 0 {
                txda.get(r.alpha_off as usize..(r.alpha_off + r.alpha_len) as usize)
                    .ok_or(LoadError::BadRecord(tag::TEXTURES, i))?
                    .as_ptr()
            } else {
                core::ptr::null()
            };
            let h = gfx.tex_create(&TexDesc {
                data: data.as_ptr(),
                alpha,
                width: r.width,
                height: r.height,
                format: r.format,
                levels: r.levels,
                wrap: r.wrap,
            });
            if h < 0 {
                return Err(LoadError::TooManyTextures);
            }
            textures.push(h);
        }
        let materials: Vec<MaterialRecord> = records::iter(need(tag::MATERIALS)?.data).collect();
        let vertices = need(tag::WORLD_VERTICES)?.data;
        let batches: Vec<BatchRecord> = records::iter(need(tag::WORLD_BATCHES)?.data).collect();
        let dls = need(tag::WORLD_DISPLAY_LISTS)?.data;
        let opt = |t: [u8; 4]| pack.section(t).map(|s| s.data).unwrap_or(&[]);
        let vertices_q = opt(tag::WORLD_VERTICES_Q);
        let batch_quant: Vec<QuantRecord> = records::iter(opt(tag::BATCH_QUANT)).collect();
        // A batch's vertices: (array, stride) by how they are stored.
        let span = |b: &BatchRecord, q: Option<&QuantRecord>, floats: &[u8], compact: &[u8]| {
            let (len, arr) = if q.is_some_and(QuantRecord::is_compact) {
                (CompactVertex::LEN, compact.len())
            } else {
                (records::WORLD_VERTEX_LEN, floats.len())
            };
            (b.vertex_base as usize + b.vertex_count as usize) * len <= arr
        };
        for (i, b) in batches.iter().enumerate() {
            if !span(b, batch_quant.get(i), vertices, vertices_q)
                || (b.dl_off + b.dl_len) as usize > dls.len()
                || b.dl_off % 32 != 0
                || b.dl_len % 32 != 0
                || usize::from(b.material) >= materials.len()
            {
                return Err(LoadError::BadRecord(tag::WORLD_BATCHES, i));
            }
        }
        // Static models (optional sections; older packs have none).
        let model_vertices = opt(tag::MODEL_VERTICES);
        let model_vertices_q = opt(tag::MODEL_VERTICES_Q);
        let surface_quant: Vec<QuantRecord> = records::iter(opt(tag::SURFACE_QUANT)).collect();
        let model_surfaces: Vec<BatchRecord> = records::iter(opt(tag::MODEL_SURFACES)).collect();
        let models: Vec<ModelRecord> = records::iter(opt(tag::MODELS)).collect();
        let instances: Vec<InstanceRecord> = records::iter(opt(tag::MODEL_INSTANCES)).collect();
        for (i, b) in model_surfaces.iter().enumerate() {
            if !span(b, surface_quant.get(i), model_vertices, model_vertices_q)
                || (b.dl_off + b.dl_len) as usize > dls.len()
                || b.dl_off % 32 != 0
                || b.dl_len % 32 != 0
                || usize::from(b.material) >= materials.len()
            {
                return Err(LoadError::BadRecord(tag::MODEL_SURFACES, i));
            }
        }
        for (i, m) in models.iter().enumerate() {
            let bad = m.lod_count == 0
                || m.lods[..usize::from(m.lod_count.min(4))].iter().any(|l| {
                    l.first_surface as usize + usize::from(l.surface_count) > model_surfaces.len()
                });
            if bad {
                return Err(LoadError::BadRecord(tag::MODELS, i));
            }
        }
        if let Some(i) = instances
            .iter()
            .position(|n| usize::from(n.model) >= models.len())
        {
            return Err(LoadError::BadRecord(tag::MODEL_INSTANCES, i));
        }
        let runtime = vis.and_then(|v| VisRuntime::new(&v.vis));
        let (batch_cells, instance_cells) = match (vis, &runtime) {
            (Some(v), Some(_)) => (v.batch_cells.clone(), v.instance_cells.clone()),
            _ => (Vec::new(), Vec::new()),
        };
        let vis = runtime;
        let handle = |t: u16| -> i32 { textures.get(usize::from(t)).copied().unwrap_or(-1) };
        let sky = pack
            .section(tag::SKYBOX)
            .and_then(|s| records::get::<Skybox>(s.data, 0))
            .map(|s| s.faces.map(handle));
        let spawns = pack
            .section(tag::SPAWNS)
            .map(|s| records::iter(s.data).collect())
            .unwrap_or_default();
        let name = core::str::from_utf8(&bytes[28..28 + wpk::NAME_LEN])
            .ok()
            .and_then(|s| s.split('\0').next())
            .unwrap_or("");
        Ok(World {
            name,
            textures,
            materials,
            order: Vec::with_capacity(batches.len()),
            batches,
            vertices,
            dls,
            sky,
            spawns,
            model_vertices,
            vertices_q,
            model_vertices_q,
            batch_quant,
            surface_quant,
            model_draws: Vec::with_capacity(model_surfaces.len()),
            model_surfaces,
            instance_bounds: instances
                .iter()
                .map(|inst| {
                    let scale = max_scale(&inst.mtx);
                    models
                        .get(usize::from(inst.model))
                        .map_or(([0.0; 3], 0.0, scale), |m| {
                            (place(&inst.mtx, m.centre), m.radius * scale, scale)
                        })
                })
                .collect(),
            models,
            instances,
            vis,
            batch_cells,
            instance_cells,
        })
    }

    /// Whether this map draws through visibility cells.
    pub fn has_cells(&self) -> bool {
        self.vis.is_some()
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    fn state(&self, b: &BatchRecord) -> DrawState {
        let m = &self.materials[usize::from(b.material)];
        let tex = |t: u16| {
            if t == NONE {
                -1
            } else {
                self.textures.get(usize::from(t)).copied().unwrap_or(-1)
            }
        };
        DrawState {
            tex: tex(m.colour),
            lightmap: tex(b.lightmap),
            blend: m.blend,
            alpha_ref: m.alpha_ref,
            cull: m.cull,
            tint: 0,
        }
    }

    /// Draw the world for one frame from `cam` (frame begin included; the caller draws its HUD and ends the frame).
    pub fn draw(
        &mut self,
        gfx: &mut impl Gfx,
        cam: &Camera,
        scratch: &mut WalkScratch,
    ) -> FrameStats {
        let mut stats = FrameStats::default();
        let frustum = cam.frustum(ZNEAR, ZFAR);
        if let Some(v) = self.vis.as_mut() {
            stats.in_cell = v.update(cam, &frustum.planes, scratch);
            stats.cells_visible = v
                .words
                .iter()
                .map(|w| w.count_ones())
                .sum::<u32>()
                .min(v.cell_ranges.len() as u32);
        }
        let cell_of = |cells: &[u16], i: usize| cells.get(i).copied().unwrap_or(ALWAYS);
        gfx.frame_begin(&cam.view(), cam.fovy, ZNEAR, ZFAR, CLEAR);
        if let Some(faces) = self.sky {
            let rot = cam.view_rotation();
            for (f, &t) in faces.iter().enumerate() {
                gfx.sky_face(&rot, &sky::face_quad(f, 100.0), t);
            }
        }

        // Visible draws: (class, sort key, draw). Class 0 world opaque / alpha test, 1 static models opaque,
        // 2 blended (world and models, far to near).
        self.order.clear();
        for (i, b) in self.batches.iter().enumerate() {
            let in_view = self
                .vis
                .as_ref()
                .is_none_or(|v| v.visible(cell_of(&self.batch_cells, i)));
            if !in_view || !frustum.sees_box(b.bounds_min, b.bounds_max) {
                stats.culled += 1;
                continue;
            }
            let m = &self.materials[usize::from(b.material)];
            if Blend::from_u8(m.blend) >= Blend::Blend {
                let c = [
                    (b.bounds_min[0] + b.bounds_max[0]) * 0.5,
                    (b.bounds_min[1] + b.bounds_max[1]) * 0.5,
                    (b.bounds_min[2] + b.bounds_max[2]) * 0.5,
                ];
                // Far first: larger depth sorts first.
                self.order.push((2, -(cam.depth_of(c) as i32), i as u32));
            } else {
                let key = (i32::from(b.material) << 16) | i32::from(b.lightmap);
                self.order.push((0, key, i as u32));
            }
        }
        // Visible static model surfaces.
        self.model_draws.clear();
        let fwd = cam.forward();
        for (ii, inst) in self.instances.iter().enumerate() {
            if self
                .vis
                .as_ref()
                .is_some_and(|v| !v.visible(cell_of(&self.instance_cells, ii)))
            {
                stats.instances_culled += 1;
                continue;
            }
            let model = &self.models[usize::from(inst.model)];
            let (centre, r, scale) = self.instance_bounds[ii];
            let d = sub(centre, cam.eye);
            if inst.cull_dist != 0 {
                let far = f32::from(inst.cull_dist);
                if dot(d, d) > far * far {
                    stats.instances_culled += 1;
                    continue;
                }
            }
            // IW4 LOD choice: camera distance over placement scale against each LOD's switch distance; past the
            // last LOD the model is not drawn.
            let Some(lod) = model.lod_for(libm::sqrtf(dot(d, d)) * LOD_SCALE / scale.max(1e-6))
            else {
                stats.instances_culled += 1;
                continue;
            };
            if !frustum.sees_sphere(centre, r) {
                stats.instances_culled += 1;
                continue;
            }
            stats.instances += 1;
            let depth = dot(d, fwd);
            let first = lod.first_surface;
            for si in first..first + u32::from(lod.surface_count) {
                let b = &self.model_surfaces[si as usize];
                let m = &self.materials[usize::from(b.material)];
                let draw = MODEL_DRAW | self.model_draws.len() as u32;
                self.model_draws.push((ii as u16, si));
                if Blend::from_u8(m.blend) >= Blend::Blend {
                    self.order.push((2, -(depth as i32), draw));
                } else {
                    // After the world's opaque batches, instance by instance so each matrix loads once.
                    self.order.push((1, self.model_draws.len() as i32, draw));
                }
            }
        }
        self.order.sort_unstable();

        let view = cam.view();
        let mut last_state: Option<DrawState> = None;
        // Bound vertex block: (model vertices?, compact?, base). Loaded matrix: (instance or u32::MAX for the view,
        // polygon offset level, vertex origin); the view alone is loaded by frame_begin.
        let float = QuantRecord {
            vtxfmt: records::WORLD_VTXFMT,
            origin: [0.0; 3],
        };
        let mut last_base = (false, false, u32::MAX);
        let mut last_mtx = (u32::MAX, 0u8, [0u32; 3]);
        for &(_, _, d) in &self.order {
            let (b, q, inst) = if d & MODEL_DRAW != 0 {
                let (ii, si) = self.model_draws[(d & !MODEL_DRAW) as usize];
                (
                    &self.model_surfaces[si as usize],
                    self.surface_quant
                        .get(si as usize)
                        .copied()
                        .unwrap_or(float),
                    Some(ii),
                )
            } else {
                (
                    &self.batches[d as usize],
                    self.batch_quant.get(d as usize).copied().unwrap_or(float),
                    None,
                )
            };
            let (verts, stride) = match (inst.is_some(), q.is_compact()) {
                (false, false) => (self.vertices, records::WORLD_VERTEX_LEN),
                (false, true) => (self.vertices_q, CompactVertex::LEN),
                (true, false) => (self.model_vertices, records::WORLD_VERTEX_LEN),
                (true, true) => (self.model_vertices_q, CompactVertex::LEN),
            };
            let offset =
                self.materials[usize::from(b.material)].flags & material_flags::POLYGON_OFFSET;
            let want = (
                inst.map_or(u32::MAX, u32::from),
                offset,
                q.origin.map(f32::to_bits),
            );
            if want != last_mtx {
                last_mtx = want;
                let mut m = match inst {
                    Some(ii) => mul34(&view, &self.instances[usize::from(ii)].mtx),
                    None => view,
                };
                // Compact vertices are relative to their batch's origin.
                for row in m.iter_mut() {
                    row[3] += row[0] * q.origin[0] + row[1] * q.origin[1] + row[2] * q.origin[2];
                }
                // Decals: pull towards the eye along each view ray (same pixels, smaller depth).
                let pull = material_flags::depth_pull(offset);
                m.iter_mut().flatten().for_each(|v| *v *= pull);
                gfx.pos_matrix(&m);
            }
            if (inst.is_some(), q.is_compact(), b.vertex_base) != last_base {
                last_base = (inst.is_some(), q.is_compact(), b.vertex_base);
                let off = b.vertex_base as usize * stride;
                gfx.world_arrays(verts[off..].as_ptr(), stride as u8);
            }
            let mut s = self.state(b);
            if let Some(ii) = inst {
                s.tint = self.instances[usize::from(ii)].light;
            }
            if last_state != Some(s) {
                gfx.world_state(&s);
                last_state = Some(s);
                stats.state_changes += 1;
            }
            gfx.call_dl(&self.dls[b.dl_off as usize..(b.dl_off + b.dl_len) as usize]);
            stats.batches += 1;
            stats.triangles += b.tri_count;
            if inst.is_some() {
                stats.model_triangles += b.tri_count;
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use wpk::records::{Cull, Recipe, WorldVertex};

    #[derive(Debug, PartialEq)]
    enum Call {
        Tex(u16, u16, bool),
        Begin,
        Arrays(usize),
        State(DrawState),
        Dl(usize),
        Sky(i32),
        /// Eye-space z of the matrix's translation (0 for the view at the origin).
        Mtx(i32),
        End,
    }

    #[derive(Default)]
    struct Rec {
        calls: Vec<Call>,
        base: usize,
        dl_base: usize,
    }

    impl Gfx for Rec {
        fn tex_create(&mut self, d: &TexDesc) -> i32 {
            self.calls
                .push(Call::Tex(d.width, d.height, !d.alpha.is_null()));
            self.calls
                .iter()
                .filter(|c| matches!(c, Call::Tex(..)))
                .count() as i32
                - 1
        }
        fn frame_begin(&mut self, _: &crate::gfx::Mtx34, _: f32, _: f32, _: f32, _: u32) {
            self.calls.push(Call::Begin);
        }
        fn world_arrays(&mut self, v: *const u8, _stride: u8) {
            self.calls
                .push(Call::Arrays((v as usize).wrapping_sub(self.base)));
        }
        fn world_state(&mut self, s: &DrawState) {
            self.calls.push(Call::State(*s));
        }
        fn call_dl(&mut self, dl: &[u8]) {
            self.calls
                .push(Call::Dl(dl.as_ptr() as usize - self.dl_base));
        }
        fn sky_face(&mut self, _: &crate::gfx::Mtx34, _: &[[f32; 5]; 4], tex: i32) {
            self.calls.push(Call::Sky(tex));
        }
        fn pos_matrix(&mut self, m: &crate::gfx::Mtx34) {
            self.calls.push(Call::Mtx(m[2][3] as i32));
        }
        fn hud_rects(&mut self, _: &[crate::hud::Rect], _: u32) {}
        fn frame_end(&mut self) {
            self.calls.push(Call::End);
        }
    }

    fn batch(base: u32, material: u16, x: f32, dl_off: u32) -> BatchRecord {
        BatchRecord {
            vertex_base: base,
            vertex_count: 3,
            material,
            lightmap: NONE,
            dl_off,
            dl_len: 32,
            tri_count: 1,
            bounds_min: [x - 1.0, -1.0, -1.0],
            bounds_max: [x + 1.0, 1.0, 1.0],
        }
    }

    fn sample_pack() -> Vec<u8> {
        sample_pack_with(|_| {})
    }

    fn sample_pack_with(extra: impl FnOnce(&mut wpk::PackBuilder)) -> Vec<u8> {
        let tex = TextureRecord {
            format: 14,
            levels: 1,
            flags: texture_flags::ALPHA_PLANE,
            wrap: 0,
            width: 8,
            height: 8,
            data_off: 0,
            data_len: 32,
            alpha_off: 32,
            alpha_len: 32,
        };
        let mat = |blend: Blend| MaterialRecord {
            recipe: Recipe::Lightmapped as u8,
            blend: blend as u8,
            alpha_ref: 0,
            cull: Cull::Back as u8,
            colour: 0,
            sort: 0,
            flags: 0,
        };
        let v = WorldVertex {
            pos: [0.0; 3],
            rgba: 0xffffffff,
            uv: [0.0; 2],
            lm: [0; 2],
        };
        let batches = [
            batch(0, 1, 100.0, 0),   // blended, near
            batch(3, 0, 200.0, 32),  // opaque
            batch(0, 1, 300.0, 64),  // blended, far
            batch(0, 0, -100.0, 96), // behind the camera
        ];
        let mut b = wpk::PackBuilder::new("mp_test");
        b.section(tag::TEXTURES, 1, records::encode(&[tex]));
        b.section(tag::TEXTURE_DATA, 0, vec![0u8; 64]);
        b.section(
            tag::MATERIALS,
            2,
            records::encode(&[mat(Blend::Opaque), mat(Blend::Blend)]),
        );
        b.section(tag::WORLD_VERTICES, 6, records::encode(&[v; 6]));
        b.section(tag::WORLD_BATCHES, 4, records::encode(&batches));
        b.section(tag::WORLD_DISPLAY_LISTS, 0, vec![0u8; 128]);
        b.section(tag::SKYBOX, 1, records::encode(&[Skybox { faces: [0; 6] }]));
        extra(&mut b);
        b.finish()
    }

    #[test]
    fn loads_and_draws_in_order() {
        let bytes = sample_pack();
        let pack = Pack::parse(&bytes, false).unwrap();
        let mut gfx = Rec {
            base: pack.section(tag::WORLD_VERTICES).unwrap().data.as_ptr() as usize,
            dl_base: pack
                .section(tag::WORLD_DISPLAY_LISTS)
                .unwrap()
                .data
                .as_ptr() as usize,
            ..Rec::default()
        };
        let mut world = World::load(&bytes, None, &mut gfx).unwrap();
        assert_eq!(world.name, "mp_test");
        assert_eq!(gfx.calls, [Call::Tex(8, 8, true)]);
        gfx.calls.clear();

        let cam = Camera::new([0.0; 3], 0.0, 0.0);
        let stats = world.draw(&mut gfx, &cam, &mut scratch());
        assert_eq!((stats.batches, stats.culled, stats.triangles), (3, 1, 3));
        let mut expect = vec![Call::Begin];
        expect.extend((0..6).map(|_| Call::Sky(0)));
        let opaque = DrawState {
            tex: 0,
            lightmap: -1,
            blend: Blend::Opaque as u8,
            alpha_ref: 0,
            cull: Cull::Back as u8,
            tint: 0,
        };
        let blended = DrawState {
            blend: Blend::Blend as u8,
            ..opaque
        };
        // Opaque first, then blended far (dl 64) before near (dl 0); arrays rebound when the vertex base changes.
        expect.extend([
            Call::Arrays(3 * records::WORLD_VERTEX_LEN),
            Call::State(opaque),
            Call::Dl(32),
            Call::Arrays(0),
            Call::State(blended),
            Call::Dl(64),
            Call::Dl(0),
        ]);
        assert_eq!(gfx.calls, expect);
    }

    fn scratch() -> alloc::boxed::Box<WalkScratch> {
        alloc::boxed::Box::new(WalkScratch::new())
    }

    /// Two cells split by the plane x = 150 (cell 0 below, cell 1 above), one square portal between them at
    /// y, z in [y0, y0 + 10].
    fn two_cells(y0: f32) -> Vis {
        use wpk::vis::{Cell, Plane, Portal};
        let quad = [
            [150.0, y0, -5.0],
            [150.0, y0 + 10.0, -5.0],
            [150.0, y0 + 10.0, 5.0],
            [150.0, y0, 5.0],
        ];
        let portal = |neighbor: u16, first_vert: u32, sign: f32| Portal {
            plane: [sign, 0.0, 0.0, -150.0 * sign],
            neighbor,
            first_vert,
            vert_count: 4,
            hull_axis: None,
        };
        Vis {
            planes: vec![Plane {
                normal: [1.0, 0.0, 0.0],
                dist: 150.0,
                kind: 0,
            }],
            // Plane 0 (node value cell_count + 1); front (x > 150) -> cell 1, back -> cell 0.
            nodes: vec![3, 3, 2, 1],
            cells: vec![
                Cell {
                    mins: [-1000.0; 3],
                    maxs: [150.0, 1000.0, 1000.0],
                    first_portal: 0,
                    portal_count: 1,
                },
                Cell {
                    mins: [150.0, -1000.0, -1000.0],
                    maxs: [1000.0; 3],
                    first_portal: 1,
                    portal_count: 1,
                },
            ],
            portals: vec![portal(1, 0, 1.0), portal(0, 4, -1.0)],
            verts: quad.iter().chain(quad.iter()).copied().collect(),
        }
    }

    #[test]
    fn portals_hide_cells_out_of_view() {
        // Sample pack batches: x = 100 (cell 0), 200 (cell 1), 300 (cell 1), -100 (cell 0, behind the camera).
        let bytes = sample_pack();
        let cam = Camera::new([0.0; 3], 0.0, 0.0);
        for (y0, drawn, cells) in [(-5.0, 3, 2), (400.0, 1, 1)] {
            let vis = VisData {
                vis: two_cells(y0),
                batch_cells: vec![0, 1, 1, 0],
                instance_cells: Vec::new(),
            };
            let mut gfx = Rec::default();
            let mut world = World::load(&bytes, Some(&vis), &mut gfx).unwrap();
            assert!(world.has_cells());
            let stats = world.draw(&mut gfx, &cam, &mut scratch());
            assert!(stats.in_cell);
            assert_eq!(
                (stats.batches, stats.cells_visible),
                (drawn, cells),
                "portal at y {y0}"
            );
        }
    }

    /// Walks the cells of a locally converted `out/packs/mp_terminal.wpk` (never in git; skipped when absent).
    #[test]
    fn cells_on_a_real_map() {
        extern crate std;
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../out/packs/mp_terminal.wpk"
        );
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local pack, skipped");
            return;
        };
        if let Err(wpk::ParseError::UnsupportedVersion(v)) = wpk::Pack::parse(&bytes, false) {
            std::println!("local pack is version {v}, re-run `mwf-convert pack`; skipped");
            return;
        }
        let vis = VisData::from_pack(&bytes).expect("pack has visibility");
        let mut gfx = Rec::default();
        let mut world = World::load(&bytes, Some(&vis), &mut gfx).unwrap();
        let mut sc = scratch();
        let (mut inside, mut partial, mut tris_all, mut tris_vis, mut model_vis) =
            (0, 0, 0u64, 0u64, 0u64);
        let spawns = world.spawns.clone();
        for s in &spawns {
            let cam = Camera::new([s.origin[0], s.origin[1], s.origin[2] + 60.0], s.yaw, 0.0);
            let st = world.draw(&mut gfx, &cam, &mut sc);
            inside += usize::from(st.in_cell);
            partial += usize::from(st.cells_visible < vis.vis.cells.len() as u32);
            tris_vis += u64::from(st.triangles);
            model_vis += u64::from(st.model_triangles);
            let saved = world.vis.take();
            tris_all += u64::from(world.draw(&mut gfx, &cam, &mut sc).triangles);
            world.vis = saved;
        }
        std::println!(
            "{} spawns: {inside} in a cell, {partial} see part of the map; triangles per view {} with cells ({} static              models) vs {} frustum only",
            spawns.len(),
            tris_vis / spawns.len() as u64,
            model_vis / spawns.len() as u64,
            tris_all / spawns.len() as u64
        );
        assert!(
            inside * 10 >= spawns.len() * 9,
            "most spawns are inside the cell BSP"
        );
        assert!(tris_vis <= tris_all);
    }

    fn at(x: f32, cull_dist: u16, light: u32) -> InstanceRecord {
        InstanceRecord {
            model: 0,
            cull_dist,
            light,
            mtx: [
                [1.0, 0.0, 0.0, x],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        }
    }

    #[test]
    fn draws_static_model_instances() {
        let v = WorldVertex {
            pos: [0.0; 3],
            rgba: 0xffffffff,
            uv: [0.0; 2],
            lm: [0; 2],
        };
        let bytes = sample_pack_with(|b| {
            b.section(tag::MODEL_VERTICES, 3, records::encode(&[v; 3]));
            // One opaque surface using the world's first display list (dl 0).
            b.section(
                tag::MODEL_SURFACES,
                1,
                records::encode(&[batch(0, 0, 0.0, 0)]),
            );
            b.section(
                tag::MODELS,
                1,
                records::encode(&[ModelRecord {
                    centre: [0.0; 3],
                    radius: 2.0,
                    lod_count: 1,
                    lods: [
                        wpk::records::ModelLod {
                            first_surface: 0,
                            surface_count: 1,
                            dist: 1000.0,
                        },
                        Default::default(),
                        Default::default(),
                        Default::default(),
                    ],
                }]),
            );
            b.section(
                tag::MODEL_INSTANCES,
                3,
                records::encode(&[
                    at(150.0, 0, 0x8080_80ff),  // visible
                    at(150.0, 50, 0x8080_80ff), // past its draw distance
                    at(-500.0, 0, 0x8080_80ff), // behind the camera
                ]),
            );
        });
        let pack = Pack::parse(&bytes, false).unwrap();
        let mut gfx = Rec {
            base: pack.section(tag::MODEL_VERTICES).unwrap().data.as_ptr() as usize,
            dl_base: pack
                .section(tag::WORLD_DISPLAY_LISTS)
                .unwrap()
                .data
                .as_ptr() as usize,
            ..Rec::default()
        };
        let mut world = World::load(&bytes, None, &mut gfx).unwrap();
        assert_eq!(world.instance_count(), 3);
        gfx.calls.clear();
        let stats = world.draw(&mut gfx, &Camera::new([0.0; 3], 0.0, 0.0), &mut scratch());
        assert_eq!((stats.instances, stats.instances_culled), (1, 2));
        let calls = &gfx.calls;
        let mtx: Vec<&Call> = calls.iter().filter(|c| matches!(c, Call::Mtx(_))).collect();
        // The instance (150 units ahead), then the view again for the blended world batches.
        assert_eq!(mtx, [&Call::Mtx(-150), &Call::Mtx(0)]);
        let i = calls.iter().position(|c| *c == Call::Mtx(-150)).unwrap();
        assert_eq!(calls[i + 1], Call::Arrays(0));
        match calls[i + 2] {
            Call::State(s) => assert_eq!((s.tint, s.lightmap), (0x8080_80ff, -1)),
            ref c => panic!("{c:?}"),
        }
        assert_eq!(calls[i + 3], Call::Dl(0));
    }

    #[test]
    fn rejects_bad_batches() {
        let mut bytes = sample_pack();
        // Corrupt the first batch's display-list offset to a non-aligned value and fix the hash.
        let pack = Pack::parse(&bytes, false).unwrap();
        let at = pack.section_offset(tag::WORLD_BATCHES).unwrap() + 12;
        wpk::be::put_u32(&mut bytes, at, 5);
        let hash = wpk::fnv1a64(&bytes[wpk::HEADER_LEN..]);
        wpk::be::put_u64(&mut bytes, 16, hash);
        assert_eq!(
            World::load(&bytes, None, &mut Rec::default()).err(),
            Some(LoadError::BadRecord(tag::WORLD_BATCHES, 0))
        );
    }
}

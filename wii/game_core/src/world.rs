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
    pub state_changes: u32,
    pub instances: u32,
    pub instances_culled: u32,
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
    model_surfaces: Vec<BatchRecord>,
    models: Vec<ModelRecord>,
    instances: Vec<InstanceRecord>,
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

impl<'a> World<'a> {
    /// Parse a pack held in memory for the program's lifetime and create its textures.
    pub fn load(bytes: &'a [u8], gfx: &mut impl Gfx) -> Result<World<'a>, LoadError> {
        let pack = Pack::parse(bytes, true).map_err(LoadError::Pack)?;
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
        for (i, b) in batches.iter().enumerate() {
            let vend =
                (b.vertex_base as usize + b.vertex_count as usize) * records::WORLD_VERTEX_LEN;
            if vend > vertices.len()
                || (b.dl_off + b.dl_len) as usize > dls.len()
                || b.dl_off % 32 != 0
                || b.dl_len % 32 != 0
                || usize::from(b.material) >= materials.len()
            {
                return Err(LoadError::BadRecord(tag::WORLD_BATCHES, i));
            }
        }
        // Static models (optional sections; older packs have none).
        let opt = |t: [u8; 4]| pack.section(t).map(|s| s.data).unwrap_or(&[]);
        let model_vertices = opt(tag::MODEL_VERTICES);
        let model_surfaces: Vec<BatchRecord> = records::iter(opt(tag::MODEL_SURFACES)).collect();
        let models: Vec<ModelRecord> = records::iter(opt(tag::MODELS)).collect();
        let instances: Vec<InstanceRecord> = records::iter(opt(tag::MODEL_INSTANCES)).collect();
        for (i, b) in model_surfaces.iter().enumerate() {
            let vend =
                (b.vertex_base as usize + b.vertex_count as usize) * records::WORLD_VERTEX_LEN;
            if vend > model_vertices.len()
                || (b.dl_off + b.dl_len) as usize > dls.len()
                || b.dl_off % 32 != 0
                || b.dl_len % 32 != 0
                || usize::from(b.material) >= materials.len()
            {
                return Err(LoadError::BadRecord(tag::MODEL_SURFACES, i));
            }
        }
        for (i, m) in models.iter().enumerate() {
            if m.first_surface as usize + m.surface_count as usize > model_surfaces.len() {
                return Err(LoadError::BadRecord(tag::MODELS, i));
            }
        }
        if let Some(i) = instances
            .iter()
            .position(|n| usize::from(n.model) >= models.len())
        {
            return Err(LoadError::BadRecord(tag::MODEL_INSTANCES, i));
        }
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
            model_draws: Vec::with_capacity(model_surfaces.len()),
            model_surfaces,
            models,
            instances,
        })
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
    pub fn draw(&mut self, gfx: &mut impl Gfx, cam: &Camera) -> FrameStats {
        let mut stats = FrameStats::default();
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
            if !cam.sees_box(b.bounds_min, b.bounds_max) {
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
            let model = &self.models[usize::from(inst.model)];
            let centre = place(&inst.mtx, model.centre);
            let d = sub(centre, cam.eye);
            if inst.cull_dist != 0 {
                let far = f32::from(inst.cull_dist);
                if dot(d, d) > far * far {
                    stats.instances_culled += 1;
                    continue;
                }
            }
            let r = model.radius * max_scale(&inst.mtx);
            let lo = [centre[0] - r, centre[1] - r, centre[2] - r];
            let hi = [centre[0] + r, centre[1] + r, centre[2] + r];
            if !cam.sees_box(lo, hi) {
                stats.instances_culled += 1;
                continue;
            }
            stats.instances += 1;
            let depth = dot(d, fwd);
            for si in model.first_surface..model.first_surface + model.surface_count {
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
        // Bound vertex block: (model vertices?, base). Loaded matrix: (instance or u32::MAX for the view, polygon
        // offset level); the view alone is loaded by frame_begin.
        let mut last_base = (false, u32::MAX);
        let mut last_mtx = (u32::MAX, 0u8);
        for &(_, _, d) in &self.order {
            let (b, verts, inst) = if d & MODEL_DRAW != 0 {
                let (ii, si) = self.model_draws[(d & !MODEL_DRAW) as usize];
                (
                    &self.model_surfaces[si as usize],
                    self.model_vertices,
                    Some(ii),
                )
            } else {
                (&self.batches[d as usize], self.vertices, None)
            };
            let offset =
                self.materials[usize::from(b.material)].flags & material_flags::POLYGON_OFFSET;
            let want = (inst.map_or(u32::MAX, u32::from), offset);
            if want != last_mtx {
                last_mtx = want;
                let mut m = match inst {
                    Some(ii) => mul34(&view, &self.instances[usize::from(ii)].mtx),
                    None => view,
                };
                // Decals: pull towards the eye along each view ray (same pixels, smaller depth).
                let pull = material_flags::depth_pull(offset);
                m.iter_mut().flatten().for_each(|v| *v *= pull);
                gfx.pos_matrix(&m);
            }
            if (inst.is_some(), b.vertex_base) != last_base {
                last_base = (inst.is_some(), b.vertex_base);
                let off = b.vertex_base as usize * records::WORLD_VERTEX_LEN;
                gfx.world_arrays(verts[off..].as_ptr());
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
        fn world_arrays(&mut self, v: *const u8) {
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
        let mut world = World::load(&bytes, &mut gfx).unwrap();
        assert_eq!(world.name, "mp_test");
        assert_eq!(gfx.calls, [Call::Tex(8, 8, true)]);
        gfx.calls.clear();

        let cam = Camera::new([0.0; 3], 0.0, 0.0);
        let stats = world.draw(&mut gfx, &cam);
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
                    first_surface: 0,
                    surface_count: 1,
                    centre: [0.0; 3],
                    radius: 2.0,
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
        let mut world = World::load(&bytes, &mut gfx).unwrap();
        assert_eq!(world.instance_count(), 3);
        gfx.calls.clear();
        let stats = world.draw(&mut gfx, &Camera::new([0.0; 3], 0.0, 0.0));
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
            World::load(&bytes, &mut Rec::default()).err(),
            Some(LoadError::BadRecord(tag::WORLD_BATCHES, 0))
        );
    }
}

//! A loaded map pack and its per-frame draw.
//!
//! Order per frame (matching the converter's reference renderer): skybox, then visible opaque and alpha-tested
//! batches (grouped by material and lightmap to cut state changes), then blended / additive / multiply batches back
//! to front. Batches whose bounds are outside the view are skipped.

use alloc::vec::Vec;

use wpk::records::{
    self, BatchRecord, Blend, MaterialRecord, NONE, Skybox, SpawnRecord, TextureRecord,
    texture_flags,
};
use wpk::{Pack, ParseError, tag};

use crate::camera::{Camera, ZFAR, ZNEAR};
use crate::gfx::{DrawState, Gfx, TexDesc};
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
    /// Scratch for the frame's draw order.
    order: Vec<(u8, i32, u16)>,
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
        })
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
        }
    }

    /// Draw one frame from `cam` (frame begin/end included).
    pub fn draw(&mut self, gfx: &mut impl Gfx, cam: &Camera) -> FrameStats {
        let mut stats = FrameStats::default();
        gfx.frame_begin(&cam.view(), cam.fovy, ZNEAR, ZFAR, CLEAR);
        if let Some(faces) = self.sky {
            let rot = cam.view_rotation();
            for (f, &t) in faces.iter().enumerate() {
                gfx.sky_face(&rot, &sky::face_quad(f, 100.0), t);
            }
        }

        // Visible batches: (class, sort key, index). Class 0 opaque/alpha test, 1 blended (sorted far to near).
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
                self.order.push((1, -(cam.depth_of(c) as i32), i as u16));
            } else {
                let key = (i32::from(b.material) << 16) | i32::from(b.lightmap);
                self.order.push((0, key, i as u16));
            }
        }
        self.order.sort_unstable();

        let mut last_state: Option<DrawState> = None;
        let mut last_base = u32::MAX;
        for &(_, _, i) in &self.order {
            let b = &self.batches[usize::from(i)];
            if b.vertex_base != last_base {
                last_base = b.vertex_base;
                let off = b.vertex_base as usize * records::WORLD_VERTEX_LEN;
                gfx.world_arrays(self.vertices[off..].as_ptr());
            }
            let s = self.state(b);
            if last_state != Some(s) {
                gfx.world_state(&s);
                last_state = Some(s);
                stats.state_changes += 1;
            }
            gfx.call_dl(&self.dls[b.dl_off as usize..(b.dl_off + b.dl_len) as usize]);
            stats.batches += 1;
            stats.triangles += b.tri_count;
        }
        gfx.frame_end();
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
            self.calls.push(Call::Arrays(v as usize - self.base));
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
            Call::End,
        ]);
        assert_eq!(gfx.calls, expect);
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

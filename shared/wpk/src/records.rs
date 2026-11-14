//! Fixed-size records stored in pack sections. Each has a byte length, `write` (append big-endian) and `read`.
//! The Wii runtime reads them in place; the converter writes them.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// A fixed-size record.
pub trait Record: Sized {
    const LEN: usize;
    fn write(&self, out: &mut Vec<u8>);
    /// Read the record starting at `b[0]`. `b` must hold at least `LEN` bytes.
    fn read(b: &[u8]) -> Self;
}

/// Records `0..n` of a section's data (stops early if the data is short).
pub fn iter<'a, R: Record + 'a>(data: &'a [u8]) -> impl Iterator<Item = R> + 'a {
    data.chunks_exact(R::LEN).map(R::read)
}

/// Record `i` of a section's data.
pub fn get<R: Record>(data: &[u8], i: usize) -> Option<R> {
    data.get(i * R::LEN..(i + 1) * R::LEN).map(R::read)
}

/// Write every record into one section buffer.
pub fn encode<R: Record>(records: &[R]) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.len() * R::LEN);
    for r in records {
        r.write(&mut out);
    }
    out
}

/// "No texture" / "no lightmap".
pub const NONE: u16 = 0xFFFF;

// ---- textures ------------------------------------------------------------------------------------------------------

pub mod material_flags {
    /// IW4 polygon offset level (0 none, 1..3): decals that lie on other surfaces. The renderer pulls them towards
    /// the eye by [`depth_pull`] so they win the depth test without moving on screen.
    pub const POLYGON_OFFSET: u8 = 3;

    /// Eye-space scale for polygon offset `level`: positions are multiplied by this (1 for level 0).
    pub fn depth_pull(level: u8) -> f32 {
        1.0 - f32::from(level & POLYGON_OFFSET) / 2048.0
    }
}

pub mod texture_flags {
    /// An I4 alpha plane follows (CMPR colour + I4 alpha; see `gx_texture::policy::Converted`).
    pub const ALPHA_PLANE: u8 = 1;
}

/// Texture wrap mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Wrap {
    Repeat = 0,
    Clamp = 1,
    Mirror = 2,
}

/// One GX texture: data and (optional) alpha plane in the `TXDA` section, 32-byte aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureRecord {
    /// GX texture format id (`gx_texture::TexFormat::gx_id`).
    pub format: u8,
    pub levels: u8,
    pub flags: u8,
    pub wrap: u8,
    pub width: u16,
    pub height: u16,
    /// Offset and length in `TXDA`.
    pub data_off: u32,
    pub data_len: u32,
    /// I4 alpha plane in `TXDA` (when `flags & ALPHA_PLANE`), same size and levels.
    pub alpha_off: u32,
    pub alpha_len: u32,
}

impl Record for TextureRecord {
    const LEN: usize = 32;
    fn write(&self, o: &mut Vec<u8>) {
        o.u8(self.format);
        o.u8(self.levels);
        o.u8(self.flags);
        o.u8(self.wrap);
        o.u16(self.width);
        o.u16(self.height);
        o.u32(self.data_off);
        o.u32(self.data_len);
        o.u32(self.alpha_off);
        o.u32(self.alpha_len);
        o.zeros(8);
    }
    fn read(b: &[u8]) -> Self {
        TextureRecord {
            format: b[0],
            levels: b[1],
            flags: b[2],
            wrap: b[3],
            width: be::u16(b, 4),
            height: be::u16(b, 6),
            data_off: be::u32(b, 8),
            data_len: be::u32(b, 12),
            alpha_off: be::u32(b, 16),
            alpha_len: be::u32(b, 20),
        }
    }
}

// ---- materials -----------------------------------------------------------------------------------------------------

/// TEV recipe: how a material's stages combine (see `docs/wii-gx.md`). The colour texture's alpha plane, if it has one,
/// adds a stage that takes alpha from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Recipe {
    /// colour texture × vertex colour.
    Unlit = 0,
    /// colour texture × lightmap × 2 (world surfaces).
    Lightmapped = 1,
    /// colour texture × a constant (instance) light colour (static models).
    ConstLit = 2,
    /// colour texture as is (sky, effects).
    Texture = 3,
}

impl Recipe {
    pub fn from_u8(v: u8) -> Recipe {
        match v {
            1 => Recipe::Lightmapped,
            2 => Recipe::ConstLit,
            3 => Recipe::Texture,
            _ => Recipe::Unlit,
        }
    }
}

/// Framebuffer blending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Blend {
    Opaque = 0,
    /// Alpha test at `alpha_ref`; drawn with the opaque pass (Z after texturing).
    AlphaTest = 1,
    /// src·a + dst·(1−a), drawn after opaque, no Z write.
    Blend = 2,
    /// src + dst.
    Additive = 3,
    /// src·dst.
    Multiply = 4,
}

impl Blend {
    pub fn from_u8(v: u8) -> Blend {
        match v {
            1 => Blend::AlphaTest,
            2 => Blend::Blend,
            3 => Blend::Additive,
            4 => Blend::Multiply,
            _ => Blend::Opaque,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cull {
    None = 0,
    Back = 1,
    Front = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialRecord {
    pub recipe: u8,
    pub blend: u8,
    pub alpha_ref: u8,
    pub cull: u8,
    /// Colour texture index into `TEXT`, or [`NONE`].
    pub colour: u16,
    /// IW4 sort key (draw order within a blend class).
    pub sort: u8,
    pub flags: u8,
}

impl Record for MaterialRecord {
    const LEN: usize = 16;
    fn write(&self, o: &mut Vec<u8>) {
        o.u8(self.recipe);
        o.u8(self.blend);
        o.u8(self.alpha_ref);
        o.u8(self.cull);
        o.u16(self.colour);
        o.u8(self.sort);
        o.u8(self.flags);
        o.zeros(8);
    }
    fn read(b: &[u8]) -> Self {
        MaterialRecord {
            recipe: b[0],
            blend: b[1],
            alpha_ref: b[2],
            cull: b[3],
            colour: be::u16(b, 4),
            sort: b[6],
            flags: b[7],
        }
    }
}

// ---- world ---------------------------------------------------------------------------------------------------------

/// Bytes per interleaved world vertex: GX arrays for POS, CLR0, TEX0 and TEX1 all point into it with this stride.
pub const WORLD_VERTEX_LEN: usize = 28;
/// Offsets of each attribute within a world vertex.
pub const WV_POS: usize = 0;
pub const WV_CLR: usize = 12;
pub const WV_UV: usize = 16;
pub const WV_LM: usize = 24;
/// Lightmap texcoords are u16 with this many fractional bits (`GX_SetVtxAttrFmt(.., GX_VA_TEX1, GX_TEX_ST, GX_U16, 15)`).
pub const LM_FRAC: u32 = 15;

/// A world vertex: position f32×3, RGBA8 colour, UV f32×2, lightmap UV u16×2 (frac [`LM_FRAC`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    pub rgba: u32,
    pub uv: [f32; 2],
    pub lm: [u16; 2],
}

impl WorldVertex {
    /// Lightmap UV in [0, 2) to fixed point.
    pub fn lm_fixed(u: f32) -> u16 {
        let scaled = u * (1u32 << LM_FRAC) as f32;
        if scaled.is_nan() || scaled <= 0.0 {
            0
        } else if scaled >= 65535.0 {
            65535
        } else {
            (scaled + 0.5) as u16
        }
    }
}

impl Record for WorldVertex {
    const LEN: usize = WORLD_VERTEX_LEN;
    fn write(&self, o: &mut Vec<u8>) {
        for v in self.pos {
            o.f32(v);
        }
        o.u32(self.rgba);
        o.f32(self.uv[0]);
        o.f32(self.uv[1]);
        o.u16(self.lm[0]);
        o.u16(self.lm[1]);
    }
    fn read(b: &[u8]) -> Self {
        WorldVertex {
            pos: [be::f32(b, 0), be::f32(b, 4), be::f32(b, 8)],
            rgba: be::u32(b, 12),
            uv: [be::f32(b, 16), be::f32(b, 20)],
            lm: [be::u16(b, 24), be::u16(b, 26)],
        }
    }
}

/// One world draw: a display list drawing triangles from the vertices starting at `vertex_base` (GX arrays point
/// there; display-list indices are u16 relative to it), with one material and one lightmap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchRecord {
    pub vertex_base: u32,
    pub vertex_count: u32,
    pub material: u16,
    /// Lightmap texture index into `TEXT`, or [`NONE`].
    pub lightmap: u16,
    /// Offset and length in `WDLS` (32-byte aligned, padded).
    pub dl_off: u32,
    pub dl_len: u32,
    pub tri_count: u32,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

impl Record for BatchRecord {
    const LEN: usize = 48;
    fn write(&self, o: &mut Vec<u8>) {
        o.u32(self.vertex_base);
        o.u32(self.vertex_count);
        o.u16(self.material);
        o.u16(self.lightmap);
        o.u32(self.dl_off);
        o.u32(self.dl_len);
        o.u32(self.tri_count);
        for v in self.bounds_min.into_iter().chain(self.bounds_max) {
            o.f32(v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let f = |i: usize| be::f32(b, 24 + 4 * i);
        BatchRecord {
            vertex_base: be::u32(b, 0),
            vertex_count: be::u32(b, 4),
            material: be::u16(b, 8),
            lightmap: be::u16(b, 10),
            dl_off: be::u32(b, 12),
            dl_len: be::u32(b, 16),
            tri_count: be::u32(b, 20),
            bounds_min: [f(0), f(1), f(2)],
            bounds_max: [f(3), f(4), f(5)],
        }
    }
}

/// Display-list opcode for indexed triangles in vertex format `vtxfmt` (`GX_TRIANGLES | fmt`).
pub const fn gx_triangles(vtxfmt: u8) -> u8 {
    0x90 | (vtxfmt & 7)
}

/// The vertex format slot world batches use, and the attribute layout of their display lists: per vertex, a u16
/// index for each of POS, CLR0, TEX0, TEX1 (all the same index, into the interleaved world vertices).
pub const WORLD_VTXFMT: u8 = 0;
pub const WORLD_DL_ATTRS: usize = 4;

/// Build a world display list: `tris` are vertex indices relative to the batch's `vertex_base`. Returns the bytes,
/// padded with GX NOPs to 32 bytes. Splits into several draw commands when there are more than 65535 vertices.
pub fn world_display_list(tris: &[u16]) -> Vec<u8> {
    world_display_list_fmt(tris, WORLD_VTXFMT)
}

/// [`world_display_list`] in vertex format `vtxfmt` (the float world format or a compact one, `crate::quant`).
pub fn world_display_list_fmt(tris: &[u16], vtxfmt: u8) -> Vec<u8> {
    let mut dl = Vec::with_capacity(tris.len() * 2 * WORLD_DL_ATTRS + 32);
    // Largest multiple of 3 that fits the u16 vertex count.
    const MAX: usize = 65535 / 3 * 3;
    for chunk in tris.chunks(MAX) {
        dl.u8(gx_triangles(vtxfmt));
        dl.u16(chunk.len() as u16);
        for &i in chunk {
            for _ in 0..WORLD_DL_ATTRS {
                dl.u16(i);
            }
        }
    }
    let padded = (dl.len() + 31) & !31;
    dl.resize(padded, 0);
    dl
}

// ---- sky and spawns ------------------------------------------------------------------------------------------------

/// Six sky face texture indices in D3D cube order (+X, −X, +Y, −Y, +Z, −Z).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Skybox {
    pub faces: [u16; 6],
}

impl Record for Skybox {
    const LEN: usize = 16;
    fn write(&self, o: &mut Vec<u8>) {
        for f in self.faces {
            o.u16(f);
        }
        o.zeros(4);
    }
    fn read(b: &[u8]) -> Self {
        let mut faces = [0u16; 6];
        for (i, f) in faces.iter_mut().enumerate() {
            *f = be::u16(b, 2 * i);
        }
        Skybox { faces }
    }
}

/// The HUD minimap (`MINI`): its texture, the map's northwest and southeast corners in world units (MW2's
/// `setupMiniMap` frame from the two `minimap_corner` entities, `hud_iw4::compass::setup_mini_map_frame`), the map's
/// north yaw, and how many world units the minimap's height covers (the map script's `compassmaxrange`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Minimap {
    pub texture: u16,
    pub northwest: [f32; 2],
    pub southeast: [f32; 2],
    pub north_yaw: f32,
    pub max_range: f32,
}

impl Record for Minimap {
    const LEN: usize = 28;
    fn write(&self, o: &mut Vec<u8>) {
        o.u16(self.texture);
        o.zeros(2);
        for v in [
            self.northwest[0],
            self.northwest[1],
            self.southeast[0],
            self.southeast[1],
            self.north_yaw,
            self.max_range,
        ] {
            o.f32(v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let f = |i: usize| be::f32(b, 4 + 4 * i);
        Minimap {
            texture: be::u16(b, 0),
            northwest: [f(0), f(1)],
            southeast: [f(2), f(3)],
            north_yaw: f(4),
            max_range: f(5),
        }
    }
}

pub mod spawn_kind {
    pub const TDM: u8 = 0;
    pub const DM: u8 = 1;
    pub const OTHER: u8 = 2;
}

pub mod spawn_team {
    pub const ANY: u8 = 0;
    pub const ALLIES: u8 = 1;
    pub const AXIS: u8 = 2;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnRecord {
    pub origin: [f32; 3],
    /// Degrees, IW4 convention (0 = +X, counter-clockwise seen from above).
    pub yaw: f32,
    pub kind: u8,
    pub team: u8,
    /// Bit 0: initial (match start) spawn.
    pub flags: u8,
}

impl Record for SpawnRecord {
    const LEN: usize = 24;
    fn write(&self, o: &mut Vec<u8>) {
        for v in self.origin {
            o.f32(v);
        }
        o.f32(self.yaw);
        o.u8(self.kind);
        o.u8(self.team);
        o.u8(self.flags);
        o.zeros(5);
    }
    fn read(b: &[u8]) -> Self {
        SpawnRecord {
            origin: [be::f32(b, 0), be::f32(b, 4), be::f32(b, 8)],
            yaw: be::f32(b, 12),
            kind: b[16],
            team: b[17],
            flags: b[18],
        }
    }
}

/// One level of detail of a static model: a run of `MSRF` surfaces, used while the camera distance (divided by the
/// placement scale) is below `dist` (IW4 `lodDist`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelLod {
    pub first_surface: u32,
    pub surface_count: u16,
    pub dist: f32,
}

/// A static model: up to four LODs (BatchRecords over `MVTX`, display lists in `WDLS`, bounds in model space) and a
/// bounding sphere in model space. Past the last LOD's distance the model is not drawn (as in IW4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelRecord {
    pub centre: [f32; 3],
    pub radius: f32,
    pub lod_count: u8,
    pub lods: [ModelLod; 4],
}

impl ModelRecord {
    /// The LOD to draw at `dist` (camera distance / placement scale), or None beyond the last.
    pub fn lod_for(&self, dist: f32) -> Option<&ModelLod> {
        self.lods[..usize::from(self.lod_count.min(4))]
            .iter()
            .find(|l| dist < l.dist || l.dist <= 0.0)
    }
}

impl Record for ModelRecord {
    const LEN: usize = 68;
    fn write(&self, o: &mut Vec<u8>) {
        for v in self.centre {
            o.f32(v);
        }
        o.f32(self.radius);
        o.u8(self.lod_count);
        o.zeros(3);
        for l in &self.lods {
            o.u32(l.first_surface);
            o.u16(l.surface_count);
            o.u16(0);
            o.f32(l.dist);
        }
    }
    fn read(b: &[u8]) -> Self {
        ModelRecord {
            centre: [be::f32(b, 0), be::f32(b, 4), be::f32(b, 8)],
            radius: be::f32(b, 12),
            lod_count: b[16],
            lods: core::array::from_fn(|i| {
                let at = 20 + i * 12;
                ModelLod {
                    first_surface: be::u32(b, at),
                    surface_count: be::u16(b, at + 4),
                    dist: be::f32(b, at + 8),
                }
            }),
        }
    }
}

/// One placed static model: model-to-world matrix (rows; axis × scale and origin), the light-grid colour it is lit
/// with (RGBA8, half intensity: the GPU doubles it, like lightmaps) and its draw distance (0 = always).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceRecord {
    pub model: u16,
    pub cull_dist: u16,
    pub light: u32,
    pub mtx: [[f32; 4]; 3],
}

impl Record for InstanceRecord {
    const LEN: usize = 56;
    fn write(&self, o: &mut Vec<u8>) {
        o.u16(self.model);
        o.u16(self.cull_dist);
        o.u32(self.light);
        for v in self.mtx.iter().flatten() {
            o.f32(*v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let f = |i: usize| be::f32(b, 8 + 4 * i);
        InstanceRecord {
            model: be::u16(b, 0),
            cull_dist: be::u16(b, 2),
            light: be::u32(b, 4),
            mtx: [
                [f(0), f(1), f(2), f(3)],
                [f(4), f(5), f(6), f(7)],
                [f(8), f(9), f(10), f(11)],
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn roundtrip<R: Record + PartialEq + core::fmt::Debug>(r: R) {
        let mut b = Vec::new();
        r.write(&mut b);
        assert_eq!(b.len(), R::LEN);
        assert_eq!(R::read(&b), r);
    }

    #[test]
    fn records_roundtrip_at_their_length() {
        roundtrip(ModelRecord {
            centre: [1.0, -2.0, 3.5],
            radius: 40.0,
            lod_count: 2,
            lods: [
                ModelLod {
                    first_surface: 3,
                    surface_count: 2,
                    dist: 500.0,
                },
                ModelLod {
                    first_surface: 5,
                    surface_count: 1,
                    dist: 1500.0,
                },
                ModelLod::default(),
                ModelLod::default(),
            ],
        });
        roundtrip(InstanceRecord {
            model: 7,
            cull_dist: 2500,
            light: 0x8070_60ff,
            mtx: [
                [1.0, 0.0, 0.0, 10.0],
                [0.0, 1.0, 0.0, -20.0],
                [0.0, 0.0, 1.0, 30.0],
            ],
        });
        roundtrip(TextureRecord {
            format: 14,
            levels: 6,
            flags: texture_flags::ALPHA_PLANE,
            wrap: Wrap::Clamp as u8,
            width: 256,
            height: 128,
            data_off: 64,
            data_len: 1024,
            alpha_off: 2048,
            alpha_len: 1024,
        });
        roundtrip(MaterialRecord {
            recipe: Recipe::Lightmapped as u8,
            blend: Blend::AlphaTest as u8,
            alpha_ref: 128,
            cull: Cull::Back as u8,
            colour: 7,
            sort: 3,
            flags: 0,
        });
        roundtrip(WorldVertex {
            pos: [1.5, -2.0, 3.25],
            rgba: 0x11223344,
            uv: [0.5, 12.0],
            lm: [100, 65535],
        });
        roundtrip(BatchRecord {
            vertex_base: 1000,
            vertex_count: 300,
            material: 2,
            lightmap: NONE,
            dl_off: 64,
            dl_len: 96,
            tri_count: 100,
            bounds_min: [-1.0, -2.0, -3.0],
            bounds_max: [4.0, 5.0, 6.0],
        });
        roundtrip(Skybox {
            faces: [1, 2, 3, 4, 5, 6],
        });
        roundtrip(SpawnRecord {
            origin: [10.0, 20.0, 30.0],
            yaw: 90.0,
            kind: spawn_kind::TDM,
            team: spawn_team::AXIS,
            flags: 1,
        });
    }

    #[test]
    fn lightmap_fixed_point() {
        assert_eq!(WorldVertex::lm_fixed(0.0), 0);
        assert_eq!(WorldVertex::lm_fixed(0.5), 16384);
        assert_eq!(WorldVertex::lm_fixed(1.0), 32768);
        assert_eq!(WorldVertex::lm_fixed(-0.1), 0);
        assert_eq!(WorldVertex::lm_fixed(3.0), 65535);
        assert_eq!(WorldVertex::lm_fixed(f32::NAN), 0);
    }

    #[test]
    fn display_list_bytes() {
        let dl = world_display_list(&[0, 1, 2]);
        // 0x90, count 3, then 3 vertices x 4 attributes x u16, padded to 32.
        assert_eq!(&dl[..3], &[0x90, 0, 3]);
        assert_eq!(&dl[3..11], &[0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&dl[11..19], &[0, 1, 0, 1, 0, 1, 0, 1]);
        assert_eq!(dl.len(), 32);
        assert!(dl[27..].iter().all(|&b| b == 0));
        // Over 65535 vertices: two draw commands.
        let big = vec![0u16; 65535 / 3 * 3 + 3];
        let dl = world_display_list(&big);
        assert_eq!(dl[0], 0x90);
        let second = 3 + (65535 / 3 * 3) * 8;
        assert_eq!((dl[second], be::u16(&dl, second + 1)), (0x90, 3));
        assert_eq!(dl.len() % 32, 0);
    }

    #[test]
    fn iterate_section_records() {
        let spawns = [
            SpawnRecord {
                origin: [0.0; 3],
                yaw: 0.0,
                kind: 0,
                team: 0,
                flags: 0,
            },
            SpawnRecord {
                origin: [1.0; 3],
                yaw: 45.0,
                kind: 1,
                team: 2,
                flags: 1,
            },
        ];
        let data = encode(&spawns);
        let back: Vec<SpawnRecord> = iter(&data).collect();
        assert_eq!(back, spawns);
        assert_eq!(get::<SpawnRecord>(&data, 1), Some(spawns[1]));
        assert_eq!(get::<SpawnRecord>(&data, 2), None);
    }
}

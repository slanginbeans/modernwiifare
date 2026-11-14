//! `mwf-convert preview`: a software reference renderer for a `.wpk`.
//!
//! It reads only the pack, the way the Wii does: textures are decoded from their GX bytes, world draws come from the
//! display-list bytes, and shading follows the TEV recipes in `docs/wii-gx.md` (colour × lightmap × 2, alpha test,
//! blend, additive, multiply). The sky is the six skybox faces sampled as a D3D cube with the world-space view ray.
//! So it checks the converter's whole output without a Wii, and gives a reference image for the GX renderer.
//!
//! IW4 view convention: yaw 0 looks along +X, +Y is left, +Z is up. Horizontal field of view 65° (MW2's default).

use std::path::Path;

use gx_texture::Converted;
use gx_texture::format::TexFormat;
use gx_texture::image::Image;
use gx_texture::policy::{Alpha, Traits};
use wpk::quant::{CompactVertex, QuantRecord, dequantize};
use wpk::records::Record;
use wpk::records::{
    self, BatchRecord, Blend, InstanceRecord, MaterialRecord, ModelRecord, NONE, Recipe, Skybox,
    SpawnRecord, TextureRecord, WORLD_VERTEX_LEN, WorldVertex, texture_flags,
};
use wpk::{Pack, tag};

pub struct View {
    pub eye: [f32; 3],
    /// Degrees.
    pub yaw: f32,
    pub pitch: f32,
    pub width: u32,
    pub height: u32,
    pub hfov_deg: f32,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub batches_drawn: usize,
    pub batches_culled: usize,
    /// Static model instances beyond their draw distance.
    pub models_culled: usize,
    pub triangles: u64,
    pub pixels_drawn: u64,
}

fn decode_texture(rec: &TextureRecord, txda: &[u8]) -> Option<Image> {
    let format = TexFormat::from_gx_id(rec.format)?;
    let data = txda.get(rec.data_off as usize..(rec.data_off + rec.data_len) as usize)?;
    let alpha_plane = if rec.flags & texture_flags::ALPHA_PLANE != 0 {
        Some(
            txda.get(rec.alpha_off as usize..(rec.alpha_off + rec.alpha_len) as usize)?
                .to_vec(),
        )
    } else {
        None
    };
    let c = Converted {
        format,
        width: u32::from(rec.width),
        height: u32::from(rec.height),
        levels: u32::from(rec.levels),
        traits: Traits {
            grey: false,
            alpha: Alpha::Opaque,
        },
        data: data.to_vec(),
        alpha_plane,
    };
    c.decode_level(0)
}

/// Bilinear sample with repeat (or clamp) addressing; returns RGBA in 0..=1.
fn sample(img: &Image, u: f32, v: f32, clamp: bool) -> [f32; 4] {
    let (w, h) = (img.width as f32, img.height as f32);
    let (x, y) = (u * w - 0.5, v * h - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f32, yi: f32| {
        let (xi, yi) = if clamp {
            (xi.clamp(0.0, w - 1.0), yi.clamp(0.0, h - 1.0))
        } else {
            (xi.rem_euclid(w), yi.rem_euclid(h))
        };
        let p = img.pixels[yi as usize * img.width as usize + xi as usize];
        [
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
            p[3] as f32 / 255.0,
        ]
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1.0, y0),
        at(x0, y0 + 1.0),
        at(x0 + 1.0, y0 + 1.0),
    );
    let mut o = [0.0; 4];
    for k in 0..4 {
        o[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    o
}

/// D3D cube lookup: direction → (face, s, t). Faces in `CUBE_FACES` order (+X, −X, +Y, −Y, +Z, −Z).
pub fn cube_face(d: [f32; 3]) -> (usize, f32, f32) {
    let [x, y, z] = d;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    let (face, sc, tc, ma) = if ax >= ay && ax >= az {
        if x > 0.0 {
            (0, -z, -y, ax)
        } else {
            (1, z, -y, ax)
        }
    } else if ay >= az {
        if y > 0.0 {
            (2, x, z, ay)
        } else {
            (3, x, -z, ay)
        }
    } else if z > 0.0 {
        (4, x, -y, az)
    } else {
        (5, -x, -y, az)
    };
    (face, (sc / ma + 1.0) * 0.5, (tc / ma + 1.0) * 0.5)
}

struct Camera {
    eye: [f32; 3],
    fwd: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    focal: f32,
    cx: f32,
    cy: f32,
}

const NEAR: f32 = 4.0;

impl Camera {
    fn new(v: &View) -> Camera {
        let (y, p) = (v.yaw.to_radians(), v.pitch.to_radians());
        let fwd = [y.cos() * p.cos(), y.sin() * p.cos(), -p.sin()];
        let right = [y.sin(), -y.cos(), 0.0];
        let up = cross(right, fwd);
        Camera {
            eye: v.eye,
            fwd,
            right,
            up,
            focal: (v.width as f32 / 2.0) / (v.hfov_deg.to_radians() / 2.0).tan(),
            cx: v.width as f32 / 2.0,
            cy: v.height as f32 / 2.0,
        }
    }
    /// World → camera space (x right, y up, z depth).
    fn to_cam(&self, p: [f32; 3]) -> [f32; 3] {
        let d = sub(p, self.eye);
        [dot(d, self.right), dot(d, self.up), dot(d, self.fwd)]
    }
    /// Whether an AABB can be visible (conservative: in front of the near plane and inside the side planes).
    fn sees_box(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        let corners = (0..8).map(|i| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        });
        let cs: Vec<[f32; 3]> = corners.map(|c| self.to_cam(c)).collect();
        if cs.iter().all(|c| c[2] < NEAR) {
            return false;
        }
        let (tx, ty) = (self.cx / self.focal, self.cy / self.focal);
        !(cs.iter().all(|c| c[0] > c[2] * tx)
            || cs.iter().all(|c| c[0] < -c[2] * tx)
            || cs.iter().all(|c| c[1] > c[2] * ty)
            || cs.iter().all(|c| c[1] < -c[2] * ty))
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A clip-space vertex: camera position and attributes.
#[derive(Clone, Copy)]
struct CV {
    c: [f32; 3],
    uv: [f32; 2],
    lm: [f32; 2],
    rgba: [f32; 4],
}

fn lerp_cv(a: CV, b: CV, t: f32) -> CV {
    let l = |x: f32, y: f32| x + (y - x) * t;
    CV {
        c: [l(a.c[0], b.c[0]), l(a.c[1], b.c[1]), l(a.c[2], b.c[2])],
        uv: [l(a.uv[0], b.uv[0]), l(a.uv[1], b.uv[1])],
        lm: [l(a.lm[0], b.lm[0]), l(a.lm[1], b.lm[1])],
        rgba: [
            l(a.rgba[0], b.rgba[0]),
            l(a.rgba[1], b.rgba[1]),
            l(a.rgba[2], b.rgba[2]),
            l(a.rgba[3], b.rgba[3]),
        ],
    }
}

/// Clip a triangle against z = NEAR (Sutherland–Hodgman); returns a fan.
fn clip_near(tri: [CV; 3]) -> Vec<CV> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let (a, b) = (tri[i], tri[(i + 1) % 3]);
        let (ina, inb) = (a.c[2] >= NEAR, b.c[2] >= NEAR);
        if ina {
            out.push(a);
        }
        if ina != inb {
            out.push(lerp_cv(a, b, (NEAR - a.c[2]) / (b.c[2] - a.c[2])));
        }
    }
    out
}

/// Parse a world display list (triangle lists and strips) into vertex-index triangles in their GX winding
/// (relative to the batch's vertex base).
pub fn display_list_triangles(dl: &[u8]) -> Result<Vec<u16>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < dl.len() {
        match dl[i] {
            0x00 => i += 1,
            op if op & 0xF8 == 0x90 || op & 0xF8 == 0x98 => {
                let strip = op & 0xF8 == 0x98;
                let n = usize::from(u16::from_be_bytes([dl[i + 1], dl[i + 2]]));
                i += 3;
                let mut verts = Vec::with_capacity(n);
                for v in 0..n {
                    let at = i + v * 2 * records::WORLD_DL_ATTRS;
                    let idx = u16::from_be_bytes([dl[at], dl[at + 1]]);
                    // Every attribute index must be the same (interleaved vertices).
                    for a in 1..records::WORLD_DL_ATTRS {
                        if u16::from_be_bytes([dl[at + 2 * a], dl[at + 2 * a + 1]]) != idx {
                            return Err("display list attribute indices differ".into());
                        }
                    }
                    verts.push(idx);
                }
                if strip {
                    out.extend(wpk::strips::strip_triangles(&verts).flatten());
                } else {
                    out.extend_from_slice(&verts);
                }
                i += n * 2 * records::WORLD_DL_ATTRS;
            }
            op => return Err(format!("unexpected display-list opcode {op:#04x} at {i}")),
        }
    }
    Ok(out)
}

/// Render a view of a pack. Returns the RGB image (alpha 255) and stats.
pub fn render(bytes: &[u8], view: &View) -> Result<(Image, Stats), String> {
    let pack = Pack::parse(bytes, true).map_err(|e| format!("{e:?}"))?;
    let section = |t| pack.section(t).map(|s| s.data).unwrap_or(&[]);
    let txda = section(tag::TEXTURE_DATA);
    let textures: Vec<Option<Image>> = records::iter::<TextureRecord>(section(tag::TEXTURES))
        .map(|r| decode_texture(&r, txda))
        .collect();
    let wraps: Vec<bool> = records::iter::<TextureRecord>(section(tag::TEXTURES))
        .map(|r| r.wrap == records::Wrap::Clamp as u8)
        .collect();
    let materials: Vec<MaterialRecord> = records::iter(section(tag::MATERIALS)).collect();
    let vtx = section(tag::WORLD_VERTICES);
    let batches: Vec<BatchRecord> = records::iter(section(tag::WORLD_BATCHES)).collect();
    let dls = section(tag::WORLD_DISPLAY_LISTS);
    let sky: Option<Skybox> = records::get(section(tag::SKYBOX), 0);

    let (w, h) = (view.width as usize, view.height as usize);
    let cam = Camera::new(view);
    let mut colour = vec![[0.0f32; 3]; w * h];
    let mut depth = vec![f32::INFINITY; w * h];
    let mut stats = Stats::default();

    // Sky first: the view ray through each pixel samples the cube.
    if let Some(sky) = sky {
        let faces: Vec<Option<&Image>> = sky
            .faces
            .iter()
            .map(|&f| textures.get(usize::from(f)).and_then(Option::as_ref))
            .collect();
        for py in 0..h {
            for px in 0..w {
                let (x, y) = (
                    (px as f32 + 0.5 - cam.cx) / cam.focal,
                    (cam.cy - py as f32 - 0.5) / cam.focal,
                );
                let d = [
                    cam.fwd[0] + cam.right[0] * x + cam.up[0] * y,
                    cam.fwd[1] + cam.right[1] * x + cam.up[1] * y,
                    cam.fwd[2] + cam.right[2] * x + cam.up[2] * y,
                ];
                let (f, s, t) = cube_face(d);
                if let Some(img) = faces[f] {
                    let c = sample(img, s, t, true);
                    colour[py * w + px] = [c[0], c[1], c[2]];
                }
            }
        }
    }

    // Draw items: world batches, then each visible static model instance's surfaces (placed by its matrix, lit by its
    // light-grid colour). Opaque and alpha-tested first, then blended back to front, like the GX renderer.
    struct Item<'a> {
        b: BatchRecord,
        /// Float and compact vertex arrays; `quant` says which this batch uses.
        vtx: &'a [u8],
        cvtx: &'a [u8],
        quant: QuantRecord,
        mtx: Option<[[f32; 4]; 3]>,
        tint: Option<[f32; 3]>,
        min: [f32; 3],
        max: [f32; 3],
    }
    let float_quant = QuantRecord {
        vtxfmt: records::WORLD_VTXFMT,
        origin: [0.0; 3],
    };
    let wvtq = section(tag::WORLD_VERTICES_Q);
    let batch_quant: Vec<QuantRecord> = records::iter(section(tag::BATCH_QUANT)).collect();
    let mut items: Vec<Item> = batches
        .iter()
        .enumerate()
        .map(|(i, b)| Item {
            b: *b,
            vtx,
            cvtx: wvtq,
            quant: batch_quant.get(i).copied().unwrap_or(float_quant),
            mtx: None,
            tint: None,
            min: b.bounds_min,
            max: b.bounds_max,
        })
        .collect();
    let mvtx = section(tag::MODEL_VERTICES);
    let msrf: Vec<BatchRecord> = records::iter(section(tag::MODEL_SURFACES)).collect();
    let mvtq = section(tag::MODEL_VERTICES_Q);
    let surface_quant: Vec<QuantRecord> = records::iter(section(tag::SURFACE_QUANT)).collect();
    let models: Vec<ModelRecord> = records::iter(section(tag::MODELS)).collect();
    let place = |m: &[[f32; 4]; 3], p: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
    };
    let no_models = std::env::var_os("MWF_PREVIEW_NOMODELS").is_some();
    for inst in
        records::iter::<InstanceRecord>(section(tag::MODEL_INSTANCES)).filter(|_| !no_models)
    {
        let Some(model) = models.get(usize::from(inst.model)) else {
            continue;
        };
        let origin = [inst.mtx[0][3], inst.mtx[1][3], inst.mtx[2][3]];
        let d = sub(origin, cam.eye);
        if inst.cull_dist != 0 && dot(d, d) > f32::from(inst.cull_dist).powi(2) {
            stats.models_culled += 1;
            continue;
        }
        let l = inst.light.to_be_bytes();
        let tint = [0, 1, 2].map(|k| f32::from(l[k]) / 255.0 * 2.0);
        let scale = (0..3)
            .map(|c| {
                (0..3)
                    .map(|r| inst.mtx[r][c] * inst.mtx[r][c])
                    .sum::<f32>()
                    .sqrt()
            })
            .fold(0.0, f32::max);
        let centre = place(&inst.mtx, model.centre);
        let cd = sub(centre, cam.eye);
        let Some(lod) = model.lod_for(dot(cd, cd).sqrt() / scale.max(1e-6)) else {
            stats.models_culled += 1;
            continue;
        };
        let first = lod.first_surface as usize;
        for (si, b) in msrf
            .iter()
            .enumerate()
            .skip(first)
            .take(usize::from(lod.surface_count))
        {
            let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
            for c in 0..8 {
                let p = [0, 1, 2].map(|k| {
                    if c & (1 << k) != 0 {
                        b.bounds_max[k]
                    } else {
                        b.bounds_min[k]
                    }
                });
                let w = place(&inst.mtx, p);
                for k in 0..3 {
                    min[k] = min[k].min(w[k]);
                    max[k] = max[k].max(w[k]);
                }
            }
            items.push(Item {
                b: *b,
                vtx: mvtx,
                cvtx: mvtq,
                quant: surface_quant.get(si).copied().unwrap_or(float_quant),
                mtx: Some(inst.mtx),
                tint: Some(tint),
                min,
                max,
            });
        }
    }
    let class = |it: &Item| {
        materials
            .get(usize::from(it.b.material))
            .map_or(0, |m| u8::from(Blend::from_u8(m.blend) >= Blend::Blend))
    };
    let dist = |it: &Item| {
        let c = [0, 1, 2].map(|k| (it.min[k] + it.max[k]) / 2.0);
        dot(sub(c, cam.eye), cam.fwd)
    };
    items.sort_by(|a, b| {
        class(a).cmp(&class(b)).then(if class(a) == 1 {
            dist(b).total_cmp(&dist(a))
        } else {
            std::cmp::Ordering::Equal
        })
    });

    for it in &items {
        let b = &it.b;
        if !cam.sees_box(it.min, it.max) {
            stats.batches_culled += 1;
            continue;
        }
        stats.batches_drawn += 1;
        let Some(m) = materials.get(usize::from(b.material)) else {
            continue;
        };
        let mut blend = Blend::from_u8(m.blend);
        if std::env::var_os("MWF_PREVIEW_OPAQUE").is_some() {
            blend = Blend::Opaque;
        }
        let cull = if std::env::var_os("MWF_PREVIEW_NOCULL").is_some() {
            0
        } else {
            m.cull
        };
        let tex = (m.colour != NONE)
            .then(|| textures.get(usize::from(m.colour)))
            .flatten()
            .and_then(Option::as_ref);
        let tex_clamp =
            (m.colour != NONE) && wraps.get(usize::from(m.colour)).copied().unwrap_or(false);
        let lm = (b.lightmap != NONE)
            .then(|| textures.get(usize::from(b.lightmap)))
            .flatten()
            .and_then(Option::as_ref);
        let lit = Recipe::from_u8(m.recipe) == Recipe::Lightmapped && lm.is_some();
        let tint = it.tint;
        let pull = records::material_flags::depth_pull(m.flags);
        let dl = dls
            .get(b.dl_off as usize..(b.dl_off + b.dl_len) as usize)
            .ok_or("batch display list out of range")?;
        let tris = display_list_triangles(dl)?;
        for t in tris.chunks_exact(3) {
            let mut v = [CV {
                c: [0.0; 3],
                uv: [0.0; 2],
                lm: [0.0; 2],
                rgba: [0.0; 4],
            }; 3];
            for (k, &i) in t.iter().enumerate() {
                let vi = b.vertex_base as usize + usize::from(i);
                let wv = if it.quant.is_compact() {
                    let len = CompactVertex::LEN;
                    it.cvtx
                        .get(vi * len..(vi + 1) * len)
                        .map(|r| dequantize(&CompactVertex::read(r), &it.quant))
                } else {
                    it.vtx
                        .get(vi * WORLD_VERTEX_LEN..(vi + 1) * WORLD_VERTEX_LEN)
                        .and_then(|r| records::get::<WorldVertex>(r, 0))
                }
                .ok_or("vertex out of range")?;
                let c = wv.rgba.to_be_bytes();
                let scale = 1.0 / (1u32 << records::LM_FRAC) as f32;
                let pos = it.mtx.as_ref().map_or(wv.pos, |m| place(m, wv.pos));
                v[k] = CV {
                    c: cam.to_cam(pos).map(|x| x * pull),
                    uv: wv.uv,
                    lm: [f32::from(wv.lm[0]) * scale, f32::from(wv.lm[1]) * scale],
                    rgba: [
                        c[0] as f32 / 255.0,
                        c[1] as f32 / 255.0,
                        c[2] as f32 / 255.0,
                        c[3] as f32 / 255.0,
                    ],
                };
            }
            stats.triangles += 1;
            let poly = clip_near(v);
            for k in 1..poly.len().saturating_sub(1) {
                raster(
                    [poly[0], poly[k], poly[k + 1]],
                    &cam,
                    w,
                    h,
                    &mut colour,
                    &mut depth,
                    &mut stats,
                    |cv| {
                        let base = tex.map_or([1.0, 1.0, 1.0, 1.0], |img| {
                            sample(img, cv.uv[0], cv.uv[1], tex_clamp)
                        });
                        let light = if let Some(t) = tint {
                            [cv.rgba[0] * t[0], cv.rgba[1] * t[1], cv.rgba[2] * t[2]]
                        } else if lit {
                            let l = sample(lm.unwrap(), cv.lm[0], cv.lm[1], true);
                            [l[0] * 2.0, l[1] * 2.0, l[2] * 2.0]
                        } else {
                            [cv.rgba[0], cv.rgba[1], cv.rgba[2]]
                        };
                        [
                            (base[0] * light[0]).min(1.0),
                            (base[1] * light[1]).min(1.0),
                            (base[2] * light[2]).min(1.0),
                            base[3] * cv.rgba[3],
                        ]
                    },
                    blend,
                    m.alpha_ref,
                    cull,
                );
            }
        }
    }

    let pixels = colour
        .iter()
        .map(|c| {
            [
                (c[0] * 255.0 + 0.5) as u8,
                (c[1] * 255.0 + 0.5) as u8,
                (c[2] * 255.0 + 0.5) as u8,
                255,
            ]
        })
        .collect();
    Ok((
        Image {
            width: view.width,
            height: view.height,
            pixels,
        },
        stats,
    ))
}

#[allow(clippy::too_many_arguments)]
fn raster(
    tri: [CV; 3],
    cam: &Camera,
    w: usize,
    h: usize,
    colour: &mut [[f32; 3]],
    depth: &mut [f32],
    stats: &mut Stats,
    shade: impl Fn(&CV) -> [f32; 4],
    blend: Blend,
    alpha_ref: u8,
    cull: u8,
) {
    // Project.
    let p: Vec<[f32; 3]> = tri
        .iter()
        .map(|v| {
            let iz = 1.0 / v.c[2];
            [
                cam.cx + v.c[0] * cam.focal * iz,
                cam.cy - v.c[1] * cam.focal * iz,
                iz,
            ]
        })
        .collect();
    let area =
        (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
    if area == 0.0 {
        return;
    }
    // Screen-space winding (y down): IW4 front faces are clockwise from the viewer in D3D, i.e. area < 0 here
    // after the y flip. Cull 1 = back, 2 = front.
    let front = area > 0.0;
    if (cull == 1 && !front) || (cull == 2 && front) {
        return;
    }
    let minx = p
        .iter()
        .map(|q| q[0])
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as usize;
    let maxx =
        (p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil() as isize).min(w as isize - 1);
    let miny = p
        .iter()
        .map(|q| q[1])
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as usize;
    let maxy =
        (p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil() as isize).min(h as isize - 1);
    if maxx < 0 || maxy < 0 {
        return;
    }
    for py in miny..=maxy as usize {
        for px in minx..=maxx as usize {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let e =
                |a: [f32; 3], b: [f32; 3]| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
            let (w0, w1, w2) = (
                e(p[1], p[2]) / area,
                e(p[2], p[0]) / area,
                e(p[0], p[1]) / area,
            );
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            // Perspective-correct weights.
            let iz = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
            let z = 1.0 / iz;
            let i = py * w + px;
            if z >= depth[i] {
                continue;
            }
            let (a, b, c) = (w0 * p[0][2] * z, w1 * p[1][2] * z, w2 * p[2][2] * z);
            let mix = |f: &dyn Fn(&CV) -> f32| a * f(&tri[0]) + b * f(&tri[1]) + c * f(&tri[2]);
            let cv = CV {
                c: [0.0, 0.0, z],
                uv: [mix(&|v| v.uv[0]), mix(&|v| v.uv[1])],
                lm: [mix(&|v| v.lm[0]), mix(&|v| v.lm[1])],
                rgba: [
                    mix(&|v| v.rgba[0]),
                    mix(&|v| v.rgba[1]),
                    mix(&|v| v.rgba[2]),
                    mix(&|v| v.rgba[3]),
                ],
            };
            let s = shade(&cv);
            let dst = colour[i];
            match blend {
                Blend::Opaque => {
                    colour[i] = [s[0], s[1], s[2]];
                    depth[i] = z;
                }
                Blend::AlphaTest => {
                    if s[3] * 255.0 < f32::from(alpha_ref) {
                        continue;
                    }
                    colour[i] = [s[0], s[1], s[2]];
                    depth[i] = z;
                }
                Blend::Blend => {
                    colour[i] = [
                        s[0] * s[3] + dst[0] * (1.0 - s[3]),
                        s[1] * s[3] + dst[1] * (1.0 - s[3]),
                        s[2] * s[3] + dst[2] * (1.0 - s[3]),
                    ];
                }
                Blend::Additive => {
                    colour[i] = [
                        (dst[0] + s[0]).min(1.0),
                        (dst[1] + s[1]).min(1.0),
                        (dst[2] + s[2]).min(1.0),
                    ];
                }
                Blend::Multiply => {
                    colour[i] = [dst[0] * s[0], dst[1] * s[1], dst[2] * s[2]];
                }
            }
            stats.pixels_drawn += 1;
        }
    }
}

/// Spawn points in a pack.
pub fn spawns(bytes: &[u8]) -> Result<Vec<SpawnRecord>, String> {
    let pack = Pack::parse(bytes, false).map_err(|e| format!("{e:?}"))?;
    Ok(pack
        .section(tag::SPAWNS)
        .map(|s| records::iter(s.data).collect())
        .unwrap_or_default())
}

pub fn write_png(path: &Path, img: &Image) -> Result<(), String> {
    crate::texture::write_png(path, img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_list_roundtrip() {
        let tris = [0u16, 1, 2, 2, 1, 3];
        let dl = records::world_display_list(&tris);
        assert_eq!(display_list_triangles(&dl).unwrap(), tris);
        assert!(display_list_triangles(&[0x61, 0, 0]).is_err());
    }

    #[test]
    fn cube_faces_by_direction() {
        assert_eq!(cube_face([1.0, 0.0, 0.0]).0, 0);
        assert_eq!(cube_face([-1.0, 0.1, 0.0]).0, 1);
        assert_eq!(cube_face([0.0, 1.0, 0.2]).0, 2);
        assert_eq!(cube_face([0.0, 0.0, 1.0]).0, 4);
        let (_, s, t) = cube_face([0.0, 0.0, -1.0]);
        assert_eq!((s, t), (0.5, 0.5));
    }

    #[test]
    fn near_clip_keeps_the_front_part() {
        let v = |z: f32| CV {
            c: [0.0, 0.0, z],
            uv: [0.0; 2],
            lm: [0.0; 2],
            rgba: [1.0; 4],
        };
        assert_eq!(clip_near([v(10.0), v(20.0), v(30.0)]).len(), 3);
        assert_eq!(clip_near([v(1.0), v(20.0), v(30.0)]).len(), 4);
        assert!(clip_near([v(1.0), v(2.0), v(3.0)]).is_empty());
    }
}

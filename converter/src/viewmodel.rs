//! A weapon's first-person model for the Wii: the view hands and the gun (attached at the hands' `tag_weapon`) as one
//! skeleton, their skinned LOD 0 surfaces, and the weapon's animations baked to evenly spaced frames.
//!
//! Layouts and conventions follow IW4L (Apache-2.0, rev `acc57a5`): bones and bind pose from
//! `asset_model/src/model_skel.rs` (`capture_model_skel_iw4`, `capture_pose_src`), skin weights from its
//! `decode_surface_skin`, the attached-model skeleton from `xmodel_runtime/src/dobj.rs` (roots of an attached model
//! sit on the parent tag; child bones: parent = index − parentList), XAnim capture from
//! `asset_anim/src/xanim_catalog.rs` (`capture_xanim`) and clip decoding from `xmodel_runtime::AnimClip`.

use std::collections::HashMap;
use std::path::Path;

use asset_iw4::size as sz;
use fastfile_iw4::{Ptr, XAnimPartsGeometry, ZonePtr, ZoneStream};
use wpk::records::{NONE, Wrap};
use wpk::view::{Bone, Clip, SkinVertex, Surface, Track, ViewModel, quat_to_i16, slot};
use xmodel_runtime::{AnimClip, ClipNotify, RawXAnimParts};

use crate::material_links::{BlendMode, Links};
use crate::pack::Textures;
use crate::static_models::{half, surface_sets, target};
use crate::zone_textures::IndexEntry;

/// Bone index step in skin data (IW4 stores bone × 64, the size of a bone matrix).
const BONE_STRIDE: u16 = 64;
/// IW4 extra skin weights are fractions of 65536.
const WEIGHT_SCALE: f32 = 1.0 / 65536.0;

type Quat = [f32; 4];
type V3 = [f32; 3];
/// A vertex's bones and weights.
type Skin = ([u8; 4], [f32; 4]);

#[cfg(test)]
fn qmul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn qnorm(q: Quat) -> Quat {
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    q.map(|v| v / l)
}

/// Rotation matrix rows of a unit quaternion (x, y, z, w).
fn qmat(q: Quat) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Inverse of the rigid transform (q, t) as 3 × 4 rows.
fn inverse_rigid(q: Quat, t: V3) -> [[f32; 4]; 3] {
    let r = qmat(qnorm(q));
    // Inverse rotation = transpose; inverse translation = -Rᵀ t.
    std::array::from_fn(|i| {
        let row = [r[0][i], r[1][i], r[2][i]];
        [
            row[0],
            row[1],
            row[2],
            -(row[0] * t[0] + row[1] * t[1] + row[2] * t[2]),
        ]
    })
}

struct ModelBones {
    names: Vec<String>,
    /// Parent within the model; None for root bones.
    parent: Vec<Option<usize>>,
    local_rot: Vec<Quat>,
    local_trans: Vec<V3>,
    /// Bind world transform (model space), from `baseMat`.
    bind: Vec<(Quat, V3)>,
}

fn read_bones(s: &ZoneStream<'_>, links: &Links, m: usize) -> Result<ModelBones, String> {
    let g = links.model_geometry.get(m).ok_or("no model geometry")?;
    let e = |e: fastfile_iw4::ZoneError| format!("{e:?}");
    let names_ptr = g.bone_names.ok_or("no bone names")?;
    let base_mat = g.base_mat.ok_or("no base matrices")?;
    let n = g.num_bones;
    let roots = g.num_root_bones.min(n);
    let mut out = ModelBones {
        names: Vec::with_capacity(n),
        parent: Vec::with_capacity(n),
        local_rot: Vec::with_capacity(n),
        local_trans: Vec::with_capacity(n),
        bind: Vec::with_capacity(n),
    };
    for i in 0..n {
        let id = s.u16_at(names_ptr, i * 2).map_err(e)?;
        out.names
            .push(links.script_string(s, id).unwrap_or("").to_owned());
        let mt = base_mat.at(i * sz::DOBJ_ANIM_MAT);
        let f = |o| s.f32_at(mt, o).map_err(e);
        out.bind
            .push(([f(0)?, f(4)?, f(8)?, f(12)?], [f(16)?, f(20)?, f(24)?]));
        if i < roots {
            out.parent.push(None);
            out.local_rot.push([0.0, 0.0, 0.0, 1.0]);
            out.local_trans.push([0.0; 3]);
        } else {
            let c = i - roots;
            let step = usize::from(
                s.u8_at(g.parent_list.ok_or("no parent list")?, c)
                    .map_err(e)?,
            );
            let parent = i
                .checked_sub(step)
                .filter(|&p| p < i && step > 0)
                .ok_or("bad parent step")?;
            let q = g.quats.ok_or("no bone rotations")?.at(c * sz::XMODEL_QUAT);
            let qi = [
                s.i16_at(q, 0).map_err(e)?,
                s.i16_at(q, 2).map_err(e)?,
                s.i16_at(q, 4).map_err(e)?,
                s.i16_at(q, 6).map_err(e)?,
            ];
            let t = g.trans.ok_or("no bone translations")?.at(c * 12);
            out.parent.push(Some(parent));
            out.local_rot.push(anim_iw4::full_quat(qi));
            out.local_trans.push([
                s.f32_at(t, 0).map_err(e)?,
                s.f32_at(t, 4).map_err(e)?,
                s.f32_at(t, 8).map_err(e)?,
            ]);
        }
    }
    Ok(out)
}

/// Per-vertex bones and weights of a surface (IW4 rigid vertex lists, else the blend buckets).
fn read_skin(
    s: &ZoneStream<'_>,
    surface: Ptr,
    vertex_count: usize,
    num_bones: usize,
) -> Result<Vec<Skin>, String> {
    let e = |e: fastfile_iw4::ZoneError| format!("{e:?}");
    let mut skins = vec![([0u8; 4], [1.0, 0.0, 0.0, 0.0]); vertex_count];
    let bone_at = |raw: u16| -> Result<u8, String> {
        let b = raw / BONE_STRIDE;
        if !raw.is_multiple_of(BONE_STRIDE) || usize::from(b) >= num_bones || b > 255 {
            return Err("bad skin bone".into());
        }
        Ok(b as u8)
    };
    let vert_list_count = s.u32_at(surface, s.layout(32, 48)).map_err(e)? as usize;
    if vert_list_count > 0
        && let Ok(ZonePtr::Offset(list)) = s.ptr_at(surface, s.layout(36, 56))
    {
        let list = s.resolve_alias(list);
        let mut v = 0usize;
        for i in 0..vert_list_count {
            let entry = list.at(i * s.layout(sz::XRIGID_VERT_LIST, 16));
            let bone = bone_at(s.u16_at(entry, 0).map_err(e)?)?;
            let run = usize::from(s.u16_at(entry, 2).map_err(e)?);
            for _ in 0..run {
                if v < vertex_count {
                    skins[v] = ([bone, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
                    v += 1;
                }
            }
        }
        if v == vertex_count {
            return Ok(skins);
        }
    }
    let vi = surface.at(s.layout(16, 24));
    let counts: [usize; 4] =
        std::array::from_fn(|k| s.i16_at(vi, k * 2).unwrap_or(0).max(0) as usize);
    let blend = match s.ptr_at(vi, 8).map_err(e)? {
        ZonePtr::Offset(p) => s.resolve_alias(p),
        // No ownership data: everything on bone 0 (IW4L does the same).
        _ => return Ok(skins),
    };
    let (mut cursor, mut v) = (0usize, 0usize);
    for (bucket, &count) in counts.iter().enumerate() {
        let influences = bucket + 1;
        for _ in 0..count {
            let mut bones = [0u8; 4];
            let mut weights = [0f32; 4];
            bones[0] = bone_at(s.u16_at(blend, cursor * 2).map_err(e)?)?;
            let mut rest = 1.0;
            for x in 1..influences {
                let off = cursor + 1 + (x - 1) * 2;
                bones[x] = bone_at(s.u16_at(blend, off * 2).map_err(e)?)?;
                weights[x] = f32::from(s.u16_at(blend, off * 2 + 2).map_err(e)?) * WEIGHT_SCALE;
                rest -= weights[x];
            }
            weights[0] = rest;
            cursor += 1 + (influences - 1) * 2;
            if v >= vertex_count {
                return Err("skin has more vertices than the surface".into());
            }
            skins[v] = (bones, weights);
            v += 1;
        }
    }
    if v != vertex_count {
        return Err("skin covers part of the surface".into());
    }
    Ok(skins)
}

struct SkinnedSurface {
    material: Option<usize>,
    verts: Vec<SkinVertex>,
    indices: Vec<u16>,
}

/// The surfaces of LOD `lod` (or the most detailed below it that has any) of model `m`, except those using a bone in `hidden` (model-local bone indices; IW4: a surface is
/// hidden when its part bits meet the hidden bones).
fn read_surfaces(
    s: &ZoneStream<'_>,
    links: &Links,
    sets: &HashMap<String, Ptr>,
    m: usize,
    bone_base: usize,
    hidden: &[usize],
    lod: usize,
) -> Result<(Vec<SkinnedSurface>, usize), String> {
    let g = links.model_geometry.get(m).ok_or("no model geometry")?;
    let e = |e: fastfile_iw4::ZoneError| format!("{e:?}");
    let lod = (0..=lod.min(g.lod_numsurfs.len() - 1))
        .rev()
        .find(|&l| g.lod_numsurfs[l] > 0)
        .unwrap_or(0);
    let surfaces = match g.lod_xsurfaces[lod] {
        Some(p) => p,
        None => {
            let name = g.lod_surface_names[lod].ok_or("no LOD surfaces")?;
            let name = s.cstr(s.resolve_alias(name)).map_err(e)?;
            *sets
                .get(name.trim_start_matches(','))
                .ok_or("shared LOD 0 surfaces not found")?
        }
    };
    let mut out = Vec::new();
    let mut hidden_count = 0;
    for i in 0..usize::from(g.lod_numsurfs[lod]) {
        let surf = surfaces.at(i * sz::XSURFACE);
        let part_bits: [u32; 6] =
            std::array::from_fn(|k| s.u32_at(surf, s.layout(40, 64) + k * 4).unwrap_or(0));
        if hidden
            .iter()
            .any(|&b| b < 192 && part_bits[b >> 5] & (0x8000_0000u32 >> (b & 31)) != 0)
        {
            hidden_count += 1;
            continue;
        }
        let vertex_count = usize::from(s.u16_at(surf, 2).map_err(e)?);
        let tri_count = usize::from(s.u16_at(surf, 4).map_err(e)?);
        let verts = target(s, surf, 28).ok_or("vertex pointer")?;
        let tris = target(s, surf, 12).ok_or("triangle pointer")?;
        let skin = read_skin(s, surf, vertex_count, g.num_bones)?;
        let mut sv = Vec::with_capacity(vertex_count);
        for (v, (bones, weights)) in skin.iter().enumerate() {
            let p = verts.at(v * sz::GFX_PACKED_VERTEX);
            let f = |o| s.f32_at(p, o).map_err(e);
            let bgra = s.u32_at(p, 16).map_err(e)?.to_le_bytes();
            let tc = s.u32_at(p, 20).map_err(e)?;
            // Weights to bytes summing to 255 (rounding remainder on the first).
            let mut w = weights.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as i32);
            w[0] += 255 - w.iter().sum::<i32>();
            sv.push(SkinVertex {
                pos: [f(0)?, f(4)?, f(8)?],
                rgba: u32::from_be_bytes([bgra[2], bgra[1], bgra[0], bgra[3]]),
                uv: [half((tc >> 16) as u16), half(tc as u16)],
                bones: bones.map(|b| (usize::from(b) + bone_base) as u8),
                weights: w.map(|x| x.clamp(0, 255) as u8),
            });
        }
        let mut indices = Vec::with_capacity(tri_count * 3);
        for t in 0..tri_count * 3 {
            indices.push(s.u16_at(tris, t * 2).map_err(e)?);
        }
        let slot = usize::from(g.lod_surf_index[lod]) + i;
        let material = (slot < g.material_handle_count)
            .then(|| {
                g.material_handles
                    .and_then(|h| links.resolve_material(h.at(slot * s.pointer_bytes())))
            })
            .flatten();
        out.push(SkinnedSurface {
            material,
            verts: sv,
            indices,
        });
    }
    Ok((out, hidden_count))
}

/// Read `n` little-endian values of `size` bytes from `p` (empty when the pointer is absent or short).
fn read_array<T>(
    p: Option<Ptr>,
    n: usize,
    size: usize,
    read: impl Fn(Ptr, usize) -> Option<T>,
) -> Vec<T> {
    let Some(p) = p else {
        return Vec::new();
    };
    let v: Vec<T> = (0..n).map_while(|i| read(p, i * size)).collect();
    if v.len() == n { v } else { Vec::new() }
}

/// An XAnim's raw parts, as IW4L's `capture_xanim` copies them (delta translation left out: view animations
/// don't move the root).
fn raw_parts(
    s: &ZoneStream<'_>,
    links: &Links,
    name: &str,
    g: &XAnimPartsGeometry,
) -> RawXAnimParts {
    let u8s = |p, n| read_array(p, n, 1, |p, o| s.u8_at(p, o).ok());
    let u16s = |p, n| read_array(p, n, 2, |p, o| s.u16_at(p, o).ok());
    let u32s = |p, n| read_array(p, n, 4, |p, o| s.u32_at(p, o).ok());
    let track_count = usize::from(g.bone_count[9]);
    let names = g.names.map_or_else(Vec::new, |arr| {
        (0..track_count)
            .map(|i| {
                let id = s.u16_at(arr, i * 2).unwrap_or(0);
                links.script_string(s, id).unwrap_or("").to_owned()
            })
            .collect()
    });
    let notifies = g.notify.map_or_else(Vec::new, |arr| {
        (0..g.notify_count)
            .map(|i| {
                let b = arr.at(i * sz::XANIM_NOTIFY_INFO);
                ClipNotify {
                    name: links
                        .script_string(s, s.u16_at(b, 0).unwrap_or(0))
                        .unwrap_or("")
                        .to_owned(),
                    time: s.f32_at(b, 4).unwrap_or(0.0),
                }
            })
            .collect()
    });
    let indices = if g.indices_are_bytes {
        u8s(g.indices, g.index_count)
            .into_iter()
            .map(u16::from)
            .collect()
    } else {
        u16s(g.indices, g.index_count)
    };
    RawXAnimParts {
        name: name.to_owned(),
        data_byte: u8s(g.data_byte, g.data_byte_count),
        data_short: u16s(g.data_short, g.data_short_count),
        data_int: u32s(g.data_int, g.data_int_count),
        random_data_byte: u8s(g.random_data_byte, g.random_data_byte_count),
        random_data_short: u16s(g.random_data_short, g.random_data_short_count),
        random_data_int: u32s(g.random_data_int, g.random_data_int_count),
        numframes: g.numframes,
        flags: g.flags & !0x2,
        bone_count: g.bone_count,
        framerate: g.framerate,
        names,
        notifies,
        indices,
        delta_trans: None,
    }
}

/// Bake a clip to `numframes + 1` evenly spaced poses. Tracks are matched to bones by name (first match, as IW4
/// applies a track to the first bone of that name); a track that never sets rotation keeps the bind rotation.
fn bake(clip: &AnimClip, slot_id: u8, bone_of: &HashMap<&str, usize>, bind_rot: &[Quat]) -> Clip {
    let frames = usize::from(clip.numframes) + 1;
    let rate = if clip.framerate > 0.0 {
        clip.framerate
    } else {
        30.0
    };
    let mut tracks = Vec::new();
    for (ti, t) in clip.tracks.iter().enumerate() {
        let Some(&bone) = bone_of.get(t.name.as_str()) else {
            continue;
        };
        let mut rot = Vec::with_capacity(frames);
        let mut trans = Vec::with_capacity(frames);
        let mut any_rot = false;
        let mut any_trans = false;
        for f in 0..frames {
            let sample = clip.sample_track(ti, f as f32 / rate);
            let (r, tr) = sample.map_or((None, None), |x| (x.rotation, x.translation));
            any_rot |= r.is_some();
            any_trans |= tr.is_some();
            rot.push(quat_to_i16(r.unwrap_or(bind_rot[bone])));
            trans.push(tr.unwrap_or([0.0; 3]));
        }
        if !any_rot && !any_trans {
            continue;
        }
        if rot.iter().all(|q| *q == rot[0]) {
            rot.truncate(1);
        }
        if !any_trans {
            trans.clear();
        } else if trans.iter().all(|v| *v == trans[0]) {
            trans.truncate(1);
        }
        tracks.push(Track {
            bone: bone as u16,
            rot,
            trans,
            keys: Vec::new(),
        });
    }
    Clip {
        slot: slot_id,
        looping: clip.looping,
        framerate: rate,
        frames: frames as u16,
        tracks,
    }
}

/// What the converter reports about a viewmodel (counts only).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ViewReport {
    pub bones: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub surfaces: usize,
    pub textures: usize,
    pub clips: usize,
    pub missing_clips: usize,
    pub hidden_surfaces: usize,
}

/// The first-person model of weapon `w` (index into `links.weapons`), with its textures added to `tex`.
pub fn build(
    s: &ZoneStream<'_>,
    links: &Links,
    w: usize,
    index: &[IndexEntry],
    tex_dir: &Path,
    tex: &mut Textures,
) -> Result<(ViewModel, ViewReport), String> {
    let view = links.weapon_views.get(w).ok_or("no such weapon")?;
    let model_named = |n: &Option<String>| {
        let n = n.as_deref()?;
        links.models.iter().position(|m| m.name == n)
    };
    let hands = model_named(&view.hands).ok_or("view hands model not in the zone")?;
    let gun = model_named(&view.gun).ok_or("gun model not in the zone")?;
    let sets = surface_sets(s, links);
    let hb = read_bones(s, links, hands)?;
    let gb = read_bones(s, links, gun)?;
    let attach = hb.names.iter().position(|n| n == "tag_weapon").unwrap_or(0);
    let base = hb.names.len();
    if base + gb.names.len() > 255 {
        return Err("more than 255 bones".into());
    }

    let mut bones = Vec::with_capacity(base + gb.names.len());
    let mut names: Vec<&str> = Vec::new();
    let mut bind_rot = Vec::new();
    for (mb, offset, root_parent) in [(&hb, 0usize, None), (&gb, base, Some(attach))] {
        for i in 0..mb.names.len() {
            let parent = match mb.parent[i] {
                Some(p) => Some(p + offset),
                None => root_parent,
            };
            let (bq, bt) = mb.bind[i];
            bones.push(Bone {
                parent: parent.map_or(-1, |p| p as i16),
                local_rot: mb.local_rot[i],
                local_trans: mb.local_trans[i],
                inv_bind: inverse_rigid(bq, bt),
            });
            names.push(mb.names[i].as_str());
            bind_rot.push(mb.local_rot[i]);
        }
    }
    let mut bone_of: HashMap<&str, usize> = HashMap::new();
    for (i, n) in names.iter().enumerate() {
        if !n.is_empty() {
            bone_of.entry(n).or_insert(i);
        }
    }
    let view_bone = names
        .iter()
        .position(|n| *n == "tag_view")
        .ok_or("no tag_view bone")?;
    let flash_bone = names
        .iter()
        .position(|n| *n == "tag_flash")
        .map_or(u16::MAX, |i| i as u16);

    // Surfaces of both models, one vertex and index list; textures from the materials' colour images.
    let mut files_by_image: HashMap<usize, &IndexEntry> = HashMap::new();
    for e in index {
        for &i in &e.images {
            files_by_image.entry(i).or_insert(e);
        }
    }
    let mut vm = ViewModel {
        bones,
        view_bone: view_bone as u16,
        flash_bone,
        ..ViewModel::default()
    };
    let mut texture_count = 0;
    let mut hidden_surfaces = 0;
    for (m, bone_base, mb) in [(hands, 0, &hb), (gun, base, &gb)] {
        // A hidden tag hides its whole subtree (e.g. tag_heartbeat's screen bones, a sight's reticle).
        let mut hide_bone = vec![false; mb.names.len()];
        for i in 0..mb.names.len() {
            let named = view
                .hide
                .iter()
                .any(|h| h.eq_ignore_ascii_case(&mb.names[i]));
            let parent_hidden = mb.parent[i].is_some_and(|p| hide_bone[p]);
            hide_bone[i] = named || parent_hidden;
        }
        let hidden: Vec<usize> = (0..mb.names.len()).filter(|&i| hide_bone[i]).collect();
        let (surfaces, h) = read_surfaces(s, links, &sets, m, bone_base, &hidden, 0)?;
        hidden_surfaces += h;
        for surf in surfaces {
            let Some(mat) = surf.material else { continue };
            let info = &links.material_info[mat];
            let texture = match links
                .colour_image(mat)
                .and_then(|img| files_by_image.get(&img))
            {
                Some(e) => {
                    texture_count += 1;
                    tex.role = e.role;
                    tex.add_file(tex_dir, &e.file, Wrap::Repeat)?
                }
                None => NONE,
            };
            let (blend, alpha_ref) = match info.blend {
                BlendMode::Opaque => (wpk::records::Blend::Opaque, 0),
                BlendMode::AlphaTest(t) => (wpk::records::Blend::AlphaTest, t),
                BlendMode::Blend => (wpk::records::Blend::Blend, 0),
                BlendMode::Additive => (wpk::records::Blend::Additive, 0),
                BlendMode::Multiply => (wpk::records::Blend::Multiply, 0),
            };
            let first_vertex = vm.verts.len();
            if first_vertex + surf.verts.len() > 65535 {
                return Err("more than 65535 viewmodel vertices".into());
            }
            vm.surfaces.push(Surface {
                first_index: vm.indices.len() as u32,
                index_count: surf.indices.len() as u32,
                texture,
                blend: blend as u8,
                alpha_ref,
            });
            vm.indices
                .extend(surf.indices.iter().map(|&i| i + first_vertex as u16));
            vm.verts.extend(surf.verts);
        }
    }

    // Animations by IW4 slot.
    // IW4 asset names are case-insensitive (weapons ask for `viewmodel_M4_idle`, the XAnim is lower case).
    let xanim: HashMap<String, &XAnimPartsGeometry> = links
        .xanims
        .iter()
        .map(|(n, g)| (n.to_ascii_lowercase(), g))
        .collect();
    let mut missing = 0;
    for slot_id in slot::ALL {
        let Some(Some(name)) = view.anims.get(usize::from(slot_id)) else {
            continue;
        };
        let Some(g) = xanim.get(&name.to_ascii_lowercase()) else {
            missing += 1;
            continue;
        };
        match AnimClip::from_parts(&raw_parts(s, links, name, g)) {
            Ok(clip) => vm.clips.push(bake(&clip, slot_id, &bone_of, &bind_rot)),
            Err(_) => missing += 1,
        }
    }
    vm.validate().map_err(|e| format!("viewmodel: {e:?}"))?;
    let report = ViewReport {
        bones: vm.bones.len(),
        vertices: vm.verts.len(),
        triangles: vm.indices.len() / 3,
        surfaces: vm.surfaces.len(),
        textures: texture_count,
        clips: vm.clips.len(),
        missing_clips: missing,
        hidden_surfaces,
    };
    Ok((vm, report))
}

/// A player character for the map pack: body model `body` with head model `head` attached as IW4 attaches a head
/// (no tag: the head's bones that share a name with the body's are the body's; its other bones hang under their
/// mapped parents), skinned surfaces of LOD `lod` (or the nearest more detailed one). Surfaces carry their material
/// index in `texture` until the pack resolves them. Returns the model and its bone names.
pub fn build_character(
    s: &ZoneStream<'_>,
    links: &Links,
    body: usize,
    head: Option<usize>,
    lod: usize,
) -> Result<(ViewModel, Vec<String>), String> {
    let sets = surface_sets(s, links);
    let bb = read_bones(s, links, body)?;
    let mut names: Vec<String> = bb.names.clone();
    let mut vm = ViewModel::default();
    for i in 0..bb.names.len() {
        let (bq, bt) = bb.bind[i];
        vm.bones.push(Bone {
            parent: bb.parent[i].map_or(-1, |p| p as i16),
            local_rot: bb.local_rot[i],
            local_trans: bb.local_trans[i],
            inv_bind: inverse_rigid(bq, bt),
        });
    }
    let mut parts: Vec<(usize, Vec<usize>)> = vec![(body, (0..bb.names.len()).collect())];
    if let Some(h) = head {
        let hb = read_bones(s, links, h)?;
        // The head's vertices are in its own model space (its root at the origin), where its `baseMat` bind poses
        // are. Each head bone the body also has follows the body's (a copy parented to it, unnamed so animations
        // keep driving the body's); the head's own bones (the face) hang under them; all unbind with the head's
        // bind poses.
        let mut map = Vec::with_capacity(hb.names.len());
        for i in 0..hb.names.len() {
            let (bq, bt) = hb.bind[i];
            let shared = names
                .iter()
                .position(|n| !n.is_empty() && *n == hb.names[i]);
            let (parent, local_rot, local_trans, name) = match shared {
                Some(k) => (Some(k), [0.0, 0.0, 0.0, 1.0], [0.0; 3], String::new()),
                None => (
                    hb.parent[i].map(|p| map[p]),
                    hb.local_rot[i],
                    hb.local_trans[i],
                    hb.names[i].clone(),
                ),
            };
            vm.bones.push(Bone {
                parent: parent.map_or(-1, |p| p as i16),
                local_rot,
                local_trans,
                inv_bind: inverse_rigid(bq, bt),
            });
            names.push(name);
            map.push(vm.bones.len() - 1);
        }
        parts.push((h, map));
    }
    if vm.bones.len() > 255 {
        return Err("more than 255 bones".into());
    }
    for (m, map) in &parts {
        let (surfaces, _) = read_surfaces(s, links, &sets, *m, 0, &[], lod)?;
        for mut surf in surfaces {
            for v in &mut surf.verts {
                v.bones = v
                    .bones
                    .map(|b| map.get(usize::from(b)).copied().unwrap_or(0) as u8);
            }
            let (blend, alpha_ref) = match surf.material.map(|m| links.material_info[m].blend) {
                Some(BlendMode::AlphaTest(t)) => (wpk::records::Blend::AlphaTest, t),
                Some(BlendMode::Blend) => (wpk::records::Blend::Blend, 0),
                Some(BlendMode::Additive) => (wpk::records::Blend::Additive, 0),
                Some(BlendMode::Multiply) => (wpk::records::Blend::Multiply, 0),
                _ => (wpk::records::Blend::Opaque, 0),
            };
            let first_vertex = vm.verts.len();
            if first_vertex + surf.verts.len() > 65535 {
                return Err("more than 65535 character vertices".into());
            }
            vm.surfaces.push(Surface {
                first_index: vm.indices.len() as u32,
                index_count: surf.indices.len() as u32,
                texture: surf.material.map_or(NONE, |m| m as u16),
                blend: blend as u8,
                alpha_ref,
            });
            vm.indices
                .extend(surf.indices.iter().map(|&i| i + first_vertex as u16));
            vm.verts.extend(surf.verts);
        }
    }
    vm.flash_bone = u16::MAX;
    vm.validate().map_err(|e| format!("character: {e:?}"))?;
    Ok((vm, names))
}

/// A character's hit boxes (`CHHB`, team `team`): every bone of `body` (then `head`, whose bones follow
/// [`build_character`]'s order: after the body's) with IW4 bone bounds (`XBoneInfo`: midpoint, half size,
/// radius² at 24; zero radius: none) and its `partClassification` hit location. As IW4L
/// `asset_model/src/model_skel.rs` `capture_bone_collision` (Apache-2.0, rev `acc57a5`).
pub fn character_hitboxes(
    s: &ZoneStream<'_>,
    links: &Links,
    body: usize,
    head: Option<usize>,
    team: u8,
) -> Vec<wpk::records::BoneHitRecord> {
    let mut out = Vec::new();
    let mut base = 0usize;
    for m in core::iter::once(body).chain(head) {
        let Some(g) = links.model_geometry.get(m) else {
            break;
        };
        if let (Some(info), Some(classes)) = (g.bone_info, g.part_classification) {
            for bone in 0..g.num_bones {
                let row = info.at(bone * sz::XBONE_INFO);
                let f = |o: usize| s.f32_at(row, o).ok();
                let (Some(r2), Some(loc)) = (f(24), s.u8_at(classes, bone).ok()) else {
                    continue;
                };
                let (Some(m0), Some(m1), Some(m2), Some(h0), Some(h1), Some(h2)) =
                    (f(0), f(4), f(8), f(12), f(16), f(20))
                else {
                    continue;
                };
                let (mid, half) = ([m0, m1, m2], [h0, h1, h2]);
                let ok = r2.is_finite()
                    && r2 > 0.0
                    && mid.iter().all(|v| v.is_finite())
                    && half.iter().all(|v| v.is_finite() && *v >= 0.0);
                let index = base + bone;
                if ok && index < 256 {
                    out.push(wpk::records::BoneHitRecord {
                        team,
                        bone: index as u8,
                        loc,
                        midpoint: mid,
                        half_size: half,
                    });
                }
            }
        }
        base += g.num_bones;
    }
    out
}

/// Most vertices a third-person weapon may have: the first LOD at or under it is used (else the last).
const HELD_MAX_VERTICES: usize = 900;
/// Third-person weapon textures are at most this many texels across (a gun is a few dozen pixels on screen).
const HELD_TEXTURE_MAX: u32 = 64;
/// GX vertex format of character and held-weapon display lists (POS, CLR0, TEX0 indexed: three u16 per vertex).
const HELD_VTXFMT: u8 = 6;

/// A weapon's third-person model (`common.wpk` `GUNM`): world model `m` at its first LOD of at most
/// [`HELD_MAX_VERTICES`] vertices, in its bind pose (origin at `tag_origin`, which goes on the hand's
/// `tag_weapon_right`), the parts under bones named in `hide` left out, as strips; textures (capped at
/// [`HELD_TEXTURE_MAX`]) into `tex`. Returns the model and its vertex count.
pub fn build_held(
    s: &ZoneStream<'_>,
    links: &Links,
    m: usize,
    hide: &[String],
    index: &[IndexEntry],
    tex_dir: &Path,
    tex: &mut Textures,
) -> Result<wpk::held::Model, String> {
    let limits = RigidLimits {
        max_vertices: HELD_MAX_VERTICES,
        texture_max: HELD_TEXTURE_MAX,
        role: "held",
    };
    build_rigid(s, links, m, hide, &limits, index, tex_dir, tex).map(|(model, _)| model)
}

/// How detailed a rigid model is built.
pub struct RigidLimits {
    /// The first LOD at or under this many vertices is used (else the last).
    pub max_vertices: usize,
    /// Textures at most this many texels across.
    pub texture_max: u32,
    /// The texture report's role.
    pub role: &'static str,
}

/// A model's bones in its bind pose (model space): name, origin and x axis.
pub type Tags = Vec<(String, [f32; 3], [f32; 3])>;

/// Model `m` as a rigid model in the [`wpk::held`] layout (see [`build_held`]) within `limits`, and its tags.
#[allow(clippy::too_many_arguments)]
pub fn build_rigid(
    s: &ZoneStream<'_>,
    links: &Links,
    m: usize,
    hide: &[String],
    limits: &RigidLimits,
    index: &[IndexEntry],
    tex_dir: &Path,
    tex: &mut Textures,
) -> Result<(wpk::held::Model, Tags), String> {
    let sets = surface_sets(s, links);
    let bones = read_bones(s, links, m)?;
    let mut hidden = vec![false; bones.names.len()];
    for i in 0..bones.names.len() {
        let named = hide.iter().any(|h| h.eq_ignore_ascii_case(&bones.names[i]));
        hidden[i] = named || bones.parent[i].is_some_and(|p| hidden[p]);
    }
    let hidden: Vec<usize> = (0..hidden.len()).filter(|&i| hidden[i]).collect();
    let lods = links
        .model_geometry
        .get(m)
        .map_or(1, |g| g.lod_numsurfs.len().max(1));
    let mut surfaces = Vec::new();
    for lod in 0..lods {
        let (surfs, _) = read_surfaces(s, links, &sets, m, 0, &hidden, lod)?;
        let n: usize = surfs.iter().map(|x| x.verts.len()).sum();
        surfaces = surfs;
        if n <= limits.max_vertices {
            break;
        }
    }
    let mut files_by_image: HashMap<usize, &IndexEntry> = HashMap::new();
    for e in index {
        for &i in &e.images {
            files_by_image.entry(i).or_insert(e);
        }
    }
    let mut out = wpk::held::Model::default();
    // The muzzle (`tag_flash`) in the bind pose: its origin and its x axis.
    if let Some(i) = bones.names.iter().position(|n| n == "tag_flash") {
        let (q, t) = bones.bind[i];
        let r = qmat(q);
        out.flash = [t, [r[0][0], r[1][0], r[2][0]]];
    } else {
        out.flash = [[0.0; 3], [1.0, 0.0, 0.0]];
    }
    for surf in surfaces {
        let Some(mat) = surf.material else { continue };
        let info = &links.material_info[mat];
        let texture = match links
            .colour_image(mat)
            .and_then(|img| files_by_image.get(&img))
        {
            Some(e) => {
                tex.role = limits.role;
                tex.add_file_capped(tex_dir, &e.file, Wrap::Repeat, limits.texture_max)?
            }
            None => NONE,
        };
        let (blend, alpha_ref) = match info.blend {
            BlendMode::Opaque => (wpk::records::Blend::Opaque, 0),
            BlendMode::AlphaTest(t) => (wpk::records::Blend::AlphaTest, t),
            BlendMode::Blend => (wpk::records::Blend::Blend, 0),
            BlendMode::Additive => (wpk::records::Blend::Additive, 0),
            BlendMode::Multiply => (wpk::records::Blend::Multiply, 0),
        };
        let first = out.positions.len();
        if first + surf.verts.len() > 65535 {
            return Err("more than 65535 held weapon vertices".into());
        }
        for v in &surf.verts {
            out.positions.push(v.pos);
            out.attributes.push((v.rgba, v.uv));
        }
        let tris: Vec<u16> = surf.indices.iter().map(|&i| i + first as u16).collect();
        let st = wpk::strips::stripify(&tris);
        out.draws.push(wpk::held::Draw {
            dl: wpk::strips::strips_display_list_attrs(&st, HELD_VTXFMT, 3),
            texture,
            blend: blend as u8,
            alpha_ref,
        });
    }
    let tags = bones
        .names
        .iter()
        .zip(&bones.bind)
        .map(|(n, (q, t))| {
            let r = qmat(*q);
            (n.clone(), *t, [r[0][0], r[1][0], r[2][0]])
        })
        .collect();
    Ok((out, tags))
}

/// Player animations baked by bone name (`common.wpk`): each animation of `list` becomes a clip (an empty one when
/// it can't be read, so clip numbers stay the list's) whose tracks index the returned name list. A track with no rotation keeps the model's bind rotation (stored as the
/// zero quaternion); frames a track leaves out repeat its first. The clips ride in a [`ViewModel`] with one
/// placeholder bone per name. Returns the clips, the names and how many animations were missing.
pub fn bake_player_anims(
    s: &ZoneStream<'_>,
    links: &Links,
    list: &[String],
) -> (ViewModel, Vec<String>, usize) {
    let xanim: HashMap<String, &XAnimPartsGeometry> = links
        .xanims
        .iter()
        .map(|(n, g)| (n.to_ascii_lowercase(), g))
        .collect();
    let mut names: Vec<String> = Vec::new();
    let mut vm = ViewModel::default();
    let mut missing = 0;
    let empty = Clip {
        slot: 0,
        looping: false,
        framerate: 30.0,
        frames: 1,
        tracks: Vec::new(),
    };
    for name in list {
        let Some(g) = xanim.get(name.as_str()) else {
            missing += 1;
            vm.clips.push(empty.clone());
            continue;
        };
        let Ok(clip) = AnimClip::from_parts(&raw_parts(s, links, name, g)) else {
            missing += 1;
            vm.clips.push(empty.clone());
            continue;
        };
        let frames = usize::from(clip.numframes) + 1;
        let rate = if clip.framerate > 0.0 {
            clip.framerate
        } else {
            30.0
        };
        let mut tracks = Vec::new();
        for (ti, t) in clip.tracks.iter().enumerate() {
            let bone = match names.iter().position(|n| *n == t.name) {
                Some(b) => b,
                None => {
                    names.push(t.name.clone());
                    names.len() - 1
                }
            };
            let samples: Vec<_> = (0..frames)
                .map(|f| {
                    clip.sample_track(ti, f as f32 / rate)
                        .map_or((None, None), |x| (x.rotation, x.translation))
                })
                .collect();
            let first_rot = samples.iter().find_map(|s| s.0);
            let first_trans = samples.iter().find_map(|s| s.1);
            if first_rot.is_none() && first_trans.is_none() {
                continue;
            }
            let mut rot: Vec<[i16; 4]> = match first_rot {
                Some(r0) => samples
                    .iter()
                    .map(|s| quat_to_i16(s.0.unwrap_or(r0)))
                    .collect(),
                None => vec![[0; 4]],
            };
            if rot.iter().all(|q| *q == rot[0]) {
                rot.truncate(1);
            }
            let mut trans: Vec<[f32; 3]> = match first_trans {
                Some(t0) => samples.iter().map(|s| s.1.unwrap_or(t0)).collect(),
                None => Vec::new(),
            };
            if trans.iter().all(|v| *v == trans[0]) {
                trans.truncate(1);
            }
            // Translations that stay within the key error of their first sample are constant (capture noise).
            if trans
                .iter()
                .all(|v| (0..3).all(|k| (v[k] - trans[0][k]).abs() <= KEY_TRANS_ERROR))
            {
                trans.truncate(1);
            }
            // Fingers can't be seen on a third-person body at 480p: they hold their first pose.
            if FINGERS.iter().any(|f| t.name.starts_with(f)) {
                rot.truncate(1);
                trans.truncate(1);
            }
            let (rot, trans, keys) = key_frames(rot, trans);
            tracks.push(Track {
                bone: bone as u16,
                rot,
                trans,
                keys,
            });
        }
        vm.clips.push(Clip {
            slot: 0,
            looping: clip.looping,
            framerate: rate,
            frames: frames as u16,
            tracks,
        });
    }
    vm.bones = vec![
        Bone {
            parent: -1,
            ..Bone::default()
        };
        names.len().max(1)
    ];
    (vm, names, missing)
}
/// Finger bones (IW4's player skeleton names).
const FINGERS: [&str; 5] = ["j_thumb", "j_index", "j_mid", "j_ring", "j_pinky"];

/// Third-person animation error allowed by [`key_frames`]: rotation (radians, about 0.9 degrees) and translation
/// (world units).
const KEY_ROT_ERROR: f32 = 0.015;
const KEY_TRANS_ERROR: f32 = 0.3;

/// Keep only the frames of a per-frame track that interpolation between neighbouring kept frames can't reproduce
/// within [`KEY_ROT_ERROR`] / [`KEY_TRANS_ERROR`] (greedy, from the first frame). Single-sample (constant) parts
/// stay single. Returns the samples at the keys and the keys (empty when every frame is kept or the track is
/// constant).
fn key_frames(
    rot: Vec<[i16; 4]>,
    trans: Vec<[f32; 3]>,
) -> (Vec<[i16; 4]>, Vec<[f32; 3]>, Vec<u16>) {
    let n = rot.len().max(trans.len());
    if n <= 2 {
        return (rot, trans, Vec::new());
    }
    let q = |i: usize| -> [f32; 4] {
        let v = rot[if rot.len() == 1 { 0 } else { i }];
        v.map(|x| f32::from(x) / 32767.0)
    };
    let fits = |a: usize, b: usize| -> bool {
        (a + 1..b).all(|i| {
            let w = (i - a) as f32 / (b - a) as f32;
            if rot.len() > 1 {
                let (qa, mut qb) = (q(a), q(b));
                if (0..4).map(|k| qa[k] * qb[k]).sum::<f32>() < 0.0 {
                    qb = qb.map(|x| -x);
                }
                let mut l: [f32; 4] = std::array::from_fn(|k| qa[k] + (qb[k] - qa[k]) * w);
                let len = l.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
                l = l.map(|x| x / len);
                let qi = q(i);
                let dot = (0..4).map(|k| l[k] * qi[k]).sum::<f32>().abs().min(1.0);
                if 2.0 * dot.acos() > KEY_ROT_ERROR {
                    return false;
                }
            }
            if trans.len() > 1 {
                let (ta, tb, ti) = (trans[a], trans[b], trans[i]);
                let d: f32 = (0..3)
                    .map(|k| {
                        let e = ta[k] + (tb[k] - ta[k]) * w - ti[k];
                        e * e
                    })
                    .sum();
                if d.sqrt() > KEY_TRANS_ERROR {
                    return false;
                }
            }
            true
        })
    };
    let mut keys = vec![0usize];
    let mut a = 0;
    let mut b = 2;
    while b < n {
        if !fits(a, b) {
            a = b - 1;
            keys.push(a);
        }
        b += 1;
    }
    keys.push(n - 1);
    if keys.len() * 4 > n * 3 {
        return (rot, trans, Vec::new());
    }
    let pick = |len: usize| -> Vec<usize> {
        if len <= 1 {
            (0..len).collect()
        } else {
            keys.clone()
        }
    };
    let rot_out = pick(rot.len()).into_iter().map(|i| rot[i]).collect();
    let trans_out = pick(trans.len()).into_iter().map(|i| trans[i]).collect();
    (
        rot_out,
        trans_out,
        keys.into_iter().map(|k| k as u16).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_frames_keep_the_turns() {
        // A rotation about z that turns steadily for 10 frames, then holds for 10: three keys (start, turn, end).
        let rot: Vec<[i16; 4]> = (0..20)
            .map(|f| {
                let a = 0.01 * f.min(10) as f32;
                quat_to_i16([0.0, 0.0, a.sin(), a.cos()])
            })
            .collect();
        let trans: Vec<[f32; 3]> = (0..20).map(|f| [f as f32, 0.0, 0.0]).collect();
        let (r, t, k) = key_frames(rot, trans);
        assert_eq!(k, [0, 10, 19]);
        assert_eq!((r.len(), t.len()), (3, 3));
        assert_eq!(t[1], [10.0, 0.0, 0.0]);
        // Too few frames to key: unchanged.
        let (r, _, k) = key_frames(vec![[0, 0, 0, 32767]; 2], vec![]);
        assert!(k.is_empty() && r.len() == 2);
    }

    #[test]
    fn rigid_inverse_undoes_the_transform() {
        // 90 degrees about Z, then translate.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let q = [0.0, 0.0, h, h];
        let t = [10.0, 20.0, 30.0];
        let r = qmat(q);
        let p = [1.0, 2.0, 3.0];
        let world: V3 =
            std::array::from_fn(|i| r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2] + t[i]);
        let inv = inverse_rigid(q, t);
        let back: V3 = std::array::from_fn(|i| {
            inv[i][0] * world[0] + inv[i][1] * world[1] + inv[i][2] * world[2] + inv[i][3]
        });
        for k in 0..3 {
            assert!((back[k] - p[k]).abs() < 1e-4, "{back:?}");
        }
        let id = qmul(q, [-q[0], -q[1], -q[2], q[3]]);
        assert!((id[3] - 1.0).abs() < 1e-6);
    }
}

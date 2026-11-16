//! The `VIEW` section of `common.wpk`: a weapon's first-person model (view hands + gun as one skeleton) and its
//! animations, ready for CPU skinning on the Wii.
//!
//! Skeleton: bones in parent-first order; each has its bind local pose (what an animation track replaces: rotation
//! replaced, translation = bind + delta, as IW4) and the inverse of its bind world transform (vertices are stored in
//! model space at the bind pose; skinning = posed world × inverse bind). `view_bone` (`tag_view`) is placed at the
//! camera.
//! Animations: each clip is baked to `frames` evenly spaced poses at `framerate`; a track stores one rotation (i16 ×
//! 4, ±32767) and optionally a translation delta per frame, or a single value when constant.
//!
//! On disk a blob may instead carry **compact tracks** ([`flags::COMPACT_TRACKS`], written by
//! [`ViewModel::encode_compact`]): only the frames linear interpolation can't reproduce within a [`Tolerance`], each with
//! its frame number, and translations as i16 with a per-track scale. [`ViewModel::decode`] expands them back to every
//! frame, so the in-memory [`ViewModel`] (and everything that samples it) is the same either way.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Format flags in the blob header (the u16 after `flash_bone`; 0 in older files).
pub mod flags {
    /// Clip tracks are stored as keyframes (see the module docs).
    pub const COMPACT_TRACKS: u16 = 1;
}

/// How far a compact track may stray from the baked frames (after decoding, against the original frames).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// Largest rotation error, degrees.
    pub rot_deg: f32,
    /// Largest translation error, world units (inches).
    pub trans: f32,
}

impl Tolerance {
    /// The `--slim-views` setting: far below a pixel for the first-person gun (the screen is ~0.1° per pixel; the
    /// gun sits ~10-20 units from the eye, where 0.005 units is ~0.02°).
    pub const SLIM: Tolerance = Tolerance {
        rot_deg: 0.02,
        trans: 0.005,
    };
}

/// Weapon animation slots (IW4 `weapAnimFiles` indices); every one a weapon names is carried.
pub mod slot {
    pub const IDLE: u8 = 0x1;
    pub const EMPTY_IDLE: u8 = 0x2;
    pub const FIRE: u8 = 0x3;
    pub const HOLD_FIRE: u8 = 0x4;
    pub const LASTSHOT: u8 = 0x5;
    pub const RECHAMBER: u8 = 0x6;
    pub const MELEE: u8 = 0x7;
    pub const MELEE_CHARGE: u8 = 0x8;
    pub const RELOAD: u8 = 0x9;
    pub const RELOAD_EMPTY: u8 = 0xA;
    pub const RELOAD_START: u8 = 0xB;
    pub const RELOAD_END: u8 = 0xC;
    pub const RAISE: u8 = 0xD;
    pub const FIRST_RAISE: u8 = 0xE;
    pub const BREACH_RAISE: u8 = 0xF;
    pub const DROP: u8 = 0x10;
    pub const ALT_RAISE: u8 = 0x11;
    pub const ALT_DROP: u8 = 0x12;
    pub const QUICK_RAISE: u8 = 0x13;
    pub const QUICK_DROP: u8 = 0x14;
    pub const EMPTY_RAISE: u8 = 0x15;
    pub const EMPTY_DROP: u8 = 0x16;
    pub const SPRINT_IN: u8 = 0x17;
    pub const SPRINT_LOOP: u8 = 0x18;
    pub const SPRINT_OUT: u8 = 0x19;
    pub const STUNNED_START: u8 = 0x1A;
    pub const STUNNED_LOOP: u8 = 0x1B;
    pub const STUNNED_END: u8 = 0x1C;
    pub const DETONATE: u8 = 0x1D;
    pub const NIGHTVISION_WEAR: u8 = 0x1E;
    pub const NIGHTVISION_REMOVE: u8 = 0x1F;
    pub const ADS_FIRE: u8 = 0x20;
    pub const ADS_LASTSHOT: u8 = 0x21;
    pub const ADS_RECHAMBER: u8 = 0x22;
    pub const ADS_UP: u8 = 0x23;
    pub const ADS_DOWN: u8 = 0x24;
    /// Number of slots (0 is IW4's root, never a clip).
    pub const COUNT: usize = 0x25;
    /// Every slot a clip can fill.
    pub const ALL: [u8; COUNT - 1] = {
        let mut a = [0u8; COUNT - 1];
        let mut i = 0;
        while i < COUNT - 1 {
            a[i] = (i + 1) as u8;
            i += 1;
        }
        a
    };
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bone {
    /// Parent bone index, or -1 for a root.
    pub parent: i16,
    pub local_rot: [f32; 4],
    pub local_trans: [f32; 3],
    /// Inverse of the bind world transform, 3 × 4 rows.
    pub inv_bind: [[f32; 4]; 3],
}

/// A skinned vertex: model-space bind position, colour, UV, up to four bones with weights summing to 255.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinVertex {
    pub pos: [f32; 3],
    pub rgba: u32,
    pub uv: [f32; 2],
    pub bones: [u8; 4],
    pub weights: [u8; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Surface {
    pub first_index: u32,
    pub index_count: u32,
    /// Texture index into the pack's `TEXT`, or `records::NONE`.
    pub texture: u16,
    pub blend: u8,
    pub alpha_ref: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub bone: u16,
    /// One rotation per frame, or a single one (constant).
    pub rot: Vec<[i16; 4]>,
    /// Translation deltas: one per frame, a single one, or none (bind translation).
    pub trans: Vec<[f32; 3]>,
    /// Key frames, ascending from 0 to the last frame: when present, `rot` and `trans` hold one sample per key
    /// (or a single one) and the frames between keys are interpolated. Empty: a sample per frame.
    pub keys: Vec<u16>,
}

impl Track {
    /// This per-frame track (`frames` frames) with only the key frames that interpolation between neighbours can't
    /// reproduce within `tol` (rotation and translation together, one key list; the runtime samples keyed tracks
    /// directly, so this also saves memory once decoded). Unchanged when keys wouldn't save space.
    pub fn keyed(&self, frames: usize, tol: Tolerance) -> Track {
        let n = self.rot.len().max(self.trans.len());
        if !self.keys.is_empty() || n <= 2 || n != frames {
            return self.clone();
        }
        let tol_r = f64::from(tol.rot_deg);
        let spread = |len: usize| len == n;
        let ok = |a: usize, b: usize| {
            (a + 1..b).all(|m| {
                let w = (m - a) as f32 / (b - a) as f32;
                let rot_ok = !spread(self.rot.len())
                    || compact::rot_err_deg(
                        compact::lerp_rot(self.rot[a], self.rot[b], w),
                        self.rot[m],
                    ) <= tol_r;
                let trans_ok = !spread(self.trans.len()) || {
                    let (ta, tb, tm) = (self.trans[a], self.trans[b], self.trans[m]);
                    let d: f32 = (0..3)
                        .map(|k| {
                            let e = ta[k] + (tb[k] - ta[k]) * w - tm[k];
                            e * e
                        })
                        .sum();
                    libm::sqrtf(d) <= tol.trans
                };
                rot_ok && trans_ok
            })
        };
        let keys = compact::reduce(n, ok);
        // A key costs its frame number (2 bytes) on top of its samples.
        let per_frame =
            8 * usize::from(spread(self.rot.len())) + 12 * usize::from(spread(self.trans.len()));
        if keys.len() * (per_frame + 2) >= n * per_frame {
            return self.clone();
        }
        let pick = |len: usize| len == n;
        Track {
            bone: self.bone,
            rot: if pick(self.rot.len()) {
                keys.iter().map(|&k| self.rot[k]).collect()
            } else {
                self.rot.clone()
            },
            trans: if pick(self.trans.len()) {
                keys.iter().map(|&k| self.trans[k]).collect()
            } else {
                self.trans.clone()
            },
            keys: keys.iter().map(|&k| k as u16).collect(),
        }
    }

    /// The samples around frame `f` (fractional, in `0..=frames-1`) and the blend between them: indices into `rot`
    /// (a `len` of 1 gives index 0) for a track of `len` samples.
    pub fn at(&self, f: f32, len: usize) -> (usize, usize, f32) {
        if len <= 1 {
            return (0, 0, 0.0);
        }
        if self.keys.len() == len {
            // The last key at or before f.
            let k = self
                .keys
                .partition_point(|&x| f32::from(x) <= f)
                .clamp(1, len - 1)
                - 1;
            let (a, b) = (f32::from(self.keys[k]), f32::from(self.keys[k + 1]));
            let w = ((f - a) / (b - a).max(1.0)).clamp(0.0, 1.0);
            return (k, k + 1, w);
        }
        let f = f.clamp(0.0, (len - 1) as f32);
        let a = libm::floorf(f) as usize;
        ((a).min(len - 1), (a + 1).min(len - 1), f - a as f32)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub slot: u8,
    /// Which weapon's animations this clip is (0: the weapon file's own; others: [`SetRecord`]'s akimbo hands and
    /// alternates on the same model). On disk in bits 1..8 of the looping byte (older files: 0).
    pub set: u8,
    pub looping: bool,
    pub framerate: f32,
    pub frames: u16,
    pub tracks: Vec<Track>,
}

/// A weapon whose first-person clips are sets of this model's (`VSET` in the weapon file; set 0, the file's own,
/// is not listed), in the companion file `views/<base>@<set>.wpk` ([`companion_name`]): an akimbo pair `name`
/// (`set` its right hand's clips, `left` its left hand's, both posing the whole model: the left gun is the model
/// drawn a second time), or the alternate of weapon `name` (`alternate`: `set` its clips, `left` 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetRecord {
    /// The weapon's name (`usp_akimbo_mp`), NUL-padded.
    pub name: [u8; SetRecord::NAME_LEN],
    pub set: u8,
    pub left: u8,
    /// The clips are `name`'s alternate's (the underbarrel launcher or shotgun), not `name`'s.
    pub alternate: bool,
}

/// The companion file of base weapon `base`'s set `set` (`views/<that>.wpk`): the clips (`VCLP`) and sounds of a
/// [`SetRecord`].
pub fn companion_name(base: &str, set: u8) -> alloc::string::String {
    alloc::format!("{base}@{set}")
}

impl SetRecord {
    pub const NAME_LEN: usize = 44;

    pub fn new(name: &str, set: u8, left: u8, alternate: bool) -> SetRecord {
        let mut n = [0u8; Self::NAME_LEN];
        let len = name.len().min(Self::NAME_LEN);
        n[..len].copy_from_slice(&name.as_bytes()[..len]);
        SetRecord {
            name: n,
            set,
            left,
            alternate,
        }
    }

    pub fn name(&self) -> &str {
        let len = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(Self::NAME_LEN);
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

impl crate::records::Record for SetRecord {
    const LEN: usize = 48;
    fn write(&self, o: &mut Vec<u8>) {
        o.extend_from_slice(&self.name);
        o.u8(self.set);
        o.u8(self.left);
        o.u8(u8::from(self.alternate));
        o.u8(0);
    }
    fn read(b: &[u8]) -> Self {
        let mut name = [0u8; Self::NAME_LEN];
        name.copy_from_slice(&b[..Self::NAME_LEN]);
        SetRecord {
            name,
            set: b[Self::NAME_LEN],
            left: b[Self::NAME_LEN + 1],
            alternate: b[Self::NAME_LEN + 2] != 0,
        }
    }
}

/// The record of weapon `name` in a `VSET` section's data.
pub fn find_set(data: &[u8], name: &str) -> Option<SetRecord> {
    crate::records::iter::<SetRecord>(data).find(|r| r.name() == name)
}

impl Clip {
    /// A clip of set `set` that plays the same set's clip of slot `target` in slot `slot` (IW4 weapons name one
    /// animation for several slots: raise, first raise, quick raise ...). Stored as a clip with no frames whose
    /// frame rate is the target slot.
    pub fn alias(slot: u8, set: u8, target: u8) -> Clip {
        Clip {
            slot,
            set,
            looping: false,
            framerate: f32::from(target),
            frames: 0,
            tracks: Vec::new(),
        }
    }

    /// The slot this clip plays the clip of, when it is an alias ([`Clip::alias`]).
    pub fn alias_of(&self) -> Option<u8> {
        (self.frames == 0 && self.tracks.is_empty()).then_some(self.framerate as u8)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewModel {
    pub bones: Vec<Bone>,
    pub view_bone: u16,
    /// Bone the muzzle flash and bullets come from (`tag_flash`), or u16::MAX.
    pub flash_bone: u16,
    pub verts: Vec<SkinVertex>,
    pub indices: Vec<u16>,
    pub surfaces: Vec<Surface>,
    pub clips: Vec<Clip>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewError {
    Truncated,
    BadIndex(&'static str),
}

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl Rd<'_> {
    fn need(&self, n: usize) -> Result<(), ViewError> {
        if self.at + n <= self.b.len() {
            Ok(())
        } else {
            Err(ViewError::Truncated)
        }
    }
    fn u8(&mut self) -> Result<u8, ViewError> {
        self.need(1)?;
        self.at += 1;
        Ok(self.b[self.at - 1])
    }
    fn u16(&mut self) -> Result<u16, ViewError> {
        self.need(2)?;
        self.at += 2;
        Ok(be::u16(self.b, self.at - 2))
    }
    fn u32(&mut self) -> Result<u32, ViewError> {
        self.need(4)?;
        self.at += 4;
        Ok(be::u32(self.b, self.at - 4))
    }
    fn f32(&mut self) -> Result<f32, ViewError> {
        Ok(f32::from_bits(self.u32()?))
    }
}

impl ViewModel {
    pub fn validate(&self) -> Result<(), ViewError> {
        let nb = self.bones.len();
        for (i, b) in self.bones.iter().enumerate() {
            if b.parent >= i as i16 {
                return Err(ViewError::BadIndex("bone parent after the bone"));
            }
        }
        if usize::from(self.view_bone) >= nb {
            return Err(ViewError::BadIndex("view bone"));
        }
        if self
            .verts
            .iter()
            .any(|v| v.bones.iter().any(|&b| usize::from(b) >= nb))
        {
            return Err(ViewError::BadIndex("vertex bone"));
        }
        if self
            .indices
            .iter()
            .any(|&i| usize::from(i) >= self.verts.len())
        {
            return Err(ViewError::BadIndex("index"));
        }
        if self
            .surfaces
            .iter()
            .any(|s| (s.first_index + s.index_count) as usize > self.indices.len())
        {
            return Err(ViewError::BadIndex("surface indices"));
        }
        for c in &self.clips {
            for t in &c.tracks {
                let frames = usize::from(c.frames.max(1));
                let n = if t.keys.is_empty() {
                    frames
                } else {
                    t.keys.len()
                };
                let keys_ok = t.keys.is_empty()
                    || (t.keys[0] == 0
                        && usize::from(t.keys[t.keys.len() - 1]) == frames - 1
                        && t.keys.windows(2).all(|w| w[0] < w[1]));
                if usize::from(t.bone) >= nb
                    || !keys_ok
                    || !(t.rot.len() == 1 || t.rot.len() == n)
                    || !(t.trans.len() <= 1 || t.trans.len() == n)
                {
                    return Err(ViewError::BadIndex("clip track"));
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        self.encode_with(None)
    }

    /// As [`ViewModel::encode`], with compact tracks within `tol` (see the module docs).
    pub fn encode_compact(&self, tol: Tolerance) -> Vec<u8> {
        self.encode_with(Some(tol))
    }

    fn encode_with(&self, compact: Option<Tolerance>) -> Vec<u8> {
        let mut o = Vec::new();
        o.u16(self.bones.len() as u16);
        o.u16(self.view_bone);
        o.u16(self.flash_bone);
        o.u16(if compact.is_some() {
            flags::COMPACT_TRACKS
        } else {
            0
        });
        for b in &self.bones {
            o.u16(b.parent as u16);
            b.local_rot.iter().for_each(|&x| o.f32(x));
            b.local_trans.iter().for_each(|&x| o.f32(x));
            b.inv_bind.iter().flatten().for_each(|&x| o.f32(x));
        }
        o.u32(self.verts.len() as u32);
        for v in &self.verts {
            v.pos.iter().for_each(|&x| o.f32(x));
            o.u32(v.rgba);
            v.uv.iter().for_each(|&x| o.f32(x));
            o.extend_from_slice(&v.bones);
            o.extend_from_slice(&v.weights);
        }
        o.u32(self.indices.len() as u32);
        self.indices.iter().for_each(|&i| o.u16(i));
        o.u16(self.surfaces.len() as u16);
        for s in &self.surfaces {
            o.u32(s.first_index);
            o.u32(s.index_count);
            o.u16(s.texture);
            o.u8(s.blend);
            o.u8(s.alpha_ref);
        }
        write_clips(&mut o, &self.clips, compact);
        o
    }

    pub fn decode(b: &[u8]) -> Result<ViewModel, ViewError> {
        let mut r = Rd { b, at: 0 };
        let nb = usize::from(r.u16()?);
        let view_bone = r.u16()?;
        let flash_bone = r.u16()?;
        let format = r.u16()?;
        let mut bones = Vec::with_capacity(nb);
        for _ in 0..nb {
            let parent = r.u16()? as i16;
            let local_rot = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
            let local_trans = [r.f32()?, r.f32()?, r.f32()?];
            let mut inv_bind = [[0.0; 4]; 3];
            for row in &mut inv_bind {
                for v in row.iter_mut() {
                    *v = r.f32()?;
                }
            }
            bones.push(Bone {
                parent,
                local_rot,
                local_trans,
                inv_bind,
            });
        }
        let nv = r.u32()? as usize;
        let mut verts = Vec::with_capacity(nv);
        for _ in 0..nv {
            verts.push(SkinVertex {
                pos: [r.f32()?, r.f32()?, r.f32()?],
                rgba: r.u32()?,
                uv: [r.f32()?, r.f32()?],
                bones: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
                weights: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
            });
        }
        let ni = r.u32()? as usize;
        let mut indices = Vec::with_capacity(ni);
        for _ in 0..ni {
            indices.push(r.u16()?);
        }
        let ns = usize::from(r.u16()?);
        let mut surfaces = Vec::with_capacity(ns);
        for _ in 0..ns {
            surfaces.push(Surface {
                first_index: r.u32()?,
                index_count: r.u32()?,
                texture: r.u16()?,
                blend: r.u8()?,
                alpha_ref: r.u8()?,
            });
        }
        let clips = read_clips(&mut r, format)?;
        let v = ViewModel {
            bones,
            view_bone,
            flash_bone,
            verts,
            indices,
            surfaces,
            clips,
        };
        v.validate()?;
        Ok(v)
    }

    /// Leave out the surfaces `hidden` names (by index; an attachment variant's hidden parts, `parts`) and the
    /// vertices only they used; the rest keep their order. Bones and clips are untouched.
    pub fn drop_surfaces(&mut self, hidden: impl Fn(usize) -> bool) {
        if !(0..self.surfaces.len()).any(&hidden) {
            return;
        }
        // Sized exactly (MEM1 is short): the kept surfaces' indices and the vertices they use.
        let mut used = alloc::vec![false; self.verts.len()];
        let mut index_count = 0;
        let mut surface_count = 0;
        for (_, s) in self
            .surfaces
            .iter()
            .enumerate()
            .filter(|(i, _)| !hidden(*i))
        {
            surface_count += 1;
            let at = s.first_index as usize;
            let end = (at + s.index_count as usize).min(self.indices.len());
            for &v in self.indices.get(at..end).unwrap_or(&[]) {
                if let Some(u) = used.get_mut(usize::from(v)) {
                    *u = true;
                    index_count += 1;
                }
            }
        }
        let vertex_count = used.iter().filter(|&&u| u).count();
        drop(used);
        let mut keep = alloc::vec![u16::MAX; self.verts.len()];
        let mut verts = Vec::with_capacity(vertex_count);
        let mut indices = Vec::with_capacity(index_count);
        let mut surfaces = Vec::with_capacity(surface_count);
        for (i, s) in self.surfaces.iter().enumerate() {
            if hidden(i) {
                continue;
            }
            let first_index = indices.len() as u32;
            let at = s.first_index as usize;
            let end = (at + s.index_count as usize).min(self.indices.len());
            for &v in self.indices.get(at..end).unwrap_or(&[]) {
                let Some(k) = keep.get_mut(usize::from(v)) else {
                    continue;
                };
                if *k == u16::MAX {
                    *k = verts.len() as u16;
                    verts.push(self.verts[usize::from(v)]);
                }
                indices.push(*k);
            }
            surfaces.push(Surface {
                first_index,
                index_count: indices.len() as u32 - first_index,
                ..*s
            });
        }
        self.verts = verts;
        self.indices = indices;
        self.surfaces = surfaces;
    }

    /// The weapon file's own clip of `slot` (set 0).
    pub fn clip(&self, slot: u8) -> Option<&Clip> {
        self.clips.iter().find(|c| c.slot == slot && c.set == 0)
    }

    /// Add `clips` (another weapon's sets on this model, [`decode_clip_set`]) when their tracks fit this model;
    /// false (nothing added) otherwise.
    pub fn add_clips(&mut self, clips: Vec<Clip>) -> bool {
        let n = self.clips.len();
        self.clips.extend(clips);
        if self.validate().is_err() {
            self.clips.truncate(n);
            return false;
        }
        true
    }
}

/// Clips alone (`VCLP` in a weapon's companion file: an akimbo pair's hands, an alternate), plain tracks (keyed ones
/// keep their keys).
pub fn encode_clip_set(clips: &[Clip]) -> Vec<u8> {
    let mut o = Vec::new();
    o.u16(0);
    write_clips(&mut o, clips, None);
    o
}

/// The clips of [`encode_clip_set`]; add them to the weapon's model with [`ViewModel::add_clips`].
pub fn decode_clip_set(b: &[u8]) -> Result<Vec<Clip>, ViewError> {
    let mut r = Rd { b, at: 0 };
    let format = r.u16()?;
    read_clips(&mut r, format)
}

fn write_clips(o: &mut Vec<u8>, clips: &[Clip], compact: Option<Tolerance>) {
    o.u16(clips.len() as u16);
    for c in clips {
        o.u8(c.slot);
        o.u8(u8::from(c.looping) | c.set << 1);
        o.f32(c.framerate);
        o.u16(c.frames);
        o.u16(c.tracks.len() as u16);
        for t in &c.tracks {
            match compact {
                Some(tol) => compact::write_track(o, t, usize::from(c.frames), tol),
                None => {
                    o.u16(t.bone);
                    o.u16(t.rot.len() as u16);
                    o.u16(t.trans.len() as u16);
                    o.u16(t.keys.len() as u16);
                    t.rot.iter().flatten().for_each(|&x| o.u16(x as u16));
                    t.trans.iter().flatten().for_each(|&x| o.f32(x));
                    t.keys.iter().for_each(|&k| o.u16(k));
                }
            }
        }
    }
}

fn read_clips(r: &mut Rd<'_>, format: u16) -> Result<Vec<Clip>, ViewError> {
    let nc = usize::from(r.u16()?);
    let mut clips = Vec::with_capacity(nc);
    for _ in 0..nc {
        let slot = r.u8()?;
        let flags = r.u8()?;
        let (looping, set) = (flags & 1 != 0, flags >> 1);
        let framerate = r.f32()?;
        let frames = r.u16()?;
        let nt = usize::from(r.u16()?);
        let mut tracks = Vec::with_capacity(nt);
        for _ in 0..nt {
            if format & flags::COMPACT_TRACKS != 0 {
                tracks.push(compact::read_track(r, usize::from(frames))?);
                continue;
            }
            let bone = r.u16()?;
            let nr = usize::from(r.u16()?);
            let ntr = usize::from(r.u16()?);
            let nk = usize::from(r.u16()?);
            let mut rot = Vec::with_capacity(nr);
            for _ in 0..nr {
                rot.push([
                    r.u16()? as i16,
                    r.u16()? as i16,
                    r.u16()? as i16,
                    r.u16()? as i16,
                ]);
            }
            let mut trans = Vec::with_capacity(ntr);
            for _ in 0..ntr {
                trans.push([r.f32()?, r.f32()?, r.f32()?]);
            }
            let mut keys = Vec::with_capacity(nk);
            for _ in 0..nk {
                keys.push(r.u16()?);
            }
            tracks.push(Track {
                bone,
                rot,
                trans,
                keys,
            });
        }
        clips.push(Clip {
            slot,
            set,
            looping,
            framerate,
            frames,
            tracks,
        });
    }
    Ok(clips)
}

/// Compact tracks: keyframes chosen against the decoder's own interpolation, expanded back to every frame on load.
pub(crate) mod compact {
    use super::{Push, Rd, Tolerance, Track, ViewError, quat_from_i16, quat_to_i16};
    use alloc::vec::Vec;

    const TRANS_NONE: u8 = 0;
    /// i16 × 3 with a per-track f32 scale.
    const TRANS_I16: u8 = 1;
    /// f32 × 3 (a track whose range is too wide for i16 within the tolerance).
    const TRANS_F32: u8 = 2;

    /// Where frame `m` falls between keys at frames `a` and `b` (the encoder and decoder share this exactly).
    fn frac(m: usize, a: usize, b: usize) -> f32 {
        (m - a) as f32 / (b - a) as f32
    }

    /// Sign-aligned nlerp of two stored rotations, quantised as a decoded frame is.
    pub(super) fn lerp_rot(a: [i16; 4], b: [i16; 4], t: f32) -> [i16; 4] {
        let qa = quat_from_i16(a);
        let mut qb = quat_from_i16(b);
        if (0..4).map(|k| qa[k] * qb[k]).sum::<f32>() < 0.0 {
            qb = qb.map(|v| -v);
        }
        quat_to_i16(core::array::from_fn(|k| qa[k] + (qb[k] - qa[k]) * t))
    }

    /// Angle between two stored rotations, degrees. `4·asin(|a − b| / 2)` in f64: exact at the tiny angles that
    /// matter here, where `acos` of a dot product rounds to zero.
    pub(super) fn rot_err_deg(a: [i16; 4], b: [i16; 4]) -> f64 {
        let unit = |q: [i16; 4]| {
            let v = q.map(f64::from);
            let len = libm::sqrt(v.iter().map(|x| x * x).sum::<f64>()).max(1e-12);
            v.map(|x| x / len)
        };
        let (qa, qb) = (unit(a), unit(b));
        let dist = |s: f64| {
            libm::sqrt(
                (0..4)
                    .map(|k| {
                        let d = qa[k] - s * qb[k];
                        d * d
                    })
                    .sum::<f64>(),
            )
        };
        let d = dist(1.0).min(dist(-1.0));
        4.0 * libm::asin((d / 2.0).min(1.0)) * (180.0 / core::f64::consts::PI)
    }

    fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
        core::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
    }

    fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
        libm::sqrtf((0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum::<f32>())
    }

    /// Longest gap between two keys, frames (bounds the encoder's search).
    const MAX_GAP: usize = 48;

    /// Keys over `n` frames: from each key, the furthest frame (within [`MAX_GAP`]) that `ok` accepts as the next key.
    /// Searched from far to near: a nearer end can fail where a farther one passes (nlerp's uneven speed makes short
    /// chords of a baked IW4 segment worse than the segment itself). Always keeps the first and the last frame.
    pub(crate) fn reduce(n: usize, ok: impl Fn(usize, usize) -> bool) -> Vec<usize> {
        let mut keys = alloc::vec![0];
        let mut i = 0;
        while i + 1 < n {
            let far = (i + MAX_GAP).min(n - 1);
            let j = (i + 2..=far).rev().find(|&c| ok(i, c)).unwrap_or(i + 1);
            keys.push(j);
            i = j;
        }
        keys
    }

    /// `keys` unless listing their frame numbers costs more than storing every frame (`value_bytes` each).
    fn worth_it(keys: Vec<usize>, n: usize, value_bytes: usize) -> Vec<usize> {
        if keys.len() > 1 && keys.len() < n && keys.len() * (2 + value_bytes) >= n * value_bytes {
            (0..n).collect()
        } else {
            keys
        }
    }

    /// A keyed track (`Track::keys`) as one sample per frame, interpolated as the runtime does.
    pub(crate) fn per_frame(t: &Track, frames: usize) -> (Vec<[i16; 4]>, Vec<[f32; 3]>) {
        if t.keys.is_empty() {
            return (t.rot.clone(), t.trans.clone());
        }
        let spread = |len: usize| len > 1;
        let rot = if spread(t.rot.len()) {
            (0..frames)
                .map(|f| {
                    let (a, b, w) = t.at(f as f32, t.rot.len());
                    lerp_rot(t.rot[a], t.rot[b], w)
                })
                .collect()
        } else {
            t.rot.clone()
        };
        let trans = if spread(t.trans.len()) {
            (0..frames)
                .map(|f| {
                    let (a, b, w) = t.at(f as f32, t.trans.len());
                    lerp3(t.trans[a], t.trans[b], w)
                })
                .collect()
        } else {
            t.trans.clone()
        };
        (rot, trans)
    }

    pub(super) fn write_track(o: &mut Vec<u8>, t: &Track, frames: usize, tol: Tolerance) {
        let (rot_frames, trans_frames) = per_frame(t, frames);
        o.u16(t.bone);
        // Rotation: one value when every frame is within tolerance of the first.
        let rot = &rot_frames;
        let tol_r = f64::from(tol.rot_deg);
        let keys = if rot.len() <= 1 || rot.iter().all(|&q| rot_err_deg(rot[0], q) <= tol_r) {
            alloc::vec![0]
        } else {
            reduce(rot.len(), |a, b| {
                (a + 1..b)
                    .all(|m| rot_err_deg(lerp_rot(rot[a], rot[b], frac(m, a, b)), rot[m]) <= tol_r)
            })
        };
        let keys = worth_it(keys, rot.len(), 8);
        o.u16(keys.len() as u16);
        // Frame numbers only when some frames were dropped (every frame kept: none needed).
        if keys.len() > 1 && keys.len() < rot.len() {
            keys.iter().for_each(|&k| o.u16(k as u16));
        }
        keys.iter()
            .flat_map(|&k| rot[k])
            .for_each(|x| o.u16(x as u16));

        // Translation.
        let tr = &trans_frames;
        if tr.is_empty() {
            o.u8(TRANS_NONE);
            o.u8(0);
            return;
        }
        let max_abs = tr.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
        let scale = if max_abs > 0.0 {
            max_abs / 32767.0
        } else {
            1.0
        };
        // i16 when its rounding (at most half a step per axis) leaves half the tolerance for interpolation.
        let kind = if scale * 0.5 * libm::sqrtf(3.0) <= tol.trans * 0.5 {
            TRANS_I16
        } else {
            TRANS_F32
        };
        let stored = |v: [f32; 3]| -> [f32; 3] {
            if kind == TRANS_I16 {
                v.map(|x| libm::roundf(x / scale).clamp(-32767.0, 32767.0) * scale)
            } else {
                v
            }
        };
        let keys = if tr.len() == 1 || tr.iter().all(|&v| dist3(stored(tr[0]), v) <= tol.trans) {
            alloc::vec![0]
        } else {
            reduce(tr.len(), |a, b| {
                let (va, vb) = (stored(tr[a]), stored(tr[b]));
                (a + 1..b).all(|m| dist3(lerp3(va, vb, frac(m, a, b)), tr[m]) <= tol.trans)
            })
        };
        let keys = worth_it(keys, tr.len(), if kind == TRANS_I16 { 6 } else { 12 });
        o.u8(kind);
        o.u8(0);
        o.u16(keys.len() as u16);
        if keys.len() > 1 && keys.len() < tr.len() {
            keys.iter().for_each(|&k| o.u16(k as u16));
        }
        if kind == TRANS_I16 {
            o.f32(scale);
            for &k in &keys {
                tr[k].iter().for_each(|&x| {
                    o.u16(libm::roundf(x / scale).clamp(-32767.0, 32767.0) as i16 as u16)
                });
            }
        } else {
            keys.iter().flat_map(|&k| tr[k]).for_each(|x| o.f32(x));
        }
    }

    /// Key frame numbers: strictly increasing, from 0 to `frames - 1`. A key per frame stores no list.
    fn read_keys(r: &mut Rd<'_>, count: usize, frames: usize) -> Result<Vec<usize>, ViewError> {
        if count <= 1 {
            return Ok(alloc::vec![0]);
        }
        if count == frames {
            return Ok((0..frames).collect());
        }
        if count > frames {
            return Err(ViewError::BadIndex("clip keys"));
        }
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            keys.push(usize::from(r.u16()?));
        }
        let ordered = keys.windows(2).all(|w| w[0] < w[1]);
        if keys[0] != 0 || keys[count - 1] + 1 != frames || !ordered {
            return Err(ViewError::BadIndex("clip keys"));
        }
        Ok(keys)
    }

    /// Every frame from keyed values: one value stays one (a constant track).
    fn expand<T: Copy>(
        keys: &[usize],
        vals: &[T],
        frames: usize,
        lerp: impl Fn(T, T, f32) -> T,
    ) -> Vec<T> {
        if vals.len() <= 1 {
            return vals.to_vec();
        }
        let mut out = Vec::with_capacity(frames);
        for w in 0..keys.len() - 1 {
            let (a, b) = (keys[w], keys[w + 1]);
            for m in a..b {
                out.push(if m == a {
                    vals[w]
                } else {
                    lerp(vals[w], vals[w + 1], frac(m, a, b))
                });
            }
        }
        out.push(vals[vals.len() - 1]);
        out
    }

    pub(super) fn read_track(r: &mut Rd<'_>, frames: usize) -> Result<Track, ViewError> {
        let bone = r.u16()?;
        let nr = usize::from(r.u16()?);
        if nr == 0 || (nr > 1 && frames < 2) {
            return Err(ViewError::BadIndex("clip keys"));
        }
        let keys = read_keys(r, nr, frames)?;
        let mut vals = Vec::with_capacity(nr);
        for _ in 0..nr {
            vals.push([
                r.u16()? as i16,
                r.u16()? as i16,
                r.u16()? as i16,
                r.u16()? as i16,
            ]);
        }
        let rot = expand(&keys, &vals, frames, lerp_rot);

        let kind = r.u8()?;
        r.u8()?;
        let trans = match kind {
            TRANS_NONE => Vec::new(),
            TRANS_I16 | TRANS_F32 => {
                let nt = usize::from(r.u16()?);
                if nt == 0 || (nt > 1 && frames < 2) {
                    return Err(ViewError::BadIndex("clip keys"));
                }
                let keys = read_keys(r, nt, frames)?;
                let mut vals = Vec::with_capacity(nt);
                if kind == TRANS_I16 {
                    let scale = r.f32()?;
                    for _ in 0..nt {
                        let mut v = [0.0f32; 3];
                        for x in &mut v {
                            *x = f32::from(r.u16()? as i16) * scale;
                        }
                        vals.push(v);
                    }
                } else {
                    for _ in 0..nt {
                        vals.push([r.f32()?, r.f32()?, r.f32()?]);
                    }
                }
                expand(&keys, &vals, frames, lerp3)
            }
            _ => return Err(ViewError::BadIndex("clip translation kind")),
        };
        Ok(Track {
            bone,
            rot,
            trans,
            keys: Vec::new(),
        })
    }
}

/// i16 quaternion (±32767) to f32.
pub fn quat_from_i16(q: [i16; 4]) -> [f32; 4] {
    q.map(|v| f32::from(v) / 32767.0)
}

/// f32 quaternion to i16 (normalized first).
pub fn quat_to_i16(q: [f32; 4]) -> [i16; 4] {
    let len = libm::sqrtf(q.iter().map(|v| v * v).sum::<f32>()).max(1e-12);
    q.map(|v| libm::roundf(v / len * 32767.0).clamp(-32767.0, 32767.0) as i16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Three surfaces (a quad, a triangle sharing a vertex with it, a lone triangle): the middle one left out.
    #[test]
    fn dropped_surfaces_take_only_their_own_vertices() {
        let v = |x: f32| SkinVertex {
            pos: [x, 0.0, 0.0],
            ..SkinVertex::default()
        };
        let mut vm = model_with(vec![]);
        vm.verts = (0..9).map(|i| v(i as f32)).collect();
        vm.indices = vec![0, 1, 2, 2, 1, 3, 3, 4, 5, 6, 7, 8];
        let surf = |first_index, index_count, texture| Surface {
            first_index,
            index_count,
            texture,
            ..Surface::default()
        };
        vm.surfaces = vec![surf(0, 6, 10), surf(6, 3, 11), surf(9, 3, 12)];
        let mut cut = vm.clone();
        cut.drop_surfaces(|i| i == 1);
        assert_eq!(cut.surfaces.len(), 2);
        assert_eq!(
            (cut.surfaces[1].texture, cut.surfaces[1].first_index),
            (12, 6)
        );
        // 4 + 3 vertices kept (4 and 5 belonged to the dropped surface only), positions as before.
        assert_eq!(cut.verts.len(), 7);
        let pos = |m: &ViewModel, k: usize| m.verts[usize::from(m.indices[k])].pos[0];
        for (k, j) in [(0, 0), (5, 5), (6, 9), (8, 11)] {
            assert_eq!(pos(&cut, k), pos(&vm, j));
        }
        assert!(cut.validate().is_ok());
        // Nothing hidden: unchanged.
        let mut same = vm.clone();
        same.drop_surfaces(|_| false);
        assert_eq!(same, vm);
    }

    /// A two-bone model with the given clips (enough for decode's checks).
    fn model_with(clips: alloc::vec::Vec<Clip>) -> ViewModel {
        ViewModel {
            bones: vec![
                Bone {
                    parent: -1,
                    local_rot: [0.0, 0.0, 0.0, 1.0],
                    ..Bone::default()
                },
                Bone {
                    parent: 0,
                    local_rot: [0.0, 0.0, 0.0, 1.0],
                    ..Bone::default()
                },
            ],
            view_bone: 0,
            flash_bone: u16::MAX,
            verts: vec![SkinVertex::default(); 3],
            indices: vec![0, 1, 2],
            surfaces: vec![],
            clips,
        }
    }

    /// Smooth, noisy, constant and near-constant tracks over `n` frames, as a baked clip would hold them.
    fn sample_clip(n: usize) -> Clip {
        let axis_angle = |ax: [f32; 3], deg: f32| {
            let h = deg.to_radians() * 0.5;
            let s = libm::sinf(h);
            quat_to_i16([ax[0] * s, ax[1] * s, ax[2] * s, libm::cosf(h)])
        };
        let mut seed: u32 = 0x2545_f491;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 2001) as f32 / 1000.0 - 1.0
        };
        let smooth_rot: Vec<[i16; 4]> = (0..n)
            .map(|f| axis_angle([0.0, 0.6, 0.8], 40.0 * libm::sinf(f as f32 * 0.07)))
            .collect();
        let smooth_trans: Vec<[f32; 3]> = (0..n)
            .map(|f| {
                let t = f as f32 * 0.05;
                [3.0 * libm::sinf(t), -1.5 * libm::cosf(t), 0.25 * t]
            })
            .collect();
        let noisy_rot: Vec<[i16; 4]> = (0..n)
            .map(|_| axis_angle([1.0, 0.0, 0.0], 5.0 * noise()))
            .collect();
        let near_const: Vec<[i16; 4]> = (0..n)
            .map(|f| {
                let mut q = axis_angle([0.0, 0.0, 1.0], 10.0);
                q[0] += (f % 2) as i16; // one quantisation step of jitter
                q
            })
            .collect();
        Clip {
            slot: slot::RELOAD,
            set: 0,
            looping: false,
            framerate: 30.0,
            frames: n as u16,
            tracks: vec![
                Track {
                    bone: 1,
                    rot: smooth_rot,
                    trans: smooth_trans,
                    keys: vec![],
                },
                Track {
                    bone: 1,
                    rot: noisy_rot,
                    trans: vec![],
                    keys: vec![],
                },
                Track {
                    bone: 1,
                    rot: near_const,
                    trans: vec![[0.5, -0.25, 1.0]],
                    keys: vec![],
                },
                Track {
                    bone: 0,
                    rot: vec![[0, 0, 0, 32767]],
                    trans: vec![],
                    keys: vec![],
                },
                // A translation range too wide for i16 at the tolerance: kept as f32.
                Track {
                    bone: 1,
                    rot: vec![[0, 0, 0, 32767]],
                    trans: (0..n)
                        .map(|f| [5000.0 * libm::sinf(f as f32 * 0.03), 0.0, 0.0])
                        .collect(),
                    keys: vec![],
                },
            ],
        }
    }

    #[test]
    fn compact_tracks_stay_within_tolerance_and_shrink() {
        let tol = Tolerance::SLIM;
        let v = model_with(vec![sample_clip(91), sample_clip(2), sample_clip(1)]);
        let full = v.encode();
        let small = v.encode_compact(tol);
        let back = ViewModel::decode(&small).unwrap();
        assert_eq!(back.clips.len(), v.clips.len());
        for (c, d) in v.clips.iter().zip(&back.clips) {
            assert_eq!(
                (c.slot, c.frames, c.tracks.len()),
                (d.slot, d.frames, d.tracks.len())
            );
            for (t, u) in c.tracks.iter().zip(&d.tracks) {
                assert_eq!(t.bone, u.bone);
                // Every frame (or the constant) within tolerance of the original.
                assert!(u.rot.len() == 1 || u.rot.len() == usize::from(c.frames));
                for f in 0..usize::from(c.frames) {
                    let a = t.rot[f.min(t.rot.len() - 1)];
                    let b = u.rot[f.min(u.rot.len() - 1)];
                    let e = compact::rot_err_deg(a, b);
                    assert!(e <= f64::from(tol.rot_deg) + 1e-9, "rot frame {f}: {e}°");
                }
                assert_eq!(t.trans.is_empty(), u.trans.is_empty());
                for f in 0..t.trans.len().max(u.trans.len()) {
                    let a = t.trans[f.min(t.trans.len() - 1)];
                    let b = u.trans[f.min(u.trans.len() - 1)];
                    let e = libm::sqrtf((0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum());
                    assert!(e <= tol.trans * 1.0001, "trans frame {f}: {e}");
                }
            }
        }
        // The constant and near-constant tracks collapse to one value; the noisy one keeps most frames.
        let c = &back.clips[0];
        assert_eq!(c.tracks[2].rot.len(), 1);
        assert_eq!(c.tracks[3].rot, vec![[0, 0, 0, 32767]]);
        // Even this mix (a noise track, an f32-wide track) must not grow.
        assert!(
            small.len() < full.len(),
            "compact {} vs full {}",
            small.len(),
            full.len()
        );
        // Re-encoding the decoded model compactly is stable in size (keys survive decode exactly).
        assert!(back.encode_compact(tol).len() <= small.len());
    }

    #[test]
    fn baked_keyframes_shrink_and_noise_does_not_grow() {
        let tol = Tolerance::SLIM;
        // What the converter bakes: an IW4 animation keyed every 6 frames, interpolated linearly to every frame.
        // Compact tracks should find those keys again.
        let n = 91;
        let mut seed: u32 = 0x1357_9bdf;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 2001) as f32 / 1000.0 - 1.0
        };
        let keys: Vec<usize> = (0..n).step_by(6).chain(core::iter::once(n - 1)).collect();
        let key_rot: Vec<[i16; 4]> = keys
            .iter()
            .map(|_| quat_to_i16([0.3 * rnd(), 0.3 * rnd(), 0.3 * rnd(), 1.0]))
            .collect();
        let key_tr: Vec<[f32; 3]> = keys
            .iter()
            .map(|_| [4.0 * rnd(), 4.0 * rnd(), 4.0 * rnd()])
            .collect();
        let (mut rot, mut trans) = (Vec::new(), Vec::new());
        for w in 0..keys.len() - 1 {
            let (a, b) = (keys[w], keys[w + 1]);
            for m in a..b {
                let t = (m - a) as f32 / (b - a) as f32;
                rot.push(compact::lerp_rot(key_rot[w], key_rot[w + 1], t));
                trans.push(core::array::from_fn(|k| {
                    key_tr[w][k] + (key_tr[w + 1][k] - key_tr[w][k]) * t
                }));
            }
        }
        rot.push(key_rot[key_rot.len() - 1]);
        trans.push(key_tr[key_tr.len() - 1]);
        let clip = Clip {
            slot: slot::RELOAD,
            set: 0,
            looping: false,
            framerate: 30.0,
            frames: n as u16,
            tracks: vec![Track {
                bone: 1,
                rot,
                trans,
                keys: vec![],
            }],
        };
        let v = model_with(vec![clip]);
        let (full, small) = (v.encode().len(), v.encode_compact(tol).len());
        assert!(
            small * 3 < full,
            "baked keys: compact {small} vs full {full}"
        );

        // Noise can't be thinned; it must not cost more than before.
        let mut noisy = sample_clip(91);
        noisy.tracks = vec![noisy.tracks.remove(1)];
        let v = model_with(vec![noisy]);
        let (full, small) = (v.encode().len(), v.encode_compact(tol).len());
        assert!(small <= full + 4, "noise: compact {small} vs full {full}");
    }

    #[test]
    fn keyed_tracks_compact_like_their_frames() {
        // A track keyed the way the converter keys player animations (samples at keys, interpolated between).
        let keyed = Track {
            bone: 1,
            rot: vec![
                quat_to_i16([0.0, 0.0, 0.0, 1.0]),
                quat_to_i16([0.0, 0.0, 0.2, 1.0]),
                quat_to_i16([0.0, 0.1, 0.2, 1.0]),
            ],
            trans: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 2.0, 0.0]],
            keys: vec![0, 7, 19],
        };
        let clip = Clip {
            slot: slot::IDLE,
            set: 0,
            looping: true,
            framerate: 30.0,
            frames: 20,
            tracks: vec![keyed.clone()],
        };
        let back =
            ViewModel::decode(&model_with(vec![clip]).encode_compact(Tolerance::SLIM)).unwrap();
        let t = &back.clips[0].tracks[0];
        assert!(t.keys.is_empty() && t.rot.len() == 20 && t.trans.len() == 20);
        for f in 0..20 {
            let (a, b, w) = keyed.at(f as f32, 3);
            let want_r = compact::lerp_rot(keyed.rot[a], keyed.rot[b], w);
            assert!(
                compact::rot_err_deg(want_r, t.rot[f]) <= f64::from(Tolerance::SLIM.rot_deg) + 1e-9
            );
            let want_t: [f32; 3] = core::array::from_fn(|k| {
                keyed.trans[a][k] + (keyed.trans[b][k] - keyed.trans[a][k]) * w
            });
            let e = libm::sqrtf((0..3).map(|k| (want_t[k] - t.trans[f][k]).powi(2)).sum());
            assert!(e <= Tolerance::SLIM.trans * 1.0001, "frame {f}: {e}");
        }
    }

    #[test]
    fn compact_keeps_exact_tracks_exact() {
        // A track that only steps (no interpolation can fake it) must come back bit for bit.
        let steps: Vec<[i16; 4]> = (0..12)
            .map(|f| {
                if f < 6 {
                    [0, 0, 0, 32767]
                } else {
                    [0, 23170, 0, 23170]
                }
            })
            .collect();
        let v = model_with(vec![Clip {
            slot: slot::FIRE,
            set: 0,
            looping: false,
            framerate: 30.0,
            frames: 12,
            tracks: vec![Track {
                bone: 1,
                rot: steps.clone(),
                trans: vec![],
                keys: vec![],
            }],
        }]);
        let back = ViewModel::decode(&v.encode_compact(Tolerance::SLIM)).unwrap();
        assert_eq!(back.clips[0].tracks[0].rot, steps);
    }

    #[test]
    fn clip_sets_round_trip_and_filter() {
        let clip = |set: u8, looping: bool| Clip {
            slot: slot::FIRE,
            set,
            looping,
            framerate: 30.0,
            frames: 2,
            tracks: vec![],
        };
        let mut v = model_with(vec![clip(0, true)]);
        let extra = vec![
            clip(3, false),
            clip(127, true),
            Clip::alias(slot::RAISE, 3, slot::FIRE),
        ];
        for blob in [v.encode(), v.encode_compact(Tolerance::SLIM)] {
            assert_eq!(ViewModel::decode(&blob).unwrap().clips, v.clips);
        }
        let back = decode_clip_set(&encode_clip_set(&extra)).unwrap();
        assert_eq!(back, extra);
        assert_eq!(back[2].alias_of(), Some(slot::FIRE));
        assert!(v.add_clips(back));
        assert_eq!(v.clips.len(), 4);
        assert_eq!(v.clip(slot::FIRE).map(|c| c.set), Some(0));
        // Tracks for bones the model hasn't: refused.
        let mut bad = clip(4, false);
        bad.tracks.push(Track {
            bone: 999,
            rot: vec![[0, 0, 0, 32767]],
            trans: vec![],
            keys: vec![],
        });
        assert!(!v.add_clips(vec![bad]));
        assert_eq!(v.clips.len(), 4);
        let r = SetRecord::new("usp_akimbo_xmags_mp", 1, 2, false);
        let mut data = Vec::new();
        crate::records::Record::write(&r, &mut data);
        crate::records::Record::write(&SetRecord::new("m4_gl_mp", 3, 0, true), &mut data);
        assert_eq!(data.len(), 2 * <SetRecord as crate::records::Record>::LEN);
        assert_eq!(find_set(&data, "usp_akimbo_xmags_mp"), Some(r));
        assert_eq!(
            find_set(&data, "m4_gl_mp").map(|r| (r.set, r.left, r.alternate)),
            Some((3, 0, true))
        );
        assert_eq!(find_set(&data, "gl_mp"), None);
        assert_eq!(companion_name("m4", 3), "m4@3");
    }

    #[test]
    fn compact_rejects_bad_keys() {
        // A steady turn over 10 frames: two keys (frames 0 and 9) with a frame list.
        let ramp: Vec<[i16; 4]> = (0..10)
            .map(|f| quat_to_i16([0.0, 0.0, 0.01 * f as f32, 1.0]))
            .collect();
        let clip = Clip {
            slot: slot::FIRE,
            set: 0,
            looping: false,
            framerate: 30.0,
            frames: 10,
            tracks: vec![Track {
                bone: 1,
                rot: ramp,
                trans: vec![],
                keys: vec![],
            }],
        };
        let good = model_with(vec![clip]).encode_compact(Tolerance::SLIM);
        assert!(ViewModel::decode(&good).is_ok());
        // Clips start where a model without clips ends (its clip count is that model's last two bytes).
        let clips_at = model_with(vec![]).encode_compact(Tolerance::SLIM).len();
        // Clip header: slot, looping, framerate, frames, track count (10 bytes); track: bone, key count.
        let keys_at = clips_at + 10 + 2;
        assert_eq!(be::u16(&good, keys_at), 2, "two rotation keys");
        let list_at = keys_at + 2;
        assert_eq!(
            (be::u16(&good, list_at), be::u16(&good, list_at + 2)),
            (0, 9)
        );

        let mut bad = good.clone();
        bad[list_at..list_at + 2].copy_from_slice(&5u16.to_be_bytes()); // first key must be frame 0
        assert_eq!(
            ViewModel::decode(&bad).unwrap_err(),
            ViewError::BadIndex("clip keys")
        );
        let mut bad = good.clone();
        bad[list_at + 2..list_at + 4].copy_from_slice(&8u16.to_be_bytes()); // last key must be the last frame
        assert_eq!(
            ViewModel::decode(&bad).unwrap_err(),
            ViewError::BadIndex("clip keys")
        );
        let mut bad = good;
        bad[keys_at..keys_at + 2].copy_from_slice(&11u16.to_be_bytes()); // more keys than frames
        assert_eq!(
            ViewModel::decode(&bad).unwrap_err(),
            ViewError::BadIndex("clip keys")
        );
    }

    #[test]
    fn round_trip_and_checks() {
        let v = ViewModel {
            bones: vec![
                Bone {
                    parent: -1,
                    local_rot: [0.0, 0.0, 0.0, 1.0],
                    ..Bone::default()
                },
                Bone {
                    parent: 0,
                    local_rot: [0.0, 0.0, 0.0, 1.0],
                    local_trans: [1.0, 2.0, 3.0],
                    ..Bone::default()
                },
            ],
            view_bone: 0,
            flash_bone: u16::MAX,
            verts: vec![
                SkinVertex {
                    bones: [1, 0, 0, 0],
                    weights: [255, 0, 0, 0],
                    ..SkinVertex::default()
                };
                3
            ],
            indices: vec![0, 1, 2],
            surfaces: vec![Surface {
                first_index: 0,
                index_count: 3,
                texture: 0,
                blend: 0,
                alpha_ref: 0,
            }],
            clips: vec![Clip {
                slot: slot::IDLE,
                set: 0,
                looping: true,
                framerate: 30.0,
                frames: 2,
                tracks: vec![
                    Track {
                        bone: 1,
                        rot: vec![[0, 0, 0, 32767]; 2],
                        trans: vec![[0.5, 0.0, 0.0]],
                        keys: alloc::vec::Vec::new(),
                    },
                    Track {
                        bone: 0,
                        rot: vec![[0, 0, 0, 32767]; 2],
                        trans: vec![],
                        keys: vec![0, 1],
                    },
                ],
            }],
        };
        let b = v.encode();
        assert_eq!(ViewModel::decode(&b).unwrap(), v);
        let mut bad = v.clone();
        bad.bones[0].parent = 1;
        assert_eq!(
            ViewModel::decode(&bad.encode()).unwrap_err(),
            ViewError::BadIndex("bone parent after the bone")
        );
        let mut bad = v.clone();
        bad.clips[0].tracks[1].keys = vec![0, 3];
        assert!(
            ViewModel::decode(&bad.encode()).is_err(),
            "keys past the last frame"
        );
        // Keyed sampling: keys 0, 4, 10; frame 7 is halfway from the second to the third.
        let t = Track {
            keys: vec![0, 4, 10],
            ..Track::default()
        };
        assert_eq!(t.at(7.0, 3), (1, 2, 0.5));
        assert_eq!(t.at(0.0, 3), (0, 1, 0.0));
        assert_eq!(t.at(10.0, 3), (1, 2, 1.0));
        assert_eq!(Track::default().at(2.25, 4), (2, 3, 0.25));
        assert_eq!(Track::default().at(2.25, 1), (0, 0, 0.0));
        assert_eq!(
            quat_from_i16(quat_to_i16([0.0, 0.0, 0.0, 2.0])),
            [0.0, 0.0, 0.0, 1.0]
        );
    }
}

/// Bone names (characters' bones, player animations' tracks): a u16 count, then each name as a u8 length and
/// its bytes.
pub fn encode_names(names: &[alloc::string::String]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(&(names.len() as u16).to_be_bytes());
    for n in names {
        let b = &n.as_bytes()[..n.len().min(255)];
        o.push(b.len() as u8);
        o.extend_from_slice(b);
    }
    o
}

pub fn decode_names(b: &[u8]) -> Option<Vec<alloc::string::String>> {
    let n = usize::from(u16::from_be_bytes([*b.first()?, *b.get(1)?]));
    let mut at = 2;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let len = usize::from(*b.get(at)?);
        let s = core::str::from_utf8(b.get(at + 1..at + 1 + len)?).ok()?;
        out.push(alloc::string::String::from(s));
        at += 1 + len;
    }
    Some(out)
}

/// A map's player characters (`CHAR`), one per team (allies, then axis): each a skinned model and its bone names,
/// as a u32 length and bytes each.
pub fn encode_characters(teams: &[(ViewModel, Vec<alloc::string::String>)]) -> Vec<u8> {
    let mut o = Vec::new();
    for (vm, names) in teams {
        for part in [vm.encode(), encode_names(names)] {
            o.extend_from_slice(&(part.len() as u32).to_be_bytes());
            o.extend_from_slice(&part);
        }
    }
    o
}

pub fn decode_characters(b: &[u8]) -> Vec<(ViewModel, Vec<alloc::string::String>)> {
    let mut out = Vec::new();
    let mut at = 0;
    let part = |at: &mut usize| -> Option<&[u8]> {
        let len = be::u32(b.get(*at..*at + 4)?, 0) as usize;
        let p = b.get(*at + 4..*at + 4 + len)?;
        *at += 4 + len;
        Some(p)
    };
    while at < b.len() {
        let (Some(m), Some(n)) = (part(&mut at), part(&mut at)) else {
            break;
        };
        match (ViewModel::decode(m), decode_names(n)) {
            (Ok(vm), Some(names)) => out.push((vm, names)),
            _ => break,
        }
    }
    out
}

/// Player animation slots (`PANM` clips in `common.wpk`; MW2's `pb_*` animations).
pub mod player_slot {
    pub const STAND: u8 = 0;
    pub const RUN_FORWARD: u8 = 1;
    pub const RUN_BACK: u8 = 2;
    pub const RUN_LEFT: u8 = 3;
    pub const RUN_RIGHT: u8 = 4;
    pub const CROUCH: u8 = 5;
    pub const CROUCH_RUN: u8 = 6;
    pub const SPRINT: u8 = 7;
    pub const PRONE: u8 = 8;
    pub const PRONE_CRAWL: u8 = 9;
}

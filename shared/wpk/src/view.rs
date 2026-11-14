//! The `VIEW` section of `common.wpk`: a weapon's first-person model (view hands + gun as one skeleton) and its
//! animations, ready for CPU skinning on the Wii.
//!
//! Skeleton: bones in parent-first order; each has its bind local pose (what an animation track replaces: rotation
//! replaced, translation = bind + delta, as IW4) and the inverse of its bind world transform (vertices are stored in
//! model space at the bind pose; skinning = posed world × inverse bind). `view_bone` (`tag_view`) is placed at the
//! camera.
//! Animations: each clip is baked to `frames` evenly spaced poses at `framerate`; a track stores one rotation (i16 ×
//! 4, ±32767) and optionally a translation delta per frame, or a single value when constant.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Weapon animation slots carried (IW4 `weapAnimFiles` indices).
pub mod slot {
    pub const IDLE: u8 = 0x1;
    pub const EMPTY_IDLE: u8 = 0x2;
    pub const FIRE: u8 = 0x3;
    pub const LASTSHOT: u8 = 0x5;
    pub const RECHAMBER: u8 = 0x6;
    pub const MELEE: u8 = 0x7;
    pub const RELOAD: u8 = 0x9;
    pub const RELOAD_EMPTY: u8 = 0xA;
    pub const RAISE: u8 = 0xD;
    pub const FIRST_RAISE: u8 = 0xE;
    pub const DROP: u8 = 0x10;
    pub const SPRINT_IN: u8 = 0x17;
    pub const SPRINT_LOOP: u8 = 0x18;
    pub const SPRINT_OUT: u8 = 0x19;
    pub const ADS_FIRE: u8 = 0x20;
    pub const ADS_LASTSHOT: u8 = 0x21;
    pub const ADS_UP: u8 = 0x23;
    pub const ADS_DOWN: u8 = 0x24;
    pub const ALL: [u8; 18] = [
        IDLE,
        EMPTY_IDLE,
        FIRE,
        LASTSHOT,
        RECHAMBER,
        MELEE,
        RELOAD,
        RELOAD_EMPTY,
        RAISE,
        FIRST_RAISE,
        DROP,
        SPRINT_IN,
        SPRINT_LOOP,
        SPRINT_OUT,
        ADS_FIRE,
        ADS_LASTSHOT,
        ADS_UP,
        ADS_DOWN,
    ];
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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub slot: u8,
    pub looping: bool,
    pub framerate: f32,
    pub frames: u16,
    pub tracks: Vec<Track>,
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
                let n = usize::from(c.frames.max(1));
                if usize::from(t.bone) >= nb
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
        let mut o = Vec::new();
        o.u16(self.bones.len() as u16);
        o.u16(self.view_bone);
        o.u16(self.flash_bone);
        o.u16(0);
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
        o.u16(self.clips.len() as u16);
        for c in &self.clips {
            o.u8(c.slot);
            o.u8(u8::from(c.looping));
            o.f32(c.framerate);
            o.u16(c.frames);
            o.u16(c.tracks.len() as u16);
            for t in &c.tracks {
                o.u16(t.bone);
                o.u16(t.rot.len() as u16);
                o.u16(t.trans.len() as u16);
                t.rot.iter().flatten().for_each(|&x| o.u16(x as u16));
                t.trans.iter().flatten().for_each(|&x| o.f32(x));
            }
        }
        o
    }

    pub fn decode(b: &[u8]) -> Result<ViewModel, ViewError> {
        let mut r = Rd { b, at: 0 };
        let nb = usize::from(r.u16()?);
        let view_bone = r.u16()?;
        let flash_bone = r.u16()?;
        r.u16()?;
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
        let nc = usize::from(r.u16()?);
        let mut clips = Vec::with_capacity(nc);
        for _ in 0..nc {
            let slot = r.u8()?;
            let looping = r.u8()? != 0;
            let framerate = r.f32()?;
            let frames = r.u16()?;
            let nt = usize::from(r.u16()?);
            let mut tracks = Vec::with_capacity(nt);
            for _ in 0..nt {
                let bone = r.u16()?;
                let nr = usize::from(r.u16()?);
                let ntr = usize::from(r.u16()?);
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
                tracks.push(Track { bone, rot, trans });
            }
            clips.push(Clip {
                slot,
                looping,
                framerate,
                frames,
                tracks,
            });
        }
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

    pub fn clip(&self, slot: u8) -> Option<&Clip> {
        self.clips.iter().find(|c| c.slot == slot)
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
                looping: true,
                framerate: 30.0,
                frames: 2,
                tracks: vec![Track {
                    bone: 1,
                    rot: vec![[0, 0, 0, 32767]; 2],
                    trans: vec![[0.5, 0.0, 0.0]],
                }],
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
        assert_eq!(
            quat_from_i16(quat_to_i16([0.0, 0.0, 0.0, 2.0])),
            [0.0, 0.0, 0.0, 1.0]
        );
    }
}

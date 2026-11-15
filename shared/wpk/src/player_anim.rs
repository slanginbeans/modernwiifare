//! Player animations (`common.wpk` `PANM`), read in place from the pack (no copy into MEM1): the clips MW2's player
//! animation script plays ([`crate::anim_script`]), tracks by bone name (`PANB`).
//!
//! Layout (big-endian): `u32 count`, `count` × `u32` clip offsets (from the section start). A clip (4-aligned):
//! `u8 looping, u8 0, u16 frames, f32 framerate, u16 tracks, u16 0`, then 12-byte track headers
//! `u16 bone, u16 keys, u16 rots, u16 trans, u32 data offset (from the clip start)`, then each track's data:
//! keys (u16 frame numbers, ascending from 0 to the last frame; none: one sample per frame), rotations
//! (i16 x, y, z of a unit quaternion with w ≥ 0, / 32767; none: the bone keeps its bind rotation), translation deltas
//! (i16 × 1/[`TRANS_SCALE`] units; none: the bind translation). A part with one sample is constant.

use alloc::vec::Vec;

use crate::be::{self, Push};
use crate::view::Clip;

/// Translations are stored in 1/32 units (±1024 units).
pub const TRANS_SCALE: f32 = 32.0;

/// Encode `clips` (tracks from the converter's bake: `rot` empty or the zero quaternion for "bind rotation").
pub fn encode(clips: &[Clip]) -> Vec<u8> {
    let mut b = Vec::new();
    b.u32(clips.len() as u32);
    let table = b.len();
    b.zeros(clips.len() * 4);
    for (ci, c) in clips.iter().enumerate() {
        while !b.len().is_multiple_of(4) {
            b.u8(0);
        }
        let start = b.len();
        be::put_u32(&mut b, table + ci * 4, start as u32);
        b.u8(u8::from(c.looping));
        b.u8(0);
        b.u16(c.frames);
        b.f32(c.framerate);
        b.u16(c.tracks.len() as u16);
        b.u16(0);
        let headers = b.len();
        b.zeros(c.tracks.len() * 12);
        for (ti, t) in c.tracks.iter().enumerate() {
            let rot: Vec<[i16; 3]> = if t.rot.len() == 1 && t.rot[0] == [0; 4] {
                Vec::new()
            } else {
                t.rot
                    .iter()
                    .map(|q| {
                        let s = if q[3] < 0 { -1 } else { 1 };
                        [q[0] * s, q[1] * s, q[2] * s]
                    })
                    .collect()
            };
            let h = headers + ti * 12;
            be::put_u16(&mut b, h, t.bone);
            be::put_u16(&mut b, h + 2, t.keys.len() as u16);
            be::put_u16(&mut b, h + 4, rot.len() as u16);
            be::put_u16(&mut b, h + 6, t.trans.len() as u16);
            let data = (b.len() - start) as u32;
            be::put_u32(&mut b, h + 8, data);
            t.keys.iter().for_each(|&k| b.u16(k));
            rot.iter().flatten().for_each(|&x| b.u16(x as u16));
            for v in &t.trans {
                for &x in v {
                    let q = libm::roundf(x * TRANS_SCALE).clamp(-32767.0, 32767.0) as i16;
                    b.u16(q as u16);
                }
            }
        }
    }
    b
}

/// The clips of a `PANM` section, in place.
#[derive(Clone, Copy, Debug)]
pub struct Anims<'a> {
    b: &'a [u8],
    count: usize,
}

impl<'a> Anims<'a> {
    /// `None` when `b` is cut short or a clip runs past it.
    pub fn parse(b: &'a [u8]) -> Option<Anims<'a>> {
        if b.len() < 4 {
            return None;
        }
        let count = be::u32(b, 0) as usize;
        if b.len() < 4 + count * 4 {
            return None;
        }
        let a = Anims { b, count };
        for i in 0..count {
            let c = a.clip(i)?;
            for t in 0..c.tracks() {
                c.track(t)?;
            }
        }
        Some(a)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clip(&self, i: usize) -> Option<ClipRef<'a>> {
        if i >= self.count {
            return None;
        }
        let at = be::u32(self.b, 4 + i * 4) as usize;
        let b = self.b.get(at..)?;
        if b.len() < 12 {
            return None;
        }
        let tracks = usize::from(be::u16(b, 8));
        (b.len() >= 12 + tracks * 12).then_some(ClipRef { b, tracks })
    }
}

/// One clip.
#[derive(Clone, Copy, Debug)]
pub struct ClipRef<'a> {
    b: &'a [u8],
    tracks: usize,
}

impl<'a> ClipRef<'a> {
    pub fn looping(&self) -> bool {
        self.b[0] != 0
    }

    pub fn frames(&self) -> u16 {
        be::u16(self.b, 2).max(1)
    }

    pub fn framerate(&self) -> f32 {
        be::f32(self.b, 4).max(1e-3)
    }

    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        f32::from(self.frames() - 1) / self.framerate()
    }

    pub fn tracks(&self) -> usize {
        self.tracks
    }

    /// The frame (fractional) at `time_s`: wrapped for a looping clip, held at the ends otherwise.
    pub fn frame_at(&self, time_s: f32) -> f32 {
        let last = f32::from(self.frames() - 1);
        if last <= 0.0 {
            return 0.0;
        }
        let f = time_s * self.framerate();
        if self.looping() {
            let w = f - libm::floorf(f / last) * last;
            w.clamp(0.0, last)
        } else {
            f.clamp(0.0, last)
        }
    }

    pub fn track(&self, i: usize) -> Option<TrackRef<'a>> {
        if i >= self.tracks {
            return None;
        }
        let h = 12 + i * 12;
        let (keys, rots, trans) = (
            usize::from(be::u16(self.b, h + 2)),
            usize::from(be::u16(self.b, h + 4)),
            usize::from(be::u16(self.b, h + 6)),
        );
        let at = be::u32(self.b, h + 8) as usize;
        let len = keys * 2 + rots * 6 + trans * 6;
        let data = self.b.get(at..at + len)?;
        Some(TrackRef {
            bone: be::u16(self.b, h),
            data,
            keys,
            rots,
            trans,
        })
    }
}

/// One bone's track.
#[derive(Clone, Copy, Debug)]
pub struct TrackRef<'a> {
    pub bone: u16,
    data: &'a [u8],
    keys: usize,
    rots: usize,
    trans: usize,
}

impl TrackRef<'_> {
    /// The samples around frame `f` and the blend between them, for a part of `len` samples.
    fn at(&self, f: f32, len: usize) -> (usize, usize, f32) {
        if len <= 1 {
            return (0, 0, 0.0);
        }
        if self.keys == len {
            let key = |k: usize| f32::from(be::u16(self.data, k * 2));
            // The last key at or before f (binary search).
            let (mut lo, mut hi) = (0, len - 1);
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if key(mid) <= f {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let (a, b) = (key(lo), key(hi));
            return (lo, hi, ((f - a) / (b - a).max(1.0)).clamp(0.0, 1.0));
        }
        let f = f.clamp(0.0, (len - 1) as f32);
        let a = libm::floorf(f) as usize;
        (a.min(len - 1), (a + 1).min(len - 1), f - a as f32)
    }

    fn i16x3(&self, at: usize) -> [f32; 3] {
        core::array::from_fn(|k| f32::from(be::u16(self.data, at + k * 2) as i16))
    }

    fn quat(&self, i: usize) -> [f32; 4] {
        let [x, y, z] = self.i16x3(self.keys * 2 + i * 6).map(|v| v / 32767.0);
        let w = libm::sqrtf((1.0 - x * x - y * y - z * z).max(0.0));
        [x, y, z, w]
    }

    /// The rotation at frame `f` (normalized linear blend), or `None` (the bind rotation).
    pub fn rotation(&self, f: f32) -> Option<[f32; 4]> {
        if self.rots == 0 {
            return None;
        }
        let (a, b, w) = self.at(f, self.rots);
        let (qa, mut qb) = (self.quat(a), self.quat(b));
        if a == b || w == 0.0 {
            return Some(qa);
        }
        if (0..4).map(|k| qa[k] * qb[k]).sum::<f32>() < 0.0 {
            qb = qb.map(|v| -v);
        }
        let q: [f32; 4] = core::array::from_fn(|k| qa[k] + (qb[k] - qa[k]) * w);
        let len = libm::sqrtf(q.iter().map(|v| v * v).sum::<f32>()).max(1e-9);
        Some(q.map(|v| v / len))
    }

    /// The translation delta at frame `f`, or `None` (the bind translation).
    pub fn translation(&self, f: f32) -> Option<[f32; 3]> {
        if self.trans == 0 {
            return None;
        }
        let (a, b, w) = self.at(f, self.trans);
        let base = self.keys * 2 + self.rots * 6;
        let (ta, tb) = (self.i16x3(base + a * 6), self.i16x3(base + b * 6));
        Some(core::array::from_fn(|k| {
            (ta[k] + (tb[k] - ta[k]) * w) / TRANS_SCALE
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Track, quat_to_i16};
    use alloc::vec;

    #[test]
    fn round_trips_in_place() {
        let h = core::f32::consts::FRAC_1_SQRT_2;
        let clips = vec![
            Clip {
                looping: true,
                framerate: 30.0,
                frames: 11,
                tracks: vec![
                    // Keyed: identity at 0, 90 degrees about z at 10 (stored with w negated: same rotation).
                    Track {
                        bone: 3,
                        rot: vec![
                            quat_to_i16([0.0, 0.0, 0.0, 1.0]),
                            quat_to_i16([0.0, 0.0, -h, -h]),
                        ],
                        trans: vec![[0.0, 0.0, 0.0], [10.0, -2.5, 0.0]],
                        keys: vec![0, 10],
                    },
                    // Bind rotation, constant translation.
                    Track {
                        bone: 1,
                        rot: vec![[0; 4]],
                        trans: vec![[1.0, 2.0, 3.0]],
                        keys: vec![],
                    },
                ],
                ..Clip::default()
            },
            Clip {
                frames: 1,
                framerate: 30.0,
                ..Clip::default()
            },
        ];
        let b = encode(&clips);
        let a = Anims::parse(&b).unwrap();
        assert_eq!(a.len(), 2);
        let c = a.clip(0).unwrap();
        assert!(c.looping() && c.frames() == 11 && c.tracks() == 2);
        assert!((c.duration() - 10.0 / 30.0).abs() < 1e-6);
        let t = c.track(0).unwrap();
        assert_eq!(t.bone, 3);
        let q = t.rotation(10.0).unwrap();
        assert!((q[2] - h).abs() < 1e-3 && (q[3] - h).abs() < 1e-3, "{q:?}");
        let tr = t.translation(5.0).unwrap();
        assert!((tr[0] - 5.0).abs() < 0.05 && (tr[1] + 1.25).abs() < 0.05);
        let t = c.track(1).unwrap();
        assert_eq!(t.rotation(3.0), None);
        assert_eq!(t.translation(3.0), Some([1.0, 2.0, 3.0]));
        // Looping clips wrap, others hold.
        assert!((c.frame_at(12.0 / 30.0) - 2.0).abs() < 1e-3);
        assert_eq!(a.clip(1).unwrap().frame_at(5.0), 0.0);
        assert!(Anims::parse(&b[..b.len() - 2]).is_none());
    }
}

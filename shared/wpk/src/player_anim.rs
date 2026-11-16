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
        // `fmath`: no square root instruction on the Wii.
        let w = crate::fmath::sqrt(1.0 - x * x - y * y - z * z);
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
        let inv = crate::fmath::rsqrt(q.iter().map(|v| v * v).sum::<f32>().max(1e-18));
        Some(q.map(|v| v * inv))
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

/// How far [`slim`] lets a track stray from the full one, as the Wii plays it. Third-person bodies at 480p: a pixel
/// at 3 m is about 0.3 units across, and 0.25° on a metre-long leg moves its foot about 0.17 units (half a pixel), only
/// on the frames between keys.
pub const SLIM: crate::view::Tolerance = crate::view::Tolerance {
    rot_deg: 0.25,
    trans: 0.1,
};

/// A rotation as `PANM` stores and reads it: x, y, z with w ≥ 0, w rebuilt.
fn stored_rot(q: [i16; 4]) -> [f64; 4] {
    let s = if q[3] < 0 { -1.0 } else { 1.0 };
    let [x, y, z] = [q[0], q[1], q[2]].map(|v| f64::from(v) * s / 32767.0);
    let w = libm::sqrt((1.0 - x * x - y * y - z * z).max(0.0));
    [x, y, z, w]
}

/// [`TrackRef::rotation`]'s blend, in f64.
fn nlerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let s = if (0..4).map(|k| a[k] * b[k]).sum::<f64>() < 0.0 {
        -1.0
    } else {
        1.0
    };
    let q: [f64; 4] = core::array::from_fn(|k| a[k] + (s * b[k] - a[k]) * t);
    let len = libm::sqrt(q.iter().map(|v| v * v).sum::<f64>()).max(1e-12);
    q.map(|v| v / len)
}

/// Angle between two unit quaternions, degrees (`4·asin(|a ∓ b| / 2)`: exact at tiny angles).
fn angle_deg(a: [f64; 4], b: [f64; 4]) -> f64 {
    let d = |s: f64| {
        libm::sqrt(
            (0..4)
                .map(|k| {
                    let d = a[k] - s * b[k];
                    d * d
                })
                .sum::<f64>(),
        )
    };
    4.0 * libm::asin((d(1.0).min(d(-1.0)) / 2.0).min(1.0)) * (180.0 / core::f64::consts::PI)
}

/// A translation as `PANM` stores it (1/[`TRANS_SCALE`] steps).
fn stored_trans(v: [f32; 3]) -> [f64; 3] {
    v.map(|x| {
        f64::from(libm::roundf(x * TRANS_SCALE).clamp(-32767.0, 32767.0)) / f64::from(TRANS_SCALE)
    })
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    libm::sqrt(
        (0..3)
            .map(|k| {
                let d = a[k] - b[k];
                d * d
            })
            .sum::<f64>(),
    )
}

/// `clip` with each track cut to the frames linear interpolation can't rebuild within `tol` (one key list per track,
/// shared by its rotation and translation), and parts that hardly move made constant. Checked against how the Wii
/// reads `PANM`, so a slim pack plays within `tol` of a full one. A track stays one sample per frame when listing
/// keys would not save bytes. The format is unchanged: [`Anims`] already reads keyed tracks.
pub fn slim(clip: &Clip, tol: crate::view::Tolerance) -> Clip {
    let n = usize::from(clip.frames.max(1));
    let tracks = clip.tracks.iter().map(|t| slim_track(t, n, tol)).collect();
    Clip {
        tracks,
        ..clip.clone()
    }
}

fn slim_track(t: &crate::view::Track, n: usize, tol: crate::view::Tolerance) -> crate::view::Track {
    use crate::view::Track;
    let bind = t.rot.len() == 1 && t.rot[0] == [0; 4];
    let (rot, trans) = crate::view::compact::per_frame(t, n);
    let (tol_r, tol_t) = (f64::from(tol.rot_deg), f64::from(tol.trans));
    let r: Vec<[f64; 4]> = if bind {
        Vec::new()
    } else {
        rot.iter().map(|&q| stored_rot(q)).collect()
    };
    let tr: Vec<[f64; 3]> = trans.iter().map(|&v| stored_trans(v)).collect();
    // A part is constant when every frame is within tolerance of the first; it varies only with a sample per frame.
    let rot_moves = r.len() == n && n > 1 && r.iter().any(|&q| angle_deg(r[0], q) > tol_r);
    let trans_moves = tr.len() == n && n > 1 && tr.iter().any(|&v| dist3(tr[0], v) > tol_t);
    let one_rot = || {
        if bind {
            alloc::vec![[0; 4]]
        } else {
            rot.first().copied().into_iter().collect()
        }
    };
    let one_trans = || trans.first().copied().into_iter().collect::<Vec<_>>();
    if !rot_moves && !trans_moves {
        return Track {
            bone: t.bone,
            rot: one_rot(),
            trans: one_trans(),
            keys: Vec::new(),
        };
    }
    let keys = crate::view::compact::reduce(n, |a, b| {
        (a + 1..b).all(|m| {
            let w = (m - a) as f64 / (b - a) as f64;
            (!rot_moves || angle_deg(nlerp(r[a], r[b], w), r[m]) <= tol_r)
                && (!trans_moves || {
                    let v: [f64; 3] =
                        core::array::from_fn(|k| tr[a][k] + (tr[b][k] - tr[a][k]) * w);
                    dist3(v, tr[m]) <= tol_t
                })
        })
    });
    let per = 6 * (usize::from(rot_moves) + usize::from(trans_moves));
    if keys.len() * (2 + per) >= n * per {
        // Every frame, but constant parts still shrink to one sample.
        return Track {
            bone: t.bone,
            rot: if rot_moves { rot } else { one_rot() },
            trans: if trans_moves { trans } else { one_trans() },
            keys: Vec::new(),
        };
    }
    Track {
        bone: t.bone,
        rot: if rot_moves {
            keys.iter().map(|&k| rot[k]).collect()
        } else {
            one_rot()
        },
        trans: if trans_moves {
            keys.iter().map(|&k| trans[k]).collect()
        } else {
            one_trans()
        },
        keys: keys.iter().map(|&k| k as u16).collect(),
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

    /// A run-like clip at 30 fps (a 32-frame cycle): a leg swinging ±30° with a 1-unit bob twice per cycle, a still
    /// bone, a bind bone, a jittery one, and an already keyed one.
    fn walk(frames: u16) -> Clip {
        let n = usize::from(frames);
        let swing = |amp: f32, speed: f32| -> Vec<[i16; 4]> {
            (0..n)
                .map(|f| {
                    let a = amp * libm::sinf(f as f32 * speed) * 0.5;
                    quat_to_i16([libm::sinf(a), 0.0, 0.0, libm::cosf(a)])
                })
                .collect()
        };
        let mut seed = 7u32;
        let mut jitter = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 16) as f32 / 65536.0 - 0.5
        };
        Clip {
            looping: true,
            framerate: 30.0,
            frames,
            tracks: vec![
                Track {
                    bone: 0,
                    rot: swing(0.52, 0.196),
                    trans: (0..n)
                        .map(|f| [0.0, 0.0, libm::sinf(f as f32 * 0.393)])
                        .collect(),
                    keys: vec![],
                },
                Track {
                    bone: 1,
                    rot: vec![quat_to_i16([0.0, 0.0, 0.0, 1.0]); n],
                    trans: vec![[3.0, 0.0, 1.0]; n],
                    keys: vec![],
                },
                Track {
                    bone: 2,
                    rot: vec![[0; 4]],
                    trans: vec![],
                    keys: vec![],
                },
                Track {
                    bone: 3,
                    rot: swing(0.3, 0.05),
                    trans: (0..n).map(|_| [jitter() * 4.0, 0.0, 0.0]).collect(),
                    keys: vec![],
                },
                Track {
                    bone: 4,
                    rot: vec![
                        quat_to_i16([0.0, 0.0, 0.0, 1.0]),
                        quat_to_i16([0.0, 0.3, 0.0, 0.954]),
                    ],
                    trans: vec![],
                    keys: vec![0, frames - 1],
                },
            ],
            ..Clip::default()
        }
    }

    #[test]
    fn slim_clips_are_smaller_and_play_within_tolerance() {
        let full = walk(61);
        let slim_clip = slim(&full, SLIM);
        let (fb, sb) = (
            encode(core::slice::from_ref(&full)),
            encode(core::slice::from_ref(&slim_clip)),
        );
        assert!(sb.len() < fb.len());
        // Without the jittery bone (which keeps every frame), the clip loses over a third: the still bone most, the run
        // cycle's fast leg little (linear keys need most frames of a 30 fps swing at a quarter degree).
        let smooth = |c: &Clip| Clip {
            tracks: c.tracks.iter().filter(|t| t.bone != 3).cloned().collect(),
            ..c.clone()
        };
        let (a, b) = (
            encode(&[smooth(&full)]).len(),
            encode(&[smooth(&slim_clip)]).len(),
        );
        assert!(b * 3 < a * 2, "{a} -> {b}");
        let (fa, sa) = (Anims::parse(&fb).unwrap(), Anims::parse(&sb).unwrap());
        let (fc, sc) = (fa.clip(0).unwrap(), sa.clip(0).unwrap());
        assert_eq!((sc.frames(), sc.looping(), sc.tracks()), (61, true, 5));
        let to64 = |q: [f32; 4]| q.map(f64::from);
        for t in 0..5 {
            let (ft, st) = (fc.track(t).unwrap(), sc.track(t).unwrap());
            assert_eq!(ft.bone, st.bone);
            // Every frame and a half frame between them.
            for h in 0..121 {
                let f = h as f32 * 0.5;
                match (ft.rotation(f), st.rotation(f)) {
                    (Some(a), Some(b)) => {
                        let e = angle_deg(to64(a), to64(b));
                        assert!(
                            e <= f64::from(SLIM.rot_deg) + 0.02,
                            "track {t} frame {f}: {e}°"
                        );
                    }
                    (a, b) => assert_eq!(a.is_none(), b.is_none(), "track {t}"),
                }
                match (ft.translation(f), st.translation(f)) {
                    (Some(a), Some(b)) => {
                        let e = dist3(a.map(f64::from), b.map(f64::from));
                        assert!(
                            e <= f64::from(SLIM.trans) + 1e-4,
                            "track {t} frame {f}: {e}"
                        );
                    }
                    (a, b) => assert_eq!(a.is_none(), b.is_none(), "track {t}"),
                }
            }
        }
        let tr = &slim_clip.tracks;
        // The still bone shrinks to one sample; the bind bone stays bind; jitter keeps every frame.
        assert_eq!(
            (tr[1].rot.len(), tr[1].trans.len(), tr[1].keys.len()),
            (1, 1, 0)
        );
        assert_eq!(tr[2].rot, vec![[0; 4]]);
        assert_eq!(tr[3].trans.len(), 61);
        assert!(
            tr[0].keys.len() > 2 && tr[0].keys.len() < 61,
            "{}",
            tr[0].keys.len()
        );
        // Never larger, even for the worst case.
        let noisy = Clip {
            tracks: vec![tr[3].clone()],
            ..walk(61)
        };
        assert!(encode(&[slim(&noisy, SLIM)]).len() <= encode(&[noisy]).len());
    }
}

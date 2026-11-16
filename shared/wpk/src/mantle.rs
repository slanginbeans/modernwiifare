//! The mantle clips' lengths and root paths (`MNTL`, in `common.wpk`).
//!
//! IW4 moves a mantling player along the root (delta) track of the mantle animation it plays: one of seven "up" clips
//! by ledge height, then one of three "over" clips (IW4L `movement_iw4::MANTLE_XANIM_NAMES`, and the `_fr` set of the
//! fast tree). The converter samples each clip's root track at every frame (IW4L `AnimClip::abs_delta_trans`), so a
//! linear blend between neighbouring samples gives the same point IW4L's keyed lerp does.
//!
//! ```text
//! table: CLIPS x { length_msec u32 (0: clip absent), samples u16 (0: no root track), reserved u16 }
//! then each clip's samples in table order: samples x [f32; 3] (frames 0..=numframes)
//! ```

use crate::be::{self, Push};
use alloc::vec::Vec;

/// Clips per tree (IW4L `MANTLE_XANIM_TREE_SIZE`: root, 7 up, 3 over).
pub const TREE: usize = 11;
/// Slow tree then fast tree.
pub const CLIPS: usize = 2 * TREE;
const ENTRY: usize = 8;

/// One clip as the converter found it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    /// 0 when the zone has no such clip (the runtime falls back to IW4L's flat lengths).
    pub length_msec: u32,
    /// The root's offset from its start at each frame; empty when the clip has no root track.
    pub samples: Vec<[f32; 3]>,
}

/// `clips[fast * TREE + index]`.
pub fn encode(clips: &[Clip]) -> Vec<u8> {
    let mut b = Vec::new();
    for i in 0..CLIPS {
        let c = clips.get(i).cloned().unwrap_or_default();
        b.u32(c.length_msec);
        b.u16(c.samples.len().min(usize::from(u16::MAX)) as u16);
        b.u16(0);
    }
    for i in 0..CLIPS {
        if let Some(c) = clips.get(i) {
            for s in c.samples.iter().take(usize::from(u16::MAX)) {
                for v in s {
                    b.f32(*v);
                }
            }
        }
    }
    b
}

/// Reads [`encode`]'s bytes; `None` when they are short.
pub fn decode(b: &[u8]) -> Option<Vec<Clip>> {
    if b.len() < CLIPS * ENTRY {
        return None;
    }
    let mut at = CLIPS * ENTRY;
    let mut out = Vec::with_capacity(CLIPS);
    for i in 0..CLIPS {
        let length_msec = be::u32(b, i * ENTRY);
        let n = usize::from(be::u16(b, i * ENTRY + 4));
        if b.len() < at + n * 12 {
            return None;
        }
        let samples = (0..n)
            .map(|k| {
                let o = at + k * 12;
                [be::f32(b, o), be::f32(b, o + 4), be::f32(b, o + 8)]
            })
            .collect();
        at += n * 12;
        out.push(Clip {
            length_msec,
            samples,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trip() {
        let mut clips = vec![Clip::default(); CLIPS];
        clips[1] = Clip {
            length_msec: 433,
            samples: vec![[0.0; 3], [1.0, 0.5, 2.0], [16.0, 0.0, 57.0]],
        };
        clips[TREE + 9] = Clip {
            length_msec: 200,
            samples: vec![],
        };
        let b = encode(&clips);
        assert_eq!(decode(&b).unwrap(), clips);
        assert!(decode(&b[..10]).is_none());
    }
}

//! The mantle's lengths and root paths from MW2's mantle clips (`wpk::mantle`, `MNTL` in `common.wpk`).
//!
//! IW4L's `sim::mantle_xanim::MantleXAnimBind` (Apache-2.0) gives pmove each mantle clip's length and its root
//! (delta) track; this is the same over the converter's per-frame samples. A clip the pack lacks falls back to
//! IW4L's `FlatMantleAnimLength` (400 / 200 ms) and `CreateAnimsMantleRootDelta` (straight lines), as there; a clip
//! present without a root track moves nothing, as there.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};
use movement_iw4::{
    CreateAnimsMantleRootDelta, FlatMantleAnimLength, MantleRootDelta, MantleXAnimLength,
};

/// Both mantle trees' clips, slow then fast (`wpk::mantle::CLIPS`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MantlePaths {
    clips: Vec<wpk::mantle::Clip>,
}

impl MantlePaths {
    pub fn new(clips: Vec<wpk::mantle::Clip>) -> MantlePaths {
        MantlePaths { clips }
    }

    /// Reads a `MNTL` section.
    pub fn parse(b: &[u8]) -> Option<MantlePaths> {
        wpk::mantle::decode(b).map(MantlePaths::new)
    }

    fn clip(&self, fast: bool, anim: i32) -> Option<&wpk::mantle::Clip> {
        let i = usize::try_from(anim).ok()?;
        if i >= wpk::mantle::TREE {
            return None;
        }
        self.clips
            .get(usize::from(fast) * wpk::mantle::TREE + i)
            .filter(|c| c.length_msec > 0)
    }

    /// Clips found and clips with a root track (for the log).
    pub fn counts(&self) -> (usize, usize) {
        (
            self.clips.iter().filter(|c| c.length_msec > 0).count(),
            self.clips.iter().filter(|c| !c.samples.is_empty()).count(),
        )
    }
}

impl MantleXAnimLength for MantlePaths {
    fn length_msec(&self, fast: bool, anim: i32) -> i32 {
        match self.clip(fast, anim) {
            Some(c) => c.length_msec as i32,
            None => FlatMantleAnimLength::default().length_msec(fast, anim),
        }
    }
}

impl MantleRootDelta for MantlePaths {
    fn abs_delta(&self, fast: bool, anim: i32, frac: f32) -> [f32; 3] {
        let Some(c) = self.clip(fast, anim) else {
            return CreateAnimsMantleRootDelta.abs_delta(fast, anim, frac);
        };
        let s = &c.samples;
        match s.len() {
            0 => [0.0; 3],
            1 => s[0],
            n => {
                // IW4L samples the keyed track at frame `frac * numframes` and blends linearly.
                let f = frac.clamp(0.0, 1.0) * (n - 1) as f32;
                let a = (f as usize).min(n - 2);
                let t = f - a as f32;
                let (p, q) = (s[a], s[a + 1]);
                [
                    p[0] + (q[0] - p[0]) * t,
                    p[1] + (q[1] - p[1]) * t,
                    p[2] + (q[2] - p[2]) * t,
                ]
            }
        }
    }
}

/// IW4 `ANIM_MT_MANTLE_ROOT` (`anim_iw4::ANIM_MT_NAMES[21]`); the up and over clips follow in tree order.
pub const ANIM_MT_MANTLE_ROOT: u8 = 21;

static PATHS: AtomicPtr<MantlePaths> = AtomicPtr::new(core::ptr::null_mut());

/// Installs the paths every player's pmove uses from now on (once, at the first load: a second call replaces the
/// first and leaks it).
pub fn install(paths: MantlePaths) {
    PATHS.store(Box::into_raw(Box::new(paths)), Ordering::Release);
}

/// The installed paths, if any.
pub fn installed() -> Option<&'static MantlePaths> {
    let p = PATHS.load(Ordering::Acquire);
    // SAFETY: only `install` stores, a pointer from `Box::into_raw` that is never freed.
    unsafe { p.as_ref() }
}

/// What pmove gets: the installed clips, or IW4L's fallbacks when none are.
#[derive(Clone, Copy, Debug)]
pub struct MantleAnims(pub Option<&'static MantlePaths>);

impl MantleAnims {
    pub fn current() -> MantleAnims {
        MantleAnims(installed())
    }
}

impl MantleXAnimLength for MantleAnims {
    fn length_msec(&self, fast: bool, anim: i32) -> i32 {
        match self.0 {
            Some(p) => p.length_msec(fast, anim),
            None => FlatMantleAnimLength::default().length_msec(fast, anim),
        }
    }
}

impl MantleRootDelta for MantleAnims {
    fn abs_delta(&self, fast: bool, anim: i32, frac: f32) -> [f32; 3] {
        match self.0 {
            Some(p) => p.abs_delta(fast, anim, frac),
            None => CreateAnimsMantleRootDelta.abs_delta(fast, anim, frac),
        }
    }
}

/// The third-person movement type of a mantle at `time_ms` into it: IW4's `ANIM_MT_MANTLE_ROOT` plus the clip
/// playing (IW4L `mantle::active_xanim`: the up clip, then the over clip). `None` when not mantling.
pub fn anim_move_type(ps: &playerstate_iw4::PlayerState) -> Option<u8> {
    if ps.pm_flags & playerstate_iw4::pm_flags::MANTLE == 0 {
        return None;
    }
    let anim = movement_iw4::mantle::active_xanim(ps, &MantleAnims::current());
    u8::try_from(anim).ok().map(|a| ANIM_MT_MANTLE_ROOT + a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn paths() -> MantlePaths {
        let mut clips = vec![wpk::mantle::Clip::default(); wpk::mantle::CLIPS];
        // Up 57: a curve that rises before it moves in.
        clips[1] = wpk::mantle::Clip {
            length_msec: 500,
            samples: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 40.0], [16.0, 0.0, 57.0]],
        };
        // Over high: present, no root track.
        clips[8] = wpk::mantle::Clip {
            length_msec: 300,
            samples: vec![],
        };
        MantlePaths::new(clips)
    }

    #[test]
    fn samples_blend_between_frames_as_iw4l() {
        let p = paths();
        assert_eq!(p.abs_delta(false, 1, 0.0), [0.0; 3]);
        assert_eq!(p.abs_delta(false, 1, 0.5), [2.0, 0.0, 40.0]);
        assert_eq!(p.abs_delta(false, 1, 0.75), [9.0, 0.0, 48.5]);
        assert_eq!(p.abs_delta(false, 1, 1.0), [16.0, 0.0, 57.0]);
        assert_eq!(p.abs_delta(false, 1, 2.0), [16.0, 0.0, 57.0]);
        assert_eq!(p.length_msec(false, 1), 500);
    }

    #[test]
    fn falls_back_like_mantle_xanim_bind() {
        let p = paths();
        // A clip present without a root track moves nothing.
        assert_eq!(p.abs_delta(false, 8, 0.5), [0.0; 3]);
        assert_eq!(p.length_msec(false, 8), 300);
        // An absent clip: IW4L's straight lines and flat lengths.
        for (fast, anim) in [(false, 2), (true, 1), (true, 9)] {
            assert_eq!(
                p.abs_delta(fast, anim, 0.3),
                CreateAnimsMantleRootDelta.abs_delta(fast, anim, 0.3)
            );
            assert_eq!(
                p.length_msec(fast, anim),
                FlatMantleAnimLength::default().length_msec(fast, anim)
            );
        }
        let none = MantleAnims(None);
        assert_eq!(
            none.abs_delta(false, 1, 0.5),
            CreateAnimsMantleRootDelta.abs_delta(false, 1, 0.5)
        );
    }
}

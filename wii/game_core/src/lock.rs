//! The Javelin's lock on a point (MW2's `maps/mp/_javelin.gsc` `JavelinUsageLoop`, point targets; IW4L
//! `sim::weapon_lock::WeaponLock` holds the result): aim down sights fully; every 25 ms the eye's trace ahead
//! (15000 units, not the sky) gathers a point, each within 400 units of the points' average, four misses in a row
//! starting over; with 12 points their average is the target and the lock runs while it stays in the reticle's
//! circle, finalizing after 1150 ms. A target nearer than 1100 units is too close to fire on (IW4L `too_close`).
//! The lock is top attack (`WeaponLockFinalize(point, (0,0,0), true)`).
//!
//! The script's numbers are MW2's stock ones (not read from the user's script yet). The reticle circle (`
//! WorldPointInReticle_Circle(point, 65, r)`: r pixels at a 65° view) is taken as the angle r pixels make on a
//! 480-line screen at that view. The Javelin's vehicle targets (killstreak aircraft) are not in this build.

use alloc::vec::Vec;

const LOCK_LENGTH_MS: u32 = 1150;
const GATHER_DELAY_MS: u32 = 25;
const ADS_DELAY_MS: u32 = 100;
const MAX_POINT_PEER_DIST: f32 = 400.0;
const POINTS_NEEDED_TO_START_LOCK: usize = 12;
const MAX_MISSES: u8 = 4;
const MIN_DIST: f32 = 1100.0;
const TRACE_RANGE: f32 = 15000.0;
/// The script loop's `wait 0.05`.
const LOOP_MS: u32 = 50;

/// Cosine of the angle `pixels` off the centre of a 480-line screen at a 65° view.
fn reticle_cos(pixels: f32) -> f32 {
    let focal = 240.0 / libm::tanf(32.5f32.to_radians());
    libm::cosf(libm::atanf(pixels / focal))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JavelinLock {
    /// 0 idle, 1 scanning, 2 locking, 3 locked (the script's `javelinStage`).
    pub stage: u8,
    points: Vec<[f32; 3]>,
    misses: u8,
    last_gather_ms: u32,
    last_out_of_ads_ms: u32,
    lock_start_ms: u32,
    next_ms: u32,
    pub target: Option<[f32; 3]>,
    pub too_close: bool,
}

/// What the lock did this step (for its sounds and the HUD).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockEvent {
    /// Locking started (`javelin_clu_aquiring_lock`).
    Acquiring,
    /// Locked (`javelin_clu_lock`).
    Locked,
}

impl JavelinLock {
    fn reset(&mut self) {
        let last_out = self.last_out_of_ads_ms;
        *self = JavelinLock {
            last_out_of_ads_ms: last_out,
            next_ms: self.next_ms,
            ..JavelinLock::default()
        };
    }

    /// The point to steer to when locked and not too close (IW4L `WeaponLock::can_fire`).
    pub fn locked(&self) -> Option<[f32; 3]> {
        (self.stage == 3 && !self.too_close)
            .then_some(self.target)
            .flatten()
    }

    /// One frame at `now`: `javelin` whether a Javelin is out, `ads` the aim fraction, the eye and view forward,
    /// `feet` the player's origin, `trace(from, to)` the world's hit point if it struck a real surface (not the sky
    /// or a default surface: the script's `EyeTraceForward`).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        now: u32,
        javelin: bool,
        ads: f32,
        eye: [f32; 3],
        forward: [f32; 3],
        feet: [f32; 3],
        mut trace: impl FnMut([f32; 3], [f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<LockEvent> {
        if (now.wrapping_sub(self.next_ms) as i32) < 0 {
            return None;
        }
        self.next_ms = now.wrapping_add(LOOP_MS);
        if !javelin {
            self.reset();
            return None;
        }
        if ads < 0.95 {
            self.last_out_of_ads_ms = now;
            self.reset();
            return None;
        }
        let dist = |a: [f32; 3], b: [f32; 3]| {
            libm::sqrtf((0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum::<f32>())
        };
        let in_reticle = |p: [f32; 3], pixels: f32| {
            let d: [f32; 3] = core::array::from_fn(|k| p[k] - eye[k]);
            let l = libm::sqrtf(d.iter().map(|x| x * x).sum::<f32>());
            l > 1e-3
                && (d[0] * forward[0] + d[1] * forward[1] + d[2] * forward[2]) / l
                    >= reticle_cos(pixels)
        };
        if self.stage == 0 {
            self.stage = 1;
        }
        if self.stage == 1 {
            if self.misses >= MAX_MISSES {
                self.reset();
                return None;
            }
            if now.wrapping_sub(self.last_out_of_ads_ms) < ADS_DELAY_MS
                || now.wrapping_sub(self.last_gather_ms) < GATHER_DELAY_MS
            {
                return None;
            }
            self.last_gather_ms = now;
            let end = core::array::from_fn(|k| eye[k] + forward[k] * TRACE_RANGE);
            let Some(p) = trace(eye, end) else {
                self.misses += 1;
                return None;
            };
            self.too_close = dist(feet, p) < MIN_DIST;
            if self.too_close {
                return None;
            }
            if !self.points.is_empty() && dist(average(&self.points), p) > MAX_POINT_PEER_DIST {
                self.misses += 1;
                return None;
            }
            self.points.push(p);
            self.misses = 0;
            if self.points.len() < POINTS_NEEDED_TO_START_LOCK {
                return None;
            }
            self.target = Some(average(&self.points));
            self.points.clear();
            self.lock_start_ms = now;
            self.stage = 2;
            return Some(LockEvent::Acquiring);
        }
        let Some(t) = self.target else {
            self.reset();
            return None;
        };
        if !in_reticle(t, 45.0) {
            self.reset();
            return None;
        }
        self.too_close = dist(feet, t) < MIN_DIST;
        if self.stage == 2 && now.wrapping_sub(self.lock_start_ms) >= LOCK_LENGTH_MS {
            self.stage = 3;
            return Some(LockEvent::Locked);
        }
        None
    }
}

fn average(p: &[[f32; 3]]) -> [f32; 3] {
    let n = p.len().max(1) as f32;
    core::array::from_fn(|k| p.iter().map(|v| v[k]).sum::<f32>() / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steady_aim_locks_after_the_gather_and_the_lock_length() {
        let mut l = JavelinLock::default();
        let (eye, fwd, feet) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0], [0.0; 3]);
        // A wall 2000 units ahead.
        let wall = |_: [f32; 3], _: [f32; 3]| Some([2000.0, 0.0, 60.0]);
        let mut acquiring = None;
        let mut locked = None;
        // Out of ADS first.
        l.update(0, true, 0.0, eye, fwd, feet, wall);
        for t in (16..6000).step_by(16) {
            match l.update(t, true, 1.0, eye, fwd, feet, wall) {
                Some(LockEvent::Acquiring) => acquiring = Some(t),
                Some(LockEvent::Locked) => locked = Some(t),
                None => {}
            }
        }
        let (a, k) = (acquiring.unwrap(), locked.unwrap());
        // 12 points a script loop apart (50 ms, here on 16 ms frames: every 64) after the 100 ms ADS delay, then
        // 1150 ms.
        assert!((600..=900).contains(&a), "acquiring at {a}");
        assert!(
            (k - a).abs_diff(LOCK_LENGTH_MS) <= 50,
            "locked {} ms later",
            k - a
        );
        assert_eq!(l.locked(), Some([2000.0, 0.0, 60.0]));
        // Looking away loses it.
        l.update(7000, true, 1.0, eye, [0.0, 1.0, 0.0], feet, wall);
        assert_eq!(l.locked(), None);
        // Too close: never a lock to fire on.
        let mut l = JavelinLock::default();
        let near = |_: [f32; 3], _: [f32; 3]| Some([500.0, 0.0, 60.0]);
        for t in (0..6000).step_by(16) {
            l.update(t, true, 1.0, eye, fwd, feet, near);
        }
        assert_eq!(l.locked(), None);
    }
}

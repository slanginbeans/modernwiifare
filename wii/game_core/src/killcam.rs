//! MW2's killcam: after your death, the last seconds replayed from your killer's eye; at the match's end, the final
//! killcam of the last kill. IW4 replays its server archive; we keep our own: a ring of every player's pose (and
//! whatever else the caller samples) every server frame ([`Ring`]), replayed in between frames.
//!
//! The timing is IW4L's `killcam_iw4` (MW2's `_damage.gsc` / `_killcam.gsc`, ported there): the death sequence
//! ([`killcam_iw4::DeathSequence`]: a quarter second to watch yourself die, a double tap of use to cancel, the
//! death delay), the camtime chain and window ([`killcam_iw4::camtime`], [`killcam_iw4::plan_window`], trimmed to the
//! archive with [`killcam_iw4::recalc_after_first_frame`]), the skip ([`killcam_iw4::SkipEdge`]: a fresh press of use)
//! and the final killcam's start ([`killcam_iw4::FinalKillcamSequence`]) with `doFinalKillCamFX`'s slow motion.

use alloc::vec::Vec;

use killcam_iw4::{
    CamtimeInput, DeathConfig, DeathOutput, DeathSequence, Recalc, SkipEdge, WindowPlan,
};

/// One sample a server frame (IW4's archive is per server frame too).
pub const PERIOD_MS: u32 = killcam_iw4::SERVER_FRAME_MS as u32;
/// History kept: a killcam reaches 5 s of camtime and the 1.5 s death delay back (the final killcam, 4 s and the
/// time since the last kill: we start it at once).
pub const HISTORY_MS: u32 = 10_000;
/// Frames in [`HISTORY_MS`].
pub const FRAMES: usize = (HISTORY_MS / PERIOD_MS) as usize;

/// A player as the killcam needs it: where they stand, their eye height and view, alive, and the weapon in hand.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Eye {
    pub origin: [f32; 3],
    pub view_height: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub alive: bool,
    /// The `WEAP` record in hand (`u32::MAX`: none).
    pub weapon: u32,
}

/// The shortest turn from `a` to `b`, degrees.
pub fn angle_delta(a: f32, b: f32) -> f32 {
    let d = b - a;
    d - libm::floorf((d + 180.0) / 360.0) * 360.0
}

impl Eye {
    pub fn eye(&self) -> [f32; 3] {
        let o = self.origin;
        [o[0], o[1], o[2] + self.view_height]
    }

    /// Between `self` and `b` at `t` (0..1): positions and angles blend, the rest is the nearer frame's.
    pub fn lerp(&self, b: &Eye, t: f32) -> Eye {
        let near = if t < 0.5 { self } else { b };
        Eye {
            origin: core::array::from_fn(|k| self.origin[k] + (b.origin[k] - self.origin[k]) * t),
            view_height: self.view_height + (b.view_height - self.view_height) * t,
            pitch: self.pitch + angle_delta(self.pitch, b.pitch) * t,
            yaw: self.yaw + angle_delta(self.yaw, b.yaw) * t,
            ..*near
        }
    }
}

/// A ring of frames: each a time and one `T` per player slot, over storage the caller provides (the Wii's in MEM2).
pub struct Ring<'a, T> {
    times: &'a mut [u32],
    items: &'a mut [T],
    slots: usize,
    /// The next frame to write, and how many are filled.
    head: usize,
    len: usize,
    next_ms: Option<u32>,
}

impl<'a, T: Copy> Ring<'a, T> {
    /// A ring over `times` (one per frame) and `items` (`slots` per frame).
    pub fn new(times: &'a mut [u32], items: &'a mut [T], slots: usize) -> Ring<'a, T> {
        let slots = slots.max(1);
        let frames = times.len().min(items.len() / slots);
        Ring {
            times: &mut times[..frames],
            items: &mut items[..frames * slots],
            slots,
            head: 0,
            len: 0,
            next_ms: None,
        }
    }

    /// Bytes a ring of `frames` frames of `slots` items takes.
    pub const fn bytes(frames: usize, slots: usize) -> usize {
        frames * (4 + slots * core::mem::size_of::<T>())
    }

    pub fn slots(&self) -> usize {
        self.slots
    }

    pub fn frames(&self) -> usize {
        self.times.len()
    }

    /// Forget everything (a new match).
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.next_ms = None;
    }

    /// Whether a frame is due at `now` (every [`PERIOD_MS`]).
    pub fn due(&self, now: u32) -> bool {
        self.next_ms
            .is_none_or(|n| (now.wrapping_sub(n) as i32) >= 0)
    }

    /// Record a frame at `now`: `sample(slot)` for each slot.
    pub fn push(&mut self, now: u32, mut sample: impl FnMut(usize) -> T) {
        if self.times.is_empty() {
            return;
        }
        let h = self.head;
        self.times[h] = now;
        for s in 0..self.slots {
            self.items[h * self.slots + s] = sample(s);
        }
        self.head = (h + 1) % self.times.len();
        self.len = (self.len + 1).min(self.times.len());
        // Keep the period's phase; after a stall, start again from now.
        self.next_ms = Some(match self.next_ms {
            Some(n) if now.wrapping_sub(n) < PERIOD_MS => n.wrapping_add(PERIOD_MS),
            _ => now.wrapping_add(PERIOD_MS),
        });
    }

    /// Frame `k` from the oldest (0) on.
    fn frame(&self, k: usize) -> (u32, &[T]) {
        let n = self.times.len();
        let i = (self.head + n - self.len + k) % n;
        (
            self.times[i],
            &self.items[i * self.slots..(i + 1) * self.slots],
        )
    }

    /// When the oldest and newest frames were recorded.
    pub fn span(&self) -> Option<(u32, u32)> {
        (self.len > 0).then(|| (self.frame(0).0, self.frame(self.len - 1).0))
    }

    /// The frames around archive time `t` and how far between them `t` is (0..1); clamped to the first and last
    /// frames.
    pub fn at(&self, t: u32) -> Option<(&[T], &[T], f32)> {
        if self.len == 0 {
            return None;
        }
        let after = |a: u32, b: u32| (a.wrapping_sub(b) as i32) > 0;
        let (t0, first) = self.frame(0);
        if !after(t, t0) {
            return Some((first, first, 0.0));
        }
        // Binary search for the last frame at or before `t`.
        let (mut lo, mut hi) = (0, self.len - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if after(self.frame(mid).0, t) {
                hi = mid - 1;
            } else {
                lo = mid;
            }
        }
        let (ta, a) = self.frame(lo);
        if lo + 1 >= self.len {
            return Some((a, a, 0.0));
        }
        let (tb, b) = self.frame(lo + 1);
        let span = tb.wrapping_sub(ta).max(1) as f32;
        Some((a, b, (t.wrapping_sub(ta) as f32 / span).clamp(0.0, 1.0)))
    }
}

/// A killcam being shown: whose eye, the archive time it starts at, how long it lasts (in archive time), and the
/// final killcam's slow motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Killcam {
    pub killer: usize,
    pub victim: usize,
    /// When the death happened (archive time).
    pub death_ms: u32,
    /// Seconds shown before the death and after it (`camtime`, `postdelay`).
    pub camtime: f32,
    pub postdelay: f32,
    /// The final killcam (`level.showingFinalKillcam`: slow motion, no skip).
    pub final_kill: bool,
    /// When it started playing (wall time).
    pub start_ms: u32,
    skip: SkipEdge,
}

/// IW4's `setSlowMotion` blend: the time scale `w` seconds into the final killcam (`doFinalKillCamFX`: from a second
/// before the death down to a quarter over at most a second, held half a second, back up over a second).
fn slow_scale(camtime: f32, w: f32) -> f32 {
    let into = camtime.min(1.0);
    let s = (camtime - 1.0).max(0.0);
    if w < s {
        1.0
    } else if w < s + into {
        1.0 - 0.75 * (w - s) / into
    } else if w < s + into + 0.5 {
        0.25
    } else if w < s + into + 1.5 {
        0.25 + 0.75 * (w - s - into - 0.5)
    } else {
        1.0
    }
}

impl Killcam {
    /// Plan a killcam of `killer` killing `victim` at archive time `death_ms`, starting at `now` (the death was
    /// `predelay` seconds ago), against the archive the ring holds (`span`): the camtime chain, the window, and the
    /// first frame's trim (`recalc_after_first_frame`). None when there is too little recorded to show the death.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        killer: usize,
        victim: usize,
        death_ms: u32,
        now: u32,
        predelay: f32,
        time_until_respawn: f32,
        final_kill: bool,
        maxtime: Option<f32>,
        span: Option<(u32, u32)>,
    ) -> Option<Killcam> {
        let (camtime, _) = killcam_iw4::camtime(&CamtimeInput {
            now_ms: now as i32,
            weapon: "",
            killcam_entity_start_time: 0,
            predelay,
            showing_final_killcam: final_kill,
            time_until_respawn,
            scr_killcam_time: None,
        });
        let postdelay = killcam_iw4::postdelay(None);
        let WindowPlan::Show(w) = killcam_iw4::plan_window(camtime, postdelay, predelay, maxtime)
        else {
            return None;
        };
        // How far back the archive reaches from now.
        let (oldest, _) = span?;
        let archive = (now.wrapping_sub(oldest) as f32 / 1000.0).min(w.killcamoffset);
        match killcam_iw4::recalc_after_first_frame(archive, w.killcamoffset, predelay, w.postdelay)
        {
            Recalc::Cancel => None,
            Recalc::ArchiveGrew => None,
            Recalc::Continue { camtime, .. } => Some(Killcam {
                killer,
                victim,
                death_ms,
                camtime,
                postdelay: w.postdelay,
                final_kill,
                start_ms: now,
                skip: SkipEdge::new(),
            }),
        }
    }

    /// Seconds of archive played `wall_ms` after the start (slow motion in the final killcam).
    fn played(&self, wall_ms: u32) -> f32 {
        let wall = wall_ms as f32 / 1000.0;
        if !self.final_kill {
            return wall;
        }
        // The slow motion's scale integrated in 10 ms steps.
        let mut t = 0.0;
        let mut w = 0.0;
        while w < wall {
            let dt = (wall - w).min(0.01);
            t += slow_scale(self.camtime, w + dt * 0.5) * dt;
            w += dt;
        }
        t
    }

    /// The archive time shown at `now`.
    pub fn replay_ms(&self, now: u32) -> u32 {
        let start = self.death_ms.wrapping_sub((self.camtime * 1000.0) as u32);
        start.wrapping_add((self.played(now.wrapping_sub(self.start_ms)) * 1000.0) as u32)
    }

    /// A clock for the replay's animations: wall time at the start plus the archive played (it only moves forward,
    /// and slows with the final killcam).
    pub fn anim_ms(&self, now: u32) -> u32 {
        self.start_ms
            .wrapping_add((self.played(now.wrapping_sub(self.start_ms)) * 1000.0) as u32)
    }

    /// Whether it has played out (`waittillKillcamOver`: `killcamlength - 0.05`).
    pub fn over(&self, now: u32) -> bool {
        self.played(now.wrapping_sub(self.start_ms))
            >= self.camtime + self.postdelay - killcam_iw4::SERVER_FRAME_SECONDS
    }

    /// The countdown MW2 shows (`kc_timer setTenthsTimer( camtime )`), in tenths of a second.
    pub fn timer_tenths(&self, now: u32) -> u32 {
        let left = self.camtime - self.played(now.wrapping_sub(self.start_ms));
        (left.max(0.0) * 10.0) as u32
    }

    /// Feed the use button: true on a fresh press (`waitSkipKillcamButton`; never in the final killcam).
    pub fn skip(&mut self, use_held: bool) -> bool {
        !self.final_kill && self.skip.tick(use_held) == killcam_iw4::CancelTick::Fired
    }
}

/// What your death does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Still watching yourself die (or the killcam).
    Wait,
    /// Respawn now.
    Respawn,
}

/// Your death: MW2's death sequence, then the killcam (unless cancelled, a suicide, or killcams off), then the
/// respawn (MW2's free-for-all and team deathmatch: no respawn delay, `scr_player_respawndelay` 0).
pub struct Death {
    pub at_ms: u32,
    pub killer: Option<usize>,
    pub victim: usize,
    seq: DeathSequence,
    pub cam: Option<Killcam>,
    done: bool,
    /// The death sequence's outputs so far (for the log).
    pub outputs: Vec<DeathOutput>,
}

impl Death {
    /// `victim` died at `now`, killed by `killer` (None: their own doing); `killcam`: killcams allowed
    /// (`scr_game_allowkillcam`); `copycat`: the victim's Copycat offers the killer's class.
    pub fn start(
        now: u32,
        victim: usize,
        killer: Option<usize>,
        killcam: bool,
        copycat: bool,
    ) -> Death {
        let (seq, log) = DeathSequence::start(
            now as i32,
            DeathConfig {
                do_killcam: killer.is_some(),
                level_killcam: killcam,
                victim_has_copycat: copycat,
                time_until_spawn: 0.0,
                ..DeathConfig::default()
            },
        );
        Death {
            at_ms: now,
            killer,
            victim,
            seq,
            cam: None,
            done: false,
            outputs: log.iter().collect(),
        }
    }

    /// Advance to `now` with the use button `use_held` and the ring's `span`: the death sequence, then the killcam.
    pub fn update(&mut self, now: u32, use_held: bool, span: Option<(u32, u32)>) -> Next {
        if self.done {
            return Next::Respawn;
        }
        if let Some(c) = self.cam.as_mut() {
            if c.skip(use_held) || c.over(now) {
                self.done = true;
                return Next::Respawn;
            }
            return Next::Wait;
        }
        let log = self.seq.advance(now as i32, use_held, &[]);
        for o in log.iter() {
            self.outputs.push(o);
            match o {
                DeathOutput::StartKillcam(s) => {
                    self.cam = self.killer.and_then(|k| {
                        Killcam::plan(
                            k,
                            self.victim,
                            self.at_ms,
                            now,
                            s.predelay,
                            s.time_until_respawn,
                            false,
                            None,
                            span,
                        )
                    });
                    if self.cam.is_none() {
                        self.done = true;
                    }
                }
                DeathOutput::NoKillcam(_) => self.done = true,
                _ => {}
            }
        }
        if self.done { Next::Respawn } else { Next::Wait }
    }

    /// The killcam playing at `now`, if any.
    pub fn watching(&self) -> Option<&Killcam> {
        self.cam.as_ref().filter(|_| !self.done)
    }
}

/// The final killcam of the match's last kill (`killer` killing `victim` at archive time `death_ms`), starting at
/// `now` (IW4L `FinalKillcamSequence::Start`: predelay the time since the death, no respawn, the long maxtime).
pub fn final_killcam(
    killer: usize,
    victim: usize,
    death_ms: u32,
    now: u32,
    span: Option<(u32, u32)>,
) -> Option<Killcam> {
    let predelay = now.wrapping_sub(death_ms) as f32 / 1000.0;
    Killcam::plan(
        killer,
        victim,
        death_ms,
        now,
        predelay,
        0.0,
        true,
        Some(killcam_iw4::FINAL_KILLCAM_MAXTIME_SECONDS),
        span,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn eye(x: f32, yaw: f32) -> Eye {
        Eye {
            origin: [x, 0.0, 0.0],
            view_height: 60.0,
            yaw,
            alive: true,
            ..Eye::default()
        }
    }

    #[test]
    fn the_ring_keeps_the_last_seconds_and_blends_between_frames() {
        let mut times = vec![0u32; 8];
        let mut items = vec![Eye::default(); 16];
        let mut r = Ring::new(&mut times, &mut items, 2);
        assert_eq!((r.frames(), r.slots()), (8, 2));
        assert!(r.at(0).is_none());
        // Frames every 50 ms from t = 1000: slot 0 walks +10 a frame and turns 350 -> 10 (through 0).
        let mut t = 1000;
        for k in 0..12 {
            if r.due(t) {
                r.push(t, |s| {
                    eye(k as f32 * 10.0 + s as f32, if k < 6 { 350.0 } else { 10.0 })
                });
            }
            t += 50;
            assert!(!r.due(t - 25));
        }
        // Eight kept: from k = 4 (t = 1200) to k = 11 (t = 1550).
        assert_eq!(r.span(), Some((1200, 1550)));
        let (a, b, f) = r.at(1325).unwrap();
        assert_eq!((a[0].origin[0], b[0].origin[0]), (60.0, 70.0));
        assert!((f - 0.5).abs() < 1e-6);
        let m = a[0].lerp(&b[0], f);
        assert!((m.origin[0] - 65.0).abs() < 1e-4);
        // 350 -> 10 turns 20 degrees through north, not 340 back.
        let (a5, b6, _) = r.at(1275).unwrap();
        let turn = a5[0].lerp(&b6[0], 0.5).yaw;
        assert!((turn - 360.0).abs() < 1e-3, "{turn}");
        assert_eq!(a[1].origin[0], 61.0);
        // Before the oldest frame: the oldest; after the newest: the newest.
        assert_eq!(r.at(0).unwrap().0[0].origin[0], 40.0);
        assert_eq!(r.at(9999).unwrap().0[0].origin[0], 110.0);
        assert_eq!(
            Ring::<Eye>::bytes(240, 18),
            240 * (4 + 18 * core::mem::size_of::<Eye>())
        );
    }

    #[test]
    fn a_death_plays_mw2s_killcam_then_respawns() {
        // Archive from t = 0; death at 10 s, by fighter 3.
        let span = |now: u32| Some((0, now));
        let mut d = Death::start(10_000, 0, Some(3), true, false);
        let mut t = 10_000;
        // The death sequence: 0.25 + 0.25 + 1.0 s, then the killcam starts (predelay 1.5 s).
        while d.watching().is_none() {
            assert_eq!(d.update(t, false, span(t)), Next::Wait);
            t += 10;
            assert!(t < 12_000, "the killcam never started");
        }
        let c = *d.watching().unwrap();
        assert!((1500..1600).contains(&c.start_ms.wrapping_sub(10_000)));
        // MW2's camtime with no respawn wait: 5 s; 2 s after the death. (The first frame trims a server frame.)
        assert!((c.camtime - 4.95).abs() < 1e-3, "{}", c.camtime);
        assert_eq!(c.postdelay, 2.0);
        assert_eq!(c.replay_ms(c.start_ms), 10_000 - 4950);
        assert_eq!(c.timer_tenths(c.start_ms), 49);
        // It plays at real speed and ends after camtime + postdelay.
        let end = c.start_ms + 6900;
        assert_eq!(c.replay_ms(c.start_ms + 1000), 10_000 - 3950);
        while t < end {
            assert_eq!(d.update(t, false, span(t)), Next::Wait);
            t += 16;
        }
        assert_eq!(d.update(end + 16, false, span(end)), Next::Respawn);
        // Skipping: a fresh press of use ends it (held from before doesn't).
        let mut d = Death::start(10_000, 0, Some(3), true, false);
        let mut t = 10_000;
        while d.watching().is_none() {
            d.update(t, false, span(t));
            t += 16;
        }
        assert_eq!(d.update(t, true, span(t)), Next::Wait, "drain first");
        assert_eq!(d.update(t + 16, false, span(t)), Next::Wait);
        assert_eq!(d.update(t + 32, true, span(t)), Next::Respawn);
        // No killcam for a suicide or with killcams off: respawn once the death delay is over.
        for mut d in [
            Death::start(10_000, 0, None, true, false),
            Death::start(10_000, 0, Some(2), false, false),
        ] {
            let mut t = 10_000;
            while d.update(t, false, span(t)) == Next::Wait {
                t += 10;
                assert!(t < 12_000);
            }
            assert!((1500..1600).contains(&(t - 10_000)), "{t}");
            assert!(d.watching().is_none());
        }
        // Too little archive (the match just started): no killcam.
        let mut d = Death::start(1000, 0, Some(1), true, false);
        let mut t = 1000;
        while d.update(t, false, Some((2400, t))) == Next::Wait {
            t += 10;
        }
        assert!(d.cam.is_none());
    }

    #[test]
    fn the_final_killcam_slows_around_the_kill() {
        let c = final_killcam(2, 0, 20_000, 25_000, Some((10_000, 25_000))).unwrap();
        // 4 s of camtime (less the first frame), no skip.
        assert!((c.camtime - 3.95).abs() < 1e-3);
        let mut c2 = c;
        assert!(!c2.skip(false) && !c2.skip(true));
        // Real speed until a second before the death, a quarter around it.
        let a = c.replay_ms(c.start_ms + 2000);
        assert!(a.abs_diff(20_000 - 3950 + 2000) <= 2, "{a}");
        let slow = c.replay_ms(c.start_ms + 4100) - c.replay_ms(c.start_ms + 4000);
        assert!((20..30).contains(&slow), "{slow}");
        // It lasts longer in wall time than camtime + postdelay.
        assert!(!c.over(c.start_ms + 5950));
        let mut w = c.start_ms;
        while !c.over(w) {
            w += 10;
        }
        assert!(w - c.start_ms > 6500, "{}", w - c.start_ms);
    }
}

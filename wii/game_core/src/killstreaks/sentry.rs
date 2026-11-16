//! The sentry gun (`_autosentry.gsc` for its life, IW4L `sim/src/script/host/turrets.rs` for the turret).
//!
//! The reward is carried first: a preview stands where IW4L's sentry placement ([`super::placement`]) would put it,
//! and fire sets it down where the placement is valid. Placed, it runs IW4L's turret `advance`: the nearest enemy in
//! range (the weapon's bullet range), inside its arcs and in sight from the muzzle is the target; the aim turns towards
//! it at 360Â° a second and is on target within 2Â°; with no target it scans 30Â° a second. On target it fires MW2's
//! `sentry_burstFireStart`: a 0.05 s spin-up, bursts of `burstMin..burstMax` shots at the weapon's fire time
//! (`sentry_minigun_mp`), pauses of `pauseMin..pauseMax`. Each shot adds its fire time to the heat; past
//! `SENTRY_OVERHEAT_TIME` it stops until the heat is gone (`sentry_heatMonitor`: a second of heat a second). It goes
//! after `SENTRY_TIME_OUT`.

use alloc::vec::Vec;

use super::{Gunline, Note, Streak, Streaks, dist2};
use crate::arena::{Arena, Event};
use crate::collision::MapClip;
use crate::gun::{MASK_SHOT, Shot};

/// IW4L `turrets`: turn and scan rates, on-target cone, muzzle and target heights, the default arcs (left, right,
/// top, bottom) and range.
pub const TURN_RATE_DEG_PER_S: f32 = 360.0;
pub const SCAN_RATE_DEG_PER_S: f32 = 30.0;
pub const ON_TARGET_DEGREES: f32 = 2.0;
pub const MUZZLE_HEIGHT: f32 = 40.0;
pub const TARGET_HEIGHT: f32 = 40.0;
pub const ARCS: [f32; 4] = [180.0, 180.0, 90.0, 90.0];
pub const DEFAULT_RANGE: f32 = 4096.0;
/// `SENTRY_SPINUP_TIME`.
pub const SPINUP_S: f32 = 0.05;
/// Shot spacing without the weapon's numbers (the minigun's 0.05 s).
const DEFAULT_FIRE_S: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sentry {
    pub owner: usize,
    pub origin: [f32; 3],
    /// The base's yaw.
    pub yaw: f32,
    pub aim: [f32; 3],
    pub carried: bool,
    /// Where a carried sentry would go, and whether it may.
    pub preview_ok: bool,
    pub target: Option<usize>,
    pub on_target: bool,
    pub scan_dir: f32,
    /// Firing: time on target so far (spin-up), shots left in the burst, time to the next shot, pause left.
    pub spin: f32,
    pub burst_left: u32,
    pub shot_in: f32,
    pub pause: f32,
    pub heat: f32,
    pub overheated: bool,
    pub health: f32,
    /// When it goes.
    pub expires_ms: u32,
}

/// IW4L `guidance::turn_toward`: `current` turned towards `wanted` by at most `max` radians.
pub fn turn_toward(current: [f32; 3], wanted: [f32; 3], max: f32) -> [f32; 3] {
    let dot = (0..3)
        .map(|k| current[k] * wanted[k])
        .sum::<f32>()
        .clamp(-1.0, 1.0);
    let angle = libm::acosf(dot);
    if angle <= max || angle.is_nan() {
        return wanted;
    }
    // Rotate `current` about current Ã— wanted (Rodrigues).
    let mut axis = [
        current[1] * wanted[2] - current[2] * wanted[1],
        current[2] * wanted[0] - current[0] * wanted[2],
        current[0] * wanted[1] - current[1] * wanted[0],
    ];
    let l = libm::sqrtf(axis.iter().map(|x| x * x).sum());
    if l < 1e-4 {
        // Opposite: any perpendicular.
        axis = if libm::fabsf(current[2]) < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
    } else {
        axis = axis.map(|x| x / l);
    }
    let (s, c) = (libm::sinf(max), libm::cosf(max));
    let kxv = [
        axis[1] * current[2] - axis[2] * current[1],
        axis[2] * current[0] - axis[0] * current[2],
        axis[0] * current[1] - axis[1] * current[0],
    ];
    let kdv: f32 = (0..3).map(|k| axis[k] * current[k]).sum();
    core::array::from_fn(|k| current[k] * c + kxv[k] * s + axis[k] * kdv * (1.0 - c))
}

/// IW4L `within_arcs` for a base at pitch 0, yaw `base_yaw`.
pub fn within_arcs(base_yaw: f32, dir: [f32; 3]) -> bool {
    let a = math_iw4::vect_to_angles(dir);
    let yaw = math_iw4::angle_subtract(a[1], base_yaw);
    let pitch = math_iw4::angle_subtract(a[0], 0.0);
    let [left, right, top, bottom] = ARCS;
    (-right..=left).contains(&yaw) && (-top..=bottom).contains(&pitch)
}

fn norm(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = libm::sqrtf(v.iter().map(|x| x * x).sum());
    (l > 1e-6).then(|| v.map(|x| x / l))
}

/// A straight bullet from `from` along `dir` for `range` (no walls passed).
pub(crate) fn straight_shot(clip: &MapClip<'_>, from: [f32; 3], dir: [f32; 3], range: f32) -> Shot {
    let end: [f32; 3] = core::array::from_fn(|k| from[k] + dir[k] * range);
    let t = clip.trace(from, end, [0.0; 3], [0.0; 3], MASK_SHOT);
    let hit_at: [f32; 3] = core::array::from_fn(|k| from[k] + (end[k] - from[k]) * t.fraction);
    Shot {
        start: from,
        end: hit_at,
        hit: t.fraction < 1.0,
        normal: t.normal,
        surface: t.surface_flags,
        pen_end: hit_at,
        layers: [(0.0, 1.0); crate::gun::MAX_LAYERS],
        layer_count: 0,
    }
}

impl Arena {
    /// Fighter `owner` gets a sentry to carry (`tryUseAutoSentry`).
    pub fn give_sentry(&mut self, owner: usize, now: u32) {
        let yaw = self.fighters[owner].player.view().1;
        let health = self.streaks.rules.sentry_health;
        self.streaks
            .sentries
            .retain(|s| !(s.owner == owner && s.carried));
        self.streaks.sentries.push(Sentry {
            owner,
            origin: self.fighters[owner].player.ps.origin,
            yaw,
            aim: super::yaw_dir(yaw),
            carried: true,
            preview_ok: false,
            target: None,
            on_target: false,
            scan_dir: 1.0,
            spin: 0.0,
            burst_left: 0,
            shot_in: 0.0,
            pause: 0.0,
            heat: 0.0,
            overheated: false,
            health,
            expires_ms: now,
        });
        self.streaks.notes.push(Note::Called {
            fighter: owner,
            streak: Streak::Sentry,
        });
    }

    /// Whether fighter `owner` carries a sentry.
    pub fn carries_sentry(&self, owner: usize) -> bool {
        self.streaks
            .sentries
            .iter()
            .any(|s| s.owner == owner && s.carried)
    }

    /// Set fighter `owner`'s carried sentry down, if it may stand where it is shown. Returns whether it did.
    pub fn place_sentry(&mut self, owner: usize, now: u32) -> bool {
        let timeout = (self.streaks.rules.sentry_timeout_s * 1000.0) as u32;
        match self
            .streaks
            .sentries
            .iter_mut()
            .find(|s| s.owner == owner && s.carried && s.preview_ok)
        {
            Some(s) => {
                s.carried = false;
                s.aim = super::yaw_dir(s.yaw);
                s.expires_ms = now.wrapping_add(timeout);
                true
            }
            None => false,
        }
    }
}

/// Carried sentries follow their owner; placed ones aim, fire and cool.
pub(super) fn update(
    a: &mut Arena,
    s: &mut Streaks,
    clip: &MapClip<'_>,
    dt: f32,
    now: u32,
    out: &mut Vec<Event>,
) {
    let gun: Option<Gunline> = s.weapons.sentry;
    let fire_s = gun.map_or(DEFAULT_FIRE_S, |g| {
        (g.facts.fire_time_ms as f32 / 1000.0).max(0.01)
    });
    let range = gun
        .map_or(DEFAULT_RANGE, |g| g.facts.bullet_range())
        .max(1.0);
    let r = s.rules;
    let mut shots: Vec<(usize, Shot)> = Vec::new();
    let mut rolls: Vec<f32> = (0..8).map(|_| 0.0).collect();
    for x in rolls.iter_mut() {
        *x = s.random();
    }
    let mut roll = rolls.into_iter().cycle();
    for t in &mut s.sentries {
        let owner = &a.fighters[t.owner];
        if t.carried {
            // A carried sentry goes with its owner, and is lost if they die.
            if !owner.life.alive() {
                t.health = 0.0;
                continue;
            }
            let p = &owner.player;
            let (pitch, yaw) = p.view();
            let f = math_iw4::angle_vectors([pitch, yaw, 0.0]).0;
            let pl = super::placement::place(clip, p.ps.origin, p.eye(), f);
            t.origin = pl.pose.origin.map(|x| x as f32);
            t.yaw = yaw;
            t.aim = super::yaw_dir(yaw);
            t.preview_ok = pl.valid;
            continue;
        }
        if (now.wrapping_sub(t.expires_ms) as i32) >= 0 {
            t.health = 0.0;
            continue;
        }
        let muzzle = [t.origin[0], t.origin[1], t.origin[2] + MUZZLE_HEIGHT];
        // IW4L `acquire`: the nearest hostile in range, in the arcs and in sight.
        let mut best: Option<(f32, usize, [f32; 3])> = None;
        for (i, f) in a.fighters.iter().enumerate() {
            if !f.life.alive() || (i == 0 && !a.human_present) || !a.enemies(t.owner, i) {
                continue;
            }
            let o = f.player.ps.origin;
            let at = [o[0], o[1], o[2] + TARGET_HEIGHT];
            let d2 = dist2(at, muzzle);
            if d2 > range * range || best.is_some_and(|b| b.0 <= d2) {
                continue;
            }
            let Some(dir) = norm(core::array::from_fn(|k| at[k] - muzzle[k])) else {
                continue;
            };
            if !within_arcs(t.yaw, dir) {
                continue;
            }
            let tr = clip.trace(muzzle, at, [0.0; 3], [0.0; 3], MASK_SHOT);
            if tr.fraction < 1.0 {
                continue;
            }
            best = Some((d2, i, at));
        }
        match best {
            Some((_, target, at)) => {
                if t.target != Some(target) {
                    t.on_target = false;
                }
                t.target = Some(target);
                if let Some(wanted) = norm(core::array::from_fn(|k| at[k] - muzzle[k])) {
                    let aim = turn_toward(t.aim, wanted, TURN_RATE_DEG_PER_S.to_radians() * dt);
                    t.aim = aim;
                    let dot: f32 = (0..3).map(|k| aim[k] * wanted[k]).sum();
                    t.on_target =
                        libm::acosf(dot.clamp(-1.0, 1.0)) <= ON_TARGET_DEGREES.to_radians();
                }
            }
            None => {
                t.target = None;
                t.on_target = false;
                // Scan to and fro within the arcs (at most 90Â° each way).
                let yaw = math_iw4::angle_subtract(math_iw4::vect_to_angles(t.aim)[1], t.yaw);
                let (left, right) = (ARCS[0].min(90.0), ARCS[1].min(90.0));
                let next = yaw + t.scan_dir * SCAN_RATE_DEG_PER_S * dt;
                if next >= left {
                    t.scan_dir = -1.0;
                } else if next <= -right {
                    t.scan_dir = 1.0;
                }
                let wanted = super::yaw_dir(t.yaw + next.clamp(-right, left));
                t.aim = turn_toward(t.aim, wanted, SCAN_RATE_DEG_PER_S.to_radians() * dt);
            }
        }
        // `sentry_heatMonitor`: a second of heat lost each second not firing (overheated: until none is left).
        let mut fired = false;
        if t.on_target && !t.overheated {
            t.spin += dt;
            if t.spin >= SPINUP_S {
                if t.pause > 0.0 {
                    t.pause -= dt;
                } else {
                    if t.burst_left == 0 {
                        let (lo, hi) = (r.sentry_burst_min as u32, r.sentry_burst_max as u32);
                        let n = lo
                            + ((hi.saturating_sub(lo) + 1) as f32 * roll.next().unwrap_or(0.5))
                                as u32;
                        t.burst_left = n.clamp(lo.max(1), hi.max(1));
                    }
                    t.shot_in -= dt;
                    while t.shot_in <= 0.0 && t.burst_left > 0 && !t.overheated {
                        t.shot_in += fire_s;
                        t.burst_left -= 1;
                        t.heat += fire_s;
                        fired = true;
                        shots.push((t.owner, straight_shot(clip, muzzle, t.aim, range)));
                        if t.heat > r.sentry_overheat_s {
                            t.overheated = true;
                        }
                        if t.burst_left == 0 {
                            let p = roll.next().unwrap_or(0.5);
                            t.pause =
                                r.sentry_pause_min + (r.sentry_pause_max - r.sentry_pause_min) * p;
                        }
                    }
                }
            }
        } else {
            t.spin = 0.0;
        }
        if !fired {
            t.heat = (t.heat - dt).max(0.0);
            if t.overheated && t.heat <= 0.0 {
                t.overheated = false;
            }
        }
    }
    s.sentries.retain(|t| t.health > 0.0);
    if let Some(g) = gun {
        for (owner, shot) in shots {
            out.push(Event::BotFired {
                bot: owner,
                path: Some((shot.start, shot.pen_end)),
            });
            a.bullet(owner, &shot, &g.facts, now, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips;
    use crate::player::Player;

    #[test]
    fn turning_and_arcs_follow_iw4l() {
        let a = turn_toward([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 10f32.to_radians());
        let yaw = libm::atan2f(a[1], a[0]).to_degrees();
        assert!((yaw - 10.0).abs() < 0.01, "{yaw}");
        assert_eq!(
            turn_toward([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 2.0),
            [0.0, 1.0, 0.0]
        );
        assert!(within_arcs(0.0, [0.0, -1.0, 0.0]));
        assert!(within_arcs(90.0, [-1.0, 0.0, 0.0]));
        // Straight down is the bottom arc's 90Â°: still in.
        assert!(within_arcs(0.0, [0.0, 0.0, -1.0]));
    }

    fn minigun() -> Gunline {
        let c = weapon_iw4::CapturedCombatInput {
            fire_time_ms: 50,
            clip_size: 100,
            start_ammo: 100,
            max_ammo: 100,
            reload_time_ms: 1000,
            reload_empty_time_ms: 1000,
            raise_time_ms: 100,
            first_raise_time_ms: 100,
            drop_time_ms: 100,
            damage: 20,
            min_damage: 20,
            max_damage_range: 1024.0,
            min_damage_range: 2048.0,
            ..weapon_iw4::CapturedCombatInput::default()
        };
        Gunline {
            weapon: 0,
            facts: weapon_iw4::WeaponCombatFacts::try_from_captured(c).expect("usable"),
        }
    }

    #[test]
    fn a_placed_sentry_turns_onto_an_enemy_and_kills_it_then_times_out() {
        let floor = test_clips::boxes(&[([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let spawns = [([0.0, 600.0, 0.0], 0.0), ([0.0, 600.0, 0.0], 0.0)];
        let human = Player::spawn(&clip, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(
            &clip,
            &spawns,
            1,
            human,
            wpk::rules::Rules::default(),
            |_| None,
        );
        a.streaks = Streaks::new(
            wpk::streaks::StreakRules::default(),
            super::super::Weapons {
                sentry: Some(minigun()),
                ..Default::default()
            },
            super::super::MapInfo::default(),
        );
        a.give_sentry(0, 0);
        assert!(a.carries_sentry(0));
        let mut ev = Vec::new();
        a.update_killstreaks(&clip, 0.05, 50, &mut ev);
        let s = a.streaks.sentries[0];
        assert!(s.preview_ok, "open floor: it may stand 64 ahead");
        assert!((s.origin[0] - 64.0).abs() < 0.5);
        assert!(a.place_sentry(0, 50));
        // The bot is 600 to the left (+y): 90Â° from the base, inside the arcs.
        let mut now = 50;
        let mut first_shot = None;
        while now < 10_000 && a.fighters[1].life.alive() {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
            if first_shot.is_none()
                && ev
                    .iter()
                    .any(|e| matches!(e, Event::BotFired { bot: 0, .. }))
            {
                first_shot = Some(now);
            }
        }
        // 90Â° at 360Â°/s is a quarter second, then the spin-up.
        let first_shot = first_shot.expect("it fired");
        assert!(first_shot <= 450, "first shot at {first_shot}");
        assert!(!a.fighters[1].life.alive(), "the bot died");
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
        // 90 s after it was placed it goes.
        a.update_killstreaks(&clip, 0.05, 90_100, &mut ev);
        assert!(a.streaks.sentries.is_empty());
    }

    #[test]
    fn it_overheats_on_long_fire_and_comes_back_once_cool() {
        let floor = test_clips::boxes(&[([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let spawns = [([64.0, 0.0, 0.0], 0.0), ([64.0, 0.0, 0.0], 0.0)];
        let human = Player::spawn(&clip, [600.0, 0.0, 0.0], 180.0);
        let mut a = Arena::new(
            &clip,
            &spawns,
            1,
            human,
            wpk::rules::Rules::default(),
            |_| None,
        );
        a.god_mode = true;
        a.streaks = Streaks::new(
            wpk::streaks::StreakRules::default(),
            super::super::Weapons {
                sentry: Some(minigun()),
                ..Default::default()
            },
            super::super::MapInfo::default(),
        );
        // The bot's sentry, at the bot's feet facing +x, shoots at the (god-mode) player.
        a.give_sentry(1, 0);
        let t = &mut a.streaks.sentries[0];
        t.carried = false;
        t.origin = [0.0; 3];
        t.yaw = 0.0;
        t.expires_ms = 60_000;
        let mut ev = Vec::new();
        let mut now = 0;
        let mut hot_at = None;
        while now < 30_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
            let t = a.streaks.sentries[0];
            if hot_at.is_none() && t.overheated {
                hot_at = Some(now);
            }
            if let Some(h) = hot_at
                && !t.overheated
            {
                // About 8 s of heat to lose at a second a second.
                let cool = now - h;
                assert!((7_500..=8_500).contains(&cool), "cooled in {cool} ms");
                return;
            }
        }
        panic!("never overheated and cooled (hot at {hot_at:?})");
    }
}

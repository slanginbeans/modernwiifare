//! The care package and the emergency airdrop (`_airdrop.gsc`, values from `StreakRules`).
//!
//! The marker (`airdrop_marker_mp`, IW4L `equipment::AIRDROP_MARKER_WEAPON`) is thrown like a grenade and goes off
//! where it comes to rest (IW4L `waits_for_ground`; `airDropMarkerActivate` on its `explode`). A care package sends a
//! Little Bird (`doFlyBy`): from `lbHalfDistance` out at a random yaw, at `lbFlyHeight` above the marker, fast for
//! the first seconds and then slow, braking onto the point 50 units short of the marker, where it lets go of the crate
//! and flies off to the side. The emergency airdrop sends a C-130 (`doC130FlyBy`) along the thrower's line to the
//! marker, dropping four crates 0.05 s apart once it is over it. Crates fall under gravity (MW2's physics), crush
//! whoever they land on (the kill is the owner's), and hold a reward rolled from the crate odds
//! (`getRandomCrateType`) or ammo. Holding use by a crate takes it: its owner in `crateOwnerCaptureThink`'s time, anyone
//! else in `useHoldThink`'s. Unclaimed crates go after `dropTimeOut`.

use alloc::vec::Vec;

use super::{Note, Streak, Streaks, dist2, yaw_dir};
use crate::arena::{Arena, Event};
use crate::collision::{MASK_PLAYER_SOLID, MapClip};

/// MPH to units a second (IW4 vehicle speeds).
pub const MPH: f32 = 17.6;
/// How hard the Little Bird brakes onto its goal (`doFlyBy`'s `Vehicle_SetSpeed( 75, 40 )`: 40 mph a second).
const LB_BRAKE: f32 = 40.0 * MPH;
/// Gravity on a falling crate (`g_gravity`).
pub const GRAVITY: f32 = 800.0;
/// A crate's half size across and its height (the care package's collision brush, about 40 × 32 × 24).
pub const CRATE_HALF: [f32; 2] = [20.0, 16.0];
pub const CRATE_HEIGHT: f32 = 24.0;
/// How near a player must be to use a crate (its use trigger).
pub const USE_RANGE: f32 = 96.0;
/// The C-130 drops once it is this near the marker across, or as soon as it starts moving away (`doC130FlyBy`).
const C130_DROP_NEAR: f32 = 256.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carrier {
    LittleBird,
    C130,
}

/// What a crate holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contents {
    Ammo,
    Streak(Streak),
}

/// A drop on its way: a Little Bird or a C-130.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drop {
    pub owner: usize,
    pub carrier: Carrier,
    pub origin: [f32; 3],
    pub yaw: f32,
    /// Where it lets go (at its height) and where it leaves for.
    pub goal: [f32; 3],
    pub end: [f32; 3],
    pub speed: f32,
    /// Seconds since it set off.
    pub age: f32,
    /// Crates still to drop, and when the next goes.
    pub crates_left: u8,
    pub next_drop_ms: Option<u32>,
    /// The nearest it has come to the goal across (the C-130 drops once it moves away).
    pub nearest: f32,
    /// Gone at this time once it has dropped everything.
    pub leave_ms: Option<u32>,
}

/// A crate, falling or on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crate {
    pub id: u32,
    pub owner: usize,
    pub origin: [f32; 3],
    pub fall_speed: f32,
    pub landed_ms: Option<u32>,
    pub contents: Contents,
}

/// MW2's `getRandomCrateType`: `value = randomInt( total )`, then the first type (ammo first) whose running weight is
/// above it. `r` is in 0..1.
pub fn roll(ammo: f32, odds: &[f32; 15], r: f32) -> Contents {
    let total = ammo + odds.iter().sum::<f32>();
    let value = libm::floorf(r * total);
    let mut running = ammo;
    if ammo > 0.0 && running > value {
        return Contents::Ammo;
    }
    let mut last = Contents::Ammo;
    for (i, w) in odds.iter().enumerate() {
        if *w <= 0.0 {
            continue;
        }
        running += w;
        last = Contents::Streak(super::ALL[i]);
        if running > value {
            break;
        }
    }
    last
}

impl Arena {
    /// Fighter `owner` throws the marker for `streak` (a care package or the emergency airdrop) from `eye` along `dir`
    /// (IW4L's grenade throw with the marker's numbers). Without the marker in the pack the drop comes straight to
    /// `fallback` (the point ahead of the thrower). Returns whether anything was thrown or called.
    #[allow(clippy::too_many_arguments)]
    pub fn throw_marker(
        &mut self,
        owner: usize,
        streak: Streak,
        eye: [f32; 3],
        dir: [f32; 3],
        vel: [f32; 3],
        fallback: [f32; 3],
        now: u32,
    ) -> bool {
        let mega = streak == Streak::Mega;
        let marker = if mega {
            self.streaks
                .weapons
                .mega_marker
                .or(self.streaks.weapons.marker)
        } else {
            self.streaks.weapons.marker
        };
        match marker {
            Some((weapon, facts)) => {
                let mut g =
                    crate::grenade::Grenade::throw(owner, weapon, facts, eye, dir, vel, None, now);
                // It goes off where it rests (`update`), never on a fuse.
                g.detonate_at_ms = now.wrapping_add(60_000);
                self.streaks.markers.push((weapon, owner, mega));
                self.grenades.push(g);
            }
            None => {
                let yaw = self.streaks.random() * 360.0;
                let from = self.fighters[owner].player.ps.origin;
                call_drop(&mut self.streaks, owner, mega, fallback, yaw, from, now);
            }
        }
        self.streaks.notes.push(Note::Called {
            fighter: owner,
            streak,
        });
        true
    }

    /// Fighter `who` (at `origin`) holding use (or not) by a crate: the capture's progress 0..1 while it goes on,
    /// None when no crate is in reach. Finishing it hands the crate over.
    pub fn use_crate(&mut self, who: usize, origin: [f32; 3], held: bool, now: u32) -> Option<f32> {
        let mut s = core::mem::take(&mut self.streaks);
        let p = capture(self, &mut s, who, origin, held, now);
        self.streaks = s;
        p
    }
}

/// The crate in reach of `origin` (landed), nearest first.
pub fn crate_in_reach(s: &Streaks, origin: [f32; 3]) -> Option<usize> {
    s.crates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.landed_ms.is_some())
        .map(|(i, c)| {
            let d = [c.origin[0] - origin[0], c.origin[1] - origin[1]];
            (
                i,
                d[0] * d[0] + d[1] * d[1],
                libm::fabsf(c.origin[2] - origin[2]),
            )
        })
        .filter(|&(_, d2, dz)| d2 < USE_RANGE * USE_RANGE && dz < 72.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, ..)| i)
}

fn capture(
    a: &mut Arena,
    s: &mut Streaks,
    who: usize,
    origin: [f32; 3],
    held: bool,
    now: u32,
) -> Option<f32> {
    s.fit(who + 1);
    let Some(i) = crate_in_reach(s, origin) else {
        s.capture[who] = None;
        return None;
    };
    let c = s.crates[i];
    if !held {
        s.capture[who] = None;
        return Some(0.0);
    }
    let since = match s.capture[who] {
        Some((id, since)) if id == c.id => since,
        _ => {
            s.capture[who] = Some((c.id, now));
            now
        }
    };
    let need = if c.owner == who {
        s.rules.crate_owner_use_s
    } else {
        s.rules.crate_other_use_s
    } * 1000.0;
    let progress = (now.wrapping_sub(since) as f32 / need.max(1.0)).min(1.0);
    if progress < 1.0 {
        return Some(progress);
    }
    s.crates.swap_remove(i);
    s.capture[who] = None;
    match c.contents {
        Contents::Ammo => {
            if let Some(g) = a.fighters[who].gun.as_mut() {
                g.refill_ammo();
            }
            s.notes.push(Note::Ammo { fighter: who });
        }
        Contents::Streak(streak) => {
            s.rewards[who].push(streak);
            s.notes.push(Note::Reward {
                fighter: who,
                streak,
            });
        }
    }
    Some(1.0)
}

/// Send the drop for a marker gone off at `site` (`airDropMarkerActivate`): a Little Bird at `yaw`, or for the
/// emergency airdrop a C-130 along the line from `thrower` to the marker.
pub fn call_drop(
    s: &mut Streaks,
    owner: usize,
    mega: bool,
    site: [f32; 3],
    yaw: f32,
    thrower: [f32; 3],
    now: u32,
) {
    let r = s.rules;
    let height = site[2] + r.drop_height;
    let at = |p: [f32; 3], f: [f32; 3], d: f32| [p[0] + f[0] * d, p[1] + f[1] * d, height];
    let d = if mega {
        let dx = [site[0] - thrower[0], site[1] - thrower[1]];
        let yaw = if dx[0] == 0.0 && dx[1] == 0.0 {
            yaw
        } else {
            libm::atan2f(dx[1], dx[0]).to_degrees()
        };
        let f = yaw_dir(yaw);
        Drop {
            owner,
            carrier: Carrier::C130,
            origin: at(site, f, -r.c130_distance),
            yaw,
            goal: at(site, f, 0.0),
            end: at(site, f, r.c130_distance),
            speed: r.c130_speed,
            age: 0.0,
            crates_left: 4,
            next_drop_ms: None,
            nearest: f32::MAX,
            leave_ms: None,
        }
    } else {
        let f = yaw_dir(yaw);
        let side = yaw_dir(yaw + 90.0);
        let jitter = |s: &mut Streaks, amount: f32| (s.random() * 2.0 - 1.0) * amount;
        let (jx, jy) = (jitter(s, 100.0), jitter(s, 100.0));
        let (ex, ey) = (jitter(s, 150.0), jitter(s, 150.0));
        let start = at(site, f, -r.drop_distance);
        let end = at(site, side, r.drop_distance);
        Drop {
            owner,
            carrier: Carrier::LittleBird,
            origin: [start[0] + jx, start[1] + jy, height],
            yaw,
            // `pathGoal + forward × -50`.
            goal: at(site, f, -50.0),
            end: [end[0] + ex, end[1] + ey, height],
            speed: r.drop_fast_mph * MPH,
            age: 0.0,
            crates_left: 1,
            next_drop_ms: None,
            nearest: f32::MAX,
            leave_ms: None,
        }
    };
    let _ = now;
    s.drops.push(d);
}

/// Markers, drops, falling crates, time-outs and the bots' captures.
pub(super) fn update(
    a: &mut Arena,
    s: &mut Streaks,
    clip: &MapClip<'_>,
    dt: f32,
    now: u32,
    out: &mut Vec<Event>,
) {
    // Markers at rest (or stuck) go off.
    let mut k = 0;
    while k < a.grenades.len() {
        let g = &a.grenades[k];
        let marker = s
            .markers
            .iter()
            .position(|&(w, o, _)| w == g.weapon && o == g.owner);
        match marker {
            Some(m) if g.resting || g.stuck_ms.is_some() => {
                let (_, owner, mega) = s.markers.swap_remove(m);
                let site = g.origin;
                a.grenades.swap_remove(k);
                let yaw = s.random() * 360.0;
                let from = a.fighters[owner].player.ps.origin;
                call_drop(s, owner, mega, site, yaw, from, now);
            }
            _ => k += 1,
        }
    }
    // Markers whose grenade is gone (never rested) are forgotten.
    s.markers
        .retain(|&(w, o, _)| a.grenades.iter().any(|g| g.weapon == w && g.owner == o));
    fly_drops(s, dt, now);
    fall_crates(a, s, clip, dt, now, out);
    // Unclaimed crates go (`dropTimeOut`), unless someone is capturing one.
    let timeout = (s.rules.crate_timeout_s * 1000.0) as u32;
    let busy: Vec<u32> = s.capture.iter().flatten().map(|c| c.0).collect();
    s.crates.retain(|c| {
        c.landed_ms
            .is_none_or(|t| now.wrapping_sub(t) < timeout || busy.contains(&c.id))
    });
    // Bots stand by a crate in reach and hold use until it is theirs.
    for i in 1..a.fighters.len() {
        let f = &a.fighters[i];
        if !f.life.alive() {
            continue;
        }
        let o = f.player.ps.origin;
        let slow = f.player.ps.velocity[0] * f.player.ps.velocity[0]
            + f.player.ps.velocity[1] * f.player.ps.velocity[1]
            < 60.0 * 60.0;
        if crate_in_reach(s, o).is_some() {
            capture(a, s, i, o, slow, now);
        }
    }
}

fn fly_drops(s: &mut Streaks, dt: f32, now: u32) {
    let r = s.rules;
    let mut released: Vec<(usize, [f32; 3], bool)> = Vec::new();
    for d in &mut s.drops {
        d.age += dt;
        let target = if d.crates_left == 0 { d.end } else { d.goal };
        let to = [target[0] - d.origin[0], target[1] - d.origin[1]];
        let dist = libm::sqrtf(to[0] * to[0] + to[1] * to[1]);
        match d.carrier {
            Carrier::LittleBird if d.crates_left > 0 => {
                // Fast, then slow (`doFlyBy`), braking onto the goal.
                let cruise = if d.age < r.drop_fast_s {
                    r.drop_fast_mph
                } else {
                    r.drop_slow_mph
                } * MPH;
                d.speed = cruise.min(libm::sqrtf(2.0 * LB_BRAKE * dist));
                if dist < 16.0 && d.next_drop_ms.is_none() {
                    // `waittill( "goal" ); wait( .10 ); notify( "drop_crate" )`.
                    d.next_drop_ms = Some(now.wrapping_add(100));
                }
            }
            Carrier::LittleBird => d.speed = r.drop_fast_mph * MPH,
            Carrier::C130 => {
                if d.crates_left > 0 && d.next_drop_ms.is_none() {
                    let along = dist;
                    if along < C130_DROP_NEAR || along > d.nearest {
                        d.next_drop_ms = Some(now.wrapping_add(50));
                    }
                    d.nearest = d.nearest.min(along);
                }
            }
        }
        // The C-130 flies straight on; the Little Bird heads for its target.
        let dir = match d.carrier {
            Carrier::C130 => yaw_dir(d.yaw),
            Carrier::LittleBird if dist > 1.0 => [to[0] / dist, to[1] / dist, 0.0],
            Carrier::LittleBird => [0.0; 3],
        };
        let step = match d.carrier {
            Carrier::LittleBird if d.crates_left > 0 => (d.speed * dt).min(dist),
            _ => d.speed * dt,
        };
        d.origin[0] += dir[0] * step;
        d.origin[1] += dir[1] * step;
        if dist > 1.0 && d.carrier == Carrier::LittleBird {
            d.yaw = libm::atan2f(to[1], to[0]).to_degrees();
        }
        if let Some(t) = d.next_drop_ms
            && (now.wrapping_sub(t) as i32) >= 0
            && d.crates_left > 0
        {
            released.push((
                d.owner,
                [d.origin[0], d.origin[1], d.origin[2] - 64.0],
                d.carrier == Carrier::C130,
            ));
            d.crates_left -= 1;
            d.next_drop_ms = (d.crates_left > 0).then(|| now.wrapping_add(50));
            if d.crates_left == 0 {
                // The C-130 goes 4 s after its last crate; the Little Bird once it is far off.
                d.leave_ms = Some(now.wrapping_add(match d.carrier {
                    Carrier::C130 => 4000,
                    Carrier::LittleBird => 12_000,
                }));
            }
        }
    }
    s.drops
        .retain(|d| d.leave_ms.is_none_or(|t| (now.wrapping_sub(t) as i32) < 0));
    for (owner, at, mega) in released {
        let (ammo, odds) = if mega {
            (s.rules.mega_ammo, s.rules.mega_odds)
        } else {
            (s.rules.crate_ammo, s.rules.crate_odds)
        };
        let r = s.random();
        let contents = roll(ammo, &odds, r);
        let id = s.id();
        s.crates.push(Crate {
            id,
            owner,
            origin: at,
            fall_speed: 0.0,
            landed_ms: None,
            contents,
        });
    }
}

fn fall_crates(
    a: &mut Arena,
    s: &mut Streaks,
    clip: &MapClip<'_>,
    dt: f32,
    now: u32,
    out: &mut Vec<Event>,
) {
    let mins = [-CRATE_HALF[0], -CRATE_HALF[1], 0.0];
    let maxs = [CRATE_HALF[0], CRATE_HALF[1], CRATE_HEIGHT];
    for c in &mut s.crates {
        if c.landed_ms.is_some() {
            continue;
        }
        c.fall_speed += GRAVITY * dt;
        let end = [c.origin[0], c.origin[1], c.origin[2] - c.fall_speed * dt];
        let t = clip.trace(c.origin, end, mins, maxs, MASK_PLAYER_SOLID);
        let from = c.origin;
        c.origin[2] += (end[2] - c.origin[2]) * if t.startsolid != 0 { 1.0 } else { t.fraction };
        // Whoever it lands on is crushed (the kill is the crate owner's).
        for v in 0..a.fighters.len() {
            let f = &a.fighters[v];
            if !f.life.alive() || (v == 0 && !a.human_present) {
                continue;
            }
            let o = f.player.ps.origin;
            let inside = libm::fabsf(o[0] - c.origin[0]) < CRATE_HALF[0] + 15.0
                && libm::fabsf(o[1] - c.origin[1]) < CRATE_HALF[1] + 15.0
                && c.origin[2] < o[2] + f.player.hull_height()
                && from[2] + CRATE_HEIGHT > o[2];
            if inside && c.fall_speed > 200.0 {
                let hp = f.life.max_health * 10;
                a.hurt(
                    c.owner,
                    v,
                    hp,
                    crate::damage::hitloc::HEAD,
                    [0.0, 0.0, -1.0],
                    now,
                    out,
                );
            }
        }
        if t.fraction < 1.0 || t.startsolid != 0 {
            c.landed_ms = Some(now);
            c.fall_speed = 0.0;
            s.notes.push(Note::CrateLanded { origin: c.origin });
        }
        // Fell out of the world: gone.
        if c.origin[2] < -65_000.0 {
            c.landed_ms = Some(now.wrapping_sub(u32::MAX / 2));
        }
    }
    let _ = dist2;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Arena;
    use crate::collision::test_clips;
    use crate::killstreaks::{MapInfo, Streaks, Weapons};
    use crate::player::Player;

    fn arena(clip: &MapClip<'_>) -> Arena {
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([300.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(clip, [-300.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(
            clip,
            &spawns,
            1,
            human,
            wpk::rules::Rules::default(),
            |_| None,
        );
        a.streaks = Streaks::new(
            wpk::streaks::StreakRules::default(),
            Weapons::default(),
            MapInfo::default(),
        );
        a
    }

    #[test]
    fn crate_odds_follow_get_random_crate_type() {
        let r = wpk::streaks::StreakRules::default();
        // MW2's care package: ammo 17 of 115, then the UAV 17, the Counter-UAV 15...
        let total: f32 = r.crate_ammo + r.crate_odds.iter().sum::<f32>();
        assert_eq!(total, 115.0);
        assert_eq!(roll(r.crate_ammo, &r.crate_odds, 0.0), Contents::Ammo);
        assert_eq!(
            roll(r.crate_ammo, &r.crate_odds, 16.9 / 115.0),
            Contents::Ammo
        );
        assert_eq!(
            roll(r.crate_ammo, &r.crate_odds, 17.0 / 115.0),
            Contents::Streak(Streak::Uav)
        );
        assert_eq!(
            roll(r.crate_ammo, &r.crate_odds, 34.0 / 115.0),
            Contents::Streak(Streak::CounterUav)
        );
        // The last value is the EMP (the nuke's weight is 0).
        assert_eq!(
            roll(r.crate_ammo, &r.crate_odds, 114.0 / 115.0),
            Contents::Streak(Streak::Emp)
        );
        // Over many rolls each type comes up in proportion to its weight.
        let mut counts = [0u32; 16];
        for k in 0..11_500u32 {
            let c = roll(r.crate_ammo, &r.crate_odds, k as f32 / 11_500.0);
            counts[match c {
                Contents::Ammo => 15,
                Contents::Streak(s) => s as usize,
            }] += 1;
        }
        assert_eq!(counts[15], 1700);
        assert_eq!(counts[Streak::Sentry as usize], 1200);
        assert_eq!(counts[Streak::Nuke as usize], 0);
        assert_eq!(counts[Streak::Airdrop as usize], 0);
    }

    #[test]
    fn a_care_package_flies_in_drops_and_is_captured_by_holding_use() {
        let floor = test_clips::boxes(&[([-20000.0, -20000.0, -64.0], [20000.0, 20000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        let site = [0.0, 0.0, 0.0];
        call_drop(&mut a.streaks, 0, false, site, 0.0, [-300.0, 0.0, 0.0], 0);
        let mut ev = Vec::new();
        let mut now = 0u32;
        let mut landed = None;
        let mut released = None;
        // 50 ms steps for up to 30 s.
        while now < 30_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
            if released.is_none() && !a.streaks.crates.is_empty() {
                released = Some(now);
            }
            if let Some(c) = a.streaks.crates.first()
                && c.landed_ms.is_some()
            {
                landed = Some((now, c.origin));
                break;
            }
        }
        let released = released.expect("the crate was dropped");
        // 2 s fast (8800 units), the rest slow and braking: about 7 s for 15 000 units (MW2's care package lag).
        assert!(
            (6000..9500).contains(&released),
            "released at {released} ms"
        );
        let (at, origin) = landed.expect("the crate landed");
        // From 850 units up it falls for about 1.5 s, landing 50 units short of the marker.
        assert!(at - released < 2000);
        assert!(
            origin[2].abs() < 1.0 && (origin[0] + 50.0).abs() < 20.0,
            "{origin:?}"
        );
        // The owner by the crate: half a second of holding use takes it.
        let by = [origin[0] - 40.0, origin[1], 0.0];
        assert_eq!(a.use_crate(0, by, false, now), Some(0.0));
        assert_eq!(a.use_crate(0, [2000.0, 0.0, 0.0], true, now), None);
        assert!(a.use_crate(0, by, true, now).unwrap() < 0.1);
        assert!(a.use_crate(0, by, true, now + 250).unwrap() < 1.0);
        assert_eq!(a.use_crate(0, by, true, now + 500), Some(1.0));
        assert!(a.streaks.crates.is_empty());
        let notes = a.streaks.take_notes();
        assert!(notes.iter().any(|n| matches!(
            n,
            Note::Reward { fighter: 0, .. } | Note::Ammo { fighter: 0 }
        )));
    }

    #[test]
    fn others_take_three_seconds_and_unclaimed_crates_time_out() {
        let floor = test_clips::boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        a.streaks.crates.push(Crate {
            id: 7,
            owner: 1,
            origin: [0.0, 0.0, 0.0],
            fall_speed: 0.0,
            landed_ms: Some(0),
            contents: Contents::Streak(Streak::Predator),
        });
        let by = [-40.0, 0.0, 0.0];
        assert!(a.use_crate(0, by, true, 1000).is_some());
        assert!(a.use_crate(0, by, true, 3900).unwrap() < 1.0);
        assert_eq!(a.use_crate(0, by, true, 4000), Some(1.0));
        assert_eq!(a.streaks.rewards[0], [Streak::Predator]);
        // Another crate, left alone for 90 s.
        a.streaks.crates.push(Crate {
            id: 8,
            owner: 1,
            origin: [500.0, 500.0, 0.0],
            fall_speed: 0.0,
            landed_ms: Some(10_000),
            contents: Contents::Ammo,
        });
        let mut ev = Vec::new();
        a.update_killstreaks(&clip, 0.05, 99_000, &mut ev);
        assert_eq!(a.streaks.crates.len(), 1);
        a.update_killstreaks(&clip, 0.05, 100_000, &mut ev);
        assert!(a.streaks.crates.is_empty());
    }

    #[test]
    fn a_falling_crate_crushes_the_player_under_it_for_its_owner() {
        let floor = test_clips::boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        let under = a.fighters[1].player.ps.origin;
        a.streaks.crates.push(Crate {
            id: 1,
            owner: 0,
            origin: [under[0], under[1], under[2] + 400.0],
            fall_speed: 0.0,
            landed_ms: None,
            contents: Contents::Ammo,
        });
        let mut ev = Vec::new();
        let mut now = 0;
        while now < 3000 && a.fighters[1].life.alive() {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert!(!a.fighters[1].life.alive());
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
    }

    #[test]
    fn the_emergency_airdrop_drops_four_crates_over_the_marker() {
        let floor = test_clips::boxes(&[([-30000.0, -30000.0, -64.0], [30000.0, 30000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        call_drop(
            &mut a.streaks,
            0,
            true,
            [0.0; 3],
            0.0,
            [-300.0, 0.0, 0.0],
            0,
        );
        let mut ev = Vec::new();
        let mut now = 0;
        while now < 20_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert_eq!(a.streaks.crates.len(), 4);
        // 24 000 units at 2000 a second: over the marker at 12 s; the crates within a few hundred units of it.
        for c in &a.streaks.crates {
            assert!(c.landed_ms.is_some());
            assert!(
                c.origin[0].abs() < 700.0 && c.origin[1].abs() < 1.0,
                "{:?}",
                c.origin
            );
        }
    }
}

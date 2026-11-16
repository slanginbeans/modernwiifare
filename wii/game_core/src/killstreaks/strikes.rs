//! The precision airstrike, the Harrier strike's bombing runs and the stealth bomber (`_airstrike.gsc` `callStrike`,
//! values from `StreakRules`).
//!
//! The target and heading are picked on the map. A plane flies from `planeHalfDistance` before the target to as far
//! past it (the stealth bomber four times as far), at the old height system's `planeFlyHeight` above the target plus
//! a random bit, at `planeFlySpeed`. Precision and Harrier: two planes 1.5-2.5 s apart (`doPlaneStrike`); each lets
//! its cluster bomb go a second before `bombTime` (the time to `planeBombExplodeDistance` past the target), the bomb
//! keeps the plane's speed / 1.5 and falls (`moveGravity`), and 2.1 s later (or as it comes within 128 units of the
//! ground: the bomb model doesn't collide in MW2, ours bursts above the ground) it bursts into `repeat` traces pitched
//! from `maxAngles` down to `minAngles` ahead, 0.05 s apart, each hit a `losRadiusDamage` of the rules' radius and
//! damage (`callStrike_bombEffect`). The stealth bomber (`doBomberStrike`) drops a bomb every 0.1 s while it is within
//! 4500 of the target on its way through, each at a random point within `accuracyRadius` of it, going off after
//! `0.85 × height / 2000` s (`callStrike_bomb`). Not here: the artillery shellshock, the kill cams.

use alloc::vec::Vec;

use super::{Note, Streak, Streaks, yaw_dir};
use crate::arena::{Arena, Event};
use crate::collision::MapClip;
use crate::gun::MASK_SHOT;

/// `callStrike_bombEffect`'s waits between the bomb's release and its burst.
pub const BURST_AFTER_S: f32 = 0.4 + 0.45 + 0.15 + 0.10 + 0.05 + 0.25 + 0.2 + 0.5;
/// The bomb bursts no lower than this above the ground.
pub const BURST_FLOOR: f32 = 128.0;
/// `moveGravity`'s gravity.
const GRAVITY: f32 = 800.0;
/// `doBomberStrike`: the bombing window around the target and the spacing.
const STEALTH_WINDOW: f32 = 5000.0;
const STEALTH_DROP: f32 = 4500.0;
const STEALTH_EVERY_S: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub owner: usize,
    pub streak: Streak,
    pub origin: [f32; 3],
    pub start: [f32; 3],
    pub dir: [f32; 3],
    pub speed: f32,
    pub age: f32,
    pub fly_s: f32,
    /// Precision / Harrier: when the bomb goes (already gone: None).
    pub bomb_at_s: Option<f32>,
    /// Stealth: the target and time to the next bomb.
    pub target: [f32; 3],
    pub next_bomb_s: f32,
}

/// A falling cluster bomb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bomb {
    pub owner: usize,
    pub origin: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub age: f32,
}

/// A blast waiting to go off: where, when, its radius and damage, and whether it shows (its effect and sound).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shell {
    pub owner: usize,
    pub at: [f32; 3],
    pub due_ms: u32,
    pub radius: f32,
    pub max: f32,
    pub min: f32,
    pub weapon: u32,
    pub show: bool,
}

/// The start of a strike plane at `target` heading `yaw` (`callStrike`), `extra` units above the run's height.
pub fn plane(
    s: &Streaks,
    owner: usize,
    streak: Streak,
    target: [f32; 3],
    yaw: f32,
    extra: f32,
) -> Plane {
    let r = &s.rules;
    let stealth = streak == Streak::Stealth;
    let (half, speed, height, explode) = if stealth {
        (
            r.stealth_distance,
            r.stealth_speed,
            r.stealth_height,
            r.strike_explode_distance,
        )
    } else {
        (
            r.strike_distance,
            r.strike_speed,
            r.strike_height,
            r.strike_explode_distance,
        )
    };
    let f = yaw_dir(yaw);
    let z = target[2] + height + extra;
    let start = [target[0] - f[0] * half, target[1] - f[1] * half, z];
    let far = if stealth { half * 4.0 } else { half };
    let end = [target[0] + f[0] * far, target[1] + f[1] * far, z];
    let d = libm::hypotf(end[0] - start[0], end[1] - start[1]);
    let bomb_time = libm::fabsf(d / 2.0 + explode) / speed;
    Plane {
        owner,
        streak,
        origin: start,
        start,
        dir: f,
        speed,
        age: 0.0,
        fly_s: d / speed,
        // `callStrike_bombEffect( ..., bombTime - 1.0, ... )`.
        bomb_at_s: (!stealth).then_some((bomb_time - 1.0).max(0.0)),
        target,
        next_bomb_s: 0.0,
    }
}

impl Arena {
    /// Fighter `owner` calls `streak` (precision airstrike, Harrier strike or stealth bomber) on `target`, the planes
    /// heading `yaw`.
    pub fn call_strike(
        &mut self,
        owner: usize,
        streak: Streak,
        target: [f32; 3],
        yaw: f32,
        now: u32,
    ) {
        let s = &mut self.streaks;
        let e1 = s.random() * 500.0;
        let first = plane(
            s,
            owner,
            streak,
            target,
            yaw,
            if streak == Streak::Stealth {
                e1 * 2.0
            } else {
                e1
            },
        );
        s.planes.push(first);
        if streak != Streak::Stealth {
            // The second plane 1.5-2.5 s later (`randomfloatrange( 1.5, 2.5 )`, up to 200 higher).
            let e2 = s.random() * 200.0;
            let wait = 1500.0 + s.random() * 1000.0;
            let p = plane(s, owner, streak, target, yaw, e2);
            s.pending_planes.push((now.wrapping_add(wait as u32), p));
            if streak == Streak::Harrier {
                // The Harrier itself follows the second run (`beginHarrier`).
                let w2 = wait + 1500.0 + s.random() * 1000.0;
                s.pending_harriers
                    .push((now.wrapping_add(w2 as u32), owner, target, yaw));
            }
        }
        s.notes.push(Note::Called {
            fighter: owner,
            streak,
        });
    }
}

/// Fly the planes, drop and burst the bombs, set off the blasts.
pub(super) fn update(
    a: &mut Arena,
    s: &mut Streaks,
    clip: &MapClip<'_>,
    dt: f32,
    now: u32,
    out: &mut Vec<Event>,
) {
    let due = |t: u32| (now.wrapping_sub(t) as i32) >= 0;
    let mut k = 0;
    while k < s.pending_planes.len() {
        if due(s.pending_planes[k].0) {
            let (_, p) = s.pending_planes.swap_remove(k);
            s.planes.push(p);
        } else {
            k += 1;
        }
    }
    let r = s.rules;
    let artillery = s.weapons.artillery.map_or(0, |b| b.weapon);
    let stealth_bomb = s.weapons.stealth_bomb.map_or(0, |b| b.weapon);
    let mut new_bombs: Vec<Bomb> = Vec::new();
    let mut stealth_drops: Vec<(usize, [f32; 3], bool)> = Vec::new();
    for p in &mut s.planes {
        p.age += dt;
        let along = p.speed * p.age;
        p.origin[0] = p.start[0] + p.dir[0] * along;
        p.origin[1] = p.start[1] + p.dir[1] * along;
        if let Some(t) = p.bomb_at_s
            && p.age >= t
        {
            p.bomb_at_s = None;
            new_bombs.push(Bomb {
                owner: p.owner,
                origin: p.origin,
                vel: p.dir.map(|x| x * p.speed / 1.5),
                yaw: libm::atan2f(p.dir[1], p.dir[0]).to_degrees(),
                age: 0.0,
            });
        }
        if p.streak == Streak::Stealth {
            let d = libm::hypotf(p.origin[0] - p.target[0], p.origin[1] - p.target[1]);
            // `for ( dist = ...; dist < 5000; ... )`: bombing while within the window, dropping within 4500.
            if d < STEALTH_WINDOW {
                p.next_bomb_s -= dt;
                while p.next_bomb_s <= 0.0 {
                    p.next_bomb_s += STEALTH_EVERY_S;
                    if d < STEALTH_DROP {
                        stealth_drops.push((
                            p.owner,
                            p.origin,
                            ((p.age * 10.0) as u32).is_multiple_of(2),
                        ));
                    }
                }
            }
        }
    }
    s.planes.retain(|p| p.age < p.fly_s);
    for (owner, from, show) in stealth_drops {
        // A random point within the accuracy radius under the plane, traced down to the ground.
        let ang = s.random() * core::f32::consts::TAU;
        let rad = s.random() * r.stealth_accuracy;
        let top = [
            from[0] + libm::cosf(ang) * rad,
            from[1] + libm::sinf(ang) * rad,
            from[2],
        ];
        let t = clip.trace(
            top,
            [top[0], top[1], top[2] - 10_000.0],
            [0.0; 3],
            [0.0; 3],
            MASK_SHOT,
        );
        let ground = [top[0], top[1], top[2] - 10_000.0 * t.fraction];
        let h = libm::sqrtf(super::dist2(from, ground));
        if h > 5000.0 {
            continue;
        }
        s.shells.push(Shell {
            owner,
            at: [ground[0], ground[1], ground[2] + 16.0],
            due_ms: now.wrapping_add((0.85 * h / 2000.0 * 1000.0) as u32),
            radius: r.stealth_radius,
            max: r.stealth_max_damage,
            min: r.stealth_min_damage,
            weapon: stealth_bomb,
            show,
        });
    }
    s.bombs.extend(new_bombs);
    // Cluster bombs: fall, then burst into their traces.
    let mut bursts: Vec<Bomb> = Vec::new();
    for b in &mut s.bombs {
        b.age += dt;
        b.vel[2] -= GRAVITY * dt;
        b.origin = core::array::from_fn(|k| b.origin[k] + b.vel[k] * dt);
        let below = [b.origin[0], b.origin[1], b.origin[2] - BURST_FLOOR];
        let near_ground = clip
            .trace(b.origin, below, [0.0; 3], [0.0; 3], MASK_SHOT)
            .fraction
            < 1.0;
        if b.age >= BURST_AFTER_S || near_ground {
            bursts.push(*b);
            b.age = f32::MAX;
        }
    }
    s.bombs.retain(|b| b.age != f32::MAX);
    for b in bursts {
        let n = r.strike_traces.max(1.0) as u32;
        let diff = (r.strike_max_angle - r.strike_min_angle) / n as f32;
        for i in 0..n {
            let jitter = (s.random() * 10.0) as i32 - 5;
            let pitch = r.strike_max_angle - diff * i as f32;
            let (dir, _, _) = math_iw4::angle_vectors([pitch, b.yaw + jitter as f32, 0.0]);
            let end: [f32; 3] = core::array::from_fn(|k| b.origin[k] + dir[k] * 10_000.0);
            let t = clip.trace(b.origin, end, [0.0; 3], [0.0; 3], MASK_SHOT);
            let hit: [f32; 3] =
                core::array::from_fn(|k| b.origin[k] + (end[k] - b.origin[k]) * t.fraction);
            s.shells.push(Shell {
                owner: b.owner,
                at: [hit[0], hit[1], hit[2] + 16.0],
                due_ms: now.wrapping_add(i * 50),
                radius: r.strike_radius,
                max: r.strike_max_damage,
                min: r.strike_min_damage,
                weapon: artillery,
                show: i % 3 == 0,
            });
        }
    }
    // Blasts due.
    let mut k = 0;
    while k < s.shells.len() {
        if !due(s.shells[k].due_ms) {
            k += 1;
            continue;
        }
        let sh = s.shells.swap_remove(k);
        if sh.show {
            out.push(Event::Explosion {
                origin: sh.at,
                weapon: sh.weapon,
                surface: 0,
            });
        }
        a.blast_players(
            clip,
            sh.at,
            sh.radius,
            sh.min,
            sh.max,
            Some(sh.owner),
            now,
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips;
    use crate::player::Player;

    fn arena(clip: &MapClip<'_>, bot_at: [f32; 3]) -> Arena {
        let spawns = [(bot_at, 0.0), (bot_at, 0.0)];
        let human = Player::spawn(clip, [-3000.0, -3000.0, 0.0], 0.0);
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
            super::super::Weapons::default(),
            super::super::MapInfo::default(),
        );
        a
    }

    #[test]
    fn a_precision_run_follows_call_strike() {
        let s = Streaks::new(
            wpk::streaks::StreakRules::default(),
            super::super::Weapons::default(),
            super::super::MapInfo::default(),
        );
        let p = plane(&s, 0, Streak::Precision, [0.0, 0.0, 100.0], 90.0, 0.0);
        // 24 000 before the target along +y, 850 above it; 48 000 at 7000 a second.
        assert!((p.start[1] + 24_000.0).abs() < 0.5 && p.start[0].abs() < 0.5);
        assert_eq!(p.start[2], 950.0);
        assert!((p.fly_s - 48_000.0 / 7000.0).abs() < 1e-3);
        // bombTime = (24 000 + 1500) / 7000; the bomb goes a second sooner.
        assert!((p.bomb_at_s.unwrap() - (25_500.0 / 7000.0 - 1.0)).abs() < 1e-3);
        let st = plane(&s, 0, Streak::Stealth, [0.0; 3], 0.0, 0.0);
        assert!((st.fly_s - 60_000.0 / 2000.0).abs() < 1e-3 && st.bomb_at_s.is_none());
    }

    #[test]
    fn a_precision_airstrike_on_a_bot_kills_it() {
        let floor = test_clips::boxes(&[([-40000.0, -40000.0, -64.0], [40000.0, 40000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        // The bursts land past the target along the run: put the bot where the cluster comes down.
        let mut a = arena(&clip, [1400.0, 0.0, 0.0]);
        a.call_strike(0, Streak::Precision, [0.0; 3], 0.0, 0);
        let mut ev = Vec::new();
        let mut now = 0;
        while now < 12_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert!(
            ev.iter()
                .filter(|e| matches!(e, Event::Explosion { .. }))
                .count()
                >= 8
        );
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
        assert!(
            a.streaks.planes.is_empty()
                && a.streaks.bombs.is_empty()
                && a.streaks.shells.is_empty()
        );
    }

    #[test]
    fn the_stealth_bomber_carpets_the_line_through_the_target() {
        let floor = test_clips::boxes(&[([-60000.0, -60000.0, -64.0], [60000.0, 60000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip, [0.0, 0.0, 0.0]);
        a.call_strike(0, Streak::Stealth, [0.0; 3], 0.0, 0);
        let mut ev = Vec::new();
        let mut now = 0;
        let mut xs: Vec<f32> = Vec::new();
        while now < 15_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
            for sh in &a.streaks.shells {
                if !xs.contains(&sh.at[0]) {
                    xs.push(sh.at[0]);
                }
            }
        }
        // 9000 units of bombing at 2000 a second, one every 0.1 s: about 45 bombs from -4500 to 4500 (± 512).
        assert!((40..=50).contains(&xs.len()), "{} bombs", xs.len());
        assert!(xs.iter().all(|x| x.abs() < 4500.0 + 512.0 + 300.0));
        assert!(!a.fighters[1].life.alive(), "the bot under it died");
    }
}

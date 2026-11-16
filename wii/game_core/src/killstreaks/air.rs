//! The killstreaks that stay in the air: the attack helicopter and Pave Low (`_helicopter.gsc`), the Harrier that
//! follows its strike (`_harrier.gsc`), the Chopper Gunner (`_helicopter.gsc` `heliRide`) and the AC-130
//! (`_ac130.gsc`). Values from `StreakRules`; guns from the common pack's weapons.
//!
//! Stand-ins: MW2 flies helicopters along the map's `heli_start` / `heli_loop_start` nodes, which the converter does not
//! carry yet, so they come in from the map's edge and circle its middle (the minimap's) at a height above the spawns,
//! at a loop speed of 40 mph (the scripts' `30 + randomInt( 20 )` when a node has none). The AC-130 circles the map's
//! middle (as MW2's `c130_zoomrig`, one turn in `ac130_Speed["rotate"]` seconds) at [`AC130_RADIUS`] out and
//! [`AC130_HEIGHT`] up (the rig model's `tag_player` offset is not converted).
//!
//! The attack helicopter's gun runs `attack_primary`: a target from `heli_targeting` (enemies in `heli_visual_range` in
//! sight, the nearest), aimed 96 units off it and converging by 0.05 a shot while it is seen, a clip of
//! `heli_turretClipSize` at `cobra_20mm_mp`'s fire time, then 0.5-2 s off. The Pave Low's minigun (`pavelow_minigun_mp`)
//! does the same. The Harrier hovers over its target for `harrierTimer`'s time firing 25-round bursts of
//! `harrier_20mm_mp` 0.1 s apart with a second between. The Chopper Gunner and AC-130 are flown by their owner: the
//! remote view looks where the pilot aims (`cobra_player_minigun_mp`; the AC-130's 105, 40 and 25 mm, each with its
//! reload). Bots aim theirs at the nearest enemy in sight.

use alloc::vec::Vec;

use super::{Blast, Gunline, Note, Streak, Streaks, dist2, yaw_dir};
use crate::arena::{Arena, Event};
use crate::collision::MapClip;
use crate::gun::MASK_SHOT;

/// Loop speed (40 mph) and how far up the helicopters fly over the map's middle.
pub const LOOP_SPEED: f32 = 40.0 * super::airdrop::MPH;
pub const HELI_HEIGHT: f32 = 1200.0;
/// The arrival speed from the map's edge (MW2's `heli_fly_simple_path` nodes: about 100 mph).
pub const ARRIVE_SPEED: f32 = 100.0 * super::airdrop::MPH;
/// `attack_primary`: the first shots' offset from the target and the convergence per shot.
pub const AIM_OFFSET: f32 = 96.0;
pub const CONVERGE: f32 = 0.05;
/// The AC-130's orbit (stand-in for the rig's `tag_player`).
pub const AC130_RADIUS: f32 = 4000.0;
pub const AC130_HEIGHT: f32 = 6000.0;
/// The Chopper Gunner's view sits under the nose.
const GUNNER_DROP: f32 = 80.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeliKind {
    Attack,
    PaveLow,
    Harrier,
    Gunner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Arriving,
    Circling,
    Leaving,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heli {
    pub owner: usize,
    pub kind: HeliKind,
    pub origin: [f32; 3],
    pub yaw: f32,
    pub phase: Phase,
    /// Where it circles (or hovers, the Harrier) and how far out.
    pub center: [f32; 3],
    pub radius: f32,
    pub angle: f32,
    pub health: f32,
    pub leave_ms: u32,
    /// The gun: target, the clip left, convergence, the aim offset, time to the next shot, the pause left.
    pub target: Option<usize>,
    pub clip: u32,
    pub converge: f32,
    pub offset: [f32; 2],
    pub shot_in: f32,
    pub pause: f32,
    pub scan_in: f32,
    /// The pilot's aim (Chopper Gunner): pitch, yaw; and fire held.
    pub aim: [f32; 2],
    pub firing: bool,
}

/// The AC-130 over the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gunship {
    pub owner: usize,
    pub center: [f32; 3],
    pub angle: f32,
    pub leave_ms: u32,
    /// The pilot's aim (pitch, yaw), the gun in use (0 = 105, 1 = 40, 2 = 25 mm) and when each is ready.
    pub aim: [f32; 2],
    pub gun: usize,
    pub ready_ms: [u32; 3],
    pub firing: bool,
}

/// A shell from the AC-130 in flight: it lands at `at` when due.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Round {
    pub owner: usize,
    pub at: [f32; 3],
    pub due_ms: u32,
    pub blast: Blast,
}

impl Heli {
    pub fn origin_eye(&self) -> [f32; 3] {
        [self.origin[0], self.origin[1], self.origin[2] - GUNNER_DROP]
    }
}

impl Gunship {
    /// Where the pilot sits.
    pub fn eye(&self) -> [f32; 3] {
        let d = yaw_dir(self.angle);
        [
            self.center[0] + d[0] * AC130_RADIUS,
            self.center[1] + d[1] * AC130_RADIUS,
            self.center[2] + AC130_HEIGHT,
        ]
    }
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = libm::sqrtf(v.iter().map(|x| x * x).sum()).max(1e-6);
    v.map(|x| x / l)
}

impl Arena {
    /// Fighter `owner` calls `streak` (attack helicopter, Pave Low, Chopper Gunner or AC-130).
    pub fn call_air(&mut self, owner: usize, streak: Streak, now: u32) {
        let s = &mut self.streaks;
        let r = s.rules;
        let m = s.map;
        let center = [m.center[0], m.center[1], m.center[2] + HELI_HEIGHT];
        if streak == Streak::Ac130 {
            let yaw = self.fighters[owner].player.view().1;
            s.gunship = Some(Gunship {
                owner,
                center: m.center,
                angle: s.random() * 360.0,
                leave_ms: now.wrapping_add((r.ac130_stay_s * 1000.0) as u32),
                aim: [60.0, yaw],
                gun: 0,
                ready_ms: [now; 3],
                firing: false,
            });
        } else {
            let kind = match streak {
                Streak::PaveLow => HeliKind::PaveLow,
                Streak::ChopperGunner => HeliKind::Gunner,
                _ => HeliKind::Attack,
            };
            let from = s.random() * 360.0;
            let d = yaw_dir(from);
            let out = m.radius * 3.0 + 4000.0;
            let health = if kind == HeliKind::PaveLow {
                r.heli_health * 2.0
            } else {
                r.heli_health
            };
            let stay = if kind == HeliKind::Gunner {
                r.gunner_stay_s
            } else {
                r.heli_stay_s
            };
            let yaw = self.fighters[owner].player.view().1;
            s.helis.push(Heli {
                owner,
                kind,
                origin: [center[0] + d[0] * out, center[1] + d[1] * out, center[2]],
                yaw: from + 180.0,
                phase: Phase::Arriving,
                center,
                radius: (m.radius * 0.6).max(600.0),
                angle: from,
                health,
                leave_ms: now.wrapping_add((stay * 1000.0) as u32),
                target: None,
                clip: r.heli_clip as u32,
                converge: 1.0,
                offset: [AIM_OFFSET, 0.0],
                shot_in: 0.0,
                pause: 0.0,
                scan_in: 0.0,
                aim: [45.0, yaw],
                firing: false,
            });
        }
        s.notes.push(Note::Called {
            fighter: owner,
            streak,
        });
    }

    /// The pilot's look and fire for fighter `owner`'s Chopper Gunner or AC-130 (`look` in degrees this frame;
    /// `switch` changes the AC-130's gun).
    pub fn steer_air(&mut self, owner: usize, look: [f32; 2], fire: bool, switch: bool) {
        if let Some(g) = self.streaks.gunship.as_mut().filter(|g| g.owner == owner) {
            g.aim[0] = (g.aim[0] + look[0]).clamp(10.0, 89.0);
            g.aim[1] += look[1];
            g.firing = fire;
            if switch {
                g.gun = (g.gun + 1) % 3;
            }
            return;
        }
        if let Some(h) = self
            .streaks
            .helis
            .iter_mut()
            .find(|h| h.owner == owner && h.kind == HeliKind::Gunner)
        {
            h.aim[0] = (h.aim[0] + look[0]).clamp(-10.0, 89.0);
            h.aim[1] += look[1];
            h.firing = fire;
        }
    }
}

/// The nearest enemy of `owner` in `range` of `from` and in sight.
fn nearest_target(
    a: &Arena,
    clip: &MapClip<'_>,
    owner: usize,
    from: [f32; 3],
    range: f32,
) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, f) in a.fighters.iter().enumerate() {
        if !f.life.alive() || (i == 0 && !a.human_present) || !a.enemies(owner, i) {
            continue;
        }
        let o = f.player.ps.origin;
        let at = [o[0], o[1], o[2] + 40.0];
        let d2 = dist2(at, from);
        if d2 > range * range || best.is_some_and(|b| b.0 <= d2) {
            continue;
        }
        if clip.trace(from, at, [0.0; 3], [0.0; 3], MASK_SHOT).fraction < 1.0 {
            continue;
        }
        best = Some((d2, i));
    }
    best.map(|b| b.1)
}

/// What a killstreak gun's shot does: an explosive one blows up where it hits, a bullet hits as a bullet.
/// A bullet's shot and which helicopter's gun ([`gun_of`]) fired it.
enum Hit {
    Bullet(usize, crate::gun::Shot, HeliKind),
    Blast(usize, [f32; 3], Blast),
}

/// The gun a helicopter of `kind` fires (the attack helicopter's when the pack lacks its own).
fn gun_of(w: &super::Weapons, kind: HeliKind) -> Option<(Gunline, Option<Blast>)> {
    match kind {
        HeliKind::Attack => w.heli_gun,
        HeliKind::PaveLow => w.pavelow_gun.or(w.heli_gun),
        HeliKind::Harrier => w.harrier_gun.or(w.heli_gun),
        HeliKind::Gunner => w.gunner,
    }
}

fn fire(
    clip: &MapClip<'_>,
    owner: usize,
    from: [f32; 3],
    at: [f32; 3],
    g: &(Gunline, Option<Blast>),
    kind: HeliKind,
) -> Hit {
    let dir = norm(core::array::from_fn(|k| at[k] - from[k]));
    let shot = super::sentry::straight_shot(clip, from, dir, 12_000.0);
    match g.1 {
        Some(b) if b.radius > 0.0 => Hit::Blast(owner, shot.end, b),
        _ => Hit::Bullet(owner, shot, kind),
    }
}

/// Fly the helicopters and the AC-130 and fire their guns.
pub(super) fn update(
    a: &mut Arena,
    s: &mut Streaks,
    clip: &MapClip<'_>,
    dt: f32,
    now: u32,
    out: &mut Vec<Event>,
) {
    let due = |t: u32| (now.wrapping_sub(t) as i32) >= 0;
    // Harriers due after their strike's runs.
    let mut k = 0;
    while k < s.pending_harriers.len() {
        if due(s.pending_harriers[k].0) {
            let (_, owner, target, yaw) = s.pending_harriers.swap_remove(k);
            let r = s.rules;
            let f = yaw_dir(yaw);
            let center = [target[0], target[1], target[2] + r.strike_height];
            s.helis.push(Heli {
                owner,
                kind: HeliKind::Harrier,
                origin: [
                    target[0] - f[0] * r.strike_distance * 0.25,
                    target[1] - f[1] * r.strike_distance * 0.25,
                    center[2],
                ],
                yaw,
                phase: Phase::Arriving,
                center,
                radius: 0.0,
                angle: 0.0,
                health: r.harrier_health,
                leave_ms: now.wrapping_add((r.harrier_stay_s * 1000.0) as u32),
                target: None,
                clip: r.harrier_burst as u32,
                converge: 1.0,
                offset: [0.0; 2],
                shot_in: 0.0,
                pause: 0.0,
                scan_in: 0.0,
                aim: [45.0, yaw],
                firing: false,
            });
        } else {
            k += 1;
        }
    }
    let r = s.rules;
    let guns = s.weapons;
    let mut hits: Vec<Hit> = Vec::new();
    let mut rolls = [0.0f32; 16];
    for x in &mut rolls {
        *x = s.random();
    }
    let mut roll = rolls.into_iter().cycle();
    for h in &mut s.helis {
        // Flight.
        if h.phase != Phase::Leaving && due(h.leave_ms) {
            h.phase = Phase::Leaving;
        }
        match h.phase {
            Phase::Arriving => {
                let goal = if h.kind == HeliKind::Harrier {
                    h.center
                } else {
                    let d = yaw_dir(h.angle);
                    [
                        h.center[0] + d[0] * h.radius,
                        h.center[1] + d[1] * h.radius,
                        h.center[2],
                    ]
                };
                let to = [goal[0] - h.origin[0], goal[1] - h.origin[1]];
                let dist = libm::hypotf(to[0], to[1]);
                let step = ARRIVE_SPEED * dt;
                if dist <= step {
                    h.origin = goal;
                    h.phase = Phase::Circling;
                } else {
                    h.origin[0] += to[0] / dist * step;
                    h.origin[1] += to[1] / dist * step;
                    h.yaw = libm::atan2f(to[1], to[0]).to_degrees();
                }
            }
            Phase::Circling if h.kind == HeliKind::Harrier => {
                // A slow drift about its spot (`randomHarrierMovement`), facing its target.
                h.angle += 20.0 * dt;
                let d = yaw_dir(h.angle);
                h.origin = [
                    h.center[0] + d[0] * 200.0,
                    h.center[1] + d[1] * 200.0,
                    h.center[2],
                ];
            }
            Phase::Circling => {
                h.angle += (LOOP_SPEED / h.radius.max(1.0)).to_degrees() * dt;
                let d = yaw_dir(h.angle);
                h.origin = [
                    h.center[0] + d[0] * h.radius,
                    h.center[1] + d[1] * h.radius,
                    h.center[2],
                ];
                h.yaw = h.angle + 90.0;
            }
            Phase::Leaving => {
                let d = yaw_dir(h.yaw);
                h.origin[0] += d[0] * ARRIVE_SPEED * dt;
                h.origin[1] += d[1] * ARRIVE_SPEED * dt;
                h.origin[2] += 200.0 * dt;
            }
        }
        if h.phase == Phase::Leaving {
            continue;
        }
        let muzzle = [h.origin[0], h.origin[1], h.origin[2] - 40.0];
        // The Chopper Gunner's pilot (a bot: the nearest enemy in sight).
        if h.kind == HeliKind::Gunner {
            let Some(g) = guns.gunner else {
                continue;
            };
            let bot = a.fighters[h.owner].brain.is_some();
            let aim_at = if bot {
                h.scan_in -= dt;
                if h.scan_in <= 0.0 {
                    h.scan_in = r.heli_targeting_s;
                    h.target = nearest_target(a, clip, h.owner, muzzle, 8000.0);
                }
                h.target.map(|t| {
                    let o = a.fighters[t].player.ps.origin;
                    [o[0], o[1], o[2] + 40.0]
                })
            } else if h.firing {
                let (dir, _, _) = math_iw4::angle_vectors([h.aim[0], h.aim[1], 0.0]);
                Some(core::array::from_fn(|k| muzzle[k] + dir[k] * 8000.0))
            } else {
                None
            };
            h.shot_in -= dt;
            if let Some(at) = aim_at
                && h.shot_in <= 0.0
            {
                h.shot_in = (g.0.facts.fire_time_ms as f32 / 1000.0).max(0.03);
                let jitter: [f32; 3] =
                    core::array::from_fn(|_| (roll.next().unwrap_or(0.5) - 0.5) * 48.0);
                let at = core::array::from_fn(|k| at[k] + jitter[k]);
                hits.push(fire(clip, h.owner, muzzle, at, &g, h.kind));
            }
            continue;
        }
        let Some(g) = gun_of(&guns, h.kind) else {
            continue;
        };
        // `heli_targeting`: a new target every `heli_targeting_delay`.
        h.scan_in -= dt;
        if h.scan_in <= 0.0 {
            h.scan_in = r.heli_targeting_s;
            let range = if h.kind == HeliKind::Harrier {
                r.harrier_range
            } else {
                r.heli_range
            };
            let t = nearest_target(a, clip, h.owner, muzzle, range);
            if t != h.target {
                // `attack_primary`: start 96 units off the new target, converging.
                let ang = roll.next().unwrap_or(0.0) * core::f32::consts::TAU;
                h.offset = [libm::cosf(ang) * AIM_OFFSET, libm::sinf(ang) * AIM_OFFSET];
                h.converge = 1.0;
            }
            h.target = t;
        }
        if h.pause > 0.0 {
            h.pause -= dt;
            continue;
        }
        let Some(t) = h.target.filter(|&t| a.fighters[t].life.alive()) else {
            continue;
        };
        let fire_s = if h.kind == HeliKind::Harrier {
            0.1
        } else {
            (g.0.facts.fire_time_ms as f32 / 1000.0).max(0.03)
        };
        h.shot_in -= dt;
        while h.shot_in <= 0.0 && h.clip > 0 {
            h.shot_in += fire_s;
            h.clip -= 1;
            let o = a.fighters[t].player.ps.origin;
            let at = [
                o[0] + h.offset[0] * h.converge + (roll.next().unwrap_or(0.5) - 0.5) * 12.0,
                o[1] + h.offset[1] * h.converge + (roll.next().unwrap_or(0.5) - 0.5) * 12.0,
                o[2] + 40.0,
            ];
            h.converge = (h.converge - CONVERGE).max(0.0);
            hits.push(fire(clip, h.owner, muzzle, at, &g, h.kind));
        }
        if h.clip == 0 {
            // A clip done: 0.5-2 s off (the Harrier: a second), and a full clip again.
            h.pause = if h.kind == HeliKind::Harrier {
                1.0
            } else {
                0.5 + 1.5 * roll.next().unwrap_or(0.5)
            };
            h.clip = if h.kind == HeliKind::Harrier {
                r.harrier_burst as u32
            } else {
                r.heli_clip as u32
            };
            h.shot_in = 0.0;
        }
    }
    let far = s.map.radius * 4.0 + 20_000.0;
    let c = s.map.center;
    s.helis.retain(|h| {
        h.health > 0.0
            && !(h.phase == Phase::Leaving
                && libm::hypotf(h.origin[0] - c[0], h.origin[1] - c[1]) > far)
    });
    // The AC-130.
    if let Some(mut g) = s.gunship {
        g.angle += 360.0 / r.ac130_turn_s.max(1.0) * dt;
        let eye = g.eye();
        if a.fighters[g.owner].brain.is_some() {
            // A bot's gunner: the nearest enemy in sight, the 105 when ready, else the 40, else the 25.
            g.firing = false;
            if let Some(t) = nearest_target(a, clip, g.owner, eye, 12_000.0) {
                let o = a.fighters[t].player.ps.origin;
                let ang = math_iw4::vect_to_angles(core::array::from_fn(|k| o[k] - eye[k]));
                g.aim = [ang[0], ang[1]];
                g.gun = (0..3).find(|&k| due(g.ready_ms[k])).unwrap_or(2);
                g.firing = true;
            }
        }
        if g.firing
            && due(g.ready_ms[g.gun])
            && let Some(b) = guns.ac130[g.gun]
        {
            g.ready_ms[g.gun] = now.wrapping_add((r.ac130_reload_s[g.gun] * 1000.0) as u32);
            let (dir, _, _) = math_iw4::angle_vectors([g.aim[0], g.aim[1], 0.0]);
            let shot = super::sentry::straight_shot(clip, eye, dir, 20_000.0);
            let d = libm::sqrtf(dist2(eye, shot.end));
            let speed = guns.ac130_speed[g.gun].max(2000.0);
            s.rounds.push(Round {
                owner: g.owner,
                at: shot.end,
                due_ms: now.wrapping_add((d / speed * 1000.0) as u32),
                blast: b,
            });
        }
        s.gunship = (!due(g.leave_ms)).then_some(g);
    }
    let mut k = 0;
    while k < s.rounds.len() {
        if due(s.rounds[k].due_ms) {
            let rd = s.rounds.swap_remove(k);
            hits.push(Hit::Blast(rd.owner, rd.at, rd.blast));
        } else {
            k += 1;
        }
    }
    for h in hits {
        match h {
            Hit::Bullet(owner, shot, kind) => {
                if let Some((g, _)) = gun_of(&guns, kind) {
                    out.push(Event::BotFired {
                        bot: owner,
                        path: Some((shot.start, shot.pen_end)),
                    });
                    a.bullet(owner, &shot, &g.facts, now, out);
                }
            }
            Hit::Blast(owner, at, b) => {
                super::predator::blast(
                    a,
                    clip,
                    owner,
                    at,
                    b.weapon,
                    0,
                    (b.radius, b.inner, b.outer),
                    now,
                    out,
                );
            }
        }
    }
}

/// A bullet from `start` to `end` against the helicopters of the shooter's enemies (`hostile[owner]`; each a box
/// about its middle): the nearest one in the way takes the damage, a third of it through its armour
/// (`heli_armor_bulletdamage`). Returns whether one did.
pub fn bullet_hits_heli(
    s: &mut Streaks,
    hostile: &[bool],
    start: [f32; 3],
    end: [f32; 3],
    damage: f32,
) -> bool {
    let mut best: Option<(f32, usize)> = None;
    for (i, h) in s.helis.iter().enumerate() {
        if !hostile.get(h.owner).copied().unwrap_or(false) {
            continue;
        }
        let half = if h.kind == HeliKind::Harrier {
            [180.0, 180.0, 60.0]
        } else {
            [220.0, 220.0, 90.0]
        };
        let o = h.origin;
        let b = crate::damage::HitBox {
            loc: 0,
            center: o,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            half,
        };
        if let Some((t, _)) = crate::damage::first_hit(start, end, core::slice::from_ref(&b))
            && best.is_none_or(|x| t < x.0)
        {
            best = Some((t, i));
        }
    }
    let Some((_, i)) = best else {
        return false;
    };
    s.helis[i].health -= damage * HELI_ARMOR;
    true
}

/// `level.heli_armor_bulletdamage`.
pub const HELI_ARMOR: f32 = 0.3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips;
    use crate::player::Player;

    fn cannon() -> (Gunline, Option<Blast>) {
        let c = weapon_iw4::CapturedCombatInput {
            fire_time_ms: 100,
            clip_size: 40,
            start_ammo: 40,
            max_ammo: 40,
            reload_time_ms: 1000,
            reload_empty_time_ms: 1000,
            raise_time_ms: 100,
            first_raise_time_ms: 100,
            drop_time_ms: 100,
            damage: 60,
            min_damage: 40,
            max_damage_range: 2048.0,
            min_damage_range: 4096.0,
            ..weapon_iw4::CapturedCombatInput::default()
        };
        (
            Gunline {
                weapon: 0,
                facts: weapon_iw4::WeaponCombatFacts::try_from_captured(c).expect("usable"),
            },
            None,
        )
    }

    fn arena(clip: &MapClip<'_>) -> Arena {
        let spawns = [([300.0, 0.0, 0.0], 0.0), ([300.0, 0.0, 0.0], 0.0)];
        let human = Player::spawn(clip, [-300.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(
            clip,
            &spawns,
            1,
            human,
            wpk::rules::Rules::default(),
            |_| None,
        );
        let g = cannon();
        a.streaks = Streaks::new(
            wpk::streaks::StreakRules::default(),
            super::super::Weapons {
                heli_gun: Some(g),
                gunner: Some(g),
                ..Default::default()
            },
            super::super::MapInfo {
                center: [0.0; 3],
                radius: 1500.0,
            },
        );
        a
    }

    #[test]
    fn an_attack_helicopter_flies_in_kills_and_leaves_after_its_stay() {
        let floor = test_clips::boxes(&[([-40000.0, -40000.0, -64.0], [40000.0, 40000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        a.call_air(0, Streak::Helicopter, 0);
        let mut ev = Vec::new();
        let mut now = 0;
        let mut circling_at = None;
        while now < 70_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
            if circling_at.is_none()
                && a.streaks
                    .helis
                    .first()
                    .is_some_and(|h| h.phase == Phase::Circling)
            {
                circling_at = Some(now);
            }
            if !a.fighters[1].life.alive() {
                break;
            }
        }
        // 3 × 1500 + 4000 out, at 100 mph: about 5 s to its loop.
        let c = circling_at.expect("it reached its loop");
        assert!((3000..8000).contains(&c), "circling at {c}");
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
        // 60 s after the call it leaves, and is gone once far out.
        let mut now = 60_000;
        while now < 120_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert!(a.streaks.helis.is_empty());
    }

    #[test]
    fn the_cannon_converges_onto_its_target() {
        let mut h = 1.0f32;
        for _ in 0..20 {
            h = (h - CONVERGE).max(0.0);
        }
        assert_eq!(h, 0.0, "on target after 20 shots in sight");
    }

    #[test]
    fn bullets_wear_a_helicopter_down_through_its_armour() {
        let floor = test_clips::boxes(&[([-40000.0, -40000.0, -64.0], [40000.0, 40000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        a.call_air(1, Streak::Helicopter, 0);
        let o = a.streaks.helis[0].origin;
        let start = [o[0], o[1], 0.0];
        let end = [o[0], o[1], o[2] + 500.0];
        // 1500 health, a third of each hit: 100 hits of 50.
        let hostile = [false, true];
        for _ in 0..99 {
            assert!(bullet_hits_heli(&mut a.streaks, &hostile, start, end, 50.0));
        }
        let mut ev = Vec::new();
        a.update_killstreaks(&clip, 0.05, 50, &mut ev);
        assert_eq!(a.streaks.helis.len(), 1);
        assert!(bullet_hits_heli(&mut a.streaks, &hostile, start, end, 50.0));
        a.update_killstreaks(&clip, 0.05, 100, &mut ev);
        assert!(a.streaks.helis.is_empty(), "shot down");
        // Its owner's own bullets (and its allies') pass.
        a.call_air(0, Streak::Helicopter, 0);
        assert!(!bullet_hits_heli(
            &mut a.streaks,
            &[false, true],
            start,
            end,
            50.0
        ));
    }

    #[test]
    fn the_ac130_circles_for_its_stay_and_its_rounds_land_where_aimed() {
        let floor = test_clips::boxes(&[([-40000.0, -40000.0, -64.0], [40000.0, 40000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let mut a = arena(&clip);
        a.streaks.weapons.ac130 = [
            Some(Blast {
                weapon: 1,
                radius: 600.0,
                inner: 1000.0,
                outer: 300.0,
            }),
            None,
            None,
        ];
        a.call_air(0, Streak::Ac130, 0);
        let eye = a.streaks.gunship.unwrap().eye();
        // Aim at the bot and fire the 105.
        let o = a.fighters[1].player.ps.origin;
        let ang = math_iw4::vect_to_angles(core::array::from_fn(|k| o[k] - eye[k]));
        {
            let g = a.streaks.gunship.as_mut().unwrap();
            g.aim = [ang[0], ang[1]];
            g.angle -= 0.0;
        }
        a.steer_air(0, [0.0, 0.0], true, false);
        let mut ev = Vec::new();
        let mut now = 0;
        while now < 6000 && a.fighters[1].life.alive() {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert!(!a.fighters[1].life.alive(), "the 105 landed on the bot");
        assert!(a.streaks.gunship.is_some());
        a.update_killstreaks(&clip, 0.05, 40_050, &mut ev);
        assert!(a.streaks.gunship.is_none(), "gone after 40 s");
    }
}

//! Thrown grenades, as IW4L's sim runs them (`sim::equipment`): the throw velocity (the weapon's projectile speed
//! along the view, its extra up / forward speed, plus the thrower's velocity along the throw), flight under IW4's
//! gravity, bouncing off the world by the weapon's per-surface factors (`bounce_reflected_scale`: the reflected
//! velocity scaled between the parallel and perpendicular factor by the angle of incidence), and the fuse (cooked
//! time left, else the weapon's fuse). The explosion damages players as IW4L's `radius_player_attempts`: inner to
//! outer damage over the radius by the distance to each player's box, scaled by how many of IW4's sample points
//! around the player the blast can see (`can_damage_player_vis_scale`).

use alloc::vec::Vec;

use wpk::rules::Rules;
use wpk::weapon::EquipRecord;

/// What a grenade does when it goes off (by its offhand class and IW4 projectile explosion type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Damage over the radius (frag, semtex).
    Blast,
    /// Blinds by distance and where you look (flashbang: explosion type 2).
    Flash,
    /// A little damage and a slowing shock by distance (stun: smoke class, explosion type 3).
    Stun,
    /// Smoke (explosion type 5): not in this build.
    Smoke,
    /// A throwing knife: no explosion; it hurts whoever it hits and can be picked back up.
    Knife,
}

/// IW4 offhand class of throwing knives.
pub const OFFHAND_CLASS_THROWINGKNIFE: i32 = 4;

pub fn effect(f: &EquipRecord) -> Effect {
    match (f.offhand_class, f.projectile_explosion_type) {
        (OFFHAND_CLASS_THROWINGKNIFE, _) => Effect::Knife,
        (_, 2) => Effect::Flash,
        (2, 3) => Effect::Stun,
        (_, 5) => Effect::Smoke,
        _ => Effect::Blast,
    }
}

/// Seconds a flashbang at `origin` blinds a player whose eye is at `eye` looking along `forward` (IW4L
/// `apply_flashbang_blast`'s distance and angle amounts, then `_flashgrenades.gsc`'s duration). 0: not blinded.
pub fn flash_seconds(
    r: &Rules,
    f: &EquipRecord,
    origin: [f32; 3],
    eye: [f32; 3],
    forward: [f32; 3],
) -> f32 {
    let min_r = (f.explosion_radius_min as f32).max(1.0);
    let max_r = (f.explosion_radius as f32).max(min_r);
    let to: [f32; 3] = core::array::from_fn(|k| origin[k] - eye[k]);
    let dist = len(to);
    if dist > max_r {
        return 0.0;
    }
    let amount_distance = if dist <= min_r || max_r <= min_r {
        1.0
    } else {
        1.0 - (dist - min_r) / (max_r - min_r)
    };
    let dot = if dist > 0.0 {
        (to[0] * forward[0] + to[1] * forward[1] + to[2] * forward[2]) / dist
    } else {
        1.0
    };
    let mut amount_angle = (dot + 1.0) * 0.5;
    if amount_angle < r.flash_angle_min {
        amount_angle = r.flash_angle_min;
    } else if amount_angle > r.flash_angle_full {
        amount_angle = 1.0;
    }
    let s = amount_distance * amount_angle * r.flash_seconds;
    if s < r.flash_min_seconds { 0.0 } else { s }
}

/// Seconds a stun grenade at distance `dist` shocks a player it hurt (`_weapons.gsc` `onWeaponDamage`).
pub fn stun_seconds(r: &Rules, dist: f32) -> f32 {
    let scale = (1.0 - dist / r.stun_radius.max(1.0)).max(0.0);
    r.stun_base + r.stun_extra * scale
}

use crate::collision::MapClip;

/// IW4's `g_gravity`.
pub const GRAVITY: f32 = 800.0;
/// IW4 `MASK_SHOT`-like contents a grenade collides with (`MASK_MISSILE`: solid, player clip excluded).
pub const MASK_MISSILE: u32 = 0x0080_0811;
/// A bounce off a floor (normal z over 0.2) that leaves less than this speed stops the grenade (the IW / Quake 3
/// engines' missile stop check).
const REST_SPEED: f32 = 40.0;
/// A bounce that changes the speed by more than this makes a sound (IW4L `BOUNCE_EVENT_SPEED_DELTA`).
const BOUNCE_SOUND_SPEED_CHANGE: f32 = 100.0;
/// A grenade's collision half size.
const HALF: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grenade {
    pub owner: usize,
    /// The weapon thrown (index in the weapon list).
    pub weapon: u32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub detonate_at_ms: u32,
    pub resting: bool,
    pub facts: EquipRecord,
    /// The last step bounced hard enough to be heard (IW4L: a speed change over 100).
    pub bounced: bool,
    /// Where the last step started (for a knife's hit on a player along the way).
    pub from: [f32; 3],
    /// A knife that already hit someone (it drops and does no more harm).
    pub spent: bool,
    /// Whether the fuse runs (IW4L `settle_equipment`: a charge without a timed detonation that sticks loses it).
    pub fused: bool,
    /// The throw's yaw in degrees (a claymore faces it).
    pub yaw: f32,
    pub thrown_ms: u32,
    /// When it stuck (C4 is live 50 ms later, `c4Activate`; a claymore starts watching, `claymoreDetonation`).
    pub stuck_ms: Option<u32>,
    /// When a tripped claymore goes off.
    pub trip_at_ms: Option<u32>,
    /// A launched projectile's flight (rockets, grenade launchers); None for thrown weapons.
    pub flight: Option<Flight>,
}

/// IW4 `WEAPCLASS_GRENADE`: a projectile weapon of this class is a grenade launcher (IW4L `fire_weapon_kind`).
pub const WEAPCLASS_GRENADE: i32 = 6;
/// IW4 `guidedMissileType` of the Javelin (IW4L `missile_guidance`: climbs, then dives on its target).
pub const GUIDANCE_JAVELIN: i32 = 3;
/// IW4L `ROCKET_CLEANUP_MS`: a rocket that hits nothing goes away after a minute.
pub const ROCKET_CLEANUP_MS: u32 = 60_000;
/// IW4L `GRENADE_DEFAULT_FUSE_MS`: a launched grenade without its own fuse or arming distance.
const LAUNCHER_DEFAULT_FUSE_MS: u32 = 30_000;

/// How a launched projectile ended its last step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Contact {
    /// It hit the world armed: it explodes (IW4L `TickOutcome::Contact` with `proj_impact_explode`).
    Explode { normal: [f32; 3], surface: u8 },
    /// It hit before its arming distance: a dud, no blast (IW4L `ProjectileHitGeometry::Dud`).
    Dud { normal: [f32; 3], surface: u8 },
}

/// A launched projectile (IW4L `sim::missile::fire_missile`, `equipment::spawn_grenade_projectile` with
/// `GrenadeLaunchKind::Launcher`, then `think_projectile` every step).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    pub record: wpk::weapon::MissileRecord,
    /// Under gravity (IW4L `TR_GRAVITY`: grenade launchers and the Javelin) or straight (`TR_LINEAR`: rockets).
    pub gravity: bool,
    /// Distance flown (IW4L `travel_distance`): armed once past `projectile_activate_dist`.
    pub travel: f32,
    /// Removed without a blast at this time (IW4L `cleanup_at_ms`).
    pub cleanup_ms: u32,
    /// Where it is guided to (a locked point; IW4L `MissileGuide`), top attack, the Javelin's stage and whether it
    /// flew past its target.
    pub target: Option<[f32; 3]>,
    pub top: bool,
    pub stage: u8,
    pub passed: bool,
    /// The last step's end: hit the world, or still flying.
    pub contact: Option<Contact>,
}

impl Flight {
    /// IW4L `ProjectileState::is_armed`.
    pub fn armed(&self, activate_dist: i32) -> bool {
        activate_dist <= 0 || self.travel >= activate_dist as f32
    }
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = len(v);
    (l > 1e-6).then(|| v.map(|x| x / l))
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Turn unit `current` toward unit `wanted` by at most `max` radians (IW4L `missile_guidance::turn_toward`: about
/// their common axis).
pub fn turn_toward(current: [f32; 3], wanted: [f32; 3], max: f32) -> [f32; 3] {
    let c = dot(current, wanted).clamp(-1.0, 1.0);
    let angle = libm::acosf(c);
    if angle <= max || angle.is_nan() {
        return wanted;
    }
    let axis = unit(cross(current, wanted)).unwrap_or_else(|| {
        // Opposite: any axis at right angles.
        let other = if libm::fabsf(current[0]) < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        unit(cross(current, other)).unwrap_or([0.0, 0.0, 1.0])
    });
    // Rodrigues: v cos + (k × v) sin + k (k·v)(1 − cos).
    let (s, co) = (libm::sinf(max), libm::cosf(max));
    let kv = cross(axis, current);
    let kd = dot(axis, current);
    core::array::from_fn(|k| current[k] * co + kv[k] * s + axis[k] * kd * (1.0 - co))
}

/// What a planted charge is (by its numbers: a charge has no timed detonation and sticks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charge {
    /// Sticks to the ground facing the throw and goes off when an enemy walks into its cone (stickiness 4).
    Claymore,
    /// Sticks to anything and goes off when its owner detonates it (stickiness 2).
    C4,
}

pub fn charge(f: &EquipRecord) -> Option<Charge> {
    if f.timed_detonation || f.offhand_class == OFFHAND_CLASS_THROWINGKNIFE {
        return None;
    }
    match f.stickiness {
        4 => Some(Charge::Claymore),
        2 => Some(Charge::C4),
        _ => None,
    }
}

/// IW4L `sticks_to_surface`: stickiness 1 and 2 stick to anything, 3 and 4 to ground (normal z over 0.7).
pub fn sticks_to(stickiness: i32, normal: [f32; 3]) -> bool {
    matches!(stickiness, 1 | 2) || (matches!(stickiness, 3 | 4) && normal[2] > 0.7)
}

fn len(v: [f32; 3]) -> f32 {
    libm::sqrtf(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}

/// The throw velocity along `dir` (unit, the view) for a thrower moving at `owner_vel` (IW4L
/// `grenade_launch_velocity` with `project_owner_velocity`).
pub fn launch_velocity(dir: [f32; 3], f: &EquipRecord, owner_vel: [f32; 3]) -> [f32; 3] {
    let speed = f.projectile_speed as f32;
    let mut v = [
        dir[0] * speed,
        dir[1] * speed,
        dir[2] * speed + f.projectile_speed_up as f32,
    ];
    let flat_len = libm::sqrtf(dir[0] * dir[0] + dir[1] * dir[1]);
    if f.projectile_speed_forward != 0 && flat_len > 1e-6 {
        let extra = f.projectile_speed_forward as f32;
        v[0] += dir[0] / flat_len * extra;
        v[1] += dir[1] / flat_len * extra;
    }
    let l = len(v);
    if l < 1e-6 {
        return v;
    }
    let d = v.map(|x| x / l);
    let along = owner_vel[0] * d[0] + owner_vel[1] * d[1] + owner_vel[2] * d[2];
    core::array::from_fn(|k| v[k] + d[k] * along)
}

/// IW4L `bounce_reflected_scale`.
pub fn bounce(incoming: [f32; 3], normal: [f32; 3], parallel: f32, perpendicular: f32) -> [f32; 3] {
    let speed = len(incoming);
    let nl = len(normal);
    if speed < 1e-6 || nl < 1e-6 {
        return [0.0; 3];
    }
    let n = normal.map(|x| x / nl);
    let d = incoming[0] * n[0] + incoming[1] * n[1] + incoming[2] * n[2];
    let r: [f32; 3] = core::array::from_fn(|k| incoming[k] - 2.0 * d * n[k]);
    let incidence = (-d / speed).clamp(0.0, 1.0);
    let factor = parallel + (perpendicular - parallel) * incidence;
    r.map(|x| x * factor)
}

impl Grenade {
    /// A grenade thrown by `owner` from `origin` along `dir` (unit), with `fuse_left_ms` of a cooked fuse (None:
    /// the weapon's full fuse).
    #[allow(clippy::too_many_arguments)]
    pub fn throw(
        owner: usize,
        weapon: u32,
        facts: EquipRecord,
        origin: [f32; 3],
        dir: [f32; 3],
        owner_vel: [f32; 3],
        fuse_left_ms: Option<i32>,
        now_ms: u32,
    ) -> Grenade {
        let fuse = if facts.timed_detonation {
            fuse_left_ms
                .filter(|&f| f > 0)
                .unwrap_or(facts.fuse_time_ms)
        } else {
            facts.fuse_time_ms
        };
        // IW4L `grenade_deadlines`: default 30 s, at most 60 s; a knife has no fuse and goes away at the 60 s cap.
        let fuse = if facts.offhand_class == OFFHAND_CLASS_THROWINGKNIFE {
            60_000
        } else if fuse <= 0 {
            30_000
        } else {
            fuse.min(60_000)
        };
        Grenade {
            owner,
            weapon,
            origin,
            velocity: launch_velocity(dir, &facts, owner_vel),
            detonate_at_ms: now_ms.wrapping_add(fuse as u32),
            resting: false,
            facts,
            bounced: false,
            from: origin,
            spent: false,
            fused: true,
            yaw: libm::atan2f(dir[1], dir[0]).to_degrees(),
            thrown_ms: now_ms,
            stuck_ms: None,
            trip_at_ms: None,
            flight: None,
        }
    }

    /// A projectile launched by `owner` from `origin` along `dir` (unit, after spread; `right` the view's right):
    /// a grenade launcher's round (weapon class grenade: IW4L `grenade_launch_velocity`, under gravity, a 30 s fuse
    /// unless it has an arming distance) or a rocket (IW4L `fire_missile`: the weapon's speed along `dir` plus the
    /// owner's velocity, truncated to whole units; straight, except the Javelin, which leaves 0.3 up and 2.5 units to
    /// the right and falls). `target` is a locked point it steers to.
    #[allow(clippy::too_many_arguments)]
    pub fn launch(
        owner: usize,
        weapon: u32,
        weap_class: i32,
        facts: EquipRecord,
        record: wpk::weapon::MissileRecord,
        origin: [f32; 3],
        dir: [f32; 3],
        right: [f32; 3],
        owner_vel: [f32; 3],
        target: Option<[f32; 3]>,
        now_ms: u32,
    ) -> Grenade {
        let trunc = |v: [f32; 3]| v.map(|x| x as i32 as f32);
        let launcher = weap_class == WEAPCLASS_GRENADE;
        let javelin = record.guidance == GUIDANCE_JAVELIN;
        let (mut origin, mut dir) = (origin, dir);
        if javelin && !launcher {
            dir = unit([dir[0], dir[1], dir[2] + 0.3]).unwrap_or(dir);
            origin = core::array::from_fn(|k| origin[k] + right[k] * 2.5);
        }
        let velocity = if launcher {
            trunc(launch_velocity(dir, &facts, owner_vel))
        } else {
            let speed = facts.projectile_speed as f32;
            trunc(core::array::from_fn(|k| dir[k] * speed + owner_vel[k]))
        };
        let cleanup_ms = now_ms.wrapping_add(ROCKET_CLEANUP_MS);
        // IW4L `grenade_deadlines` (Launcher): no timed fuse → none with an arming distance, else 30 s.
        let (fused, detonate_at_ms) = if launcher {
            if facts.timed_detonation && facts.fuse_time_ms > 0 {
                (
                    true,
                    now_ms.wrapping_add((facts.fuse_time_ms as u32).min(ROCKET_CLEANUP_MS)),
                )
            } else if facts.projectile_activate_dist != 0 {
                (false, cleanup_ms)
            } else {
                (true, now_ms.wrapping_add(LAUNCHER_DEFAULT_FUSE_MS))
            }
        } else {
            (false, cleanup_ms)
        };
        Grenade {
            owner,
            weapon,
            origin,
            velocity,
            detonate_at_ms,
            resting: false,
            facts,
            bounced: false,
            from: origin,
            spent: false,
            fused,
            yaw: libm::atan2f(dir[1], dir[0]).to_degrees(),
            thrown_ms: now_ms,
            stuck_ms: None,
            trip_at_ms: None,
            flight: Some(Flight {
                record,
                gravity: launcher || javelin,
                travel: 0.0,
                cleanup_ms,
                target,
                top: target.is_some() && javelin,
                stage: 0,
                passed: false,
                contact: None,
            }),
        }
    }

    /// Steer toward the locked point (IW4L `missile_guidance::steer`, point targets): the Javelin waits out its
    /// ignition delay, climbs (to 15000 above a top-attack target until it is 3000 over it within 50° of straight
    /// down, or 400 units off) then dives, speeding up while turning less than 30°; other guided missiles turn at
    /// 240°/s. `owner_feet` pulls a direct-attack Javelin's climb point 700 units back toward the owner.
    fn steer(&mut self, dt: f32, now_ms: u32, owner_feet: [f32; 3]) {
        let Some(fl) = self.flight.as_mut() else {
            return;
        };
        if fl.passed {
            return;
        }
        let Some(point) = fl.target else {
            return;
        };
        let Some(forward) = unit(self.velocity) else {
            return;
        };
        let delta: [f32; 3] = core::array::from_fn(|k| point[k] - self.origin[k]);
        let mut speed = len(self.velocity);
        let mut goal = point;
        let javelin = fl.record.guidance == GUIDANCE_JAVELIN;
        if javelin {
            if (now_ms.wrapping_sub(self.thrown_ms) as i32) < fl.record.ignition_delay_ms {
                return;
            }
            fl.stage = fl.stage.max(1);
            if fl.stage == 1 {
                let flat = unit([forward[0], forward[1], 0.0]).unwrap_or([0.0; 3]);
                let climb = libm::fabsf(libm::atanf(
                    (flat[0] * delta[0] + flat[1] * delta[1]) / delta[2],
                ))
                .to_degrees();
                let (ceiling, limit) = if fl.top { (3000.0, 50.0) } else { (0.0, 85.0) };
                let flat_dist = libm::sqrtf(delta[0] * delta[0] + delta[1] * delta[1]);
                if self.origin[2] - point[2] > ceiling && (climb < limit || flat_dist < 400.0) {
                    fl.stage = 2;
                } else {
                    goal[2] += if fl.top { 15000.0 } else { 10000.0 };
                    if !fl.top {
                        let back = unit([owner_feet[0] - goal[0], owner_feet[1] - goal[1], 0.0])
                            .unwrap_or([0.0; 3]);
                        goal[0] += back[0] * 700.0;
                        goal[1] += back[1] * 700.0;
                    }
                }
            }
            if fl.stage == 2
                && dot(delta, delta) <= 90000.0
                && dot(forward, unit(delta).unwrap_or([0.0; 3])) < -0.2
            {
                fl.passed = true;
                return;
            }
        }
        let Some(wanted) = unit(core::array::from_fn(|k| goal[k] - self.origin[k])) else {
            return;
        };
        let rate: f32 = if javelin {
            if fl.top { 100.0 } else { 60.0 }
        } else {
            240.0
        };
        let mut turn = 0.0;
        let direction = if javelin {
            // IW4L slerps yaw / pitch rotations; here the direction turns along the great circle by the same share.
            turn = (90.0 * (1.0 - dot(forward, wanted))).clamp(0.0, 180.0);
            if turn <= 0.1 || turn <= rate * dt {
                wanted
            } else {
                let full = libm::acosf(dot(forward, wanted).clamp(-1.0, 1.0));
                turn_toward(forward, wanted, full * (rate * dt / turn))
            }
        } else {
            turn_toward(forward, wanted, rate.to_radians() * dt)
        };
        if javelin {
            let climbing = fl.stage == 1 || self.velocity[2] > 0.0;
            if turn < 30.0 {
                let (accel, top) = if climbing {
                    (300.0, 1000.0)
                } else {
                    (4500.0, 2500.0)
                };
                speed = (speed + accel * dt).min(top);
            }
            speed *= 1.0 - turn / 180.0 * 0.05;
        }
        // A steered missile flies straight from here (IW4L sets `TR_LINEAR`).
        fl.gravity = false;
        self.velocity = direction.map(|x| x * speed);
    }

    /// One step of a launched projectile (IW4L `think_projectile` against the world): steer, move (under gravity or
    /// straight), and on a world hit explode when armed and `projImpactExplode`, else bounce when armed, else dud.
    /// `from` is where the step started (the arena tests it against the players). True when it goes off now (its
    /// contact or its fuse); a rocket past its cleanup time is gone (`cleanup_due`).
    pub fn fly(&mut self, clip: &MapClip<'_>, dt: f32, now_ms: u32, owner_feet: [f32; 3]) -> bool {
        self.bounced = false;
        self.from = self.origin;
        self.steer(dt, now_ms, owner_feet);
        let Some(fl) = self.flight else {
            return false;
        };
        let fuse_due = self.fused && (now_ms.wrapping_sub(self.detonate_at_ms) as i32) >= 0;
        if self.resting || dt <= 0.0 {
            return fuse_due;
        }
        let v = self.velocity;
        let g = if fl.gravity { GRAVITY } else { 0.0 };
        let end = [
            self.origin[0] + v[0] * dt,
            self.origin[1] + v[1] * dt,
            self.origin[2] + v[2] * dt - 0.5 * g * dt * dt,
        ];
        let t = clip.trace(self.origin, end, [0.0; 3], [0.0; 3], crate::gun::MASK_SHOT);
        let start = self.origin;
        if t.fraction >= 1.0 {
            self.origin = end;
            self.velocity[2] -= g * dt;
            if let Some(f) = self.flight.as_mut() {
                f.travel += len(core::array::from_fn(|k| end[k] - start[k]));
            }
            return fuse_due;
        }
        let hit: [f32; 3] = if t.startsolid != 0 { start } else { t.endpos };
        let normal = if t.startsolid != 0 {
            unit(v.map(|x| -x)).unwrap_or([0.0, 0.0, 1.0])
        } else {
            t.normal
        };
        self.velocity[2] -= g * dt * t.fraction;
        let surface = ((t.surface_flags >> 20) & 0x1f) as u8;
        let travel = fl.travel + len(core::array::from_fn(|k| hit[k] - start[k]));
        let armed = Flight { travel, ..fl }.armed(self.facts.projectile_activate_dist);
        let f = self.flight.as_mut().expect("flight");
        f.travel = travel;
        // The blast goes off a hair off the surface (the visibility traces start inside the world otherwise).
        self.origin = core::array::from_fn(|k| hit[k] + normal[k] * 1.0);
        if armed && fl.record.impact_explode {
            f.contact = Some(Contact::Explode { normal, surface });
            return true;
        }
        if !armed {
            f.contact = Some(Contact::Dud { normal, surface });
            return true;
        }
        // Armed without an impact explosion: it bounces by the surface's factors (IW4L `bounce_missile`).
        let s = usize::from(surface);
        let (par, perp) = (
            self.facts.parallel_bounce.get(s).copied().unwrap_or(0.0),
            self.facts
                .perpendicular_bounce
                .get(s)
                .copied()
                .unwrap_or(0.0),
        );
        let before = self.velocity;
        self.velocity = bounce(self.velocity, normal, par, perp);
        self.bounced =
            len(core::array::from_fn(|k| self.velocity[k] - before[k])) > BOUNCE_SOUND_SPEED_CHANGE;
        if normal[2] > 0.2 && len(self.velocity) < REST_SPEED {
            self.velocity = [0.0; 3];
            self.resting = true;
        }
        fuse_due
    }

    /// A launched projectile past its cleanup time with no blast due (IW4L's `Cleanup` outcome).
    pub fn expired(&self, now_ms: u32) -> bool {
        self.flight.is_some_and(|f| {
            (now_ms.wrapping_sub(f.cleanup_ms) as i32) >= 0
                && !(self.fused && (now_ms.wrapping_sub(self.detonate_at_ms) as i32) >= 0)
        })
    }

    pub fn is_knife(&self) -> bool {
        self.facts.offhand_class == OFFHAND_CLASS_THROWINGKNIFE
    }

    pub fn charge(&self) -> Option<Charge> {
        charge(&self.facts)
    }

    /// Go off at `at_ms` (a detonated C4, a tripped claymore, the oldest charge over the limit).
    pub fn detonate_at(&mut self, at_ms: u32) {
        self.fused = true;
        self.detonate_at_ms = at_ms;
    }

    /// The blast's cone (direction, cosine of the half angle) when the weapon has one (IW4L `ExplosionBlast::cone`:
    /// along the charge's facing).
    pub fn cone(&self) -> Option<([f32; 3], f32)> {
        let a = self.facts.damage_cone_angle;
        (a > 0.0 && a < 180.0).then(|| {
            let y = self.yaw.to_radians();
            (
                [libm::cosf(y), libm::sinf(y), 0.0],
                libm::cosf(a.to_radians()),
            )
        })
    }

    /// Fly for `dt` seconds: under gravity, bouncing off what it hits. Returns true when the fuse is up at `now_ms`
    /// (for a knife: its time is up and it goes away).
    pub fn step(&mut self, clip: &MapClip<'_>, dt: f32, now_ms: u32) -> bool {
        self.bounced = false;
        self.from = self.origin;
        if !self.resting && dt > 0.0 {
            let mut left = dt;
            for _ in 0..4 {
                if left <= 0.0 {
                    break;
                }
                let v = self.velocity;
                let end = [
                    self.origin[0] + v[0] * left,
                    self.origin[1] + v[1] * left,
                    self.origin[2] + v[2] * left - 0.5 * GRAVITY * left * left,
                ];
                let t = clip.trace(self.origin, end, [-HALF; 3], [HALF; 3], MASK_MISSILE);
                if t.fraction >= 1.0 || t.startsolid != 0 {
                    self.origin = end;
                    self.velocity[2] -= GRAVITY * left;
                    break;
                }
                let used = left * t.fraction;
                self.velocity[2] -= GRAVITY * used;
                if self.is_knife() {
                    if self.knife_impact(t.endpos, t.normal, t.surface_flags) {
                        break;
                    }
                    left -= used.max(1e-4);
                    continue;
                }
                // Sticky grenades stay where they hit (IW4L `settle_equipment`): semtex and C4 anywhere, claymores on
                // the ground; one without a timed detonation then waits for its trigger instead of its fuse.
                if sticks_to(self.facts.stickiness, t.normal) {
                    self.origin = core::array::from_fn(|k| t.endpos[k] + t.normal[k] * 0.25);
                    self.velocity = [0.0; 3];
                    self.resting = true;
                    self.stuck_ms = Some(now_ms);
                    if !self.facts.timed_detonation {
                        self.fused = false;
                    }
                    break;
                }
                // Back off the surface a little, then bounce by its surface type.
                self.origin = core::array::from_fn(|k| t.endpos[k] + t.normal[k] * 0.25);
                let surf = ((t.surface_flags >> 20) & 0x1f) as usize;
                let (par, perp) = (
                    self.facts.parallel_bounce.get(surf).copied().unwrap_or(0.0),
                    self.facts
                        .perpendicular_bounce
                        .get(surf)
                        .copied()
                        .unwrap_or(0.0),
                );
                let before = self.velocity;
                self.velocity = bounce(self.velocity, t.normal, par, perp);
                let change: [f32; 3] = core::array::from_fn(|k| self.velocity[k] - before[k]);
                if len(change) > BOUNCE_SOUND_SPEED_CHANGE {
                    self.bounced = true;
                }
                if t.normal[2] > 0.2 && len(self.velocity) < REST_SPEED {
                    self.velocity = [0.0; 3];
                    self.resting = true;
                    break;
                }
                left -= used.max(1e-4);
            }
        }
        self.fused && (now_ms.wrapping_sub(self.detonate_at_ms) as i32) >= 0
    }

    /// A knife hitting the world at `end` with surface `normal` (IW4L `knife_impact`): it bounces by the surface's
    /// factors unless it comes off nearly straight back from the surface (sticks in) or crawls along a floor (lies
    /// there). True when it stopped.
    fn knife_impact(&mut self, end: [f32; 3], normal: [f32; 3], surface_flags: u32) -> bool {
        let surf = ((surface_flags >> 20) & 0x1f) as usize;
        let (par, perp) = (
            self.facts.parallel_bounce.get(surf).copied().unwrap_or(0.0),
            self.facts
                .perpendicular_bounce
                .get(surf)
                .copied()
                .unwrap_or(0.0),
        );
        let before = self.velocity;
        self.velocity = bounce(self.velocity, normal, par, perp);
        let speed = len(self.velocity);
        let incidence = if speed > 1e-6 {
            (0..3).map(|k| self.velocity[k] / speed * normal[k]).sum()
        } else {
            0.0
        };
        let floor = normal[2] > 0.7;
        let change: [f32; 3] = core::array::from_fn(|k| self.velocity[k] - before[k]);
        self.bounced = len(change) > BOUNCE_SOUND_SPEED_CHANGE;
        if (floor && speed < 20.0) || incidence > 0.7 {
            self.origin = end;
            self.velocity = [0.0; 3];
            self.resting = true;
            return true;
        }
        // Off the surface a hair (IW4L: never upwards; here a floor lifts it 0.25 as other grenades are, so the
        // next box trace doesn't start inside it), and on.
        self.origin = core::array::from_fn(|k| {
            end[k]
                + if k == 2 {
                    (normal[k] * 0.1).min(0.0) + if floor { 0.25 } else { 0.0 }
                } else {
                    normal[k] * 0.1
                }
        });
        false
    }
}

/// Where the segment `a`→`b` first enters the box `lo`..`hi` (fraction 0..=1), if it does.
pub fn segment_box(a: [f32; 3], b: [f32; 3], lo: [f32; 3], hi: [f32; 3]) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        let d = b[k] - a[k];
        if libm::fabsf(d) < 1e-9 {
            if a[k] < lo[k] || a[k] > hi[k] {
                return None;
            }
            continue;
        }
        let (mut e, mut x) = ((lo[k] - a[k]) / d, (hi[k] - a[k]) / d);
        if e > x {
            core::mem::swap(&mut e, &mut x);
        }
        t0 = t0.max(e);
        t1 = t1.min(x);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// One player's damage from a blast at `origin` (IW4L `radius_player_attempts`): `feet` is the player's origin,
/// `height` its hull's top and `view_height` its eye, `yaw` its view yaw, `sees(from, to)` a world trace that passes.
#[allow(clippy::too_many_arguments)]
pub fn blast_damage(
    f: &EquipRecord,
    origin: [f32; 3],
    feet: [f32; 3],
    height: f32,
    view_height: f32,
    yaw: f32,
    cone: Option<([f32; 3], f32)>,
    mut sees: impl FnMut([f32; 3], [f32; 3]) -> bool,
) -> i32 {
    let radius = f.explosion_radius as f32;
    if radius <= 0.0 {
        return 0;
    }
    let mid = [feet[0], feet[1], feet[2] + height * 0.5];
    let half = [15.0, 15.0, height * 0.5];
    let dist = gamemode_iw4::radius_damage_distance_to_aabb(origin, mid, half);
    if dist >= radius {
        return 0;
    }
    if let Some((fwd, cos)) = cone {
        let d: [f32; 3] = core::array::from_fn(|k| mid[k] - origin[k]);
        let l = len(d);
        if l > f32::EPSILON && d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2] < cos * l {
            return 0;
        }
    }
    let r = yaw.to_radians();
    let right = [libm::sinf(r), -libm::cosf(r), 0.0];
    let vis =
        gamemode_iw4::can_damage_player_vis_scale(feet, view_height, right, origin, |a, b| {
            sees(a, b)
        });
    gamemode_iw4::radius_damage_amount(
        f.explosion_inner_damage as f32,
        f.explosion_outer_damage as f32,
        radius,
        dist,
        vis,
    )
}

/// The grenades in flight.
#[derive(Clone, Debug, Default)]
pub struct Grenades {
    pub live: Vec<Grenade>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips::boxes;

    fn frag() -> EquipRecord {
        EquipRecord {
            offhand_class: 1,
            fuse_time_ms: 3500,
            timed_detonation: true,
            explosion_radius: 256,
            explosion_inner_damage: 130,
            explosion_outer_damage: 55,
            projectile_speed: 900,
            projectile_speed_up: 150,
            parallel_bounce: [0.4; 31],
            perpendicular_bounce: [0.25; 31],
            ..EquipRecord::default()
        }
    }

    #[test]
    fn a_frag_flies_bounces_and_rests_until_its_fuse() {
        let clip = boxes(&[([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        // Thrown level from 60 units up: it lands ahead, bounces lower and lower, and lies still.
        let mut g = Grenade::throw(
            0,
            7,
            frag(),
            [0.0, 0.0, 60.0],
            [1.0, 0.0, 0.0],
            [0.0; 3],
            None,
            0,
        );
        let mut t = 0;
        let mut max_x: f32 = 0.0;
        while !g.step(&mc, 0.016, t) {
            t += 16;
            max_x = max_x.max(g.origin[0]);
            assert!(
                g.origin[2] >= -0.5,
                "never through the floor: {:?}",
                g.origin
            );
        }
        assert!((3490..=3520).contains(&t), "fuse 3.5 s: {t}");
        assert!(g.resting, "came to rest");
        assert!(max_x > 300.0, "thrown well ahead: {max_x}");
        // Cooked for 2 s: 1.5 s left.
        let g = Grenade::throw(
            0,
            7,
            frag(),
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0; 3],
            Some(1500),
            1000,
        );
        assert_eq!(g.detonate_at_ms, 2500);
    }

    /// MW2's frag from a locally converted `out/packs/common.wpk` (never in git; skipped when absent): a frag-class
    /// offhand on a fuse with a blast and bounce factors, thrown across a floor.
    #[test]
    fn real_frag_numbers() {
        extern crate std;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/common.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local common pack, skipped");
            return;
        };
        let pack = wpk::Pack::parse(&bytes, true).unwrap();
        let Some(eq) = pack.section(wpk::tag::WEAPON_EQUIP) else {
            std::println!("old pack, skipped");
            return;
        };
        let weap = pack.section(wpk::tag::WEAPONS).unwrap().data;
        // MW2's claymore and C4: charges (a claymore with its blast cone).
        for (n, kind) in [("claymore_mp", Charge::Claymore), ("c4_mp", Charge::C4)] {
            if let Some((k, _)) = wpk::weapon::find_indexed(weap, n) {
                let e = wpk::records::get::<EquipRecord>(eq.data, k as usize).unwrap();
                std::println!(
                    "{n}: cone {} radius {}",
                    e.damage_cone_angle,
                    e.explosion_radius
                );
                assert_eq!(charge(&e), Some(kind));
            }
        }
        // MW2's throwing knife: a knife-class offhand that kills on a direct hit.
        if let Some((k, _)) = wpk::weapon::find_indexed(weap, "throwingknife_mp") {
            let e = wpk::records::get::<EquipRecord>(eq.data, k as usize).unwrap();
            std::println!(
                "knife: impact {} speed {}",
                e.impact_damage,
                e.projectile_speed
            );
            assert_eq!(effect(&e), Effect::Knife);
            assert!(e.impact_damage >= 100, "a knife hit kills");
        }
        let (i, _) = wpk::weapon::find_indexed(weap, "frag_grenade_mp").expect("frag in the pack");
        let f = wpk::records::get::<EquipRecord>(eq.data, i as usize).unwrap();
        std::println!(
            "frag: class {} fuse {} ms radius {} damage {}..{} speed {} up {} bounce {:?} {:?}",
            f.offhand_class,
            f.fuse_time_ms,
            f.explosion_radius,
            f.explosion_outer_damage,
            f.explosion_inner_damage,
            f.projectile_speed,
            f.projectile_speed_up,
            &f.parallel_bounce[..4],
            &f.perpendicular_bounce[..4]
        );
        assert_eq!(f.offhand_class, 1);
        std::println!("frag sounds {:?}", f.sounds);
        assert_ne!(
            f.sounds[wpk::weapon::equip_sound::EXPLODE],
            wpk::sound::NONE,
            "the explosion has a sound"
        );
        assert!(
            f.fuse_time_ms > 1000 && f.explosion_radius > 100 && f.explosion_inner_damage >= 100
        );
        assert!(f.projectile_speed > 0);
        assert!(f.parallel_bounce.iter().any(|&b| b > 0.0));
        let clip = boxes(&[([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let mut g = Grenade::throw(
            0,
            i,
            f,
            [0.0, 0.0, 60.0],
            [0.94, 0.0, 0.34],
            [0.0; 3],
            None,
            0,
        );
        let mut t = 0;
        while !g.step(&mc, 0.016, t) {
            t += 16;
        }
        std::println!("landed {:?} after {t} ms", g.origin);
        assert!(g.origin[0] > 300.0 && g.origin[2] > -1.0);
    }

    /// MW2's launchers, alternates and akimbo pairs from a locally converted `out/packs/common.wpk` (never in git;
    /// skipped when absent or older than `WMSL`): projectile weapons with a speed, a blast and impact explosion, the
    /// Javelin's guidance; the M4's grenade launcher as its alternate; an akimbo pistol pair.
    #[test]
    fn real_launcher_numbers() {
        extern crate std;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/common.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local common pack, skipped");
            return;
        };
        let pack = wpk::Pack::parse(&bytes, true).unwrap();
        let (Some(eq), Some(ms)) = (
            pack.section(wpk::tag::WEAPON_EQUIP),
            pack.section(wpk::tag::WEAPON_MISSILE),
        ) else {
            std::println!("old pack, skipped");
            return;
        };
        let weap = pack.section(wpk::tag::WEAPONS).unwrap().data;
        for n in [
            "rpg_mp",
            "at4_mp",
            "javelin_mp",
            "stinger_mp",
            "m79_mp",
            "gl_m4_mp",
        ] {
            let Some((i, w)) = wpk::weapon::find_indexed(weap, n) else {
                continue;
            };
            let e = wpk::records::get::<EquipRecord>(eq.data, i as usize).unwrap();
            let m = wpk::records::get::<wpk::weapon::MissileRecord>(ms.data, i as usize).unwrap();
            std::println!(
                "{n}: type {} class {} speed {} up {} activate {} radius {} damage {}..{} impact {} | {m:?} explode sound {}",
                w.combat.weap_type,
                w.combat.weap_class,
                e.projectile_speed,
                e.projectile_speed_up,
                e.projectile_activate_dist,
                e.explosion_radius,
                e.explosion_outer_damage,
                e.explosion_inner_damage,
                e.impact_damage,
                e.sounds[wpk::weapon::equip_sound::EXPLODE]
            );
            assert_eq!(w.combat.weap_type, weapon_iw4::WEAPTYPE_PROJECTILE, "{n}");
            assert!(e.projectile_speed > 0 && e.explosion_radius > 0, "{n}");
            assert!(m.impact_explode, "{n} explodes on impact");
            if n == "javelin_mp" {
                assert_eq!(m.guidance, GUIDANCE_JAVELIN);
                assert!(w.combat.ads_fire_only);
            }
        }
        // The Grenadier's burst FAMAS with its launcher, Riot Control's shield and akimbo pair: all build as guns (a
        // burst weapon needs IW4L's burst cooldown).
        for n in [
            "famas_gl_mp",
            "gl_famas_mp",
            "riotshield_mp",
            "pp2000_akimbo_mp",
        ] {
            if let Some((i, r)) = wpk::weapon::find_indexed(weap, n) {
                let g = crate::gun::Gun::with_index(&r, i);
                assert!(g.is_some(), "{n} builds");
                assert_eq!(g.unwrap().akimbo(), n.contains("_akimbo"), "{n}");
            }
        }
        if let Some((_, m4)) = wpk::weapon::find_indexed(weap, "m4_gl_mp") {
            let alt = m4.combat.alternate_weapon;
            let r = wpk::records::get::<wpk::weapon::WeaponRecord>(weap, alt as usize).unwrap();
            std::println!("m4_gl_mp's alternate: {}", r.name());
            assert_eq!(r.name(), "gl_m4_mp");
        }
        if let Some((_, b)) = wpk::weapon::find_indexed(weap, "beretta_akimbo_mp") {
            assert!(b.combat.dual_wield);
        }
        if let Some((_, b)) = wpk::weapon::find_indexed(weap, "beretta_mp") {
            assert!(!b.combat.dual_wield);
        }
        // The catalog's launchers, shield and the default classes (for the debug menu's primary weapon in the
        // Dolphin scripts).
        if let Some(c) = pack
            .section(wpk::tag::CLASSES)
            .and_then(|s| wpk::classes::Catalog::decode(s.data).ok())
        {
            for (k, w) in c.weapons.iter().enumerate() {
                if [
                    "rpg",
                    "at4",
                    "javelin",
                    "m79",
                    "riotshield",
                    "stinger",
                    "m4",
                ]
                .contains(&w.name.as_str())
                {
                    std::println!("catalog weapon {k}: {}", w.name);
                }
            }
            for l in &c.defaults {
                std::println!(
                    "default class {}: {:?} / {:?}",
                    l.name,
                    c.weapon_names(l.primary, l.primary_attachments),
                    c.weapon_names(l.secondary, l.secondary_attachments)
                );
            }
        }
    }

    #[test]
    fn a_knife_sticks_in_a_wall_it_hits_square_and_skips_off_a_glancing_one() {
        let knife = EquipRecord {
            offhand_class: OFFHAND_CLASS_THROWINGKNIFE,
            fuse_time_ms: 30_000,
            projectile_speed: 1000,
            impact_damage: 135,
            parallel_bounce: [0.5; 31],
            perpendicular_bounce: [0.3; 31],
            ..EquipRecord::default()
        };
        // A wall across x = 200.
        let clip = boxes(&[
            ([200.0, -4000.0, -64.0], [264.0, 4000.0, 400.0]),
            ([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0]),
        ]);
        let mc = MapClip::new(&clip);
        let mut g = Grenade::throw(
            0,
            3,
            knife,
            [0.0, 0.0, 100.0],
            [1.0, 0.0, 0.0],
            [0.0; 3],
            None,
            0,
        );
        assert_eq!(effect(&g.facts), Effect::Knife);
        let mut t = 0;
        while !g.resting && t < 2000 {
            assert!(!g.step(&mc, 0.016, t), "no fuse");
            t += 16;
        }
        assert!(
            g.resting && g.origin[0] > 190.0 && g.origin[2] > 50.0,
            "stuck in the wall: {:?}",
            g.origin
        );
        // Thrown along the wall at a shallow angle it skips off and keeps going.
        let mut g = Grenade::throw(
            0,
            3,
            knife,
            [150.0, 0.0, 100.0],
            [0.6, 0.8, 0.0],
            [0.0; 3],
            None,
            0,
        );
        let mut skipped = false;
        for k in 0..20 {
            g.step(&mc, 0.016, k * 16);
            skipped |= g.velocity[0] < 0.0 && !g.resting;
        }
        assert!(skipped, "bounced off: {:?} {:?}", g.origin, g.velocity);
        // The flight segment crossing a player's box, and missing it.
        let lo = [100.0, -15.0, 0.0];
        let hi = [130.0, 15.0, 70.0];
        let t = segment_box([0.0, 0.0, 40.0], [200.0, 0.0, 40.0], lo, hi).unwrap();
        assert!((t - 0.5).abs() < 1e-4);
        assert!(segment_box([0.0, 0.0, 90.0], [200.0, 0.0, 90.0], lo, hi).is_none());
    }

    fn rocket() -> EquipRecord {
        EquipRecord {
            projectile_speed: 1000,
            explosion_radius: 256,
            explosion_inner_damage: 150,
            explosion_outer_damage: 30,
            impact_damage: 100,
            ..EquipRecord::default()
        }
    }

    fn impact() -> wpk::weapon::MissileRecord {
        wpk::weapon::MissileRecord {
            impact_explode: true,
            ..wpk::weapon::MissileRecord::default()
        }
    }

    /// IW4L `fire_missile`: the weapon's speed along the direction plus the owner's velocity, each axis truncated
    /// (`truncated_tr_delta`), flying straight (`TR_LINEAR`); it explodes where it meets the wall, armed.
    #[test]
    fn a_rocket_flies_straight_and_explodes_on_the_wall() {
        let clip = boxes(&[([1000.0, -500.0, -500.0], [1064.0, 500.0, 500.0])]);
        let mc = MapClip::new(&clip);
        let g = Grenade::launch(
            0,
            5,
            11,
            rocket(),
            impact(),
            [0.0, 0.0, 60.0],
            [0.6, 0.8, 0.0],
            [0.8, -0.6, 0.0],
            [10.7, -3.9, 0.0],
            None,
            0,
        );
        assert_eq!(g.velocity, [610.0, 796.0, 0.0]);
        let mut g2 = Grenade::launch(
            0,
            5,
            11,
            rocket(),
            impact(),
            [0.0, 0.0, 60.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0; 3],
            None,
            0,
        );
        let mut t = 0;
        while !g2.fly(&mc, 0.05, t, [0.0; 3]) {
            t += 50;
            assert!(t < 3000, "never hit: {:?}", g2.origin);
            assert!((g2.origin[2] - 60.0).abs() < 1e-3, "no drop");
        }
        // 1000 units at 1000/s: the 20th 50 ms step.
        assert_eq!(t, 950);
        assert!((g2.origin[0] - 999.0).abs() < 0.5, "{:?}", g2.origin);
        assert!(matches!(
            g2.flight.unwrap().contact,
            Some(Contact::Explode { .. })
        ));
        // Without a hit it goes away after a minute, no blast.
        assert!(!g.expired(59_999) && g.expired(60_000));
    }

    /// Before its arming distance the round is a dud (IW4L `!armed` → `Dud`, no splash); a grenade launcher's round
    /// falls under gravity (`TR_GRAVITY`, IW4L `grenade_launch_velocity`).
    #[test]
    fn a_launcher_round_drops_and_is_a_dud_inside_its_arming_distance() {
        let clip = boxes(&[([300.0, -500.0, -500.0], [364.0, 500.0, 500.0])]);
        let mc = MapClip::new(&clip);
        let gl = EquipRecord {
            projectile_speed: 1500,
            projectile_activate_dist: 400,
            ..rocket()
        };
        let mut g = Grenade::launch(
            0,
            6,
            WEAPCLASS_GRENADE,
            gl,
            impact(),
            [0.0, 0.0, 60.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0; 3],
            None,
            0,
        );
        assert!(
            !g.fused,
            "an arming distance: no fuse (IW4L grenade_deadlines)"
        );
        let mut t = 0;
        while !g.fly(&mc, 0.016, t, [0.0; 3]) {
            t += 16;
            assert!(t < 2000);
        }
        // 300 units at 1500/s is 0.2 s: it fell ½·800·0.2² = 16.
        assert!(
            (g.origin[2] - 44.0).abs() < 1.5,
            "dropped to {:?}",
            g.origin
        );
        assert!(matches!(
            g.flight.unwrap().contact,
            Some(Contact::Dud { .. })
        ));
        // The Javelin leaves 0.3 up (normalised) and 2.5 to the right, and falls.
        let j = Grenade::launch(
            0,
            7,
            11,
            rocket(),
            wpk::weapon::MissileRecord {
                guidance: GUIDANCE_JAVELIN,
                ..impact()
            },
            [0.0, 0.0, 60.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0; 3],
            None,
            0,
        );
        assert_eq!(j.origin, [0.0, -2.5, 60.0]);
        let l = libm::sqrtf(1.0 + 0.09);
        assert_eq!(
            j.velocity,
            [(1000.0 / l) as i32 as f32, 0.0, (300.0 / l) as i32 as f32]
        );
        assert!(j.flight.unwrap().gravity);
    }

    /// IW4L `turn_toward` (at most `max` radians about the common axis) and `steer`'s 240°/s for guided missiles;
    /// the Javelin climbs over a top-attack target before diving on it.
    #[test]
    fn guided_missiles_turn_at_their_rate_and_the_javelin_dives() {
        let v = turn_toward([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 10f32.to_radians());
        let a = libm::atan2f(v[1], v[0]).to_degrees();
        assert!(
            (a - 10.0).abs() < 1e-3 && (len(v) - 1.0).abs() < 1e-5,
            "{v:?}"
        );
        assert_eq!(
            turn_toward([1.0, 0.0, 0.0], [0.8, 0.6, 0.0], 1.0),
            [0.8, 0.6, 0.0]
        );
        let clip = boxes(&[([-8000.0, -8000.0, -64.0], [8000.0, 8000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let mut g = Grenade::launch(
            0,
            5,
            11,
            rocket(),
            wpk::weapon::MissileRecord {
                guidance: 1,
                ..impact()
            },
            [0.0, 0.0, 500.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0; 3],
            Some([0.0, 3000.0, 500.0]),
            0,
        );
        g.fly(&mc, 0.05, 50, [0.0; 3]);
        let yaw = libm::atan2f(g.velocity[1], g.velocity[0]).to_degrees();
        assert!((yaw - 12.0).abs() < 0.01, "240°/s for 50 ms: {yaw}");
        // A Javelin locked on a point 3000 ahead: up, over, then down onto it.
        let mut j = Grenade::launch(
            0,
            7,
            11,
            rocket(),
            wpk::weapon::MissileRecord {
                guidance: GUIDANCE_JAVELIN,
                ignition_delay_ms: 200,
                ..impact()
            },
            [0.0, 0.0, 60.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0; 3],
            Some([3000.0, 0.0, 0.0]),
            0,
        );
        let mut top: f32 = 0.0;
        let mut t = 0;
        while !j.fly(&mc, 0.05, t, [0.0; 3]) {
            t += 50;
            top = top.max(j.origin[2]);
            assert!(t < 30_000, "never came down: {:?}", j.origin);
        }
        assert!(top > 3000.0, "climbed for the top attack: {top}");
        assert!(
            (j.origin[0] - 3000.0).abs() < 400.0 && j.origin[1].abs() < 400.0,
            "came down on it: {:?}",
            j.origin
        );
    }

    #[test]
    fn blast_damage_falls_off_and_walls_stop_it() {
        let f = frag();
        let at = |x: f32| {
            blast_damage(
                &f,
                [0.0, 0.0, 10.0],
                [x, 0.0, 0.0],
                70.0,
                60.0,
                0.0,
                None,
                |_, _| true,
            )
        };
        assert!(at(0.0) >= 125, "point blank: {}", at(0.0));
        assert!(at(100.0) < at(0.0) && at(100.0) > at(200.0));
        assert_eq!(at(400.0), 0, "outside the radius");
        let behind_wall = blast_damage(
            &f,
            [0.0, 0.0, 10.0],
            [50.0, 0.0, 0.0],
            70.0,
            60.0,
            0.0,
            None,
            |_, _| false,
        );
        assert_eq!(behind_wall, 0);
        // A claymore's cone (facing +x, 70°): nothing behind it, full in front.
        let cone = Some(([1.0, 0.0, 0.0], libm::cosf(70f32.to_radians())));
        let front = blast_damage(
            &f,
            [0.0, 0.0, 10.0],
            [80.0, 0.0, 0.0],
            70.0,
            60.0,
            0.0,
            cone,
            |_, _| true,
        );
        let behind = blast_damage(
            &f,
            [0.0, 0.0, 10.0],
            [-80.0, 0.0, 0.0],
            70.0,
            60.0,
            0.0,
            cone,
            |_, _| true,
        );
        assert!(front > 0 && behind == 0, "{front} {behind}");
        // A bounce off a floor keeps the parallel share of the speed.
        let v = bounce([100.0, 0.0, -100.0], [0.0, 0.0, 1.0], 0.5, 0.5);
        assert!((v[0] - 50.0).abs() < 1e-3 && (v[2] - 50.0).abs() < 1e-3);
    }
}

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
        }
    }

    pub fn is_knife(&self) -> bool {
        self.facts.offhand_class == OFFHAND_CLASS_THROWINGKNIFE
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
                // Sticky grenades (semtex) stay where they hit.
                if self.facts.stickiness != 0 {
                    self.origin = core::array::from_fn(|k| t.endpos[k] + t.normal[k] * 0.25);
                    self.velocity = [0.0; 3];
                    self.resting = true;
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
        (now_ms.wrapping_sub(self.detonate_at_ms) as i32) >= 0
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
/// `crouched` its stance, `yaw` its view yaw, `sees(from, to)` a world trace that passes.
#[allow(clippy::too_many_arguments)]
pub fn blast_damage(
    f: &EquipRecord,
    origin: [f32; 3],
    feet: [f32; 3],
    crouched: bool,
    yaw: f32,
    mut sees: impl FnMut([f32; 3], [f32; 3]) -> bool,
) -> i32 {
    let radius = f.explosion_radius as f32;
    if radius <= 0.0 {
        return 0;
    }
    let height = if crouched { 50.0 } else { 70.0 };
    let mid = [feet[0], feet[1], feet[2] + height * 0.5];
    let half = [15.0, 15.0, height * 0.5];
    let dist = gamemode_iw4::radius_damage_distance_to_aabb(origin, mid, half);
    if dist >= radius {
        return 0;
    }
    let view_height = if crouched { 40.0 } else { 60.0 };
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

    #[test]
    fn blast_damage_falls_off_and_walls_stop_it() {
        let f = frag();
        let at =
            |x: f32| blast_damage(&f, [0.0, 0.0, 10.0], [x, 0.0, 0.0], false, 0.0, |_, _| true);
        assert!(at(0.0) >= 125, "point blank: {}", at(0.0));
        assert!(at(100.0) < at(0.0) && at(100.0) > at(200.0));
        assert_eq!(at(400.0), 0, "outside the radius");
        let behind_wall = blast_damage(
            &f,
            [0.0, 0.0, 10.0],
            [50.0, 0.0, 0.0],
            false,
            0.0,
            |_, _| false,
        );
        assert_eq!(behind_wall, 0);
        // A bounce off a floor keeps the parallel share of the speed.
        let v = bounce([100.0, 0.0, -100.0], [0.0, 0.0, 1.0], 0.5, 0.5);
        assert!((v[0] - 50.0).abs() < 1e-3 && (v[2] - 50.0).abs() < 1e-3);
    }
}

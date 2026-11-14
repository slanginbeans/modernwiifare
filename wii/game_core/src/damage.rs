//! Bullet damage: IW4L's range falloff (`sim::bullet::bullet_damage_at_distance`, Apache-2.0, rev `acc57a5`) and
//! the weapon's per-hit-location multipliers (MW2's `mp_lochit_dmgtable` baked in by the converter), against players'
//! hit volumes.
//!
//! Hit volumes: MW2 traces bullets against each character model's hit boxes. Until the character models are
//! converted, a player is a stack of boxes around the standing or crouched hull (head, neck, upper and lower torso,
//! upper and lower legs, arms on each side), sized from IW4's player hull and view heights. Damage numbers are the
//! weapon's own; only where a shot lands on the body is approximated (docs/deviations.md).

use weapon_iw4::{HITLOC_COUNT, WeaponCombatFacts, location_damage_scale};

/// IW4 hit locations (`weapon_iw4::HITLOC_NAMES` order).
pub mod hitloc {
    pub const HEAD: u8 = 2;
    pub const NECK: u8 = 3;
    pub const TORSO_UPPER: u8 = 4;
    pub const TORSO_LOWER: u8 = 5;
    pub const RIGHT_ARM_UPPER: u8 = 6;
    pub const LEFT_ARM_UPPER: u8 = 7;
    pub const RIGHT_LEG_UPPER: u8 = 12;
    pub const LEFT_LEG_UPPER: u8 = 13;
    pub const RIGHT_LEG_LOWER: u8 = 14;
    pub const LEFT_LEG_LOWER: u8 = 15;
}

/// Damage of one bullet at `dist` units (IW4L `bullet_damage_at_distance`).
pub fn at_distance(facts: &WeaponCombatFacts, dist: f32) -> i32 {
    let base = facts.damage;
    let min = if facts.min_damage > 0 {
        facts.min_damage
    } else {
        base
    };
    let mut damage = base;
    let range = facts.min_damage_range - facts.max_damage_range;
    if base != min && range != 0.0 && facts.max_damage_range <= dist {
        if facts.min_damage_range <= dist {
            damage = min;
        } else {
            let lerp = ((dist - facts.max_damage_range) / range).clamp(0.0, 1.0);
            damage = ((base as f32) * (1.0 - lerp) + (min as f32) * lerp) as i32;
        }
    }
    damage.max(0)
}

/// A bullet's damage at `dist` on `loc` (range falloff × location multiplier, IW4 truncation, at least 1).
pub fn bullet(facts: &WeaponCombatFacts, table: &[f32; HITLOC_COUNT], dist: f32, loc: u8) -> i32 {
    let d = at_distance(facts, dist);
    if d <= 0 {
        return 0;
    }
    ((d as f32 * location_damage_scale(table, loc)) as i32).max(1)
}

/// One hit box: offsets from the player's origin (feet), axis-aligned in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitBox {
    pub loc: u8,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

const fn b(loc: u8, mins: [f32; 3], maxs: [f32; 3]) -> HitBox {
    HitBox { loc, mins, maxs }
}

/// Standing: 70 units tall (IW4 hull), eyes at 60. The arms sit at the torso's sides; `right` is +x before the yaw
/// turn is applied (boxes stay axis-aligned, so the side split is only a coarse one).
const STANDING: [HitBox; 10] = [
    b(hitloc::HEAD, [-5.0, -5.0, 58.0], [5.0, 5.0, 70.0]),
    b(hitloc::NECK, [-4.0, -4.0, 53.0], [4.0, 4.0, 58.0]),
    b(hitloc::TORSO_UPPER, [-9.0, -9.0, 40.0], [9.0, 9.0, 53.0]),
    b(hitloc::TORSO_LOWER, [-8.0, -8.0, 30.0], [8.0, 8.0, 40.0]),
    b(
        hitloc::RIGHT_ARM_UPPER,
        [9.0, -4.0, 34.0],
        [13.0, 4.0, 52.0],
    ),
    b(
        hitloc::LEFT_ARM_UPPER,
        [-13.0, -4.0, 34.0],
        [-9.0, 4.0, 52.0],
    ),
    b(hitloc::RIGHT_LEG_UPPER, [0.0, -5.0, 15.0], [7.0, 5.0, 30.0]),
    b(hitloc::LEFT_LEG_UPPER, [-7.0, -5.0, 15.0], [0.0, 5.0, 30.0]),
    b(hitloc::RIGHT_LEG_LOWER, [0.0, -5.0, 0.0], [7.0, 5.0, 15.0]),
    b(hitloc::LEFT_LEG_LOWER, [-7.0, -5.0, 0.0], [0.0, 5.0, 15.0]),
];

/// The hit boxes of a player at `origin`, crouched or not (crouched: 50 tall, IW4's crouch hull; heights scaled).
pub fn boxes(origin: [f32; 3], crouched: bool) -> [HitBox; 10] {
    let scale = if crouched { 50.0 / 70.0 } else { 1.0 };
    STANDING.map(|h| HitBox {
        loc: h.loc,
        mins: [
            origin[0] + h.mins[0],
            origin[1] + h.mins[1],
            origin[2] + h.mins[2] * scale,
        ],
        maxs: [
            origin[0] + h.maxs[0],
            origin[1] + h.maxs[1],
            origin[2] + h.maxs[2] * scale,
        ],
    })
}

/// Where along `start → end` (0..1) the segment first enters the box, if it does (slab test).
pub fn segment_box(start: [f32; 3], end: [f32; 3], mins: [f32; 3], maxs: [f32; 3]) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        let d = end[k] - start[k];
        if d.abs() < 1e-6 {
            if start[k] < mins[k] || start[k] > maxs[k] {
                return None;
            }
            continue;
        }
        let (a, b) = ((mins[k] - start[k]) / d, (maxs[k] - start[k]) / d);
        let (near, far) = if a < b { (a, b) } else { (b, a) };
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// The first hit box a segment enters: (fraction, location).
pub fn first_hit(start: [f32; 3], end: [f32; 3], boxes: &[HitBox]) -> Option<(f32, u8)> {
    boxes
        .iter()
        .filter_map(|h| segment_box(start, end, h.mins, h.maxs).map(|t| (t, h.loc)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use weapon_iw4::{CapturedCombatInput, LOCATION_DAMAGE_IDENTITY};

    fn rifle() -> WeaponCombatFacts {
        let c = CapturedCombatInput {
            fire_time_ms: 100,
            clip_size: 30,
            start_ammo: 90,
            max_ammo: 300,
            reload_time_ms: 2000,
            reload_empty_time_ms: 2500,
            raise_time_ms: 500,
            first_raise_time_ms: 500,
            drop_time_ms: 300,
            damage: 40,
            min_damage: 20,
            max_damage_range: 1000.0,
            min_damage_range: 2000.0,
            ..CapturedCombatInput::default()
        };
        WeaponCombatFacts::try_from_captured(c).expect("usable")
    }

    #[test]
    fn damage_falls_off_between_the_ranges() {
        let f = rifle();
        assert_eq!(at_distance(&f, 500.0), 40);
        assert_eq!(at_distance(&f, 1500.0), 30);
        assert_eq!(at_distance(&f, 5000.0), 20);
        let mut table = LOCATION_DAMAGE_IDENTITY;
        table[usize::from(hitloc::HEAD)] = 1.5;
        assert_eq!(bullet(&f, &table, 500.0, hitloc::HEAD), 60);
        assert_eq!(bullet(&f, &table, 500.0, hitloc::TORSO_LOWER), 40);
    }

    #[test]
    fn shots_find_the_first_box() {
        let bx = boxes([100.0, 0.0, 0.0], false);
        // Level with the head, from the side: the head.
        assert_eq!(
            first_hit([0.0, 0.0, 64.0], [200.0, 0.0, 64.0], &bx).map(|h| h.1),
            Some(hitloc::HEAD)
        );
        // Chest height: the left arm comes first from -x, then the torso.
        assert_eq!(
            first_hit([0.0, 0.0, 45.0], [200.0, 0.0, 45.0], &bx).map(|h| h.1),
            Some(hitloc::LEFT_ARM_UPPER)
        );
        // Over the head: nothing.
        assert!(first_hit([0.0, 0.0, 80.0], [200.0, 0.0, 80.0], &bx).is_none());
        // Crouched, the head is lower.
        let c = boxes([100.0, 0.0, 0.0], true);
        assert!(first_hit([0.0, 0.0, 64.0], [200.0, 0.0, 64.0], &c).is_none());
        assert_eq!(
            first_hit([0.0, 0.0, 46.0], [200.0, 0.0, 46.0], &c).map(|h| h.1),
            Some(hitloc::HEAD)
        );
    }
}

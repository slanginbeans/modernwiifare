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
    pub const NONE: u8 = 0;
    pub const HELMET: u8 = 1;
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

/// One hit box, oriented: its centre in world space, its three unit axes and the half size along each (IW4L
/// `xmodel_runtime::CollisionBone`). The stand-in column's boxes turn with the body's yaw; bone boxes take their
/// bone's posed axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitBox {
    pub loc: u8,
    pub center: [f32; 3],
    pub axes: [[f32; 3]; 3],
    pub half: [f32; 3],
}

impl HitBox {
    /// The world-space box around it (for drawing and broad tests).
    pub fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        let e: [f32; 3] = core::array::from_fn(|w| {
            (0..3)
                .map(|a| libm::fabsf(self.axes[a][w]) * self.half[a])
                .sum()
        });
        (
            core::array::from_fn(|k| self.center[k] - e[k]),
            core::array::from_fn(|k| self.center[k] + e[k]),
        )
    }
}

/// A box of the stand-in column: offsets from the feet, before the yaw turn (`right` is +x, forward is -y).
#[derive(Clone, Copy)]
struct ColumnBox {
    loc: u8,
    mins: [f32; 3],
    maxs: [f32; 3],
}

const fn b(loc: u8, mins: [f32; 3], maxs: [f32; 3]) -> ColumnBox {
    ColumnBox { loc, mins, maxs }
}

/// Standing: 70 units tall (IW4 hull), eyes at 60. The arms sit at the torso's sides.
const STANDING: [ColumnBox; 10] = [
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

/// The stand-in hit boxes of a player at `origin` facing `yaw` degrees (IW4: 0 = +x, counter-clockwise), the
/// column squashed to the stance's hull `height` (crouched 50, prone 30) and turned with the body.
pub fn boxes(origin: [f32; 3], height: f32, yaw: f32) -> [HitBox; 10] {
    let scale = height / 70.0;
    let (s, c) = (libm::sinf(yaw.to_radians()), libm::cosf(yaw.to_radians()));
    // Column x (right) is the body's right: forward (c, s) turned -90 degrees; column -y is forward.
    let right = [s, -c, 0.0];
    let back = [-c, -s, 0.0];
    STANDING.map(|h| {
        let mid: [f32; 3] = core::array::from_fn(|k| (h.mins[k] + h.maxs[k]) * 0.5);
        let half: [f32; 3] = core::array::from_fn(|k| (h.maxs[k] - h.mins[k]) * 0.5);
        HitBox {
            loc: h.loc,
            center: [
                origin[0] + right[0] * mid[0] + back[0] * mid[1],
                origin[1] + right[1] * mid[0] + back[1] * mid[1],
                origin[2] + mid[2] * scale,
            ],
            axes: [right, back, [0.0, 0.0, 1.0]],
            half: [half[0], half[1], half[2] * scale],
        }
    })
}

/// A character bone's hit box (`CHHB`'s bounds in the bone's space) under the posed bone transform `bone` (model
/// space) and the model's placement `model` (model to world), as IW4L `xmodel_runtime`'s
/// `collision_dobj_with_controller`.
pub fn bone_box(
    model: &crate::gfx::Mtx34,
    bone: &crate::gfx::Mtx34,
    rec: &wpk::records::BoneHitRecord,
) -> Option<HitBox> {
    let m = crate::viewmodel::mul(model, bone);
    let mid = rec.midpoint;
    let center: [f32; 3] =
        core::array::from_fn(|r| m[r][0] * mid[0] + m[r][1] * mid[1] + m[r][2] * mid[2] + m[r][3]);
    let mut axes = [[0.0; 3]; 3];
    let mut half = [0.0; 3];
    for a in 0..3 {
        let d = [m[0][a], m[1][a], m[2][a]];
        let len = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
        if !(len.is_finite() && len > 0.0) {
            return None;
        }
        axes[a] = d.map(|v| v / len);
        half[a] = rec.half_size[a] * len;
    }
    center.iter().all(|v| v.is_finite()).then_some(HitBox {
        loc: rec.loc,
        center,
        axes,
        half,
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

/// Where along `start → end` (0..1) the segment enters an oriented box (IW4L `sim` `ray_obb`: the slab test in the
/// box's own axes; a start inside it hits at 0).
pub fn segment_obb(start: [f32; 3], end: [f32; 3], h: &HitBox) -> Option<f32> {
    let rel = |p: [f32; 3]| {
        let d = [p[0] - h.center[0], p[1] - h.center[1], p[2] - h.center[2]];
        h.axes.map(|a| a[0] * d[0] + a[1] * d[1] + a[2] * d[2])
    };
    let (ls, le) = (rel(start), rel(end));
    segment_box(ls, le, h.half.map(|v| -v), h.half)
}

/// The first hit box a segment enters: (fraction, location).
pub fn first_hit(start: [f32; 3], end: [f32; 3], boxes: &[HitBox]) -> Option<(f32, u8)> {
    body_span(start, end, boxes).map(|s| (s.enter, s.loc))
}

/// How a segment crosses one body: where it enters its first box (and that box's location) and where it leaves
/// the last box it crosses (fractions of `start → end`; the body's thickness along the bullet is between them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub enter: f32,
    pub leave: f32,
    pub loc: u8,
}

/// The [`Span`] of `boxes` along `start → end`, if any box is crossed.
pub fn body_span(start: [f32; 3], end: [f32; 3], boxes: &[HitBox]) -> Option<Span> {
    let mut out: Option<Span> = None;
    for h in boxes {
        let Some(t) = segment_obb(start, end, h) else {
            continue;
        };
        // Where it leaves this box: the same slab test from the far end.
        let leave = segment_obb(end, start, h).map_or(1.0, |r| 1.0 - r);
        out = Some(match out {
            None => Span {
                enter: t,
                leave,
                loc: h.loc,
            },
            Some(s) => Span {
                enter: s.enter.min(t),
                leave: s.leave.max(leave),
                loc: if t < s.enter { h.loc } else { s.loc },
            },
        });
    }
    out
}

/// Whether the segment passes within `radius` of a player's column (its feet at `origin`, `top` high): the broad
/// test before the player's boxes.
pub fn near_body(start: [f32; 3], end: [f32; 3], origin: [f32; 3], radius: f32, top: f32) -> bool {
    let mins = [origin[0] - radius, origin[1] - radius, origin[2] - radius];
    let maxs = [
        origin[0] + radius,
        origin[1] + radius,
        origin[2] + top + radius,
    ];
    segment_box(start, end, mins, maxs).is_some()
}

/// What the hit volume tests cost (summed over a stretch of frames, for the match log).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitCost {
    /// Segments tested against a body that passed the broad test.
    pub bodies: u32,
    /// Boxes tested.
    pub boxes: u32,
    /// Of the bodies, those tested against their drawn bones.
    pub bone_bodies: u32,
    /// Microseconds (on the Wii; 0 on the host).
    pub us: u32,
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
        // Facing +y: its left side faces -x.
        let bx = boxes([100.0, 0.0, 0.0], 70.0, 90.0);
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
        let c = boxes([100.0, 0.0, 0.0], 50.0, 90.0);
        assert!(first_hit([0.0, 0.0, 64.0], [200.0, 0.0, 64.0], &c).is_none());
        assert_eq!(
            first_hit([0.0, 0.0, 46.0], [200.0, 0.0, 46.0], &c).map(|h| h.1),
            Some(hitloc::HEAD)
        );
    }

    #[test]
    fn boxes_turn_with_the_body() {
        // Facing the shooter (yaw 180 from +x... the shot comes from -x, the body faces -x): the chest first, arms
        // at the sides miss a shot down the middle.
        let facing = boxes([100.0, 0.0, 0.0], 70.0, 180.0);
        assert_eq!(
            first_hit([0.0, 0.0, 45.0], [200.0, 0.0, 45.0], &facing).map(|h| h.1),
            Some(hitloc::TORSO_UPPER)
        );
        // Side on (facing +y): the left arm first, 4 units further out than the chest.
        let side = boxes([100.0, 0.0, 0.0], 70.0, 90.0);
        let (t_side, _) = first_hit([0.0, 0.0, 45.0], [200.0, 0.0, 45.0], &side).unwrap();
        let (t_face, _) = first_hit([0.0, 0.0, 45.0], [200.0, 0.0, 45.0], &facing).unwrap();
        assert!((t_side * 200.0 - 87.0).abs() < 1e-3, "{}", t_side * 200.0);
        assert!((t_face * 200.0 - 91.0).abs() < 1e-3, "{}", t_face * 200.0);
        // Turned 45 degrees, the torso box's corner points at the shooter: it reaches 9·√2 out.
        let diag = boxes([100.0, 0.0, 0.0], 70.0, 135.0);
        let (t, loc) = first_hit([0.0, 0.0, 45.0], [200.0, 0.0, 45.0], &diag).unwrap();
        assert_eq!(loc, hitloc::TORSO_UPPER);
        assert!((t * 200.0 - (100.0 - 9.0 * core::f32::consts::SQRT_2)).abs() < 1e-3);
        // The bounding box of a turned box covers its corners.
        let (lo, hi) = diag[2].aabb();
        assert!((hi[0] - lo[0] - 18.0 * core::f32::consts::SQRT_2).abs() < 1e-3);
    }

    #[test]
    fn segments_starting_inside_hit_at_zero() {
        let bx = boxes([0.0; 3], 70.0, 0.0);
        assert_eq!(
            first_hit([0.0, 0.0, 45.0], [200.0, 0.0, 45.0], &bx).map(|h| h.0),
            Some(0.0)
        );
    }

    #[test]
    fn bone_boxes_follow_the_bone() {
        // A bone 50 up, turned 90 degrees about z; the model turned another 90 and placed at x = 100.
        let bone: crate::gfx::Mtx34 = [
            [0.0, -1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 50.0],
        ];
        let model: crate::gfx::Mtx34 = [
            [0.0, -1.0, 0.0, 100.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let rec = wpk::records::BoneHitRecord {
            team: 0,
            bone: 3,
            loc: hitloc::HEAD,
            midpoint: [4.0, 0.0, 0.0],
            half_size: [6.0, 2.0, 3.0],
        };
        let h = bone_box(&model, &bone, &rec).unwrap();
        // Turned 180 in all: the midpoint's +x lands at -x.
        assert!((h.center[0] - 96.0).abs() < 1e-5 && h.center[1].abs() < 1e-5);
        assert!((h.center[2] - 50.0).abs() < 1e-5);
        // Its long axis (6) lies along world x: a shot down y at x = 91 still hits, at x = 89 misses.
        assert!(first_hit([91.0, -50.0, 50.0], [91.0, 50.0, 50.0], &[h]).is_some());
        assert!(first_hit([89.0, -50.0, 50.0], [89.0, 50.0, 50.0], &[h]).is_none());
        assert_eq!(h.loc, hitloc::HEAD);
    }
}

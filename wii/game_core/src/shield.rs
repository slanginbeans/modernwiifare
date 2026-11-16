//! The riot shield as cover (IW4L `sim/src/shield.rs`, `bullet_collision::is_shield` and `combat::segment_is_shield`):
//! a bullet that meets the shield before the body stops there, deals no damage (IW4L hit location 19, `shield`)
//! and strikes surface 29 (`riotshield`), and an explosion behind it can't see the parts of the player it covers.
//! Carried in the hands it covers the front (`tag_weapon_left`); as the other weapon it hangs on the back
//! (`tag_shield_back`, IW4L `ShieldAttachment::tag`).
//!
//! IW4L takes the shield's shape from its model's collision bones posed on the player. Those aren't converted
//! for the Wii, so the shield here is a box in the player's frame (as wide as the body's 30-unit hit boxes, just
//! outside them; from the shins to above the head in front, the back from the waist up), turned with the view's yaw.

/// IW4L `shield::HITLOC`.
pub const HITLOC: u8 = 19;
/// IW4L `shield::SURFACE` (IW4 surface type `riotshield`).
pub const SURFACE: u8 = 29;

/// Where a player carries a riot shield.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Carry {
    #[default]
    None,
    /// In the hands, out in front (IW4L `on_back: false`).
    Front,
    /// The other weapon, on the back (IW4L `on_back: true`).
    Back,
}

/// Whether `weapon_name` is a riot shield (MW2's `riotshield_mp`; IW4 `WEAPTYPE_SHIELD`).
pub fn is_shield(weapon_name: &str, weap_type: i32) -> bool {
    weap_type == weapon_iw4::WEAPTYPE_SHIELD || weapon_name.starts_with("riotshield")
}

/// The shield's box in the player's frame (x along the view's yaw, y to the left, z up from the feet), for a hull
/// `height` units tall.
pub fn local_box(carry: Carry, height: f32) -> Option<([f32; 3], [f32; 3])> {
    match carry {
        Carry::None => None,
        // As wide as the body's hit boxes (30), so the box doesn't leave their edges open.
        Carry::Front => Some(([16.0, -16.0, height * 0.12], [21.0, 16.0, height + 4.0])),
        Carry::Back => Some(([-21.0, -16.0, height * 0.45], [-16.0, 16.0, height * 0.95])),
    }
}

/// `p` in the frame of a player standing at `feet` facing `yaw` degrees.
fn to_local(p: [f32; 3], feet: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = (libm::sinf(yaw.to_radians()), libm::cosf(yaw.to_radians()));
    let d = [p[0] - feet[0], p[1] - feet[1], p[2] - feet[2]];
    [d[0] * c + d[1] * s, -d[0] * s + d[1] * c, d[2]]
}

/// Where the segment `a`→`b` first meets the shield of a player at `feet` facing `yaw` (fraction 0..=1), if it does.
pub fn segment_hit(
    carry: Carry,
    feet: [f32; 3],
    yaw: f32,
    height: f32,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<f32> {
    let (lo, hi) = local_box(carry, height)?;
    crate::grenade::segment_box(to_local(a, feet, yaw), to_local(b, feet, yaw), lo, hi)
}

/// Whether the shield blocks the line from `from` to `to` (a blast's view of a point on the player): it crosses the
/// shield and doesn't start inside it.
pub fn blocks(
    carry: Carry,
    feet: [f32; 3],
    yaw: f32,
    height: f32,
    from: [f32; 3],
    to: [f32; 3],
) -> bool {
    segment_hit(carry, feet, yaw, height, from, to).is_some_and(|t| t > 0.0 && t < 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_shield_covers_the_front_only() {
        let feet = [0.0, 0.0, 0.0];
        // Facing +x: a shot from ahead at the chest meets the shield, one from behind doesn't.
        let ahead = segment_hit(
            Carry::Front,
            feet,
            0.0,
            70.0,
            [500.0, 0.0, 45.0],
            [-500.0, 0.0, 45.0],
        );
        assert!(ahead.is_some_and(|t| (t - (500.0 - 21.0) / 1000.0).abs() < 1e-3));
        let behind = segment_hit(
            Carry::Front,
            feet,
            0.0,
            70.0,
            [-500.0, 0.0, 45.0],
            [-10.0, 0.0, 45.0],
        );
        assert!(behind.is_none());
        // Turned to face +y, a shot from +y meets it.
        assert!(
            segment_hit(
                Carry::Front,
                feet,
                90.0,
                70.0,
                [0.0, 500.0, 45.0],
                [0.0, -500.0, 45.0]
            )
            .is_some()
        );
        // On the back: covers from behind, not from ahead (stopping at the body).
        assert!(
            segment_hit(
                Carry::Back,
                feet,
                0.0,
                70.0,
                [-500.0, 0.0, 45.0],
                [0.0, 0.0, 45.0]
            )
            .is_some()
        );
        assert!(
            segment_hit(
                Carry::Back,
                feet,
                0.0,
                70.0,
                [500.0, 0.0, 45.0],
                [0.0, 0.0, 45.0]
            )
            .is_none()
        );
        assert!(!blocks(
            Carry::None,
            feet,
            0.0,
            70.0,
            [500.0, 0.0, 45.0],
            [0.0, 0.0, 45.0]
        ));
    }
}

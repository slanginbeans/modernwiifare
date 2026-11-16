//! The first-person camera's feel on top of the eye: the view bob while moving, the dip on a landing, the kick of
//! being hit, and the scope's idle sway (steadied by holding breath). IW4L's `weapon_iw4::view_bob` does the maths
//! (`view_angle_bob`, `view_org_bob`, `crash_land_fall_height` / `crash_land_view_dip` / `land_origin_z`,
//! `damage_feedback_kick`); how they are put together follows IW4L's camera
//! (`render_anim/src/occupancy/view_kick.rs`, Apache-2.0, rev `acc57a5`: the first-person branch,
//! `stamp_and_land_origin_z` and `stamp_damage_feedback`), written here without Bevy.
//!
//! IW4L's code is the reference for these values; a trace against MW2 is still to do (`docs/gaps.md`).
//!
//! The angles ([`ViewPose::angles`]) go on the view shots follow too (what you see at the screen's centre is where
//! you shoot: the scope's sway and the hit kick move your aim, as in MW2); the eye offset ([`ViewPose::origin`]) only
//! moves the drawn camera (the server fires from the player's eye, without the bob).

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState};
use weapon_iw4::{
    VIEW_DAMAGE_UNDIRECTED, VIEW_ORG_BOB_Z_MIN_OFS, ViewAngleBobInputs, ViewOrgBobInputs,
    WeaponIdleInputs, crash_land_fall_height, crash_land_view_dip, damage_feedback_kick,
    land_origin_z, view_angle_bob, view_damage_angles, view_org_bob,
};

/// What the held weapon changes in the view (from its `WFPV` record and combat facts).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewWeapon {
    /// A scope overlay (sniper rifles): the camera sways while aimed.
    pub overlay_reticle: i32,
    pub aim_down_sight: bool,
    pub ads_bob_factor: f32,
    pub ads_view_bob_mult: f32,
    pub idle: WeaponIdleInputs,
}

/// The view's change this frame: angles (pitch, yaw, roll degrees) added to the view, and a world offset added to
/// the eye.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewPose {
    pub angles: [f32; 3],
    pub origin: [f32; 3],
    /// The landing dip this frame (units, negative down), part of `origin`.
    pub land: f32,
}

/// The view's state between frames (IW4L `SessionViewKick`'s first-person fields).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewKick {
    have_prev: bool,
    last_origin_z: f32,
    last_velocity_z: f32,
    last_ground: i32,
    land_change: f32,
    land_time: i32,
    /// The last landing's dip (IW4 `landViewDip`) and how many landings dipped, for the log.
    pub land_view_dip: i32,
    pub landings: u32,
    v_dmg_pitch: f32,
    v_dmg_roll: f32,
    damage_time: i32,
    weap_idle_time: i32,
    view_last_idle_factor: f32,
}

/// IW4 `vectoangles` (pitch down positive), in 0..360.
fn vectoangles(v: [f32; 3]) -> (f32, f32) {
    let deg = |r: f32| r.to_degrees();
    let wrap = |a: f32| a - libm::floorf(a / 360.0) * 360.0;
    if v[0] == 0.0 && v[1] == 0.0 {
        return (if v[2] > 0.0 { 270.0 } else { 90.0 }, 0.0);
    }
    let yaw = wrap(deg(libm::atan2f(v[1], v[0])));
    let forward = libm::sqrtf(v[0] * v[0] + v[1] * v[1]);
    let pitch = wrap(-deg(libm::atan2f(v[2], forward)));
    (pitch, yaw)
}

/// An angle as IW4's damage byte (`P_DamageFeedback`: angle / 360 of 255; 255 means "no direction").
fn angle_byte(a: f32) -> u32 {
    ((a / 360.0 * 255.0) as u32).min(254)
}

impl ViewKick {
    /// Start over (a spawn).
    pub fn reset(&mut self) {
        *self = ViewKick::default();
    }

    /// Being hit (IW4 `P_DamageFeedback` then IW4L `stamp_damage_feedback`): `from` is where the damage came from
    /// (the attacker), `None` for the world (a fall) or yourself; `damage` the points taken.
    pub fn hit(&mut self, ps: &PlayerState, from: Option<[f32; 3]>, damage: i32, now_ms: u32) {
        if damage <= 0 {
            return;
        }
        let (yaw_byte, pitch_byte) = match from {
            Some(f) => {
                let d = [
                    f[0] - ps.origin[0],
                    f[1] - ps.origin[1],
                    f[2] - ps.origin[2],
                ];
                let (pitch, yaw) = vectoangles(d);
                (angle_byte(yaw), angle_byte(pitch))
            }
            None => (VIEW_DAMAGE_UNDIRECTED, VIEW_DAMAGE_UNDIRECTED),
        };
        let punch = damage_feedback_kick(yaw_byte, pitch_byte, damage.min(255), ps.viewangles);
        self.v_dmg_pitch = punch.v_dmg_pitch;
        self.v_dmg_roll = punch.v_dmg_roll;
        self.damage_time = (now_ms as i32).max(1);
    }

    /// The view this frame from the player's state after its move. `weapon`: the held weapon's view numbers (`None`
    /// without one: only the hit kick, as IW4L). `frametime`: seconds since the last call.
    pub fn frame(
        &mut self,
        ps: &PlayerState,
        weapon: Option<&ViewWeapon>,
        now_ms: u32,
        frametime: f32,
    ) -> ViewPose {
        let now = now_ms as i32;
        let xyspeed =
            libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]);
        let org = ViewOrgBobInputs {
            bob_cycle: (ps.bob_cycle as u32 & 0xff) as u8,
            xyspeed,
            view_height_target: ps.view_height_target,
            pm_flags: ps.pm_flags,
            weapon_pos_frac: ps.f_weapon_pos_frac,
            perks0: ps.perks[0],
        };
        let base = ViewAngleBobInputs {
            org,
            time: now,
            damage_time: self.damage_time,
            v_dmg_pitch: self.v_dmg_pitch,
            v_dmg_roll: self.v_dmg_roll,
            ..ViewAngleBobInputs::default()
        };
        let bob = match weapon {
            Some(w) => view_angle_bob(ViewAngleBobInputs {
                e_flags: ps.e_flags,
                overlay_reticle: w.overlay_reticle,
                ads_bob_factor: w.ads_bob_factor,
                ads_view_bob_mult: w.ads_view_bob_mult,
                aim_down_sight: w.aim_down_sight,
                idle: w.idle,
                frametime,
                hold_breath_scale: ps.hold_breath_scale,
                weap_idle_time: self.weap_idle_time,
                view_last_idle_factor: self.view_last_idle_factor,
                ..base
            }),
            None => view_damage_angles(base),
        };
        if weapon.is_some() {
            self.weap_idle_time = bob.weap_idle_time;
            self.view_last_idle_factor = bob.view_last_idle_factor;
        }
        let angles = [bob.pitch, bob.yaw, bob.roll];

        // The eye: up and down with the bob, sideways along the (bobbed) view's right.
        let ob = view_org_bob(org);
        let view = [
            ps.viewangles[0] + angles[0],
            ps.viewangles[1] + angles[1],
            ps.viewangles[2] + angles[2],
        ];
        let (_, right, _) = math_iw4::angle_vectors(view);
        let mut origin = [
            ob.horizontal * right[0],
            ob.horizontal * right[1],
            ob.vertical + ob.horizontal * right[2],
        ];
        let land = self.land(ps, now);
        origin[2] += land;
        // Never below 8 units over the feet (IW4L's `VIEW_ORG_BOB_Z_MIN_OFS`).
        let min = VIEW_ORG_BOB_Z_MIN_OFS - ps.view_height_current;
        if origin[2] < min {
            origin[2] = min;
        }
        ViewPose {
            angles,
            origin,
            land,
        }
    }

    /// IW4L `stamp_and_land_origin_z`: a landing (in the air last frame, on the ground now) starts a dip as deep as
    /// the fall was high; the dip's height now.
    fn land(&mut self, ps: &PlayerState, now: i32) -> f32 {
        if self.have_prev
            && self.last_ground == ENTITYNUM_NONE
            && ps.ground_entity_num != ENTITYNUM_NONE
            && let Some(fall) = crash_land_fall_height(
                ps.gravity,
                self.last_origin_z,
                ps.origin[2],
                self.last_velocity_z,
            )
        {
            let dip = crash_land_view_dip(fall);
            if dip > 0 {
                self.land_change = -(dip as f32);
                self.land_time = now;
                self.land_view_dip = dip;
                self.landings += 1;
            }
        }
        self.last_origin_z = ps.origin[2];
        self.last_velocity_z = ps.velocity[2];
        self.last_ground = ps.ground_entity_num;
        self.have_prev = true;
        land_origin_z(now.wrapping_sub(self.land_time) as f32, self.land_change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> PlayerState {
        let mut ps = PlayerState::ZERO;
        ps.gravity = 800;
        ps.view_height_target = 60;
        ps.view_height_current = 60.0;
        ps.hold_breath_scale = 1.0;
        ps.ground_entity_num = 0;
        ps
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn standing_still_changes_nothing() {
        let mut k = ViewKick::default();
        let ps = standing();
        for t in 0..10 {
            let v = k.frame(&ps, Some(&ViewWeapon::default()), 1000 + t * 16, 0.016);
            assert_eq!(v, ViewPose::default());
        }
    }

    #[test]
    fn running_bobs_the_eye_as_iw4l() {
        let mut k = ViewKick::default();
        let mut ps = standing();
        ps.velocity = [190.0, 0.0, 0.0];
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for c in 0..=255u32 {
            ps.bob_cycle = c as _;
            let v = k.frame(&ps, None, 1000, 0.016);
            let want = view_org_bob(ViewOrgBobInputs {
                bob_cycle: c as u8,
                xyspeed: 190.0,
                view_height_target: 60,
                ..ViewOrgBobInputs::default()
            });
            assert!(close(v.origin[2], want.vertical), "{c}: {v:?} {want:?}");
            // Facing +x, the view's right is -y.
            assert!(close(v.origin[1], -want.horizontal), "{c}");
            lo = lo.min(v.origin[2]);
            hi = hi.max(v.origin[2]);
        }
        // IW4L's standing amplitude, 0.007 × 190 = 1.33 units, through 0.75 × (0.2 sin(4c + π/2) + sin 2c): about
        // 1.2 down and 0.8 up.
        assert!(
            hi > 0.7 && hi < 0.9 && lo < -1.1 && lo > -1.33,
            "{lo}..{hi}"
        );
    }

    #[test]
    fn a_landing_dips_the_view_and_comes_back() {
        let mut k = ViewKick::default();
        let mut ps = standing();
        // Falling at 400 units/s, then on the ground 6 units lower.
        ps.ground_entity_num = ENTITYNUM_NONE;
        ps.origin[2] = 106.0;
        ps.velocity[2] = -400.0;
        k.frame(&ps, None, 1000, 0.016);
        ps.ground_entity_num = 0;
        ps.origin[2] = 100.0;
        ps.velocity[2] = 0.0;
        let first = k.frame(&ps, None, 1000, 0.016);
        assert_eq!(first.land, 0.0, "the dip starts from 0");
        // A ~100 unit fall: IW4L's dip of round((h - 12) / 26 × 4 + 4).
        let fall =
            crash_land_fall_height(800, 106.0, 100.0, -400.0).expect("a landing has a height");
        assert_eq!(k.land_view_dip, crash_land_view_dip(fall));
        assert!(k.land_view_dip > 4, "{fall} -> {}", k.land_view_dip);
        let deepest = k.frame(&ps, None, 1150, 0.016);
        assert!(
            close(deepest.land, -(k.land_view_dip as f32)),
            "{deepest:?}"
        );
        assert!(close(deepest.origin[2], deepest.land));
        let back = k.frame(&ps, None, 1300, 0.016);
        assert!(back.land < 0.0 && back.land > deepest.land, "{back:?}");
        assert_eq!(k.frame(&ps, None, 1450, 0.016).land, 0.0);
        // A step down (12 units or less of fall) doesn't dip.
        let mut k = ViewKick::default();
        ps.ground_entity_num = ENTITYNUM_NONE;
        ps.velocity[2] = -100.0;
        k.frame(&ps, None, 2000, 0.016);
        ps.ground_entity_num = 0;
        k.frame(&ps, None, 2016, 0.016);
        assert_eq!(k.land_view_dip, 0);
    }

    #[test]
    fn a_hit_kicks_the_view_away_and_back() {
        let ps = standing();
        // Shot from in front: the view tips up (negative pitch is up in IW4... the kick's sign is IW4L's).
        let mut k = ViewKick::default();
        k.hit(&ps, Some([500.0, 0.0, 0.0]), 40, 1000);
        let want = damage_feedback_kick(0, 0, 40, ps.viewangles);
        assert!(close(k.v_dmg_pitch, want.v_dmg_pitch) && close(k.v_dmg_roll, want.v_dmg_roll));
        let peak = k.frame(&ps, None, 1100, 0.016);
        // 40 × 0.2 = 8 degrees at the end of the 100 ms deflection.
        assert!(close(peak.angles[0], 8.0), "{peak:?}");
        let mid = k.frame(&ps, None, 1300, 0.016);
        assert!(mid.angles[0].abs() < 8.0 && mid.angles[0].abs() > 0.0);
        assert_eq!(k.frame(&ps, None, 1500, 0.016).angles, [0.0; 3]);
        // From the right: a roll, no pitch.
        let mut k = ViewKick::default();
        k.hit(&ps, Some([0.0, -500.0, 0.0]), 40, 1000);
        let side = k.frame(&ps, None, 1100, 0.016);
        assert!(
            side.angles[0].abs() < 0.1 && close(side.angles[2].abs(), 8.0),
            "{side:?}"
        );
        // From nowhere (a fall): IW4L's undirected kick, the 5 degree minimum.
        let mut k = ViewKick::default();
        k.hit(&ps, None, 10, 1000);
        let fall = k.frame(&ps, None, 1100, 0.016);
        assert!(close(fall.angles[0], -5.0), "{fall:?}");
    }

    #[test]
    fn the_scope_sways_and_holding_breath_steadies_it() {
        let w = ViewWeapon {
            overlay_reticle: 1,
            aim_down_sight: true,
            idle: WeaponIdleInputs {
                ads_idle_amount: 60.0,
                hip_idle_amount: 10.0,
                ads_idle_speed: 1.0,
                hip_idle_speed: 1.0,
                idle_crouch_factor: 0.6,
                idle_prone_factor: 0.3,
            },
            ..ViewWeapon::default()
        };
        let sway = |breath: f32| {
            let mut k = ViewKick {
                view_last_idle_factor: 1.0,
                ..ViewKick::default()
            };
            let mut ps = standing();
            ps.f_weapon_pos_frac = 1.0;
            ps.hold_breath_scale = breath;
            let mut most = 0.0f32;
            for i in 0..600 {
                let v = k.frame(&ps, Some(&w), 1000 + i * 16, 0.016);
                most = most.max(libm::hypotf(v.angles[0], v.angles[1]));
            }
            most
        };
        let free = sway(1.0);
        // IW4L: amount × ADS fraction × breath × 0.01 = 0.6 degrees at most per axis.
        assert!(free > 0.2 && free < 0.85, "{free}");
        assert_eq!(sway(0.0), 0.0, "breath held: steady");
        assert!(sway(4.5) > free * 3.0, "out of breath: worse");
        // Not aimed: no camera sway (the gun sways in `weapon_view`).
        let mut k = ViewKick::default();
        let ps = standing();
        for i in 0..100 {
            assert_eq!(
                k.frame(&ps, Some(&w), 1000 + i * 16, 0.016).angles,
                [0.0; 3]
            );
        }
    }

    #[test]
    fn damage_bytes_point_at_the_attacker() {
        let (p, y) = vectoangles([0.0, 100.0, 0.0]);
        assert!(close(p, 0.0) && close(y, 90.0));
        let (p, _) = vectoangles([100.0, 0.0, -100.0]);
        assert!(close(p, 45.0), "below is pitch down: {p}");
        assert_eq!(angle_byte(359.9), 254);
    }
}

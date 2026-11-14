//! Where the first-person weapon sits each frame, as IW4L's `apply_fpv_placement` (MW2's
//! `CG_CalculateWeaponPosition`): `weapon_placement_assemble` with the view sway (the gun lags behind turning:
//! `LookSway` springs scaled by the weapon's hip / ADS sway numbers), the movement bob, idle sway, the stance and
//! movement offsets (`standMove` .. `proneRot`), and the gun's recoil. The weapon's numbers come from `WFPV`
//! (`wpk::weapon::FpvRecord`). The result is a view-relative offset and turn, applied about the eye.

use playerstate_iw4::PlayerState;
use weapon_iw4::{
    GunRecoilPlacementState, GunRecoilResponse, LookSway, PLACEMENT_ASSEMBLE_STEP_COUNT,
    StanceTransitionFadeGlobals, SwaySpringState, WeaponBobInputs, WeaponBobWaveformInputs,
    WeaponCombatFacts, WeaponIdleInputs, WeaponMovementKinematics, WeaponMovementOfsInputs,
    WeaponPlacementAssembleStep, WeaponPlacementPsInputs, WeaponPlacementState,
    WeaponStanceStaticOfsInputs, WeaponSwayParams, calculate_weapon_movement_bob_waveform,
    lerp_sway_params, sway_config, sway_from_drive, weapon_placement_assemble,
};
use wpk::weapon::FpvRecord;

use crate::gfx::Mtx34;

/// The weapon's placement state between frames.
#[derive(Clone, Debug, Default)]
pub struct WeaponView {
    state: WeaponPlacementState,
    look: LookSway,
    sway: SwaySpringState,
}

/// The view-relative placement: offset (forward, left, up) and turn (pitch, yaw, roll degrees, IW4 convention).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placed {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
}

fn sway_params(v: [f32; 5]) -> WeaponSwayParams {
    WeaponSwayParams {
        max_angle: v[0],
        pitch_scale: v[1],
        yaw_scale: v[2],
        horiz_scale: v[3],
        vert_scale: v[4],
    }
}

impl WeaponView {
    /// Start over (a spawn or a new weapon).
    pub fn reset(&mut self) {
        *self = WeaponView::default();
    }

    /// Place the weapon for this frame: `ps` is the player's state (view angles, velocity, stance, ADS fraction,
    /// bob cycle), `f` / `fpv` / `gun_max` the weapon's numbers, `gun_recoil` the gun's recoil springs, `dt` the
    /// frame's seconds, `now_ms` the game time (idle sway and damage kick run on it).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        ps: &PlayerState,
        f: &WeaponCombatFacts,
        fpv: &FpvRecord,
        gun_max: [f32; 2],
        gun_recoil: GunRecoilPlacementState,
        dt: f32,
        now_ms: u32,
    ) -> Placed {
        let frac = ps.f_weapon_pos_frac;
        // The gun lags behind turning (IW4L `ViewSwayState::advance`).
        if dt > 0.0 {
            let cfg = sway_config();
            let sample = [f64::from(ps.viewangles[0]), f64::from(ps.viewangles[1])];
            if f.overlay_reticle != 0 && frac > 0.0 {
                if self.look.observe_only(sample, f64::from(dt), cfg) {
                    self.sway = SwaySpringState::default();
                }
            } else if self.look.sample(sample, f64::from(dt), cfg) {
                let params = if f.aim_down_sight {
                    lerp_sway_params(sway_params(fpv.sway_hip), sway_params(fpv.sway_ads), frac)
                } else {
                    sway_params(fpv.sway_hip)
                };
                self.sway = sway_from_drive(self.look.drive(cfg), params, 1.0);
            }
        }
        self.state.sway_springs = self.sway;
        self.state.gun_recoil = gun_recoil;
        self.state.damage_kick_time = now_ms as i32;
        let m = &fpv.movement;
        let r = &fpv.movement_rates;
        let movement = WeaponMovementOfsInputs {
            stand_move: m[0],
            stand_rot: m[1],
            strafe_move: m[2],
            strafe_rot: m[3],
            ducked_move: m[4],
            ducked_rot: m[5],
            prone_move: m[6],
            prone_rot: m[7],
            pos_move_rate: r[0],
            pos_prone_move_rate: r[1],
            stand_move_min_speed: r[2],
            ducked_move_min_speed: r[3],
            prone_move_min_speed: r[4],
            pos_rot_rate: r[5],
            pos_prone_rot_rate: r[6],
        };
        let xyspeed =
            libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]);
        let kinematics = WeaponMovementKinematics {
            xyspeed,
            speed: ps.speed as f32,
            velocity: ps.velocity,
            viewangles: ps.viewangles,
            weaponstate: ps.weaponstate_primary,
            weaponstate_secondary: ps.weaponstate_secondary,
            pm_flags: ps.pm_flags,
            frametime: dt,
        };
        let waveform = calculate_weapon_movement_bob_waveform(WeaponBobWaveformInputs {
            bob_cycle: (ps.bob_cycle as u32 & 0xff) as u8,
            xyspeed,
            view_height_target: ps.view_height_target,
            pm_flags: ps.pm_flags,
            weapon_pos_frac: frac,
        });
        let idle = WeaponIdleInputs {
            ads_idle_amount: fpv.idle[0],
            hip_idle_amount: fpv.idle[1],
            ads_idle_speed: fpv.idle[2],
            hip_idle_speed: fpv.idle[3],
            idle_crouch_factor: fpv.idle[4],
            idle_prone_factor: fpv.idle[5],
        };
        let mut steps = [WeaponPlacementAssembleStep::Sway; PLACEMENT_ASSEMBLE_STEP_COUNT];
        let c = weapon_placement_assemble(
            &mut self.state,
            WeaponPlacementPsInputs {
                e_flags: ps.e_flags,
                weapon_pos_frac: frac,
                weapon_time: ps.weapon_time,
                aim_down_sight: f.aim_down_sight,
                overlay_reticle: f.overlay_reticle,
                weapon_transition_active: false,
                lean_fraction: 0.0,
            },
            WeaponStanceStaticOfsInputs {
                ducked_ofs: fpv.ducked_ofs,
                prone_ofs: fpv.prone_ofs,
                ads_aim_pitch: fpv.ads_aim_pitch,
                night_vision_wear_time: 0,
            },
            StanceTransitionFadeGlobals::default(),
            movement,
            kinematics,
            WeaponBobInputs {
                ads_bob_factor: fpv.ads_bob_factor,
            },
            idle,
            Some(waveform),
            GunRecoilResponse::default(),
            GunRecoilResponse::default(),
            gun_max[0],
            gun_max[1],
            // The gun's recoil springs are advanced by the gun itself.
            0.0,
            &mut steps,
        );
        Placed {
            origin: c.origin,
            angles: c.angles,
        }
    }
}

/// The placement as an eye-space transform (GX eye: x right, y up, looking down -z), turning and moving the weapon
/// about the eye: IW4's view-relative (forward, left, up) frame mapped to eye axes.
pub fn eye_transform(p: &Placed) -> Mtx34 {
    // IW4 axes of the turned weapon in the view's frame (rows: forward, left, up).
    let r = math_iw4::angles_to_axis(p.angles);
    // eye = C · iw, with C mapping (forward, left, up) to (-z, -x, +y).
    let to_eye = |v: [f32; 3]| [-v[1], v[2], -v[0]];
    // Columns of the eye-space rotation: the eye images of each eye axis's IW direction through R^T.
    // A weapon vector along IW axis k ends up along r[k] in the view frame.
    let col = |iw_axis: usize| to_eye(r[iw_axis]);
    let (f, l, u) = (col(0), col(1), col(2));
    // Eye basis vectors expressed as IW axes: x = -left, y = up, z = -forward.
    let ex = [-l[0], -l[1], -l[2]];
    let ey = u;
    let ez = [-f[0], -f[1], -f[2]];
    let t = to_eye(p.origin);
    [
        [ex[0], ey[0], ez[0], t[0]],
        [ex[1], ey[1], ez[1], t[1]],
        [ex[2], ey[2], ez[2], t[2]],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn placement_maps_to_eye_space() {
        // No placement: identity.
        let m = eye_transform(&Placed::default());
        for (r, row) in m.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!(close(*v, if r == c { 1.0 } else { 0.0 }), "{m:?}");
            }
        }
        // Forward 2, left 3, up 4 -> eye (-3, 4, -2).
        let m = eye_transform(&Placed {
            origin: [2.0, 3.0, 4.0],
            angles: [0.0; 3],
        });
        assert!(close(m[0][3], -3.0) && close(m[1][3], 4.0) && close(m[2][3], -2.0));
        // Yaw 90 (turn left): what pointed forward (-z) now points left (-x).
        let m = eye_transform(&Placed {
            origin: [0.0; 3],
            angles: [0.0, 90.0, 0.0],
        });
        let fwd = [-m[0][2], -m[1][2], -m[2][2]];
        assert!(close(fwd[0], -1.0) && close(fwd[2], 0.0), "{fwd:?}");
        // Pitch 10 (IW4: down): forward tips towards -y.
        let m = eye_transform(&Placed {
            origin: [0.0; 3],
            angles: [10.0, 0.0, 0.0],
        });
        assert!(-m[1][2] < 0.0, "{m:?}");
    }

    #[test]
    fn turning_makes_the_gun_lag_and_settle() {
        let mut v = WeaponView::default();
        let f = WeaponCombatFacts::default();
        let fpv = FpvRecord {
            sway_hip: [3.0, 0.5, 1.0, 0.5, 0.5],
            ..FpvRecord::default()
        };
        let mut ps = PlayerState::ZERO;
        let mut yaws = alloc::vec::Vec::new();
        for i in 0..60 {
            // Turn for half a second, then hold still.
            if i < 30 {
                ps.viewangles[1] += 3.0;
            }
            let p = v.update(
                &ps,
                &f,
                &fpv,
                [0.0, 0.0],
                GunRecoilPlacementState::default(),
                1.0 / 60.0,
                i * 16,
            );
            yaws.push(p.angles[1]);
        }
        assert!(
            yaws[29].abs() > 0.05,
            "the gun lags while turning: {}",
            yaws[29]
        );
        // Once the turn stops the spring swings back past the middle (IW4L's look sway), staying small.
        assert!(
            yaws[29] * yaws[59] < 0.0,
            "swings back: {} then {}",
            yaws[29],
            yaws[59]
        );
        assert!(yaws.iter().all(|y| y.abs() < 2.0));
    }
}

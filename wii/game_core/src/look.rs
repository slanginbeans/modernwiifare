//! Gamepad look and aim assist, ported from IW4L (`crates/net/src/client/pad_aim.rs` `pad_look_frame` and
//! `crates/console/src/gamepad.rs` stick shaping, Apache-2.0, rev `acc57a5`): a radial dead zone, IW4L's default look
//! curve, turn rates of 260 / 90 °/s (hip) and 90 / 55 °/s (ADS) scaled by the zoom, turn acceleration, and aim
//! assist: slowdown over a target, lock-on that follows a moving target, and the ADS snap. The constants are IW4L's
//! (its own reading of MW2's console feel, not MW2 data).
//!
//! The GameCube sticks don't reach the full s8 range (the gates stop short); [`StickRange`] learns how far each
//! stick actually goes and scales that to full deflection.

use alloc::vec::Vec;

pub use weapon_iw4::AimAssistRanges;

/// MW2's hip field of view (horizontal at 4:3, `cg_fov`).
pub const HIP_FOV: f32 = hud_iw4::fov::CG_FOV_DEFAULT;

const TURN_YAW: f32 = 260.0;
const TURN_PITCH: f32 = 90.0;
const TURN_YAW_ADS: f32 = 90.0;
const TURN_PITCH_ADS: f32 = 55.0;
const TURN_ACCEL: f32 = 1200.0;

const SLOWDOWN_HIP: f32 = 0.4;
const SLOWDOWN_ADS: f32 = 0.5;
const SLOWDOWN_REGION: [f32; 2] = [90.0, 90.0];
const LOCKON_REGION: [f32; 2] = [90.0, 90.0];
const AUTOAIM_REGION: [f32; 2] = [160.0, 120.0];
const LOCKON_STRENGTH: f32 = 0.6;
const LOCKON_DEFLECTION: f32 = 0.05;
const AUTOAIM_LERP: f32 = 40.0;
const AUTOAIM_TIME: f32 = 0.5;
const FALLBACK_RANGE: f32 = 1500.0;

/// IW4L's default stick dead zone (fraction of full deflection).
pub const DEADZONE: f32 = 0.12;
/// Before a stick has been pushed further, this much counts as full (s8 units). The learned range never drops below
/// it, so a stick resting off-centre can't become hypersensitive.
const MIN_FULL: f32 = 64.0;

/// How far each stick has been seen to go (radially), which then counts as full deflection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickRange {
    pub full: [f32; 2],
}

impl Default for StickRange {
    fn default() -> Self {
        StickRange {
            full: [MIN_FULL; 2],
        }
    }
}

impl StickRange {
    /// Stick `which` (0 main, 1 C) at raw `(x, y)`: learn its range, then return it as -1..1 with IW4L's radial dead
    /// zone (`console::gamepad::radial`).
    pub fn normalize(&mut self, which: usize, x: i8, y: i8) -> [f32; 2] {
        let v = [f32::from(x), f32::from(y)];
        let len = libm::sqrtf(v[0] * v[0] + v[1] * v[1]);
        let full = &mut self.full[which.min(1)];
        *full = full.max(len).min(127.0);
        let length = len / *full;
        if length <= DEADZONE || length <= f32::EPSILON {
            return [0.0; 2];
        }
        let scaled = ((length - DEADZONE) / (1.0 - DEADZONE)).min(1.0);
        [v[0] / len * scaled, v[1] / len * scaled]
    }
}

/// A normalised stick as IW4 move amounts (IW4L `input_iw4::axis_to_move` rounds): (forward, right).
pub fn to_move(stick: [f32; 2]) -> (i8, i8) {
    let m = |v: f32| libm::roundf(v * 127.0).clamp(-127.0, 127.0) as i8;
    (m(stick[1]), m(stick[0]))
}

/// IW4L's default look curve (`curve` kind 0): gentle near the centre, full at the edge.
pub fn curve(deflection: f32) -> f32 {
    let d = deflection.clamp(0.0, 1.0);
    0.35 * d + 0.65 * d * d * d
}

/// Settings (IW4L `GameSettings` defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookSettings {
    pub sensitivity: f32,
    pub ads_sensitivity: f32,
    pub acceleration: bool,
    /// 0 off, 1 slowdown + lock-on, 2 also the ADS snap (MW2's console target assist).
    pub aim_assist: u8,
    pub invert: bool,
}

impl Default for LookSettings {
    fn default() -> Self {
        LookSettings {
            sensitivity: 1.0,
            ads_sensitivity: 1.0,
            acceleration: true,
            aim_assist: 2,
            invert: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AimTarget {
    pub key: u64,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub aim: [f32; 3],
    pub velocity: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct AimView {
    pub eye: [f32; 3],
    /// Pitch and yaw, degrees.
    pub angles: [f32; 2],
    pub velocity: [f32; 3],
    pub ads_lerp: f32,
    /// The current field of view over the hip one (65°).
    pub fov_scale: f32,
    pub ranges: AimAssistRanges,
    pub dt: f32,
}

/// What persists between frames.
#[derive(Clone, Copy, Debug, Default)]
pub struct LookState {
    turn_rate: [f32; 2],
    lockon: Option<u64>,
    autoaim: Option<(u64, f32)>,
    was_ads: bool,
}

struct OnScreen {
    target: AimTarget,
    min: [f32; 2],
    max: [f32; 2],
    dist_sqr: f32,
    crosshair_sqr: f32,
}

fn axes(angles: [f32; 2]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sp, cp) = (
        libm::sinf(angles[0].to_radians()),
        libm::cosf(angles[0].to_radians()),
    );
    let (sy, cy) = (
        libm::sinf(angles[1].to_radians()),
        libm::cosf(angles[1].to_radians()),
    );
    (
        [cp * cy, cp * sy, -sp],
        [sy, -cy, 0.0],
        [sp * cy, sp * sy, cp],
    )
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn angles_to(eye: [f32; 3], point: [f32; 3]) -> [f32; 2] {
    let d = [point[0] - eye[0], point[1] - eye[1], point[2] - eye[2]];
    let flat = libm::sqrtf(d[0] * d[0] + d[1] * d[1]);
    [
        -libm::atan2f(d[2], flat).to_degrees(),
        libm::atan2f(d[1], d[0]).to_degrees(),
    ]
}

fn angle_delta(to: f32, from: f32) -> f32 {
    let d = to - from + 540.0;
    d - libm::floorf(d / 360.0) * 360.0 - 180.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn project(view: &AimView, targets: &[AimTarget]) -> Vec<OnScreen> {
    let (forward, right, up) = axes(view.angles);
    let tan_x = (libm::tanf(32.5f32.to_radians()) * view.fov_scale).max(1e-3);
    let tan_y = tan_x * 9.0 / 16.0;
    let mut out: Vec<OnScreen> = targets
        .iter()
        .filter_map(|target| {
            let mut min = [f32::MAX; 2];
            let mut max = [f32::MIN; 2];
            for corner in 0..8 {
                let p = [
                    if corner & 1 == 0 {
                        target.mins[0]
                    } else {
                        target.maxs[0]
                    },
                    if corner & 2 == 0 {
                        target.mins[1]
                    } else {
                        target.maxs[1]
                    },
                    if corner & 4 == 0 {
                        target.mins[2]
                    } else {
                        target.maxs[2]
                    },
                ];
                let d = [p[0] - view.eye[0], p[1] - view.eye[1], p[2] - view.eye[2]];
                let depth = dot(d, forward);
                if depth <= 0.0 {
                    continue;
                }
                let x = dot(d, right) / (depth * tan_x);
                let y = -dot(d, up) / (depth * tan_y);
                min = [min[0].min(x), min[1].min(y)];
                max = [max[0].max(x), max[1].max(y)];
            }
            if max[0] <= min[0]
                || max[1] <= min[1]
                || min[0] > 1.0
                || min[1] > 1.0
                || max[0] < -1.0
                || max[1] < -1.0
            {
                return None;
            }
            let min = [min[0].clamp(-1.0, 1.0), min[1].clamp(-1.0, 1.0)];
            let max = [max[0].clamp(-1.0, 1.0), max[1].clamp(-1.0, 1.0)];
            let centre = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
            let d = [
                target.aim[0] - view.eye[0],
                target.aim[1] - view.eye[1],
                target.aim[2] - view.eye[2],
            ];
            Some(OnScreen {
                target: *target,
                min,
                max,
                dist_sqr: dot(d, d),
                crosshair_sqr: centre[0] * centre[0] + centre[1] * centre[1],
            })
        })
        .collect();
    out.sort_by(|a, b| a.crosshair_sqr.total_cmp(&b.crosshair_sqr));
    out
}

fn best(screen: &[OnScreen], range: f32, region: [f32; 2], scale: f32) -> Option<&OnScreen> {
    let half = [region[0] / 640.0 * scale, region[1] / 480.0 * scale];
    screen.iter().find(|s| {
        s.dist_sqr <= range * range
            && s.min[0] <= half[0]
            && s.max[0] >= -half[0]
            && s.min[1] <= half[1]
            && s.max[1] >= -half[1]
    })
}

/// One look step: the view change (pitch, yaw degrees) from the C-stick `look` (normalised, x right, y up) over
/// `view.dt`, with aim assist against `targets` (visible enemies). IW4L `pad_look_frame`.
pub fn look_frame(
    st: &mut LookState,
    set: &LookSettings,
    look: [f32; 2],
    view: &AimView,
    targets: &[AimTarget],
    ads: bool,
) -> [f32; 2] {
    let dt = view.dt;
    let deflection = libm::sqrtf(look[0] * look[0] + look[1] * look[1]);
    let shaped = if deflection <= f32::EPSILON {
        [0.0; 2]
    } else {
        let c = curve(deflection) / deflection;
        [
            look[0] * c,
            if set.invert {
                -look[1] * c
            } else {
                look[1] * c
            },
        ]
    };
    let ads_lerp = view.ads_lerp.clamp(0.0, 1.0);
    let sensitivity = set.sensitivity * lerp(1.0, set.ads_sensitivity, ads_lerp);
    let zoom = view.fov_scale.clamp(0.02, 2.0);
    let max_yaw = lerp(TURN_YAW, TURN_YAW_ADS, ads_lerp) * zoom * sensitivity;
    let max_pitch = lerp(TURN_PITCH, TURN_PITCH_ADS, ads_lerp) * zoom * sensitivity;
    let wanted = [-shaped[1] * max_pitch, -shaped[0] * max_yaw];
    for (rate, goal) in st.turn_rate.iter_mut().zip(wanted) {
        if set.acceleration && libm::fabsf(goal) > libm::fabsf(*rate) {
            let step = TURN_ACCEL * set.sensitivity * dt;
            *rate = (libm::fabsf(*rate) + step).min(libm::fabsf(goal)) * goal.signum();
        } else {
            *rate = goal;
        }
    }
    let mut delta = [st.turn_rate[0] * dt, st.turn_rate[1] * dt];
    if set.aim_assist == 0 {
        st.lockon = None;
        st.autoaim = None;
        st.was_ads = ads;
        return delta;
    }
    let screen = project(view, targets);
    let range = |hip: f32, ads: f32| {
        let hip = if hip > 0.0 { hip } else { FALLBACK_RANGE };
        let ads = if ads > 0.0 { ads } else { hip };
        lerp(hip, ads, ads_lerp)
    };
    let assist_range = range(view.ranges.hip, view.ranges.ads);
    if best(&screen, assist_range, SLOWDOWN_REGION, 1.0).is_some() {
        let slow = lerp(SLOWDOWN_HIP, SLOWDOWN_ADS, ads_lerp);
        delta[0] *= slow;
        delta[1] *= slow;
    }
    let kept = st
        .lockon
        .and_then(|key| screen.iter().find(|s| s.target.key == key))
        .filter(|s| best(core::slice::from_ref(*s), assist_range, LOCKON_REGION, 1.0).is_some());
    let lock = kept.or_else(|| best(&screen, assist_range, LOCKON_REGION, 1.0));
    st.lockon = lock.map(|s| s.target.key);
    if let Some(lock) = lock
        && st.autoaim.is_none()
        && deflection > LOCKON_DEFLECTION
    {
        let t = lock.target;
        let moved: [f32; 3] =
            core::array::from_fn(|k| t.aim[k] + (t.velocity[k] - view.velocity[k]) * dt);
        let now = angles_to(view.eye, t.aim);
        let next = angles_to(view.eye, moved);
        delta[0] += LOCKON_STRENGTH * angle_delta(next[0], now[0]);
        delta[1] += LOCKON_STRENGTH * angle_delta(next[1], now[1]);
    }
    if set.aim_assist == 2 && ads && !st.was_ads {
        let auto_range = if view.ranges.auto_aim > 0.0 {
            view.ranges.auto_aim
        } else {
            FALLBACK_RANGE
        };
        st.autoaim = best(&screen, auto_range, AUTOAIM_REGION, (1.0 / zoom).max(1.0))
            .map(|s| (s.target.key, AUTOAIM_TIME));
    }
    if !ads {
        st.autoaim = None;
    }
    if let Some((key, left)) = st.autoaim {
        match screen
            .iter()
            .find(|s| s.target.key == key)
            .filter(|_| left > 0.0)
        {
            Some(s) => {
                let goal = angles_to(view.eye, s.target.aim);
                let pitch = angle_delta(goal[0], view.angles[0]);
                let yaw = angle_delta(goal[1], view.angles[1]);
                let length = libm::sqrtf(pitch * pitch + yaw * yaw);
                let step = (AUTOAIM_LERP * dt).min(length);
                if length > 1e-4 {
                    delta[0] += pitch / length * step;
                    delta[1] += yaw / length * step;
                }
                st.autoaim = (length > 0.25).then_some((key, left - dt));
            }
            None => st.autoaim = None,
        }
    }
    st.was_ads = ads;
    delta
}

/// The view's horizontal field of view (degrees, MW2's 65 at the hip) for an ADS fraction: IW4L
/// `hud_iw4::calc_fov_from_ads` with the weapon's zoom FOV and zoom window. `zooming_in`: moving into ADS.
pub fn view_fov(
    ads_frac: f32,
    zooming_in: bool,
    aim: &wpk::weapon::AimRecord,
    has_weapon: bool,
) -> f32 {
    let inputs = hud_iw4::fov::FovInputs {
        cg_fov: hud_iw4::fov::CG_FOV_DEFAULT,
        pm_type: 0,
        link_flags: 0,
        e_flags: 0,
        weapon_index_nonzero: has_weapon,
        aim_down_sight: has_weapon && aim.ads_zoom_fov > 0.0,
        ads_zoom_fov: aim.ads_zoom_fov,
        overlay_zoom: 0.0,
        fov_scale: hud_iw4::fov::CG_FOV_SCALE_DEFAULT,
        fov_min: hud_iw4::fov::CG_FOV_MIN_DEFAULT,
    };
    let overlay = hud_iw4::WeaponAdsOverlayFacts {
        ads_zoom_in_frac: aim.ads_zoom_in_frac,
        ads_zoom_out_frac: aim.ads_zoom_out_frac,
        ..Default::default()
    };
    hud_iw4::fov::calc_fov_from_ads(&inputs, ads_frac, zooming_in, &overlay).0
}

/// MW2's horizontal FOV (4:3) as the vertical FOV our camera takes (IW4 `cg_fov` is the 4:3 horizontal).
pub fn vertical_fov(horizontal_deg: f32) -> f32 {
    hud_iw4::fov::horizontal_to_vertical_fov_deg(horizontal_deg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticks_learn_their_range() {
        let mut r = StickRange::default();
        // Before it's been pushed far, 64 counts as full.
        assert_eq!(r.normalize(0, 64, 0), [1.0, 0.0]);
        // A GameCube stick stopping at 100: 100 becomes full, half of it is just under half after the dead zone.
        assert_eq!(r.normalize(0, 100, 0), [1.0, 0.0]);
        let half = r.normalize(0, 50, 0)[0];
        assert!((0.4..0.5).contains(&half), "{half}");
        assert_eq!(r.normalize(0, 5, 5), [0.0, 0.0]);
        // Full forward is 127 for IW4 (enough to sprint).
        assert_eq!(to_move(r.normalize(0, 0, 100)), (127, 0));
    }

    fn view(angles: [f32; 2]) -> AimView {
        AimView {
            eye: [0.0, 0.0, 60.0],
            angles,
            velocity: [0.0; 3],
            ads_lerp: 0.0,
            fov_scale: 1.0,
            ranges: AimAssistRanges::NONE,
            dt: 1.0 / 60.0,
        }
    }

    #[test]
    fn turns_accelerate_and_aim_assist_slows_over_a_target() {
        let set = LookSettings::default();
        let mut st = LookState::default();
        // Full right on the C-stick: yaw rate builds up at 1200 °/s² towards 260 °/s.
        let d1 = look_frame(&mut st, &set, [1.0, 0.0], &view([0.0, 0.0]), &[], false);
        assert!((d1[1] + 20.0 / 60.0).abs() < 1e-3, "{d1:?}");
        for _ in 0..60 {
            look_frame(&mut st, &set, [1.0, 0.0], &view([0.0, 0.0]), &[], false);
        }
        let free = look_frame(&mut st, &set, [1.0, 0.0], &view([0.0, 0.0]), &[], false);
        assert!((free[1] + 260.0 / 60.0).abs() < 1e-3, "{free:?}");
        // A target under the crosshair: the same push turns slower.
        let t = AimTarget {
            key: 1,
            mins: [500.0, -15.0, 0.0],
            maxs: [530.0, 15.0, 70.0],
            aim: [515.0, 0.0, 45.0],
            velocity: [0.0; 3],
        };
        let slowed = look_frame(&mut st, &set, [1.0, 0.0], &view([0.0, 0.0]), &[t], false);
        assert!(
            slowed[1].abs() < free[1].abs() * 0.5,
            "{slowed:?} vs {free:?}"
        );
    }

    #[test]
    fn ads_snaps_towards_a_target_near_the_crosshair() {
        let set = LookSettings::default();
        let mut st = LookState::default();
        let t = AimTarget {
            key: 7,
            mins: [500.0, 40.0, 0.0],
            maxs: [530.0, 70.0, 70.0],
            aim: [515.0, 55.0, 45.0],
            velocity: [0.0; 3],
        };
        let mut v = view([0.0, 0.0]);
        look_frame(&mut st, &set, [0.0, 0.0], &v, &[t], false);
        v.ads_lerp = 1.0;
        let d = look_frame(&mut st, &set, [0.0, 0.0], &v, &[t], true);
        // The target is to the left (positive yaw): the snap turns that way.
        assert!(d[1] > 0.1, "{d:?}");
    }

    #[test]
    fn ads_zooms_to_the_weapons_fov() {
        let aim = wpk::weapon::AimRecord {
            ads_zoom_fov: 50.0,
            ads_zoom_in_frac: 0.7,
            ads_zoom_out_frac: 0.1,
            ..Default::default()
        };
        assert_eq!(view_fov(0.0, true, &aim, true), 65.0);
        assert_eq!(view_fov(1.0, true, &aim, true), 50.0);
        let mid = view_fov(0.65, true, &aim, true);
        assert!(mid < 65.0 && mid > 50.0, "{mid}");
        assert!((vertical_fov(65.0) - 51.08).abs() < 0.05);
    }
}

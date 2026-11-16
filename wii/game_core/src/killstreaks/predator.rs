//! The Predator missile (`_remotemissile.gsc` for the launch, IW4L `sim/src/remote_missile.rs` for the flight).
//!
//! It is fired from `missileRemoteLaunchVert` above and `missileRemoteLaunchHorz` behind its caller at the point
//! `missileRemoteLaunchTargetDist` ahead (`MagicBullet`). The pilot steers it: IW4L `steer` turns the missile's axis
//! by the remote stick (15° a second of pitch, 20° of yaw at full deflection) and keeps its pitch between 1° and 87°;
//! IW4L `advance` holds it at 3000 units a second (up at 2000/s², down at 500/s²) until the pilot, having let go of
//! fire once (armed), fires again: one boost to 6000. It flies through sky brushes (IW4L `projectile_mask`) and blows
//! up on the first thing it hits with its weapon's (`remotemissile_projectile_mp`) radius and damage. A pilot who dies
//! loses the link; the missile flies on.

use alloc::vec::Vec;

use super::{Note, Streak, Streaks};
use crate::arena::{Arena, Event};
use crate::collision::MapClip;
use crate::gun::MASK_SHOT;

/// IW4L `remote_missile`: pitch range, turn rates (degrees a second at full stick), speeds.
pub const PITCH_RANGE: [f32; 2] = [1.0, 87.0];
pub const PITCH_RATE: f32 = 15.0;
pub const YAW_RATE: f32 = 20.0;
pub const SPEED_RANGE: [f32; 2] = [3_000.0, 6_000.0];
pub const SPEED_UP: f32 = 2_000.0;
pub const SPEED_DOWN: f32 = 500.0;
/// IW4L `bullet_collision::CONTENTS_SKY`: remote missiles pass through the sky.
pub const CONTENTS_SKY: u32 = 0x0000_0800;
/// IW4L's match tick (`advance` runs once a tick).
const TICK_S: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Missile {
    pub owner: usize,
    pub origin: [f32; 3],
    /// Pitch, yaw, roll (degrees).
    pub angles: [f32; 3],
    pub speed: f32,
    /// The pilot let go of fire since launch; the boost was used; fire is held now.
    pub armed: bool,
    pub boosted: bool,
    pub attack: bool,
    /// Steered by its owner (false once the owner died).
    pub linked: bool,
    /// Seconds not yet run as ticks.
    pub tick_left: f32,
}

/// IW4L `steer`: turn `angles` by the remote stick (`pitch`, `yaw` in -127..127) over `seconds`.
pub fn steer_angles(angles: [f32; 3], pitch: i8, yaw: i8, seconds: f32) -> [f32; 3] {
    let delta = math_iw4::angles_to_axis([
        f32::from(pitch) / 127.0 * seconds * PITCH_RATE,
        f32::from(yaw) / 127.0 * seconds * YAW_RATE,
        0.0,
    ]);
    let transposed: [[f32; 3]; 3] =
        core::array::from_fn(|row| core::array::from_fn(|col| delta[col][row]));
    let mut a = math_iw4::axis_to_angles(math_iw4::matrix_multiply(
        transposed,
        math_iw4::angles_to_axis(angles),
    ));
    a[0] = (a[0] / 360.0 - libm::floorf(a[0] / 360.0 + 0.5)) * 360.0;
    a[0] = a[0].clamp(PITCH_RANGE[0], PITCH_RANGE[1]);
    a
}

/// IW4L `advance`'s speed for one tick: the boost, or towards the cruise speed.
pub fn tick_speed(speed: f32, armed: bool, boosted: &mut bool, attack: bool) -> f32 {
    if armed && !*boosted && attack {
        *boosted = true;
        return SPEED_RANGE[1];
    }
    let target = SPEED_RANGE[0];
    if speed < target {
        (speed + SPEED_UP * TICK_S).min(target)
    } else {
        (speed - SPEED_DOWN * TICK_S).max(target)
    }
}

impl Arena {
    /// Fighter `owner` fires a Predator missile (`_remotemissile.gsc`'s launch over its caller).
    pub fn launch_predator(&mut self, owner: usize) {
        let r = self.streaks.rules;
        let p = &self.fighters[owner].player;
        let o = p.ps.origin;
        let f = super::yaw_dir(p.view().1);
        let start = [
            o[0] - f[0] * r.predator_back,
            o[1] - f[1] * r.predator_back,
            o[2] + r.predator_up,
        ];
        let target = [
            o[0] + f[0] * r.predator_ahead,
            o[1] + f[1] * r.predator_ahead,
            o[2],
        ];
        let dir: [f32; 3] = core::array::from_fn(|k| target[k] - start[k]);
        let speed = self.streaks.weapons.predator_speed.max(0.0);
        self.streaks.missiles.push(Missile {
            owner,
            origin: start,
            angles: math_iw4::vect_to_angles(dir),
            speed,
            armed: false,
            boosted: false,
            attack: false,
            linked: true,
            tick_left: 0.0,
        });
        self.streaks.notes.push(Note::Called {
            fighter: owner,
            streak: Streak::Predator,
        });
    }

    /// The pilot's stick and fire for fighter `owner`'s missile this frame (`seconds` long).
    pub fn steer_predator(&mut self, owner: usize, pitch: i8, yaw: i8, fire: bool, seconds: f32) {
        if let Some(m) = self
            .streaks
            .missiles
            .iter_mut()
            .find(|m| m.owner == owner && m.linked)
        {
            m.angles = steer_angles(m.angles, pitch, yaw, seconds);
            m.attack = fire;
            m.armed |= !fire;
        }
    }
}

/// Fly the missiles; each blows up on what it hits.
pub(super) fn update(
    a: &mut Arena,
    s: &mut Streaks,
    clip: &MapClip<'_>,
    dt: f32,
    now: u32,
    out: &mut Vec<Event>,
) {
    let mask = MASK_SHOT & !CONTENTS_SKY;
    let mut blasts: Vec<(usize, [f32; 3], u8)> = Vec::new();
    for m in &mut s.missiles {
        if m.linked && !a.fighters[m.owner].life.alive() {
            m.linked = false;
        }
        // Bots fly theirs onto the nearest enemy (see `bots::pilot`).
        m.tick_left += dt;
        while m.tick_left >= TICK_S {
            m.tick_left -= TICK_S;
            m.speed = tick_speed(m.speed, m.armed, &mut m.boosted, m.attack);
        }
        let (dir, _, _) = math_iw4::angle_vectors(m.angles);
        let end: [f32; 3] = core::array::from_fn(|k| m.origin[k] + dir[k] * m.speed * dt);
        let t = clip.trace(m.origin, end, [0.0; 3], [0.0; 3], mask);
        // A player in the way stops it too.
        let mut frac = if t.startsolid != 0 { 0.0 } else { t.fraction };
        for (i, f) in a.fighters.iter().enumerate() {
            if !f.life.alive() || (i == 0 && !a.human_present) {
                continue;
            }
            let boxes = crate::damage::boxes(f.player.ps.origin, f.player.hull_height());
            if let Some((hit, _)) = crate::damage::first_hit(m.origin, end, &boxes)
                && hit < frac
            {
                frac = hit;
            }
        }
        m.origin = core::array::from_fn(|k| m.origin[k] + (end[k] - m.origin[k]) * frac);
        if frac < 1.0 || m.origin[2] < -65_000.0 {
            blasts.push((m.owner, m.origin, ((t.surface_flags >> 20) & 0x1f) as u8));
            m.speed = -1.0;
        }
    }
    s.missiles.retain(|m| m.speed >= 0.0);
    let Some(b) = s.weapons.predator else {
        // Without the weapon's numbers: MW2's stock Predator blast.
        for (owner, at, surface) in blasts {
            blast(
                a,
                clip,
                owner,
                at,
                0,
                surface,
                (600.0, 2000.0, 300.0),
                now,
                out,
            );
        }
        return;
    };
    for (owner, at, surface) in blasts {
        blast(
            a,
            clip,
            owner,
            at,
            b.weapon,
            surface,
            (b.radius, b.inner, b.outer),
            now,
            out,
        );
    }
}

/// An explosion from a killstreak weapon: the effect and sound event, and the damage (radius, inner, outer).
#[allow(clippy::too_many_arguments)]
pub(crate) fn blast(
    a: &mut Arena,
    clip: &MapClip<'_>,
    owner: usize,
    at: [f32; 3],
    weapon: u32,
    surface: u8,
    (radius, inner, outer): (f32, f32, f32),
    now: u32,
    out: &mut Vec<Event>,
) {
    out.push(Event::Explosion {
        origin: at,
        weapon,
        surface,
    });
    a.blast_players(clip, at, radius, outer, inner, Some(owner), now, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_follows_iw4l_rates_and_clamps_the_pitch() {
        // Full yaw stick for one second, nose level: 20° to the right (IW4L turns about the missile's own axes, so
        // pitched down the same stick also rolls it); full pitch stick: 15° less pitch (nose up).
        let a = steer_angles([1.0, 0.0, 0.0], 0, 127, 1.0);
        assert!(
            (a[1] - 340.0).abs() < 0.5 && (a[0] - 1.0).abs() < 0.5,
            "{a:?}"
        );
        let a = steer_angles([45.0, 0.0, 0.0], 0, 127, 1.0);
        assert!(a[1] > 300.0 && a[1] < 350.0 && a[2].abs() > 1.0, "{a:?}");
        let a = steer_angles([45.0, 90.0, 0.0], 127, 0, 1.0);
        assert!(
            (a[0] - 30.0).abs() < 0.5 && (a[1] - 90.0).abs() < 0.5,
            "{a:?}"
        );
        // Never past straight down or level (steered a frame at a time, as it is).
        let mut a = [86.0, 0.0, 0.0];
        for _ in 0..60 {
            a = steer_angles(a, -127, 0, 1.0 / 60.0);
        }
        assert!((a[0] - 87.0).abs() < 1e-3, "{a:?}");
        let a = steer_angles([2.0, 0.0, 0.0], 127, 0, 1.0);
        assert!((a[0] - 1.0).abs() < 1e-3, "{a:?}");
    }

    #[test]
    fn speed_cruises_at_3000_and_boosts_once_after_arming() {
        let mut boosted = false;
        let mut v = 0.0;
        for _ in 0..40 {
            v = tick_speed(v, false, &mut boosted, true);
        }
        assert_eq!(v, 3000.0, "2000/s² for 1.5 s then held");
        // Fire held since launch: not armed, no boost.
        assert_eq!(tick_speed(v, false, &mut boosted, true), 3000.0);
        v = tick_speed(v, true, &mut boosted, true);
        assert_eq!(v, 6000.0);
        assert!(boosted);
        // Down at 500/s² and never boosted again.
        v = tick_speed(v, true, &mut boosted, true);
        assert_eq!(v, 5975.0);
    }

    #[test]
    fn a_predator_falls_from_above_and_kills_the_bot_it_hits() {
        use crate::collision::test_clips;
        use crate::player::Player;
        let floor = test_clips::boxes(&[([-20000.0, -20000.0, -64.0], [20000.0, 20000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let spawns = [([1500.0, 0.0, 0.0], 0.0), ([1500.0, 0.0, 0.0], 0.0)];
        let human = Player::spawn(&clip, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(
            &clip,
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
        a.launch_predator(0);
        let m = a.streaks.missiles[0];
        // 14 000 up, 7000 behind, aimed 1500 ahead.
        let o = a.fighters[0].player.ps.origin;
        assert_eq!(m.origin, [-7000.0, 0.0, o[2] + 14000.0]);
        let mut ev = Vec::new();
        let mut now = 0;
        while !a.streaks.missiles.is_empty() && now < 20_000 {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert!(a.streaks.missiles.is_empty(), "it hit the ground");
        assert!(ev.iter().any(|e| matches!(e, Event::Explosion { .. })));
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
    }
}

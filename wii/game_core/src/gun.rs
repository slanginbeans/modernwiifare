//! The player's gun: IW4L's weapon state machine (`weapon_iw4::weapon_ordinary`: raise, fire rate, bursts, reload,
//! rechamber) driven by the pad, with combat numbers from `common.wpk`, and bullets traced against the map's
//! collision (IW4 `MASK_SHOT`).
//!
//! Aim spread is IW4L's (decay, move / turn / fire increases, stance and ADS cones); a bullet leaves in a random
//! direction inside the cone. Not yet: recoil / view kick, penetration, damage to players, sounds. Timings are IW4L's and therefore the PC game's.

use playerstate_iw4::PlayerState;
use playerstate_iw4::buttons;
use trace_iw4::Trace;
use weapon_iw4::{
    AimSpreadMotion, AimSpreadState, SpreadOverrideState, WeaponCmd, WeaponCombatFacts,
    WeaponHandState, WeaponTickEvent, add_aim_spread_fire, adjust_aim_spread_scale,
    fire_weapon_spread_degrees, get_spread_for_weapon, spawn_weapon_hand, weapon_ordinary,
};
use wpk::weapon::WeaponRecord;

use crate::collision::MapClip;

/// IW4 `MASK_SHOT`.
pub const MASK_SHOT: u32 = 0x0280_6831;

/// What the gun did this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub start: [f32; 3],
    /// Where the bullet stopped (the impact, or the end of its range).
    pub end: [f32; 3],
    pub hit: bool,
    pub normal: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunInput {
    pub fire: bool,
    pub reload: bool,
    /// The player's `pm_flags` (sprinting blocks firing, as in IW4).
    pub pm_flags: u32,
}

/// What aim spread needs from the player each frame (IW4 `PM_UpdateAimDownSightFlag` / `BG_UpdateAimSpread` inputs).
#[derive(Clone, Copy, Debug)]
pub struct SpreadInput<'a> {
    pub ps: &'a PlayerState,
    pub cmd_angles: [i32; 3],
    pub old_cmd_angles: [i32; 3],
    pub forwardmove: i8,
    pub rightmove: i8,
}

pub struct Gun {
    pub name: [u8; wpk::weapon::NAME_LEN],
    facts: WeaponCombatFacts,
    hand: WeaponHandState,
    server_time: i32,
    old_buttons: u32,
    /// Shots fired since the gun was raised (for the HUD / tests).
    pub shots: u32,
    /// IW4 aim spread scale (0..255) and override state.
    spread: AimSpreadState,
    /// Spread cone of the last frame, degrees (for the crosshair).
    pub spread_degrees: f32,
    rng: u32,
    /// ADS fraction of the last frame.
    ads_frac: f32,
}

impl Gun {
    /// A gun from its record, raised as at spawn (first raise). None when IW4L can't build its facts.
    pub fn new(rec: &WeaponRecord) -> Option<Gun> {
        Gun::with_index(rec, 1)
    }

    /// As [`Gun::new`], remembering the record's index in the weapon list (used to find its first-person model).
    pub fn with_index(rec: &WeaponRecord, index: u32) -> Option<Gun> {
        let facts = WeaponCombatFacts::try_from_captured(rec.combat).ok()?;
        if !facts.is_usable() {
            return None;
        }
        Some(Gun {
            name: rec.name,
            hand: spawn_weapon_hand(index, &facts, true),
            facts,
            server_time: 1000,
            old_buttons: 0,
            shots: 0,
            spread: AimSpreadState {
                aim_spread_scale: 0.0,
                spread_override: 0,
                spread_override_state: SpreadOverrideState::None as i32,
            },
            spread_degrees: 0.0,
            rng: 0x1234_5678 ^ index,
            ads_frac: 0.0,
        })
    }

    pub fn name(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    /// Rounds in the magazine and in reserve.
    pub fn ammo(&self) -> (i32, i32) {
        (self.hand.clip, self.hand.stock)
    }

    /// The first-person animation the weapon state machine wants: (IW4 anim slot, restart toggle). The toggle flips
    /// each time an animation is (re)started, so a repeated fire restarts the clip. Idle is the empty idle when the
    /// magazine is empty.
    pub fn anim(&self) -> (u8, bool) {
        let raw = self.hand.weap_anim as u32;
        let restart = raw & weapon_iw4::WEAP_ANIM_RESTART_BIT != 0;
        let slot = match weapon_iw4::slot_for_weap_anim_event(raw) {
            Some(s) => s as u8,
            None if self.hand.clip == 0 => wpk::view::slot::EMPTY_IDLE,
            None => wpk::view::slot::IDLE,
        };
        (slot, restart)
    }

    /// Index of this weapon in `common.wpk`'s weapon list (set by whoever built the gun).
    pub fn weapon_index(&self) -> u32 {
        self.hand.weapon
    }

    /// IW4 weapon state (0 ready, 1 raising, 5 firing, 8.. reloading; see `weapon_iw4::WeaponState`).
    pub fn state(&self) -> i32 {
        self.hand.weaponstate
    }

    /// Update aim spread for this frame (IW4L `adjust_aim_spread_scale`) and the current cone.
    pub fn update_spread(&mut self, msec: i32, s: &SpreadInput<'_>) {
        let ps = s.ps;
        let motion = AimSpreadMotion {
            frametime: msec as f32 / 1000.0,
            cmd_angles: s.cmd_angles,
            old_angles: s.old_cmd_angles,
            forwardmove: s.forwardmove,
            rightmove: s.rightmove,
            velocity_xy: [ps.velocity[0], ps.velocity[1]],
            speed: ps.speed,
            move_speed_threshold: weapon_iw4::AIM_SPREAD_MOVE_SPEED_THRESHOLD_DEFAULT,
        };
        adjust_aim_spread_scale(
            &mut self.spread,
            &self.facts.spread_facts(),
            &self.facts.aim_spread_decay_facts(),
            ps.ground_entity_num,
            ps.pm_type,
            ps.e_flags,
            ps.f_weapon_pos_frac,
            &motion,
        );
        let cone = get_spread_for_weapon(
            ps.view_height_current,
            self.spread.spread_override,
            SpreadOverrideState::from_i32(self.spread.spread_override_state),
            &self.facts.spread_facts(),
            1.0,
        );
        self.spread_degrees = fire_weapon_spread_degrees(
            cone,
            self.facts.ads_spread,
            ps.f_weapon_pos_frac,
            self.spread.aim_spread_scale,
        );
        self.ads_frac = ps.f_weapon_pos_frac;
    }

    fn random(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Run the weapon for `msec`; a fired bullet leaves `eye` along `dir` (unit) inside the spread cone, with
    /// `right` and `up` the view's other axes.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        clip: &MapClip<'_>,
        msec: i32,
        input: &GunInput,
        eye: [f32; 3],
        dir: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
    ) -> (Option<WeaponTickEvent>, Option<Shot>) {
        let mut btn = 0;
        if input.fire {
            btn |= buttons::ATTACK;
        }
        if input.reload {
            btn |= buttons::RELOAD;
        }
        self.server_time += msec;
        let mut cmd = WeaponCmd {
            msec,
            server_time: self.server_time,
            buttons: btn,
            old_buttons: self.old_buttons,
            cmd_weapon: self.hand.weapon as u16,
            cmd_weapon_owned: true,
            pm_flags: input.pm_flags,
            ..WeaponCmd::default()
        };
        self.old_buttons = btn;
        let event = weapon_ordinary(&mut self.hand, &self.facts, &mut cmd);
        let shot = match event {
            Some(WeaponTickEvent::ShotAccepted { .. }) => {
                self.shots += 1;
                // A random direction in the cone (angle and radius uniform, so hits gather towards the centre),
                // then the shot widens the spread (IW4L add_aim_spread_fire).
                let t = libm::tanf(self.spread_degrees.to_radians());
                let a = self.random() * core::f32::consts::TAU;
                let r = self.random() * t;
                let (x, y) = (libm::cosf(a) * r, libm::sinf(a) * r);
                let d: [f32; 3] = core::array::from_fn(|k| dir[k] + right[k] * x + up[k] * y);
                let len = libm::sqrtf(d.iter().map(|v| v * v).sum::<f32>()).max(1e-6);
                let shot = self.trace_bullet(clip, eye, d.map(|v| v / len));
                add_aim_spread_fire(
                    &mut self.spread.aim_spread_scale,
                    self.ads_frac,
                    self.facts.hip_spread_fire_add,
                );
                Some(shot)
            }
            _ => None,
        };
        (event, shot)
    }

    fn trace_bullet(&self, clip: &MapClip<'_>, eye: [f32; 3], dir: [f32; 3]) -> Shot {
        let range = self.facts.bullet_range();
        let end = [
            eye[0] + dir[0] * range,
            eye[1] + dir[1] * range,
            eye[2] + dir[2] * range,
        ];
        let t: Trace = clip.trace(eye, end, [0.0; 3], [0.0; 3], MASK_SHOT);
        let f = t.fraction.clamp(0.0, 1.0);
        Shot {
            start: eye,
            end: [
                eye[0] + (end[0] - eye[0]) * f,
                eye[1] + (end[1] - eye[1]) * f,
                eye[2] + (end[2] - eye[2]) * f,
            ],
            hit: t.fraction < 1.0,
            normal: t.normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use weapon_iw4::{CapturedCombatInput, EmptyReloadPolicy};
    use wpk::clip::Clip;

    const Y: [f32; 3] = [0.0, -1.0, 0.0];
    const Z: [f32; 3] = [0.0, 0.0, 1.0];

    /// A full-auto rifle: 600 rpm (100 ms), 30-round magazine, 2 s reload.
    fn rifle() -> WeaponRecord {
        WeaponRecord::new(
            "test_rifle_mp",
            CapturedCombatInput {
                fire_time_ms: 100,
                raise_time_ms: 500,
                first_raise_time_ms: 500,
                drop_time_ms: 300,
                reload_time_ms: 2000,
                reload_empty_time_ms: 2500,
                empty_reload: EmptyReloadPolicy::Authored,
                clip_size: 30,
                start_ammo: 120,
                max_ammo: 210,
                fire_type: 0,
                weap_type: 0,
                weap_class: 0,
                shots_per_fire: 1,
                damage: 30,
                min_damage: 20,
                max_damage_range: 1000.0,
                min_damage_range: 2000.0,
                ..CapturedCombatInput::default()
            },
        )
    }

    fn wall() -> Clip {
        // A wall at x in [500, 520].
        let (min, max) = ([500.0, -1000.0, -1000.0], [520.0, 1000.0, 1000.0]);
        let mut c = Clip::default();
        c.push_brush(
            &[
                [1.0, 0.0, 0.0, max[0]],
                [-1.0, 0.0, 0.0, -min[0]],
                [0.0, 1.0, 0.0, max[1]],
                [0.0, -1.0, 0.0, -min[1]],
                [0.0, 0.0, 1.0, max[2]],
                [0.0, 0.0, -1.0, -min[2]],
            ],
            &[0; 6],
            1,
            0,
        );
        c
    }

    /// The M4A1 from a locally converted `out/packs/common.wpk` (never in git; skipped when absent).
    #[test]
    fn real_m4_fires_at_its_authored_rate() {
        extern crate std;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/common.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local common pack, skipped");
            return;
        };
        let pack = wpk::Pack::parse(&bytes, true).unwrap();
        let weap = pack.section(wpk::tag::WEAPONS).unwrap().data;
        let rec = wpk::weapon::find(weap, "m4_mp").expect("m4_mp in common.wpk");
        let c = rec.combat;
        std::println!(
            "m4_mp: fire {} ms, clip {}, start ammo {}, reload {} / {} ms, damage {}-{}, raise {} ms",
            c.fire_time_ms,
            c.clip_size,
            c.start_ammo,
            c.reload_time_ms,
            c.reload_empty_time_ms,
            c.min_damage,
            c.damage,
            c.first_raise_time_ms
        );
        let w = wall();
        let clip = MapClip::new(&w);
        let mut g = Gun::new(&rec).expect("usable");
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        for _ in 0..200 {
            g.update(
                &clip,
                16,
                &GunInput::default(),
                [0.0; 3],
                [1.0, 0.0, 0.0],
                Y,
                Z,
            );
        }
        let mut shots = 0;
        for _ in 0..125 {
            shots += usize::from(
                g.update(&clip, 16, &fire, [0.0; 3], [1.0, 0.0, 0.0], Y, Z)
                    .1
                    .is_some(),
            );
        }
        // IW engines restart the weapon timer each shot (no carry-over), so the interval rounds up to whole frames:
        // 70 ms at 16 ms frames fires every 80 ms (frame-rate dependent, like the PC and 360 games).
        let frames_per_shot = (c.fire_time_ms.max(1) as usize).div_ceil(16);
        let expected = 125 / frames_per_shot;
        std::println!("{shots} shots in 2 s ({expected} expected at 16 ms frames)");
        assert!(shots.abs_diff(expected.min(c.clip_size as usize)) <= 1);
    }

    #[test]
    fn spread_widens_while_firing_and_bullets_stay_in_the_cone() {
        let c = wall();
        let clip = MapClip::new(&c);
        let mut rec = rifle();
        rec.combat.hip_spread_stand_min = 2.0;
        rec.combat.hip_spread_stand_max = 6.0;
        rec.combat.hip_spread_decay_rate = 4.0;
        rec.combat.hip_spread_fire_add = 0.6;
        let mut g = Gun::new(&rec).expect("usable");
        let mut ps = PlayerState::ZERO;
        ps.view_height_current = 60.0;
        ps.ground_entity_num = 0x7fe;
        let input = SpreadInput {
            ps: &ps,
            cmd_angles: [0; 3],
            old_cmd_angles: [0; 3],
            forwardmove: 0,
            rightmove: 0,
        };
        let (eye, x) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0]);
        for _ in 0..40 {
            g.update_spread(16, &input);
            g.update(&clip, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let rest = g.spread_degrees;
        assert!(
            (rest - 2.0).abs() < 0.01,
            "standing still the cone is the minimum: {rest}"
        );
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let mut widest = 0.0f32;
        for _ in 0..60 {
            g.update_spread(16, &input);
            widest = widest.max(g.spread_degrees);
            if let (_, Some(s)) = g.update(&clip, 16, &fire, eye, x, Y, Z) {
                // Off-axis angle of the hit, at 500 units.
                let off = libm::sqrtf(s.end[1] * s.end[1] + (s.end[2] - 60.0) * (s.end[2] - 60.0));
                let deg = libm::atanf(off / (s.end[0] - eye[0])).to_degrees();
                assert!(deg <= 6.0 + 0.01, "hit {deg} degrees off, cone at most 6");
            }
        }
        assert!(widest > 4.0, "firing widens the cone ({widest})");
        for _ in 0..120 {
            g.update_spread(16, &input);
        }
        assert!(g.spread_degrees < widest, "and it recovers");
    }

    #[test]
    fn raises_fires_at_its_rate_hits_the_wall_and_reloads() {
        let c = wall();
        let clip = MapClip::new(&c);
        let mut g = Gun::new(&rifle()).expect("usable");
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let eye = [0.0, 0.0, 60.0];
        let x = [1.0, 0.0, 0.0];
        // Raising: no shots for the first 500 ms.
        for _ in 0..30 {
            let (_, s) = g.update(&clip, 16, &GunInput::default(), eye, x, Y, Z);
            assert!(s.is_none());
        }
        // One second of fire at 100 ms per round: about 10 shots, all into the wall at x = 500.
        let mut hits = 0;
        for _ in 0..63 {
            if let (_, Some(s)) = g.update(&clip, 16, &fire, eye, x, Y, Z) {
                assert!(s.hit && (s.end[0] - 500.0).abs() < 1.0, "{s:?}");
                hits += 1;
            }
        }
        assert!((9..=11).contains(&hits), "{hits} shots in a second");
        assert_eq!(g.ammo().0, 30 - hits);
        // Reload: the magazine refills from the reserve after the reload time.
        let (clip_before, stock_before) = g.ammo();
        g.update(
            &clip,
            16,
            &GunInput {
                reload: true,
                ..GunInput::default()
            },
            eye,
            x,
            Y,
            Z,
        );
        for _ in 0..200 {
            g.update(&clip, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let (clip_after, stock_after) = g.ammo();
        assert_eq!(clip_after, 30);
        assert_eq!(stock_after, stock_before - (30 - clip_before));
    }
}

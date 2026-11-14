//! The player's gun: IW4L's weapon state machine (`weapon_iw4::weapon_ordinary`: raise, fire rate, bursts, reload,
//! rechamber) driven by the pad, with combat numbers from `common.wpk`, and bullets traced against the map's
//! collision (IW4 `MASK_SHOT`).
//!
//! Aim spread is IW4L's (decay, move / turn / fire increases, stance and ADS cones); a bullet leaves in a random
//! direction inside the cone. Not yet: recoil / view kick, penetration, damage to players, sounds. Timings are IW4L's and therefore the PC game's.

use playerstate_iw4::PlayerState;
use playerstate_iw4::buttons;
use trace_iw4::Trace;
pub use weapon_iw4::WeaponTickEvent;
use weapon_iw4::{
    AimSpreadMotion, AimSpreadState, SpreadOverrideState, WeaponCmd, WeaponCombatFacts,
    WeaponHandState, add_aim_spread_fire, adjust_aim_spread_scale, fire_weapon_spread_degrees,
    get_spread_for_weapon, spawn_weapon_hand, weapon_hands,
};
use weapon_iw4::{
    CAMERA_LIMIT_DEGREES, CAMERA_PEAK_SECONDS, FireRecoilPsScales, GunKickRange,
    GunRecoilPlacementState, GunRecoilResponse, MODEL_PEAK_SECONDS, RecoilAxis, ViewKickRange,
    calculate_weapon_position_gun_recoil, fire_recoil_gun_range, fire_recoil_view_range,
    recoil_ads_weight, weapon_fire_recoil,
};
use wpk::weapon::{AimRecord, KickRecord, MoveRecord, WeaponRecord};

use crate::player::WeaponMove;

use crate::collision::MapClip;

/// Most pellets one shot traces.
const MAX_PELLETS: usize = 16;

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
    /// The melee button (IW4 `BUTTON_MELEE`).
    pub melee: bool,
    /// The player's `pm_flags` (sprinting blocks firing, as in IW4).
    pub pm_flags: u32,
    /// The weapon the player wants out instead of this one (IW4 `usercmd.weapon`): this one drops, then that one
    /// raises ([`Gun::take_switch`]).
    pub switch_to: Option<SwitchTarget>,
}

/// The weapon being switched to, with the raise times IW4L's weapon change reads from it (`WeaponCmd`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SwitchTarget {
    pub weapon: u32,
    pub raise_ms: i32,
    pub quick_raise_ms: i32,
    /// 0 when the weapon was out before this life (no first raise).
    pub first_raise_ms: i32,
    /// A pistol comes out quick (IW4 `weapClass` 5).
    pub pistol: bool,
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
    /// The weapon's index in the weapon list (IW4 weapon number).
    index: u32,
    facts: WeaponCombatFacts,
    hand: WeaponHandState,
    server_time: i32,
    old_buttons: u32,
    /// Shots fired since the gun was raised (for the HUD / tests).
    pub shots: u32,
    /// The last step started a melee swing (IW4L `melee_started`: the swipe sound plays now, the hit lands at
    /// `MeleeFired`).
    pub swung: bool,
    /// IW4 aim spread scale (0..255) and override state.
    spread: AimSpreadState,
    /// Spread cone of the last frame, degrees (for the crosshair).
    pub spread_degrees: f32,
    rng: u32,
    /// ADS fraction of the last frame.
    ads_frac: f32,
    /// Recoil numbers, when the pack has them.
    kick: Option<KickRecord>,
    /// ADS zoom and aim-assist ranges (`WAIM`; none when the pack has none).
    aim: AimRecord,
    /// Pellets after the first of the last shot (shotguns).
    extra_pellets: alloc::vec::Vec<Shot>,
    /// Movement scales (`WMOV`; MW2's usual numbers when the pack has none).
    moves: MoveRecord,
    /// `pm_flags` after the last weapon step (the weapon clears and sets some, IW4L `advance_weapon_command`).
    pm_flags_out: Option<u32>,
    /// Camera kick springs (pitch, yaw, roll) and their angles in degrees; gun (viewmodel) kick.
    camera: [RecoilAxis; 3],
    pub kick_angles: [f32; 3],
    gun_recoil: GunRecoilPlacementState,
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
            index,
            hand: spawn_weapon_hand(index, &facts, true),
            facts,
            server_time: 1000,
            old_buttons: 0,
            shots: 0,
            swung: false,
            spread: AimSpreadState {
                aim_spread_scale: 0.0,
                spread_override: 0,
                spread_override_state: SpreadOverrideState::None as i32,
            },
            spread_degrees: 0.0,
            rng: 0x1234_5678 ^ index,
            ads_frac: 0.0,
            kick: None,
            moves: MoveRecord::default(),
            aim: AimRecord::default(),
            extra_pellets: alloc::vec::Vec::with_capacity(MAX_PELLETS),
            pm_flags_out: None,
            camera: Default::default(),
            kick_angles: [0.0; 3],
            gun_recoil: GunRecoilPlacementState::default(),
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

    /// Raise the gun again (switching to it), keeping its ammunition: IW4L's spawn hand with the ordinary raise.
    pub fn raise(&mut self) {
        let (clip, stock) = (self.hand.clip, self.hand.stock);
        self.hand = spawn_weapon_hand(self.hand.weapon, &self.facts, false);
        self.hand.clip = clip;
        self.hand.stock = stock;
    }

    /// This weapon as a switch target (raised for the first time this life when `first`).
    pub fn as_target(&self, first: bool) -> SwitchTarget {
        SwitchTarget {
            weapon: self.index,
            raise_ms: self.facts.raise_time_ms,
            quick_raise_ms: self.facts.quick_raise_time_ms,
            first_raise_ms: if first {
                self.facts.first_raise_time_ms.max(0)
            } else {
                0
            },
            pistol: self.facts.weap_class == 5,
        }
    }

    /// After a step that finished dropping this weapon for another (IW4L's change sets the hand to the new
    /// weapon, raising): the hand, for the new weapon's gun to take ([`Gun::adopt`]). This gun keeps its ammunition
    /// and is put away ready.
    pub fn take_switch(&mut self) -> Option<WeaponHandState> {
        if self.hand.weapon == self.index {
            return None;
        }
        let out = self.hand;
        let (clip, stock) = (self.hand.clip, self.hand.stock);
        self.hand = spawn_weapon_hand(self.index, &self.facts, false);
        self.hand.weaponstate = weapon_iw4::WeaponState::Ready as i32;
        self.hand.weapon_time = 0;
        self.hand.clip = clip;
        self.hand.stock = stock;
        self.kick_angles = [0.0; 3];
        Some(out)
    }

    /// Take over the hand of the weapon put away for this one: its state (raising) and timers, with this weapon's
    /// own ammunition.
    pub fn adopt(&mut self, from: &WeaponHandState) {
        let (clip, stock) = (self.hand.clip, self.hand.stock);
        self.hand = *from;
        self.hand.weapon = self.index;
        self.hand.clip = clip;
        self.hand.stock = stock;
        self.shots = 0;
    }

    /// Whether a switch would start now (IW4L `check_for_change_admits`: not mid-melee, reload interrupts, not
    /// while a shot is still cycling).
    pub fn can_switch(&self) -> bool {
        weapon_iw4::check_for_change_admits(
            self.hand.weaponstate,
            self.hand.weapon_time,
            self.hand.weapon_delay,
        )
    }

    /// The weapon's combat numbers (damage, ranges, location multipliers ...).
    pub fn facts(&self) -> &WeaponCombatFacts {
        &self.facts
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

    /// Recoil numbers for this weapon (`KICK` record); without them the gun has no recoil.
    pub fn set_kick(&mut self, kick: Option<KickRecord>) {
        self.kick = kick;
    }

    /// The pellets after the first of this frame's shot (shotguns; empty otherwise).
    pub fn extra_pellets(&self) -> &[Shot] {
        &self.extra_pellets
    }

    /// ADS zoom and aim-assist ranges for this weapon (`WAIM` record).
    pub fn set_aim(&mut self, a: Option<AimRecord>) {
        self.aim = a.unwrap_or_default();
    }

    pub fn aim(&self) -> &AimRecord {
        &self.aim
    }

    /// Aim-assist ranges (IW4L `AimAssistRanges`: auto-aim, hip, ADS).
    pub fn aim_ranges(&self) -> weapon_iw4::AimAssistRanges {
        weapon_iw4::AimAssistRanges {
            auto_aim: self.aim.auto_aim_range,
            hip: self.aim.aim_assist_range,
            ads: self.aim.aim_assist_range_ads,
        }
    }

    /// Movement scales for this weapon (`WMOV` record).
    pub fn set_moves(&mut self, m: Option<MoveRecord>) {
        self.moves = m.unwrap_or_default();
    }

    /// What this weapon does to movement right now (IW4L `sim::step` builds the same from the weapon's facts). ADS is
    /// allowed when the weapon aims down sights, and not on an empty magazine for weapons that forbid it.
    pub fn move_context(&self) -> WeaponMove {
        let f = &self.facts;
        let (ads_in_rate, ads_out_rate, rechamber_while_ads, ads_fire_only) = f.ads_frac_context();
        WeaponMove {
            move_speed_scale: self.moves.move_speed_scale,
            ads_move_speed_scale: self.moves.ads_move_speed_scale,
            sprint_duration_scale: self.moves.sprint_duration_scale,
            ads_allowed: f.aim_down_sight && !(f.no_ads_when_mag_empty && self.hand.clip <= 0),
            aim_down_sight: f.aim_down_sight,
            ads_in_rate,
            ads_out_rate,
            ads_reload_trans_time_ms: f.ads_reload_trans_time_ms,
            segmented_reload: f.segmented_reload,
            rechamber_while_ads,
            ads_fire_only,
            melee_delay_ms: f.melee_delay_ms,
            melee_charge_delay_ms: f.melee_charge_delay_ms,
            scope: f.overlay_reticle != 0,
            can_hold_breath: f.can_hold_breath,
        }
    }

    /// Copy the weapon's state into the player's (IW4L `advance_weapon_command`): pmove reads the weapon, its state
    /// and the `pm_flags` the weapon step changed (sprint, ADS, reload blocks).
    pub fn sync_player(&self, ps: &mut playerstate_iw4::PlayerState) {
        ps.weapon = self.hand.weapon;
        ps.weaponstate_primary = self.hand.weaponstate;
        ps.weapon_time = self.hand.weapon_time;
        ps.weapon_delay = self.hand.weapon_delay;
        ps.weap_anim = self.hand.weap_anim;
        ps.weapon_restrict_kick_time = self.hand.weapon_restrict_kick_time;
        if let Some(f) = self.pm_flags_out {
            ps.pm_flags = f;
        }
    }

    /// Viewmodel kick, degrees (pitch, yaw).
    pub fn gun_kick(&self) -> [f32; 2] {
        self.gun_recoil.angles
    }

    /// A shot's recoil impulse (IW4L `ViewKickState::seed_fire_scaled`).
    fn seed_recoil(&mut self) {
        let Some(k) = self.kick else {
            return;
        };
        let frac = self.ads_frac;
        let four = |v: [f32; 4]| (v[0], v[1], v[2], v[3]);
        let (a, b, c, d) = four(k.hip_view);
        let (e, f, g, h) = four(k.ads_view);
        let view = fire_recoil_view_range(
            frac,
            ViewKickRange {
                pitch_min: a,
                pitch_max: b,
                yaw_min: c,
                yaw_max: d,
            },
            ViewKickRange {
                pitch_min: e,
                pitch_max: f,
                yaw_min: g,
                yaw_max: h,
            },
        );
        let (a, b, c, d) = four(k.hip_gun);
        let (e, f, g, h) = four(k.ads_gun);
        let gun = fire_recoil_gun_range(
            frac,
            GunKickRange {
                pitch_min: a,
                pitch_max: b,
                yaw_min: c,
                yaw_max: d,
            },
            GunKickRange {
                pitch_min: e,
                pitch_max: f,
                yaw_min: g,
                yaw_max: h,
            },
        );
        let reduce = self.hand.weapon_restrict_kick_time > 0;
        let t = recoil_ads_weight(frac);
        let ps = FireRecoilPsScales {
            reduce_window_active: reduce,
            reduced_percent: if reduce {
                k.hip_reduced_percent * (1.0 - t) + k.ads_reduced_percent * t
            } else {
                0.0
            },
            weap_flags: 0,
            recoil_scale: 0,
        };
        let samples = [self.random(), self.random(), self.random(), self.random()];
        let impulse = weapon_fire_recoil(view, gun, ps, samples);
        for (axis, amplitude) in self.camera.iter_mut().zip(impulse.camera_peak) {
            axis.add_peak(f64::from(amplitude), CAMERA_PEAK_SECONDS);
        }
        for (axis, amplitude) in self.gun_recoil.axes.iter_mut().zip(impulse.model_peak) {
            axis.add_peak(f64::from(amplitude), MODEL_PEAK_SECONDS);
        }
    }

    /// Let the recoil springs settle for `dt` seconds (IW4L `ViewKickState::advance`).
    fn advance_recoil(&mut self, dt: f32) {
        let Some(k) = self.kick else {
            return;
        };
        if dt <= 0.0 {
            return;
        }
        for (axis, out) in self.camera.iter_mut().zip(&mut self.kick_angles) {
            axis.advance(f64::from(dt), CAMERA_PEAK_SECONDS);
            *out = axis.bounded(CAMERA_LIMIT_DEGREES) as f32;
        }
        calculate_weapon_position_gun_recoil(
            &mut self.gun_recoil,
            dt,
            self.ads_frac,
            true,
            GunRecoilResponse::default(),
            GunRecoilResponse::default(),
            [k.gun_max_pitch, k.gun_max_yaw],
        );
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
        if input.melee {
            btn |= weapon_iw4::BUTTON_MELEE;
        }
        self.server_time += msec;
        let target = input.switch_to.unwrap_or(SwitchTarget {
            weapon: self.hand.weapon,
            ..SwitchTarget::default()
        });
        let mut cmd = WeaponCmd {
            msec,
            server_time: self.server_time,
            buttons: btn,
            old_buttons: self.old_buttons,
            cmd_weapon: target.weapon as u16,
            cmd_weapon_owned: true,
            cmd_weapon_pistol_quick: target.pistol,
            switch_raise_time_ms: target.raise_ms,
            switch_quick_raise_time_ms: target.quick_raise_ms,
            switch_first_raise_time_ms: target.first_raise_ms,
            pm_flags: input.pm_flags,
            ..WeaponCmd::default()
        };
        self.old_buttons = btn;
        self.advance_recoil(msec as f32 / 1000.0);
        // IW4L's hand driver (as `advance_weapon_command`): stun, melee start, then the hand's own step.
        let event = weapon_hands(
            core::slice::from_mut(&mut self.hand),
            &self.facts,
            &mut cmd,
            0,
        )[0]
        .map(|(_, e)| e);
        self.swung = cmd.melee_started.is_some();
        self.pm_flags_out = Some(cmd.pm_flags);
        self.extra_pellets.clear();
        let shot = match event {
            Some(WeaponTickEvent::ShotAccepted { ammo_used }) => {
                self.shots += 1;
                // Each pellet in a random direction in the cone (angle and radius uniform, so hits gather towards the
                // centre); shotguns fire several (IW4L `pellet_count`, times the barrels for double-barrel fire), then
                // the shot widens the spread (IW4L add_aim_spread_fire).
                let barrels = if self.facts.fire_type == 5 {
                    ammo_used.max(1)
                } else {
                    1
                };
                let pellets = (self.facts.pellet_count() * barrels).clamp(1, MAX_PELLETS as i32);
                let t = libm::tanf(self.spread_degrees.to_radians());
                let mut first = None;
                for _ in 0..pellets {
                    let a = self.random() * core::f32::consts::TAU;
                    let r = self.random() * t;
                    let (x, y) = (libm::cosf(a) * r, libm::sinf(a) * r);
                    let d: [f32; 3] = core::array::from_fn(|k| dir[k] + right[k] * x + up[k] * y);
                    let len = libm::sqrtf(d.iter().map(|v| v * v).sum::<f32>()).max(1e-6);
                    let s = self.trace_bullet(clip, eye, d.map(|v| v / len));
                    if first.is_none() {
                        first = Some(s);
                    } else {
                        self.extra_pellets.push(s);
                    }
                }
                let shot = first.expect("at least one pellet");
                self.seed_recoil();
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

    /// IW4L's weapon change through the gun: the rifle drops (its drop time), hands over, the pistol raises (its
    /// raise time), and only then fires.
    #[test]
    fn switching_drops_then_raises_the_other_gun() {
        let clip = Clip::default();
        let mc = MapClip::new(&clip);
        let mut pistol_rec = rifle();
        pistol_rec.combat.raise_time_ms = 400;
        pistol_rec.combat.quick_raise_time_ms = 200;
        pistol_rec.combat.first_raise_time_ms = 700;
        pistol_rec.combat.weap_class = 0;
        let mut r = Gun::with_index(&rifle(), 3).unwrap();
        let mut p = Gun::with_index(&pistol_rec, 4).unwrap();
        let eye = [0.0, 0.0, 60.0];
        let x = [1.0, 0.0, 0.0];
        // Let the rifle come up.
        for _ in 0..60 {
            r.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        assert!(r.can_switch());
        let want = GunInput {
            switch_to: Some(p.as_target(true)),
            ..GunInput::default()
        };
        let mut t = 0;
        let hand = loop {
            r.update(&mc, 16, &want, eye, x, Y, Z);
            t += 16;
            if let Some(h) = r.take_switch() {
                break h;
            }
            assert!(t < 2000, "the drop never finished");
        };
        assert!(
            (280..=340).contains(&t),
            "dropped in {t} ms (drop time 300)"
        );
        p.adopt(&hand);
        assert_eq!(p.weapon_index(), 4);
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let mut first_shot = None;
        for k in 0..100 {
            if let (_, Some(_)) = p.update(&mc, 16, &fire, eye, x, Y, Z) {
                first_shot = Some((k + 1) * 16);
                break;
            }
        }
        let first = first_shot.expect("the pistol fired after raising");
        assert!(
            (680..=740).contains(&first),
            "first raise 700 ms, fired at {first}"
        );
        assert_eq!(r.ammo().0, 30, "the rifle kept its magazine");
    }

    /// The melee button swings (IW4L `weapon_try_melee`): `MeleeFired` after the weapon's melee delay.
    #[test]
    fn melee_fires_after_its_delay() {
        let clip = Clip::default();
        let mc = MapClip::new(&clip);
        let mut rec = rifle();
        rec.combat.melee_damage = 135;
        rec.combat.melee_time_ms = 600;
        rec.combat.melee_delay_ms = 150;
        let mut g = Gun::with_index(&rec, 2).unwrap();
        let eye = [0.0, 0.0, 60.0];
        let x = [1.0, 0.0, 0.0];
        for _ in 0..60 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let swing = GunInput {
            melee: true,
            ..GunInput::default()
        };
        let mut fired = None;
        for k in 0..60 {
            let (e, shot) = g.update(&mc, 16, &swing, eye, x, Y, Z);
            assert!(shot.is_none(), "a swing is not a shot");
            if e == Some(WeaponTickEvent::MeleeFired) {
                fired = Some((k + 1) * 16);
                break;
            }
        }
        let at = fired.expect("the swing landed");
        assert!(
            (140..=200).contains(&at),
            "melee delay 150 ms, fired at {at}"
        );
        assert!(!g.can_switch(), "no switching mid-swing");
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
        // MW2 melee: 135 damage (kills from full health) with its swing times.
        assert!(
            c.melee_damage >= 100 && c.melee_time_ms > 0,
            "melee {} / {} ms",
            c.melee_damage,
            c.melee_time_ms
        );
        // Its aim and movement records (when the pack has them).
        if let (Some(aims), Some((i, _))) = (
            pack.section(wpk::tag::WEAPON_AIM),
            wpk::weapon::find_indexed(weap, "m4_mp"),
        ) {
            let a = wpk::records::get::<wpk::weapon::AimRecord>(aims.data, i as usize).unwrap();
            std::println!(
                "m4_mp aim: zoom fov {}, zoom in/out {} / {}, assist {} / {} (auto {})",
                a.ads_zoom_fov,
                a.ads_zoom_in_frac,
                a.ads_zoom_out_frac,
                a.aim_assist_range,
                a.aim_assist_range_ads,
                a.auto_aim_range
            );
            assert!(a.ads_zoom_fov > 10.0 && a.ads_zoom_fov < 65.0);
        }
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
    fn shotguns_trace_every_pellet() {
        let c = wall();
        let clip = MapClip::new(&c);
        let mut rec = rifle();
        rec.combat.weap_class = weapon_iw4::WEAPCLASS_SPREAD;
        rec.combat.shots_per_fire = 6;
        let mut g = Gun::new(&rec).expect("usable");
        let (eye, x) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0]);
        for _ in 0..40 {
            g.update(&clip, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let mut pellets = 0;
        for _ in 0..10 {
            if let (_, Some(_)) = g.update(&clip, 16, &fire, eye, x, Y, Z) {
                pellets = 1 + g.extra_pellets().len();
                break;
            }
        }
        assert_eq!(pellets, 6);
    }

    #[test]
    fn recoil_kicks_the_view_up_and_settles() {
        let c = wall();
        let clip = MapClip::new(&c);
        let mut g = Gun::new(&rifle()).expect("usable");
        g.set_kick(Some(KickRecord {
            hip_view: [20.0, 30.0, -5.0, 5.0],
            hip_gun: [5.0, 8.0, -2.0, 2.0],
            gun_max_pitch: 6.0,
            gun_max_yaw: 6.0,
            ..KickRecord::default()
        }));
        let (eye, x) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0]);
        for _ in 0..40 {
            g.update(&clip, 16, &GunInput::default(), eye, x, Y, Z);
        }
        assert_eq!(g.kick_angles, [0.0; 3]);
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let mut peak = 0.0f32;
        for _ in 0..30 {
            g.update(&clip, 16, &fire, eye, x, Y, Z);
            peak = peak.min(g.kick_angles[0]);
        }
        // IW4 pitch up is negative; bounded by the 8 degree camera limit.
        assert!((-8.0..-0.1).contains(&peak), "pitch kick {peak}");
        assert!(g.gun_kick()[0].abs() > 0.0);
        for _ in 0..120 {
            g.update(&clip, 16, &GunInput::default(), eye, x, Y, Z);
        }
        assert!(
            g.kick_angles[0].abs() < 0.05,
            "settles: {:?}",
            g.kick_angles
        );
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

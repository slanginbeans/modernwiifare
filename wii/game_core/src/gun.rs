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
    /// The first surface's IW4 surface flags (its surface type in bits 20..25).
    pub surface: u32,
    /// Where it finally stopped after going through walls (`end` when it went through none).
    pub pen_end: [f32; 3],
    /// Past each wall: the distance from `start` where the bullet came out and its damage scale after it (IW4L
    /// `fire_penetrate`'s segment `damage_mult`).
    pub layers: [(f32, f32); MAX_LAYERS],
    pub layer_count: u8,
}

/// A projectile the gun fired this frame (IW4L `fire_accepted_shot` for `FireWeaponKind::Missile` /
/// `GrenadeLauncher`): the weapon, where it leaves (the eye), its direction inside the spread cone, and the view's
/// right (the Javelin leaves a little to the right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launch {
    pub weapon: u32,
    pub origin: [f32; 3],
    pub dir: [f32; 3],
    pub right: [f32; 3],
}

/// IW4L `MAX_PENETRATE_STEPS`.
pub const MAX_LAYERS: usize = 5;

impl Shot {
    /// The damage scale at `dist` along the shot (1 before any wall).
    pub fn scale_at(&self, dist: f32) -> f32 {
        self.layers[..usize::from(self.layer_count)]
            .iter()
            .rev()
            .find(|(d, _)| *d <= dist)
            .map_or(1.0, |(_, m)| *m)
    }
}

/// A weapon's bullet penetration (IW4 `penetrateType` 0 none, 1 small, 2 medium, 3 large; `penetrateMultiplier`)
/// and the depth table by type and surface (`info/bullet_penetration_mp`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Penetration {
    pub kind: i32,
    pub multiplier: f32,
    pub table: weapon_iw4::PenetrationDepthTable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunInput {
    pub fire: bool,
    pub reload: bool,
    /// The melee button (IW4 `BUTTON_MELEE`).
    pub melee: bool,
    /// The lethal and tactical grenade buttons (IW4 `BUTTON_FRAG`, `BUTTON_SMOKE`).
    pub frag: bool,
    pub smoke: bool,
    /// The player's `pm_flags` (sprinting blocks firing, as in IW4).
    pub pm_flags: u32,
    /// The player's perk bits (`ps.perks`: Sleight of Hand reloads, fast sprint recovery ...).
    pub perks: [u32; 2],
    /// The weapon the player wants out instead of this one (IW4 `usercmd.weapon`): this one drops, then that one
    /// raises ([`Gun::take_switch`]).
    pub switch_to: Option<SwitchTarget>,
    /// The melee lunge's target this command (IW4 `usercmd.meleeChargeYaw` / `meleeChargeDist`; from
    /// [`crate::arena::Arena::melee_charge_target`]).
    pub melee_charge: Option<(f32, u8)>,
    /// The player's charge state (`ps.meleeChargeYaw` / `Dist` / `Time`), its type and whether it is in the air.
    pub charge_state: (f32, i32, i32),
    pub pm_type: i32,
    pub in_air: bool,
    /// The aim button held (an akimbo pair fires its left gun with it).
    pub ads: bool,
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
    /// To or from the weapon's alternate (the underbarrel launcher or shotgun: IW4L `alternate_switch`, the
    /// `altDropTime` / `altRaiseTime` pair instead of the usual drop and raise).
    pub alternate: bool,
    /// The raise time of an alternate switch (the incoming weapon's `altRaiseTime`).
    pub alternate_raise_ms: i32,
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
    /// The grenades carried and the offhand state between steps (IW4L `OffhandCmd`, kept as the sim keeps it in the
    /// player state).
    offhand: weapon_iw4::OffhandCmd,
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
    /// The stance's spread cone (min, max) of the last frame, degrees (MW2's crosshair, `hud_fx::Crosshair`).
    spread_cone: (f32, f32),
    rng: u32,
    /// ADS fraction of the last frame.
    ads_frac: f32,
    /// Recoil numbers, when the pack has them.
    kick: Option<KickRecord>,
    /// Bullet penetration (None: bullets stop at the first wall).
    penetration: Option<Penetration>,
    /// ADS zoom and aim-assist ranges (`WAIM`; none when the pack has none).
    aim: AimRecord,
    /// Pellets after the first of the last shot (shotguns).
    extra_pellets: alloc::vec::Vec<Shot>,
    /// Movement scales (`WMOV`; MW2's usual numbers when the pack has none).
    moves: MoveRecord,
    /// `pm_flags` after the last weapon step (the weapon clears and sets some, IW4L `advance_weapon_command`).
    pm_flags_out: Option<u32>,
    /// The charge state the weapon step left (copied to the player state).
    charge_out: Option<weapon_iw4::MeleeChargeState>,
    /// Camera kick springs (pitch, yaw, roll) and their angles in degrees; gun (viewmodel) kick.
    camera: [RecoilAxis; 3],
    pub kick_angles: [f32; 3],
    gun_recoil: GunRecoilPlacementState,
    /// Multiplies each shot's camera and gun kick (1: IW4L's amplitudes; the debug menu's comparison sets 2).
    pub kick_scale: f32,
    /// The projectile fired by the last step (launchers; [`Gun::take_launch`]).
    launch: Option<Launch>,
    /// The left hand of an akimbo pair (IW4 hand 1; `hand` is the right, hand 0).
    left: Option<WeaponHandState>,
    /// The hands the last step fired (right, left).
    fired: [bool; 2],
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
        // Akimbo: both hands raised, each with a magazine, the rest shared (IW4L `spawn_clip_stock` for two hands).
        let mut hand = spawn_weapon_hand(index, &facts, true);
        let left = facts.dual_wield.then(|| {
            let (c0, c1, stock) = weapon_iw4::spawn_clip_stock(&facts, 1);
            hand.clip = c0;
            hand.stock = stock;
            WeaponHandState {
                clip: c1,
                hand_index: 1,
                ..hand
            }
        });
        Some(Gun {
            name: rec.name,
            index,
            offhand: weapon_iw4::OffhandCmd::default(),
            hand,
            left,
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
            spread_cone: (0.0, 0.0),
            rng: 0x1234_5678 ^ index,
            ads_frac: 0.0,
            kick: None,
            penetration: None,
            moves: MoveRecord::default(),
            aim: AimRecord::default(),
            extra_pellets: alloc::vec::Vec::with_capacity(MAX_PELLETS),
            pm_flags_out: None,
            charge_out: None,
            camera: Default::default(),
            kick_angles: [0.0; 3],
            gun_recoil: GunRecoilPlacementState::default(),
            kick_scale: 1.0,
            launch: None,
            fired: [false; 2],
        })
    }

    /// Whether this weapon fires projectiles instead of bullets (IW4 `WEAPTYPE_PROJECTILE`: rockets and grenade
    /// launchers; IW4L `fire_weapon_kind`).
    pub fn launches(&self) -> bool {
        self.facts.weap_type == weapon_iw4::WEAPTYPE_PROJECTILE
    }

    /// The projectile the last step fired, once (launchers: the step returns no [`Shot`]).
    pub fn take_launch(&mut self) -> Option<Launch> {
        self.launch.take()
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
        self.match_left();
    }

    /// The left hand follows the right one's state (raise, switch), keeping its own magazine.
    fn match_left(&mut self) {
        if let Some(l) = self.left.as_mut() {
            *l = WeaponHandState {
                clip: l.clip,
                hand_index: 1,
                ..self.hand
            };
        }
    }

    /// The grenades this gun's hand can throw: `rows` (weapon, class, ammo, timings), the class the lethal button
    /// throws and the one the tactical button throws (IW4 `offhandPrimary` / `offhandSecondary`).
    pub fn set_offhands(&mut self, rows: &[weapon_iw4::OffhandInvRow], lethal: i32, tactical: i32) {
        let mut inv = [weapon_iw4::OffhandInvRow::default(); weapon_iw4::OFFHAND_INV_SLOTS];
        for (slot, r) in inv.iter_mut().zip(rows) {
            *slot = *r;
        }
        self.offhand.inventory = inv;
        self.offhand.offhand_primary = lethal;
        self.offhand.offhand_secondary = tactical;
    }

    /// The grenades carried (weapon, ammo).
    pub fn offhand_ammo(&self) -> impl Iterator<Item = (u32, i32)> + '_ {
        self.offhand
            .inventory
            .iter()
            .filter(|r| r.weapon != 0)
            .map(|r| (r.weapon, r.ammo))
    }

    /// Pick one of thrown weapon `weapon` back up (a throwing knife): true when it is carried and not full (IW4L
    /// `projectile_pickup_ammo`: one more, up to a clip of one; start ammo here, MW2's knife carries one).
    pub fn pick_up_offhand(&mut self, weapon: u32) -> bool {
        let Some(r) = self
            .offhand
            .inventory
            .iter_mut()
            .find(|r| r.weapon == weapon && r.weapon != 0)
        else {
            return false;
        };
        if r.ammo >= 1 {
            return false;
        }
        r.ammo += 1;
        true
    }

    /// Whether weapon `weapon` is carried as a grenade with room for one more.
    pub fn can_pick_up_offhand(&self, weapon: u32) -> bool {
        self.offhand
            .inventory
            .iter()
            .any(|r| r.weapon == weapon && r.weapon != 0 && r.ammo < 1)
    }

    /// Hand the grenades over to the gun switched to (they belong to the player, not the gun).
    pub fn take_offhands(&mut self, from: &Gun) {
        self.offhand.inventory = from.offhand.inventory;
        self.offhand.offhand_primary = from.offhand.offhand_primary;
        self.offhand.offhand_secondary = from.offhand.offhand_secondary;
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
            alternate: false,
            alternate_raise_ms: self.facts.alternate_raise_time_ms,
        }
    }

    /// This weapon as the target of an alternate switch (from or to its underbarrel weapon).
    pub fn as_alternate_target(&self) -> SwitchTarget {
        SwitchTarget {
            alternate: true,
            ..self.as_target(false)
        }
    }

    /// The weapon's alternate (`altWeapon`) as its index in the weapon list (0: none).
    pub fn alternate_weapon(&self) -> u32 {
        self.facts.alternate_weapon
    }

    /// Whether this is an akimbo pair (two hands, IW4L `weapon_hands` with `last_hand` 1).
    pub fn akimbo(&self) -> bool {
        self.left.is_some()
    }

    /// The left hand's rounds in its magazine (akimbo).
    pub fn left_clip(&self) -> Option<i32> {
        self.left.map(|h| h.clip)
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
        self.match_left();
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
        self.hand.hand_index = 0;
        self.match_left();
        self.shots = 0;
    }

    /// Whether a switch would start now (IW4L `check_for_change_admits`: not mid-melee, reload interrupts, not
    /// while a shot is still cycling).
    pub fn can_switch(&self) -> bool {
        [Some(self.hand), self.left].iter().flatten().all(|h| {
            weapon_iw4::check_for_change_admits(h.weaponstate, h.weapon_time, h.weapon_delay)
        })
    }

    /// The weapon's combat numbers (damage, ranges, location multipliers ...).
    pub fn facts(&self) -> &WeaponCombatFacts {
        &self.facts
    }

    /// Fill the magazine, the reserve and the grenades (the debug menu's infinite ammo).
    pub fn refill_ammo(&mut self) {
        self.hand.clip = self.facts.clip_size.max(self.hand.clip);
        self.hand.stock = self.facts.max_ammo.max(self.hand.stock);
        if let Some(l) = self.left.as_mut() {
            l.clip = self.facts.clip_size.max(l.clip);
            l.stock = self.hand.stock;
        }
        for r in self.offhand.inventory.iter_mut().filter(|r| r.weapon != 0) {
            r.ammo = r.ammo.max(1);
        }
    }

    /// Rounds in the (right hand's) magazine and in reserve.
    pub fn ammo(&self) -> (i32, i32) {
        (self.hand.clip, self.hand.stock)
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
    /// The weapon's bullet penetration.
    pub fn set_penetration(&mut self, p: Option<Penetration>) {
        self.penetration = p;
    }

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
        if let Some(m) = self.charge_out {
            ps.melee_charge_yaw = m.melee_charge_yaw;
            ps.melee_charge_dist = m.melee_charge_dist;
            ps.melee_charge_time = m.melee_charge_time;
        }
    }

    /// The crosshair's inputs: the last frame's spread cone (min, max degrees) and aim spread scale (0..255).
    pub fn hud_spread(&self) -> ((f32, f32), f32) {
        (self.spread_cone, self.spread.aim_spread_scale)
    }

    /// Viewmodel kick, degrees (pitch, yaw).
    pub fn gun_kick(&self) -> [f32; 2] {
        self.gun_recoil.angles
    }

    /// The gun's recoil springs (MW2's weapon placement turns the gun by them).
    pub fn gun_recoil_state(&self) -> GunRecoilPlacementState {
        self.gun_recoil
    }

    /// The gun's largest recoil turn (pitch, yaw degrees; the kick record's, 0 without one).
    pub fn gun_max(&self) -> [f32; 2] {
        self.kick
            .map_or([0.0; 2], |k| [k.gun_max_pitch, k.gun_max_yaw])
    }

    /// The raw `weap_anim` word (IW4: the weapon animation event and its restart bit).
    pub fn weap_anim_raw(&self) -> i32 {
        self.hand.weap_anim
    }

    /// The left gun's `weap_anim` word (an akimbo pair's hand 1, stepped by IW4L's `weapon_hands` on its own:
    /// it fires, reloads and raises apart from the right).
    pub fn left_weap_anim_raw(&self) -> Option<i32> {
        self.left.map(|h| h.weap_anim)
    }

    /// Which guns the last step fired: the right (hand 0, every gun) and the left (an akimbo pair's hand 1).
    pub fn fired_hands(&self) -> [bool; 2] {
        self.fired
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
        let scale = f64::from(self.kick_scale);
        for (axis, amplitude) in self.camera.iter_mut().zip(impulse.camera_peak) {
            axis.add_peak(f64::from(amplitude) * scale, CAMERA_PEAK_SECONDS);
        }
        for (axis, amplitude) in self.gun_recoil.axes.iter_mut().zip(impulse.model_peak) {
            axis.add_peak(f64::from(amplitude) * scale, MODEL_PEAK_SECONDS);
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
        self.spread_cone = (cone.min, cone.max);
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
        let akimbo = self.left.is_some();
        if akimbo {
            // Akimbo on a pad (IW4L `console::binds::gameplay_binding`): the fire trigger becomes `+speed_throw`
            // (the right gun, hand 0) and the aim trigger `+attack` (the left gun, hand 1; IW4L
            // `get_weapon_fire_button` with `last_weapon_hand` 1).
            if input.fire {
                btn |= weapon_iw4::BUTTON_THROW;
            }
            if input.ads {
                btn |= buttons::ATTACK;
            }
        } else if input.fire {
            btn |= buttons::ATTACK;
        }
        if input.reload {
            btn |= buttons::RELOAD;
        }
        if input.melee {
            btn |= weapon_iw4::BUTTON_MELEE;
        }
        if input.frag {
            btn |= weapon_iw4::BUTTON_FRAG;
        }
        if input.smoke {
            btn |= weapon_iw4::BUTTON_SMOKE;
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
            alternate_switch: target.alternate,
            switch_alternate_raise_time_ms: target.alternate_raise_ms,
            last_weapon_hand: i32::from(akimbo),
            // The ADS fraction ([`Gun::update_spread`] took it from the player): ADS-only weapons (the Javelin) fire
            // only aimed in (IW4L `ads_fire_only`).
            f_weapon_pos_frac: self.ads_frac,
            pm_flags: input.pm_flags,
            perks0: input.perks[0],
            perks1: input.perks[1],
            offhand: self.offhand,
            melee_charge_yaw: input.melee_charge.map_or(0.0, |c| c.0),
            melee_charge_dist: input.melee_charge.map_or(0, |c| c.1),
            player_melee_range: movement_iw4::MELEE_CHARGE_PLAYER_MELEE_RANGE_DEFAULT,
            is_in_air: input.in_air,
            pm_type: input.pm_type,
            melee_charge: weapon_iw4::MeleeChargeState {
                pm_flags: input.pm_flags,
                pm_type: input.pm_type,
                e_flags: 0,
                melee_charge_yaw: input.charge_state.0,
                melee_charge_dist: input.charge_state.1,
                melee_charge_time: input.charge_state.2,
            },
            ..WeaponCmd::default()
        };
        self.old_buttons = btn;
        self.advance_recoil(msec as f32 / 1000.0);
        // IW4L's hand driver (as `advance_weapon_command`): stun, melee start, then each hand's own step (both of an
        // akimbo pair; they share the reserve).
        let events = match self.left {
            Some(left) => {
                let mut hands = [self.hand, left];
                let e = weapon_hands(&mut hands, &self.facts, &mut cmd, 1);
                (self.hand, self.left) = (hands[0], Some(hands[1]));
                e
            }
            None => weapon_hands(
                core::slice::from_mut(&mut self.hand),
                &self.facts,
                &mut cmd,
                0,
            ),
        };
        self.swung = cmd.melee_started.is_some();
        self.offhand = cmd.offhand;
        self.pm_flags_out = Some(cmd.pm_flags);
        self.charge_out = Some(cmd.melee_charge);
        self.extra_pellets.clear();
        let [right_event, left_event] = events.map(|e| e.map(|(_, e)| e));
        let shot_of =
            |e: Option<WeaponTickEvent>| matches!(e, Some(WeaponTickEvent::ShotAccepted { .. }));
        self.fired = [shot_of(right_event), shot_of(left_event)];
        // The left gun's shot: its bullets join this frame's extra pellets.
        if let Some(WeaponTickEvent::ShotAccepted { ammo_used }) = left_event
            && let Some(s) = self.fire(clip, ammo_used, eye, dir, right, up)
        {
            self.extra_pellets.insert(0, s);
        }
        let event = right_event.or(left_event);
        let shot = match right_event {
            Some(WeaponTickEvent::ShotAccepted { ammo_used }) => {
                self.fire(clip, ammo_used, eye, dir, right, up)
            }
            _ => None,
        };
        // Only the left gun fired: its first bullet is the frame's shot.
        let shot = match shot {
            None if !self.extra_pellets.is_empty() => Some(self.extra_pellets.remove(0)),
            s => s,
        };
        (event, shot)
    }

    /// A shot of `ammo_used` rounds from `eye` along `dir`: its pellets traced (the first returned, the rest in
    /// the extra pellets), or a projectile handed over ([`Gun::take_launch`]); then recoil and the spread's widening.
    #[allow(clippy::too_many_arguments)]
    fn fire(
        &mut self,
        clip: &MapClip<'_>,
        ammo_used: i32,
        eye: [f32; 3],
        dir: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
    ) -> Option<Shot> {
        {
            {
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
                if self.launches() {
                    // One projectile in the cone (IW4L `spread_direction_on_plane`: the same uniform radius and
                    // angle on a plane ahead), no bullet.
                    let a = self.random() * core::f32::consts::TAU;
                    let r = self.random() * t;
                    let (x, y) = (libm::cosf(a) * r, libm::sinf(a) * r);
                    let d: [f32; 3] = core::array::from_fn(|k| dir[k] + right[k] * x + up[k] * y);
                    let len = libm::sqrtf(d.iter().map(|v| v * v).sum::<f32>()).max(1e-6);
                    self.launch = Some(Launch {
                        weapon: self.hand.weapon,
                        origin: eye,
                        dir: d.map(|v| v / len),
                        right,
                    });
                    self.seed_recoil();
                    add_aim_spread_fire(
                        &mut self.spread.aim_spread_scale,
                        self.ads_frac,
                        self.facts.hip_spread_fire_add,
                    );
                    return None;
                }
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
        }
    }

    fn trace_bullet(&self, clip: &MapClip<'_>, eye: [f32; 3], dir: [f32; 3]) -> Shot {
        let range = self.facts.bullet_range();
        let end = [
            eye[0] + dir[0] * range,
            eye[1] + dir[1] * range,
            eye[2] + dir[2] * range,
        ];
        let t: Trace = clip.bullet_trace(eye, end, MASK_SHOT);
        let f = t.fraction.clamp(0.0, 1.0);
        let first = [
            eye[0] + (end[0] - eye[0]) * f,
            eye[1] + (end[1] - eye[1]) * f,
            eye[2] + (end[2] - eye[2]) * f,
        ];
        let mut shot = Shot {
            start: eye,
            end: first,
            hit: t.fraction < 1.0,
            normal: t.normal,
            surface: t.surface_flags,
            pen_end: first,
            layers: [(0.0, 0.0); MAX_LAYERS],
            layer_count: 0,
        };
        if shot.hit
            && let Some(pen) = self.penetration
            && pen.kind != 0
        {
            penetrate(clip, &mut shot, &t, dir, end, &pen);
        }
        shot
    }
}

/// Go on through walls from the first hit (IW4L `fire_penetrate`, world surfaces): each wall's depth limit is the
/// table's for the weapon's type and the entry (and exit) surface times the weapon's multiplier; the wall's
/// thickness (entry to the exit found by tracing back from the far side) takes `thickness / depth` off the bullet's
/// damage scale; it stops when that runs out, at an unpenetrable surface, or after five walls.
fn penetrate(
    clip: &MapClip<'_>,
    shot: &mut Shot,
    first: &Trace,
    dir: [f32; 3],
    end: [f32; 3],
    pen: &Penetration,
) {
    use weapon_iw4::{
        ADVANCE_TRACE_FWD, ADVANCE_TRACE_REV, PEN_THICKNESS_FLOOR, REV_END_EPS, advance_trace,
        depth_surface_type,
    };
    let dist = |a: [f32; 3], b: [f32; 3]| {
        libm::sqrtf((0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum::<f32>())
    };
    let mut multiplier = 1.0f32;
    let mut entry = shot.end;
    let mut normal = first.normal;
    let mut last_surf = depth_surface_type(first.surface_flags, 0);
    for _ in 0..MAX_LAYERS {
        let mut max_depth = pen.table.depth(pen.kind, last_surf) * pen.multiplier;
        if max_depth <= 0.0 {
            return;
        }
        let Some(next) = advance_trace(entry, normal, dir, true, ADVANCE_TRACE_FWD) else {
            return;
        };
        let fwd = clip.bullet_trace(next, end, MASK_SHOT);
        if fwd.startsolid != 0 && fwd.allsolid != 0 {
            // Still inside the wall all the way: it doesn't come out.
            return;
        }
        let fwd_hit = fwd.fraction < 1.0;
        let fwd_pos: [f32; 3] =
            core::array::from_fn(|k| next[k] + (end[k] - next[k]) * fwd.fraction);
        // Back from the far side to the wall's exit face.
        let mut rev_start = if fwd_hit { fwd_pos } else { end };
        if fwd_hit
            && let Some(n) = advance_trace(
                fwd_pos,
                fwd.normal.map(|v| -v),
                dir.map(|v| -v),
                true,
                ADVANCE_TRACE_REV,
            )
        {
            rev_start = n;
        }
        let rev_end: [f32; 3] = core::array::from_fn(|k| entry[k] - dir[k] * REV_END_EPS);
        let rev = clip.trace(rev_start, rev_end, [0.0; 3], [0.0; 3], MASK_SHOT);
        if rev.fraction >= 1.0 {
            return;
        }
        let exit: [f32; 3] =
            core::array::from_fn(|k| rev_start[k] + (rev_end[k] - rev_start[k]) * rev.fraction);
        let thickness = dist(entry, exit).max(PEN_THICKNESS_FLOOR);
        let exit_depth = pen
            .table
            .depth(pen.kind, depth_surface_type(rev.surface_flags, 0))
            * pen.multiplier;
        max_depth = max_depth.min(exit_depth);
        if max_depth <= 0.0 {
            return;
        }
        multiplier -= thickness / max_depth;
        if multiplier <= 0.0 {
            return;
        }
        let k = usize::from(shot.layer_count);
        shot.layers[k] = (dist(shot.start, exit), multiplier);
        shot.layer_count += 1;
        if !fwd_hit {
            shot.pen_end = end;
            return;
        }
        shot.pen_end = fwd_pos;
        entry = fwd_pos;
        normal = fwd.normal;
        last_surf = depth_surface_type(fwd.surface_flags, last_surf);
    }
}

/// Ammunition from the ground (`crate::item`): pickups, swaps and Scavenger bags.
impl Gun {
    /// Set the magazine and the reserve (a weapon taken from the ground keeps its rounds, IW4L `set_ammo_on_ps`).
    pub fn set_ammo(&mut self, clip: i32, stock: i32) {
        self.hand.clip = clip.clamp(0, self.facts.clip_size.max(0));
        self.hand.stock = stock.clamp(0, self.facts.max_ammo.max(0));
    }

    /// Add up to `rounds` to the reserve, at most the weapon's maximum (IW4L `add_ammo_on_ps`); returns how many
    /// went in.
    pub fn add_stock(&mut self, rounds: i32) -> i32 {
        let room = (self.facts.max_ammo - self.hand.stock).max(0);
        let n = rounds.clamp(0, room);
        self.hand.stock += n;
        n
    }

    /// One more of each grenade carried, each at most `cap(weapon)` (MW2's Scavenger: `SetWeaponAmmoClip` + 1).
    pub fn add_offhand_ammo(&mut self, cap: impl Fn(u32) -> i32) {
        for r in self.offhand.inventory.iter_mut().filter(|r| r.weapon != 0) {
            r.ammo = (r.ammo + 1).min(cap(r.weapon).max(r.ammo));
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

    /// A projectile weapon (IW4 `WEAPTYPE_PROJECTILE`) fires a projectile, not a bullet: no [`Shot`], one
    /// [`Launch`] along the view.
    #[test]
    fn a_launcher_hands_over_a_projectile() {
        let clip = wall();
        let mc = MapClip::new(&clip);
        let mut rec = rifle();
        rec.combat.weap_type = weapon_iw4::WEAPTYPE_PROJECTILE;
        rec.combat.weap_class = 11;
        rec.combat.clip_size = 1;
        let mut g = Gun::with_index(&rec, 6).unwrap();
        let (eye, x) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0]);
        for _ in 0..40 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let mut launches = 0;
        for _ in 0..20 {
            let (e, s) = g.update(&mc, 16, &fire, eye, x, Y, Z);
            assert!(s.is_none(), "no bullet");
            if matches!(e, Some(WeaponTickEvent::ShotAccepted { .. })) {
                let l = g.take_launch().expect("a projectile");
                assert_eq!((l.weapon, l.origin), (6, eye));
                assert!(l.dir[0] > 0.99);
                launches += 1;
            }
        }
        assert_eq!(launches, 1, "one round in the magazine");
        assert!(g.take_launch().is_none());
        // An ADS-only launcher (the Javelin's `adsFireOnly`): from the hip the shot waits the time left to aim in
        // (IW4L `ads_fire_only_delay_ms`: 800 ms at the Javelin's rate), aimed in it leaves at once.
        rec.combat.ads_fire_only = true;
        rec.combat.aim_down_sight = true;
        rec.combat.ads_in_rate = 0.00125;
        let mut g = Gun::with_index(&rec, 6).unwrap();
        let mut ps = PlayerState::ZERO;
        ps.view_height_current = 60.0;
        let fired = |g: &mut Gun, ps: &PlayerState| {
            let s = SpreadInput {
                ps,
                cmd_angles: [0; 3],
                old_cmd_angles: [0; 3],
                forwardmove: 0,
                rightmove: 0,
            };
            for _ in 0..40 {
                g.update_spread(16, &s);
                g.update(&mc, 16, &fire, eye, x, Y, Z);
                if g.take_launch().is_some() {
                    return true;
                }
            }
            false
        };
        for _ in 0..40 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        assert!(!fired(&mut g, &ps), "not within 640 ms from the hip");
        let mut g = Gun::with_index(&rec, 6).unwrap();
        for _ in 0..40 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        ps.f_weapon_pos_frac = 1.0;
        assert!(fired(&mut g, &ps), "aimed in");
    }

    /// Akimbo (IW4L `weapon_hands` over two hands, `last_weapon_hand` 1): the fire trigger fires the right gun, the
    /// aim trigger the left, each from its own magazine; the reserve is shared.
    #[test]
    fn akimbo_fires_each_hand_from_its_own_magazine() {
        let clip = wall();
        let mc = MapClip::new(&clip);
        let mut rec = rifle();
        rec.combat.dual_wield = true;
        rec.combat.clip_size = 15;
        rec.combat.start_ammo = 90;
        rec.combat.fire_type = 1;
        let mut g = Gun::with_index(&rec, 3).unwrap();
        assert!(g.akimbo());
        assert_eq!((g.ammo(), g.left_clip()), ((15, 60), Some(15)));
        let (eye, x) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0]);
        for _ in 0..40 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let right = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let (_, s) = g.update(&mc, 16, &right, eye, x, Y, Z);
        assert!(s.is_some());
        assert_eq!((g.ammo().0, g.left_clip()), (14, Some(15)), "the right gun");
        for _ in 0..10 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let left = GunInput {
            ads: true,
            ..GunInput::default()
        };
        let event = |raw: i32| raw as u32 & weapon_iw4::WEAP_ANIM_EVENT_MASK;
        let right_anim = g.weap_anim_raw();
        let (_, s) = g.update(&mc, 16, &left, eye, x, Y, Z);
        assert!(s.is_some());
        assert_eq!((g.ammo().0, g.left_clip()), (14, Some(14)), "the left gun");
        // The left hand's anim word plays its fire (IW4 event 2); the right hand's doesn't change.
        assert_eq!(g.fired_hands(), [false, true]);
        assert_eq!(g.left_weap_anim_raw().map(event), Some(2));
        assert_eq!(g.weap_anim_raw(), right_anim);
        for _ in 0..10 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        // Both at once: two bullets this frame.
        let both = GunInput {
            fire: true,
            ads: true,
            ..GunInput::default()
        };
        let (_, s) = g.update(&mc, 16, &both, eye, x, Y, Z);
        assert!(s.is_some() && g.extra_pellets().len() == 1);
        assert_eq!((g.ammo().0, g.left_clip()), (13, Some(13)));
    }

    /// The alternate switch (IW4L `weapon_check_for_change` with `alternate_switch`): the weapon drops in its
    /// `altDropTime`, then the alternate raises in its `altRaiseTime`, not the usual drop and raise.
    #[test]
    fn the_alternate_switch_uses_the_alt_drop_and_raise_times() {
        let clip = Clip::default();
        let mc = MapClip::new(&clip);
        let mut base = rifle();
        base.combat.alternate_drop_time_ms = 150;
        let mut gl = rifle();
        gl.combat.alternate_raise_time_ms = 250;
        gl.combat.weap_type = weapon_iw4::WEAPTYPE_PROJECTILE;
        gl.combat.weap_class = crate::grenade::WEAPCLASS_GRENADE;
        let mut r = Gun::with_index(&base, 3).unwrap();
        let mut a = Gun::with_index(&gl, 4).unwrap();
        let (eye, x) = ([0.0, 0.0, 60.0], [1.0, 0.0, 0.0]);
        for _ in 0..60 {
            r.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let want = GunInput {
            switch_to: Some(a.as_alternate_target()),
            ..GunInput::default()
        };
        let mut t = 0;
        let mut started = false;
        let hand = loop {
            let (e, _) = r.update(&mc, 16, &want, eye, x, Y, Z);
            started |= e == Some(WeaponTickEvent::AlternateStarted);
            t += 16;
            if let Some(h) = r.take_switch() {
                break h;
            }
            assert!(t < 2000);
        };
        assert!(started, "an alternate switch");
        assert!((140..=180).contains(&t), "alt drop 150 ms (drop 300): {t}");
        assert_eq!(
            hand.weaponstate,
            weapon_iw4::WeaponState::RaisingAltswitch as i32
        );
        a.adopt(&hand);
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let mut at = None;
        for k in 0..100 {
            a.update(&mc, 16, &fire, eye, x, Y, Z);
            if a.take_launch().is_some() {
                at = Some((k + 1) * 16);
                break;
            }
        }
        let at = at.expect("the launcher fired");
        assert!((240..=290).contains(&at), "alt raise 250 ms: {at}");
    }

    /// Sleight of Hand (IW4 `PERK_FASTRELOAD` in `ps.perks`) halves the reload, through IW4L's weapon step.
    #[test]
    fn sleight_of_hand_halves_the_reload() {
        let clip = Clip::default();
        let mc = MapClip::new(&clip);
        let mut rec = rifle();
        rec.combat.inherits_perks = true;
        let reload_ms = |perks: [u32; 2]| {
            let mut g = Gun::with_index(&rec, 2).unwrap();
            let eye = [0.0, 0.0, 60.0];
            let x = [1.0, 0.0, 0.0];
            let fire = GunInput {
                fire: true,
                perks,
                ..GunInput::default()
            };
            // Raise, then empty the magazine.
            for _ in 0..400 {
                g.update(&mc, 16, &fire, eye, x, Y, Z);
                if g.ammo().0 == 0 {
                    break;
                }
            }
            let idle = GunInput {
                perks,
                ..GunInput::default()
            };
            let reload = GunInput {
                reload: true,
                perks,
                ..GunInput::default()
            };
            g.update(&mc, 16, &idle, eye, x, Y, Z);
            let mut t = 0;
            g.update(&mc, 16, &reload, eye, x, Y, Z);
            while g.ammo().0 == 0 && t < 10_000 {
                g.update(&mc, 16, &idle, eye, x, Y, Z);
                t += 16;
            }
            t
        };
        let plain = reload_ms([0, 0]);
        let fast = reload_ms([weapon_iw4::PERK_FASTRELOAD, 0]);
        assert!(plain > 1500, "{plain}");
        assert!(fast * 10 < plain * 6, "{fast} vs {plain}");
    }

    /// The lethal button throws a frag through IW4L's offhand states (prepare, hold, throw): `OffhandUsed` once,
    /// spending the grenade; with none left, nothing.
    #[test]
    fn the_lethal_button_throws_a_frag() {
        let clip = Clip::default();
        let mc = MapClip::new(&clip);
        let mut g = Gun::with_index(&rifle(), 2).unwrap();
        g.set_offhands(
            &[weapon_iw4::OffhandInvRow {
                weapon: 9,
                offhand_class: 1,
                ammo: 1,
                fire_time_ms: 600,
                fire_delay_ms: 200,
                fuse_time_ms: 3500,
                weap_type: 1,
                ..weapon_iw4::OffhandInvRow::default()
            }],
            1,
            2,
        );
        let eye = [0.0, 0.0, 60.0];
        let x = [1.0, 0.0, 0.0];
        for _ in 0..60 {
            g.update(&mc, 16, &GunInput::default(), eye, x, Y, Z);
        }
        let mut thrown = 0;
        for k in 0..200 {
            let input = GunInput {
                frag: k < 3,
                ..GunInput::default()
            };
            if let (Some(WeaponTickEvent::OffhandUsed { weapon, .. }), _) =
                g.update(&mc, 16, &input, eye, x, Y, Z)
            {
                assert_eq!(weapon, 9);
                thrown += 1;
            }
        }
        assert_eq!(thrown, 1, "one throw");
        assert_eq!(g.offhand_ammo().next(), Some((9, 0)), "the frag is spent");
        assert!(g.can_switch(), "and the gun is back up");
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
    #[test]
    fn bullets_go_through_thin_walls_losing_damage() {
        // A 20-thick wall at x 500..520 faced with concrete (surface type 5).
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
            &[5 << 20; 6],
            1,
            0,
        );
        let mc = MapClip::new(&c);
        let mut g = Gun::new(&rifle()).unwrap();
        let shoot = |g: &Gun| g.trace_bullet(&mc, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        // No penetration: stops at the wall.
        let s = shoot(&g);
        assert!(s.hit && (s.end[0] - 500.0).abs() < 1.0 && s.layer_count == 0);
        // Medium penetration through 40 units of concrete: 20 of it leaves half.
        let mut depths = [[0.0; 31]; 4];
        depths[2][5] = 40.0;
        g.set_penetration(Some(Penetration {
            kind: 2,
            multiplier: 1.0,
            table: weapon_iw4::PenetrationDepthTable::from_depths(depths),
        }));
        let s = shoot(&g);
        assert_eq!(s.layer_count, 1, "{s:?}");
        let (at, m) = s.layers[0];
        assert!(
            (at - 520.0).abs() < 1.0 && (m - 0.5).abs() < 0.02,
            "{at} {m}"
        );
        assert!(s.pen_end[0] > 1000.0, "flew on: {:?}", s.pen_end);
        assert_eq!(s.scale_at(400.0), 1.0);
        assert!((s.scale_at(800.0) - 0.5).abs() < 0.02);
        // A thicker wall than the depth stops it.
        depths[2][5] = 15.0;
        g.set_penetration(Some(Penetration {
            kind: 2,
            multiplier: 1.0,
            table: weapon_iw4::PenetrationDepthTable::from_depths(depths),
        }));
        assert_eq!(shoot(&g).layer_count, 0);
    }
}

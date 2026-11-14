//! The `WEAP` section of `common.wpk`: every multiplayer weapon's combat numbers, as IW4L's
//! `weapon_iw4::CapturedCombatInput` (what `WeaponCombatFacts::try_from_captured` builds the weapon state machine's
//! facts from). Lossless: these are the gameplay tuning.
//!
//! Records are fixed size: a 48-byte name (NUL padded) then the fields in the order of [`for_fields`]. Names are game
//! data (packs are never in git).

use alloc::vec::Vec;

use weapon_iw4::{CapturedCombatInput, DualMagTimes, EmptyReloadPolicy, HITLOC_COUNT, ScopeZoom};

use crate::be::{self, Push};

pub const NAME_LEN: usize = 48;

/// Calls `$cb!` with the scalar fields of `CapturedCombatInput`, by type. One list drives both directions.
macro_rules! for_fields {
    ($cb:ident) => {
        $cb! {
            i32: [
                fire_time_ms, fire_delay_ms, raise_time_ms, drop_time_ms, alternate_raise_time_ms,
                alternate_drop_time_ms, first_raise_time_ms, reload_time_ms, reload_empty_time_ms, clip_size,
                start_ammo, max_ammo, ammo_index, clip_index, fire_type, weap_type, weap_class, player_anim_type,
                inventory_type, impact_type, shots_per_fire, burst_cooldown_ms, rechamber_time_ms,
                rechamber_bolt_time_ms, rechamber_bolt_delay_ms, reload_start_time_ms, reload_end_time_ms,
                reload_ammo_add, reload_add_time_ms, reload_empty_add_time_ms, reload_start_add_time_ms,
                reload_start_add, sprint_raise_time_ms, sprint_drop_time_ms, stunned_start_time_ms,
                stunned_end_time_ms, damage, flame_damage_interval_ms, min_damage, ads_reload_trans_time_ms,
                melee_damage, overlay_reticle, melee_time_ms, melee_delay_ms, melee_charge_time_ms,
                melee_charge_delay_ms, quick_raise_time_ms, quick_drop_time_ms, ads_gun_kick_reduced_kick_bullets,
                hip_gun_kick_reduced_kick_bullets
            ],
            f32: [
                max_damage_range, min_damage_range, hip_spread_stand_min, hip_spread_ducked_min,
                hip_spread_prone_min, hip_spread_stand_max, hip_spread_ducked_max, hip_spread_prone_max,
                hip_spread_decay_rate, hip_spread_fire_add, hip_spread_turn_add, hip_spread_move_add,
                hip_spread_ducked_decay, hip_spread_prone_decay, ads_spread, ads_in_rate, ads_out_rate
            ],
            bool: [
                dual_wield, fire_melees, bolt_action, segmented_reload, no_partial_reload, aim_down_sight,
                no_ads_when_mag_empty, inherits_perks, rechamber_while_ads, ads_fire_only, can_hold_breath,
                melee_charge_anim
            ],
            u32: [alternate_weapon, knife_model]
        }
    };
}

macro_rules! count {
    ($($x:ident),*) => { 0 $(+ { let _ = stringify!($x); 1 })* };
}

macro_rules! sizes {
    (i32: [$($i:ident),*], f32: [$($f:ident),*], bool: [$($b:ident),*], u32: [$($u:ident),*]) => {
        /// Bytes of one record.
        pub const LEN: usize = NAME_LEN + 4 * (count!($($i),*) + count!($($f),*) + count!($($u),*))
            + count!($($b),*)
            // empty_reload, select_requires_ammo, offhand_hold_is_cancelable, has dual_mag
            + 4
            + 4 * HITLOC_COUNT
            + 4 * 4;
    };
}
for_fields!(sizes);

macro_rules! encoder {
    (i32: [$($i:ident),*], f32: [$($f:ident),*], bool: [$($b:ident),*], u32: [$($u:ident),*]) => {
        fn encode_fields(c: &CapturedCombatInput, o: &mut Vec<u8>) {
            $(o.u32(c.$i as u32);)*
            $(o.f32(c.$f);)*
            $(o.u32(c.$u);)*
            $(o.u8(u8::from(c.$b));)*
        }
    };
}
for_fields!(encoder);

macro_rules! decoder {
    (i32: [$($i:ident),*], f32: [$($f:ident),*], bool: [$($b:ident),*], u32: [$($u:ident),*]) => {
        fn decode_fields(r: &mut Reader<'_>) -> CapturedCombatInput {
            // Struct literal fields are evaluated in the order written: the same order as `encode_fields`.
            CapturedCombatInput {
                $($i: r.u32() as i32,)*
                $($f: r.f32(),)*
                $($u: r.u32(),)*
                $($b: r.u8() != 0,)*
                empty_reload: if r.u8() == 0 { EmptyReloadPolicy::Ordinary } else { EmptyReloadPolicy::Authored },
                select_requires_ammo: opt_bool(r.u8()),
                offhand_hold_is_cancelable: opt_bool(r.u8()),
                dual_mag: {
                    let has = r.u8() != 0;
                    let t = [r.u32() as i32, r.u32() as i32, r.u32() as i32, r.u32() as i32];
                    has.then_some(DualMagTimes {
                        reload_ms: t[0],
                        reload_empty_ms: t[1],
                        add_ms: t[2],
                        empty_add_ms: t[3],
                    })
                },
                location_damage: core::array::from_fn(|_| r.f32()),
                // Variable scope zoom (snipers' zoom levels) is not carried yet.
                scope_zoom: ScopeZoom::NONE,
            }
        }
    };
}
for_fields!(decoder);

fn opt_bool(v: u8) -> Option<bool> {
    match v {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

fn opt_code(v: Option<bool>) -> u8 {
    match v {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        self.at += 1;
        self.b[self.at - 1]
    }
    fn u32(&mut self) -> u32 {
        self.at += 4;
        be::u32(self.b, self.at - 4)
    }
    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }
}

/// One weapon: its name and combat numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponRecord {
    pub name: [u8; NAME_LEN],
    pub combat: CapturedCombatInput,
}

impl WeaponRecord {
    pub fn new(name: &str, combat: CapturedCombatInput) -> WeaponRecord {
        let mut n = [0u8; NAME_LEN];
        let len = name.len().min(NAME_LEN - 1);
        n[..len].copy_from_slice(&name.as_bytes()[..len]);
        WeaponRecord { name: n, combat }
    }

    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

impl crate::records::Record for WeaponRecord {
    const LEN: usize = LEN;
    fn write(&self, o: &mut Vec<u8>) {
        let start = o.len();
        o.extend_from_slice(&self.name);
        let c = &self.combat;
        encode_fields(c, o);
        o.u8(u8::from(c.empty_reload == EmptyReloadPolicy::Authored));
        o.u8(opt_code(c.select_requires_ammo));
        o.u8(opt_code(c.offhand_hold_is_cancelable));
        o.u8(u8::from(c.dual_mag.is_some()));
        let d = c.dual_mag.unwrap_or(DualMagTimes {
            reload_ms: 0,
            reload_empty_ms: 0,
            add_ms: 0,
            empty_add_ms: 0,
        });
        for v in [d.reload_ms, d.reload_empty_ms, d.add_ms, d.empty_add_ms] {
            o.u32(v as u32);
        }
        c.location_damage.iter().for_each(|&v| o.f32(v));
        debug_assert_eq!(o.len() - start, LEN);
    }
    fn read(b: &[u8]) -> Self {
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&b[..NAME_LEN]);
        let mut r = Reader { b, at: NAME_LEN };
        WeaponRecord {
            name,
            combat: decode_fields(&mut r),
        }
    }
}

/// A weapon's recoil numbers (`KICK` section, one per `WEAP` record in the same order): view and gun kick ranges
/// (pitch min, pitch max, yaw min, yaw max) for hip and ADS, the reduced-kick percentages and the gun's maximum
/// pitch / yaw (IW4 `WeaponDef` kick fields, as IW4L's `WeaponKickFacts`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KickRecord {
    pub hip_view: [f32; 4],
    pub ads_view: [f32; 4],
    pub hip_gun: [f32; 4],
    pub ads_gun: [f32; 4],
    pub hip_reduced_percent: f32,
    pub ads_reduced_percent: f32,
    pub gun_max_pitch: f32,
    pub gun_max_yaw: f32,
}

impl crate::records::Record for KickRecord {
    const LEN: usize = 80;
    fn write(&self, o: &mut Vec<u8>) {
        for v in self
            .hip_view
            .iter()
            .chain(&self.ads_view)
            .chain(&self.hip_gun)
            .chain(&self.ads_gun)
            .chain(&[
                self.hip_reduced_percent,
                self.ads_reduced_percent,
                self.gun_max_pitch,
                self.gun_max_yaw,
            ])
        {
            o.f32(*v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let f = |i: usize| be::f32(b, i * 4);
        let four = |at: usize| [f(at), f(at + 1), f(at + 2), f(at + 3)];
        KickRecord {
            hip_view: four(0),
            ads_view: four(4),
            hip_gun: four(8),
            ads_gun: four(12),
            hip_reduced_percent: f(16),
            ads_reduced_percent: f(17),
            gun_max_pitch: f(18),
            gun_max_yaw: f(19),
        }
    }
}

/// A weapon's effect on movement (`WMOV` section, parallel to `WEAP`): IW4 `moveSpeedScale`, `adsMoveSpeedScale`
/// and `sprintDurationScale` (IW4L's `weapon_def_scales`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveRecord {
    pub move_speed_scale: f32,
    pub ads_move_speed_scale: f32,
    pub sprint_duration_scale: f32,
}

impl Default for MoveRecord {
    /// No record: full speed, MW2's usual half speed in ADS, the standard sprint.
    fn default() -> Self {
        MoveRecord {
            move_speed_scale: 1.0,
            ads_move_speed_scale: 0.5,
            sprint_duration_scale: 1.0,
        }
    }
}

impl crate::records::Record for MoveRecord {
    const LEN: usize = 12;
    fn write(&self, o: &mut Vec<u8>) {
        o.f32(self.move_speed_scale);
        o.f32(self.ads_move_speed_scale);
        o.f32(self.sprint_duration_scale);
    }
    fn read(b: &[u8]) -> Self {
        MoveRecord {
            move_speed_scale: be::f32(b, 0),
            ads_move_speed_scale: be::f32(b, 4),
            sprint_duration_scale: be::f32(b, 8),
        }
    }
}

/// A weapon's aim numbers (`WAIM` section, parallel to `WEAP`): IW4 `adsZoomFov`, `adsZoomInFrac`,
/// `adsZoomOutFrac` (when in the ADS transition the zoom happens), and the aim-assist ranges `autoAimRange`,
/// `aimAssistRange`, `aimAssistRangeAds`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimRecord {
    pub ads_zoom_fov: f32,
    pub ads_zoom_in_frac: f32,
    pub ads_zoom_out_frac: f32,
    pub auto_aim_range: f32,
    pub aim_assist_range: f32,
    pub aim_assist_range_ads: f32,
}

impl crate::records::Record for AimRecord {
    const LEN: usize = 24;
    fn write(&self, o: &mut Vec<u8>) {
        for v in [
            self.ads_zoom_fov,
            self.ads_zoom_in_frac,
            self.ads_zoom_out_frac,
            self.auto_aim_range,
            self.aim_assist_range,
            self.aim_assist_range_ads,
        ] {
            o.f32(v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let f = |i: usize| be::f32(b, i * 4);
        AimRecord {
            ads_zoom_fov: f(0),
            ads_zoom_in_frac: f(1),
            ads_zoom_out_frac: f(2),
            auto_aim_range: f(3),
            aim_assist_range: f(4),
            aim_assist_range_ads: f(5),
        }
    }
}

/// How the first-person weapon moves (IW4 weapon def, `WFPV`, parallel to `WEAP`): view sway (hip and ADS: max
/// angle, pitch, yaw, horizontal and vertical scales), idle sway (ADS / hip amount and speed, crouch and prone
/// factors), the stance / movement offsets (`standMove` .. `proneRot`, rates and minimum speeds), the crouched and
/// prone gun offsets, the ADS aim pitch and bob factors. Read by IW4L's `weapon_placement_assemble` on the Wii.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FpvRecord {
    pub sway_hip: [f32; 5],
    pub sway_ads: [f32; 5],
    /// ADS amount, hip amount, ADS speed, hip speed, crouch factor, prone factor.
    pub idle: [f32; 6],
    /// stand move, stand rot, strafe move, strafe rot, ducked move, ducked rot, prone move, prone rot.
    pub movement: [[f32; 3]; 8],
    /// pos move rate, pos prone move rate, stand / ducked / prone move min speed, pos rot rate, pos prone rot rate.
    pub movement_rates: [f32; 7],
    pub ducked_ofs: [f32; 3],
    pub prone_ofs: [f32; 3],
    pub ads_aim_pitch: f32,
    pub ads_bob_factor: f32,
    pub ads_view_bob_mult: f32,
}

impl Default for FpvRecord {
    fn default() -> Self {
        FpvRecord {
            sway_hip: [0.0; 5],
            sway_ads: [0.0; 5],
            idle: [0.0; 6],
            movement: [[0.0; 3]; 8],
            movement_rates: [0.0; 7],
            ducked_ofs: [0.0; 3],
            prone_ofs: [0.0; 3],
            ads_aim_pitch: 0.0,
            ads_bob_factor: 0.0,
            ads_view_bob_mult: 0.0,
        }
    }
}

impl FpvRecord {
    const FLOATS: usize = 56;

    fn floats(&self) -> [f32; Self::FLOATS] {
        let mut v = [0.0; Self::FLOATS];
        let mut i = 0;
        let mut put = |x: f32| {
            v[i] = x;
            i += 1;
        };
        self.sway_hip.iter().for_each(|&x| put(x));
        self.sway_ads.iter().for_each(|&x| put(x));
        self.idle.iter().for_each(|&x| put(x));
        self.movement.iter().flatten().for_each(|&x| put(x));
        self.movement_rates.iter().for_each(|&x| put(x));
        self.ducked_ofs.iter().for_each(|&x| put(x));
        self.prone_ofs.iter().for_each(|&x| put(x));
        put(self.ads_aim_pitch);
        put(self.ads_bob_factor);
        put(self.ads_view_bob_mult);
        v
    }

    fn from_floats(v: &[f32; Self::FLOATS]) -> FpvRecord {
        let mut i = 0;
        let mut take = || {
            i += 1;
            v[i - 1]
        };
        let arr = |n: usize, take: &mut dyn FnMut() -> f32| -> [f32; 8] {
            let mut a = [0.0; 8];
            for x in a.iter_mut().take(n) {
                *x = take();
            }
            a
        };
        let sh = arr(5, &mut take);
        let sa = arr(5, &mut take);
        let id = arr(6, &mut take);
        let mut movement = [[0.0; 3]; 8];
        for m in &mut movement {
            for x in m.iter_mut() {
                *x = take();
            }
        }
        let mr = arr(7, &mut take);
        let d = arr(3, &mut take);
        let p = arr(3, &mut take);
        FpvRecord {
            sway_hip: [sh[0], sh[1], sh[2], sh[3], sh[4]],
            sway_ads: [sa[0], sa[1], sa[2], sa[3], sa[4]],
            idle: [id[0], id[1], id[2], id[3], id[4], id[5]],
            movement,
            movement_rates: [mr[0], mr[1], mr[2], mr[3], mr[4], mr[5], mr[6]],
            ducked_ofs: [d[0], d[1], d[2]],
            prone_ofs: [p[0], p[1], p[2]],
            ads_aim_pitch: take(),
            ads_bob_factor: take(),
            ads_view_bob_mult: take(),
        }
    }
}

impl crate::records::Record for FpvRecord {
    const LEN: usize = FpvRecord::FLOATS * 4;
    fn write(&self, o: &mut Vec<u8>) {
        for v in self.floats() {
            o.f32(v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let v: [f32; FpvRecord::FLOATS] = core::array::from_fn(|i| be::f32(b, i * 4));
        FpvRecord::from_floats(&v)
    }
}
/// A thrown or launched weapon's numbers (IW4 weapon def, `WEQP`, parallel to `WEAP`; zero for guns): the offhand
/// class (1 frag, 2 smoke, 3 flash ...), fuse and cooking, the explosion (radius, inner / outer damage), the throw
/// (speed, extra up / forward speed, arming distance), stickiness, and the bounce factors per surface type (IW4's 31
/// surface types: velocity kept along and against the surface).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EquipRecord {
    pub offhand_class: i32,
    pub fuse_time_ms: i32,
    pub hold_fire_time_ms: i32,
    pub cook_off_hold: bool,
    pub timed_detonation: bool,
    pub projectile_rotates: bool,
    pub stickiness: i32,
    pub explosion_radius: i32,
    pub explosion_radius_min: i32,
    pub explosion_inner_damage: i32,
    pub explosion_outer_damage: i32,
    pub projectile_speed: i32,
    pub projectile_speed_up: i32,
    pub projectile_speed_forward: i32,
    pub projectile_activate_dist: i32,
    pub projectile_explosion_type: i32,
    pub start_ammo: i32,
    /// Damage on a direct hit (IW4L `impact_damage`: the weapon's damage; a throwing knife's kill).
    pub impact_damage: i32,
    /// The blast's cone half angle in degrees (IW4 `damageConeAngle`; a claymore's; 0 or 180: all round).
    pub damage_cone_angle: f32,
    pub parallel_bounce: [f32; 31],
    pub perpendicular_bounce: [f32; 31],
    /// Sounds in `common.wpk`'s bank (`equip_sound`; `crate::sound::NONE` when none): the explosion, a bounce, the
    /// pull back and the throw.
    pub sounds: [u16; 4],
}

/// Indices into [`EquipRecord::sounds`].
pub mod equip_sound {
    pub const EXPLODE: usize = 0;
    pub const BOUNCE: usize = 1;
    pub const PULLBACK: usize = 2;
    pub const THROW: usize = 3;
}

impl Default for EquipRecord {
    fn default() -> Self {
        EquipRecord {
            offhand_class: 0,
            fuse_time_ms: 0,
            hold_fire_time_ms: 0,
            cook_off_hold: false,
            timed_detonation: false,
            projectile_rotates: false,
            stickiness: 0,
            explosion_radius: 0,
            explosion_radius_min: 0,
            explosion_inner_damage: 0,
            explosion_outer_damage: 0,
            projectile_speed: 0,
            projectile_speed_up: 0,
            projectile_speed_forward: 0,
            projectile_activate_dist: 0,
            projectile_explosion_type: 0,
            start_ammo: 0,
            impact_damage: 0,
            damage_cone_angle: 0.0,
            parallel_bounce: [0.0; 31],
            perpendicular_bounce: [0.0; 31],
            sounds: [crate::sound::NONE; 4],
        }
    }
}

impl crate::records::Record for EquipRecord {
    const LEN: usize = 16 * 4 + 4 + 62 * 4 + 8;
    fn write(&self, o: &mut Vec<u8>) {
        for v in [
            self.offhand_class,
            self.fuse_time_ms,
            self.hold_fire_time_ms,
            self.stickiness,
            self.explosion_radius,
            self.explosion_radius_min,
            self.explosion_inner_damage,
            self.explosion_outer_damage,
            self.projectile_speed,
            self.projectile_speed_up,
            self.projectile_speed_forward,
            self.projectile_activate_dist,
            self.projectile_explosion_type,
            self.start_ammo,
            self.impact_damage,
            self.damage_cone_angle.to_bits() as i32,
        ] {
            o.u32(v as u32);
        }
        o.push(u8::from(self.cook_off_hold));
        o.push(u8::from(self.timed_detonation));
        o.push(u8::from(self.projectile_rotates));
        o.push(0);
        for v in self
            .parallel_bounce
            .iter()
            .chain(&self.perpendicular_bounce)
        {
            o.f32(*v);
        }
        for v in self.sounds {
            o.u16(v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let i = |k: usize| be::u32(b, k * 4) as i32;
        let f = |k: usize| be::f32(b, 68 + k * 4);
        EquipRecord {
            offhand_class: i(0),
            fuse_time_ms: i(1),
            hold_fire_time_ms: i(2),
            stickiness: i(3),
            explosion_radius: i(4),
            explosion_radius_min: i(5),
            explosion_inner_damage: i(6),
            explosion_outer_damage: i(7),
            projectile_speed: i(8),
            projectile_speed_up: i(9),
            projectile_speed_forward: i(10),
            projectile_activate_dist: i(11),
            projectile_explosion_type: i(12),
            start_ammo: i(13),
            impact_damage: i(14),
            damage_cone_angle: f32::from_bits(i(15) as u32),
            cook_off_hold: b[64] != 0,
            timed_detonation: b[65] != 0,
            projectile_rotates: b[66] != 0,
            parallel_bounce: core::array::from_fn(f),
            perpendicular_bounce: core::array::from_fn(|k| f(31 + k)),
            sounds: core::array::from_fn(|k| be::u16(b, 68 + 62 * 4 + k * 2)),
        }
    }
}
/// The record named `name` in a `WEAP` section, with its index.
pub fn find_indexed(section: &[u8], name: &str) -> Option<(u32, WeaponRecord)> {
    section.chunks_exact(LEN).enumerate().find_map(|(i, r)| {
        let end = r[..NAME_LEN]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(NAME_LEN);
        (&r[..end] == name.as_bytes())
            .then(|| (i as u32, <WeaponRecord as crate::records::Record>::read(r)))
    })
}

/// The record named `name` in a `WEAP` section.
pub fn find(section: &[u8], name: &str) -> Option<WeaponRecord> {
    section
        .chunks_exact(LEN)
        .find(|r| {
            let end = r[..NAME_LEN]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(NAME_LEN);
            &r[..end] == name.as_bytes()
        })
        .map(<WeaponRecord as crate::records::Record>::read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::Record;

    #[test]
    fn round_trip_every_field() {
        let mut c = CapturedCombatInput {
            empty_reload: EmptyReloadPolicy::Authored,
            ..CapturedCombatInput::default()
        };
        c.fire_time_ms = 86;
        c.clip_size = 30;
        c.hip_spread_fire_add = 0.75;
        c.bolt_action = true;
        c.select_requires_ammo = Some(true);
        c.dual_mag = Some(DualMagTimes {
            reload_ms: 1,
            reload_empty_ms: 2,
            add_ms: 3,
            empty_add_ms: 4,
        });
        c.location_damage[3] = 1.4;
        let r = WeaponRecord::new("test_mp", c);
        let mut b = Vec::new();
        r.write(&mut b);
        assert_eq!(b.len(), LEN);
        assert_eq!(WeaponRecord::read(&b), r);
        assert_eq!(find(&b, "test_mp").unwrap().name(), "test_mp");
        assert!(find(&b, "other_mp").is_none());
    }
}

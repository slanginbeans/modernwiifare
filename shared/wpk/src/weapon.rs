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

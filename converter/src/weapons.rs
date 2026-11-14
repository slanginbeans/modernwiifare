//! Weapon combat numbers for the Wii: each loaded weapon's `WeaponGeometry` (decoded by IW4L's zone loader) as IW4L's
//! `weapon_iw4::CapturedCombatInput`, the input of its weapon state machine.
//!
//! The mapping follows IW4L `crates/asset_game/src/weapon_catalog/capture_iw4.rs` (geometry → body facts) and
//! `combat.rs` (`captured_input`), Apache-2.0, rev `acc57a5`, field for field. Not carried yet: variable scope zoom,
//! the melee-charge animation flag, alternate weapons and dual wield (IW4 MP base weapons don't need them to fire).

use fastfile_iw4::WeaponGeometry;
use weapon_iw4::{CapturedCombatInput, EmptyReloadPolicy, LOCATION_DAMAGE_IDENTITY, ScopeZoom};

/// IW4 `WEAPCLASS_ROCKETLAUNCHER`: rocket launchers can't hold breath even with an overlay.
const WEAPCLASS_ROCKETLAUNCHER: i32 = 11;

/// The rawfile holding MW2's per-hit-location damage multipliers.
pub const LOCHIT_RAWFILE: &str = "info/mp_lochit_dmgtable";

/// `LOCDMGTABLE\<location>\<scale>...` (IW4L `asset_game::lochit::parse_lochit_info_string`, Apache-2.0, rev
/// `acc57a5`): unknown keys, odd pairs or bad numbers reject the table.
pub fn parse_lochit(data: &[u8], zlib_compressed: bool) -> Option<[f32; weapon_iw4::HITLOC_COUNT]> {
    let mut bytes = if zlib_compressed {
        asset_transport::inflate_zlib(data).ok()?
    } else {
        data.to_vec()
    };
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    let text = core::str::from_utf8(&bytes).ok()?.trim();
    let rest = text.strip_prefix("LOCDMGTABLE")?;
    let mut table = LOCATION_DAMAGE_IDENTITY;
    let mut parts = rest.split('\\').filter(|p| !p.is_empty());
    while let Some(key) = parts.next() {
        let value: f32 = parts.next()?.parse().ok()?;
        let slot = weapon_iw4::HITLOC_NAMES.iter().position(|n| *n == key)?;
        table[slot] = value;
    }
    weapon_iw4::location_damage_is_valid(&table).then_some(table)
}

/// Bake the global location table into a weapon's (IW4L `bake_location_damage`: bullets only, not turrets).
pub fn with_global_location(
    mut c: CapturedCombatInput,
    global: Option<[f32; weapon_iw4::HITLOC_COUNT]>,
) -> CapturedCombatInput {
    c.location_damage = weapon_iw4::bake_location_damage(
        c.weap_type,
        c.weap_class,
        global.unwrap_or(LOCATION_DAMAGE_IDENTITY),
        Some(c.location_damage),
    );
    c
}

/// Movement scales (IW4L `weapon_def_scales`: moveSpeedScale, adsMoveSpeedScale, sprintDurationScale).
pub fn moves(g: &WeaponGeometry) -> wpk::weapon::MoveRecord {
    wpk::weapon::MoveRecord {
        move_speed_scale: g.move_speed_scale,
        ads_move_speed_scale: g.ads_move_speed_scale,
        sprint_duration_scale: g.sprint_duration_scale,
    }
}

/// How the first-person weapon moves: view sway, idle sway, stance / movement offsets, ADS pitch and bob.
pub fn fpv(g: &WeaponGeometry) -> wpk::weapon::FpvRecord {
    let s = &g.sway;
    let m = &g.movement;
    let i = &g.idle;
    wpk::weapon::FpvRecord {
        sway_hip: [
            s.sway_max_angle,
            s.sway_pitch_scale,
            s.sway_yaw_scale,
            s.sway_horiz_scale,
            s.sway_vert_scale,
        ],
        sway_ads: [
            s.ads_sway_max_angle,
            s.ads_sway_pitch_scale,
            s.ads_sway_yaw_scale,
            s.ads_sway_horiz_scale,
            s.ads_sway_vert_scale,
        ],
        idle: [
            i.ads_idle_amount,
            i.hip_idle_amount,
            i.ads_idle_speed,
            i.hip_idle_speed,
            i.idle_crouch_factor,
            i.idle_prone_factor,
        ],
        movement: [
            m.stand_move,
            m.stand_rot,
            m.strafe_move,
            m.strafe_rot,
            m.ducked_move,
            m.ducked_rot,
            m.prone_move,
            m.prone_rot,
        ],
        movement_rates: [
            m.pos_move_rate,
            m.pos_prone_move_rate,
            m.stand_move_min_speed,
            m.ducked_move_min_speed,
            m.prone_move_min_speed,
            m.pos_rot_rate,
            m.pos_prone_rot_rate,
        ],
        ducked_ofs: g.ducked_ofs,
        prone_ofs: g.prone_ofs,
        ads_aim_pitch: g.ads_aim_pitch,
        ads_bob_factor: g.ads_bob_factor,
        ads_view_bob_mult: g.ads_view_bob_mult,
    }
}

/// A thrown / launched weapon's numbers (zero for guns).
pub fn equip(g: &WeaponGeometry) -> wpk::weapon::EquipRecord {
    wpk::weapon::EquipRecord {
        offhand_class: g.offhand_class,
        fuse_time_ms: g.fuse_time_ms,
        hold_fire_time_ms: g.hold_fire_time_ms,
        cook_off_hold: g.cook_off_hold,
        timed_detonation: g.timed_detonation,
        projectile_rotates: g.projectile_rotates,
        stickiness: g.stickiness,
        explosion_radius: g.explosion_radius,
        explosion_radius_min: g.explosion_radius_min,
        explosion_inner_damage: g.explosion_inner_damage,
        explosion_outer_damage: g.explosion_outer_damage,
        projectile_speed: g.projectile_speed,
        projectile_speed_up: g.projectile_speed_up,
        projectile_speed_forward: g.projectile_speed_forward,
        projectile_activate_dist: g.projectile_activate_dist,
        projectile_explosion_type: g.projectile_explosion_type,
        start_ammo: g.start_ammo,
        impact_damage: g.damage,
        parallel_bounce: g.parallel_bounce.unwrap_or([0.0; 31]),
        perpendicular_bounce: g.perpendicular_bounce.unwrap_or([0.0; 31]),
        sounds: [wpk::sound::NONE; 4],
    }
}

/// ADS zoom and aim-assist ranges.
pub fn aims(g: &WeaponGeometry) -> wpk::weapon::AimRecord {
    wpk::weapon::AimRecord {
        ads_zoom_fov: g.ads_zoom_fov,
        ads_zoom_in_frac: g.ads_zoom_in_frac,
        ads_zoom_out_frac: g.ads_zoom_out_frac,
        auto_aim_range: g.auto_aim_range,
        aim_assist_range: g.aim_assist_range,
        aim_assist_range_ads: g.aim_assist_range_ads,
    }
}

/// Recoil numbers (IW4L `WeaponKickFacts` from the same capture).
pub fn kick(g: &WeaponGeometry) -> wpk::weapon::KickRecord {
    let k = &g.kick;
    wpk::weapon::KickRecord {
        hip_view: [
            k.hip_view_kick_pitch_min,
            k.hip_view_kick_pitch_max,
            k.hip_view_kick_yaw_min,
            k.hip_view_kick_yaw_max,
        ],
        ads_view: [
            k.ads_view_kick_pitch_min,
            k.ads_view_kick_pitch_max,
            k.ads_view_kick_yaw_min,
            k.ads_view_kick_yaw_max,
        ],
        hip_gun: [
            k.hip_gun_kick_pitch_min,
            k.hip_gun_kick_pitch_max,
            k.hip_gun_kick_yaw_min,
            k.hip_gun_kick_yaw_max,
        ],
        ads_gun: [
            k.ads_gun_kick_pitch_min,
            k.ads_gun_kick_pitch_max,
            k.ads_gun_kick_yaw_min,
            k.ads_gun_kick_yaw_max,
        ],
        hip_reduced_percent: k.hip_gun_kick_reduced_kick_percent,
        ads_reduced_percent: k.ads_gun_kick_reduced_kick_percent,
        gun_max_pitch: k.gun_max_pitch,
        gun_max_yaw: k.gun_max_yaw,
    }
}

pub fn captured(g: &WeaponGeometry) -> CapturedCombatInput {
    let segmented_reload = g.segmented_reload && g.reload_start_time_ms > 0;
    let reload_ammo_add = if g.reload_ammo_add > 0 {
        g.reload_ammo_add
    } else if segmented_reload {
        1
    } else {
        0
    };
    CapturedCombatInput {
        dual_wield: false,
        fire_melees: false,
        fire_time_ms: g.fire_time_ms,
        fire_delay_ms: g.fire_delay_ms,
        raise_time_ms: g.raise_time_ms,
        drop_time_ms: g.drop_time_ms,
        alternate_weapon: 0,
        alternate_raise_time_ms: g.alternate_raise_time_ms,
        alternate_drop_time_ms: g.alternate_drop_time_ms,
        first_raise_time_ms: g.first_raise_time_ms,
        reload_time_ms: g.reload_time_ms,
        reload_empty_time_ms: g.reload_empty_time_ms,
        empty_reload: EmptyReloadPolicy::Authored,
        clip_size: g.clip_size,
        // IW4 ammo counts are in rounds (not clip-relative).
        start_ammo: g.start_ammo,
        max_ammo: g.max_ammo,
        ammo_index: g.ammo_index,
        clip_index: g.clip_index,
        fire_type: g.fire_type,
        weap_type: g.weap_type,
        weap_class: g.weap_class,
        player_anim_type: g.player_anim_type,
        inventory_type: g.inventory_type,
        impact_type: g.impact_type,
        shots_per_fire: g.shots_per_fire,
        burst_cooldown_ms: 0,
        bolt_action: g.bolt_action,
        rechamber_time_ms: g.rechamber_time_ms,
        rechamber_bolt_time_ms: g.rechamber_bolt_time_ms,
        rechamber_bolt_delay_ms: g.rechamber_bolt_delay_ms,
        segmented_reload,
        reload_start_time_ms: g.reload_start_time_ms,
        reload_end_time_ms: g.reload_end_time_ms,
        reload_ammo_add,
        reload_add_time_ms: g.reload_add_time_ms,
        reload_empty_add_time_ms: 0,
        reload_start_add_time_ms: g.reload_start_add_time_ms,
        reload_start_add: g.reload_start_add,
        no_partial_reload: g.no_partial_reload,
        dual_mag: None,
        inherits_perks: g.inherits_perks,
        sprint_raise_time_ms: g.sprint_raise_time_ms,
        sprint_drop_time_ms: g.sprint_drop_time_ms,
        stunned_start_time_ms: g.stunned_start_time_ms,
        stunned_end_time_ms: g.stunned_end_time_ms,
        damage: g.damage,
        flame_damage_interval_ms: 0,
        min_damage: g.min_damage,
        max_damage_range: g.max_damage_range,
        min_damage_range: g.min_damage_range,
        hip_spread_stand_min: g.hip_spread_stand_min,
        hip_spread_ducked_min: g.hip_spread_ducked_min,
        hip_spread_prone_min: g.hip_spread_prone_min,
        hip_spread_stand_max: g.hip_spread_stand_max,
        hip_spread_ducked_max: g.hip_spread_ducked_max,
        hip_spread_prone_max: g.hip_spread_prone_max,
        hip_spread_decay_rate: g.hip_spread_decay_rate,
        hip_spread_fire_add: g.hip_spread_fire_add,
        hip_spread_turn_add: g.hip_spread_turn_add,
        hip_spread_move_add: g.hip_spread_move_add,
        hip_spread_ducked_decay: g.hip_spread_ducked_decay,
        hip_spread_prone_decay: g.hip_spread_prone_decay,
        ads_spread: g.ads_spread,
        aim_down_sight: g.aim_down_sight,
        no_ads_when_mag_empty: g.no_ads_when_mag_empty,
        ads_reload_trans_time_ms: g.ads_reload_trans_time_ms,
        ads_in_rate: g.ads_in_rate,
        ads_out_rate: g.ads_out_rate,
        rechamber_while_ads: g.rechamber_while_ads,
        ads_fire_only: g.ads_fire_only,
        melee_damage: g.melee_damage,
        can_hold_breath: g.overlay_reticle != 0 && g.weap_class != WEAPCLASS_ROCKETLAUNCHER,
        scope_zoom: ScopeZoom::NONE,
        overlay_reticle: g.overlay_reticle,
        melee_time_ms: g.melee_time_ms,
        melee_delay_ms: g.melee_delay_ms,
        melee_charge_time_ms: g.melee_charge_time_ms,
        melee_charge_delay_ms: g.melee_charge_delay_ms,
        melee_charge_anim: false,
        knife_model: g.knife_model,
        quick_raise_time_ms: g.quick_raise_time_ms,
        quick_drop_time_ms: g.quick_drop_time_ms,
        select_requires_ammo: g.select_requires_ammo,
        offhand_hold_is_cancelable: g.offhand_hold_is_cancelable,
        ads_gun_kick_reduced_kick_bullets: g.kick.ads_gun_kick_reduced_kick_bullets,
        hip_gun_kick_reduced_kick_bullets: g.kick.hip_gun_kick_reduced_kick_bullets,
        location_damage: g.location_damage_mult.unwrap_or(LOCATION_DAMAGE_IDENTITY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_table_parses_and_rejects_junk() {
        let mut text = br"LOCDMGTABLE\head\1.5\torso_upper\1.2".to_vec();
        text.push(0);
        let t = parse_lochit(&text, false).expect("parses");
        assert_eq!(t[2], 1.5);
        assert_eq!(t[4], 1.2);
        assert_eq!(t[0], 1.0);
        assert!(parse_lochit(br"LOCDMGTABLE\elbow\2", false).is_none());
        assert!(parse_lochit(br"LOCDMGTABLE\head", false).is_none());
        assert!(parse_lochit(b"something", false).is_none());
    }
}

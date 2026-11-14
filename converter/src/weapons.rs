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

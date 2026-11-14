//! MW2's perks for a class: the three chosen perks and, with everything unlocked, their Pro perks (perkTable's
//! upgrade column, as `_class.gsc` gives them). What they do:
//!
//! - Engine perks become IW4's player-state perk bits (IW4L `sim::script_player::perk_bits`), which IW4L's movement
//!   and weapon code read: Marathon (unlimited sprint) and its Pro (faster mantle), Sleight of Hand (half reload
//!   time) and its Pro (faster ADS), Steady Aim (tighter hip spread) and its Pro (longer breath hold), Lightweight's
//!   Pro (faster sprint recovery), Commando (melee lunge range), Cold-Blooded's and Ninja's flags.
//! - Script perks are applied here as MW2's scripts and engine do: Lightweight's move speed (`_perkfunctions.gsc`,
//!   read into the match rules) and Stopping Power's extra bullet damage (the engine's `perk_bulletDamage`, 40%).

use alloc::vec::Vec;

use wpk::classes::{Catalog, Loadout, NONE};

/// Stopping Power: bullet damage times 1.4 (IW4's `perk_bulletDamage` dvar, 40 percent).
pub const STOPPING_POWER_SCALE: f32 = 1.4;

/// IW4 player-state perk bits for perk `name` (`perks[0]`, `perks[1]`), as IW4L's `setPerk` maps them; 0 for perks
/// the scripts handle.
pub fn bits(name: &str) -> [u32; 2] {
    let perk = match name {
        "specialty_fastreload" => weapon_iw4::PERK_FASTRELOAD,
        "specialty_coldblooded" => playerstate_iw4::PERK_COLDBLOODED,
        "specialty_lightweight" => weapon_iw4::PERK_LIGHTWEIGHT_VIEW_BOB_BIT,
        "specialty_scavenger" => playerstate_iw4::PERK_SCAVENGER,
        "specialty_quieter" => playerstate_iw4::PERK_QUIETER,
        "specialty_heartbreaker" => playerstate_iw4::PERK_HEARTBREAKER,
        "specialty_marathon" => movement_iw4::PERK_MARATHON,
        "specialty_bulletaccuracy" => weapon_iw4::PERK_BULLETACCURACY,
        "specialty_pistoldeath" => playerstate_iw4::PERK_PISTOLDEATH,
        "specialty_fastmantle" => playerstate_iw4::PERK_FASTMANTLE,
        "specialty_quickdraw" => playerstate_iw4::PERK_QUICKDRAW,
        "specialty_holdbreath" => playerstate_iw4::PERK_HOLDBREATH,
        "specialty_selectivehearing" => playerstate_iw4::PERK_SELECTIVEHEARING,
        _ => 0,
    };
    let perk1 = match name {
        "specialty_spygame" => playerstate_iw4::PERK1_SPYGAME,
        "specialty_extendedmelee" => playerstate_iw4::PERK1_EXTENDEDMELEE,
        "specialty_fastsprintrecovery" => playerstate_iw4::PERK1_FASTSPRINTRECOVERY,
        _ => 0,
    };
    [perk, perk1]
}

/// A class's perks, resolved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerkSet {
    /// Every perk the class has, Pro perks included (script names).
    pub names: Vec<alloc::string::String>,
    /// IW4 `ps.perks`.
    pub bits: [u32; 2],
    pub stopping_power: bool,
    pub lightweight: bool,
}

impl PerkSet {
    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// Bullet damage scale for this class's shots.
    pub fn bullet_scale(&self) -> f32 {
        if self.stopping_power {
            STOPPING_POWER_SCALE
        } else {
            1.0
        }
    }
}

/// The perks of class `l`: its three perks and their Pro perks (everything is unlocked).
pub fn of_loadout(cat: &Catalog, l: &Loadout) -> PerkSet {
    let mut set = PerkSet::default();
    for &p in &l.perks {
        if p == NONE {
            continue;
        }
        let Some(item) = cat.perks.get(usize::from(p)) else {
            continue;
        };
        for name in [&item.name, &item.upgrade] {
            if !name.is_empty() && !set.has(name) {
                set.names.push(name.clone());
            }
        }
    }
    for n in &set.names {
        let b = bits(n);
        set.bits[0] |= b[0];
        set.bits[1] |= b[1];
    }
    set.stopping_power = set.has("specialty_bulletdamage");
    set.lightweight = set.has("specialty_lightweight");
    set
}

/// IW4 weapon classes (`weapClass_t`) to `_weapons.gsc`'s `updateMoveSpeedScale` classes, as indices into
/// `wpk::rules::Rules::class_speed` (rifle, pistol, mg, smg, spread, rocketlauncher, sniper, riot shield).
pub fn speed_class(weap_class: i32, riot_shield: bool) -> Option<usize> {
    if riot_shield {
        return Some(7);
    }
    match weap_class {
        0 => Some(0),
        5 => Some(1),
        2 => Some(2),
        3 => Some(3),
        4 => Some(4),
        7 => Some(5),
        1 => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use wpk::classes::{Item, slot};

    fn perk(name: &str, kind: u8, upgrade: &str) -> Item {
        Item {
            name: name.to_string(),
            display: name.to_string(),
            kind,
            attachments: 0,
            upgrade: upgrade.to_string(),
        }
    }

    #[test]
    fn a_class_gets_its_perks_and_their_pro_perks() {
        let cat = Catalog {
            perks: vec![
                perk("specialty_marathon", slot::PERK1, "specialty_fastmantle"),
                perk(
                    "specialty_bulletdamage",
                    slot::PERK2,
                    "specialty_armorpiercing",
                ),
                perk(
                    "specialty_extendedmelee",
                    slot::PERK3,
                    "specialty_falldamage",
                ),
            ],
            ..Catalog::default()
        };
        let l = Loadout {
            perks: [0, 1, 2],
            ..Loadout::default()
        };
        let s = of_loadout(&cat, &l);
        assert!(s.has("specialty_fastmantle") && s.has("specialty_armorpiercing"));
        assert_ne!(s.bits[0] & movement_iw4::PERK_MARATHON, 0);
        assert_ne!(s.bits[0] & playerstate_iw4::PERK_FASTMANTLE, 0);
        assert_ne!(s.bits[1] & playerstate_iw4::PERK1_EXTENDEDMELEE, 0);
        assert!(s.stopping_power && !s.lightweight);
        assert_eq!(s.bullet_scale(), 1.4);
        assert_eq!(of_loadout(&cat, &Loadout::default()), PerkSet::default());
        assert_eq!(speed_class(2, false), Some(2));
        assert_eq!(speed_class(0, true), Some(7));
        assert_eq!(speed_class(6, false), None);
    }
}

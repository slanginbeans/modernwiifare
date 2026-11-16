//! MW2's perks for a class: the three chosen perks and, with everything unlocked, their Pro perks (perkTable's
//! upgrade column, as `_class.gsc` gives them). What they do:
//!
//! - Engine perks become IW4's player-state perk bits (IW4L `sim::script_player::perk_bits`), which IW4L's movement
//!   and weapon code read: Marathon (unlimited sprint) and its Pro (faster mantle), Sleight of Hand (half reload
//!   time) and its Pro (faster ADS), Steady Aim (tighter hip spread) and its Pro (longer breath hold), Lightweight's
//!   Pro (faster sprint recovery), Commando (melee lunge range), Cold-Blooded's and Ninja's flags.
//! - Script perks are applied here as MW2's scripts and engine do: Lightweight's move speed (`_perkfunctions.gsc`,
//!   read into the match rules) and Stopping Power's extra bullet damage (the engine's `perk_bulletDamage`, 40%).
//! - The rest (`crate::arena` applies them, the numbers read from the user's scripts into `wpk::life_rules`):
//!   Cold-Blooded off the UAV's radar (IW4L `hud_iw4::radar_contact_trail_visible` on the perk bits), Scrambler's
//!   radar jam (the `RADAR_JAM` entity flag, IW4L `perk_bits`; `hud_iw4::radar_jam_intensity`), Ninja Pro's silent
//!   steps, Hardline's cheaper killstreaks ([`streak_cost`]) and its Pro's earlier deathstreaks, Last Stand
//!   ([`may_last_stand`], [`enter_last_stand`]: IW4L `sim::script_player`), Bomb Squad's view of enemy explosives,
//!   Scrambler Pro's claymore delay, One Man Army's class change, and the deathstreaks ([`Deathstreak`]).

use alloc::vec::Vec;

use playerstate_iw4::PlayerState;
use wpk::classes::{Catalog, Loadout, NONE, slot};

/// MW2's deathstreaks (perkTable `perk4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deathstreak {
    /// `specialty_copycat`: copy your killer's class for the next life.
    Copycat,
    /// `specialty_combathigh`: a third of the damage for a while after spawning.
    Painkiller,
    /// `specialty_grenadepulldeath`: drop a live grenade on death.
    Martyrdom,
    /// `specialty_finalstand`: go down in last stand, then get back up.
    FinalStand,
}

impl Deathstreak {
    pub fn of_name(name: &str) -> Option<Deathstreak> {
        Some(match name {
            "specialty_copycat" => Deathstreak::Copycat,
            "specialty_combathigh" => Deathstreak::Painkiller,
            "specialty_grenadepulldeath" => Deathstreak::Martyrdom,
            "specialty_finalstand" => Deathstreak::FinalStand,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Deathstreak::Copycat => "specialty_copycat",
            Deathstreak::Painkiller => "specialty_combathigh",
            Deathstreak::Martyrdom => "specialty_grenadepulldeath",
            Deathstreak::FinalStand => "specialty_finalstand",
        }
    }

    /// MW2's stock deaths in a row (perkTable column 6), for packs without the column.
    pub fn stock_deaths(self) -> u32 {
        match self {
            Deathstreak::Painkiller => 3,
            _ => 4,
        }
    }
}

/// How damage was dealt (MW2's `sMeansOfDeath`, as far as the rules below need it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Means {
    /// `MOD_RIFLE_BULLET` / `MOD_PISTOL_BULLET`.
    Bullet,
    /// `MOD_MELEE`.
    Melee,
    /// `MOD_GRENADE_SPLASH`, `MOD_EXPLOSIVE` ...
    Explosive,
    /// `MOD_FALLING`.
    Falling,
    /// `MOD_IMPACT` of a thrown knife (`throwingknife_mp`).
    Knife,
    /// Anything else (the nuke, `MOD_TRIGGER_HURT` ...).
    Other,
}

/// Whether a killing blow sends a Last Stand player down instead (`_damage.gsc` `mayDoLastStand`): bullets and
/// falls only, never a headshot.
pub fn may_last_stand(means: Means, headshot: bool) -> bool {
    matches!(means, Means::Bullet | Means::Falling) && !headshot
}

/// Go down in last stand (IW4L `sim::script_player` finish damage with `PERK_PISTOLDEATH`): health 1, the last
/// stand player type, its view height and flag (pmove crawls at `player_lastStandCrawlSpeedScale`).
pub fn enter_last_stand(ps: &mut PlayerState) {
    ps.health = 1;
    ps.pm_type = playerstate_iw4::PM_TYPE_LAST_STAND;
    ps.view_height_target = movement_iw4::view_height::LAST_STAND;
    ps.pm_flags |= playerstate_iw4::pm_flags::LAST_STAND;
}

/// Get back up from last stand (IW4L `sim::script_player::revive`): a normal player, crouched.
pub fn revive(ps: &mut PlayerState) {
    ps.pm_type = 0;
    ps.pm_flags &= !playerstate_iw4::pm_flags::LAST_STAND;
    ps.view_height_target = movement_iw4::view_height::CROUCH;
}

/// Whether `ps` is in last stand.
pub fn in_last_stand(ps: &PlayerState) -> bool {
    ps.pm_flags & playerstate_iw4::pm_flags::LAST_STAND != 0
}

/// A killstreak's kills with or without Hardline (`_class.gsc` `setKillstreaks`: the cost plus `modifier`, -1 with
/// `specialty_hardline`), at least one.
pub fn streak_cost(cost: u8, hardline: bool, modifier: i32) -> u32 {
    let c = i32::from(cost) + if hardline { modifier } else { 0 };
    c.max(1) as u32
}

/// Painkiller's damage (`_perks.gsc` `cac_modified_damage`: divided by `divisor` except falls, melee and the
/// thrown knife).
pub fn painkiller_damage(damage: i32, means: Means, divisor: f32) -> i32 {
    match means {
        Means::Falling | Means::Melee | Means::Knife => damage,
        _ => (damage as f32 / divisor.max(1.0)) as i32,
    }
}

/// Whether weapon `name` is silenced (a silencer attachment: `usp_silencer_mp`, `m4_acog_silencer_mp`): its shots
/// don't show on the enemies' radar. (IW4 reads the weapon file's `silenced`; the packed records don't carry it, and
/// every silenced MW2 multiplayer weapon is a `silencer` variant.)
pub fn silenced(name: &str) -> bool {
    name.split('_').any(|p| p == "silencer")
}

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
    /// Danger Close (`specialty_explosivedamage`).
    pub danger_close: bool,
    /// Cold-Blooded (`specialty_coldblooded`): off the UAV's radar.
    pub cold_blooded: bool,
    /// Ninja Pro (`specialty_quieter`): silent footsteps.
    pub quieter: bool,
    /// Scrambler (`specialty_localjammer`): jams nearby enemies' radar; its Pro (`specialty_delaymine`) slows
    /// claymores tripped by you.
    pub scrambler: bool,
    pub delay_mine: bool,
    /// Hardline (`specialty_hardline`) and its Pro (`specialty_rollover`: deathstreaks a death sooner).
    pub hardline: bool,
    pub rollover: bool,
    /// One Man Army (`specialty_onemanarmy`) and its Pro (`specialty_omaquickchange`).
    pub one_man_army: bool,
    pub oma_quick: bool,
    /// Last Stand (`specialty_pistoldeath`) and its Pro (`specialty_laststandoffhand`: grenades while down).
    pub last_stand: bool,
    pub last_stand_offhand: bool,
    /// Bomb Squad (`specialty_detectexplosive`): enemy explosives shown.
    pub bomb_squad: bool,
    /// The class's deathstreak and the deaths in a row it needs.
    pub deathstreak: Option<(Deathstreak, u32)>,
}

impl PerkSet {
    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// The deathstreak due after `deaths` deaths in a row without a kill (`_class.gsc` `giveLoadout`: at or past the
    /// deathstreak's count, one sooner with Hardline Pro).
    pub fn deathstreak_due(&self, deaths: u32) -> Option<Deathstreak> {
        let (d, need) = self.deathstreak?;
        let need = if self.rollover {
            need.saturating_sub(1)
        } else {
            need
        };
        (need > 0 && deaths >= need).then_some(d)
    }

    /// IW4 entity flags the perks set (Scrambler's `RADAR_JAM`, IW4L `perk_bits`).
    pub fn e_flags(&self) -> u32 {
        if self.scrambler {
            playerstate_iw4::eflags::RADAR_JAM
        } else {
            0
        }
    }

    /// Bullet damage scale for this class's shots (Stopping Power: `_perks.gsc` adds `perk_bulletDamage` percent).
    pub fn bullet_scale(&self, rules: &wpk::rules::Rules) -> f32 {
        if self.stopping_power {
            1.0 + rules.perk_bullet_damage
        } else {
            1.0
        }
    }

    /// Explosive damage scale for this class's grenades and charges (Danger Close: `perk_explosiveDamage` percent
    /// more, `_perks.gsc` `cac_modified_damage`).
    pub fn explosive_scale(&self, rules: &wpk::rules::Rules) -> f32 {
        if self.danger_close {
            1.0 + rules.perk_explosive_damage
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
    set.danger_close = set.has("specialty_explosivedamage");
    set.cold_blooded = set.has("specialty_coldblooded");
    set.quieter = set.has("specialty_quieter");
    set.scrambler = set.has("specialty_localjammer");
    set.delay_mine = set.has("specialty_delaymine");
    set.hardline = set.has("specialty_hardline");
    set.rollover = set.has("specialty_rollover");
    set.one_man_army = set.has("specialty_onemanarmy");
    set.oma_quick = set.has("specialty_omaquickchange");
    set.last_stand = set.has("specialty_pistoldeath");
    set.last_stand_offhand = set.has("specialty_laststandoffhand");
    set.bomb_squad = set.has("specialty_detectexplosive");
    set.deathstreak = cat
        .perks
        .get(usize::from(l.deathstreak))
        .filter(|p| p.kind == slot::DEATHSTREAK)
        .and_then(|p| {
            let d = Deathstreak::of_name(&p.name)?;
            let need = if p.attachments > 0 {
                p.attachments
            } else {
                d.stock_deaths()
            };
            Some((d, need))
        });
    set
}

/// The perks of a copied class (Copycat: `_class.gsc` keeps `specialty_copycat` as the deathstreak).
pub fn copied(mut theirs: PerkSet, mine: &PerkSet) -> PerkSet {
    theirs.deathstreak = mine.deathstreak;
    theirs
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
        assert_eq!(s.bullet_scale(&wpk::rules::Rules::default()), 1.4);
        assert_eq!(of_loadout(&cat, &Loadout::default()), PerkSet::default());
        assert_eq!(speed_class(2, false), Some(2));
        assert_eq!(speed_class(0, true), Some(7));
        assert_eq!(speed_class(6, false), None);
    }

    #[test]
    fn script_perks_deathstreaks_and_last_stand_follow_mw2() {
        let mut ds = perk("specialty_combathigh", slot::DEATHSTREAK, "");
        ds.attachments = 3;
        let cat = Catalog {
            perks: vec![
                perk("specialty_coldblooded", slot::PERK3, "specialty_spygame"),
                perk("specialty_hardline", slot::PERK2, "specialty_rollover"),
                perk(
                    "specialty_pistoldeath",
                    slot::PERK3,
                    "specialty_laststandoffhand",
                ),
                ds,
                perk("specialty_localjammer", slot::PERK3, "specialty_delaymine"),
                perk("specialty_copycat", slot::DEATHSTREAK, ""),
            ],
            ..Catalog::default()
        };
        let l = Loadout {
            perks: [1, 0, 2],
            deathstreak: 3,
            ..Loadout::default()
        };
        let s = of_loadout(&cat, &l);
        assert!(s.cold_blooded && s.hardline && s.rollover && s.last_stand && s.last_stand_offhand);
        assert_ne!(s.bits[0] & playerstate_iw4::PERK_COLDBLOODED, 0);
        assert_ne!(s.bits[1] & playerstate_iw4::PERK1_SPYGAME, 0);
        // Cold-Blooded's bit takes the player off the radar's trail (IW4L's compass rule).
        assert!(!hud_iw4::radar_contact_trail_visible(s.bits[0]));
        assert!(hud_iw4::radar_contact_trail_visible(0));
        // Painkiller after 3 deaths, one sooner with Hardline Pro.
        assert_eq!(s.deathstreak, Some((Deathstreak::Painkiller, 3)));
        assert_eq!(s.deathstreak_due(1), None);
        assert_eq!(s.deathstreak_due(2), Some(Deathstreak::Painkiller));
        assert_eq!(s.deathstreak_due(7), Some(Deathstreak::Painkiller));
        // Copycat without perkTable's column: MW2's stock 4.
        let c = of_loadout(
            &cat,
            &Loadout {
                deathstreak: 5,
                perks: [4, NONE, NONE],
                ..Loadout::default()
            },
        );
        assert_eq!(c.deathstreak, Some((Deathstreak::Copycat, 4)));
        assert_eq!(c.deathstreak_due(3), None);
        assert!(c.scrambler && c.delay_mine);
        assert_eq!(c.e_flags(), playerstate_iw4::eflags::RADAR_JAM);
        assert_eq!(copied(s.clone(), &c).deathstreak, c.deathstreak);
        // Hardline: a kill sooner (`modifier = -1`), never under one.
        assert_eq!(streak_cost(3, true, -1), 2);
        assert_eq!(streak_cost(3, false, -1), 3);
        assert_eq!(streak_cost(1, true, -1), 1);
        // Last stand: bullets and falls, not headshots, melee or explosions.
        assert!(may_last_stand(Means::Bullet, false));
        assert!(may_last_stand(Means::Falling, false));
        assert!(!may_last_stand(Means::Bullet, true));
        assert!(!may_last_stand(Means::Explosive, false));
        assert!(!may_last_stand(Means::Melee, false));
        let mut ps = PlayerState::ZERO;
        ps.health = 30;
        enter_last_stand(&mut ps);
        assert!(in_last_stand(&ps));
        assert_eq!(
            (ps.health, ps.pm_type, ps.view_height_target),
            (1, 7, movement_iw4::view_height::LAST_STAND)
        );
        revive(&mut ps);
        assert!(!in_last_stand(&ps) && ps.pm_type == 0);
        // Painkiller: a third, except falls, melee and knives.
        assert_eq!(painkiller_damage(90, Means::Bullet, 3.0), 30);
        assert_eq!(painkiller_damage(90, Means::Explosive, 3.0), 30);
        assert_eq!(painkiller_damage(90, Means::Melee, 3.0), 90);
        assert!(silenced("usp_silencer_mp") && silenced("m4_acog_silencer_mp"));
        assert!(!silenced("m4_mp") && !silenced("silencerless"));
    }
}

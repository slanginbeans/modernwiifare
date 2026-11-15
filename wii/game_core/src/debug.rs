//! Debug settings for testing on hardware: match overrides, visual overlays and performance switches, kept in the
//! app folder (`debug.dat`) between runs. The menu that edits them (start screen and pause) lists [`ITEMS`]; each
//! item cycles through its values with A.

use alloc::string::String;
use alloc::vec::Vec;

use crate::bot::Skill;

/// File format version (first byte of `debug.dat`).
const VERSION: u8 = 2;

/// Draw distances (world units, inches) the debug menu steps through; 0 is the full far plane, [`AUTO`] follows the
/// frame time (`auto_distance`).
pub const DRAW_DISTANCES: [u16; 6] = [0, 6000, 4000, 3000, 2000, AUTO];

/// The AUTO draw distance in [`DRAW_DISTANCES`].
pub const AUTO: u16 = u16::MAX;

/// Model detail: the static-model LOD distance scale of each step (1 is the PC game's LOD distances).
pub const MODEL_DETAIL: [(f32, &str); 4] = [
    (1.0, "NORMAL"),
    (1.5, "LOW"),
    (2.0, "LOWER"),
    (3.0, "LOWEST"),
];

/// The most bots a match can have from the debug menu (the arena's fighter slots, the Wii's CPU permitting).
pub const MAX_BOTS: u8 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugSettings {
    // Match overrides.
    pub bots: u8,
    /// 0 easy, 1 regular, 2 hard.
    pub bot_skill: u8,
    /// Bots never shoot, swing or throw.
    pub bots_passive: bool,
    /// Bots stand still and don't think (they can still be shot).
    pub bots_frozen: bool,
    /// You take no damage.
    pub god_mode: bool,
    /// Magazine, reserve and grenades refilled every frame.
    pub infinite_ammo: bool,
    /// Every killstreak reward is yours from the start of each life.
    pub all_killstreaks: bool,
    /// Your primary weapon replaced by this catalog weapon (`wpk::classes::NONE`: the class's).
    pub primary: u8,
    // Visual overlays.
    pub show_hitboxes: bool,
    pub show_bot_goals: bool,
    pub show_objectives: bool,
    pub show_shots: bool,
    /// Draw characters (off: the placeholder boxes).
    pub characters: bool,
    // Performance.
    pub perf_overlay: bool,
    pub world_models: bool,
    pub sound: bool,
    /// Index into [`DRAW_DISTANCES`].
    pub draw_distance: u8,
    /// Index into [`MODEL_DETAIL`].
    pub model_detail: u8,
    /// The map's own fog (MW2's `setExpFog`); a shortened draw distance fogs regardless.
    pub map_fog: bool,
    // Feel comparisons (default off: the pinned IW4L's values).
    /// Recoil kick at the amplitudes of IW4L's `major-overhaul` branch (camera 0.040, gun 0.030: twice the pinned
    /// revision's 0.020 / 0.015), to compare against the user's copy before adopting it.
    pub kick_x2: bool,
}

impl Default for DebugSettings {
    fn default() -> Self {
        DebugSettings {
            bots: 5,
            bot_skill: 1,
            bots_passive: false,
            bots_frozen: false,
            god_mode: false,
            infinite_ammo: false,
            all_killstreaks: false,
            primary: wpk::classes::NONE,
            show_hitboxes: false,
            show_bot_goals: false,
            show_objectives: false,
            show_shots: false,
            characters: true,
            perf_overlay: false,
            world_models: true,
            sound: true,
            draw_distance: 0,
            model_detail: 0,
            map_fog: true,
            kick_x2: false,
        }
    }
}

/// One line of the debug menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Bots,
    BotSkill,
    BotsPassive,
    BotsFrozen,
    GodMode,
    InfiniteAmmo,
    AllKillstreaks,
    Primary,
    ShowHitboxes,
    ShowBotGoals,
    ShowObjectives,
    ShowShots,
    Characters,
    PerfOverlay,
    WorldModels,
    DrawDistance,
    ModelDetail,
    MapFog,
    Sound,
    KickAmplitude,
    ResetDefaults,
}

/// The debug page's lines (the bot count and difficulty are in Game Setup).
pub const ITEMS: [Item; 19] = [
    Item::BotsPassive,
    Item::BotsFrozen,
    Item::GodMode,
    Item::InfiniteAmmo,
    Item::AllKillstreaks,
    Item::Primary,
    Item::ShowHitboxes,
    Item::ShowBotGoals,
    Item::ShowObjectives,
    Item::ShowShots,
    Item::Characters,
    Item::PerfOverlay,
    Item::WorldModels,
    Item::DrawDistance,
    Item::ModelDetail,
    Item::MapFog,
    Item::Sound,
    Item::KickAmplitude,
    Item::ResetDefaults,
];

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::Bots => "BOTS",
            Item::BotSkill => "BOT SKILL",
            Item::BotsPassive => "BOTS DON'T ATTACK",
            Item::BotsFrozen => "BOTS FROZEN",
            Item::GodMode => "GOD MODE",
            Item::InfiniteAmmo => "INFINITE AMMO",
            Item::AllKillstreaks => "ALL KILLSTREAKS",
            Item::Primary => "PRIMARY WEAPON",
            Item::ShowHitboxes => "SHOW HIT BOXES",
            Item::ShowBotGoals => "SHOW BOT GOALS",
            Item::ShowObjectives => "SHOW OBJECTIVES",
            Item::ShowShots => "SHOW BULLET PATHS",
            Item::Characters => "CHARACTER MODELS",
            Item::PerfOverlay => "PERF OVERLAY",
            Item::WorldModels => "WORLD MODELS",
            Item::DrawDistance => "DRAW DISTANCE",
            Item::ModelDetail => "MODEL DETAIL",
            Item::MapFog => "MAP FOG",
            Item::Sound => "SOUND",
            Item::KickAmplitude => "RECOIL KICK",
            Item::ResetDefaults => "RESET TO DEFAULTS",
        }
    }

    /// Whether the item only applies when a match starts (greyed out in the pause menu).
    pub fn at_start_only(self) -> bool {
        matches!(self, Item::Bots | Item::BotSkill)
    }
}

fn on_off(b: bool) -> String {
    String::from(if b { "ON" } else { "OFF" })
}

impl DebugSettings {
    /// The value shown for `item`; `weapon_name` names a catalog weapon.
    pub fn value(&self, item: Item, weapon_name: impl Fn(u8) -> Option<String>) -> Option<String> {
        Some(match item {
            Item::Bots => alloc::format!("{}", self.bots),
            Item::BotSkill => {
                String::from(["EASY", "REGULAR", "HARD"][usize::from(self.bot_skill.min(2))])
            }
            Item::BotsPassive => on_off(self.bots_passive),
            Item::BotsFrozen => on_off(self.bots_frozen),
            Item::GodMode => on_off(self.god_mode),
            Item::InfiniteAmmo => on_off(self.infinite_ammo),
            Item::AllKillstreaks => on_off(self.all_killstreaks),
            Item::Primary => {
                if self.primary == wpk::classes::NONE {
                    String::from("CLASS")
                } else {
                    weapon_name(self.primary).unwrap_or_else(|| String::from("?"))
                }
            }
            Item::ShowHitboxes => on_off(self.show_hitboxes),
            Item::ShowBotGoals => on_off(self.show_bot_goals),
            Item::ShowObjectives => on_off(self.show_objectives),
            Item::ShowShots => on_off(self.show_shots),
            Item::Characters => on_off(self.characters),
            Item::PerfOverlay => on_off(self.perf_overlay),
            Item::WorldModels => on_off(self.world_models),
            Item::DrawDistance => match self.draw_distance_units() {
                0 => String::from("FULL"),
                AUTO => String::from("AUTO"),
                d => alloc::format!("{d}"),
            },
            Item::ModelDetail => String::from(MODEL_DETAIL[self.detail_index()].1),
            Item::MapFog => on_off(self.map_fog),
            Item::Sound => on_off(self.sound),
            Item::KickAmplitude => String::from(if self.kick_x2 {
                "IW4L NEW (X2)"
            } else {
                "IW4L PINNED"
            }),
            Item::ResetDefaults => return None,
        })
    }

    /// Step `item` to its next value (`weapons`: how many catalog weapons there are).
    pub fn cycle(&mut self, item: Item, weapons: u8) {
        let flip = |b: &mut bool| *b = !*b;
        match item {
            Item::Bots => self.bots = (self.bots + 1) % (MAX_BOTS + 1),
            Item::BotSkill => self.bot_skill = (self.bot_skill + 1) % 3,
            Item::BotsPassive => flip(&mut self.bots_passive),
            Item::BotsFrozen => flip(&mut self.bots_frozen),
            Item::GodMode => flip(&mut self.god_mode),
            Item::InfiniteAmmo => flip(&mut self.infinite_ammo),
            Item::AllKillstreaks => flip(&mut self.all_killstreaks),
            Item::Primary => {
                self.primary = if weapons == 0 {
                    wpk::classes::NONE
                } else if self.primary == wpk::classes::NONE {
                    0
                } else if u16::from(self.primary) + 1 >= u16::from(weapons) {
                    wpk::classes::NONE
                } else {
                    self.primary + 1
                };
            }
            Item::ShowHitboxes => flip(&mut self.show_hitboxes),
            Item::ShowBotGoals => flip(&mut self.show_bot_goals),
            Item::ShowObjectives => flip(&mut self.show_objectives),
            Item::ShowShots => flip(&mut self.show_shots),
            Item::Characters => flip(&mut self.characters),
            Item::PerfOverlay => flip(&mut self.perf_overlay),
            Item::WorldModels => flip(&mut self.world_models),
            Item::DrawDistance => {
                self.draw_distance = (self.draw_distance + 1) % DRAW_DISTANCES.len() as u8
            }
            Item::ModelDetail => {
                self.model_detail = (self.model_detail + 1) % MODEL_DETAIL.len() as u8
            }
            Item::MapFog => flip(&mut self.map_fog),
            Item::Sound => flip(&mut self.sound),
            Item::KickAmplitude => flip(&mut self.kick_x2),
            Item::ResetDefaults => *self = DebugSettings::default(),
        }
    }

    /// The draw distance in world units (0: the full far plane; [`AUTO`]: from the frame time).
    pub fn draw_distance_units(&self) -> u16 {
        DRAW_DISTANCES[usize::from(self.draw_distance) % DRAW_DISTANCES.len()]
    }

    fn detail_index(&self) -> usize {
        usize::from(self.model_detail) % MODEL_DETAIL.len()
    }

    /// What the recoil kick's amplitudes are multiplied by ([`DebugSettings::kick_x2`]).
    pub fn kick_scale(&self) -> f32 {
        if self.kick_x2 { 2.0 } else { 1.0 }
    }

    /// The static-model LOD distance scale.
    pub fn lod_scale(&self) -> f32 {
        MODEL_DETAIL[self.detail_index()].0
    }

    /// The bots' skill (debug presets around MW2's regular bot: easy reacts slower and aims worse, hard the reverse).
    pub fn skill(&self) -> Skill {
        match self.bot_skill {
            0 => Skill {
                reaction_ms: Skill::REGULAR.reaction_ms * 2,
                aim_error_deg: Skill::REGULAR.aim_error_deg * 2.0,
            },
            2 => Skill {
                reaction_ms: Skill::REGULAR.reaction_ms / 2,
                aim_error_deg: Skill::REGULAR.aim_error_deg * 0.4,
            },
            _ => Skill::REGULAR,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let flags = [
            self.bots_passive,
            self.bots_frozen,
            self.god_mode,
            self.infinite_ammo,
            self.all_killstreaks,
            self.show_hitboxes,
            self.show_bot_goals,
            self.show_objectives,
            self.show_shots,
            self.characters,
            self.perf_overlay,
            self.world_models,
            self.sound,
            self.map_fog,
            self.kick_x2,
        ];
        let bits = flags
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, &f)| a | (u32::from(f) << i));
        let mut o = alloc::vec![VERSION, self.bots, self.bot_skill];
        o.extend_from_slice(&u16::from(self.primary).to_be_bytes());
        o.extend_from_slice(&bits.to_be_bytes());
        o.extend_from_slice(&[self.draw_distance, self.model_detail]);
        o
    }

    /// Settings from `debug.dat` (None when unreadable or an unknown version: the defaults then). Version 1 files
    /// (before the draw-distance settings) read with those at their defaults.
    pub fn decode(b: &[u8]) -> Option<DebugSettings> {
        let v2 = match b.first() {
            Some(1) if b.len() >= 9 => false,
            Some(2) if b.len() >= 11 => true,
            _ => return None,
        };
        let defaults = DebugSettings::default();
        let bits = u32::from_be_bytes([b[5], b[6], b[7], b[8]]);
        let f = |i: u32| bits & (1 << i) != 0;
        Some(DebugSettings {
            bots: b[1].min(MAX_BOTS),
            bot_skill: b[2].min(2),
            primary: b[4],
            bots_passive: f(0),
            bots_frozen: f(1),
            god_mode: f(2),
            infinite_ammo: f(3),
            all_killstreaks: f(4),
            show_hitboxes: f(5),
            show_bot_goals: f(6),
            show_objectives: f(7),
            show_shots: f(8),
            characters: f(9),
            perf_overlay: f(10),
            world_models: f(11),
            sound: f(12),
            map_fog: if v2 { f(13) } else { defaults.map_fog },
            // A bit older files never set: off.
            kick_x2: f(14),
            draw_distance: if v2 {
                b[9] % DRAW_DISTANCES.len() as u8
            } else {
                defaults.draw_distance
            },
            model_detail: if v2 {
                b[10] % MODEL_DETAIL.len() as u8
            } else {
                defaults.model_detail
            },
        })
    }
}

/// Sandbox actions in the pause menu's debug page (done once, not saved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TeleportSpawn,
    TeleportObjective,
    BotInFront,
    KillBots,
    CallUav,
    CallCounterUav,
    CallEmp,
    CallNuke,
    RefillRewards,
    ResetClock,
    WriteLog,
}

pub const ACTIONS: [Action; 11] = [
    Action::TeleportSpawn,
    Action::TeleportObjective,
    Action::BotInFront,
    Action::KillBots,
    Action::CallUav,
    Action::CallCounterUav,
    Action::CallEmp,
    Action::CallNuke,
    Action::RefillRewards,
    Action::ResetClock,
    Action::WriteLog,
];

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::TeleportSpawn => "TELEPORT: NEXT SPAWN",
            Action::TeleportObjective => "TELEPORT: NEXT OBJECTIVE",
            Action::BotInFront => "BRING A BOT IN FRONT",
            Action::KillBots => "KILL ALL BOTS",
            Action::CallUav => "UAV NOW",
            Action::CallCounterUav => "COUNTER-UAV NOW",
            Action::CallEmp => "EMP NOW",
            Action::CallNuke => "TACTICAL NUKE NOW",
            Action::RefillRewards => "GIVE ALL KILLSTREAKS",
            Action::ResetClock => "RESET MATCH CLOCK",
            Action::WriteLog => "WRITE THE LOG NOW",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_cycle() {
        let mut s = DebugSettings::default();
        assert_eq!(DebugSettings::decode(&s.encode()), Some(s));
        for item in ITEMS {
            s.cycle(item, 3);
            if item != Item::ResetDefaults {
                assert_eq!(DebugSettings::decode(&s.encode()), Some(s), "{item:?}");
            }
        }
        // Reset came last: defaults again.
        assert_eq!(s, DebugSettings::default());
        // Bots wrap 7 -> 0; the primary goes class -> 0 -> 1 -> 2 -> class.
        let mut s = DebugSettings {
            bots: MAX_BOTS,
            ..DebugSettings::default()
        };
        s.cycle(Item::Bots, 3);
        assert_eq!(s.bots, 0);
        let seq: Vec<u8> = (0..4)
            .map(|_| {
                s.cycle(Item::Primary, 3);
                s.primary
            })
            .collect();
        assert_eq!(seq, [0, 1, 2, wpk::classes::NONE]);
        assert!(DebugSettings::decode(&[9, 0, 0]).is_none());
        // A version 1 file keeps the new settings at their defaults.
        let v1 = [1, 2, 0, 0, 255, 0, 0, 0, 1];
        let old = DebugSettings::decode(&v1).unwrap();
        assert!(
            old.bots_passive && old.map_fog && old.draw_distance == 0 && old.lod_scale() == 1.0
        );
        // Draw distance steps full -> 6000 -> ... and wraps.
        let mut d = DebugSettings::default();
        assert_eq!(d.draw_distance_units(), 0);
        d.cycle(Item::DrawDistance, 0);
        assert_eq!(d.draw_distance_units(), DRAW_DISTANCES[1]);
        d.cycle(Item::ModelDetail, 0);
        assert!(d.lod_scale() > 1.0);
        assert!(s.skill().reaction_ms > 0);
        // The kick switch: off by default, doubles when on, survives a save.
        let mut k = DebugSettings::default();
        assert_eq!(k.kick_scale(), 1.0);
        k.cycle(Item::KickAmplitude, 0);
        assert_eq!(k.kick_scale(), 2.0);
        assert_eq!(
            DebugSettings::decode(&k.encode()).unwrap().kick_scale(),
            2.0
        );
    }
}

//! MW2's player animation script (`mp/playeranim.script`) and its anim-type list (`mp/playeranimtypes.txt`), read
//! from the user's zone at convert time (ADR 0007) into [`wpk::anim_script::Script`]. The animations it names become
//! the `PANM` clips.
//!
//! The format (Wolfenstein-style animation script): `DEFINES` (`set <condition> <alias> = a AND b`), then
//! `ANIMATIONS` (`STATE COMBAT { <movetype> { <conditions> { <part> <anim> [duration N] [blendtime N] } } }`),
//! then `EVENTS` (`<event> { <conditions> { ... } }`). Conditions are comma separated, `<name> [values]`, where
//! values are names joined by `AND`, `all` is every bit and `NOT` removes the names after it; `default` has none.
//! Names come from IW4L's `anim_iw4` tables (IW4's own).
//!
//! Items that can never match in Modern Wiifare (mounted turrets, last stand, anim types no weapon has) are dropped:
//! the first item that matches is unchanged, and their animations aren't baked.

use std::collections::HashMap;

use anim_iw4::{
    ANIM_COND_AKIMBO, ANIM_COND_IS_BITFLAGS, ANIM_COND_MOUNTED, ANIM_COND_MOVETYPE,
    ANIM_COND_NAMES, ANIM_COND_PERK, ANIM_COND_PLAYERANIMTYPE, ANIM_COND_PLAYERANIMTYPEPRIMARY,
    ANIM_COND_RIOTSHIELDNEXT, ANIM_ET_NAMES, ANIM_MT_NAMES, anim_cond_evaluable,
    anim_cond_null_value_defaults_to_one, anim_cond_value_names,
};
use wpk::anim_script::{Command, Condition, Item, NO_BLEND, Script, part};

pub const SCRIPT: &str = "mp/playeranim.script";
pub const TYPES: &str = "mp/playeranimtypes.txt";
/// The rawfiles read.
pub const SOURCES: [&str; 2] = [SCRIPT, TYPES];

/// Movement types our players never have: climbing (no ladders), the mantle tree's root (never a movement type:
/// a mantle plays `MANTLE_UP_*` then `MANTLE_OVER_*`, `game_core::mantle_path::anim_move_type`), the dive and
/// last-stand types (no such perks).
fn movetype_possible(mt: usize) -> bool {
    !matches!(mt, 18 | 19 | 21 | 43..=50 | 51..=53)
}

/// Whether clip `clip` keeps its playback rate at 1 (IW4L `asset_anim::playeranim_parse::animation_properties`
/// `stationary`): a legs or both command of a mantle movement type, or any command of the death or reload events.
pub fn stationary(script: &Script, clip: u16) -> bool {
    let moves = script.moves.iter().any(|(mt, items)| {
        (22..32).contains(mt)
            && items
                .iter()
                .flat_map(|i| &i.commands)
                .any(|c| c.clip == clip && c.part != part::TORSO)
    });
    let events = script.events.iter().any(|(ev, items)| {
        matches!(*ev, 1 | 10)
            && items
                .iter()
                .flat_map(|i| &i.commands)
                .any(|c| c.clip == clip)
    });
    moves || events
}

/// What the parse found.
#[derive(Debug, Default)]
pub struct Parsed {
    pub script: Script,
    /// The animations the commands play, in clip order (lower case).
    pub anims: Vec<String>,
    pub items: usize,
    pub dropped_items: usize,
    pub skipped_items: usize,
    pub unresolved: usize,
}

fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        let mut cur = String::new();
        for ch in line.chars() {
            match ch {
                '{' | '}' | ',' => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    out.push(ch.to_string());
                }
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

fn index_ci(name: &str, names: &[&str]) -> Option<usize> {
    names.iter().position(|n| n.eq_ignore_ascii_case(name))
}

struct Ctx<'a> {
    types: Vec<String>,
    aliases: HashMap<(u8, String), u64>,
    used_types: u64,
    exists: &'a dyn Fn(&str) -> bool,
}

impl Ctx<'_> {
    fn value_names(&self, cond: u8) -> Vec<String> {
        if cond == ANIM_COND_PLAYERANIMTYPE || cond == ANIM_COND_PLAYERANIMTYPEPRIMARY {
            self.types.clone()
        } else {
            anim_cond_value_names(cond)
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect()
        }
    }

    /// The bits named by `words` (`a AND b`, `all`, `NOT x`, aliases) for bit-flag condition `cond`.
    fn bits(&self, cond: u8, words: &[String]) -> Option<u64> {
        let names = self.value_names(cond);
        let (mut bits, mut not) = (0u64, false);
        for w in words {
            let w = w.to_ascii_lowercase();
            if w == "and" {
                continue;
            }
            if w == "not" {
                not = true;
                continue;
            }
            let b = if w == "all" {
                u64::MAX
            } else if let Some(&a) = self.aliases.get(&(cond, w.clone())) {
                a
            } else {
                1u64 << names.iter().position(|n| *n == w)?
            };
            if not {
                bits &= !b;
            } else {
                bits |= b;
            }
        }
        Some(bits)
    }

    /// One condition from `name values...`; `None` when it isn't understood.
    fn condition(&self, group: &[String]) -> Option<Condition> {
        let index = index_ci(&group[0], ANIM_COND_NAMES)? as u8;
        let words = &group[1..];
        if ANIM_COND_IS_BITFLAGS[usize::from(index)] {
            return Some(Condition {
                index,
                bitflags: true,
                bits: self.bits(index, words)?,
                value: 0,
            });
        }
        let value = match words.first().map(|w| w.to_ascii_lowercase()) {
            None if anim_cond_null_value_defaults_to_one(index) => 1,
            None => return None,
            Some(w) if w == "true" => 1,
            Some(w) if w == "false" => 0,
            Some(w) => self.value_names(index).iter().position(|n| *n == w)? as i32,
        };
        Some(Condition {
            index,
            bitflags: false,
            bits: 0,
            value,
        })
    }

    /// Whether `c` can hold for one of our players.
    fn possible(&self, c: &Condition) -> bool {
        match c.index {
            ANIM_COND_PLAYERANIMTYPE | ANIM_COND_PLAYERANIMTYPEPRIMARY => {
                c.bits & self.used_types != 0
            }
            ANIM_COND_MOVETYPE => (0..64).any(|m| c.bits & (1 << m) != 0 && movetype_possible(m)),
            ANIM_COND_MOUNTED | ANIM_COND_AKIMBO | ANIM_COND_RIOTSHIELDNEXT => c.value == 0,
            ANIM_COND_PERK => false,
            _ => true,
        }
    }
}

/// Parse `script` with the anim-type names `types` (one per line). `used_types` has a bit for each player-anim
/// type some weapon has; `exists` says whether an animation is in the zone.
pub fn parse(script: &str, types: &str, used_types: u64, exists: &dyn Fn(&str) -> bool) -> Parsed {
    let mut ctx = Ctx {
        types: types
            .split_whitespace()
            .map(|s| s.to_ascii_lowercase())
            .collect(),
        aliases: HashMap::new(),
        used_types,
        exists,
    };
    let t = tokens(script);
    let mut out = Parsed::default();
    let mut clip_of: HashMap<String, u16> = HashMap::new();
    let mut i = 0;
    let mut mode = "";
    // Read one block of items `{ conds { cmds } ... }` starting at t[i] == "{"; returns the items.
    // `keep` false: the block's items are dropped (a movetype we never have).
    let mut read_items = |i: &mut usize, ctx: &Ctx, out: &mut Parsed, keep: bool| -> Vec<Item> {
        let mut items = Vec::new();
        *i += 1;
        while *i < t.len() && t[*i] != "}" {
            // Conditions up to "{".
            let start = *i;
            while *i < t.len() && t[*i] != "{" {
                *i += 1;
            }
            let cond_tokens = &t[start..*i];
            *i += 1;
            let cmd_start = *i;
            while *i < t.len() && t[*i] != "}" {
                *i += 1;
            }
            let cmd_tokens = &t[cmd_start..*i];
            *i += 1;
            out.items += 1;
            let mut skip = false;
            let mut conditions = Vec::new();
            for group in cond_tokens.split(|w| w == ",") {
                if group.is_empty() || group[0].eq_ignore_ascii_case("default") {
                    continue;
                }
                match ctx.condition(group) {
                    Some(c) if anim_cond_evaluable(c.index) => conditions.push(c),
                    _ => skip = true,
                }
            }
            if !keep || (!skip && !conditions.iter().all(|c| ctx.possible(c))) {
                out.dropped_items += 1;
                continue;
            }
            if skip {
                out.skipped_items += 1;
                continue;
            }
            let mut commands = Vec::new();
            let mut k = 0;
            while k < cmd_tokens.len() {
                let p = match cmd_tokens[k].to_ascii_lowercase().as_str() {
                    "legs" => part::LEGS,
                    "torso" => part::TORSO,
                    "both" => part::BOTH,
                    _ => 0,
                };
                k += 1;
                let anim = cmd_tokens.get(k).map(|a| a.to_ascii_lowercase());
                k += 1;
                let (mut duration_ms, mut blend_ms) = (0, NO_BLEND);
                while k + 1 < cmd_tokens.len() {
                    let key = cmd_tokens[k].to_ascii_lowercase();
                    let v = cmd_tokens[k + 1].parse::<i32>().ok();
                    match key.as_str() {
                        "duration" => duration_ms = v.unwrap_or(0),
                        "blendtime" => blend_ms = v.unwrap_or(NO_BLEND),
                        "sound" => {}
                        _ => break,
                    }
                    k += 2;
                }
                let Some(anim) = anim else { break };
                if p == 0 || !(ctx.exists)(&anim) {
                    out.unresolved += 1;
                    continue;
                }
                let n = clip_of.len() as u16;
                let clip = *clip_of.entry(anim.clone()).or_insert_with(|| {
                    out.anims.push(anim);
                    n
                });
                commands.push(Command {
                    part: p,
                    clip,
                    duration_ms,
                    blend_ms,
                });
            }
            items.push(Item {
                skip: false,
                conditions,
                commands,
            });
        }
        *i += 1;
        items
    };
    while i < t.len() {
        let w = t[i].to_ascii_uppercase();
        if matches!(w.as_str(), "DEFINES" | "ANIMATIONS" | "EVENTS") {
            mode = match w.as_str() {
                "DEFINES" => "DEFINES",
                "ANIMATIONS" => "ANIMATIONS",
                _ => "EVENTS",
            };
            i += 1;
            continue;
        }
        match mode {
            "DEFINES" if t[i].eq_ignore_ascii_case("set") => {
                // set <cond> <alias> = values...
                let cond = t.get(i + 1).and_then(|c| index_ci(c, ANIM_COND_NAMES));
                let alias = t.get(i + 2).map(|a| a.to_ascii_lowercase());
                // One value, then `AND x` / `NOT x` pairs.
                let mut j = (i + 5).min(t.len());
                while j < t.len()
                    && (t[j].eq_ignore_ascii_case("and") || t[j].eq_ignore_ascii_case("not"))
                {
                    j += 2;
                }
                let j = j.min(t.len());
                if let (Some(c), Some(a)) = (cond, alias)
                    && let Some(bits) = ctx.bits(c as u8, &t[i + 4..j])
                {
                    ctx.aliases.insert((c as u8, a), bits);
                }
                i = j;
            }
            "ANIMATIONS" => {
                // STATE <name> { <movetype> { items } ... }
                if t[i].eq_ignore_ascii_case("state") {
                    i += 3;
                    while i < t.len() && t[i] != "}" {
                        let mt = index_ci(&t[i], ANIM_MT_NAMES).filter(|&m| movetype_possible(m));
                        i += 1;
                        let items = read_items(&mut i, &ctx, &mut out, mt.is_some());
                        if let Some(mt) = mt {
                            out.script.moves.push((mt as u8, items));
                        }
                    }
                }
                i += 1;
            }
            "EVENTS" => {
                let ev = index_ci(&t[i], ANIM_ET_NAMES);
                i += 1;
                if t.get(i).is_some_and(|b| b == "{") {
                    let items = read_items(&mut i, &ctx, &mut out, ev.is_some());
                    if let Some(ev) = ev {
                        out.script.events.push((ev as u8, items));
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Print the parsed script to stderr (`MWF_ANIM_DEBUG`; names the user's animations, so it stays on their machine).
pub fn dump(p: &Parsed) {
    let item = |it: &Item| {
        let conds: Vec<String> = it
            .conditions
            .iter()
            .map(|c| {
                let n = ANIM_COND_NAMES.get(usize::from(c.index)).unwrap_or(&"?");
                if c.bitflags {
                    format!("{n}={:#x}", c.bits)
                } else {
                    format!("{n}={}", c.value)
                }
            })
            .collect();
        let cmds: Vec<String> = it
            .commands
            .iter()
            .map(|c| {
                let a = p.anims.get(usize::from(c.clip)).map_or("?", String::as_str);
                format!("{}:{a} d{} b{}", c.part, c.duration_ms, c.blend_ms)
            })
            .collect();
        format!("    [{}] -> {}", conds.join(", "), cmds.join("; "))
    };
    for (mt, items) in &p.script.moves {
        eprintln!(
            "move {}",
            ANIM_MT_NAMES.get(usize::from(*mt)).unwrap_or(&"?")
        );
        for it in items {
            eprintln!("{}", item(it));
        }
    }
    for (ev, items) in &p.script.events {
        eprintln!(
            "event {}",
            ANIM_ET_NAMES.get(usize::from(*ev)).unwrap_or(&"?")
        );
        for it in items {
            eprintln!("{}", item(it));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anim_iw4::{ANIM_COND_WEAPON_POSITION, ANIM_ET_DEATH};

    // Made up, in the format's shape (not MW2's text).
    const SCRIPT_TEXT: &str = "
DEFINES
set movetype moving = walk AND run   // a comment
set movetype gone = idlelaststand
ANIMATIONS
STATE COMBAT
{
	idle
	{
		mounted mg42
		{
			both turret_thing turretanim
		}
		playerAnimType pistol, weapon_position ads
		{
			both a_pistol_ads blendtime 50
		}
		playerAnimType laptop
		{
			both a_laptop
		}
		default
		{
			both a_idle
			both a_idle_2
		}
	}
	idlelaststand
	{
		default { both a_gone }
	}
}
EVENTS
fireweapon
{
	movetype moving, playerAnimType all NOT pistol
	{
		// nothing
	}
	default
	{
		torso a_fire duration 150
	}
}
DEATH
{
	movetype gone
	{
		both a_gone
	}
	default
	{
		both a_missing
		both a_die
	}
}
";
    const TYPES_TEXT: &str = "none\nother\npistol\nsmg\nautorifle\n\nlaptop\n";

    fn parsed() -> Parsed {
        // Weapons with types none, pistol and smg.
        let used = 1 | 1 << 2 | 1 << 3;
        parse(SCRIPT_TEXT, TYPES_TEXT, used, &|a| {
            a.starts_with("a_") && a != "a_missing"
        })
    }

    #[test]
    fn reads_moves_events_and_conditions() {
        let p = parsed();
        let s = &p.script;
        assert_eq!(s.moves.len(), 1, "last stand isn't a movetype we have");
        let idle = s.move_items(1);
        // The turret item and the laptop (no weapon has it) are dropped.
        assert_eq!(idle.len(), 2);
        assert_eq!(
            p.dropped_items, 4,
            "turret, laptop, the last-stand move and death"
        );
        let c = &idle[0].conditions;
        assert_eq!((c[0].index, c[0].bits), (ANIM_COND_PLAYERANIMTYPE, 1 << 2));
        assert_eq!((c[1].index, c[1].value), (ANIM_COND_WEAPON_POSITION, 1));
        assert_eq!(idle[0].commands[0].blend_ms, 50);
        assert_eq!(idle[1].commands.len(), 2);
        // Fire: the empty item stays (it plays nothing), its movetype from the alias, NOT removing pistol.
        let fire = s.event_items(2);
        assert!(fire[0].commands.is_empty());
        assert_eq!(fire[0].conditions[0].bits, 1 << 4 | 1 << 10);
        assert_eq!(fire[0].conditions[1].bits, !(1 << 2));
        assert_eq!(fire[1].commands[0].duration_ms, 150);
        // Death: the missing animation is left out.
        let death = s.event_items(ANIM_ET_DEATH);
        assert_eq!(death.len(), 1);
        assert_eq!(death[0].commands.len(), 1);
        assert_eq!(p.unresolved, 1);
        assert_eq!(
            p.anims,
            ["a_pistol_ads", "a_idle", "a_idle_2", "a_fire", "a_die"]
        );
        assert!(s.clips().all(|c| usize::from(c) < p.anims.len()));
    }
}

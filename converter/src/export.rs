//! `export`: the class weapons' handling numbers and sounds as plain files other tools can read
//! (`out/export/`, git-ignored). Written for Slick Blasters' `tools/mw2/` importers, which read
//! `WEAPONFILE\key\value...` text and WAVs; nothing here is Wii-specific.
//!
//! What it writes:
//! - `weapons/<base>_mp`: one weapon file per Create-a-Class base weapon, with the keys of IW4's own weapon files
//!   for the numbers `WeaponGeometry` decodes (spread and sway in degrees, view kick in degrees/second, times in
//!   seconds as the shipped files write them).
//! - `sounds/<alias>_<n>.wav`: each variant of every player-view alias those weapons use (event sounds and
//!   notetrack sounds), 16-bit PCM at the clip's own rate and channel count.
//! - `index.json`: per weapon, its file, class, and each event's / notetrack's WAVs (paths relative to the folder).
//!
//! The output is the user's own game data: it stays under `out/` and must never be committed (CLAUDE.md rule 1).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use fastfile_iw4::WeaponGeometry;

use crate::sounds::{Capture, LoadedPcm};

/// IW4 `weapClass_t` names, as weapon files spell them.
const CLASSES: &[&str] = &[
    "rifle",
    "sniper",
    "mg",
    "smg",
    "spread",
    "pistol",
    "grenade",
    "rocketlauncher",
    "turret",
    "throwingknife",
    "non-player",
    "item",
];

/// IW4 `weapType_t` names.
const TYPES: &[&str] = &["bullet", "grenade", "projectile", "riotshield"];

/// `wpk::sound::event` names, in that order.
const EVENTS: [&str; wpk::sound::event::COUNT] = [
    "fire",
    "fire_last",
    "empty_fire",
    "reload",
    "reload_empty",
    "raise",
    "first_raise",
    "putaway",
    "melee_swipe",
];

fn enum_name(names: &[&str], i: i32) -> String {
    usize::try_from(i)
        .ok()
        .and_then(|i| names.get(i))
        .map_or_else(|| i.to_string(), |n| (*n).to_owned())
}

/// A float as the weapon files write it (no trailing zeros).
fn num(v: f32) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

fn secs(ms: i32) -> String {
    num(ms as f32 / 1000.0)
}

/// The weapon file text for `g`: `WEAPONFILE\key\value\...`.
pub fn weapon_file(g: &WeaponGeometry) -> String {
    let k = &g.kick;
    let w = &g.sway;
    let fields: Vec<(&str, String)> = vec![
        ("weaponClass", enum_name(CLASSES, g.weap_class)),
        ("weaponType", enum_name(TYPES, g.weap_type)),
        ("fireType", g.fire_type.to_string()),
        ("boltAction", u8::from(g.bolt_action).to_string()),
        ("aimDownSight", u8::from(g.aim_down_sight).to_string()),
        ("clipSize", g.clip_size.to_string()),
        ("shotCount", g.shots_per_fire.to_string()),
        ("damage", g.damage.to_string()),
        ("minDamage", g.min_damage.to_string()),
        ("maxDamageRange", num(g.max_damage_range)),
        ("minDamageRange", num(g.min_damage_range)),
        ("meleeDamage", g.melee_damage.to_string()),
        ("moveSpeedScale", num(g.move_speed_scale)),
        ("adsMoveSpeedScale", num(g.ads_move_speed_scale)),
        ("adsZoomFov", num(g.ads_zoom_fov)),
        // Hip spread and bloom.
        ("hipSpreadStandMin", num(g.hip_spread_stand_min)),
        ("hipSpreadDuckedMin", num(g.hip_spread_ducked_min)),
        ("hipSpreadProneMin", num(g.hip_spread_prone_min)),
        ("hipSpreadMax", num(g.hip_spread_stand_max)),
        ("hipSpreadDuckedMax", num(g.hip_spread_ducked_max)),
        ("hipSpreadProneMax", num(g.hip_spread_prone_max)),
        ("hipSpreadDecayRate", num(g.hip_spread_decay_rate)),
        ("hipSpreadFireAdd", num(g.hip_spread_fire_add)),
        ("hipSpreadTurnAdd", num(g.hip_spread_turn_add)),
        ("hipSpreadMoveAdd", num(g.hip_spread_move_add)),
        ("hipSpreadDuckedDecay", num(g.hip_spread_ducked_decay)),
        ("hipSpreadProneDecay", num(g.hip_spread_prone_decay)),
        ("adsSpread", num(g.ads_spread)),
        // View kick.
        ("hipViewKickPitchMin", num(k.hip_view_kick_pitch_min)),
        ("hipViewKickPitchMax", num(k.hip_view_kick_pitch_max)),
        ("hipViewKickYawMin", num(k.hip_view_kick_yaw_min)),
        ("hipViewKickYawMax", num(k.hip_view_kick_yaw_max)),
        (
            "hipViewKickCenterSpeed",
            num(k.f_hip_view_kick_center_speed),
        ),
        ("adsViewKickPitchMin", num(k.ads_view_kick_pitch_min)),
        ("adsViewKickPitchMax", num(k.ads_view_kick_pitch_max)),
        ("adsViewKickYawMin", num(k.ads_view_kick_yaw_min)),
        ("adsViewKickYawMax", num(k.ads_view_kick_yaw_max)),
        (
            "adsViewKickCenterSpeed",
            num(k.f_ads_view_kick_center_speed),
        ),
        // Gun kick.
        ("hipGunKickPitchMin", num(k.hip_gun_kick_pitch_min)),
        ("hipGunKickPitchMax", num(k.hip_gun_kick_pitch_max)),
        ("hipGunKickYawMin", num(k.hip_gun_kick_yaw_min)),
        ("hipGunKickYawMax", num(k.hip_gun_kick_yaw_max)),
        ("hipGunKickAccel", num(k.hip_gun_kick_accel)),
        ("hipGunKickSpeedMax", num(k.hip_gun_kick_speed_max)),
        ("hipGunKickSpeedDecay", num(k.hip_gun_kick_speed_decay)),
        ("hipGunKickStaticDecay", num(k.hip_gun_kick_static_decay)),
        ("adsGunKickPitchMin", num(k.ads_gun_kick_pitch_min)),
        ("adsGunKickPitchMax", num(k.ads_gun_kick_pitch_max)),
        ("adsGunKickYawMin", num(k.ads_gun_kick_yaw_min)),
        ("adsGunKickYawMax", num(k.ads_gun_kick_yaw_max)),
        ("adsGunKickAccel", num(k.ads_gun_kick_accel)),
        ("adsGunKickSpeedMax", num(k.ads_gun_kick_speed_max)),
        ("adsGunKickSpeedDecay", num(k.ads_gun_kick_speed_decay)),
        ("adsGunKickStaticDecay", num(k.ads_gun_kick_static_decay)),
        ("gunMaxPitch", num(k.gun_max_pitch)),
        ("gunMaxYaw", num(k.gun_max_yaw)),
        // Sway.
        ("swayMaxAngle", num(w.sway_max_angle)),
        ("swayLerpSpeed", num(w.sway_lerp_speed)),
        ("swayPitchScale", num(w.sway_pitch_scale)),
        ("swayYawScale", num(w.sway_yaw_scale)),
        ("swayHorizScale", num(w.sway_horiz_scale)),
        ("swayVertScale", num(w.sway_vert_scale)),
        ("adsSwayMaxAngle", num(w.ads_sway_max_angle)),
        ("adsSwayLerpSpeed", num(w.ads_sway_lerp_speed)),
        ("adsSwayPitchScale", num(w.ads_sway_pitch_scale)),
        ("adsSwayYawScale", num(w.ads_sway_yaw_scale)),
        ("adsSwayHorizScale", num(w.ads_sway_horiz_scale)),
        ("adsSwayVertScale", num(w.ads_sway_vert_scale)),
        // Timings (seconds).
        ("fireTime", secs(g.fire_time_ms)),
        ("fireDelay", secs(g.fire_delay_ms)),
        ("meleeTime", secs(g.melee_time_ms)),
        ("reloadTime", secs(g.reload_time_ms)),
        ("reloadEmptyTime", secs(g.reload_empty_time_ms)),
        ("reloadAddTime", secs(g.reload_add_time_ms)),
        ("rechamberTime", secs(g.rechamber_time_ms)),
        ("rechamberBoltTime", secs(g.rechamber_bolt_time_ms)),
        ("raiseTime", secs(g.raise_time_ms)),
        ("dropTime", secs(g.drop_time_ms)),
        ("firstRaiseTime", secs(g.first_raise_time_ms)),
        ("sprintDurationScale", num(g.sprint_duration_scale)),
    ];
    let mut s = String::from("WEAPONFILE");
    for (key, value) in fields {
        let _ = write!(s, "\\{key}\\{value}");
    }
    s
}

/// A RIFF WAV of `pcm` as 16-bit little-endian at its own rate and channels, or `None` for formats we don't read.
pub fn wav(pcm: &LoadedPcm) -> Option<Vec<u8>> {
    if pcm.format != crate::sounds::MSS_PCM || pcm.channels < 1 || pcm.rate == 0 {
        return None;
    }
    let data: Vec<u8> = match pcm.bits {
        16 => pcm.data[..pcm.data.len() & !1].to_vec(),
        8 => pcm
            .data
            .iter()
            .flat_map(|&b| (((i16::from(b)) - 128) << 8).to_le_bytes())
            .collect(),
        _ => return None,
    };
    let ch = u16::try_from(pcm.channels).ok()?;
    let block = ch * 2;
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&pcm.rate.to_le_bytes());
    out.extend_from_slice(&(pcm.rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    Some(out)
}

/// A file-name-safe version of an alias name.
pub(crate) fn file_stem(alias: &str) -> String {
    alias
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Writes an alias's variants as WAVs (once per alias), returning their paths relative to the export folder.
struct SoundWriter<'a> {
    capture: &'a Capture,
    dir: &'a Path,
    done: HashMap<String, Vec<String>>,
    files: usize,
    bytes: u64,
    missing: usize,
}

impl SoundWriter<'_> {
    fn alias(&mut self, name: &str) -> std::io::Result<Vec<String>> {
        let lower = name.to_ascii_lowercase();
        if let Some(v) = self.done.get(&lower) {
            return Ok(v.clone());
        }
        let mut out = Vec::new();
        let rows = self
            .capture
            .aliases
            .get(&lower)
            .map_or(&[][..], Vec::as_slice);
        for row in rows {
            let Some(pcm) = row
                .loaded
                .as_deref()
                .and_then(|l| self.capture.loaded.get(l))
            else {
                continue;
            };
            let Some(bytes) = wav(pcm) else {
                continue;
            };
            let rel = format!("sounds/{}_{}.wav", file_stem(&lower), out.len() + 1);
            std::fs::write(self.dir.join(&rel), &bytes)?;
            self.files += 1;
            self.bytes += bytes.len() as u64;
            out.push(rel);
        }
        if out.is_empty() {
            self.missing += 1;
        }
        self.done.insert(lower, out.clone());
        Ok(out)
    }
}

#[derive(serde::Serialize)]
struct NoteOut {
    /// `wpk::view::slot` of the first-person clip.
    anim: u8,
    note: String,
    /// Where in the clip, as a fraction of its length.
    time: f32,
    alias: String,
    wavs: Vec<String>,
}

#[derive(serde::Serialize)]
struct WeaponOut {
    file: String,
    class: String,
    /// Event name → its alias and WAVs.
    sounds: BTreeMap<&'static str, (String, Vec<String>)>,
    notes: Vec<NoteOut>,
}

#[derive(serde::Serialize)]
struct Index {
    comment: &'static str,
    weapons: BTreeMap<String, WeaponOut>,
}

pub fn run(args: &[String]) -> ExitCode {
    let mut out_dir = std::path::PathBuf::from("out/export");
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--out" {
            match it.next() {
                Some(v) => out_dir = v.into(),
                None => {
                    eprintln!("--out needs a folder");
                    return ExitCode::from(2);
                }
            }
        } else {
            rest.push(a.clone());
        }
    }
    let mut a = match crate::parse_map_args(&rest) {
        Ok(a) => a,
        Err(code) => return code,
    };
    // The Create-a-Class tables list the base weapons.
    let mut tables = HashMap::new();
    for zone in crate::CLASS_ZONES {
        match crate::walk_zone(&a, zone) {
            Some(l) => tables.extend(l.tables),
            None => eprintln!("warning: {zone} unreadable: no class data"),
        }
    }
    let bases = crate::ui_tables::weapon_bases(&tables);
    if bases.is_empty() {
        eprintln!("no Create-a-Class weapons found (are the class zones in this install?)");
        return ExitCode::FAILURE;
    }
    // The weapons, and the notetracks of their first-person clips, from common_mp.
    a.map = "common_mp".to_owned();
    let (image, label, _) = match crate::open_map_zone(&a) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let walked = crate::zone_textures::walk_with(&image, &HashSet::new(), |s, links| {
        bases
            .iter()
            .filter_map(|b| {
                let w = links.weapons.iter().position(|n| *n == format!("{b}_mp"))?;
                Some((
                    w,
                    crate::sounds::view_notes(s, links, &links.weapon_views[w]),
                ))
            })
            .collect::<Vec<_>>()
    });
    let (mut collector, found) = match walked {
        Ok(x) => x,
        Err(e) => {
            eprintln!("reading {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut capture = std::mem::take(&mut collector.links.sounds);
    match crate::walk_zone(&a, crate::SOUND_ZONE) {
        Some(l) => capture.absorb(l.sounds),
        None => eprintln!(
            "warning: no weapon sounds ({} unreadable)",
            crate::SOUND_ZONE
        ),
    }
    let links = &collector.links;

    for sub in ["weapons", "sounds"] {
        if let Err(e) = std::fs::create_dir_all(out_dir.join(sub)) {
            eprintln!("can't create {}: {e}", out_dir.join(sub).display());
            return ExitCode::FAILURE;
        }
    }
    let mut sw = SoundWriter {
        capture: &capture,
        dir: &out_dir,
        done: HashMap::new(),
        files: 0,
        bytes: 0,
        missing: 0,
    };
    let mut index = Index {
        comment: "Written by mwf-convert export from your own copy of the game. Not for redistribution; never commit it.",
        weapons: BTreeMap::new(),
    };
    let result: std::io::Result<()> = (|| {
        for (w, notes) in &found {
            let name = &links.weapons[*w];
            let g = &links.weapon_geometry[*w];
            let file = format!("weapons/{name}");
            std::fs::write(out_dir.join(&file), weapon_file(g))?;
            let names = &links.weapon_views[*w].sounds;
            let mut sounds = BTreeMap::new();
            for (event, alias) in EVENTS.iter().zip(&names.events) {
                if let Some(alias) = alias {
                    sounds.insert(*event, (alias.clone(), sw.alias(alias)?));
                }
            }
            let mut notes_out = Vec::new();
            for (slot, note, time) in notes {
                let Some((_, alias)) = names
                    .note_map
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(note))
                else {
                    continue;
                };
                notes_out.push(NoteOut {
                    anim: *slot,
                    note: note.clone(),
                    time: *time,
                    alias: alias.clone(),
                    wavs: sw.alias(alias)?,
                });
            }
            let base = name.strip_suffix("_mp").unwrap_or(name).to_owned();
            index.weapons.insert(
                base,
                WeaponOut {
                    file,
                    class: enum_name(CLASSES, g.weap_class),
                    sounds,
                    notes: notes_out,
                },
            );
        }
        let json = serde_json::to_string_pretty(&index).map_err(std::io::Error::other)?;
        std::fs::write(out_dir.join("index.json"), json)
    })();
    if let Err(e) = result {
        eprintln!("writing {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }
    println!(
        "export: {} weapons, {} sounds ({:.1} MB), {} aliases with nothing playable -> {}",
        index.weapons.len(),
        sw.files,
        sw.bytes as f64 / 1_048_576.0,
        sw.missing,
        out_dir.display()
    );
    println!(
        "This is your own game data: keep it on this PC, out of git and out of any build you share."
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_like_the_shipped_files() {
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(3.0), "3");
        assert_eq!(num(0.0), "0");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(-2.25), "-2.25");
        assert_eq!(secs(86), "0.086");
    }

    #[test]
    fn weapon_file_has_the_keys_the_importers_read() {
        let mut g = WeaponGeometry {
            hip_spread_stand_min: 4.0,
            weap_class: 3,
            fire_time_ms: 75,
            ..WeaponGeometry::default()
        };
        g.kick.hip_view_kick_pitch_max = -15.0;
        let t = weapon_file(&g);
        assert!(t.starts_with("WEAPONFILE\\"));
        assert!(t.contains("\\weaponClass\\smg\\"));
        assert!(t.contains("\\hipSpreadStandMin\\4\\"));
        assert!(t.contains("\\hipViewKickPitchMax\\-15\\"));
        assert!(t.contains("\\fireTime\\0.075\\"));
        // Every key is followed by a value.
        assert_eq!(t.split('\\').count() % 2, 1);
    }

    #[test]
    fn wav_header_matches_the_samples() {
        let pcm = LoadedPcm {
            format: crate::sounds::MSS_PCM,
            rate: 22050,
            bits: 8,
            channels: 2,
            data: vec![128, 255, 0, 128],
        };
        let w = wav(&pcm).unwrap();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize,
            w.len() - 8
        );
        assert_eq!(u16::from_le_bytes([w[22], w[23]]), 2);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 22050);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes([w[44], w[45]]), 0);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 127 << 8);
        // Formats it can't read are skipped, not guessed.
        assert!(wav(&LoadedPcm { format: 2, ..pcm }).is_none());
    }

    #[test]
    fn alias_names_become_safe_file_names() {
        assert_eq!(file_stem("Weap_M4/Fire plr"), "weap_m4_fire_plr");
    }
}

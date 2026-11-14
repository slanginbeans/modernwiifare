//! Sounds for the Wii: MW2 sound aliases and the loaded (in-zone) sounds they play, captured while a zone is walked,
//! and the first-person weapon sounds written to `common.wpk` as mono 16-bit big-endian PCM (`wpk::sound`).
//!
//! The capture follows IW4L `crates/asset_audio/src/sound_catalog/capture_iw4.rs` (Apache-2.0, rev `acc57a5`): the
//! x86 `LoadedSound` header fields, the alias row fields (`asset_iw4::snd_alias`), and how an alias row's sound
//! file finds its loaded sound (the loader's binding for inline sounds, else the header the pointer resolves to).
//! Streamed sounds (music, long ambience) are not read: they live in the install's sound files, not the zone.
//! Weapon notetrack sounds use the weapon's `notetrackSoundMap` as IW4L's `play_note_mapped_sound_alias`.

use std::collections::HashMap;

use asset_iw4::snd_alias as sa;
use fastfile_iw4::{Ptr, ZonePtr, ZoneStream};
use wpk::sound::{AliasRecord, NONE, Note, VariantRecord, WeaponSounds};

/// Miles `MSS_PCM`: the only loaded format IW4 PC uses for in-zone sounds that IW4L plays.
const MSS_PCM: i32 = 1;

/// A loaded sound: interleaved little-endian PCM as stored in the zone.
#[derive(Clone, Debug)]
pub struct LoadedPcm {
    pub format: i32,
    pub rate: u32,
    pub bits: i32,
    pub channels: i32,
    pub data: Vec<u8>,
}

/// One variant of an alias.
#[derive(Clone, Debug)]
pub struct AliasRow {
    pub loaded: Option<String>,
    pub vol_min: f32,
    pub vol_max: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    pub probability: f32,
    pub dist_min: f32,
    pub dist_max: f32,
}

#[derive(Default)]
pub struct Capture {
    /// Loaded sounds by name.
    pub loaded: HashMap<String, LoadedPcm>,
    /// Alias lists by name (lower case), each a list of variants.
    pub aliases: HashMap<String, Vec<AliasRow>>,
    last_loaded: Option<String>,
    /// Inline loaded sounds by the sound file that holds them.
    file_to_loaded: HashMap<(u8, u32), String>,
    /// Loaded sounds by the asset slots that hold them (load and alias hooks).
    loaded_by_slot: HashMap<(u8, u32), String>,
}

fn key(p: Ptr) -> (u8, u32) {
    (p.block, p.offset)
}

fn name_at(s: &ZoneStream<'_>, parent: Ptr, field: usize) -> Option<String> {
    match s.ptr_at(parent, field).ok()? {
        ZonePtr::Offset(p) => s.cstr(s.resolve_alias(p)).ok().map(str::to_owned),
        _ => None,
    }
}

impl Capture {
    /// Add another zone's aliases and loaded sounds (the ones already here win).
    pub fn absorb(&mut self, other: Capture) {
        for (k, v) in other.loaded {
            self.loaded.entry(k).or_insert(v);
        }
        for (k, v) in other.aliases {
            self.aliases.entry(k).or_insert(v);
        }
    }

    /// Hook for `AssetLinkSink::capture_loaded_sound` (the PCM lives in a temporary block: copied now).
    pub fn loaded_sound(&mut self, s: &ZoneStream<'_>, header: Ptr, pcm: Ptr, len: usize) {
        let name = name_at(s, header, 0).filter(|n| !n.is_empty());
        self.last_loaded = name.clone();
        let Some(name) = name else {
            return;
        };
        let x64 = s.wire_format() == fastfile_iw4::Iw4WireFormat::X64;
        let read = || -> fastfile_iw4::Result<(i32, u32, i32, i32)> {
            Ok(if x64 {
                (
                    i32::from(s.u16_at(header, 8)?),
                    s.u32_at(header, 12)?,
                    i32::from(s.u16_at(header, 22)?),
                    i32::from(s.u16_at(header, 10)?),
                )
            } else {
                (
                    s.i32_at(header, 4)?,
                    s.u32_at(header, 16)?,
                    s.i32_at(header, 20)?,
                    s.i32_at(header, 24)?,
                )
            })
        };
        let (Ok((format, rate, bits, channels)), Ok(data)) = (read(), s.slice_at(pcm, 0, len))
        else {
            return;
        };
        if data.is_empty() {
            return;
        }
        self.loaded.insert(
            name,
            LoadedPcm {
                format,
                rate,
                bits,
                channels,
                data: data.to_vec(),
            },
        );
    }

    /// Hook for `AssetLinkSink::loaded` of a `LoadedSound`.
    pub fn loaded_slot(&mut self, slot: Ptr, insert_slot: Option<Ptr>) {
        if let Some(n) = self.last_loaded.clone() {
            self.loaded_by_slot.insert(key(slot), n.clone());
            if let Some(i) = insert_slot {
                self.loaded_by_slot.insert(key(i), n);
            }
        }
    }

    /// Hook for `AssetLinkSink::alias` of a `LoadedSound`.
    pub fn alias_slot(&mut self, slot: Ptr, target: Ptr) {
        if let Some(n) = self.loaded_by_slot.get(&key(target)).cloned() {
            self.loaded_by_slot.insert(key(slot), n);
        }
    }

    /// Hook for `AssetLinkSink::bind_last_loaded_to_sound_file`.
    pub fn bind_file(&mut self, file: Ptr) {
        if let Some(n) = self.last_loaded.clone() {
            self.file_to_loaded.insert(key(file), n);
        }
    }

    /// Hook for `AssetLinkSink::capture_sound`.
    pub fn sound_list(&mut self, s: &ZoneStream<'_>, list: Ptr, count: usize, head: Option<Ptr>) {
        let Some(name) = name_at(s, list, 0).filter(|n| !n.is_empty()) else {
            return;
        };
        let Some(arr) = head else {
            return;
        };
        let stride = s.layout(asset_iw4::size::SND_ALIAS, 136);
        let f = |row: Ptr, x86: usize, x64: usize, default: f32| {
            s.f32_at(row, s.layout(x86, x64)).unwrap_or(default)
        };
        let rows = (0..count)
            .map(|i| {
                let row = arr.at(i * stride);
                AliasRow {
                    loaded: self.loaded_of(s, row),
                    vol_min: f(row, sa::SND_ALIAS_VOL_MIN, 52, 0.0),
                    vol_max: f(row, sa::SND_ALIAS_VOL_MAX, 56, 0.0),
                    pitch_min: f(row, sa::SND_ALIAS_PITCH_MIN, 60, 1.0),
                    pitch_max: f(row, sa::SND_ALIAS_PITCH_MAX, 64, 1.0),
                    probability: f(row, sa::SND_ALIAS_PROBABILITY, 88, 1.0),
                    dist_min: f(row, sa::SND_ALIAS_DIST_MIN, 68, 0.0),
                    dist_max: f(row, sa::SND_ALIAS_DIST_MAX, 72, 0.0),
                }
            })
            .collect();
        self.aliases
            .entry(name.to_ascii_lowercase())
            .or_insert(rows);
    }

    /// The loaded sound an alias row plays (`None` for streamed or missing sounds).
    fn loaded_of(&self, s: &ZoneStream<'_>, row: Ptr) -> Option<String> {
        let ZonePtr::Offset(p) = s.ptr_at(row, s.layout(sa::SND_ALIAS_SOUND_FILE, 40)).ok()? else {
            return None;
        };
        let file = s.resolve_alias(p);
        if s.u8_at(file, 0).ok()? != 1 {
            return None;
        }
        if let Some(n) = self.file_to_loaded.get(&key(file)) {
            return Some(n.clone());
        }
        let ZonePtr::Offset(q) = s.ptr_at(file, s.layout(4, 8)).ok()? else {
            return None;
        };
        if let Some(n) = self
            .loaded_by_slot
            .get(&key(q))
            .or_else(|| self.loaded_by_slot.get(&key(s.resolve_alias(q))))
        {
            return Some(n.clone());
        }
        let header = s.resolve_alias(q);
        match s.u32_at(header, 0).ok().map(ZonePtr::decode) {
            Some(ZonePtr::Following) | Some(ZonePtr::Insert) => None,
            _ => name_at(s, header, 0),
        }
    }
}

/// Mono 16-bit big-endian samples from a loaded sound (channels averaged), or `None` for formats we don't read.
pub fn to_mono_be(pcm: &LoadedPcm) -> Option<Vec<u8>> {
    if pcm.format != MSS_PCM || pcm.channels < 1 || pcm.rate == 0 {
        return None;
    }
    let ch = pcm.channels as usize;
    let samples: Vec<i32> = match pcm.bits {
        16 => pcm
            .data
            .chunks_exact(2)
            .map(|b| i32::from(i16::from_le_bytes([b[0], b[1]])))
            .collect(),
        8 => pcm
            .data
            .iter()
            .map(|&b| (i32::from(b) - 128) << 8)
            .collect(),
        _ => return None,
    };
    let mut out = Vec::with_capacity(samples.len() / ch * 2);
    for frame in samples.chunks_exact(ch) {
        let v = (frame.iter().sum::<i32>() / ch as i32).clamp(-32768, 32767) as i16;
        out.extend_from_slice(&v.to_be_bytes());
    }
    Some(out)
}

/// The sounds `common.wpk` carries, built alias by alias.
#[derive(Default)]
pub struct Bank {
    pub aliases: Vec<AliasRecord>,
    pub variants: Vec<VariantRecord>,
    pub data: Vec<u8>,
    by_name: HashMap<String, u16>,
    /// Clips written so far by loaded-sound name: (offset, length, rate).
    clips: HashMap<String, (u32, u32, u32)>,
    /// Aliases asked for that had nothing playable.
    pub missing: usize,
}

impl Bank {
    /// A loaded sound's clip in `data`, written the first time it's asked for.
    fn clip(&mut self, capture: &Capture, loaded: &str) -> Option<(u32, u32, u32)> {
        if let Some(&c) = self.clips.get(loaded) {
            return Some(c);
        }
        let pcm = capture.loaded.get(loaded)?;
        let mut mono = to_mono_be(pcm)?;
        mono.resize(mono.len().div_ceil(32) * 32, 0);
        let c = (self.data.len() as u32, mono.len() as u32, pcm.rate);
        self.data.extend_from_slice(&mono);
        self.clips.insert(loaded.to_owned(), c);
        Some(c)
    }

    /// The index of alias `name`, adding it (and its clips) the first time; `NONE` when nothing in it is playable.
    pub fn add(&mut self, capture: &Capture, name: &str) -> u16 {
        let lower = name.to_ascii_lowercase();
        if let Some(&i) = self.by_name.get(&lower) {
            return i;
        }
        let first = self.variants.len() as u16;
        for row in capture.aliases.get(&lower).into_iter().flatten() {
            let Some(loaded) = row.loaded.as_deref() else {
                continue;
            };
            let Some((data_off, data_len, rate)) = self.clip(capture, loaded) else {
                continue;
            };
            self.variants.push(VariantRecord {
                data_off,
                data_len,
                rate,
                vol_min: row.vol_min,
                vol_max: row.vol_max.max(row.vol_min),
                pitch_min: row.pitch_min,
                pitch_max: row.pitch_max.max(row.pitch_min),
                probability: row.probability,
                dist_min: row.dist_min,
                dist_max: row.dist_max,
            });
        }
        let count = self.variants.len() as u16 - first;
        let index = if count == 0 {
            self.missing += 1;
            NONE
        } else {
            self.aliases.push(AliasRecord {
                first_variant: first,
                variant_count: count,
            });
            self.aliases.len() as u16 - 1
        };
        self.by_name.insert(lower, index);
        index
    }
}

/// A weapon's sounds, by alias name: the player-view alias of each `wpk::sound::event`, and its notetrack map.
#[derive(Clone, Debug, Default)]
pub struct WeaponSoundNames {
    pub events: [Option<String>; wpk::sound::event::COUNT],
    pub note_map: Vec<(String, String)>,
}

/// Notetracks of a weapon's first-person clips: (anim slot, name, time fraction) from each XAnim's notify list
/// (IW4 `XAnimNotifyInfo`: script-string name, time; 8 bytes).
pub fn view_notes(
    s: &ZoneStream<'_>,
    links: &crate::material_links::Links,
    view: &crate::material_links::WeaponView,
) -> Vec<(u8, String, f32)> {
    let mut out = Vec::new();
    for slot in wpk::view::slot::ALL {
        let Some(Some(name)) = view.anims.get(usize::from(slot)) else {
            continue;
        };
        let Some((_, g)) = links
            .xanims
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        let Some(notify) = g.notify else {
            continue;
        };
        for i in 0..g.notify_count {
            let (Ok(id), Ok(time)) = (s.u16_at(notify, i * 8), s.f32_at(notify, i * 8 + 4)) else {
                break;
            };
            if let Some(n) = links.script_string(s, id).filter(|n| !n.is_empty()) {
                out.push((slot, n.to_owned(), time));
            }
        }
    }
    out
}

/// The weapon's sounds in `bank`; `notes` are (anim slot, notetrack name, time fraction) of its first-person clips.
pub fn weapon_sounds(
    capture: &Capture,
    bank: &mut Bank,
    names: &WeaponSoundNames,
    notes: &[(u8, String, f32)],
) -> WeaponSounds {
    let mut events = [NONE; wpk::sound::event::COUNT];
    for (e, n) in events.iter_mut().zip(&names.events) {
        if let Some(n) = n {
            *e = bank.add(capture, n);
        }
    }
    let mut out = Vec::new();
    for (slot, note, time) in notes {
        if note.eq_ignore_ascii_case("end") {
            continue;
        }
        let Some((_, alias)) = names
            .note_map
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(note))
        else {
            continue;
        };
        let a = bank.add(capture, alias);
        if a != NONE {
            out.push(Note {
                slot: *slot,
                time: *time,
                alias: a,
            });
        }
    }
    out.sort_by(|a, b| {
        (a.slot, a.time)
            .partial_cmp(&(b.slot, b.time))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    WeaponSounds { events, notes: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_16_bit_becomes_mono_big_endian() {
        let mut data = Vec::new();
        for (l, r) in [(100i16, 300i16), (-200, -400)] {
            data.extend_from_slice(&l.to_le_bytes());
            data.extend_from_slice(&r.to_le_bytes());
        }
        let pcm = LoadedPcm {
            format: MSS_PCM,
            rate: 44100,
            bits: 16,
            channels: 2,
            data,
        };
        assert_eq!(
            to_mono_be(&pcm).unwrap(),
            [200i16.to_be_bytes(), (-300i16).to_be_bytes()].concat()
        );
        assert!(
            to_mono_be(&LoadedPcm {
                format: 0x11,
                ..pcm
            })
            .is_none()
        );
    }

    #[test]
    fn bank_shares_clips_and_maps_notetracks() {
        let mut c = Capture::default();
        c.loaded.insert(
            "shot".into(),
            LoadedPcm {
                format: MSS_PCM,
                rate: 22050,
                bits: 16,
                channels: 1,
                data: vec![0; 10],
            },
        );
        let row = AliasRow {
            loaded: Some("shot".into()),
            vol_min: 0.5,
            vol_max: 0.7,
            pitch_min: 1.0,
            pitch_max: 1.0,
            probability: 1.0,
            dist_min: 0.0,
            dist_max: 0.0,
        };
        c.aliases
            .insert("fire".into(), vec![row.clone(), row.clone()]);
        c.aliases.insert("clipout".into(), vec![row]);
        let mut bank = Bank::default();
        let mut names = WeaponSoundNames::default();
        names.events[wpk::sound::event::FIRE] = Some("FIRE".into());
        names.events[wpk::sound::event::RELOAD] = Some("nothing".into());
        names.note_map = vec![("mag_out".into(), "clipout".into())];
        let w = weapon_sounds(
            &c,
            &mut bank,
            &names,
            &[(9, "MAG_OUT".into(), 0.4), (9, "end".into(), 1.0)],
        );
        assert_eq!(w.events[wpk::sound::event::FIRE], 0);
        assert_eq!(w.events[wpk::sound::event::RELOAD], NONE);
        assert_eq!(
            w.notes,
            vec![Note {
                slot: 9,
                time: 0.4,
                alias: 1
            }]
        );
        // Both variants and the notetrack alias share one 32-byte padded clip.
        assert_eq!(bank.variants.len(), 3);
        assert_eq!(bank.data.len(), 32);
        assert_eq!(bank.missing, 1);
    }
}

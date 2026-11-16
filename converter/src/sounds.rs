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
pub(crate) const MSS_PCM: i32 = 1;

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
    /// Clips above this rate are resampled down to it (`--slim-views`: 32 kHz, which keeps everything up to 16 kHz).
    pub max_rate: Option<u32>,
    /// Clips as 4-bit DSP ADPCM (`--adpcm`, [`wpk::adpcm`]; `SNDD`'s count says so), else 16-bit PCM.
    pub adpcm: bool,
    /// Cut each clip's quiet tail (`--trim-sounds`, [`trim_tail`]).
    pub trim: bool,
    /// Samples cut by `trim`, by rate (the report).
    trimmed_by_rate: std::collections::BTreeMap<u32, u64>,
    /// Samples written so far, by rate (the report).
    samples_by_rate: std::collections::BTreeMap<u32, (usize, u64)>,
}

/// What a sound bank holds (content-free).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct BankReport {
    pub encoding: &'static str,
    pub aliases: usize,
    pub variants: usize,
    pub clips: usize,
    pub seconds: f64,
    pub bytes: u64,
    /// Seconds of quiet tails cut (`--trim-sounds`).
    pub trimmed_seconds: f64,
    /// Clips and seconds by sample rate (Hz).
    pub by_rate: std::collections::BTreeMap<u32, RateBucket>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct RateBucket {
    pub clips: usize,
    pub seconds: f64,
}

impl BankReport {
    /// Add another bank's numbers (the view files' banks summed).
    pub fn merge(&mut self, o: &BankReport) {
        self.encoding = o.encoding;
        self.aliases += o.aliases;
        self.variants += o.variants;
        self.clips += o.clips;
        self.seconds += o.seconds;
        self.bytes += o.bytes;
        self.trimmed_seconds += o.trimmed_seconds;
        for (r, b) in &o.by_rate {
            let e = self.by_rate.entry(*r).or_default();
            e.clips += b.clips;
            e.seconds += b.seconds;
        }
    }
}

/// `SNDD`'s section count for a bank.
pub fn data_count(bank: &Bank) -> u32 {
    if bank.adpcm {
        wpk::adpcm::SNDD_ADPCM
    } else {
        0
    }
}

/// The `--slim-views` sound rate.
pub const SLIM_RATE: u32 = 32_000;

/// Mono 16-bit big-endian PCM resampled from `from` Hz down to `to` Hz, band-limited: a Blackman-windowed sinc low-pass
/// just under the new Nyquist frequency, normalised so a constant signal keeps its level.
pub fn resample_be(pcm: &[u8], from: u32, to: u32) -> Vec<u8> {
    if to >= from || to == 0 {
        return pcm.to_vec();
    }
    let input: Vec<f64> = pcm
        .chunks_exact(2)
        .map(|b| f64::from(i16::from_be_bytes([b[0], b[1]])))
        .collect();
    if input.is_empty() {
        return Vec::new();
    }
    let ratio = f64::from(from) / f64::from(to);
    // Cutoff as a fraction of the input rate: 95% of the output's Nyquist.
    let fc = 0.5 / ratio * 0.95;
    // Filter half-width: 16 output samples, in input samples.
    let half = (16.0 * ratio).ceil();
    let n_out = ((input.len() as f64) / ratio).floor().max(1.0) as usize;
    let mut out = Vec::with_capacity(n_out * 2);
    for i in 0..n_out {
        let center = i as f64 * ratio;
        let lo = (center - half).ceil().max(0.0) as usize;
        let hi = ((center + half).floor() as usize).min(input.len() - 1);
        let (mut acc, mut wsum) = (0.0, 0.0);
        for (k, &v) in input.iter().enumerate().take(hi + 1).skip(lo) {
            let x = k as f64 - center;
            let sinc = if x.abs() < 1e-12 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let p = std::f64::consts::PI * x / half;
            let window = 0.42 + 0.5 * p.cos() + 0.08 * (2.0 * p).cos();
            acc += v * sinc * window;
            wsum += sinc * window;
        }
        let s = if wsum.abs() > 1e-12 { acc / wsum } else { 0.0 };
        out.extend_from_slice(&(s.round().clamp(-32768.0, 32767.0) as i16).to_be_bytes());
    }
    out
}

/// Loudest a sample may be and still count as silence for [`trim_tail`]: -54 dBFS (65 of 32767), under the noise
/// floor of the Wii's output and far under anything mixed with a match around it.
pub const TRIM_THRESHOLD: i16 = 65;

/// How much of the quiet tail [`trim_tail`] keeps after the last audible sample, seconds (the decay's last few
/// milliseconds, so the cut never lands mid-wave).
pub const TRIM_KEEP_S: f64 = 0.010;

/// Samples to keep of mono 16-bit big-endian `pcm` at `rate` Hz: up to the last sample louder than
/// [`TRIM_THRESHOLD`], plus [`TRIM_KEEP_S`]. Only the end is cut: a clip still starts on the same sample, so nothing
/// plays earlier or later (notetrack and event timings are unchanged). A clip that is silent throughout keeps one
/// sample.
pub fn trim_tail(pcm: &[u8], rate: u32) -> usize {
    let n = pcm.len() / 2;
    let last = (0..n).rev().find(|&i| {
        i16::from_be_bytes([pcm[i * 2], pcm[i * 2 + 1]]).unsigned_abs() > TRIM_THRESHOLD as u16
    });
    match last {
        Some(i) => (i + 1 + (f64::from(rate) * TRIM_KEEP_S) as usize).min(n),
        None => n.min(1),
    }
}

impl Bank {
    /// A loaded sound's clip in `data`, written the first time it's asked for.
    fn clip(&mut self, capture: &Capture, loaded: &str) -> Option<(u32, u32, u32)> {
        if let Some(&c) = self.clips.get(loaded) {
            return Some(c);
        }
        let pcm = capture.loaded.get(loaded)?;
        let mut mono = to_mono_be(pcm)?;
        let mut rate = pcm.rate;
        if let Some(max) = self.max_rate
            && rate > max
        {
            mono = resample_be(&mono, rate, max);
            rate = max;
        }
        if self.trim {
            let keep = trim_tail(&mono, rate);
            *self.trimmed_by_rate.entry(rate).or_default() += (mono.len() / 2 - keep) as u64;
            mono.truncate(keep * 2);
        }
        let samples = mono.len() / 2;
        let e = self.samples_by_rate.entry(rate).or_default();
        e.0 += 1;
        e.1 += samples as u64;
        if self.adpcm {
            mono = wpk::adpcm::encode(&wpk::adpcm::samples_be(&mono));
        } else {
            mono.resize(mono.len().div_ceil(32) * 32, 0);
        }
        let c = (self.data.len() as u32, mono.len() as u32, rate);
        self.data.extend_from_slice(&mono);
        self.clips.insert(loaded.to_owned(), c);
        Some(c)
    }

    /// What the bank holds so far.
    pub fn report(&self) -> BankReport {
        let mut r = BankReport {
            encoding: if self.adpcm { "dsp-adpcm" } else { "pcm16" },
            aliases: self.aliases.len(),
            variants: self.variants.len(),
            clips: self.clips.len(),
            bytes: self.data.len() as u64,
            ..BankReport::default()
        };
        for (&rate, &cut) in &self.trimmed_by_rate {
            r.trimmed_seconds += cut as f64 / f64::from(rate.max(1));
        }
        for (&rate, &(clips, samples)) in &self.samples_by_rate {
            let seconds = samples as f64 / f64::from(rate.max(1));
            r.seconds += seconds;
            r.by_rate.insert(rate, RateBucket { clips, seconds });
        }
        r
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
    anim_notes(s, links, &view.anims)
}

/// The notetracks of the animations `anims` names per slot: (slot, notetrack name, time fraction).
pub fn anim_notes(
    s: &ZoneStream<'_>,
    links: &crate::material_links::Links,
    anims: &[Option<String>],
) -> Vec<(u8, String, f32)> {
    let mut out = Vec::new();
    for slot in wpk::view::slot::ALL {
        let Some(Some(name)) = anims.get(usize::from(slot)) else {
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

/// The weapon's sounds in `bank`; `notes` are (anim slot, clip set, notetrack name, time fraction) of its
/// first-person clips.
pub fn weapon_sounds(
    capture: &Capture,
    bank: &mut Bank,
    names: &WeaponSoundNames,
    notes: &[(u8, u8, String, f32)],
) -> WeaponSounds {
    let mut events = [NONE; wpk::sound::event::COUNT];
    for (e, n) in events.iter_mut().zip(&names.events) {
        if let Some(n) = n {
            *e = bank.add(capture, n);
        }
    }
    let mut out = Vec::new();
    for (slot, set, note, time) in notes {
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
                set: *set,
                time: *time,
                alias: a,
            });
        }
    }
    out.sort_by(|a, b| {
        (a.set, a.slot, a.time)
            .partial_cmp(&(b.set, b.slot, b.time))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    WeaponSounds { events, notes: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `secs` of a sine at `hz`, amplitude `amp`, as mono 16-bit big-endian at `rate`.
    fn tone(hz: f64, rate: u32, secs: f64, amp: f64) -> Vec<u8> {
        let n = (f64::from(rate) * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin();
                (v.round() as i16).to_be_bytes()
            })
            .collect()
    }

    /// Peak level and zero crossings of big-endian samples, ignoring `edge` samples at each end.
    fn peak_and_crossings(pcm: &[u8], edge: usize) -> (f64, usize) {
        let s: Vec<f64> = pcm
            .chunks_exact(2)
            .map(|b| f64::from(i16::from_be_bytes([b[0], b[1]])))
            .collect();
        let s = &s[edge..s.len() - edge];
        let peak = s.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let crossings = s
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        (peak, crossings)
    }

    #[test]
    fn resampling_keeps_audible_tones_and_drops_what_cannot_fit() {
        // 1 kHz at 44.1 kHz -> 32 kHz: same pitch, same level, the right length.
        let src = tone(1000.0, 44_100, 0.5, 20_000.0);
        let out = resample_be(&src, 44_100, 32_000);
        assert_eq!(out.len() / 2, (src.len() / 2) * 32_000 / 44_100);
        let (peak, crossings) = peak_and_crossings(&out, 64);
        assert!((peak - 20_000.0).abs() < 400.0, "peak {peak}");
        // 1 kHz over (0.5 s minus the trimmed edges) has ~2 crossings per cycle.
        let secs = (out.len() / 2 - 128) as f64 / 32_000.0;
        let want = 2.0 * 1000.0 * secs;
        assert!(
            (crossings as f64 - want).abs() <= 2.0,
            "{crossings} crossings, want ~{want}"
        );

        // 20 kHz can't exist at 32 kHz: it must be filtered out, not folded down to an audible 12 kHz.
        let high = resample_be(&tone(20_000.0, 48_000, 0.25, 20_000.0), 48_000, 32_000);
        let (peak, _) = peak_and_crossings(&high, 64);
        assert!(peak < 600.0, "20 kHz leaked through at {peak}");

        // At or below the target rate: untouched.
        let low = tone(440.0, 22_050, 0.1, 10_000.0);
        assert_eq!(resample_be(&low, 22_050, 32_000), low);
        assert_eq!(resample_be(&low, 32_000, 32_000), low);
    }

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
            &[(9, 2, "MAG_OUT".into(), 0.4), (9, 0, "end".into(), 1.0)],
        );
        assert_eq!(w.events[wpk::sound::event::FIRE], 0);
        assert_eq!(w.events[wpk::sound::event::RELOAD], NONE);
        assert_eq!(
            w.notes,
            vec![Note {
                slot: 9,
                set: 2,
                time: 0.4,
                alias: 1
            }]
        );
        // Both variants and the notetrack alias share one 32-byte padded clip.
        assert_eq!(bank.variants.len(), 3);
        assert_eq!(bank.data.len(), 32);
        assert_eq!(bank.missing, 1);
    }

    #[test]
    fn adpcm_bank_is_smaller_decodes_and_reports() {
        let pcm = tone(440.0, 44_100, 0.5, 12_000.0);
        let mut c = Capture::default();
        c.loaded.insert(
            "loop".into(),
            LoadedPcm {
                format: MSS_PCM,
                rate: 44_100,
                bits: 16,
                channels: 1,
                // The capture holds little-endian PCM, as the zone does.
                data: pcm.chunks_exact(2).flat_map(|b| [b[1], b[0]]).collect(),
            },
        );
        c.aliases.insert(
            "hum".into(),
            vec![AliasRow {
                loaded: Some("loop".into()),
                vol_min: 1.0,
                vol_max: 1.0,
                pitch_min: 1.0,
                pitch_max: 1.0,
                probability: 1.0,
                dist_min: 0.0,
                dist_max: 0.0,
            }],
        );
        let mut plain = Bank::default();
        let mut small = Bank {
            adpcm: true,
            ..Bank::default()
        };
        // Half a second of tone, then a second of near-silence (under the threshold): trimming keeps the tone and
        // 10 ms, and the clip still starts on its first sample.
        let mut tail = pcm.clone();
        tail.extend((0..44_100).flat_map(|i| (if i % 2 == 0 { 40i16 } else { -40 }).to_be_bytes()));
        let keep = trim_tail(&tail, 44_100);
        assert!(
            keep >= pcm.len() / 2 && keep <= pcm.len() / 2 + 441,
            "{keep}"
        );
        assert_eq!(&tail[..pcm.len()], &pcm[..]);
        assert_eq!(trim_tail(&[0; 64], 44_100), 1);
        assert_eq!(plain.add(&c, "hum"), 0);
        assert_eq!(small.add(&c, "hum"), 0);
        assert_eq!(data_count(&plain), 0);
        assert_eq!(data_count(&small), wpk::adpcm::SNDD_ADPCM);
        let v = small.variants[0];
        assert_eq!(v.rate, 44_100);
        assert_eq!(v.data_len as usize, small.data.len());
        assert!(plain.data.len() as f64 / small.data.len() as f64 > 3.4);
        let clip = &small.data[v.data_off as usize..(v.data_off + v.data_len) as usize];
        let n = wpk::adpcm::sample_count(clip) as usize;
        assert_eq!(n * 2, pcm.len());
        let mut out = vec![0i16; n];
        wpk::adpcm::decode(clip, 0, &mut [0; 2], &mut out);
        let want = wpk::adpcm::samples_be(&pcm);
        let err = want
            .iter()
            .zip(&out)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(err < 400, "max error {err}");
        let r = small.report();
        assert_eq!((r.encoding, r.clips, r.aliases), ("dsp-adpcm", 1, 1));
        assert!((r.seconds - 0.5).abs() < 0.01);
        assert_eq!(r.by_rate[&44_100].clips, 1);
        // Through a bank: the tail is cut and reported.
        c.loaded.insert(
            "tail".into(),
            LoadedPcm {
                format: MSS_PCM,
                rate: 44_100,
                bits: 16,
                channels: 1,
                data: tail.chunks_exact(2).flat_map(|b| [b[1], b[0]]).collect(),
            },
        );
        let row = AliasRow {
            loaded: Some("tail".into()),
            ..c.aliases["hum"][0].clone()
        };
        c.aliases.insert("t".into(), vec![row]);
        let mut trimmed = Bank {
            trim: true,
            ..Bank::default()
        };
        trimmed.add(&c, "t");
        let r = trimmed.report();
        assert!(
            (r.trimmed_seconds - 0.99).abs() < 0.01,
            "{}",
            r.trimmed_seconds
        );
        assert!((r.seconds - 0.51).abs() < 0.01, "{}", r.seconds);
    }
}

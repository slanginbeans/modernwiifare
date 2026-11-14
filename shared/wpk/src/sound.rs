//! Sounds in `common.wpk`: alias lists (one MW2 sound alias each, a random variant per play), their variants, mono
//! 16-bit big-endian PCM, and each first-person weapon's event and notetrack sounds.
//!
//! - `SNDA`: [`AliasRecord`] per alias list, its variants a run of `SNDV`.
//! - `SNDV`: [`VariantRecord`]: a clip in `SNDD` with its volume and pitch ranges (IW4 `snd_alias_t`).
//! - `SNDD`: the clips, each 32-byte aligned and padded to 32 bytes (DSP DMA).
//! - `WSND`: [`WeaponSounds`] of the weapon whose `WEAP` index is the section's `count`.

use alloc::vec::Vec;

use crate::be::{self, Push};
use crate::records::Record;

/// "No sound" in an alias index.
pub const NONE: u16 = 0xFFFF;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AliasRecord {
    pub first_variant: u16,
    pub variant_count: u16,
}

impl Record for AliasRecord {
    const LEN: usize = 4;
    fn write(&self, o: &mut Vec<u8>) {
        o.u16(self.first_variant);
        o.u16(self.variant_count);
    }
    fn read(b: &[u8]) -> Self {
        AliasRecord {
            first_variant: be::u16(b, 0),
            variant_count: be::u16(b, 2),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VariantRecord {
    /// Byte offset of the clip in `SNDD` (32-byte aligned).
    pub data_off: u32,
    /// Bytes of the clip, padded to a multiple of 32.
    pub data_len: u32,
    /// Sample rate, Hz.
    pub rate: u32,
    /// Volume range (0..1) and pitch range (×), picked at random per play as in IW4.
    pub vol_min: f32,
    pub vol_max: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    /// IW4 alias probability weight of this variant.
    pub probability: f32,
    /// Full volume up to `dist_min` units from the listener, silent from `dist_max` (IW4 `distMin` / `distMax`).
    pub dist_min: f32,
    pub dist_max: f32,
}

impl Record for VariantRecord {
    const LEN: usize = 40;
    fn write(&self, o: &mut Vec<u8>) {
        o.u32(self.data_off);
        o.u32(self.data_len);
        o.u32(self.rate);
        for v in [
            self.vol_min,
            self.vol_max,
            self.pitch_min,
            self.pitch_max,
            self.probability,
            self.dist_min,
            self.dist_max,
        ] {
            o.f32(v);
        }
    }
    fn read(b: &[u8]) -> Self {
        VariantRecord {
            data_off: be::u32(b, 0),
            data_len: be::u32(b, 4),
            rate: be::u32(b, 8),
            vol_min: be::f32(b, 12),
            vol_max: be::f32(b, 16),
            pitch_min: be::f32(b, 20),
            pitch_max: be::f32(b, 24),
            probability: be::f32(b, 28),
            dist_min: be::f32(b, 32),
            dist_max: be::f32(b, 36),
        }
    }
}

/// Weapon events that play a sound (the player-view alias of each, IW4 `WeaponDef`).
pub mod event {
    pub const FIRE: usize = 0;
    pub const FIRE_LAST: usize = 1;
    pub const EMPTY_FIRE: usize = 2;
    pub const RELOAD: usize = 3;
    pub const RELOAD_EMPTY: usize = 4;
    pub const RAISE: usize = 5;
    pub const FIRST_RAISE: usize = 6;
    pub const PUTAWAY: usize = 7;
    pub const MELEE_SWIPE: usize = 8;
    pub const COUNT: usize = 9;
}

/// A sound at a point of a first-person animation (an XAnim notetrack mapped through the weapon's
/// `notetrackSoundMap`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Note {
    /// Animation slot (`view::slot`).
    pub slot: u8,
    /// Where in the clip, as a fraction of its length (IW4 notify time).
    pub time: f32,
    pub alias: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponSounds {
    /// Alias of each `event` (`NONE` when the weapon has none).
    pub events: [u16; event::COUNT],
    /// Sorted by slot, then time.
    pub notes: Vec<Note>,
}

impl WeaponSounds {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        for e in self.events {
            o.u16(e);
        }
        o.u16(self.notes.len() as u16);
        for n in &self.notes {
            o.push(n.slot);
            o.push(0);
            o.u16(n.alias);
            o.f32(n.time);
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<WeaponSounds> {
        let mut events = [NONE; event::COUNT];
        for (i, e) in events.iter_mut().enumerate() {
            *e = be::u16(b.get(..(i + 1) * 2)?, i * 2);
        }
        let at = event::COUNT * 2;
        let n = usize::from(be::u16(b.get(..at + 2)?, at));
        let notes = (0..n)
            .map(|i| {
                let r = b.get(at + 2 + i * 8..at + 2 + (i + 1) * 8)?;
                Some(Note {
                    slot: r[0],
                    alias: be::u16(r, 2),
                    time: be::f32(r, 4),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(WeaponSounds { events, notes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records;

    #[test]
    fn records_round_trip() {
        let v = VariantRecord {
            data_off: 64,
            data_len: 4096,
            rate: 44100,
            vol_min: 0.8,
            vol_max: 0.9,
            pitch_min: 0.95,
            pitch_max: 1.05,
            probability: 1.0,
            dist_min: 120.0,
            dist_max: 5000.0,
        };
        let bytes = records::encode(&[v]);
        assert_eq!(bytes.len(), VariantRecord::LEN);
        assert_eq!(VariantRecord::read(&bytes), v);
        let a = AliasRecord {
            first_variant: 3,
            variant_count: 2,
        };
        assert_eq!(AliasRecord::read(&records::encode(&[a])), a);
    }

    #[test]
    fn weapon_sounds_round_trip() {
        let mut events = [NONE; event::COUNT];
        events[event::FIRE] = 4;
        events[event::RELOAD] = 7;
        let w = WeaponSounds {
            events,
            notes: alloc::vec![
                Note {
                    slot: 5,
                    time: 0.25,
                    alias: 9
                },
                Note {
                    slot: 5,
                    time: 0.6,
                    alias: 10
                }
            ],
        };
        assert_eq!(WeaponSounds::decode(&w.encode()), Some(w));
        assert_eq!(WeaponSounds::decode(&[0; 3]), None);
    }
}

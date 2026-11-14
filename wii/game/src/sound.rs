//! Sound playback on the Wii: alias lists from `common.wpk` (`wpk::sound`) played on ASND voices, the clips read in
//! place from the pack in MEM2 (flushed when it was loaded).
//!
//! Each play picks a variant by its probability weight and a volume and pitch in the alias's ranges, as IW4 does.
//! Player-view sounds only: no distance falloff or panning yet (they play at the listener).

use wpk::records::{self, Record};
use wpk::sound::{AliasRecord, NONE, VariantRecord};

use crate::ffi;

pub struct Sounds<'a> {
    aliases: &'a [u8],
    variants: &'a [u8],
    data: &'a [u8],
    rng: u32,
}

impl<'a> Sounds<'a> {
    /// The sounds of the common pack, or `None` when it has none.
    pub fn load(common: &'a [u8]) -> Option<Sounds<'a>> {
        let pack = wpk::Pack::parse(common, false).ok()?;
        Some(Sounds {
            aliases: pack.section(wpk::tag::SOUND_ALIASES)?.data,
            variants: pack.section(wpk::tag::SOUND_VARIANTS)?.data,
            data: pack.section(wpk::tag::SOUND_DATA)?.data,
            rng: 0x9e37_79b9,
        })
    }

    fn random(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Play alias `alias` (an index into `SNDA`) once.
    pub fn play(&mut self, alias: u16) {
        if alias == NONE {
            return;
        }
        let Some(a) = records::get::<AliasRecord>(self.aliases, usize::from(alias)) else {
            return;
        };
        let first = usize::from(a.first_variant);
        let n = usize::from(a.variant_count);
        let variant = |i: usize| -> Option<VariantRecord> {
            let at = (first + i) * VariantRecord::LEN;
            self.variants
                .get(at..at + VariantRecord::LEN)
                .map(VariantRecord::read)
        };
        // Weighted pick by probability (all-zero weights: uniform).
        let total: f32 = (0..n)
            .filter_map(variant)
            .map(|v| v.probability.max(0.0))
            .sum();
        let mut r = self.random() * if total > 0.0 { total } else { n as f32 };
        let mut pick = None;
        for i in 0..n {
            let Some(v) = variant(i) else {
                break;
            };
            let w = if total > 0.0 {
                v.probability.max(0.0)
            } else {
                1.0
            };
            pick = Some(v);
            if r < w {
                break;
            }
            r -= w;
        }
        let Some(v) = pick else {
            return;
        };
        let Some(clip) = self
            .data
            .get(v.data_off as usize..(v.data_off + v.data_len) as usize)
        else {
            return;
        };
        let vol = v.vol_min + (v.vol_max - v.vol_min) * self.random();
        let pitch = v.pitch_min + (v.pitch_max - v.pitch_min) * self.random();
        let level = (vol.clamp(0.0, 1.0) * 255.0) as u8;
        let rate = (v.rate as f32 * pitch.clamp(0.25, 4.0)) as u32;
        unsafe { ffi::mwf_snd_play(clip.as_ptr(), clip.len() as u32, rate, level, level) };
    }
}

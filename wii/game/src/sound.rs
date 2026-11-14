//! Sound playback on the Wii: alias lists from `common.wpk` (`wpk::sound`) played on ASND voices, the clips read in
//! place from the pack in MEM2 (flushed when it was loaded).
//!
//! Each play picks a variant by its probability weight and a volume and pitch in the alias's ranges, as IW4 does.
//! Player-view sounds play at the listener; world sounds ([`Sounds::play_at`]) fade with distance between the alias's
//! `distMin` and `distMax` (IW4's linear falloff) and pan left / right by where they are.

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

    /// Play alias `alias` (an index into `SNDA`) once, at the listener.
    pub fn play(&mut self, alias: u16) {
        self.play_with(alias, None);
    }

    /// Play alias `alias` at world point `at`, heard from `listener` with its right-hand axis `right` (unit).
    pub fn play_at(&mut self, alias: u16, at: [f32; 3], listener: [f32; 3], right: [f32; 3]) {
        self.play_with(alias, Some((at, listener, right)));
    }

    fn play_with(&mut self, alias: u16, place: Option<([f32; 3], [f32; 3], [f32; 3])>) {
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
        let (mut left, mut right) = (vol, vol);
        if let Some((at, ear, axis)) = place {
            let d: [f32; 3] = core::array::from_fn(|k| at[k] - ear[k]);
            let dist = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            // IW4's linear falloff between the alias's distances (none when it names no far distance).
            let gain = if v.dist_max <= v.dist_min || dist <= v.dist_min {
                1.0
            } else if dist >= v.dist_max {
                0.0
            } else {
                1.0 - (dist - v.dist_min) / (v.dist_max - v.dist_min)
            };
            if gain <= 0.0 {
                return;
            }
            // Pan by how far to the side the sound is.
            let pan = if dist > 1.0 {
                ((d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2]) / dist).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            left = vol * gain * (1.0 - pan.max(0.0) * 0.7);
            right = vol * gain * (1.0 + pan.min(0.0) * 0.7);
        }
        let level = |x: f32| (x.clamp(0.0, 1.0) * 255.0) as u8;
        let rate = (v.rate as f32 * pitch.clamp(0.25, 4.0)) as u32;
        unsafe {
            ffi::mwf_snd_play(
                clip.as_ptr(),
                clip.len() as u32,
                rate,
                level(left),
                level(right),
            )
        };
    }
}

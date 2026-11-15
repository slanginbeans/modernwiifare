//! Where a pack's texture bytes go: by use, GX format and size, with what a lower size cap would save. Content-free
//! (no names): it decides which texture cut is worth making next.

use std::collections::BTreeMap;

use gx_texture::TexFormat;
use serde::Serialize;
use wpk::records::{TextureRecord, texture_flags};

/// Bytes and counts of a group of textures.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Bucket {
    pub count: usize,
    /// All bytes: colour data (every level) plus alpha planes.
    pub bytes: u64,
    /// Of `bytes`: the levels below the top one (colour and alpha plane).
    pub mip_bytes: u64,
    /// Of `bytes`: split alpha planes (I4).
    pub alpha_plane_bytes: u64,
    /// What capping every texture at 256 / 128 texels across would save (top levels dropped; for a texture without
    /// mips, the difference to a smaller copy).
    pub save_cap_256: u64,
    pub save_cap_128: u64,
}

impl Bucket {
    fn add(&mut self, o: &Bucket) {
        self.count += o.count;
        self.bytes += o.bytes;
        self.mip_bytes += o.mip_bytes;
        self.alpha_plane_bytes += o.alpha_plane_bytes;
        self.save_cap_256 += o.save_cap_256;
        self.save_cap_128 += o.save_cap_128;
    }
}

/// One of the largest textures (no name).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Large {
    pub role: String,
    pub format: String,
    pub width: u16,
    pub height: u16,
    pub levels: u8,
    pub alpha_plane: bool,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Breakdown {
    pub total: Bucket,
    pub by_role: BTreeMap<String, Bucket>,
    pub by_format: BTreeMap<String, Bucket>,
    /// By longest side: "<=64", "128", "256", "512", ">=1024".
    pub by_size: BTreeMap<String, Bucket>,
    /// The largest textures, biggest first.
    pub largest: Vec<Large>,
}

/// How many of the largest textures the breakdown lists.
pub const LARGEST: usize = 12;

pub fn format_name(id: u8) -> &'static str {
    match TexFormat::from_gx_id(id) {
        Some(TexFormat::I4) => "I4",
        Some(TexFormat::I8) => "I8",
        Some(TexFormat::IA4) => "IA4",
        Some(TexFormat::IA8) => "IA8",
        Some(TexFormat::Rgb565) => "RGB565",
        Some(TexFormat::Rgb5a3) => "RGB5A3",
        Some(TexFormat::Rgba8) => "RGBA8",
        Some(TexFormat::Cmpr) => "CMPR",
        None => "unknown",
    }
}

fn size_class(w: u16, h: u16) -> &'static str {
    match w.max(h) {
        0..=64 => "<=64",
        65..=128 => "128",
        129..=256 => "256",
        257..=512 => "512",
        _ => ">=1024",
    }
}

/// Bytes saved by capping a `w` × `h` chain of `levels` in `fmt` at `cap` texels across.
fn cap_saving(fmt: TexFormat, w: u32, h: u32, levels: u32, cap: u32) -> u64 {
    let mut k = 0;
    while (w >> k) > cap || (h >> k) > cap {
        k += 1;
    }
    if k == 0 {
        return 0;
    }
    let now = fmt.chain_size(w, h, levels) as u64;
    let kept = if levels > k {
        fmt.chain_size((w >> k).max(1), (h >> k).max(1), levels - k)
    } else {
        // No mips (or fewer than the levels to drop): a smaller copy with as many levels as there were.
        fmt.chain_size((w >> k).max(1), (h >> k).max(1), levels)
    } as u64;
    now.saturating_sub(kept)
}

/// The bucket of one texture record.
fn bucket_of(r: &TextureRecord) -> Bucket {
    let (w, h, levels) = (u32::from(r.width), u32::from(r.height), u32::from(r.levels));
    let plane = r.flags & texture_flags::ALPHA_PLANE != 0;
    let mut b = Bucket {
        count: 1,
        bytes: u64::from(r.data_len) + if plane { u64::from(r.alpha_len) } else { 0 },
        alpha_plane_bytes: if plane { u64::from(r.alpha_len) } else { 0 },
        ..Bucket::default()
    };
    let mut chain = |fmt: TexFormat, len: u32| {
        let top = (fmt.level_size(w, h) as u64).min(u64::from(len));
        b.mip_bytes += u64::from(len) - top;
        b.save_cap_256 += cap_saving(fmt, w, h, levels, 256);
        b.save_cap_128 += cap_saving(fmt, w, h, levels, 128);
    };
    if let Some(fmt) = TexFormat::from_gx_id(r.format) {
        chain(fmt, r.data_len);
        if plane {
            chain(TexFormat::I4, r.alpha_len);
        }
    }
    b
}

/// The breakdown of `records`, `roles[i]` naming the use of record `i` ("other" when missing).
pub fn breakdown(records: &[TextureRecord], roles: &[&str]) -> Breakdown {
    let mut out = Breakdown::default();
    out.merge(records, roles);
    out
}

impl Breakdown {
    /// Add `records` (another pack's textures) to this breakdown.
    pub fn merge(&mut self, records: &[TextureRecord], roles: &[&str]) {
        for (i, r) in records.iter().enumerate() {
            let b = bucket_of(r);
            let role = roles
                .get(i)
                .copied()
                .filter(|s| !s.is_empty())
                .unwrap_or("other");
            self.total.add(&b);
            self.by_role.entry(role.to_owned()).or_default().add(&b);
            self.by_format
                .entry(format_name(r.format).to_owned())
                .or_default()
                .add(&b);
            self.by_size
                .entry(size_class(r.width, r.height).to_owned())
                .or_default()
                .add(&b);
            self.largest.push(Large {
                role: role.to_owned(),
                format: format_name(r.format).to_owned(),
                width: r.width,
                height: r.height,
                levels: r.levels,
                alpha_plane: r.flags & texture_flags::ALPHA_PLANE != 0,
                bytes: b.bytes,
            });
        }
        self.largest.sort_by_key(|l| core::cmp::Reverse(l.bytes));
        self.largest.truncate(LARGEST);
    }
}

/// A few lines for the console.
pub fn render_text(b: &Breakdown) -> String {
    let mib = |x: u64| x as f64 / (1024.0 * 1024.0);
    let line = |k: &str, v: &Bucket| {
        format!(
            "    {k:<10} {:>4} textures {:>7.2} MiB (mips {:.2}, alpha planes {:.2}; cap 256 saves {:.2}, cap 128 saves {:.2})\n",
            v.count,
            mib(v.bytes),
            mib(v.mip_bytes),
            mib(v.alpha_plane_bytes),
            mib(v.save_cap_256),
            mib(v.save_cap_128)
        )
    };
    let mut s = String::from("textures by use:\n");
    for (k, v) in &b.by_role {
        s.push_str(&line(k, v));
    }
    s.push_str("  by format:\n");
    for (k, v) in &b.by_format {
        s.push_str(&line(k, v));
    }
    s.push_str("  by longest side:\n");
    for (k, v) in &b.by_size {
        s.push_str(&line(k, v));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(fmt: TexFormat, w: u16, h: u16, levels: u8, plane: bool) -> TextureRecord {
        let (wu, hu, l) = (u32::from(w), u32::from(h), u32::from(levels));
        TextureRecord {
            format: fmt.gx_id(),
            levels,
            flags: if plane { texture_flags::ALPHA_PLANE } else { 0 },
            wrap: 0,
            width: w,
            height: h,
            data_off: 0,
            data_len: fmt.chain_size(wu, hu, l) as u32,
            alpha_off: 0,
            alpha_len: if plane {
                TexFormat::I4.chain_size(wu, hu, l) as u32
            } else {
                0
            },
        }
    }

    #[test]
    fn buckets_add_up_and_caps_save_the_top_levels() {
        let big = rec(TexFormat::Cmpr, 512, 512, 10, false);
        let split = rec(TexFormat::Cmpr, 256, 256, 9, true);
        let flat = rec(TexFormat::Rgb5a3, 512, 256, 1, false);
        let b = breakdown(&[big, split, flat], &["world", "prop"]);
        assert_eq!(b.total.count, 3);
        let sum: u64 = b.by_role.values().map(|v| v.bytes).sum();
        assert_eq!(sum, b.total.bytes);
        assert_eq!(b.by_role["other"].count, 1);
        assert_eq!(b.by_size["512"].count, 2);
        // A 512 CMPR chain capped at 256 loses exactly its top level.
        let top = TexFormat::Cmpr.level_size(512, 512) as u64;
        assert_eq!(bucket_of(&big).save_cap_256, top);
        assert_eq!(bucket_of(&big).mip_bytes, u64::from(big.data_len) - top);
        // 256 is under the cap; its plane counts in bytes and as alpha.
        let s = bucket_of(&split);
        assert_eq!(s.save_cap_256, 0);
        assert_eq!(s.alpha_plane_bytes, u64::from(split.alpha_len));
        // No mips: the saving is the difference to a 256 × 128 copy.
        assert_eq!(
            bucket_of(&flat).save_cap_256,
            (TexFormat::Rgb5a3.level_size(512, 256) - TexFormat::Rgb5a3.level_size(256, 128))
                as u64
        );
        assert_eq!(
            b.largest[0].bytes,
            bucket_of(&flat).bytes.max(bucket_of(&big).bytes)
        );
        assert!(b.largest.windows(2).all(|w| w[0].bytes >= w[1].bytes));
    }
}

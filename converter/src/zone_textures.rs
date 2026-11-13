//! Phase 1: a map zone's textures (GfxImage) through the Wii encoders (`shared/gx_texture`).
//!
//! For each image the zone loads: decide from its semantic, map type and name whether the Wii needs it, take its
//! pixels from the zone (in-zone `GfxImageLoadDef`) or from `images/<name>.iwi` in `main/*.iwd`, decode the top mip,
//! and encode it with `gx_texture::policy::convert` for a role. Identical sources are encoded once.
//!
//! The `.gxt` files are converted game data and stay under the git-ignored `out/`. The JSON report is content-free
//! (counts, bytes, quality; no names), like `inventory` and `map`.
//!
//! Provisional: the role comes from the image name. It should come from what uses the image (world, xmodel,
//! viewmodel, effect, UI material); see docs/spikes/results.md.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use asset_transport::{IwdIndex, ZoneMemory, fnv1a64};
use fastfile_iw4::{
    AssetLinkSink, AssetSink, AssetType, Ptr, ScriptStrings, ZoneStream, load_asset_at_observed,
    load_zone, parse_zone_header,
};
use gx_texture::image::{Image, resize};
use gx_texture::policy::{Role, convert};
use serde::Serialize;

use crate::inventory::IW4L_REV;
use crate::pixel_decode::{self, PixelFormat};
use crate::texture::{format_name, psnr};

/// Report format version. Bump when fields change meaning.
pub const REPORT_VERSION: u32 = 1;

/// IW4 `TextureSemantic` values that the Wii does not take as textures.
const TS_NORMAL_MAP: u8 = 5;
const TS_SPECULAR_MAP: u8 = 8;
/// IW4 `MapType` for plain 2D images.
const MAPTYPE_2D: u8 = 3;

/// One GfxImage as the zone describes it.
#[derive(Clone, Debug)]
pub struct ZoneImage {
    pub name: String,
    pub semantic: u8,
    pub category: u8,
    pub map_type: u8,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    /// D3D format of the in-zone payload (0 when streamed).
    pub d3d_format: u32,
    /// In-zone top-mip-first pixel data; empty when the image streams from an IWI.
    pub payload: Vec<u8>,
}

#[derive(Default)]
struct Collector {
    images: Vec<ZoneImage>,
}

impl AssetLinkSink for Collector {
    fn loaded(
        &mut self,
        s: &ZoneStream<'_>,
        ty: AssetType,
        _slot: Ptr,
        _insert_slot: Option<Ptr>,
    ) -> fastfile_iw4::Result<()> {
        if ty != AssetType::Image {
            return Ok(());
        }
        let Some(g) = s.latest_image() else {
            return Ok(());
        };
        let name = g
            .name
            .and_then(|p| s.cstr(s.resolve_alias(p)).ok())
            .unwrap_or_default()
            .to_owned();
        let payload = if g.source_len > 0 {
            s.source_slice(g.source_offset, g.source_len)?.to_vec()
        } else {
            Vec::new()
        };
        self.images.push(ZoneImage {
            name,
            semantic: g.semantic,
            category: g.category,
            map_type: g.map_type,
            width: u32::from(g.width),
            height: u32::from(g.height),
            depth: u32::from(g.depth),
            d3d_format: g.format,
            payload,
        });
        Ok(())
    }

    fn alias(&mut self, _ty: AssetType, _slot: Ptr, _target: Ptr) -> fastfile_iw4::Result<()> {
        Ok(())
    }
}

struct Driver(Collector);

impl AssetSink for Driver {
    fn set_script_strings(&mut self, _strings: ScriptStrings) {}

    fn load_asset(
        &mut self,
        s: &mut ZoneStream<'_>,
        _index: usize,
        ty: AssetType,
        slot: Ptr,
    ) -> fastfile_iw4::Result<()> {
        load_asset_at_observed(s, ty, slot, &mut self.0).map(|_| ())
    }
}

/// Every GfxImage loaded by an inflated zone (top level and nested), in load order.
pub fn collect(image: &[u8]) -> Result<Vec<ZoneImage>, String> {
    let header = parse_zone_header(image).map_err(|e| format!("{e:?}"))?;
    let mut memory = ZoneMemory::for_header(&header);
    let mut stream = memory.stream(image).map_err(|e| format!("{e:?}"))?;
    let mut driver = Driver(Collector::default());
    load_zone(&mut stream, &mut driver).map_err(|e| format!("{e:?}"))?;
    Ok(driver.0.images)
}

/// What to do with an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Encode(Role),
    Skip(&'static str),
}

pub fn plan(img: &ZoneImage) -> Plan {
    if img.name.starts_with('*') {
        return Plan::Skip("engine-generated (lightmap, probe): handled with the world");
    }
    if img.name.starts_with('$') {
        return Plan::Skip("engine built-in ($white...): made on the Wii");
    }
    match img.semantic {
        TS_NORMAL_MAP => return Plan::Skip("normal map: baked into lighting"),
        TS_SPECULAR_MAP => return Plan::Skip("specular map: dropped"),
        _ => {}
    }
    if img.map_type != MAPTYPE_2D || img.depth > 1 {
        return Plan::Skip("cube or 3D map: not supported yet");
    }
    Plan::Encode(role_from_name(&img.name))
}

/// Provisional role from the image name, until roles come from the materials that use each image.
pub fn role_from_name(name: &str) -> Role {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if n.starts_with("sky") || n.contains("_sky") {
        Role::Sky
    } else if has(&["viewmodel", "viewhands"]) || n.starts_with("weapon_") {
        Role::Viewmodel
    } else if has(&[
        "hud",
        "compass",
        "cardicon",
        "cardtitle",
        "ui_",
        "minimap",
        "loadscreen",
    ]) {
        Role::Ui
    } else if n.starts_with("fx_")
        || has(&[
            "smoke",
            "fire",
            "flash",
            "spark",
            "explosion",
            "glow",
            "tracer",
        ])
    {
        Role::Effect
    } else if has(&["_body", "body_", "head_", "_head", "hands", "gear_"]) {
        Role::Character
    } else if n.starts_with("com_") || has(&["vehicle", "_prop", "prop_", "debris"]) {
        Role::Prop
    } else {
        Role::World
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RoleTotal {
    pub textures: usize,
    pub source_rgba_bytes: u64,
    pub wii_bytes: u64,
    pub psnr_min_db: Option<f64>,
    /// Mean over finite values (lossless textures are counted in `lossless`).
    pub psnr_mean_db: Option<f64>,
    pub lossless: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct TexturesReport {
    pub report_version: u32,
    pub iw4l_rev: &'static str,
    pub map: String,
    /// Path relative to the game folder, `/`-separated.
    pub zone_path: String,
    pub images: usize,
    pub in_zone: usize,
    pub streamed: usize,
    pub skipped: BTreeMap<&'static str, usize>,
    pub failed: BTreeMap<String, usize>,
    pub duplicates: usize,
    pub encoded: usize,
    pub wii_bytes: u64,
    pub by_role: BTreeMap<&'static str, RoleTotal>,
    pub by_wii_format: BTreeMap<&'static str, (usize, u64)>,
    pub by_source_format: BTreeMap<&'static str, usize>,
    pub semantic_counts: BTreeMap<u8, usize>,
    pub map_type_counts: BTreeMap<u8, usize>,
    pub category_counts: BTreeMap<u8, usize>,
}

/// Pixels of one image: from the zone, or from its IWI.
fn source_pixels(
    img: &ZoneImage,
    iwds: Option<&IwdIndex>,
) -> Result<(Image, PixelFormat, u64), String> {
    if !img.payload.is_empty() {
        let fmt = pixel_decode::d3d_format(img.d3d_format)?;
        let pixels = pixel_decode::decode(&img.payload, img.width, img.height, fmt)?;
        return Ok((pixels, fmt, fnv1a64(&img.payload)));
    }
    let iwds = iwds.ok_or("streamed image, but no main/*.iwd index")?;
    let file = iwds
        .image_candidates(&img.name)
        .and_then(<[_]>::last)
        .ok_or("streamed image: IWI not found in main/*.iwd")?;
    let bytes = file.read()?;
    let info = pixel_decode::iwi_info(&bytes)?;
    let fmt = info.format.map_err(str::to_owned)?;
    let (start, end) = info.top_mip;
    let pixels = pixel_decode::decode(&bytes[start..end], info.width, info.height, fmt)?;
    Ok((pixels, fmt, fnv1a64(&bytes)))
}

/// Strip anything that is not safe in a file name.
fn file_stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "unnamed".into() } else { s }
}

/// Convert every image of `zone_image` (an inflated zone). `.gxt` files go to `out_dir`.
pub fn convert_zone(
    zone_image: &[u8],
    map: &str,
    zone_path: &str,
    iwds: Option<&IwdIndex>,
    out_dir: &Path,
    verbose: bool,
) -> Result<TexturesReport, String> {
    let images = collect(zone_image)?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let mut report = TexturesReport {
        report_version: REPORT_VERSION,
        iw4l_rev: IW4L_REV,
        map: map.to_owned(),
        zone_path: zone_path.to_owned(),
        images: images.len(),
        in_zone: images.iter().filter(|i| !i.payload.is_empty()).count(),
        streamed: images.iter().filter(|i| i.payload.is_empty()).count(),
        skipped: BTreeMap::new(),
        failed: BTreeMap::new(),
        duplicates: 0,
        encoded: 0,
        wii_bytes: 0,
        by_role: BTreeMap::new(),
        by_wii_format: BTreeMap::new(),
        by_source_format: BTreeMap::new(),
        semantic_counts: BTreeMap::new(),
        map_type_counts: BTreeMap::new(),
        category_counts: BTreeMap::new(),
    };
    let mut psnr_sums: HashMap<&'static str, (f64, usize)> = HashMap::new();
    let mut seen: HashMap<(u64, &'static str), ()> = HashMap::new();
    let mut used_stems: HashMap<String, usize> = HashMap::new();

    for img in &images {
        *report.semantic_counts.entry(img.semantic).or_default() += 1;
        *report.map_type_counts.entry(img.map_type).or_default() += 1;
        *report.category_counts.entry(img.category).or_default() += 1;
        let role = match plan(img) {
            Plan::Skip(why) => {
                *report.skipped.entry(why).or_default() += 1;
                continue;
            }
            Plan::Encode(role) => role,
        };
        let (pixels, src_fmt, hash) = match source_pixels(img, iwds) {
            Ok(x) => x,
            Err(e) => {
                *report.failed.entry(e).or_default() += 1;
                continue;
            }
        };
        if seen.insert((hash, role.name()), ()).is_some() {
            report.duplicates += 1;
            continue;
        }
        *report.by_source_format.entry(src_fmt.name()).or_default() += 1;

        let c = convert(&pixels, role);
        let q = c
            .decode_level(0)
            .map(|level0| psnr(&resize(&pixels, c.width, c.height), &level0, c.format));

        let mut stem = file_stem(&img.name);
        let n = used_stems.entry(stem.clone()).or_default();
        *n += 1;
        if *n > 1 {
            stem = format!("{stem}~{n}");
        }
        let gxt = out_dir.join(format!("{stem}.gxt"));
        std::fs::write(&gxt, gx_texture::container::write(&c))
            .map_err(|e| format!("{}: {e}", gxt.display()))?;

        let bytes = c.data.len() as u64;
        if verbose {
            // Console only, for tuning: names never go into the report.
            println!(
                "  {:<9} {:<5} {:>4}x{:<4} -> {:<6} {:>4}x{:<4} {:>6.1} KiB {:>6} dB  alpha {:?}  {}",
                role.name(),
                src_fmt.name(),
                pixels.width,
                pixels.height,
                format_name(c.format),
                c.width,
                c.height,
                bytes as f64 / 1024.0,
                q.map_or("-".into(), |d| format!("{d:.1}")),
                c.traits.alpha,
                img.name
            );
        }
        report.encoded += 1;
        report.wii_bytes += bytes;
        let f = report
            .by_wii_format
            .entry(format_name(c.format))
            .or_default();
        f.0 += 1;
        f.1 += bytes;
        let t = report.by_role.entry(role.name()).or_default();
        t.textures += 1;
        t.source_rgba_bytes += pixels.pixels.len() as u64 * 4;
        t.wii_bytes += bytes;
        match q {
            Some(db) if db.is_finite() => {
                t.psnr_min_db = Some(t.psnr_min_db.map_or(db, |m| m.min(db)));
                let s = psnr_sums.entry(role.name()).or_default();
                s.0 += db;
                s.1 += 1;
            }
            Some(_) => t.lossless += 1,
            None => {}
        }
    }
    for (role, (sum, n)) in psnr_sums {
        if let Some(t) = report.by_role.get_mut(role) {
            t.psnr_mean_db = Some(sum / n as f64);
        }
    }
    Ok(report)
}

pub fn render_text(r: &TexturesReport) -> String {
    use std::fmt::Write as _;
    let kib = |b: u64| b as f64 / 1024.0;
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Modern Wiifare zone textures (report v{}, IW4L {})",
        r.report_version,
        &r.iw4l_rev[..7]
    );
    let _ = writeln!(s, "Zone: {} ({})", r.zone_path, r.map);
    let _ = writeln!(
        s,
        "images: {} ({} in zone, {} streamed from IWI) -> encoded {}, duplicates {}, skipped {}, failed {}",
        r.images,
        r.in_zone,
        r.streamed,
        r.encoded,
        r.duplicates,
        r.skipped.values().sum::<usize>(),
        r.failed.values().sum::<usize>()
    );
    let _ = writeln!(s, "Wii texture memory: {:.2} MiB", mib(r.wii_bytes));
    let _ = writeln!(
        s,
        "\n{:<10} {:>5} {:>12} {:>10} {:>9} {:>9} {:>8}",
        "role", "count", "source MiB", "Wii KiB", "PSNR min", "mean", "lossless"
    );
    for (role, t) in &r.by_role {
        let db = |v: Option<f64>| v.map_or("-".into(), |d| format!("{d:.1}"));
        let _ = writeln!(
            s,
            "{role:<10} {:>5} {:>12.2} {:>10.0} {:>9} {:>9} {:>8}",
            t.textures,
            mib(t.source_rgba_bytes),
            kib(t.wii_bytes),
            db(t.psnr_min_db),
            db(t.psnr_mean_db),
            t.lossless
        );
    }
    let _ = writeln!(s, "\nWii formats:");
    for (f, (n, b)) in &r.by_wii_format {
        let _ = writeln!(s, "  {f:<7} {n:>5}  {:>9.0} KiB", kib(*b));
    }
    let _ = writeln!(s, "Source formats: {:?}", r.by_source_format);
    if !r.skipped.is_empty() {
        let _ = writeln!(s, "\nSkipped:");
        for (why, n) in &r.skipped {
            let _ = writeln!(s, "  {n:>5}  {why}");
        }
    }
    if !r.failed.is_empty() {
        let _ = writeln!(s, "\nFailed:");
        for (why, n) in &r.failed {
            let _ = writeln!(s, "  {n:>5}  {why}");
        }
    }
    let _ = writeln!(
        s,
        "\nsemantic {:?} | map type {:?} | category {:?}",
        r.semantic_counts, r.map_type_counts, r.category_counts
    );
    s
}

/// `main/` of the install, for streamed images.
pub fn main_dir(game: &Path) -> Option<PathBuf> {
    let main = game.join("main");
    main.is_dir().then_some(main)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(name: &str, semantic: u8, map_type: u8) -> ZoneImage {
        ZoneImage {
            name: name.into(),
            semantic,
            category: 3,
            map_type,
            width: 4,
            height: 4,
            depth: 1,
            d3d_format: 0,
            payload: Vec::new(),
        }
    }

    #[test]
    fn plans_from_semantic_and_type() {
        assert_eq!(
            plan(&img("brick_wall", 2, MAPTYPE_2D)),
            Plan::Encode(Role::World)
        );
        assert!(matches!(
            plan(&img("brick_wall_nml", TS_NORMAL_MAP, MAPTYPE_2D)),
            Plan::Skip(_)
        ));
        assert!(matches!(
            plan(&img("brick_wall_spc", TS_SPECULAR_MAP, MAPTYPE_2D)),
            Plan::Skip(_)
        ));
        assert!(matches!(
            plan(&img("*lightmap0_primary", 0, MAPTYPE_2D)),
            Plan::Skip(_)
        ));
        assert!(matches!(plan(&img("$white", 2, MAPTYPE_2D)), Plan::Skip(_)));
        assert!(matches!(plan(&img("sky_cube", 2, 5)), Plan::Skip(_)));
    }

    #[test]
    fn provisional_roles() {
        assert_eq!(role_from_name("sky_test_ft"), Role::Sky);
        assert_eq!(role_from_name("weapon_test_col"), Role::Viewmodel);
        assert_eq!(role_from_name("hud_test"), Role::Ui);
        assert_eq!(role_from_name("fx_test"), Role::Effect);
        assert_eq!(role_from_name("mp_body_test"), Role::Character);
        assert_eq!(role_from_name("com_test_crate"), Role::Prop);
        assert_eq!(role_from_name("test_concrete"), Role::World);
    }

    #[test]
    fn in_zone_bgra_converts_without_iwds() {
        let mut i = img("test_wall", 2, MAPTYPE_2D);
        i.d3d_format = 21;
        i.payload = [10u8, 20, 30, 255].repeat(16);
        let (pixels, fmt, _) = source_pixels(&i, None).unwrap();
        assert_eq!(fmt, PixelFormat::Bgra8);
        assert_eq!(pixels.pixels[0], [30, 20, 10, 255]);
        let streamed = img("test_streamed", 2, MAPTYPE_2D);
        assert!(source_pixels(&streamed, None).is_err());
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("a/b c~d"), "a_b_c_d");
        assert_eq!(file_stem(""), "unnamed");
    }
}

//! Spike D: inventory of the user's install through IW4L's zone readers.
//!
//! Output is metadata only (relative paths, byte sizes, per-type asset counts). Asset *names* and
//! asset *content* are never read into the report.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use asset_transport::{ZoneOpenError, open_zone};
use serde::Serialize;

/// Report format version. Bump when fields change meaning.
pub const REPORT_VERSION: u32 = 1;
/// IW4L revision the readers come from (keep in sync with Cargo.toml).
pub const IW4L_REV: &str = "acc57a5c9dd027db8eb5ef17c792fa69f059daa1";
const MAX_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneKind {
    /// `mp_*` map zone: what the Wii will load per match.
    MpMap,
    /// `mp_*_load` loading-screen zone.
    MpLoadScreen,
    /// Shared multiplayer zones (common_mp, code_post_gfx_mp, patch_mp, localized variants...).
    MpCommon,
    /// Anything else (campaign, Spec Ops, UI-only...). Multiplayer-only project: ignored.
    Other,
}

pub fn classify(stem: &str) -> ZoneKind {
    let s = stem.to_ascii_lowercase();
    if s.starts_with("mp_") && s.ends_with("_load") {
        ZoneKind::MpLoadScreen
    } else if s.starts_with("mp_") {
        ZoneKind::MpMap
    } else if s.ends_with("_mp") || s.contains("_mp_") || s.starts_with("iw4x") {
        ZoneKind::MpCommon
    } else {
        ZoneKind::Other
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ZoneEntry {
    /// Path relative to the game folder, `/`-separated.
    pub path: String,
    pub stem: String,
    pub kind: Option<ZoneKind>,
    pub file_bytes: u64,
    pub game: Option<String>,
    pub version: Option<u32>,
    /// Size after inflating the fastfile.
    pub inflated_bytes: Option<u64>,
    /// Sum of the zone's XFILE memory blocks: what the PC game allocates to hold it.
    pub memory_bytes: Option<u64>,
    pub block_bytes: Option<Vec<u32>>,
    pub asset_count: Option<usize>,
    pub assets_by_type: BTreeMap<String, u32>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub report_version: u32,
    pub tool_version: &'static str,
    pub iw4l_rev: &'static str,
    /// Only the folder's own name, never the absolute path.
    pub game_folder_name: String,
    pub zones: Vec<ZoneEntry>,
    pub totals: Totals,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Totals {
    pub zones: usize,
    pub zones_failed: usize,
    pub mp_maps: Vec<String>,
    /// Asset counts per type summed over all `mp_*` map zones.
    pub mp_map_assets_by_type: BTreeMap<String, u64>,
    pub mp_common_memory_bytes: u64,
}

/// All `.ff` files under `root`, sorted, depth-limited.
pub fn find_zone_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < MAX_DEPTH {
                    pending.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ff")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Error text without the absolute path IW4L attaches.
fn describe(e: &ZoneOpenError) -> String {
    match e {
        ZoneOpenError::AtPath { source, .. } => describe(source),
        other => other.to_string(),
    }
}

pub fn scan_zone(root: &Path, path: &Path) -> ZoneEntry {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut z = ZoneEntry {
        path: rel(root, path),
        kind: Some(classify(&stem)),
        stem,
        file_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        ..ZoneEntry::default()
    };
    let image = match open_zone(path) {
        Ok(i) => i,
        Err(e) => {
            z.error = Some(describe(&e));
            return z;
        }
    };
    z.game = Some(format!("{:?}", image.game));
    z.version = Some(image.version);
    z.inflated_bytes = Some(image.bytes.len() as u64);
    if let Ok(h) = image.header() {
        z.memory_bytes = Some(h.block_size.iter().map(|&b| u64::from(b)).sum());
        z.block_bytes = Some(h.block_size.to_vec());
    }
    match image.iw4_table() {
        Ok(table) => {
            z.asset_count = Some(table.asset_count);
            for kind in table.kinds() {
                *z.assets_by_type.entry(format!("{kind:?}")).or_default() += 1;
            }
        }
        Err(e) => z.error = Some(describe(&e)),
    }
    z
}

pub fn scan(root: &Path) -> Report {
    let zones: Vec<ZoneEntry> = find_zone_files(root)
        .iter()
        .map(|p| scan_zone(root, p))
        .collect();
    let totals = summarize(&zones);
    Report {
        report_version: REPORT_VERSION,
        tool_version: env!("CARGO_PKG_VERSION"),
        iw4l_rev: IW4L_REV,
        game_folder_name: root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        zones,
        totals,
    }
}

pub fn summarize(zones: &[ZoneEntry]) -> Totals {
    let mut t = Totals {
        zones: zones.len(),
        ..Totals::default()
    };
    for z in zones {
        if z.error.is_some() {
            t.zones_failed += 1;
        }
        match z.kind {
            Some(ZoneKind::MpMap) => {
                if !t.mp_maps.contains(&z.stem) {
                    t.mp_maps.push(z.stem.clone());
                }
                for (k, n) in &z.assets_by_type {
                    *t.mp_map_assets_by_type.entry(k.clone()).or_default() += u64::from(*n);
                }
            }
            Some(ZoneKind::MpCommon) => t.mp_common_memory_bytes += z.memory_bytes.unwrap_or(0),
            _ => {}
        }
    }
    t.mp_maps.sort();
    t
}

fn mb(b: u64) -> String {
    format!("{:.1}", b as f64 / (1024.0 * 1024.0))
}

pub fn render_text(r: &Report) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Modern Wiifare inventory (report v{}, IW4L {})",
        r.report_version,
        &r.iw4l_rev[..7]
    );
    let _ = writeln!(s, "Game folder: {}", r.game_folder_name);
    let _ = writeln!(
        s,
        "Zones: {} found, {} could not be read\n",
        r.totals.zones, r.totals.zones_failed
    );

    for (title, kind) in [
        ("Multiplayer maps", ZoneKind::MpMap),
        ("Multiplayer shared zones", ZoneKind::MpCommon),
    ] {
        let _ = writeln!(s, "{title}:");
        let _ = writeln!(
            s,
            "  {:<28} {:>8} {:>9} {:>9} {:>7}",
            "zone", "file MB", "inflated", "PC mem MB", "assets"
        );
        for z in r.zones.iter().filter(|z| z.kind == Some(kind)) {
            let _ = writeln!(
                s,
                "  {:<28} {:>8} {:>9} {:>9} {:>7}{}",
                z.stem,
                mb(z.file_bytes),
                z.inflated_bytes.map(mb).unwrap_or_else(|| "-".into()),
                z.memory_bytes.map(mb).unwrap_or_else(|| "-".into()),
                z.asset_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
                z.error
                    .as_deref()
                    .map(|e| format!("  ERROR: {e}"))
                    .unwrap_or_default()
            );
        }
        let _ = writeln!(s);
    }

    let other = r
        .zones
        .iter()
        .filter(|z| matches!(z.kind, Some(ZoneKind::Other | ZoneKind::MpLoadScreen)))
        .count();
    let _ = writeln!(
        s,
        "Other zones (loading screens, non-multiplayer; not used): {other}"
    );

    if !r.totals.mp_map_assets_by_type.is_empty() {
        let _ = writeln!(s, "\nAsset types across all multiplayer maps:");
        let mut v: Vec<_> = r.totals.mp_map_assets_by_type.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (k, n) in v {
            let _ = writeln!(s, "  {k:<24} {n:>7}");
        }
    }
    let _ = writeln!(
        s,
        "\nShared multiplayer zones need {} MB of PC memory; the Wii has ~88 MB in total.",
        mb(r.totals.mp_common_memory_bytes)
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_zone_names() {
        assert_eq!(classify("mp_rust"), ZoneKind::MpMap);
        assert_eq!(classify("MP_Terminal"), ZoneKind::MpMap);
        assert_eq!(classify("mp_rust_load"), ZoneKind::MpLoadScreen);
        for c in [
            "common_mp",
            "code_post_gfx_mp",
            "patch_mp",
            "localized_common_mp",
            "iw4x_patch_mp",
        ] {
            assert_eq!(classify(c), ZoneKind::MpCommon, "{c}");
        }
        for o in ["common", "ui", "trainer", "contingency"] {
            assert_eq!(classify(o), ZoneKind::Other, "{o}");
        }
    }

    #[test]
    fn summarizes_maps_and_common() {
        let mut a = ZoneEntry {
            stem: "mp_b".into(),
            kind: Some(ZoneKind::MpMap),
            ..Default::default()
        };
        a.assets_by_type.insert("XModel".into(), 3);
        let mut b = ZoneEntry {
            stem: "mp_a".into(),
            kind: Some(ZoneKind::MpMap),
            ..Default::default()
        };
        b.assets_by_type.insert("XModel".into(), 2);
        b.assets_by_type.insert("GfxImage".into(), 5);
        let c = ZoneEntry {
            stem: "common_mp".into(),
            kind: Some(ZoneKind::MpCommon),
            memory_bytes: Some(1000),
            ..Default::default()
        };
        let d = ZoneEntry {
            stem: "broken".into(),
            error: Some("x".into()),
            ..Default::default()
        };
        let t = summarize(&[a, b, c, d]);
        assert_eq!(t.zones, 4);
        assert_eq!(t.zones_failed, 1);
        assert_eq!(t.mp_maps, ["mp_a", "mp_b"]);
        assert_eq!(t.mp_map_assets_by_type["XModel"], 5);
        assert_eq!(t.mp_map_assets_by_type["GfxImage"], 5);
        assert_eq!(t.mp_common_memory_bytes, 1000);
    }
}

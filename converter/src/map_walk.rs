//! Spike D, per map: walk one map zone with IW4L's loader and measure every asset entry.
//!
//! Sizes per top-level asset entry:
//! - `stream`: bytes of the inflated zone stream the entry consumed (nested assets included).
//! - `resident`: bytes it allocated in the XFile blocks other than temp. This is what the PC game keeps in memory:
//!   virtual/large = structs and arrays, physical = GPU-bound data, vertex/index = mesh buffers.
//! - `payload`: raw image pixel bytes and loaded-sound bytes inside the entry.
//!
//! Assets loaded inline inside another (e.g. a material and its images inside an xmodel) count toward the parent.
//!
//! The report stays content-free (CLAUDE.md): asset names are resolved to prove IW4L can, but only counted.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use asset_transport::{GamesRoot, ZoneMemory};
use fastfile_iw4::{
    AssetLinkSink, AssetSink, AssetType, MAX_XFILE_COUNT, Ptr, ScriptStrings, XFILE_BLOCK_TEMP,
    ZonePtr, ZoneStream, load_asset_at_observed, load_zone, parse_zone_header,
};
use serde::Serialize;

use crate::inventory::IW4L_REV;

/// Report format version. Bump when fields change meaning.
pub const REPORT_VERSION: u32 = 1;

pub const BLOCK_NAMES: [&str; MAX_XFILE_COUNT] = [
    "temp", "physical", "runtime", "virtual", "large", "callback", "vertex", "index",
];

struct Row {
    index: usize,
    ty: AssetType,
    named: bool,
    /// false: the entry only points at an asset loaded earlier (an alias); it costs nothing new.
    loaded: bool,
    stream: usize,
    blocks: [usize; MAX_XFILE_COUNT],
    payload: usize,
}

impl Row {
    fn resident(&self) -> usize {
        // temp is scratch reused during load.
        self.blocks
            .iter()
            .enumerate()
            .filter(|(b, _)| *b != XFILE_BLOCK_TEMP)
            .map(|(_, n)| n)
            .sum()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TypeTotal {
    pub entries: usize,
    pub loaded: usize,
    pub stream_bytes: u64,
    pub resident_bytes: u64,
    pub payload_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LargeEntry {
    pub index: usize,
    pub asset_type: &'static str,
    pub resident_bytes: u64,
    pub stream_bytes: u64,
    pub payload_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MapReport {
    pub report_version: u32,
    pub iw4l_rev: &'static str,
    pub map: String,
    /// Path relative to the game folder, `/`-separated.
    pub zone_path: String,
    pub wire_format: &'static str,
    pub file_bytes: u64,
    pub inflated_bytes: u64,
    pub block_bytes: BTreeMap<&'static str, u32>,
    pub script_strings: usize,
    pub entries: usize,
    pub loaded: usize,
    pub names_resolved: usize,
    pub by_type: BTreeMap<&'static str, TypeTotal>,
    /// Largest loaded entries by resident size (index and type only, no names).
    pub largest: Vec<LargeEntry>,
}

#[derive(Default)]
struct Links {
    named: bool,
    payload: usize,
}

impl AssetLinkSink for Links {
    fn loaded(
        &mut self,
        s: &ZoneStream<'_>,
        ty: AssetType,
        _slot: Ptr,
        _insert_slot: Option<Ptr>,
    ) -> fastfile_iw4::Result<()> {
        let text = |p: Option<Ptr>| p.is_some_and(|p| s.cstr(s.resolve_alias(p)).is_ok());
        match ty {
            AssetType::Image => {
                if let Some(g) = s.latest_image() {
                    self.payload += g.source_len;
                    // Inline nested assets finish before their parent, so the last one seen is the top-level entry.
                    self.named = text(g.name);
                }
            }
            AssetType::XModel => self.named = text(s.xmodel().and_then(|g| g.name)),
            _ => self.named = false,
        }
        Ok(())
    }

    fn alias(&mut self, _ty: AssetType, _slot: Ptr, _target: Ptr) -> fastfile_iw4::Result<()> {
        Ok(())
    }

    fn capture_loaded_sound(
        &mut self,
        _s: &ZoneStream<'_>,
        _header: Ptr,
        _pcm: Ptr,
        data_len: usize,
    ) -> fastfile_iw4::Result<()> {
        self.payload += data_len;
        Ok(())
    }
}

/// True when the top-level asset's header carries a readable name. IW4L allocates the header first, in the TEMP
/// block, at the TEMP watermark the entry started from; nearly every IW4 header starts with its name pointer.
fn header_named(s: &ZoneStream<'_>, ty: AssetType, temp_mark: usize) -> bool {
    let field = match ty {
        AssetType::Image => return false, // GfxImage's name is not first; Links handles it
        AssetType::Localize => s.pointer_bytes(), // LocalizeEntry is { value, name }
        _ => 0,
    };
    let header = Ptr {
        block: XFILE_BLOCK_TEMP as u8,
        offset: temp_mark.next_multiple_of(4) as u32,
    };
    match s.ptr_at(header, field) {
        Ok(ZonePtr::Offset(p)) => s.cstr(s.resolve_alias(p)).is_ok_and(|n| !n.is_empty()),
        _ => false,
    }
}

#[derive(Default)]
struct Driver {
    script_strings: usize,
    rows: Vec<Row>,
}

impl AssetSink for Driver {
    fn set_script_strings(&mut self, strings: ScriptStrings) {
        self.script_strings = strings.count();
    }

    fn begin_assets(&mut self, count: usize) {
        self.rows.reserve(count);
    }

    fn load_asset(
        &mut self,
        s: &mut ZoneStream<'_>,
        index: usize,
        ty: AssetType,
        slot: Ptr,
    ) -> fastfile_iw4::Result<()> {
        let cursor0 = s.cursor();
        let marks0: [usize; MAX_XFILE_COUNT] = std::array::from_fn(|b| s.watermark(b as u8));
        let mut links = Links::default();
        let loaded = load_asset_at_observed(s, ty, slot, &mut links)?;
        let blocks = std::array::from_fn(|b| s.watermark(b as u8).saturating_sub(marks0[b]));
        let named = loaded && (header_named(s, ty, marks0[XFILE_BLOCK_TEMP]) || links.named);
        self.rows.push(Row {
            index,
            ty,
            named,
            loaded,
            stream: s.cursor() - cursor0,
            blocks,
            payload: links.payload,
        });
        Ok(())
    }
}

/// Walk an inflated XFile (header + stream), as `asset_transport::open_zone` returns it.
/// `map`, `zone_path` and `file_bytes` only label the report.
pub fn walk(
    image: &[u8],
    map: &str,
    zone_path: &str,
    file_bytes: u64,
) -> Result<MapReport, String> {
    let header = parse_zone_header(image).map_err(|e| format!("{e:?}"))?;
    let mut memory = ZoneMemory::for_header(&header);
    let mut stream = memory.stream(image).map_err(|e| format!("{e:?}"))?;
    let wire_format = match stream.wire_format() {
        fastfile_iw4::Iw4WireFormat::X86 => "x86",
        fastfile_iw4::Iw4WireFormat::X64 => "x64",
    };
    let mut driver = Driver::default();
    load_zone(&mut stream, &mut driver).map_err(|e| {
        format!(
            "{e:?} (after {} assets, stream offset {})",
            driver.rows.len(),
            stream.cursor()
        )
    })?;

    let mut by_type: BTreeMap<&'static str, TypeTotal> = BTreeMap::new();
    for r in &driver.rows {
        let t = by_type.entry(r.ty.name()).or_default();
        t.entries += 1;
        t.loaded += usize::from(r.loaded);
        t.stream_bytes += r.stream as u64;
        t.resident_bytes += r.resident() as u64;
        t.payload_bytes += r.payload as u64;
    }
    let mut big: Vec<&Row> = driver.rows.iter().filter(|r| r.loaded).collect();
    big.sort_by(|a, b| b.resident().cmp(&a.resident()).then(a.index.cmp(&b.index)));
    let largest = big
        .iter()
        .take(15)
        .map(|r| LargeEntry {
            index: r.index,
            asset_type: r.ty.name(),
            resident_bytes: r.resident() as u64,
            stream_bytes: r.stream as u64,
            payload_bytes: r.payload as u64,
        })
        .collect();

    Ok(MapReport {
        report_version: REPORT_VERSION,
        iw4l_rev: IW4L_REV,
        map: map.to_owned(),
        zone_path: zone_path.to_owned(),
        wire_format,
        file_bytes,
        inflated_bytes: image.len() as u64,
        block_bytes: BLOCK_NAMES.into_iter().zip(header.block_size).collect(),
        script_strings: driver.script_strings,
        entries: driver.rows.len(),
        loaded: driver.rows.iter().filter(|r| r.loaded).count(),
        names_resolved: driver.rows.iter().filter(|r| r.named).count(),
        by_type,
        largest,
    })
}

/// The zone to walk for `map`. Current IW4x moves the retail x86 zones to `zone/iw4x/x86/english/` and rewrites
/// `zone/english/` as x64; the retail bytes are preferred. IW4L's own search is only the fallback, because it can pick
/// a same-named custom map under `usermaps/` (docs/upstream-patches.md).
pub fn locate_zone(game: &Path, map: &str) -> Result<PathBuf, String> {
    let file = format!("{map}.ff");
    let candidates = [
        game.join("zone/iw4x/x86/english").join(&file),
        game.join("zone/english").join(&file),
    ];
    if let Some(stock) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(stock);
    }
    asset_transport::find_zone_file(&GamesRoot(game.to_owned()), map).map(|f| f.path)
}

fn mib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

pub fn render_text(r: &MapReport) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Modern Wiifare map walk (report v{}, IW4L {})",
        r.report_version,
        &r.iw4l_rev[..7]
    );
    let _ = writeln!(s, "Zone: {} ({})", r.zone_path, r.map);
    let _ = writeln!(
        s,
        "fastfile {:.1} MiB -> inflated {:.1} MiB, wire format {}",
        mib(r.file_bytes),
        mib(r.inflated_bytes),
        r.wire_format
    );
    let _ = writeln!(
        s,
        "script strings: {}, asset entries: {} ({} loaded, {} named)",
        r.script_strings, r.entries, r.loaded, r.names_resolved
    );
    let blocks: Vec<String> = BLOCK_NAMES
        .iter()
        .map(|n| format!("{n} {:.2}", mib(u64::from(r.block_bytes[n]))))
        .collect();
    let total: u64 = r.block_bytes.values().map(|&b| u64::from(b)).sum();
    let _ = writeln!(
        s,
        "XFile blocks (MiB): {} | total {:.1}",
        blocks.join(", "),
        mib(total)
    );

    let _ = writeln!(
        s,
        "\n{:<18} {:>7} {:>7} {:>11} {:>12} {:>11}",
        "type", "entries", "loaded", "stream MiB", "resident MiB", "payload MiB"
    );
    let mut types: Vec<_> = r.by_type.iter().collect();
    types.sort_by(|a, b| {
        (b.1.resident_bytes + b.1.payload_bytes)
            .cmp(&(a.1.resident_bytes + a.1.payload_bytes))
            .then(a.0.cmp(b.0))
    });
    for (name, t) in types {
        let _ = writeln!(
            s,
            "{name:<18} {:>7} {:>7} {:>11.2} {:>12.2} {:>11.2}",
            t.entries,
            t.loaded,
            mib(t.stream_bytes),
            mib(t.resident_bytes),
            mib(t.payload_bytes)
        );
    }
    let _ = writeln!(s, "\nLargest entries by resident size:");
    for e in &r.largest {
        let _ = writeln!(
            s,
            "  #{:<5} {:<14} {:>8.2} MiB",
            e.index,
            e.asset_type,
            mib(e.resident_bytes)
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic, empty x86 zone written by hand: XFile header (size, externalSize, 8 block sizes) then an
    /// XAssetList with no strings and no assets. No game data involved.
    fn empty_zone() -> Vec<u8> {
        let mut z = Vec::new();
        z.extend_from_slice(&16u32.to_le_bytes()); // size of the stream after the header
        z.extend_from_slice(&0u32.to_le_bytes()); // externalSize
        for _ in 0..MAX_XFILE_COUNT {
            z.extend_from_slice(&0u32.to_le_bytes());
        }
        for _ in 0..4 {
            z.extend_from_slice(&0u32.to_le_bytes()); // stringCount, strings, assetCount, assets
        }
        z
    }

    #[test]
    fn empty_zone_has_no_assets() {
        let r = walk(&empty_zone(), "mp_empty", "zone/english/mp_empty.ff", 0).expect("walks");
        assert_eq!(r.wire_format, "x86");
        assert_eq!(r.entries, 0);
        assert_eq!(r.script_strings, 0);
        assert_eq!(r.block_bytes.len(), MAX_XFILE_COUNT);
        assert!(render_text(&r).contains("asset entries: 0"));
    }

    #[test]
    fn truncated_zone_is_an_error() {
        assert!(walk(&[0u8; 8], "x", "x.ff", 0).is_err());
    }

    #[test]
    fn prefers_retail_x86_zone() {
        let game = std::env::temp_dir().join(format!("mwf_map_locate_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&game);
        for dir in ["zone/english", "zone/iw4x/x86/english"] {
            std::fs::create_dir_all(game.join(dir)).unwrap();
            std::fs::write(game.join(dir).join("mp_test.ff"), b"x").unwrap();
        }
        let found = locate_zone(&game, "mp_test").unwrap();
        assert!(
            found.ends_with("zone/iw4x/x86/english/mp_test.ff"),
            "{}",
            found.display()
        );
        let _ = std::fs::remove_dir_all(&game);
    }
}

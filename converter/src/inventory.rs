//! Walk an inflated IW4 zone with IW4L's loader and record each asset's name and size.
//!
//! Sizes per top-level asset entry:
//! - `stream`: bytes of the inflated zone stream the entry consumed (its on-disk weight, nested assets included).
//! - `blocks`: bytes it allocated in each XFile block. This is what the PC game keeps resident:
//!   virtual/large = structs and arrays, physical = GPU-bound data, vertex/index = mesh buffers.
//! - `payload`: raw pixel bytes of images and PCM/compressed bytes of loaded sounds inside the entry.
//!
//! Assets loaded inline inside another (e.g. a material and its images inside an xmodel) are counted in the parent
//! entry; `nested` says how many there were.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use asset_transport::ZoneMemory;
use fastfile_iw4::{
    AssetLinkSink, AssetSink, AssetType, MAX_XFILE_COUNT, Ptr, ScriptStrings, XFILE_BLOCK_TEMP,
    ZonePtr, ZoneStream, load_asset_at_observed, load_zone, parse_zone_header,
};

use crate::layout::mib;

pub const BLOCK_NAMES: [&str; MAX_XFILE_COUNT] = [
    "temp", "physical", "runtime", "virtual", "large", "callback", "vertex", "index",
];

pub struct Row {
    pub index: usize,
    pub ty: AssetType,
    pub name: Option<String>,
    /// false: the entry only points at an asset loaded earlier (an alias); it costs nothing new.
    pub loaded: bool,
    pub nested: usize,
    pub stream: usize,
    pub blocks: [usize; MAX_XFILE_COUNT],
    pub payload: usize,
}

impl Row {
    pub fn resident(&self) -> usize {
        // temp is scratch reused during load; runtime/callback are zero-filled at load time but still resident.
        self.blocks
            .iter()
            .enumerate()
            .filter(|(b, _)| *b != 0)
            .map(|(_, n)| n)
            .sum()
    }
}

pub struct Inventory {
    pub wire_format: &'static str,
    pub block_size: [u32; MAX_XFILE_COUNT],
    pub inflated_len: usize,
    pub script_strings: usize,
    pub rows: Vec<Row>,
}

#[derive(Default)]
struct Links {
    last_name: Option<String>,
    loads: usize,
    payload: usize,
}

impl AssetLinkSink for Links {
    fn loaded(
        &mut self,
        s: &ZoneStream<'_>,
        ty: AssetType,
        slot: Ptr,
        _insert_slot: Option<Ptr>,
    ) -> fastfile_iw4::Result<()> {
        self.loads += 1;
        if ty == AssetType::Image
            && let Some(g) = s.latest_image()
        {
            self.payload += g.source_len;
        }
        // Inline nested assets finish before their parent, so the last name seen belongs to the top-level entry.
        self.last_name = asset_name(s, ty, slot);
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

/// Name of an inline-loaded asset from what IW4L recorded while loading it (only for types it records).
fn asset_name(s: &ZoneStream<'_>, ty: AssetType, _slot: Ptr) -> Option<String> {
    let text = |p: Ptr| s.cstr(s.resolve_alias(p)).ok().map(str::to_owned);
    match ty {
        AssetType::Image => s.latest_image()?.name.and_then(text),
        AssetType::XModel => s.xmodel()?.name.and_then(text),
        _ => None,
    }
}

/// Name of a top-level asset read from its header struct. IW4L allocates the header first, in the TEMP block, at the
/// TEMP watermark the entry started from; nearly every IW4 header starts with its name pointer.
fn header_name(s: &ZoneStream<'_>, ty: AssetType, temp_mark: usize) -> Option<String> {
    let field = match ty {
        AssetType::Image => return None, // GfxImage's name is not first; asset_name handles it
        AssetType::Localize => s.pointer_bytes(), // LocalizeEntry is { value, name }
        _ => 0,
    };
    let header = Ptr {
        block: XFILE_BLOCK_TEMP as u8,
        offset: temp_mark.next_multiple_of(4) as u32,
    };
    match s.ptr_at(header, field).ok()? {
        ZonePtr::Offset(p) => s
            .cstr(s.resolve_alias(p))
            .ok()
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

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
        let name = if loaded {
            header_name(s, ty, marks0[XFILE_BLOCK_TEMP]).or(links.last_name)
        } else {
            None
        };
        self.rows.push(Row {
            index,
            ty,
            name,
            loaded,
            nested: links.loads.saturating_sub(1),
            stream: s.cursor() - cursor0,
            blocks,
            payload: links.payload,
        });
        Ok(())
    }
}

/// `image` is an inflated XFile (header + stream), as `asset_transport::open_zone` returns it.
pub fn take(image: &[u8]) -> Result<Inventory, String> {
    let header = parse_zone_header(image).map_err(|e| format!("{e:?}"))?;
    let mut memory = ZoneMemory::for_header(&header);
    let mut stream = memory.stream(image).map_err(|e| format!("{e:?}"))?;
    let wire_format = match stream.wire_format() {
        fastfile_iw4::Iw4WireFormat::X86 => "x86",
        fastfile_iw4::Iw4WireFormat::X64 => "x64",
    };
    let mut driver = Driver {
        script_strings: 0,
        rows: Vec::new(),
    };
    load_zone(&mut stream, &mut driver).map_err(|e| {
        let at = driver.rows.len();
        format!(
            "{e:?} (after {at} assets, stream offset {})",
            stream.cursor()
        )
    })?;
    Ok(Inventory {
        wire_format,
        block_size: header.block_size,
        inflated_len: image.len(),
        script_strings: driver.script_strings,
        rows: driver.rows,
    })
}

#[derive(Default)]
struct TypeTotal {
    entries: usize,
    loaded: usize,
    stream: usize,
    resident: usize,
    payload: usize,
}

impl Inventory {
    fn totals(&self) -> BTreeMap<&'static str, TypeTotal> {
        let mut t: BTreeMap<&'static str, TypeTotal> = BTreeMap::new();
        for r in &self.rows {
            let e = t.entry(r.ty.name()).or_default();
            e.entries += 1;
            e.loaded += usize::from(r.loaded);
            e.stream += r.stream;
            e.resident += r.resident();
            e.payload += r.payload;
        }
        t
    }

    pub fn render_summary(&self, file_len: u64, inflate_ms: u128, walk_ms: u128) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "fastfile {:.1} MiB -> inflated {:.1} MiB ({} ms), wire format {}, walked in {} ms",
            mib(file_len),
            mib(self.inflated_len as u64),
            inflate_ms,
            self.wire_format,
            walk_ms
        );
        let _ = writeln!(
            s,
            "script strings: {}, asset entries: {}",
            self.script_strings,
            self.rows.len()
        );
        let blocks: Vec<String> = BLOCK_NAMES
            .iter()
            .zip(self.block_size)
            .map(|(n, b)| format!("{n} {:.2}", mib(u64::from(b))))
            .collect();
        let total: u64 = self.block_size.iter().map(|&b| u64::from(b)).sum();
        let _ = writeln!(
            s,
            "XFile blocks (MiB): {}  | total {:.1}",
            blocks.join(", "),
            mib(total)
        );

        let _ = writeln!(
            s,
            "\n{:<18} {:>7} {:>7} {:>11} {:>11} {:>11}",
            "type", "entries", "loaded", "stream MiB", "resident MiB", "payload MiB"
        );
        let mut totals: Vec<_> = self.totals().into_iter().collect();
        totals.sort_by(|a, b| b.1.resident.cmp(&a.1.resident).then(a.0.cmp(b.0)));
        for (name, t) in &totals {
            let _ = writeln!(
                s,
                "{name:<18} {:>7} {:>7} {:>11.2} {:>11.2} {:>11.2}",
                t.entries,
                t.loaded,
                mib(t.stream as u64),
                mib(t.resident as u64),
                mib(t.payload as u64)
            );
        }
        let unnamed = self
            .rows
            .iter()
            .filter(|r| r.loaded && r.name.is_none())
            .count();
        let _ = writeln!(s, "loaded entries without a resolved name: {unnamed}");

        let _ = writeln!(s, "\nLargest 15 entries by resident size:");
        let mut big: Vec<&Row> = self.rows.iter().filter(|r| r.loaded).collect();
        big.sort_by(|a, b| b.resident().cmp(&a.resident()).then(a.index.cmp(&b.index)));
        for r in big.iter().take(15) {
            let _ = writeln!(
                s,
                "  {:>5} {:<14} {:>9.2} MiB  {}",
                r.index,
                r.ty.name(),
                mib(r.resident() as u64),
                r.name.as_deref().unwrap_or("?")
            );
        }
        s
    }

    pub fn render_tsv(&self) -> String {
        let mut s = String::from("index\ttype\tname\tloaded\tnested\tstream\tresident\tpayload");
        for n in BLOCK_NAMES {
            s.push('\t');
            s.push_str(n);
        }
        s.push('\n');
        for r in &self.rows {
            let _ = write!(
                s,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                r.index,
                r.ty.name(),
                r.name.as_deref().unwrap_or(""),
                u8::from(r.loaded),
                r.nested,
                r.stream,
                r.resident(),
                r.payload
            );
            for b in r.blocks {
                let _ = write!(s, "\t{b}");
            }
            s.push('\n');
        }
        s
    }
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
        let inv = take(&empty_zone()).expect("empty zone walks");
        assert_eq!(inv.wire_format, "x86");
        assert_eq!(inv.rows.len(), 0);
        assert_eq!(inv.script_strings, 0);
        assert!(inv.render_tsv().starts_with("index\ttype"));
    }

    #[test]
    fn truncated_zone_is_an_error() {
        assert!(take(&[0u8; 8]).is_err());
    }
}

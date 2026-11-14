//! `.wpk`: the Modern Wiifare pack. One file per map (and later per shared zone), read straight into MEM2.
//!
//! Golden rule (wii-platform skill): zero runtime conversion. Everything is big-endian (the Wii's byte order) and every
//! section starts 32-byte aligned, so texture data and display lists are handed to GX where they lie.
//!
//! ```text
//! header (64 bytes)
//!   0  magic "MWFP"          4  version u16, flags u16     8  section count u32   12  reserved u32
//!  16  content hash u64 (FNV-1a over bytes 64..file end)  24  file length u32     28  map name [32] (NUL padded)
//!  60  reserved u32
//! section table at 64: count x { tag [4], offset u32, length u32, record count u32 }   (16 bytes each)
//! sections: each 32-byte aligned, padded with zeros
//! ```
//!
//! [`records`] defines the typed records the sections hold; [`PackBuilder`] writes a pack, [`Pack::parse`] reads one.
#![no_std]

extern crate alloc;

pub mod be;
pub mod classes;
pub mod clip;
pub mod nav;
pub mod quant;
pub mod records;
pub mod sound;
pub mod strips;
pub mod view;
pub mod vis;
pub mod weapon;

use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"MWFP";
pub const VERSION: u16 = 3;
pub const HEADER_LEN: usize = 64;
pub const SECTION_ENTRY_LEN: usize = 16;
/// GX needs 32-byte aligned texture data and display lists.
pub const ALIGN: usize = 32;
pub const NAME_LEN: usize = 32;

/// A four-byte section tag.
pub type Tag = [u8; 4];

/// Section tags (see [`records`] for what each holds).
pub mod tag {
    use super::Tag;
    /// Texture records ([`crate::records::TextureRecord`]).
    pub const TEXTURES: Tag = *b"TEXT";
    /// Texture data (GX tiled), referenced by texture records.
    pub const TEXTURE_DATA: Tag = *b"TXDA";
    /// Material records ([`crate::records::MaterialRecord`]).
    pub const MATERIALS: Tag = *b"MATL";
    /// World vertices ([`crate::records::WorldVertex`], interleaved, stride [`crate::records::WORLD_VERTEX_LEN`]).
    pub const WORLD_VERTICES: Tag = *b"WVTX";
    /// World draw batches ([`crate::records::BatchRecord`]).
    pub const WORLD_BATCHES: Tag = *b"WBAT";
    /// World display lists, referenced by batches.
    pub const WORLD_DISPLAY_LISTS: Tag = *b"WDLS";
    /// Skybox: six texture indices ([`crate::records::Skybox`]).
    pub const SKYBOX: Tag = *b"SKYB";
    /// Player spawn points ([`crate::records::SpawnRecord`]).
    pub const SPAWNS: Tag = *b"SPWN";
    /// Static model vertices (`WorldVertex`, model space, lightmap UV unused).
    pub const MODEL_VERTICES: Tag = *b"MVTX";
    /// Static model surfaces (`BatchRecord` over `MVTX`; display lists in `WDLS`).
    pub const MODEL_SURFACES: Tag = *b"MSRF";
    /// Static models (`ModelRecord`).
    pub const MODELS: Tag = *b"MODL";
    /// Placed static models (`InstanceRecord`).
    pub const MODEL_INSTANCES: Tag = *b"SMDL";
    /// Compact world vertices (`quant::CompactVertex`, 18 B) of batches stored fixed-point.
    pub const WORLD_VERTICES_Q: Tag = *b"WVTQ";
    /// How each `WBAT` batch's vertices are stored (`quant::QuantRecord`: format, origin).
    pub const BATCH_QUANT: Tag = *b"WBQZ";
    /// Compact static model vertices.
    pub const MODEL_VERTICES_Q: Tag = *b"MVTQ";
    /// How each `MSRF` surface's vertices are stored.
    pub const SURFACE_QUANT: Tag = *b"MSQZ";
    /// First-person model and animations of a weapon (`view::ViewModel`, in `common.wpk`); `count` = weapon index
    /// in `WEAP` it belongs to.
    pub const VIEWMODEL: Tag = *b"VIEW";
    /// Recoil of every weapon (`weapon::KickRecord`, parallel to `WEAP`).
    pub const KICK: Tag = *b"KICK";
    /// Sound alias lists ([`crate::sound::AliasRecord`]).
    pub const SOUND_ALIASES: Tag = *b"SNDA";
    /// Sound variants ([`crate::sound::VariantRecord`]).
    pub const SOUND_VARIANTS: Tag = *b"SNDV";
    /// Sound clips: mono 16-bit big-endian PCM, 32-byte aligned.
    pub const SOUND_DATA: Tag = *b"SNDD";
    /// A first-person weapon's sounds ([`crate::sound::WeaponSounds`]); `count` = its `WEAP` index.
    pub const WEAPON_SOUNDS: Tag = *b"WSND";
    /// Create-a-Class catalog and default classes (one blob, [`crate::classes::Catalog`]).
    pub const CLASSES: Tag = *b"CLSS";
    /// Weapons (`weapon::WeaponRecord`): combat numbers of every weapon (in `common.wpk`).
    pub const WEAPONS: Tag = *b"WEAP";
    /// World visibility (`vis::Vis`): cell BSP, cells, portals.
    pub const VIS: Tag = *b"VISD";
    /// Cell of each `WBAT` batch (u16, `vis::ALWAYS` = none).
    pub const BATCH_CELLS: Tag = *b"WBCL";
    /// Cell of each `SMDL` instance (u16, `vis::ALWAYS` = none).
    pub const INSTANCE_CELLS: Tag = *b"SMCL";
    /// Collision (`clip::Clip`), one blob.
    pub const CLIP: Tag = *b"CLIP";
    /// Bot walk graph ([`crate::nav::NavGraph`]); decoded at load.
    pub const NAV: Tag = *b"NAVG";
}

/// FNV-1a 64-bit, the pack's content hash (also what both Wiis compare in the system-link handshake).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

const fn align_up(n: usize) -> usize {
    (n + ALIGN - 1) & !(ALIGN - 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u16),
    BadLength,
    BadSectionTable,
    /// A section does not start 32-byte aligned or runs past the end.
    BadSection(Tag),
    HashMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
    pub section_count: u32,
    pub content_hash: u64,
    pub file_len: u32,
    pub name: [u8; NAME_LEN],
}

impl Header {
    /// The map name, without padding.
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    pub tag: Tag,
    pub count: u32,
    pub data: &'a [u8],
}

/// A parsed pack borrowing its bytes. Nothing is copied.
#[derive(Clone, Debug)]
pub struct Pack<'a> {
    pub header: Header,
    bytes: &'a [u8],
}

impl<'a> Pack<'a> {
    /// Parse and validate the header and section table. `check_hash` also verifies the content hash (a full pass over
    /// the file; do it once after loading from SD).
    pub fn parse(bytes: &'a [u8], check_hash: bool) -> Result<Pack<'a>, ParseError> {
        Self::parse_inner(bytes, false, check_hash)
    }

    /// Parse the first part of a pack (at least the header and section table): sections that lie past the end of
    /// `bytes` are not loaded ([`Pack::section`] returns None) but [`Pack::section_location`] still finds them. No
    /// hash check. The Wii loads a pack's GPU data this way and reads the decoded-at-load tail into a temporary
    /// buffer (see [`decoded_at_load`]).
    pub fn parse_prefix(bytes: &'a [u8]) -> Result<Pack<'a>, ParseError> {
        Self::parse_inner(bytes, true, false)
    }

    fn parse_inner(
        bytes: &'a [u8],
        partial: bool,
        check_hash: bool,
    ) -> Result<Pack<'a>, ParseError> {
        if bytes.len() < HEADER_LEN {
            return Err(ParseError::TooShort);
        }
        if bytes[0..4] != MAGIC {
            return Err(ParseError::BadMagic);
        }
        let version = be::u16(bytes, 4);
        if version != VERSION {
            return Err(ParseError::UnsupportedVersion(version));
        }
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&bytes[28..28 + NAME_LEN]);
        let header = Header {
            version,
            flags: be::u16(bytes, 6),
            section_count: be::u32(bytes, 8),
            content_hash: be::u64(bytes, 16),
            file_len: be::u32(bytes, 24),
            name,
        };
        let file_len = header.file_len as usize;
        if file_len != bytes.len() && !(partial && bytes.len() < file_len) {
            return Err(ParseError::BadLength);
        }
        let table_end = HEADER_LEN
            .checked_add(header.section_count as usize * SECTION_ENTRY_LEN)
            .ok_or(ParseError::BadSectionTable)?;
        if table_end > bytes.len() {
            return Err(ParseError::BadSectionTable);
        }
        let pack = Pack { header, bytes };
        for s in pack.entries() {
            let (tag, off, len) = (s.0, s.1 as usize, s.2 as usize);
            if off % ALIGN != 0
                || off < table_end
                || off.checked_add(len).is_none_or(|e| e > file_len)
            {
                return Err(ParseError::BadSection(tag));
            }
        }
        if check_hash && fnv1a64(&bytes[HEADER_LEN..]) != header.content_hash {
            return Err(ParseError::HashMismatch);
        }
        Ok(pack)
    }

    fn entries(&self) -> impl Iterator<Item = (Tag, u32, u32, u32)> + '_ {
        (0..self.header.section_count as usize).map(move |i| {
            let at = HEADER_LEN + i * SECTION_ENTRY_LEN;
            let mut tag = [0u8; 4];
            tag.copy_from_slice(&self.bytes[at..at + 4]);
            (
                tag,
                be::u32(self.bytes, at + 4),
                be::u32(self.bytes, at + 8),
                be::u32(self.bytes, at + 12),
            )
        })
    }

    /// The first section with `tag`, if any.
    pub fn section(&self, tag: Tag) -> Option<Section<'a>> {
        let bytes = self.bytes;
        self.entries()
            .find(|e| e.0 == tag)
            .filter(|e| (e.1 as usize + e.2 as usize) <= bytes.len())
            .map(|(tag, off, len, count)| Section {
                tag,
                count,
                data: &bytes[off as usize..off as usize + len as usize],
            })
    }

    /// Where a section is in the file (offset, length, record count), loaded or not.
    pub fn section_location(&self, tag: Tag) -> Option<(usize, usize, u32)> {
        self.entries()
            .find(|e| e.0 == tag)
            .map(|e| (e.1 as usize, e.2 as usize, e.3))
    }

    /// Bytes from the start of the file that hold every section the GPU reads in place: everything before the first
    /// section in [`decoded_at_load`] (the whole file when there is none).
    pub fn in_place_len(&self) -> usize {
        self.entries()
            .filter(|e| decoded_at_load().contains(&e.0))
            .map(|e| e.1 as usize)
            .min()
            .unwrap_or(self.header.file_len as usize)
    }

    /// Offset of a section's data within the file (for computing addresses once the file is in memory).
    pub fn section_offset(&self, tag: Tag) -> Option<usize> {
        self.entries().find(|e| e.0 == tag).map(|e| e.1 as usize)
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// Sections the runtime decodes into its own memory at load and never reads in place. The converter writes them
/// last, so a loader can keep only the bytes before them ([`Pack::in_place_len`]).
pub const fn decoded_at_load() -> [Tag; 5] {
    [
        tag::VIS,
        tag::BATCH_CELLS,
        tag::INSTANCE_CELLS,
        tag::CLIP,
        tag::NAV,
    ]
}

/// Builds a pack in memory. Sections are written in the order they are added.
pub struct PackBuilder {
    name: [u8; NAME_LEN],
    flags: u16,
    sections: Vec<(Tag, u32, Vec<u8>)>,
}

impl PackBuilder {
    /// `name` is truncated to 31 bytes.
    pub fn new(name: &str) -> PackBuilder {
        let mut n = [0u8; NAME_LEN];
        let len = name.len().min(NAME_LEN - 1);
        n[..len].copy_from_slice(&name.as_bytes()[..len]);
        PackBuilder {
            name: n,
            flags: 0,
            sections: Vec::new(),
        }
    }

    pub fn flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    /// Add a section holding `count` records in `data`.
    pub fn section(&mut self, tag: Tag, count: u32, data: Vec<u8>) -> &mut Self {
        self.sections.push((tag, count, data));
        self
    }

    pub fn finish(self) -> Vec<u8> {
        let table_end = HEADER_LEN + self.sections.len() * SECTION_ENTRY_LEN;
        let mut offsets = Vec::with_capacity(self.sections.len());
        let mut at = align_up(table_end);
        for (_, _, data) in &self.sections {
            offsets.push(at);
            at = align_up(at + data.len());
        }
        let mut out = alloc::vec![0u8; at];
        out[0..4].copy_from_slice(&MAGIC);
        be::put_u16(&mut out, 4, VERSION);
        be::put_u16(&mut out, 6, self.flags);
        be::put_u32(&mut out, 8, self.sections.len() as u32);
        be::put_u32(&mut out, 24, at as u32);
        out[28..28 + NAME_LEN].copy_from_slice(&self.name);
        for (i, ((tag, count, data), &off)) in self.sections.iter().zip(&offsets).enumerate() {
            let e = HEADER_LEN + i * SECTION_ENTRY_LEN;
            out[e..e + 4].copy_from_slice(tag);
            be::put_u32(&mut out, e + 4, off as u32);
            be::put_u32(&mut out, e + 8, data.len() as u32);
            be::put_u32(&mut out, e + 12, *count);
            out[off..off + data.len()].copy_from_slice(data);
        }
        let hash = fnv1a64(&out[HEADER_LEN..]);
        be::put_u64(&mut out, 16, hash);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sample() -> Vec<u8> {
        let mut b = PackBuilder::new("mp_test");
        b.section(tag::TEXTURE_DATA, 2, vec![1u8; 40]);
        b.section(tag::SPAWNS, 1, vec![7u8; 5]);
        b.finish()
    }

    #[test]
    fn prefix_parse_finds_unloaded_tail() {
        let mut b = PackBuilder::new("mp_x");
        b.section(tag::TEXTURES, 1, alloc::vec![1u8; 40]);
        b.section(tag::CLIP, 1, alloc::vec![2u8; 100]);
        let bytes = b.finish();
        let full = Pack::parse(&bytes, true).unwrap();
        let cut = full.in_place_len();
        let (clip_off, clip_len, _) = full.section_location(tag::CLIP).unwrap();
        assert_eq!(cut, clip_off);
        let p = Pack::parse_prefix(&bytes[..cut]).unwrap();
        assert!(p.section(tag::TEXTURES).is_some());
        assert!(p.section(tag::CLIP).is_none());
        assert_eq!(p.section_location(tag::CLIP), Some((clip_off, clip_len, 1)));
        assert_eq!(
            Pack::parse(&bytes[..cut], false).unwrap_err(),
            ParseError::BadLength
        );
        // The header and table alone are enough to plan the load.
        let table_end = HEADER_LEN + 2 * SECTION_ENTRY_LEN;
        assert_eq!(
            Pack::parse_prefix(&bytes[..table_end])
                .unwrap()
                .in_place_len(),
            cut
        );
    }

    #[test]
    fn roundtrip_and_alignment() {
        let bytes = sample();
        let p = Pack::parse(&bytes, true).unwrap();
        assert_eq!(p.header.name(), "mp_test");
        assert_eq!(p.header.section_count, 2);
        let t = p.section(tag::TEXTURE_DATA).unwrap();
        assert_eq!((t.count, t.data), (2, &[1u8; 40][..]));
        assert_eq!(p.section_offset(tag::TEXTURE_DATA).unwrap() % ALIGN, 0);
        assert_eq!(p.section_offset(tag::SPAWNS).unwrap() % ALIGN, 0);
        assert_eq!(p.section(tag::SPAWNS).unwrap().data, &[7u8; 5]);
        assert!(p.section(tag::MATERIALS).is_none());
        assert_eq!(bytes.len() % ALIGN, 0);
    }

    #[test]
    fn rejects_damage() {
        let bytes = sample();
        assert_eq!(
            Pack::parse(&bytes[..10], false).unwrap_err(),
            ParseError::TooShort
        );
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(Pack::parse(&bad, false).unwrap_err(), ParseError::BadMagic);
        let mut bad = bytes.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(
            Pack::parse(&bad, true).unwrap_err(),
            ParseError::HashMismatch
        );
        assert!(Pack::parse(&bad, false).is_ok());
        assert_eq!(
            Pack::parse(&bytes[..bytes.len() - 32], false).unwrap_err(),
            ParseError::BadLength
        );
        // A section offset that is not 32-byte aligned.
        let mut bad = bytes.clone();
        be::put_u32(&mut bad, HEADER_LEN + 4, 70);
        assert!(matches!(
            Pack::parse(&bad, false),
            Err(ParseError::BadSection(_))
        ));
    }

    #[test]
    fn deterministic() {
        assert_eq!(sample(), sample());
    }
}

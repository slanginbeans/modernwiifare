//! Rebuild only what changed: every pack the converter writes carries a stamp (`STMP` section, first in the file):
//! the converter revision that made it and a fingerprint of the game files it was read from (each zone's name, size
//! and modification time). `pack` / `common` skip an output whose stamp matches, unless `--force`.
//!
//! Bump [`REVISION`] whenever a change alters what the converter writes (a new section, a format change, different
//! numbers), so everyone's packs are rebuilt on the next run.

use std::io::Read;
use std::path::{Path, PathBuf};

/// The converter's output revision. 1: walk graphs, hit-location damage, per-weapon view files. 2: weapon movement
/// scales (`WMOV`). 3: ADS zoom and aim-assist ranges (`WAIM`). 4: blend / cull of model materials whose technique set
/// comes from another zone (lit models were additive: see-through). 5: menu fonts and screens (`UIPK`).
/// 6: class select, pause, Create-a-Class and scoreboard screens, with their text labels. 7: match rules (`RULE`). 8: perk upgrades in `CLSS`, move speed rules.
/// 9: HUD minimap (`MINI`), minimap layout and compass icons in `UIPK`, killstreak costs, UAV time.
/// 10: every weapon animation slot in the view files, first-person weapon movement (`WFPV`).
/// 11: thrown weapons' numbers (`WEQP`). 12: team deathmatch rules, friendly compass icon.
/// 13: flashbang and stun durations, their shock files. 14: sound distances, thrown weapons' world sounds.
/// 15: thrown weapons' impact damage. 16: blast cones, claymore and explosives rules.
/// 17: smoke's sight blockers (`WSMK`).
pub const REVISION: u32 = 17;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub revision: u32,
    pub inputs: u64,
}

impl Stamp {
    /// The stamp for an output made now from `zones` (paths of the zone files read).
    pub fn of(zones: &[PathBuf]) -> Stamp {
        // FNV-1a over each file's name, size and modification time (no game content).
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for z in zones {
            eat(z.file_name().map_or(&[][..], |n| n.as_encoded_bytes()));
            let meta = std::fs::metadata(z).ok();
            eat(&meta.as_ref().map_or(0, |m| m.len()).to_le_bytes());
            let mtime = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            eat(&mtime.to_le_bytes());
        }
        Stamp {
            revision: REVISION,
            inputs: h,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(12);
        o.extend_from_slice(&self.revision.to_be_bytes());
        o.extend_from_slice(&self.inputs.to_be_bytes());
        o
    }

    pub fn decode(b: &[u8]) -> Option<Stamp> {
        Some(Stamp {
            revision: u32::from_be_bytes(b.get(..4)?.try_into().ok()?),
            inputs: u64::from_be_bytes(b.get(4..12)?.try_into().ok()?),
        })
    }

    /// The stamp of an existing pack file (reads only its header, section table and stamp).
    pub fn read(path: &Path) -> Option<Stamp> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut head = vec![0u8; wpk::HEADER_LEN + 64 * wpk::SECTION_ENTRY_LEN];
        let n = f.read(&mut head).ok()?;
        let pack = wpk::Pack::parse_prefix(&head[..n]).ok()?;
        let (off, len, _) = pack.section_location(wpk::tag::STAMP)?;
        if off + len <= n {
            return Stamp::decode(&head[off..off + len]);
        }
        let mut f = std::fs::File::open(path).ok()?;
        let mut all = Vec::new();
        f.read_to_end(&mut all).ok()?;
        Stamp::decode(all.get(off..off + len)?)
    }

    /// Whether `path` exists and was made from the same inputs by this converter revision.
    pub fn up_to_date(&self, path: &Path) -> bool {
        Stamp::read(path).is_some_and(|s| s == *self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_round_trip_and_follow_their_inputs() {
        let dir = std::env::temp_dir().join(format!("mwf_stamp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zone = dir.join("zone_a.ff");
        std::fs::write(&zone, b"one").unwrap();
        let s = Stamp::of(std::slice::from_ref(&zone));
        assert_eq!(Stamp::decode(&s.encode()), Some(s));
        // A pack carrying the stamp is up to date; another input or revision is not.
        let mut b = wpk::PackBuilder::new("t");
        b.section(wpk::tag::STAMP, 0, s.encode());
        b.section(wpk::tag::SPAWNS, 0, vec![0; 100]);
        let out = dir.join("t.wpk");
        std::fs::write(&out, b.finish()).unwrap();
        assert!(s.up_to_date(&out));
        std::fs::write(&zone, b"changed").unwrap();
        assert!(!Stamp::of(std::slice::from_ref(&zone)).up_to_date(&out));
        assert!(
            !Stamp {
                revision: REVISION + 1,
                ..s
            }
            .up_to_date(&out)
        );
        assert!(!s.up_to_date(&dir.join("missing.wpk")));
        std::fs::remove_dir_all(&dir).ok();
    }
}

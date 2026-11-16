//! Attachment parts: which surfaces of a shared model each weapon variant hides.
//!
//! MW2's attachment variants (`m4_acog_mp`, `m4_reflex_silencer_mp`, ...) share their base weapon's first-person and
//! world models and differ only by the bones they hide (IW4 `hideTags`: a surface is hidden when its part bits meet
//! a hidden bone or one under it). So one model per base weapon carries every attachment, and this table says, per
//! weapon record, which of its surfaces (first-person file: `ViewModel` surfaces; held models: `GUNM` draws) to leave
//! out.
//!
//! Layout (big-endian): `u32 words`, then per row a `u32` key (`GUNH`: the weapon's `WEAP` index; `VHID`: its
//! [`name_key`]) and `words` × `u32` bits, bit `i`
//! (word `i / 32`, `1 << (i % 32)`) set when surface `i` is hidden. Rows are sorted by key. A weapon without a row
//! hides nothing.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// A `views/*.wpk` `VHID` row's key: weapon file `name` (ASCII case folded) by FNV-1a. The game knows a class's
/// weapon names before the file is in, not their `WEAP` indices.
pub fn name_key(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0193)
    })
}

/// A set of hidden surfaces, built by the converter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bits(pub Vec<u32>);

impl Bits {
    pub fn set(&mut self, i: usize) {
        let w = i / 32;
        if self.0.len() <= w {
            self.0.resize(w + 1, 0);
        }
        self.0[w] |= 1 << (i % 32);
    }

    pub fn contains(&self, i: usize) -> bool {
        self.0.get(i / 32).is_some_and(|w| w & (1 << (i % 32)) != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }
}

/// `rows`: (weapon index, hidden surfaces). Rows are sorted here; empty sets are kept (a variant that shows
/// everything still has a row, so a reader can tell it was considered).
pub fn encode(rows: &[(u32, Bits)]) -> Vec<u8> {
    let words = rows.iter().map(|(_, b)| b.0.len()).max().unwrap_or(0);
    let mut rows: Vec<&(u32, Bits)> = rows.iter().collect();
    rows.sort_by_key(|r| r.0);
    rows.dedup_by_key(|r| r.0);
    let mut b = Vec::with_capacity(4 + rows.len() * (4 + words * 4));
    b.u32(words as u32);
    for (w, bits) in rows {
        b.u32(*w);
        for k in 0..words {
            b.u32(bits.0.get(k).copied().unwrap_or(0));
        }
    }
    b
}

/// A parts table, read in place.
#[derive(Clone, Copy, Debug)]
pub struct Parts<'a> {
    b: &'a [u8],
    words: usize,
    rows: usize,
}

/// One weapon's hidden surfaces, in place.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hidden<'a> {
    b: &'a [u8],
}

impl Hidden<'_> {
    /// Nothing hidden.
    pub const NONE: Hidden<'static> = Hidden { b: &[] };

    pub fn contains(&self, i: usize) -> bool {
        let at = (i / 32) * 4;
        at + 4 <= self.b.len() && be::u32(self.b, at) & (1 << (i % 32)) != 0
    }
}

impl<'a> Parts<'a> {
    pub fn parse(b: &'a [u8]) -> Option<Parts<'a>> {
        if b.len() < 4 {
            return None;
        }
        let words = be::u32(b, 0) as usize;
        if words > 64 {
            return None;
        }
        let row = 4 + words * 4;
        let rows = (b.len() - 4) / row;
        Some(Parts {
            b: &b[4..4 + rows * row],
            words,
            rows,
        })
    }

    pub fn len(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// Key `weapon`'s hidden surfaces (none when it has no row).
    pub fn hidden(&self, weapon: u32) -> Hidden<'a> {
        self.row(weapon).unwrap_or(Hidden::NONE)
    }

    /// Key `weapon`'s row, if it has one.
    pub fn row(&self, weapon: u32) -> Option<Hidden<'a>> {
        let row = 4 + self.words * 4;
        let (mut lo, mut hi) = (0, self.rows);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let at = mid * row;
            let w = be::u32(self.b, at);
            match w.cmp(&weapon) {
                core::cmp::Ordering::Equal => {
                    return Some(Hidden {
                        b: &self.b[at + 4..at + row],
                    });
                }
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(list: &[usize]) -> Bits {
        let mut b = Bits::default();
        list.iter().for_each(|&i| b.set(i));
        b
    }

    #[test]
    fn round_trip_sorted_rows_and_wide_sets() {
        let rows = [
            (9, bits(&[0, 40])),
            (3, bits(&[1, 2])),
            (5, Bits::default()),
        ];
        let enc = encode(&rows);
        let p = Parts::parse(&enc).unwrap();
        assert_eq!(p.len(), 3);
        let h = p.hidden(3);
        assert!(h.contains(1) && h.contains(2) && !h.contains(0) && !h.contains(40));
        let h = p.hidden(9);
        assert!(h.contains(0) && h.contains(40) && !h.contains(1));
        assert!(!p.hidden(5).contains(0));
        // No row: nothing hidden.
        assert!(p.row(4).is_none() && p.row(5).is_some());
        assert!(!p.hidden(4).contains(1));
        assert!(!p.hidden(100).contains(0));
    }

    #[test]
    fn name_keys_fold_case_and_differ() {
        assert_eq!(name_key("M4_ACOG_mp"), name_key("m4_acog_mp"));
        assert_ne!(name_key("m4_acog_mp"), name_key("m4_reflex_mp"));
        assert_eq!(name_key(""), 0x811c_9dc5);
    }

    #[test]
    fn empty_and_short_tables() {
        assert!(Parts::parse(&[]).is_none());
        let enc = encode(&[]);
        let p = Parts::parse(&enc).unwrap();
        assert!(p.is_empty());
        assert!(!p.hidden(0).contains(0));
        assert!(!Hidden::NONE.contains(3));
    }
}

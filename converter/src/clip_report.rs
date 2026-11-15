//! Where a map's collision (`CLIP`, decoded into MEM1 on the Wii) goes: bytes per table, and what lossless repacking
//! could save. Measuring only: the pack's collision is unchanged (playable collision must match the PC game exactly,
//! and any change to it needs a parity trace first).

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

/// The tables of a `CLIP` section, in `wpk::clip::Clip::encode`'s order.
pub const TABLES: [&str; wpk::clip::ARRAYS] = [
    "planes",
    "plane_flags",
    "brushes",
    "nodes",
    "leaves",
    "leafbrushes",
    "mesh_verts",
    "mesh_tri_indices",
    "mesh_tri_walkable",
    "mesh_tri_surface_flags",
    "mesh_tri_content_flags",
    "mesh_aabb_trees",
    "mesh_partitions",
    "mesh_borders",
    "mesh_aabb_roots",
    "brush_models",
];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Table {
    pub count: u64,
    /// Bytes in the section, with its 32-byte padding.
    pub bytes: u64,
}

/// A lossless repacking idea and what it would save.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Saving {
    /// Distinct values (or rows) of the table.
    pub distinct: u64,
    pub bytes_saved: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipBreakdown {
    pub bytes: u64,
    pub tables: BTreeMap<String, Table>,
    /// Lossless ideas, not applied: per-plane / per-triangle u32 flags as an index into a palette of their distinct
    /// values, brush planes deduplicated behind a u16 index, mesh vertices deduplicated.
    pub savings: BTreeMap<String, Saving>,
}

/// Bytes saved storing `n` u32 values with `distinct` different ones as u8 / u16 indices into a palette.
fn palette_saving(n: usize, distinct: usize) -> u64 {
    let idx = if distinct <= 256 {
        1
    } else if distinct <= 65_536 {
        2
    } else {
        4
    };
    (n * 4).saturating_sub(n * idx + distinct * 4) as u64
}

fn distinct_u32(v: &[u32]) -> usize {
    v.iter().collect::<HashSet<_>>().len()
}

/// The breakdown of `clip` and its encoded section `section`.
pub fn breakdown(clip: &wpk::clip::Clip, section: &[u8]) -> ClipBreakdown {
    let mut out = ClipBreakdown {
        bytes: section.len() as u64,
        ..ClipBreakdown::default()
    };
    let entry = |i: usize| -> Option<(u64, u64)> {
        let at = i * 8;
        let b = section.get(at..at + 8)?;
        Some((
            u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            u64::from(u32::from_be_bytes([b[4], b[5], b[6], b[7]])),
        ))
    };
    for (i, name) in TABLES.iter().enumerate() {
        let (Some((count, off)), next) = (entry(i), entry(i + 1)) else {
            continue;
        };
        let end = next
            .map(|(_, o)| o)
            .filter(|_| i + 1 < TABLES.len())
            .unwrap_or(section.len() as u64);
        out.tables.insert(
            (*name).to_owned(),
            Table {
                count,
                bytes: end.saturating_sub(off),
            },
        );
    }
    let mut add = |name: &str, distinct: usize, saved: u64| {
        out.savings.insert(
            name.to_owned(),
            Saving {
                distinct: distinct as u64,
                bytes_saved: saved,
            },
        );
    };
    let d = distinct_u32(&clip.plane_flags);
    add(
        "plane_flags_palette",
        d,
        palette_saving(clip.plane_flags.len(), d),
    );
    let m = &clip.mesh;
    let d = distinct_u32(&m.tri_surface_flags);
    add(
        "mesh_tri_surface_flags_palette",
        d,
        palette_saving(m.tri_surface_flags.len(), d),
    );
    let d = distinct_u32(&m.tri_content_flags);
    add(
        "mesh_tri_content_flags_palette",
        d,
        palette_saving(m.tri_content_flags.len(), d),
    );
    let bits4 = |p: &[f32; 4]| p.map(f32::to_bits);
    let unique_planes = clip.planes.iter().map(bits4).collect::<HashSet<_>>().len();
    let n = clip.planes.len();
    let idx = if unique_planes <= 65_536 { 2 } else { 4 };
    add(
        "brush_planes_dedup",
        unique_planes,
        (n * 16).saturating_sub(unique_planes * 16 + n * idx) as u64,
    );
    let unique_verts = m
        .verts
        .iter()
        .map(|v| v.map(f32::to_bits))
        .collect::<HashSet<_>>()
        .len();
    add(
        "mesh_verts_dedup",
        unique_verts,
        ((m.verts.len() - unique_verts) * 12) as u64,
    );
    out
}

/// A few lines for the console.
pub fn render_text(b: &ClipBreakdown) -> String {
    let mib = |x: u64| x as f64 / (1024.0 * 1024.0);
    let mut s = format!("collision tables ({:.2} MiB in MEM1):\n", mib(b.bytes));
    let mut t: Vec<_> = b.tables.iter().filter(|(_, v)| v.bytes > 0).collect();
    t.sort_by_key(|(_, v)| std::cmp::Reverse(v.bytes));
    for (k, v) in t {
        s.push_str(&format!(
            "    {k:<24} {:>8} {:>7.2} MiB\n",
            v.count,
            mib(v.bytes)
        ));
    }
    s.push_str("  lossless repacking (not applied):\n");
    for (k, v) in &b.savings {
        s.push_str(&format!(
            "    {k:<32} {:>8} distinct, saves {:.2} MiB\n",
            v.distinct,
            mib(v.bytes_saved)
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_add_up_and_savings_count_repeats() {
        let mut clip = wpk::clip::Clip {
            planes: vec![[0.0, 0.0, 1.0, 8.0]; 10],
            plane_flags: vec![7; 10],
            ..wpk::clip::Clip::default()
        };
        clip.planes[3] = [1.0, 0.0, 0.0, 2.0];
        clip.mesh.verts = vec![[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        let section = clip.encode();
        let b = breakdown(&clip, &section);
        assert_eq!(b.bytes, section.len() as u64);
        let sum: u64 = b.tables.values().map(|t| t.bytes).sum();
        // Every byte after the table of contents belongs to one table.
        assert_eq!(
            sum + (wpk::clip::ARRAYS as u64 * 8).div_ceil(32) * 32,
            b.bytes
        );
        assert_eq!(b.tables["planes"].count, 10);
        assert_eq!(b.savings["brush_planes_dedup"].distinct, 2);
        // 10 planes of 16 bytes -> 2 unique + 10 u16 indices.
        assert_eq!(b.savings["brush_planes_dedup"].bytes_saved, 160 - (32 + 20));
        // One flag value: u8 indices + a 4-byte palette.
        assert_eq!(b.savings["plane_flags_palette"].bytes_saved, 40 - (10 + 4));
        assert_eq!(b.savings["mesh_verts_dedup"].bytes_saved, 12);
        assert!(render_text(&b).contains("planes"));
    }
}

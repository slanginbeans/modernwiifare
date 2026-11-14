//! The `VISD` section: IW4's world visibility (the GfxWorld "dpvs"): a BSP that finds the cell holding a point, and
//! the cells with their portals, which the runtime walks through the view frustum (`dpvs_iw4::visit_cells`).
//!
//! Layout: a table of [`ARRAYS`] entries (count u32, offset u32 from the section start), then the arrays, 32-byte
//! aligned, big-endian. Batches and instances name their cell in `WBCL` / `SMCL` (u16 per record, [`ALWAYS`] = not in
//! any cell, always drawn).

use alloc::vec::Vec;

use crate::be::{self, Push};

pub const ARRAYS: usize = 5;
/// "Not in a cell": drawn whenever it is in the view frustum.
pub const ALWAYS: u16 = 0xFFFF;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
    pub kind: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_portal: u32,
    pub portal_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Portal {
    pub plane: [f32; 4],
    pub neighbor: u16,
    pub first_vert: u32,
    pub vert_count: u32,
    pub hull_axis: Option<[[f32; 3]; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vis {
    pub planes: Vec<Plane>,
    /// IW4 dpvs node stream (cell leaves and plane splits, `dpvs_iw4::DpvsPlanes`).
    pub nodes: Vec<u16>,
    pub cells: Vec<Cell>,
    pub portals: Vec<Portal>,
    pub verts: Vec<[f32; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisError {
    Truncated,
    BadIndex(&'static str),
}

fn align(n: usize) -> usize {
    n.div_ceil(crate::ALIGN) * crate::ALIGN
}

impl Vis {
    pub fn validate(&self) -> Result<(), VisError> {
        for c in &self.cells {
            if c.first_portal as usize + c.portal_count as usize > self.portals.len() {
                return Err(VisError::BadIndex("cell portals"));
            }
        }
        for p in &self.portals {
            if p.first_vert as usize + p.vert_count as usize > self.verts.len() {
                return Err(VisError::BadIndex("portal vertices"));
            }
            if usize::from(p.neighbor) >= self.cells.len() {
                return Err(VisError::BadIndex("portal neighbour"));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut arrays: Vec<(usize, Vec<u8>)> = Vec::with_capacity(ARRAYS);
        let mut o = Vec::new();
        for p in &self.planes {
            p.normal.iter().for_each(|&x| o.f32(x));
            o.f32(p.dist);
            o.u32(u32::from(p.kind));
        }
        arrays.push((self.planes.len(), core::mem::take(&mut o)));
        self.nodes.iter().for_each(|&n| o.u16(n));
        arrays.push((self.nodes.len(), core::mem::take(&mut o)));
        for c in &self.cells {
            c.mins.iter().chain(&c.maxs).for_each(|&x| o.f32(x));
            o.u32(c.first_portal);
            o.u32(c.portal_count);
        }
        arrays.push((self.cells.len(), core::mem::take(&mut o)));
        for p in &self.portals {
            p.plane.iter().for_each(|&x| o.f32(x));
            o.u16(p.neighbor);
            o.u16(u16::from(p.hull_axis.is_some()));
            o.u32(p.first_vert);
            o.u32(p.vert_count);
            let h = p.hull_axis.unwrap_or_default();
            h.iter().flatten().for_each(|&x| o.f32(x));
        }
        arrays.push((self.portals.len(), core::mem::take(&mut o)));
        self.verts.iter().flatten().for_each(|&x| o.f32(x));
        arrays.push((self.verts.len(), core::mem::take(&mut o)));

        let mut out = Vec::new();
        out.zeros(align(ARRAYS * 8));
        for (i, (count, data)) in arrays.iter().enumerate() {
            let off = out.len();
            be::put_u32(&mut out, i * 8, *count as u32);
            be::put_u32(&mut out, i * 8 + 4, off as u32);
            out.extend_from_slice(data);
            out.zeros(align(out.len()) - out.len());
        }
        out
    }

    pub fn decode(b: &[u8]) -> Result<Vis, VisError> {
        if b.len() < ARRAYS * 8 {
            return Err(VisError::Truncated);
        }
        let array = |i: usize, stride: usize| -> Result<&[u8], VisError> {
            let n = be::u32(b, i * 8) as usize;
            let off = be::u32(b, i * 8 + 4) as usize;
            let end = n
                .checked_mul(stride)
                .and_then(|l| l.checked_add(off))
                .ok_or(VisError::Truncated)?;
            b.get(off..end).ok_or(VisError::Truncated)
        };
        let f3 = |r: &[u8], at: usize| [be::f32(r, at), be::f32(r, at + 4), be::f32(r, at + 8)];
        let v = Vis {
            planes: array(0, 20)?
                .chunks_exact(20)
                .map(|r| Plane {
                    normal: f3(r, 0),
                    dist: be::f32(r, 12),
                    kind: be::u32(r, 16) as u8,
                })
                .collect(),
            nodes: array(1, 2)?
                .chunks_exact(2)
                .map(|r| be::u16(r, 0))
                .collect(),
            cells: array(2, 32)?
                .chunks_exact(32)
                .map(|r| Cell {
                    mins: f3(r, 0),
                    maxs: f3(r, 12),
                    first_portal: be::u32(r, 24),
                    portal_count: be::u32(r, 28),
                })
                .collect(),
            portals: array(3, 52)?
                .chunks_exact(52)
                .map(|r| Portal {
                    plane: [be::f32(r, 0), be::f32(r, 4), be::f32(r, 8), be::f32(r, 12)],
                    neighbor: be::u16(r, 16),
                    first_vert: be::u32(r, 20),
                    vert_count: be::u32(r, 24),
                    hull_axis: (be::u16(r, 18) != 0).then(|| [f3(r, 28), f3(r, 40)]),
                })
                .collect(),
            verts: array(4, 12)?.chunks_exact(12).map(|r| f3(r, 0)).collect(),
        };
        v.validate()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trip_and_checks() {
        let v = Vis {
            planes: vec![Plane {
                normal: [1.0, 0.0, 0.0],
                dist: 5.0,
                kind: 0,
            }],
            nodes: vec![3, 4, 1, 2],
            cells: vec![
                Cell {
                    mins: [0.0; 3],
                    maxs: [1.0; 3],
                    first_portal: 0,
                    portal_count: 1,
                },
                Cell {
                    mins: [1.0; 3],
                    maxs: [2.0; 3],
                    first_portal: 1,
                    portal_count: 0,
                },
            ],
            portals: vec![Portal {
                plane: [1.0, 0.0, 0.0, -1.0],
                neighbor: 1,
                first_vert: 0,
                vert_count: 3,
                hull_axis: Some([[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            }],
            verts: vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 1.0]],
        };
        let b = v.encode();
        assert_eq!(b.len() % 32, 0);
        assert_eq!(Vis::decode(&b).unwrap(), v);
        let mut bad = v.clone();
        bad.portals[0].neighbor = 7;
        assert_eq!(
            Vis::decode(&bad.encode()).unwrap_err(),
            VisError::BadIndex("portal neighbour")
        );
    }
}

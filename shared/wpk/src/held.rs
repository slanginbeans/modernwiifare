//! Third-person weapon models (`common.wpk`): the weapon in a character's hand (each weapon's MW2 world model at a
//! low LOD), drawn rigidly at the character's `tag_weapon_right`. Read in place: the vertex arrays and display lists
//! are what GX reads.
//!
//! `GUNM` (one blob, offsets from its start, 32-aligned): `u32 count`, then per model a 24-byte header
//! `u32 vertices, u32 positions offset, u32 attributes offset, u16 draws, u16 0, u32 draws offset, u32 0`, the
//! draws (12 bytes each: `u32 display list offset, u32 length, u16 texture (GTEX index or NONE), u8 blend,
//! u8 alpha ref`), then the data: positions (f32 x, y, z in the model's space: x forward, z up, origin at
//! `tag_origin`), attributes (u32 RGBA, f32 u, f32 v), display lists (GX vertex format 6: POS, CLR0, TEX0 indexed
//! by u16, strips and triangles).
//! `GUNI`: u16 per `WEAP` record, its model (or NONE).

use alloc::vec::Vec;

use crate::be::{self, Push};

/// One model for [`encode`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub positions: Vec<[f32; 3]>,
    /// (RGBA, UV) per vertex.
    pub attributes: Vec<(u32, [f32; 2])>,
    pub draws: Vec<Draw>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draw {
    pub dl: Vec<u8>,
    pub texture: u16,
    pub blend: u8,
    pub alpha_ref: u8,
}

fn align(b: &mut Vec<u8>) {
    while !b.len().is_multiple_of(32) {
        b.u8(0);
    }
}

pub fn encode(models: &[Model]) -> Vec<u8> {
    let mut b = Vec::new();
    b.u32(models.len() as u32);
    let headers = b.len();
    b.zeros(models.len() * 24);
    for (mi, m) in models.iter().enumerate() {
        let h = headers + mi * 24;
        let draws = b.len();
        b.zeros(m.draws.len() * 12);
        align(&mut b);
        let pos = b.len();
        for p in &m.positions {
            p.iter().for_each(|&x| b.f32(x));
        }
        align(&mut b);
        let attr = b.len();
        for (rgba, uv) in &m.attributes {
            b.u32(*rgba);
            b.f32(uv[0]);
            b.f32(uv[1]);
        }
        for (di, d) in m.draws.iter().enumerate() {
            align(&mut b);
            let at = b.len();
            b.extend_from_slice(&d.dl);
            let r = draws + di * 12;
            be::put_u32(&mut b, r, at as u32);
            be::put_u32(&mut b, r + 4, d.dl.len() as u32);
            be::put_u16(&mut b, r + 8, d.texture);
            b[r + 10] = d.blend;
            b[r + 11] = d.alpha_ref;
        }
        align(&mut b);
        be::put_u32(&mut b, h, m.positions.len() as u32);
        be::put_u32(&mut b, h + 4, pos as u32);
        be::put_u32(&mut b, h + 8, attr as u32);
        be::put_u16(&mut b, h + 12, m.draws.len() as u16);
        be::put_u32(&mut b, h + 16, draws as u32);
    }
    b
}

/// The models of a `GUNM` section, in place.
#[derive(Clone, Copy, Debug)]
pub struct Guns<'a> {
    b: &'a [u8],
    count: usize,
}

/// One model: its vertex arrays and draws.
#[derive(Clone, Copy, Debug)]
pub struct GunRef<'a> {
    pub vertices: usize,
    pub positions: &'a [u8],
    pub attributes: &'a [u8],
    b: &'a [u8],
    draws_at: usize,
    draws: usize,
}

/// One draw of a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawRef<'a> {
    pub dl: &'a [u8],
    pub texture: u16,
    pub blend: u8,
    pub alpha_ref: u8,
}

impl<'a> Guns<'a> {
    /// `None` when a model's data runs past `b`.
    pub fn parse(b: &'a [u8]) -> Option<Guns<'a>> {
        if b.len() < 4 {
            return None;
        }
        let count = be::u32(b, 0) as usize;
        if b.len() < 4 + count * 24 {
            return None;
        }
        let g = Guns { b, count };
        for i in 0..count {
            let m = g.model(i)?;
            for d in 0..m.draws {
                m.draw(d)?;
            }
        }
        Some(g)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn model(&self, i: usize) -> Option<GunRef<'a>> {
        if i >= self.count {
            return None;
        }
        let h = 4 + i * 24;
        let n = be::u32(self.b, h) as usize;
        let (pos, attr) = (
            be::u32(self.b, h + 4) as usize,
            be::u32(self.b, h + 8) as usize,
        );
        let draws = usize::from(be::u16(self.b, h + 12));
        let draws_at = be::u32(self.b, h + 16) as usize;
        self.b.get(draws_at..draws_at + draws * 12)?;
        Some(GunRef {
            vertices: n,
            positions: self.b.get(pos..pos + n * 12)?,
            attributes: self.b.get(attr..attr + n * 12)?,
            b: self.b,
            draws_at,
            draws,
        })
    }
}

impl<'a> GunRef<'a> {
    pub fn draws(&self) -> usize {
        self.draws
    }

    pub fn draw(&self, i: usize) -> Option<DrawRef<'a>> {
        if i >= self.draws {
            return None;
        }
        let r = self.draws_at + i * 12;
        let (at, len) = (be::u32(self.b, r) as usize, be::u32(self.b, r + 4) as usize);
        Some(DrawRef {
            dl: self.b.get(at..at + len)?,
            texture: be::u16(self.b, r + 8),
            blend: self.b[r + 10],
            alpha_ref: self.b[r + 11],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trips_in_place() {
        let m = Model {
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            attributes: vec![(0xffffffff, [0.0, 1.0]); 3],
            draws: vec![
                Draw {
                    dl: vec![
                        0x96, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 2, 0, 2, 0, 2,
                    ],
                    texture: 4,
                    blend: 1,
                    alpha_ref: 128,
                },
                Draw::default(),
            ],
        };
        let b = encode(&[m.clone(), Model::default()]);
        let g = Guns::parse(&b).unwrap();
        assert_eq!(g.len(), 2);
        let r = g.model(0).unwrap();
        assert_eq!(r.vertices, 3);
        assert_eq!(r.positions.as_ptr() as usize % 32, b.as_ptr() as usize % 32);
        assert_eq!(be::f32(r.positions, 12), 4.0);
        assert_eq!(be::f32(r.attributes, 8), 1.0);
        let d = r.draw(0).unwrap();
        assert_eq!(
            (d.dl, d.texture, d.blend, d.alpha_ref),
            (&m.draws[0].dl[..], 4, 1, 128)
        );
        assert_eq!(g.model(1).unwrap().vertices, 0);
        assert!(Guns::parse(&b[..40]).is_none());
    }
}

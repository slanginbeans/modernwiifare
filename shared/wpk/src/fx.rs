//! Effects (`common.wpk`): MW2's effect definitions the game plays (muzzle flashes, bullet impacts, explosions), baked
//! from the user's `FxEffectDef`s at convert time. Each element keeps IW4's numbers: spawn count, delay and life
//! ranges, spawn origin and offset, the velocity graphs (local and world), gravity, rotation, the sprite atlas and
//! the visual-state samples (IW4's raw little-endian `FxElemVisStateSample`s, read by IW4L's
//! `fx_iw4::evaluate_*`). Only sprite elements (billboard, oriented, tail, cloud) and runners (child effects) are
//! kept; models, lights, decals, trails and sounds are left out.
//!
//! `FXDF`: `u32 count`, `count` × `u32` offsets (from the section start), each effect `u16 elems, u16 0`, then the
//! elements (a [`ELEM_HEADER`]-byte header, the velocity samples as BE f32 local base/amplitude then world
//! base/amplitude, the visual samples raw, padded to 4).
//! `FXWP`: per `WEAP` record, u16 × 3: its world muzzle flash, first-person flash and explosion effect (or NONE).
//! `FXIM`: IW4's impact table, [`IMPACT_ROWS`] rows (impact types) × [`IMPACT_COLUMNS`] (31 surface types, then 4
//! flesh), u16 effect each (or NONE).
//! `FTEX` / `FTXD`: the effects' textures.

use alloc::vec::Vec;

use crate::be::{self, Push};

pub const IMPACT_ROWS: usize = 15;
pub const IMPACT_COLUMNS: usize = 35;
/// Bytes of an element header.
pub const ELEM_HEADER: usize = 148;
/// Bytes of one IW4 visual-state sample.
pub const VIS_SAMPLE: usize = 0x30;

/// IW4 element types kept (`fx_iw4::FxElemType`).
pub mod kind {
    pub const BILLBOARD: u8 = 0;
    pub const ORIENTED: u8 = 1;
    pub const TAIL: u8 = 2;
    pub const CLOUD: u8 = 4;
    pub const RUNNER: u8 = 12;
}

/// A velocity graph point: base and amplitude per axis (IW4 `FxElemVec3Range`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3Range {
    pub base: [f32; 3],
    pub amplitude: [f32; 3],
}

/// One element.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Elem<'a> {
    pub kind: u8,
    /// IW4 element flags.
    pub flags: i32,
    pub looping: bool,
    /// Sprites: texture (`FTEX` index or NONE) and blend (`records::Blend`); runners: the child effect (this
    /// section's index).
    pub texture: u16,
    pub blend: u8,
    /// One-shot: count base and amplitude; looping: period (ms) and count per spawn.
    pub spawn: [i32; 2],
    pub delay_ms: [i32; 2],
    pub life_ms: [i32; 2],
    /// Base and amplitude per axis (effect axes).
    pub origin: [[f32; 2]; 3],
    pub offset_radius: [f32; 2],
    pub offset_height: [f32; 2],
    pub angles: [[f32; 2]; 3],
    pub angular_velocity: [[f32; 2]; 3],
    pub initial_rotation: [f32; 2],
    pub gravity: [f32; 2],
    /// IW4 `FxElemAtlas` (behaviour, index, fps, loop count, column bits, row bits, entry count i16 LE).
    pub atlas: [u8; 8],
    pub vis_intervals: u8,
    pub vel_local: Vec<Vec3Range>,
    pub vel_world: Vec<Vec3Range>,
    /// `vis_intervals + 1` raw samples.
    pub vis: &'a [u8],
}

/// One effect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effect<'a> {
    pub elems: Vec<Elem<'a>>,
}

pub fn encode(effects: &[Effect]) -> Vec<u8> {
    let mut b = Vec::new();
    b.u32(effects.len() as u32);
    let table = b.len();
    b.zeros(effects.len() * 4);
    for (i, e) in effects.iter().enumerate() {
        let at = b.len() as u32;
        be::put_u32(&mut b, table + i * 4, at);
        b.u16(e.elems.len() as u16);
        b.u16(0);
        for el in &e.elems {
            let start = b.len();
            b.u8(el.kind);
            b.u8(u8::from(el.looping));
            b.u8(el.blend);
            b.u8(el.vis_intervals);
            b.u32(el.flags as u32);
            b.u16(el.texture);
            b.u8(el.vel_local.len() as u8);
            b.u8(el.vel_world.len() as u8);
            for v in [el.spawn, el.delay_ms, el.life_ms] {
                v.iter().for_each(|&x| b.u32(x as u32));
            }
            let pairs = el
                .origin
                .iter()
                .chain([&el.offset_radius, &el.offset_height])
                .chain(&el.angles)
                .chain(&el.angular_velocity)
                .chain([&el.initial_rotation, &el.gravity]);
            for p in pairs {
                b.f32(p[0]);
                b.f32(p[1]);
            }
            b.extend_from_slice(&el.atlas);
            debug_assert!(b.len() - start <= ELEM_HEADER);
            b.resize(start + ELEM_HEADER, 0);
            for v in el.vel_local.iter().chain(&el.vel_world) {
                v.base.iter().chain(&v.amplitude).for_each(|&x| b.f32(x));
            }
            b.extend_from_slice(el.vis);
            while !b.len().is_multiple_of(4) {
                b.u8(0);
            }
        }
    }
    b
}

/// The effects of an `FXDF` section (visual samples borrowed from it). `None` when cut short.
pub fn decode(b: &[u8]) -> Option<Vec<Effect<'_>>> {
    let n = be::u32(b.get(..4)?, 0) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut at = be::u32(b.get(4 + i * 4..8 + i * 4)?, 0) as usize;
        let count = usize::from(be::u16(b.get(at..at + 2)?, 0));
        at += 4;
        let mut elems = Vec::with_capacity(count);
        for _ in 0..count {
            let h = b.get(at..at + ELEM_HEADER)?;
            let f = |k: usize| be::f32(h, 36 + k * 4);
            let pair = |k: usize| [f(k * 2), f(k * 2 + 1)];
            let (nl, nw) = (usize::from(h[10]), usize::from(h[11]));
            let vis_intervals = h[3];
            at += ELEM_HEADER;
            let vel = |at: &mut usize, n: usize| -> Option<Vec<Vec3Range>> {
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    let r = b.get(*at..*at + 24)?;
                    v.push(Vec3Range {
                        base: core::array::from_fn(|k| be::f32(r, k * 4)),
                        amplitude: core::array::from_fn(|k| be::f32(r, 12 + k * 4)),
                    });
                    *at += 24;
                }
                Some(v)
            };
            let vel_local = vel(&mut at, nl)?;
            let vel_world = vel(&mut at, nw)?;
            let vis_len = (usize::from(vis_intervals) + 1) * VIS_SAMPLE;
            let vis = b.get(at..at + vis_len)?;
            at += vis_len;
            at = at.div_ceil(4) * 4;
            let i32_at = |k: usize| be::u32(h, k) as i32;
            elems.push(Elem {
                kind: h[0],
                looping: h[1] != 0,
                blend: h[2],
                vis_intervals,
                flags: i32_at(4),
                texture: be::u16(h, 8),
                spawn: [i32_at(12), i32_at(16)],
                delay_ms: [i32_at(20), i32_at(24)],
                life_ms: [i32_at(28), i32_at(32)],
                origin: [pair(0), pair(1), pair(2)],
                offset_radius: pair(3),
                offset_height: pair(4),
                angles: [pair(5), pair(6), pair(7)],
                angular_velocity: [pair(8), pair(9), pair(10)],
                initial_rotation: pair(11),
                gravity: pair(12),
                atlas: h[36 + 26 * 4..36 + 26 * 4 + 8].try_into().ok()?,
                vel_local,
                vel_world,
                vis,
            });
        }
        out.push(Effect { elems });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trips() {
        let vis = [7u8; 2 * VIS_SAMPLE];
        let fx = vec![
            Effect {
                elems: vec![
                    Elem {
                        kind: kind::BILLBOARD,
                        flags: 0x0100_0000,
                        texture: 3,
                        blend: 2,
                        spawn: [2, 3],
                        delay_ms: [0, 10],
                        life_ms: [100, 50],
                        origin: [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]],
                        offset_radius: [7.0, 8.0],
                        gravity: [-1.0, 0.5],
                        initial_rotation: [0.25, 0.5],
                        angular_velocity: [[0.0, 1.0]; 3],
                        atlas: [1, 2, 3, 4, 5, 6, 7, 0],
                        vis_intervals: 1,
                        vel_local: vec![
                            Vec3Range {
                                base: [1.0, 0.0, 0.0],
                                amplitude: [0.5; 3],
                            };
                            2
                        ],
                        vel_world: vec![],
                        vis: &vis,
                        ..Elem::default()
                    },
                    Elem {
                        kind: kind::RUNNER,
                        texture: 1,
                        vis: &vis[..VIS_SAMPLE],
                        ..Elem::default()
                    },
                ],
            },
            Effect::default(),
        ];
        let b = encode(&fx);
        assert_eq!(decode(&b), Some(fx));
        assert_eq!(decode(&b[..b.len() - 3]), None);
    }
}

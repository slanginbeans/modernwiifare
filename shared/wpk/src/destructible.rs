//! Destructible objects in a map pack (`DS..` sections): the cars, barrels, electric boxes, copiers and the like that
//! MW2's `_destructible.gsc` runs, as values read from its type scripts at convert time (ADR 0007). Only each
//! object's main body for now: a chain of states, each with its health and model, and what breaking it does.
//!
//! - `DSTM` models in the [`crate::held`] layout (`tag_flash` unused), textures `DSTX` / `DSTD`.
//! - `DSTB` collision boxes per model ([`encode_boxes`]): model-space boxes from the model's collision surfaces.
//! - `DSTY` the types ([`encode_types`]), `DSTE` the placed objects ([`encode_instances`]).
//! - `DSFX` / `DSFT` / `DSFD` the map's own effects ([`crate::fx`]) and their textures, which the types play.
//!
//! Positions in a type are in its model's space (x forward, z up, at the entity's origin); the game turns them with
//! the object's axes.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Who may damage a state (`destructible_state`'s `validAttackers`).
pub mod attackers {
    pub const ANY: u8 = 0;
    /// Players (and the object itself: its own health drain).
    pub const PLAYER_ONLY: u8 = 1;
    /// Anything but players.
    pub const NO_PLAYER: u8 = 2;
}

/// What may damage a state (`validDamageCause`).
pub mod cause {
    pub const ANY: u8 = 0;
    /// Not melee (MW2's check also turns away impacts, whatever the state says: see `game_core::destructible`).
    pub const NO_MELEE: u8 = 1;
    /// Explosions only.
    pub const SPLASH: u8 = 2;
}

/// An effect played at a point of the model.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    /// Index into `DSFX`, or [`crate::records::NONE`].
    pub effect: u16,
    /// `destructible_fx`'s group: one group plays per break, picked at random.
    pub group: u8,
    /// Faces up the world (MW2's `PlayFX` without tag angles) rather than along `forward`.
    pub world_up: bool,
    pub origin: [f32; 3],
    pub forward: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoopFx {
    pub spot: Spot,
    /// Played again every this many ms.
    pub rate_ms: u32,
}

/// `destructible_explode` (MP's range): radius damage at `origin` raised by `lift` world units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Explode {
    pub origin: [f32; 3],
    pub lift: f32,
    pub range: f32,
    pub min_damage: f32,
    pub max_damage: f32,
    /// Keeps taking damage after it went off.
    pub continue_damage: bool,
}

/// One state of the body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Model shown in this state (index into `DSTM`).
    pub model: u16,
    /// Health; negative: this state can't be damaged (the last one).
    pub health: i32,
    pub attackers: u8,
    pub cause: u8,
    /// The state's tag (model space), when it names one: only then do explosions reach it (MW2 measures their falloff
    /// to it).
    pub splash_at: Option<[f32; 3]>,
    /// What breaking this state does.
    pub fx: Vec<Spot>,
    /// Effects that then repeat (replacing those running).
    pub loop_fx: Vec<LoopFx>,
    /// Health drain started (amount, every ms): the object hurts itself until it's done.
    pub drain: Option<(i32, u32)>,
    pub explode: Option<Explode>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Type {
    /// Explosion damage multiplier (`destructible_splash_damage_scaler`), or 0 for MW2's MP default.
    pub splash_scale: f32,
    pub states: Vec<State>,
}

/// A placed object.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Instance {
    pub kind: u16,
    pub origin: [f32; 3],
    /// Model-space x, y and z axes in the world (rows).
    pub axes: [[f32; 3]; 3],
}

/// A model-space collision box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CollBox {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// IW4 contents bits.
    pub contents: u32,
    /// IW4 surface flags (the surface type in bits 20..24: what a bullet's impact looks like).
    pub surface_flags: u32,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn need(&self, n: usize) -> Option<()> {
        (self.at + n <= self.b.len()).then_some(())
    }
    fn u8(&mut self) -> Option<u8> {
        self.need(1)?;
        self.at += 1;
        Some(self.b[self.at - 1])
    }
    fn u16(&mut self) -> Option<u16> {
        self.need(2)?;
        self.at += 2;
        Some(be::u16(self.b, self.at - 2))
    }
    fn u32(&mut self) -> Option<u32> {
        self.need(4)?;
        self.at += 4;
        Some(be::u32(self.b, self.at - 4))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn v3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
}

fn put_v3(b: &mut Vec<u8>, v: [f32; 3]) {
    for x in v {
        b.f32(x);
    }
}

fn put_spot(b: &mut Vec<u8>, s: &Spot) {
    b.u16(s.effect);
    b.u8(s.group);
    b.u8(u8::from(s.world_up));
    put_v3(b, s.origin);
    put_v3(b, s.forward);
}

fn spot(r: &mut Reader) -> Option<Spot> {
    Some(Spot {
        effect: r.u16()?,
        group: r.u8()?,
        world_up: r.u8()? != 0,
        origin: r.v3()?,
        forward: r.v3()?,
    })
}

pub fn encode_types(types: &[Type]) -> Vec<u8> {
    let mut b = Vec::new();
    b.u32(types.len() as u32);
    for t in types {
        b.f32(t.splash_scale);
        b.u32(t.states.len() as u32);
        for s in &t.states {
            b.u16(s.model);
            b.u8(s.attackers);
            b.u8(s.cause);
            b.u32(s.health as u32);
            b.u16(s.fx.len() as u16);
            b.u16(s.loop_fx.len() as u16);
            b.u32(u32::from(s.splash_at.is_some()));
            put_v3(&mut b, s.splash_at.unwrap_or([0.0; 3]));
            let (amount, every) = s.drain.unwrap_or((0, 0));
            b.u32(amount as u32);
            b.u32(every);
            match &s.explode {
                Some(e) => {
                    b.u8(1);
                    b.u8(u8::from(e.continue_damage));
                    put_v3(&mut b, e.origin);
                    b.f32(e.lift);
                    b.f32(e.range);
                    b.f32(e.min_damage);
                    b.f32(e.max_damage);
                }
                None => {
                    b.u8(0);
                    b.u8(0);
                }
            }
            for f in &s.fx {
                put_spot(&mut b, f);
            }
            for l in &s.loop_fx {
                put_spot(&mut b, &l.spot);
                b.u32(l.rate_ms);
            }
        }
    }
    b
}

pub fn decode_types(bytes: &[u8]) -> Option<Vec<Type>> {
    let mut r = Reader { b: bytes, at: 0 };
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let splash_scale = r.f32()?;
        let states = r.u32()? as usize;
        let mut t = Type {
            splash_scale,
            states: Vec::with_capacity(states.min(256)),
        };
        for _ in 0..states {
            let model = r.u16()?;
            let attackers = r.u8()?;
            let cause = r.u8()?;
            let health = r.u32()? as i32;
            let fx_n = usize::from(r.u16()?);
            let loop_n = usize::from(r.u16()?);
            let tagged = r.u32()? != 0;
            let at = r.v3()?;
            let splash_at = tagged.then_some(at);
            let amount = r.u32()? as i32;
            let every = r.u32()?;
            let explode = if r.u8()? != 0 {
                let continue_damage = r.u8()? != 0;
                Some(Explode {
                    continue_damage,
                    origin: r.v3()?,
                    lift: r.f32()?,
                    range: r.f32()?,
                    min_damage: r.f32()?,
                    max_damage: r.f32()?,
                })
            } else {
                r.u8()?;
                None
            };
            let mut fx = Vec::with_capacity(fx_n);
            for _ in 0..fx_n {
                fx.push(spot(&mut r)?);
            }
            let mut loop_fx = Vec::with_capacity(loop_n);
            for _ in 0..loop_n {
                let spot = spot(&mut r)?;
                loop_fx.push(LoopFx {
                    spot,
                    rate_ms: r.u32()?,
                });
            }
            t.states.push(State {
                model,
                health,
                attackers,
                cause,
                splash_at,
                fx,
                loop_fx,
                drain: (every > 0).then_some((amount, every)),
                explode,
            });
        }
        out.push(t);
    }
    Some(out)
}

pub fn encode_instances(list: &[Instance]) -> Vec<u8> {
    let mut b = Vec::new();
    for i in list {
        b.u16(i.kind);
        b.u16(0);
        put_v3(&mut b, i.origin);
        for a in i.axes {
            put_v3(&mut b, a);
        }
    }
    b
}

pub fn decode_instances(bytes: &[u8]) -> Vec<Instance> {
    let mut r = Reader { b: bytes, at: 0 };
    let mut out = Vec::new();
    let mut one = || -> Option<Instance> {
        let kind = r.u16()?;
        r.u16()?;
        Some(Instance {
            kind,
            origin: r.v3()?,
            axes: [r.v3()?, r.v3()?, r.v3()?],
        })
    };
    while let Some(i) = one() {
        out.push(i);
    }
    out
}

/// Each model's boxes: `u32 models`, per model `u32 first, u32 count`, then the boxes (mins, maxs, contents, surface
/// flags).
pub fn encode_boxes(per_model: &[Vec<CollBox>]) -> Vec<u8> {
    let mut b = Vec::new();
    b.u32(per_model.len() as u32);
    let mut first = 0u32;
    for m in per_model {
        b.u32(first);
        b.u32(m.len() as u32);
        first += m.len() as u32;
    }
    for c in per_model.iter().flatten() {
        put_v3(&mut b, c.mins);
        put_v3(&mut b, c.maxs);
        b.u32(c.contents);
        b.u32(c.surface_flags);
    }
    b
}

pub fn decode_boxes(bytes: &[u8]) -> Option<Vec<Vec<CollBox>>> {
    let mut r = Reader { b: bytes, at: 0 };
    let n = r.u32()? as usize;
    let mut ranges = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        ranges.push((r.u32()? as usize, r.u32()? as usize));
    }
    let base = r.at;
    let mut out = Vec::with_capacity(ranges.len());
    for (first, count) in ranges {
        let mut r = Reader {
            b: bytes,
            at: base + first * 32,
        };
        let mut v = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            v.push(CollBox {
                mins: r.v3()?,
                maxs: r.v3()?,
                contents: r.u32()?,
                surface_flags: r.u32()?,
            });
        }
        out.push(v);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn types_instances_and_boxes_round_trip() {
        let spot = Spot {
            effect: 3,
            group: 1,
            world_up: true,
            origin: [1.0, 2.0, 3.0],
            forward: [0.0, 0.0, 1.0],
        };
        let types = vec![Type {
            splash_scale: 15.0,
            states: vec![
                State {
                    model: 0,
                    health: 150,
                    attackers: attackers::ANY,
                    cause: cause::NO_MELEE,
                    splash_at: Some([0.0, 1.0, 2.0]),
                    fx: vec![spot],
                    loop_fx: vec![LoopFx { spot, rate_ms: 400 }],
                    drain: Some((12, 200)),
                    explode: Some(Explode {
                        origin: [0.0, 0.0, 10.0],
                        lift: 80.0,
                        range: 250.0,
                        min_damage: 50.0,
                        max_damage: 300.0,
                        continue_damage: false,
                    }),
                },
                State {
                    model: 1,
                    health: -1,
                    ..State::default()
                },
            ],
        }];
        assert_eq!(decode_types(&encode_types(&types)).unwrap(), types);
        let inst = vec![Instance {
            kind: 0,
            origin: [5.0, 6.0, 7.0],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }];
        assert_eq!(decode_instances(&encode_instances(&inst)), inst);
        let boxes = vec![
            vec![CollBox {
                mins: [-1.0; 3],
                maxs: [1.0; 3],
                contents: 1,
                surface_flags: 0,
            }],
            vec![],
            vec![
                CollBox::default(),
                CollBox {
                    contents: 5,
                    surface_flags: 13 << 20,
                    ..CollBox::default()
                },
            ],
        ];
        assert_eq!(decode_boxes(&encode_boxes(&boxes)).unwrap(), boxes);
    }
}

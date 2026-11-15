//! Destructible objects (the map pack's `DST*`, `wpk::destructible`): the cars, electric boxes, copiers and the like
//! MW2's `_destructible.gsc` runs, with its rules as values (ADR 0007). Only each object's main body for now.
//!
//! - Damage: bullets do their damage (four times for shotguns), explosions their damage times the type's splash
//!   scale or 13 (MW2's MP biases), melee is worth 100000. A state takes damage only from the attackers and causes
//!   it allows (`player_only`: players and the object's own drain; `no_melee`: not melee, and MW2's check turns away
//!   grenade impacts whenever a cause is set). Explosions only reach a body whose current state names a tag, and
//!   lose 0.4 × the distance to it (MW2's part falloff with one part).
//! - Breaking: damage past a state's health carries into the next. A broken state plays its effects (one group, at
//!   random; a tagged one stops the repeating effects), starts its repeating effects and health drain, and may
//!   explode: 0.1 s later, radius damage at its point raised by its lift, from the last player who damaged it (else
//!   the object), after which it takes no more damage. The model is then the next state's.
//! - A state without health (the last) takes no damage.
//!
//! Collision: each model's collision boxes, turned with the object, block movement ([`Objects::move_trace`]) and stop
//! bullets ([`Objects::ray`]). Not yet: separate parts (doors, tires, glass), physics pieces, sounds, animations,
//! grenades bouncing off objects.

use alloc::vec::Vec;

use wpk::destructible::{CollBox, Explode, Instance, Spot, Type, attackers, cause};

/// MW2's MP damage biases (`_destructible.gsc`).
const SHOTGUN_BIAS: f32 = 4.0;
const EXPLOSIVE_BIAS: f32 = 13.0;
/// Melee's damage to a state that allows it.
const MELEE_DAMAGE: i32 = 100_000;
/// Between a break and its explosion (two script waits).
const EXPLODE_DELAY_MS: u32 = 100;

/// What damaged an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Bullet,
    /// A shotgun's pellet (four times the damage).
    Pellet,
    Melee,
    Splash,
    /// The object's own health drain.
    Drain,
}

/// Who damaged an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attacker {
    Player(usize),
    /// The object itself (its drain).
    Itself,
    /// Something else that isn't a player (another object's explosion with no player behind it).
    Other,
}

/// What the objects ask of the game.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Out {
    /// Play map effect `effect` (`DSFX`).
    Fx {
        effect: u16,
        origin: [f32; 3],
        forward: [f32; 3],
    },
    /// Radius damage (`RadiusDamage`): `max` at `origin` down to `min` at `range`, from `attacker` (a player, or
    /// none: the object).
    Explode {
        object: usize,
        origin: [f32; 3],
        range: f32,
        min: f32,
        max: f32,
        attacker: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Running {
    spot: Spot,
    rate_ms: u32,
    next_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Object {
    inst: Instance,
    state: usize,
    health: i32,
    /// The last player who damaged it (MW2's `damageOwner`).
    owner: Option<usize>,
    /// (amount, every ms, next ms).
    drain: Option<(i32, u32, u32)>,
    loops: Vec<Running>,
    /// An explosion due: (when, what, by whom).
    pending: Option<(u32, Explode, Option<usize>)>,
    exploded: bool,
    /// Done taking damage (after an explosion).
    finished: bool,
}

/// The map's objects and their state.
#[derive(Clone, Debug, Default)]
pub struct Objects {
    types: Vec<Type>,
    boxes: Vec<Vec<CollBox>>,
    live: Vec<Object>,
    rng: u32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A box placed in the world: its object's origin and axes, and its model-space extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub origin: [f32; 3],
    pub axes: [[f32; 3]; 3],
    pub b: CollBox,
}

impl Placed {
    /// Whether the box may reach into the world-space bounds `lo..hi` (a cheap test before [`Placed::sweep`]).
    pub fn reaches(&self, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let r: f32 = (0..3)
            .map(|k| libm::fabsf(self.b.mins[k]).max(libm::fabsf(self.b.maxs[k])))
            .sum();
        (0..3).all(|k| self.origin[k] + r >= lo[k] && self.origin[k] - r <= hi[k])
    }

    /// A point in the box's (model) space.
    fn local(&self, p: [f32; 3]) -> [f32; 3] {
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        [
            dot(d, self.axes[0]),
            dot(d, self.axes[1]),
            dot(d, self.axes[2]),
        ]
    }

    /// Sweep a world-axis-aligned box `mins..maxs` from `start` to `end` against this one (slabs in model space; the
    /// moving box widened to its extent along the model's axes). (fraction, world normal, started inside), or `None`
    /// when it misses.
    pub fn sweep(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
    ) -> Option<(f32, [f32; 3], bool)> {
        let centre: [f32; 3] = core::array::from_fn(|k| (mins[k] + maxs[k]) * 0.5);
        let half: [f32; 3] = core::array::from_fn(|k| (maxs[k] - mins[k]) * 0.5);
        let a = self.local(core::array::from_fn(|k| start[k] + centre[k]));
        let b = self.local(core::array::from_fn(|k| end[k] + centre[k]));
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        let mut axis = None;
        for k in 0..3 {
            let grow: f32 = (0..3).map(|j| libm::fabsf(self.axes[k][j]) * half[j]).sum();
            let (lo, hi) = (self.b.mins[k] - grow, self.b.maxs[k] + grow);
            let d = b[k] - a[k];
            if libm::fabsf(d) < 1e-6 {
                if a[k] < lo || a[k] > hi {
                    return None;
                }
                continue;
            }
            let (mut e, mut x) = ((lo - a[k]) / d, (hi - a[k]) / d);
            let sign = if e > x {
                core::mem::swap(&mut e, &mut x);
                1.0
            } else {
                -1.0
            };
            if e > t0 {
                t0 = e;
                axis = Some((k, sign));
            }
            t1 = t1.min(x);
            if t0 > t1 {
                return None;
            }
        }
        match axis {
            // Inside from the start.
            None => Some((0.0, [0.0, 0.0, 1.0], true)),
            Some((k, sign)) => Some((t0, self.axes[k].map(|v| v * sign), false)),
        }
    }
}

impl Objects {
    /// The pack's objects: their types, each model's boxes and the placed objects.
    pub fn new(types: Vec<Type>, boxes: Vec<Vec<CollBox>>, instances: &[Instance]) -> Objects {
        let live = instances
            .iter()
            .filter(|i| {
                types
                    .get(usize::from(i.kind))
                    .is_some_and(|t| !t.states.is_empty())
            })
            .map(|&inst| Object {
                health: types[usize::from(inst.kind)].states[0].health,
                inst,
                ..Object::default()
            })
            .collect();
        Objects {
            types,
            boxes,
            live,
            rng: 0x2c1b_3c6d,
        }
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Object `i`'s placement and the model it shows (`DSTM`).
    pub fn draw(&self, i: usize) -> Option<(Instance, u16)> {
        let o = self.live.get(i)?;
        let t = &self.types[usize::from(o.inst.kind)];
        let st = &t.states[o.state.min(t.states.len() - 1)];
        Some((o.inst, st.model))
    }

    /// Object `i`'s state (0 whole).
    pub fn state(&self, i: usize) -> usize {
        self.live.get(i).map_or(0, |o| o.state)
    }

    /// Every collision box of every object as it is now.
    pub fn placed(&self) -> impl Iterator<Item = (usize, Placed)> + '_ {
        (0..self.live.len()).flat_map(move |i| {
            let (inst, model) = self.draw(i).unwrap_or_default();
            self.boxes
                .get(usize::from(model))
                .into_iter()
                .flatten()
                .map(move |b| {
                    (
                        i,
                        Placed {
                            origin: inst.origin,
                            axes: inst.axes,
                            b: *b,
                        },
                    )
                })
        })
    }

    /// The first object box a ray from `start` to `end` meets: (fraction, object, world normal, surface flags).
    pub fn ray(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mask: u32,
    ) -> Option<(f32, usize, [f32; 3], u32)> {
        let mut best: Option<(f32, usize, [f32; 3], u32)> = None;
        for (i, p) in self.placed() {
            if p.b.contents & mask == 0 {
                continue;
            }
            if let Some((t, n, inside)) = p.sweep(start, end, [0.0; 3], [0.0; 3])
                && !inside
                && best.is_none_or(|b| t < b.0)
            {
                best = Some((t, i, n, p.b.surface_flags));
            }
        }
        best
    }

    /// A movement sweep against the objects' boxes: (fraction, normal, contents) of the first hit; boxes the mover
    /// starts inside are passed through (as player bodies are).
    pub fn move_trace(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        mask: u32,
    ) -> Option<(f32, [f32; 3], u32)> {
        let mut best: Option<(f32, [f32; 3], u32)> = None;
        // Broad phase: the sweep's bounds against each object's reach.
        let lo: [f32; 3] = core::array::from_fn(|k| start[k].min(end[k]) + mins[k]);
        let hi: [f32; 3] = core::array::from_fn(|k| start[k].max(end[k]) + maxs[k]);
        for (_, p) in self.placed() {
            if p.b.contents & mask == 0 || !p.reaches(lo, hi) {
                continue;
            }
            if let Some((t, n, inside)) = p.sweep(start, end, mins, maxs)
                && !inside
                && best.is_none_or(|b| t < b.0)
            {
                best = Some((t, n, p.b.contents));
            }
        }
        best
    }

    /// Where an explosion's falloff measures to (world): the current state's tag, when it names one (MW2 damages a
    /// body by explosions only then).
    pub fn splash_point(&self, i: usize) -> Option<[f32; 3]> {
        let o = self.live.get(i)?;
        if o.finished || o.exploded {
            return None;
        }
        let p = self.types[usize::from(o.inst.kind)]
            .states
            .get(o.state)?
            .splash_at?;
        let a = o.inst.axes;
        Some(core::array::from_fn(|k| {
            o.inst.origin[k] + a[0][k] * p[0] + a[1][k] * p[1] + a[2][k] * p[2]
        }))
    }

    /// An explosion's damage to object `i`: `damage` at `origin` less 0.4 × the distance to its tag (MW2's part
    /// falloff with one part), before the type's splash scale.
    pub fn splash(
        &mut self,
        i: usize,
        origin: [f32; 3],
        damage: f32,
        attacker: Attacker,
        now: u32,
        out: &mut Vec<Out>,
    ) {
        let Some(p) = self.splash_point(i) else {
            return;
        };
        let d = libm::sqrtf(
            (0..3)
                .map(|k| (p[k] - origin[k]) * (p[k] - origin[k]))
                .sum(),
        );
        self.damage(i, damage - 0.4 * d, Cause::Splash, attacker, now, out);
    }

    fn random(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    /// `amount` of damage to object `i` by `attacker` through `cause` at `now`.
    pub fn damage(
        &mut self,
        i: usize,
        amount: f32,
        cause: Cause,
        attacker: Attacker,
        now: u32,
        out: &mut Vec<Out>,
    ) {
        let Some(o) = self.live.get(i) else {
            return;
        };
        if o.finished || amount <= 0.0 {
            return;
        }
        let t = &self.types[usize::from(o.inst.kind)];
        let mut amount = match cause {
            Cause::Pellet => amount * SHOTGUN_BIAS,
            Cause::Splash => {
                amount
                    * if t.splash_scale > 0.0 {
                        t.splash_scale
                    } else {
                        EXPLOSIVE_BIAS
                    }
            }
            _ => amount,
        } as i32;
        if amount <= 0 {
            return;
        }
        if let Attacker::Player(p) = attacker {
            self.live[i].owner = Some(p);
        }
        loop {
            let o = &self.live[i];
            let types = &self.types;
            let t = &types[usize::from(o.inst.kind)];
            let Some(st) = t.states.get(o.state) else {
                return;
            };
            if st.health < 0 {
                return;
            }
            let valid_attacker = match (st.attackers, attacker) {
                (_, Attacker::Itself) => true,
                (attackers::PLAYER_ONLY, a) => matches!(a, Attacker::Player(_)),
                (attackers::NO_PLAYER, a) => !matches!(a, Attacker::Player(_)),
                _ => true,
            };
            let valid_cause = match st.cause {
                cause::SPLASH => cause == Cause::Splash,
                cause::NO_MELEE => cause != Cause::Melee,
                _ => true,
            };
            let o = &mut self.live[i];
            if valid_attacker && valid_cause {
                if cause == Cause::Melee {
                    amount = MELEE_DAMAGE;
                }
                o.health -= amount;
            }
            if o.health > 0 {
                return;
            }
            // Broken: what's left carries over.
            amount = o.health.unsigned_abs() as i32;
            let broke = o.state;
            o.state += 1;
            self.broke(i, broke, now, out);
            let o = &mut self.live[i];
            let t = &self.types[usize::from(o.inst.kind)];
            match t.states.get(o.state) {
                Some(next) if next.health >= 0 && !o.finished => o.health = next.health,
                _ => return,
            }
        }
    }

    /// What breaking state `broke` of object `i` does.
    fn broke(&mut self, i: usize, broke: usize, now: u32, out: &mut Vec<Out>) {
        let kind = usize::from(self.live[i].inst.kind);
        let groups = self.types[kind].states[broke]
            .fx
            .iter()
            .map(|f| f.group)
            .max()
            .map_or(0, |g| u32::from(g) + 1);
        let group = if groups > 1 {
            (self.random() % groups) as u8
        } else {
            0
        };
        let o = &mut self.live[i];
        let st = &self.types[kind].states[broke];
        for f in st.fx.iter().filter(|f| f.group == group) {
            out.push(Self::fx(&o.inst, f));
            // A tagged effect stops the repeating ones (MW2's `FX_State_Change`).
            if !f.world_up || f.origin != [0.0; 3] {
                o.loops.clear();
            }
        }
        if !st.loop_fx.is_empty() {
            o.loops.clear();
            o.loops.extend(st.loop_fx.iter().map(|l| Running {
                spot: l.spot,
                rate_ms: l.rate_ms,
                next_ms: now,
            }));
        }
        if let Some((amount, every)) = st.drain {
            o.drain = Some((amount, every, now.wrapping_add(every)));
        }
        if let Some(e) = st.explode
            && (e.continue_damage || !o.exploded)
        {
            o.exploded = true;
            o.pending = Some((now.wrapping_add(EXPLODE_DELAY_MS), e, o.owner));
        }
    }

    fn fx(inst: &Instance, s: &Spot) -> Out {
        let a = inst.axes;
        let turn = |p: [f32; 3]| -> [f32; 3] {
            core::array::from_fn(|k| a[0][k] * p[0] + a[1][k] * p[1] + a[2][k] * p[2])
        };
        let o = turn(s.origin);
        Out::Fx {
            effect: s.effect,
            origin: core::array::from_fn(|k| inst.origin[k] + o[k]),
            forward: if s.world_up {
                [0.0, 0.0, 1.0]
            } else {
                turn(s.forward)
            },
        }
    }

    /// Run drains, repeating effects and due explosions up to `now`.
    pub fn update(&mut self, now: u32, out: &mut Vec<Out>) {
        let due = |t: u32| now.wrapping_sub(t) < 0x8000_0000;
        for i in 0..self.live.len() {
            let o = &mut self.live[i];
            for l in &mut o.loops {
                if due(l.next_ms) {
                    l.next_ms = now.wrapping_add(l.rate_ms.max(50));
                    out.push(Self::fx(&o.inst, &l.spot));
                }
            }
            if let Some((t, e, by)) = o.pending
                && due(t)
            {
                o.pending = None;
                if !e.continue_damage {
                    o.finished = true;
                    o.drain = None;
                }
                let a = o.inst.axes;
                let p = e.origin;
                let origin: [f32; 3] = core::array::from_fn(|k| {
                    o.inst.origin[k] + a[0][k] * p[0] + a[1][k] * p[1] + a[2][k] * p[2]
                });
                out.push(Out::Explode {
                    object: i,
                    origin: [origin[0], origin[1], origin[2] + e.lift],
                    range: e.range,
                    min: e.min_damage,
                    max: e.max_damage,
                    attacker: by,
                });
            }
            if let Some((amount, every, next)) = self.live[i].drain
                && due(next)
            {
                self.live[i].drain = Some((amount, every, now.wrapping_add(every.max(50))));
                let ty = &self.types[usize::from(self.live[i].inst.kind)];
                // The drain runs while the state has health left (MW2's loop condition).
                if ty
                    .states
                    .get(self.live[i].state)
                    .is_none_or(|s| s.health < 0)
                {
                    self.live[i].drain = None;
                    continue;
                }
                self.damage(i, amount as f32, Cause::Drain, Attacker::Itself, now, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::destructible::{LoopFx, State};

    const ID: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    fn spot(effect: u16) -> Spot {
        Spot {
            effect,
            group: 0,
            world_up: true,
            origin: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
        }
    }

    /// A car-like type: smoke at 100, fire and a drain at 50 more, then 60 for players only, then 40 and it blows up.
    fn car() -> Type {
        Type {
            splash_scale: 0.0,
            states: vec![
                State {
                    model: 0,
                    health: 100,
                    cause: cause::NO_MELEE,
                    splash_at: Some([0.0; 3]),
                    loop_fx: vec![LoopFx {
                        spot: spot(1),
                        rate_ms: 400,
                    }],
                    ..State::default()
                },
                State {
                    model: 0,
                    health: 50,
                    cause: cause::NO_MELEE,
                    drain: Some((10, 250)),
                    ..State::default()
                },
                State {
                    model: 0,
                    health: 60,
                    attackers: attackers::PLAYER_ONLY,
                    cause: cause::NO_MELEE,
                    ..State::default()
                },
                State {
                    model: 0,
                    health: 40,
                    cause: cause::NO_MELEE,
                    fx: vec![Spot {
                        world_up: false,
                        origin: [0.0, 0.0, 30.0],
                        ..spot(2)
                    }],
                    explode: Some(Explode {
                        origin: [0.0; 3],
                        lift: 80.0,
                        range: 250.0,
                        min_damage: 50.0,
                        max_damage: 300.0,
                        continue_damage: false,
                    }),
                    ..State::default()
                },
                State {
                    model: 1,
                    health: -1,
                    ..State::default()
                },
            ],
        }
    }

    fn one(t: Type) -> Objects {
        let boxes = vec![
            vec![CollBox {
                mins: [-50.0, -20.0, 0.0],
                maxs: [50.0, 20.0, 40.0],
                contents: 1,
                surface_flags: 0,
            }],
            vec![],
        ];
        Objects::new(
            vec![t],
            boxes,
            &[Instance {
                kind: 0,
                origin: [100.0, 0.0, 0.0],
                axes: ID,
            }],
        )
    }

    #[test]
    fn damage_carries_over_and_a_drain_burns_it_out() {
        let mut o = one(car());
        let mut out = Vec::new();
        // Melee does nothing to a no_melee state.
        o.damage(0, 500.0, Cause::Melee, Attacker::Player(1), 0, &mut out);
        assert_eq!(o.state(0), 0);
        // 120 bullet damage: the first state breaks (smoke starts) and 20 carries into the second.
        o.damage(0, 120.0, Cause::Bullet, Attacker::Player(1), 0, &mut out);
        assert_eq!(o.state(0), 1);
        out.clear();
        o.update(0, &mut out);
        assert!(matches!(out[0], Out::Fx { effect: 1, .. }), "{out:?}");
        // 30 more breaks the second state: the drain starts; it ticks 10 every 250 ms through the player-only state.
        o.damage(0, 30.0, Cause::Bullet, Attacker::Player(1), 100, &mut out);
        assert_eq!(o.state(0), 2);
        let mut t = 100;
        out.clear();
        while o.state(0) < 4 && t < 20_000 {
            t += 50;
            o.update(t, &mut out);
        }
        // 60 + 40 of drain: 10 ticks of 250 ms.
        assert_eq!(o.state(0), 4);
        assert!((2500..=2700).contains(&(t - 100)), "{t}");
        // The tagged explosion effect, then the blast 0.1 s later, from the last player who hurt it, raised 80.
        assert!(out.iter().any(|e| matches!(e, Out::Fx { effect: 2, .. })));
        out.clear();
        o.update(t + 100, &mut out);
        let blast = out.iter().find_map(|e| match e {
            Out::Explode {
                origin, attacker, ..
            } => Some((*origin, *attacker)),
            _ => None,
        });
        assert_eq!(blast, Some(([100.0, 0.0, 80.0], Some(1))));
        assert_eq!(o.draw(0).unwrap().1, 1);
        // Done: no more damage, no more drain.
        out.clear();
        o.damage(
            0,
            1000.0,
            Cause::Bullet,
            Attacker::Player(2),
            t + 200,
            &mut out,
        );
        o.update(t + 5000, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn player_only_states_ignore_other_attackers_and_splash_is_scaled() {
        let mut t = car();
        t.states[0].attackers = attackers::PLAYER_ONLY;
        let mut o = one(t);
        let mut out = Vec::new();
        o.damage(0, 1000.0, Cause::Splash, Attacker::Other, 0, &mut out);
        assert_eq!(o.state(0), 0);
        // 10 splash × 13 = 130 from a player: through the first state into the second.
        o.damage(0, 10.0, Cause::Splash, Attacker::Player(0), 0, &mut out);
        assert_eq!(o.state(0), 1);
        // The second state names no tag: explosions don't reach it any more.
        o.splash(
            0,
            [100.0, 0.0, 0.0],
            1000.0,
            Attacker::Player(0),
            0,
            &mut out,
        );
        assert_eq!(o.state(0), 1);
        // An explosion 50 away from the tag loses 20 before the scale.
        let mut o = one(car());
        o.splash(0, [150.0, 0.0, 0.0], 27.0, Attacker::Player(0), 0, &mut out);
        assert_eq!(o.state(0), 0, "(27 - 20) × 13 = 91 < 100");
        let mut o = one(car());
        o.splash(0, [150.0, 0.0, 0.0], 28.0, Attacker::Player(0), 0, &mut out);
        assert_eq!(o.state(0), 1);
    }

    #[test]
    fn boxes_stop_rays_and_movement() {
        let o = one(car());
        // A ray along x hits the box's near face (x 50) a quarter of the way.
        let (t, i, n, _) = o.ray([0.0, 0.0, 20.0], [200.0, 0.0, 20.0], 1).unwrap();
        assert_eq!(i, 0);
        assert!((t - 0.25).abs() < 1e-4, "{t}");
        assert_eq!(n, [-1.0, 0.0, 0.0]);
        assert!(o.ray([0.0, 0.0, 60.0], [200.0, 0.0, 60.0], 1).is_none());
        // A player hull (15 wide) stops 15 units sooner.
        let (t, _, _) = o
            .move_trace(
                [0.0, 0.0, 0.0],
                [200.0, 0.0, 0.0],
                [-15.0, -15.0, 0.0],
                [15.0, 15.0, 70.0],
                1,
            )
            .unwrap();
        assert!((t - 0.175).abs() < 1e-4, "{t}");
        // Turned 90 degrees, the box is 40 long in x.
        let mut o = one(car());
        o.live[0].inst.axes = [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let (t, _, _, _) = o.ray([0.0, 0.0, 20.0], [200.0, 0.0, 20.0], 1).unwrap();
        assert!((t - 0.4).abs() < 1e-4, "{t}");
        // The destroyed model has no boxes.
        let mut o = one(car());
        o.live[0].state = 4;
        assert!(o.ray([0.0, 0.0, 20.0], [200.0, 0.0, 20.0], 1).is_none());
    }
}

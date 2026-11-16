//! IW4L's `movement_iw4::CollisionBackend` over a pack's `CLIP` section: world brushes (through the BSP) and the
//! triangle mesh, with `clipmap_iw4::trace_brush_and_mesh`, the same trace IW4L's sim uses for the world.
//!
//! Movement also collides with the other players' bodies ([`MapClip::set_bodies`]), as IW4's player clipping does:
//! each pmove trace sweeps against every body with IW4L's `transformed_temp_capsule_trace`. Bullets and sight
//! ([`MapClip::trace`]) see the world only (players have their own hit volumes).
//!
//! Movement also stops at destructible objects' collision boxes ([`MapClip::set_props`]).
//!
//! Glass ([`crate::glass`]): a shattered pane's brushes drop out of every trace ([`MapClip::set_glass`], IW4L's
//! `glass_is_solid` filter). Bullets go through intact panes, which take their damage ([`MapClip::bullet_trace`]).
//!
//! Not yet: brush models (doors and movers; the pack carries them) and penetration recovery (`penetrations`
//! reports Unsupported, so `correct_solid` uses its trace fallback).

use alloc::vec::Vec;
use core::cell::RefCell;

use clipmap_iw4::{ClipMapRef, TraceExtents, trace_brush_and_mesh};
use movement_iw4::{CollisionBackend, GroundTraceInput};
use trace_iw4::Trace;
use wpk::clip::{BrushSlice, Clip};

/// IW4 `MASK_PLAYER_SOLID`.
pub const MASK_PLAYER_SOLID: u32 = 0x0281_0011;
/// Standing player hull (IW4 `playerMins` / `playerMaxs`).
pub const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 70.0];
/// IW4 `CONTENTS_BODY`.
pub const CONTENTS_BODY: u32 = 0x0200_0000;

/// Another player's body for movement: its origin and the top of its hull (70 standing, 50 crouched).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub origin: [f32; 3],
    pub height: f32,
}

pub struct MapClip<'a> {
    clip: &'a Clip,
    brushes: Vec<BrushSlice<'a>>,
    bodies: RefCell<Vec<Body>>,
    /// Destructible objects' collision boxes (`destructible::Objects::placed`).
    props: RefCell<Vec<crate::destructible::Placed>>,
    /// Glass pieces that no longer block (a bit per piece).
    glass_open: RefCell<Vec<u32>>,
    /// Intact panes bullets went through since [`MapClip::take_glass_hits`].
    glass_hits: RefCell<Vec<GlassHit>>,
}

/// Glass hits kept until taken (a shotgun blast through a few panes fits).
const MAX_GLASS_HITS: usize = 64;

/// A bullet through an intact pane: the piece, where, and how far along the bullet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassHit {
    pub piece: usize,
    pub at: [f32; 3],
    pub dist: f32,
}

impl<'a> MapClip<'a> {
    pub fn new(clip: &'a Clip) -> MapClip<'a> {
        MapClip {
            clip,
            brushes: clip.brush_slices(),
            bodies: RefCell::new(Vec::with_capacity(8)),
            props: RefCell::new(Vec::new()),
            glass_open: RefCell::new(Vec::new()),
            glass_hits: RefCell::new(Vec::new()),
        }
    }

    /// Which glass pieces no longer block (bits from [`crate::glass::Glass::open_bits`]).
    pub fn set_glass(&self, open: impl IntoIterator<Item = u32>) {
        let mut g = self.glass_open.borrow_mut();
        g.clear();
        g.extend(open);
    }

    /// The panes bullets went through since the last call.
    pub fn take_glass_hits(&self) -> Vec<GlassHit> {
        core::mem::take(&mut *self.glass_hits.borrow_mut())
    }

    /// A bullet's trace (IW4L `fire_extended`'s glass step): an intact pane on the way is noted
    /// ([`MapClip::take_glass_hits`]) and the trace goes on past it (`advance_trace`), up to
    /// `MAX_EXTENDED_STEPS` panes. The result is as one trace from `start` to `end`.
    pub fn bullet_trace(&self, start: [f32; 3], end: [f32; 3], mask: u32) -> Trace {
        let total = dist(start, end).max(1e-6);
        let dir: [f32; 3] = core::array::from_fn(|k| (end[k] - start[k]) / total);
        let mut from = start;
        let mut t = self.trace(from, end, [0.0; 3], [0.0; 3], mask);
        for _ in 0..weapon_iw4::MAX_EXTENDED_STEPS {
            let piece = trace_iw4::trace_get_glass_hit_id(t.hit_type, t.hit_id);
            if t.fraction >= 1.0 || piece == 0 || t.startsolid != 0 {
                break;
            }
            let at: [f32; 3] = core::array::from_fn(|k| from[k] + (end[k] - from[k]) * t.fraction);
            let mut hits = self.glass_hits.borrow_mut();
            // Bounded when nobody takes them (no match running).
            if hits.len() >= MAX_GLASS_HITS {
                hits.remove(0);
            }
            hits.push(GlassHit {
                piece: usize::from(piece - 1),
                at,
                dist: dist(start, at),
            });
            drop(hits);
            let Some(next) =
                weapon_iw4::advance_trace(at, t.normal, dir, true, weapon_iw4::ADVANCE_TRACE_FWD)
            else {
                break;
            };
            from = next;
            t = self.trace(from, end, [0.0; 3], [0.0; 3], mask);
        }
        // As one trace from `start`.
        let stop: [f32; 3] = core::array::from_fn(|k| from[k] + (end[k] - from[k]) * t.fraction);
        t.fraction = if t.fraction >= 1.0 {
            1.0
        } else {
            (dist(start, stop) / total).min(1.0)
        };
        t.endpos = stop;
        t
    }

    /// The destructible objects' boxes movement collides with (as they are now).
    pub fn set_props(&self, props: impl IntoIterator<Item = crate::destructible::Placed>) {
        let mut p = self.props.borrow_mut();
        p.clear();
        p.extend(props);
    }

    /// The bodies the next moves collide with (everyone but the mover).
    pub fn set_bodies(&self, bodies: impl IntoIterator<Item = Body>) {
        let mut b = self.bodies.borrow_mut();
        b.clear();
        b.extend(bodies);
    }

    /// A movement trace: the world, then each body (IW4 clips players against each other's hulls). A body the
    /// mover already overlaps is passed through, so players who spawn or land inside each other separate.
    pub fn move_trace(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        mask: u32,
    ) -> Trace {
        let mut t = self.trace(start, end, mins, maxs, mask);
        // Destructible objects' boxes (a box the mover starts inside is passed through).
        let lo: [f32; 3] = core::array::from_fn(|k| start[k].min(end[k]) + mins[k]);
        let hi: [f32; 3] = core::array::from_fn(|k| start[k].max(end[k]) + maxs[k]);
        for p in self.props.borrow().iter() {
            if p.b.contents & mask == 0 || !p.reaches(lo, hi) {
                continue;
            }
            if let Some((f, normal, false)) = p.sweep(start, end, mins, maxs)
                && f < t.fraction
            {
                t = Trace {
                    fraction: f,
                    normal,
                    contents: p.b.contents,
                    surface_flags: p.b.surface_flags,
                    startsolid: 0,
                    allsolid: 0,
                    walkable: u8::from(normal[2] >= 0.7),
                    endpos: core::array::from_fn(|k| start[k] + (end[k] - start[k]) * f),
                    ..t
                };
            }
        }
        if mask & CONTENTS_BODY == 0 {
            return t;
        }
        for b in self.bodies.borrow().iter() {
            let bt = clipmap_iw4::transformed_temp_capsule_trace(
                start,
                end,
                mins,
                maxs,
                b.origin,
                PLAYER_MINS,
                [PLAYER_MAXS[0], PLAYER_MAXS[1], b.height],
                CONTENTS_BODY,
                mask,
            );
            if bt.startsolid == 0 && bt.fraction < t.fraction {
                t = bt;
            }
        }
        t
    }

    pub fn trace(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        mask: u32,
    ) -> Trace {
        let map = ClipMapRef {
            nodes: &self.clip.nodes,
            leaves: &self.clip.leaves,
            leafbrushes: &self.clip.leafbrushes,
            brushes: &self.brushes,
        };
        let ext = TraceExtents::new(start, end, mins, maxs, mask);
        let open = self.glass_open.borrow();
        let solid = |piece: u16| {
            let i = usize::from(piece);
            open.get(i / 32).is_none_or(|w| w & (1 << (i % 32)) == 0)
        };
        trace_brush_and_mesh(&map, &self.clip.mesh_ref(), &ext, &solid)
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    libm::sqrtf((0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum())
}

impl CollisionBackend for MapClip<'_> {
    fn trace(&self, i: GroundTraceInput) -> Trace {
        MapClip::move_trace(self, i.start, i.end, i.mins, i.maxs, i.tracemask)
    }
}

/// Synthetic maps for tests (axis-aligned box brushes).
#[cfg(test)]
pub mod test_clips {
    use wpk::clip::Clip;

    pub fn boxes(list: &[([f32; 3], [f32; 3])]) -> Clip {
        let mut c = Clip::default();
        for (min, max) in list {
            c.push_brush(
                &[
                    [1.0, 0.0, 0.0, max[0]],
                    [-1.0, 0.0, 0.0, -min[0]],
                    [0.0, 1.0, 0.0, max[1]],
                    [0.0, -1.0, 0.0, -min[1]],
                    [0.0, 0.0, 1.0, max[2]],
                    [0.0, 0.0, -1.0, -min[2]],
                ],
                &[0; 6],
                1,
                0,
            );
        }
        c
    }

    /// An 800 × 800 floor (top at z 0) with a wall across the middle (x -8..8, y -100..100, 200 high).
    pub fn floor_with_wall() -> Clip {
        boxes(&[
            ([-400.0, -400.0, -64.0], [400.0, 400.0, 0.0]),
            ([-8.0, -100.0, 0.0], [8.0, 100.0, 200.0]),
        ])
    }

    /// A 64-unit ledge: high floor for x < 0, low floor for x > 0.
    pub fn ledge() -> Clip {
        boxes(&[
            ([-400.0, -200.0, -64.0], [0.0, 200.0, 64.0]),
            ([0.0, -200.0, -64.0], [400.0, 200.0, 0.0]),
        ])
    }
}

#[cfg(test)]
mod glass_tests {
    use super::*;

    /// A floor, a glass pane across x = 100 (piece 0) and a wall at x = 300.
    fn pane() -> Clip {
        let mut c = test_clips::boxes(&[
            ([-400.0, -400.0, -64.0], [400.0, 400.0, 0.0]),
            ([300.0, -400.0, 0.0], [316.0, 400.0, 200.0]),
        ]);
        let (lo, hi) = ([99.0, -100.0, 0.0], [101.0, 100.0, 100.0]);
        c.push_brush(
            &[
                [1.0, 0.0, 0.0, hi[0]],
                [-1.0, 0.0, 0.0, -lo[0]],
                [0.0, 1.0, 0.0, hi[1]],
                [0.0, -1.0, 0.0, -lo[1]],
                [0.0, 0.0, 1.0, hi[2]],
                [0.0, 0.0, -1.0, -lo[2]],
            ],
            &[0; 6],
            0x10,
            1,
        );
        c
    }

    #[test]
    fn bullets_go_through_intact_glass_and_note_it() {
        let c = pane();
        let mc = MapClip::new(&c);
        let (start, end) = ([0.0, 0.0, 50.0], [1000.0, 0.0, 50.0]);
        // A plain trace stops at the pane; the bullet goes on to the wall and notes the pane.
        let t = mc.trace(start, end, [0.0; 3], [0.0; 3], crate::gun::MASK_SHOT);
        assert!((t.fraction * 1000.0 - 99.0).abs() < 0.5);
        let b = mc.bullet_trace(start, end, crate::gun::MASK_SHOT);
        assert!((b.fraction * 1000.0 - 300.0).abs() < 0.5, "{}", b.fraction);
        let hits = mc.take_glass_hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].piece, 0);
        assert!((hits[0].dist - 99.0).abs() < 0.5);
        assert!(mc.take_glass_hits().is_empty());
        // Players can't walk through it ...
        let m = mc.move_trace(
            [50.0, 0.0, 1.0],
            [150.0, 0.0, 1.0],
            PLAYER_MINS,
            PLAYER_MAXS,
            MASK_PLAYER_SOLID,
        );
        assert!(m.fraction < 1.0);
        // ... until it shatters: then nothing sees it.
        mc.set_glass([1]);
        let t = mc.trace(start, end, [0.0; 3], [0.0; 3], crate::gun::MASK_SHOT);
        assert!((t.fraction * 1000.0 - 300.0).abs() < 0.5);
        let m = mc.move_trace(
            [50.0, 0.0, 1.0],
            [150.0, 0.0, 1.0],
            PLAYER_MINS,
            PLAYER_MAXS,
            MASK_PLAYER_SOLID,
        );
        assert_eq!(m.fraction, 1.0);
        mc.bullet_trace(start, end, crate::gun::MASK_SHOT);
        assert!(mc.take_glass_hits().is_empty());
    }
}

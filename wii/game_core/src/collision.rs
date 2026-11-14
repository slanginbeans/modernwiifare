//! IW4L's `movement_iw4::CollisionBackend` over a pack's `CLIP` section: world brushes (through the BSP) and the
//! triangle mesh, with `clipmap_iw4::trace_brush_and_mesh`, the same trace IW4L's sim uses for the world.
//!
//! Movement also collides with the other players' bodies ([`MapClip::set_bodies`]), as IW4's player clipping does:
//! each pmove trace sweeps against every body with IW4L's `transformed_temp_capsule_trace`. Bullets and sight
//! ([`MapClip::trace`]) see the world only (players have their own hit volumes).
//!
//! Not yet: brush models (doors and movers; the pack carries them), glass damage (glass is solid), and penetration
//! recovery (`penetrations` reports Unsupported, so `correct_solid` uses its trace fallback).

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
}

impl<'a> MapClip<'a> {
    pub fn new(clip: &'a Clip) -> MapClip<'a> {
        MapClip {
            clip,
            brushes: clip.brush_slices(),
            bodies: RefCell::new(Vec::with_capacity(8)),
        }
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
        trace_brush_and_mesh(&map, &self.clip.mesh_ref(), &ext, &|_| true)
    }
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

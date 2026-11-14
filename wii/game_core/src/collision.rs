//! IW4L's `movement_iw4::CollisionBackend` over a pack's `CLIP` section: world brushes (through the BSP) and the
//! triangle mesh, with `clipmap_iw4::trace_brush_and_mesh`, the same trace IW4L's sim uses for the world.
//!
//! Not yet: brush models (doors and movers; the pack carries them), other players, glass damage (glass is solid),
//! and penetration recovery (`penetrations` reports Unsupported, so `correct_solid` uses its trace fallback).

use alloc::vec::Vec;

use clipmap_iw4::{ClipMapRef, TraceExtents, trace_brush_and_mesh};
use movement_iw4::{CollisionBackend, GroundTraceInput};
use trace_iw4::Trace;
use wpk::clip::{BrushSlice, Clip};

/// IW4 `MASK_PLAYER_SOLID`.
pub const MASK_PLAYER_SOLID: u32 = 0x0281_0011;
/// Standing player hull (IW4 `playerMins` / `playerMaxs`).
pub const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 70.0];

pub struct MapClip<'a> {
    clip: &'a Clip,
    brushes: Vec<BrushSlice<'a>>,
}

impl<'a> MapClip<'a> {
    pub fn new(clip: &'a Clip) -> MapClip<'a> {
        MapClip {
            clip,
            brushes: clip.brush_slices(),
        }
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
        MapClip::trace(self, i.start, i.end, i.mins, i.maxs, i.tracemask)
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

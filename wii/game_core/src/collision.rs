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

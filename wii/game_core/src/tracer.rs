//! Tracers (`common.wpk` `FXTR` / `FXWT`, IW4 `TracerDef`): a glowing beam that runs down a bullet's path.
//!
//! Ported from IW4L `crates/render_fx/src/tracer.rs` (Apache-2.0, rev `acc57a5`): a shooter draws a tracer every
//! `draw_interval` shots (`TracerDrawGate::should_spawn`, a countdown per shooter); a path no longer than
//! `fx_iw4::FX_TRACER_MIN_DIST` draws none; a tracer moves from the muzzle at its speed (`fx::tracer_travel_msec`)
//! and is gone when its head reaches the hit; your own are at most `FX_TRACER_FIRST_PERSON_MAX_WIDTH` wide; its beam
//! is `beam_length` long, clipped at the hit (`tick_tracer_beams`). Its colour runs along the beam through the five
//! colours (`fx_iw4::beam_sample_color`). Drawn here as one camera-facing quad per tracer (screw wiggle left out;
//! MW2's multiplayer tracers have none), at most `fx_iw4::FX_BEAM_ADD_CAP` a frame.

use alloc::vec::Vec;

use fx_iw4::{FX_BEAM_ADD_CAP, FX_TRACER_FIRST_PERSON_MAX_WIDTH, FX_TRACER_MIN_DIST};
use wpk::fx::TracerRecord;

use crate::gfx::SpriteVert;

/// Shooters the draw gate counts for (fighters).
const SOURCES: usize = 32;
/// Tracers alive at once.
pub const MAX_TRACERS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Live {
    def: u16,
    start: [f32; 3],
    dir: [f32; 3],
    born_ms: u32,
    travel_ms: u32,
    dist: f32,
    width: f32,
}

/// The tracers in flight and each shooter's count to its next one.
#[derive(Clone, Debug)]
pub struct Tracers {
    live: Vec<Live>,
    counts: [u8; SOURCES],
}

impl Default for Tracers {
    fn default() -> Self {
        Tracers {
            live: Vec::with_capacity(MAX_TRACERS),
            counts: [0; SOURCES],
        }
    }
}

/// IW4L `fx::tracer_travel_msec`.
fn travel_msec(dist: f32, speed: f32) -> Option<u32> {
    if dist <= 0.0 || speed <= 0.0 {
        return None;
    }
    let ms = libm::roundf(dist * 1000.0 / speed) as i32;
    (ms > 0).then_some(ms as u32)
}

impl Tracers {
    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    pub fn clear(&mut self) {
        self.live.clear();
        self.counts = [0; SOURCES];
    }

    /// IW4L `TracerDrawGate::should_spawn`: whether shooter `source`'s shot draws a tracer of period `interval`.
    pub fn should_spawn(&mut self, source: usize, interval: u32) -> bool {
        if interval == 0 {
            return false;
        }
        let period = interval.min(255) as u8;
        let entry = &mut self.counts[source % SOURCES];
        if *entry == 0 || period < *entry {
            *entry = period;
        }
        *entry = entry.saturating_sub(1);
        if *entry != 0 {
            return false;
        }
        *entry = period;
        true
    }

    /// A shot by `source` from `start` to `end` with tracer `def` (index `index` of `defs`): spawns one when the gate
    /// lets it and the path is long enough. True when spawned.
    #[allow(clippy::too_many_arguments)]
    pub fn fire(
        &mut self,
        defs: &[TracerRecord],
        index: u16,
        source: usize,
        start: [f32; 3],
        end: [f32; 3],
        own: bool,
        now: u32,
    ) -> bool {
        let Some(def) = defs.get(usize::from(index)) else {
            return false;
        };
        if !self.should_spawn(source, def.draw_interval) {
            return false;
        }
        let d: [f32; 3] = core::array::from_fn(|k| end[k] - start[k]);
        let dist = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
        if dist <= FX_TRACER_MIN_DIST {
            return false;
        }
        let Some(travel_ms) = travel_msec(dist, def.speed) else {
            return false;
        };
        let mut width = def.beam_width;
        if own {
            width = width.min(FX_TRACER_FIRST_PERSON_MAX_WIDTH);
        }
        if self.live.len() >= MAX_TRACERS {
            self.live.remove(0);
        }
        self.live.push(Live {
            def: index,
            start,
            dir: d.map(|v| v / dist),
            born_ms: now,
            travel_ms,
            dist,
            width: width.max(0.0),
        });
        true
    }

    /// Drop the tracers whose head has reached the hit, and put one quad per tracer into `out` (4 vertices each,
    /// facing `eye`), with the texture and blend of the first's definition per run in `runs` ((def, first, count)).
    pub fn quads(
        &mut self,
        defs: &[TracerRecord],
        now: u32,
        eye: [f32; 3],
        out: &mut Vec<SpriteVert>,
        runs: &mut Vec<(u16, usize, usize)>,
    ) {
        out.clear();
        runs.clear();
        self.live
            .retain(|t| (now.wrapping_sub(t.born_ms) as i32) < t.travel_ms as i32);
        for t in self.live.iter().take(FX_BEAM_ADD_CAP) {
            let Some(def) = defs.get(usize::from(t.def)) else {
                continue;
            };
            let age = now.wrapping_sub(t.born_ms) as f32 / 1000.0;
            // The head runs at the tracer's speed; the beam trails it, never past the muzzle or the hit.
            let head = (def.speed * age).min(t.dist);
            let len = def.beam_length.max(0.0).min(head);
            if len <= 0.0 || t.width <= 0.0 {
                continue;
            }
            let at = |s: f32| -> [f32; 3] { core::array::from_fn(|k| t.start[k] + t.dir[k] * s) };
            let (back, front) = (at(head - len), at(head));
            let mid: [f32; 3] = core::array::from_fn(|k| (back[k] + front[k]) * 0.5);
            let to_eye: [f32; 3] = core::array::from_fn(|k| eye[k] - mid[k]);
            let f = t.dir;
            let side = [
                f[1] * to_eye[2] - f[2] * to_eye[1],
                f[2] * to_eye[0] - f[0] * to_eye[2],
                f[0] * to_eye[1] - f[1] * to_eye[0],
            ];
            let l = libm::sqrtf(side.iter().map(|v| v * v).sum::<f32>());
            if l < 1e-4 {
                continue;
            }
            let half = t.width * 0.5 / l;
            let side = side.map(|v| v * half);
            // Colour along the beam: IW4's five colours from its back (0) to its head (1).
            let rgba = |c: [f32; 4]| {
                let b = fx_iw4::beam_color_rgba(c);
                u32::from_be_bytes(b)
            };
            let c_back = rgba(fx_iw4::beam_sample_color(&def.colors, 0.0));
            let c_front = rgba(fx_iw4::beam_sample_color(&def.colors, 1.0));
            let first = out.len();
            for (p, s, uv, c) in [
                (back, 1.0, [0.0, 0.0], c_back),
                (front, 1.0, [1.0, 0.0], c_front),
                (front, -1.0, [1.0, 1.0], c_front),
                (back, -1.0, [0.0, 1.0], c_back),
            ] {
                out.push(SpriteVert {
                    pos: core::array::from_fn(|k| p[k] + side[k] * s),
                    rgba: c,
                    uv,
                });
            }
            match runs.last_mut() {
                Some(r) if r.0 == t.def => r.2 += 4,
                _ => runs.push((t.def, first, 4)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn def(interval: u32) -> TracerRecord {
        TracerRecord {
            texture: 0,
            blend: 2,
            draw_interval: interval,
            speed: 1000.0,
            beam_length: 100.0,
            beam_width: 8.0,
            colors: [[1.0, 1.0, 1.0, 1.0]; 5],
            ..TracerRecord::default()
        }
    }

    #[test]
    fn gate_draws_every_interval_per_shooter() {
        let mut t = Tracers::default();
        // IW4L: period 3 draws on the third shot, then every third.
        let got: Vec<bool> = (0..7).map(|_| t.should_spawn(1, 3)).collect();
        assert_eq!(got, [false, false, true, false, false, true, false]);
        // Another shooter counts on its own; interval 1 draws every shot; 0 never.
        assert!(t.should_spawn(2, 1) && t.should_spawn(2, 1));
        assert!(!t.should_spawn(3, 0));
    }

    #[test]
    fn tracer_flies_clips_and_expires() {
        let defs = vec![def(1)];
        let mut t = Tracers::default();
        // Too short a path draws nothing.
        assert!(!t.fire(&defs, 0, 0, [0.0; 3], [4.0, 0.0, 0.0], false, 0));
        assert!(t.fire(&defs, 0, 0, [0.0; 3], [500.0, 0.0, 0.0], false, 1000));
        let (mut out, mut runs) = (Vec::new(), Vec::new());
        // 50 ms: the head at 50 units, the beam 50 long (not past the muzzle).
        t.quads(&defs, 1050, [0.0, 0.0, 100.0], &mut out, &mut runs);
        assert_eq!(out.len(), 4);
        assert_eq!(runs, [(0, 0, 4)]);
        let xs: Vec<f32> = out.iter().map(|v| v.pos[0]).collect();
        assert!(
            (xs[0] - 0.0).abs() < 1e-3 && (xs[1] - 50.0).abs() < 1e-3,
            "{xs:?}"
        );
        // Width 8 across the view (here along y).
        assert!((out[0].pos[1] - out[3].pos[1]).abs() - 8.0 < 1e-3);
        // 300 ms: a full 100-unit beam from 200 to 300.
        t.quads(&defs, 1300, [0.0, 0.0, 100.0], &mut out, &mut runs);
        assert!((out[0].pos[0] - 200.0).abs() < 1e-3 && (out[1].pos[0] - 300.0).abs() < 1e-3);
        // 500 units at 1000/s: gone at 500 ms.
        t.quads(&defs, 1500, [0.0, 0.0, 100.0], &mut out, &mut runs);
        assert!(out.is_empty() && t.is_empty());
        // Your own are at most 4 wide.
        t.fire(&defs, 0, 0, [0.0; 3], [500.0, 0.0, 0.0], true, 2000);
        t.quads(&defs, 2100, [0.0, 0.0, 100.0], &mut out, &mut runs);
        assert!(((out[0].pos[1] - out[3].pos[1]).abs() - 4.0).abs() < 1e-3);
    }
}

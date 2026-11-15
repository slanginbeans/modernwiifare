//! The AUTO draw distance: the world's far plane follows the frame time. When frames miss 60 fps and the cost is one a
//! shorter distance relieves (the CPU waits on the GPU, or the CPU's world draw submission is a large share of the
//! frame, as on `mp_terminal`), the distance shrinks in proportion to the miss; while
//! frames hold 60 fps it grows back slowly. Fog follows the far plane ([`crate::world::World::fog_ramp`]), so the
//! distance moves in small steps a few seconds apart and never below [`MIN`].

/// The shortest AUTO distance (world units: about 64 m, most of a small map).
pub const MIN: f32 = 2500.0;
/// The longest (past it the far plane is the full one).
pub const MAX: f32 = crate::camera::ZFAR;
/// Where AUTO starts on a map.
pub const START: f32 = 8000.0;

/// A 60 fps frame with a little slack (frames are capped at one per retrace: 16.7 ms when on time).
const ON_TIME_US: f32 = 17_500.0;
/// The CPU waiting this long on the GPU means the GPU is the bottleneck.
const GPU_BOUND_US: f32 = 1_500.0;
/// World draw submission (CPU) taking this share of the frame means it is the bottleneck: fewer cells, batches and
/// model instances in range cut it. Hardware log, `mp_terminal`: 19-42 ms of 25-45 ms frames with no GPU wait.
const WORLD_SHARE: f32 = 0.35;
/// ...and at least this long (so a cheap world in a slow frame for other reasons, e.g. bots, isn't blamed).
const WORLD_BOUND_US: f32 = 4_000.0;
/// Growth: this many milliseconds of on-time frames, then this factor.
const GROW_AFTER_MS: u32 = 3_000;
const GROW: f32 = 1.05;
/// Shrink at most this often, by at most this much.
const SHRINK_EVERY_MS: u32 = 500;
const SHRINK_MIN: f32 = 0.8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoDistance {
    pub distance: f32,
    frame_avg: f32,
    gpu_avg: f32,
    world_avg: f32,
    on_time_ms: u32,
    since_shrink_ms: u32,
}

impl Default for AutoDistance {
    fn default() -> Self {
        AutoDistance {
            distance: START,
            frame_avg: ON_TIME_US,
            gpu_avg: 0.0,
            world_avg: 0.0,
            on_time_ms: 0,
            since_shrink_ms: 0,
        }
    }
}

impl AutoDistance {
    /// After a frame of `frame_us` (start to start) that waited `gpu_wait_us` on the GPU and spent `world_us` of CPU
    /// submitting the world: the distance to draw next.
    pub fn update(&mut self, frame_us: u32, gpu_wait_us: u32, world_us: u32) -> f32 {
        let ms = (frame_us / 1000).max(1);
        // About a quarter-second average.
        let k = 0.12;
        self.frame_avg += (frame_us as f32 - self.frame_avg) * k;
        self.gpu_avg += (gpu_wait_us as f32 - self.gpu_avg) * k;
        self.world_avg += (world_us as f32 - self.world_avg) * k;
        self.since_shrink_ms = self.since_shrink_ms.saturating_add(ms);
        if self.frame_avg > ON_TIME_US {
            self.on_time_ms = 0;
            let gpu_bound = self.gpu_avg > GPU_BOUND_US;
            let world_bound =
                self.world_avg > WORLD_BOUND_US && self.world_avg > self.frame_avg * WORLD_SHARE;
            if (gpu_bound || world_bound) && self.since_shrink_ms >= SHRINK_EVERY_MS {
                self.since_shrink_ms = 0;
                let f = (ON_TIME_US / self.frame_avg).clamp(SHRINK_MIN, 0.97);
                self.distance = (self.distance * f).max(MIN);
            }
        } else {
            self.on_time_ms = self.on_time_ms.saturating_add(ms);
            if self.on_time_ms >= GROW_AFTER_MS {
                self.on_time_ms = 0;
                self.distance = (self.distance * GROW).min(MAX);
            }
        }
        self.distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinks_when_gpu_bound_grows_when_on_time() {
        let mut a = AutoDistance::default();
        // 25 ms frames waiting 8 ms on the GPU for 5 s: shrinks, not below the floor.
        for _ in 0..200 {
            a.update(25_000, 8_000, 0);
        }
        assert!(
            a.distance < START * 0.6 && a.distance >= MIN,
            "{}",
            a.distance
        );
        let low = a.distance;
        // CPU-bound misses that aren't the world's (no GPU wait, cheap world) leave it alone.
        for _ in 0..200 {
            a.update(25_000, 0, 0);
        }
        assert_eq!(a.distance, low);
        // 60 fps for 10 s: grows back a few steps.
        for _ in 0..600 {
            a.update(16_700, 500, 0);
        }
        assert!(a.distance > low * 1.1, "{} vs {low}", a.distance);
        // Never past the far plane.
        for _ in 0..200_000 {
            a.update(16_700, 0, 0);
        }
        assert_eq!(a.distance, MAX);
    }

    #[test]
    fn shrinks_when_world_submission_is_the_cpu_cost() {
        // mp_terminal's hardware log: ~37 ms frames, ~30 ms of them world submission, no GPU wait.
        let mut a = AutoDistance::default();
        for _ in 0..200 {
            a.update(37_000, 0, 30_000);
        }
        assert!(
            a.distance < START * 0.6 && a.distance >= MIN,
            "{}",
            a.distance
        );

        // Slow frames whose CPU goes elsewhere (bots), with a cheap world: no change.
        let mut b = AutoDistance::default();
        for _ in 0..200 {
            b.update(30_000, 0, 3_000);
        }
        assert_eq!(b.distance, START);
        // A small share of a long frame isn't the world's fault either.
        for _ in 0..200 {
            b.update(40_000, 0, 6_000);
        }
        assert_eq!(b.distance, START);
    }
}

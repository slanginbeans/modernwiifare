//! Smoke: the particles of a smoke grenade's explosion effect that block sight, as IW4 runs them (IW4L `fx_iw4`
//! vis blockers). Each sight-blocking element of the effect (`WSMK`) spawns its particles around the blast after
//! its delay; each lives its life, growing and fading by the element's size and alpha curves. A sight line's
//! visibility is IW4L's `get_client_visibility`: every particle (inside its distance fade from the viewer) whose
//! sphere the line passes through takes its alpha off. A line is blocked when its visibility drops under IW4L's
//! `FX_CLIENT_VISIBILITY_THRESHOLD`.
//!
//! Not modelled: particles' own motion (their velocity graphs), looping sight-blocking elements (MW2's smoke has
//! only one-shot ones).

use alloc::vec::Vec;

use fx_iw4::{
    FX_CLIENT_VISIBILITY_THRESHOLD, FX_DISTANCE_FADE_SCALE, FX_ELEM_FLAG_VIS_BLOCKER,
    FX_VIS_MIN_TRACE_DIST_DEFAULT, FxVisBlockerBuf, get_client_visibility, vis_blocker_add,
    vis_blocker_add_prepared, vis_blocker_param4,
};
use wpk::weapon::BlockerRecord;

/// One smoke particle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub origin: [f32; 3],
    /// When it appears and how long it lives.
    pub born_ms: u32,
    pub life_ms: u32,
    /// Its size (radius) and alpha curves.
    pub size: [f32; 5],
    pub alpha: [u8; 5],
    pub fade_in: [f32; 2],
    pub fade_out: [f32; 2],
}

impl Particle {
    /// Its life fraction at `now`, or None before it appears / after it died.
    fn age(&self, now: u32) -> Option<f32> {
        let a = now.wrapping_sub(self.born_ms) as i32;
        if a < 0 || a as u32 >= self.life_ms {
            return None;
        }
        Some(a as f32 / self.life_ms.max(1) as f32)
    }

    /// Its radius and alpha at `now` (the curves sampled at 0, ¼, ½, ¾ and 1 of its life, linearly between).
    pub fn state(&self, now: u32) -> Option<(f32, u8)> {
        let t = self.age(now)? * 4.0;
        let k = (libm::floorf(t) as usize).min(3);
        let f = t - k as f32;
        let size = self.size[k] + (self.size[k + 1] - self.size[k]) * f;
        let a0 = f32::from(self.alpha[k]);
        let a1 = f32::from(self.alpha[k + 1]);
        Some((size, libm::roundf(a0 + (a1 - a0) * f) as u8))
    }

    fn dead(&self, now: u32) -> bool {
        now.wrapping_sub(self.born_ms) as i32 >= self.life_ms as i32
    }
}

/// Every smoke particle in the match.
#[derive(Clone, Debug, Default)]
pub struct Smoke {
    pub particles: Vec<Particle>,
    rng: u32,
}

/// A record's `(base, amplitude)` at random `r` in 0..1.
fn range_i(v: [i32; 2], r: f32) -> i32 {
    v[0] + (v[1] as f32 * r) as i32
}
fn range_f(v: [f32; 2], r: f32) -> f32 {
    v[0] + v[1] * r
}

impl Smoke {
    fn rand(&mut self) -> f32 {
        // xorshift32 (deterministic per match; the particles' exact spots don't matter).
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A smoke grenade went off at `origin` at `now`: spawn the particles of its sight-blocking elements.
    pub fn spawn<'a>(
        &mut self,
        origin: [f32; 3],
        elements: impl IntoIterator<Item = &'a BlockerRecord>,
        now: u32,
    ) {
        for e in elements {
            if e.looping {
                continue;
            }
            let count = (e.spawn_a + (e.spawn_b as f32 * self.rand()) as i32).max(0);
            for _ in 0..count {
                let delay = range_i(e.delay_ms, self.rand()).max(0) as u32;
                let life = range_i(e.life_ms, self.rand()).max(1) as u32;
                let r = range_f(e.offset_radius, self.rand());
                let h = range_f(e.offset_height, self.rand());
                let a = self.rand() * core::f32::consts::TAU;
                self.particles.push(Particle {
                    origin: [
                        origin[0] + r * libm::cosf(a),
                        origin[1] + r * libm::sinf(a),
                        origin[2] + h,
                    ],
                    born_ms: now.wrapping_add(delay),
                    life_ms: life,
                    // The curves at the mid random point the converter sampled them at.
                    size: e.size,
                    alpha: e.alpha,
                    fade_in: e.fade_in,
                    fade_out: e.fade_out,
                });
            }
        }
    }

    /// Drop the particles that died by `now`.
    pub fn update(&mut self, now: u32) {
        self.particles.retain(|p| !p.dead(now));
    }

    /// The sight-blocker buffer a viewer at `camera` sees at `now` (IW4L `vis_blocker_add_prepared` per particle).
    /// Deviation: an element without a distance fade (MW2's smoke has none) counts at full strength; IW4L's
    /// `vis_blocker_add_prepared` skips it, which would leave smoke blocking nothing (`docs/runtime.md`, smoke).
    pub fn blockers(&self, camera: [f32; 3], now: u32) -> FxVisBlockerBuf {
        let mut buf = FxVisBlockerBuf::default();
        for p in &self.particles {
            let Some((size, alpha)) = p.state(now) else {
                continue;
            };
            let faded = p.fade_in[1] != 0.0 || p.fade_out[1] != 0.0;
            if faded {
                vis_blocker_add_prepared(
                    &mut buf,
                    FX_ELEM_FLAG_VIS_BLOCKER,
                    p.origin,
                    size,
                    alpha,
                    p.fade_in,
                    p.fade_out,
                    camera,
                );
            } else {
                vis_blocker_add(
                    &mut buf,
                    p.origin,
                    size,
                    vis_blocker_param4(alpha, FX_DISTANCE_FADE_SCALE as u32),
                );
            }
        }
        buf
    }

    /// How much of a sight line from `start` to `end` the smoke lets through (1: clear).
    pub fn visibility(&self, start: [f32; 3], end: [f32; 3], now: u32) -> f32 {
        if self.particles.is_empty() {
            return 1.0;
        }
        get_client_visibility(
            &self.blockers(start, now),
            start,
            end,
            FX_VIS_MIN_TRACE_DIST_DEFAULT,
        )
    }

    /// Whether the smoke hides `end` from `start`.
    pub fn blocks(&self, start: [f32; 3], end: [f32; 3], now: u32) -> bool {
        self.visibility(start, end, now) < FX_CLIENT_VISIBILITY_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element() -> BlockerRecord {
        BlockerRecord {
            weapon: 1,
            looping: false,
            spawn_a: 40,
            spawn_b: 0,
            delay_ms: [1000, 0],
            life_ms: [8000, 0],
            offset_radius: [50.0, 100.0],
            offset_height: [0.0, 0.0],
            fade_in: [0.0, 0.0],
            fade_out: [10_000.0, 1000.0],
            size: [50.0, 100.0, 150.0, 200.0, 0.0],
            alpha: [0, 255, 255, 255, 0],
        }
    }

    /// MW2's smoke grenade from a locally converted `out/packs/common.wpk` (never in git; skipped when absent).
    #[test]
    fn real_smoke_blocks_a_line_through_it() {
        extern crate std;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/common.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local common pack, skipped");
            return;
        };
        let pack = wpk::Pack::parse(&bytes, true).unwrap();
        let Some(sec) = pack.section(wpk::tag::WEAPON_BLOCKERS) else {
            std::println!("old pack, skipped");
            return;
        };
        let weap = pack.section(wpk::tag::WEAPONS).unwrap().data;
        let (w, _) = wpk::weapon::find_indexed(weap, "smoke_grenade_mp").expect("smoke");
        let recs: Vec<BlockerRecord> = wpk::records::iter::<BlockerRecord>(sec.data)
            .filter(|b| b.weapon == w)
            .collect();
        for r in &recs {
            std::println!("smoke element: {r:?}");
        }
        assert!(!recs.is_empty());
        let mut s = Smoke::default();
        s.spawn([0.0, 0.0, 0.0], &recs, 0);
        let (a, b) = ([-800.0, 0.0, 40.0], [800.0, 0.0, 40.0]);
        let mut first = None;
        let mut last = None;
        for t in (0..20_000).step_by(250) {
            s.update(t);
            let v = s.visibility(a, b, t);
            if s.blocks(a, b, t) {
                first.get_or_insert(t);
                last = Some(t);
            }
            if t % 2000 == 0 {
                std::println!("{t} ms: visibility {v}");
            }
        }
        std::println!("blocked from {first:?} to {last:?} ms");
        assert!(first.is_some(), "the smoke blocks sight at some point");
    }

    #[test]
    fn smoke_hides_what_is_behind_it_while_it_lasts() {
        let mut s = Smoke::default();
        let e = element();
        s.spawn([0.0, 0.0, 0.0], [&e], 0);
        assert_eq!(s.particles.len(), 40);
        let (a, b) = ([-600.0, 0.0, 40.0], [600.0, 0.0, 40.0]);
        assert!(!s.blocks(a, b, 500), "not yet (the delay)");
        assert!(s.blocks(a, b, 4000), "thick smoke");
        // A line well clear of the cloud.
        assert!(!s.blocks([-600.0, 900.0, 40.0], [600.0, 900.0, 40.0], 4000));
        // Gone after the particles' lives.
        s.update(9500);
        assert!(s.particles.is_empty());
        assert!(!s.blocks(a, b, 9500));
    }
}

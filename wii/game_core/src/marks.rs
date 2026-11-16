//! Bullet marks on the world (the impact effect's decal, `wpk::fx::Mark`): a small pool of flat polygons laid on the
//! surface a bullet hit, kept inside that surface.
//!
//! IW4 clips a mark against the world's drawn triangles inside its box (IW4L `marks_iw4::mark_clip_world_surfaces`).
//! The Wii keeps no triangles on the CPU, so the mark starts as a square on the hit plane (radius from the decal's
//! size, a random roll as IW4 gives marks) and is cut back where the surface ends: for each of its four sides, short
//! traces across the surface (`probe`, the world's collision) find how far the same plane goes, and the square is
//! chopped there with IW4L's `marks_iw4::mark_chop_world_poly_behind_plane` (also used for IW4's own box planes,
//! `mark_fragment_clip_planes`). A mark that would hang over an edge on every side, or whose centre isn't on a
//! surface, is dropped. Marks fade out over their last second; the oldest makes way when the pool is full.

use alloc::vec::Vec;

use marks_iw4::{
    FxWorldMarkPoint, R_MARK_CHOP_MAX_POINTS, mark_chop_world_poly_behind_plane,
    mark_fragment_clip_planes,
};

use crate::gfx::SpriteVert;

/// Marks at once.
pub const MAX_MARKS: usize = 48;
/// How long a mark stays, and how long it takes to fade at the end (ms).
pub const MARK_LIFE_MS: u32 = 30_000;
const FADE_MS: u32 = 1_000;
/// How far marks sit off their surface (units), against depth fighting.
const OFFSET: f32 = 0.25;
/// Halvings when searching for where a surface ends.
const SEARCH_STEPS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub points: [[f32; 3]; R_MARK_CHOP_MAX_POINTS],
    pub uv: [[f32; 2]; R_MARK_CHOP_MAX_POINTS],
    pub count: u8,
    pub texture: u16,
    pub blend: u8,
    pub born_ms: u32,
}

/// A trace for marks: from `start` to `end` through the world, `Some((point, normal))` where it hits.
pub trait Probe {
    fn hit(&self, start: [f32; 3], end: [f32; 3]) -> Option<([f32; 3], [f32; 3])>;
}

impl<F: Fn([f32; 3], [f32; 3]) -> Option<([f32; 3], [f32; 3])>> Probe for F {
    fn hit(&self, start: [f32; 3], end: [f32; 3]) -> Option<([f32; 3], [f32; 3])> {
        self(start, end)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Marks {
    pub list: Vec<Mark>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn mad(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

/// Whether the same surface (plane `normal` through `origin`) is under `q`: a short trace into it hits a point on
/// that plane with that normal.
fn on_surface(probe: &impl Probe, origin: [f32; 3], normal: [f32; 3], q: [f32; 3]) -> bool {
    match probe.hit(mad(q, normal, 2.0), mad(q, normal, -2.0)) {
        Some((p, n)) => {
            dot(n, normal) > 0.95 && libm::fabsf(dot(normal, p) - dot(normal, origin)) < 1.0
        }
        None => false,
    }
}

impl Marks {
    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// Lay a mark of `radius` (rolled by `roll` radians) on the surface hit at `origin` with `normal`. False when
    /// the surface can't hold it.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        probe: &impl Probe,
        origin: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        roll: f32,
        texture: u16,
        blend: u8,
        now: u32,
    ) -> bool {
        if radius <= 0.1 || !on_surface(probe, origin, normal, origin) {
            return false;
        }
        // The mark's axes: the normal, and two across the surface turned by its roll.
        let [_, r, u] = fx_iw4::vector_vectors(normal);
        let (s, c) = (libm::sinf(roll), libm::cosf(roll));
        let t: [f32; 3] = core::array::from_fn(|k| r[k] * c + u[k] * s);
        let b: [f32; 3] = core::array::from_fn(|k| u[k] * c - r[k] * s);
        let mut pts = [FxWorldMarkPoint::ZERO; R_MARK_CHOP_MAX_POINTS];
        for (i, (dt, db)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .enumerate()
        {
            pts[i].xyz = mad(mad(origin, t, dt * radius), b, db * radius);
        }
        let mut n = 4;
        // IW4's box planes around the mark (a no-op for the square, kept as IW4 clips by them).
        let mut planes: Vec<[f32; 4]> = mark_fragment_clip_planes(origin, [normal, t, b], radius)
            .iter()
            .skip(2)
            .copied()
            .collect();
        // Where the surface ends towards each side.
        let mut full = 0;
        for dir in [t, t.map(|v| -v), b, b.map(|v| -v)] {
            if on_surface(probe, origin, normal, mad(origin, dir, radius)) {
                full += 1;
                continue;
            }
            let (mut lo, mut hi) = (0.0f32, radius);
            for _ in 0..SEARCH_STEPS {
                let mid = (lo + hi) * 0.5;
                if on_surface(probe, origin, normal, mad(origin, dir, mid)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            // Keep dir·x < dir·origin + lo.
            planes.push([-dir[0], -dir[1], -dir[2], -(dot(dir, origin) + lo)]);
        }
        if full == 0 {
            return false;
        }
        let mut tmp = [FxWorldMarkPoint::ZERO; R_MARK_CHOP_MAX_POINTS];
        for p in &planes {
            n = mark_chop_world_poly_behind_plane(n, &pts, *p, &mut tmp);
            if n < 3 {
                return false;
            }
            pts = tmp;
        }
        let mut m = Mark {
            points: [[0.0; 3]; R_MARK_CHOP_MAX_POINTS],
            uv: [[0.0; 2]; R_MARK_CHOP_MAX_POINTS],
            count: n as u8,
            texture,
            blend,
            born_ms: now,
        };
        for (i, pt) in pts.iter().enumerate().take(n) {
            let d: [f32; 3] = core::array::from_fn(|k| pt.xyz[k] - origin[k]);
            m.points[i] = mad(pt.xyz, normal, OFFSET);
            m.uv[i] = [
                0.5 + dot(d, t) / (2.0 * radius),
                0.5 - dot(d, b) / (2.0 * radius),
            ];
        }
        if self.list.len() >= MAX_MARKS {
            self.list.remove(0);
        }
        self.list.push(m);
        true
    }

    /// Drop the marks that have lived out and put the rest into `out` as quads (a polygon of n points as a fan of
    /// quads, the last one doubling a point when n is odd), grouped into `runs` ((texture, blend, first, count)).
    pub fn quads(
        &mut self,
        now: u32,
        out: &mut Vec<SpriteVert>,
        runs: &mut Vec<(u16, u8, usize, usize)>,
    ) {
        out.clear();
        runs.clear();
        self.list
            .retain(|m| now.wrapping_sub(m.born_ms) < MARK_LIFE_MS);
        for m in &self.list {
            let left = MARK_LIFE_MS - now.wrapping_sub(m.born_ms);
            let a = if left < FADE_MS {
                left * 255 / FADE_MS
            } else {
                255
            };
            let rgba = 0xffff_ff00 | a;
            let first = out.len();
            let n = usize::from(m.count);
            let mut i = 1;
            while i + 1 < n {
                let k = [0, i, i + 1, (i + 2).min(n - 1)];
                for &j in &k {
                    out.push(SpriteVert {
                        pos: m.points[j],
                        rgba,
                        uv: m.uv[j],
                    });
                }
                i += 2;
            }
            let count = out.len() - first;
            match runs.last_mut() {
                Some(r) if r.0 == m.texture && r.1 == m.blend => r.3 += count,
                _ => runs.push((m.texture, m.blend, first, count)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor at z = 0 for x in -10..10 (a ledge), y anything.
    fn ledge(start: [f32; 3], end: [f32; 3]) -> Option<([f32; 3], [f32; 3])> {
        if start[2] > 0.0 && end[2] <= 0.0 {
            let f = start[2] / (start[2] - end[2]);
            let p: [f32; 3] = core::array::from_fn(|k| start[k] + (end[k] - start[k]) * f);
            (p[0] > -10.0 && p[0] < 10.0).then_some((p, [0.0, 0.0, 1.0]))
        } else {
            None
        }
    }

    #[test]
    fn mark_stays_on_its_surface() {
        let mut m = Marks::default();
        // In the middle of the ledge: a full square, lifted off the floor.
        assert!(m.add(&ledge, [0.0; 3], [0.0, 0.0, 1.0], 4.0, 0.0, 3, 1, 0));
        let a = m.list[0];
        assert_eq!(a.count, 4);
        assert!(a.points[..4].iter().all(|p| (p[2] - OFFSET).abs() < 1e-4));
        // Near the edge (x = 8, radius 4): cut back to within the ledge (search to 1 / 8 of the radius).
        assert!(m.add(&ledge, [8.0, 0.0, 0.0], [0.0, 0.0, 1.0], 4.0, 0.0, 3, 1, 0));
        let b = m.list[1];
        let max_x = b.points[..usize::from(b.count)]
            .iter()
            .map(|p| p[0])
            .fold(f32::MIN, f32::max);
        assert!(max_x <= 10.0 && max_x > 9.0, "{max_x}");
        // Off the surface: nothing.
        assert!(!m.add(&ledge, [20.0, 0.0, 0.0], [0.0, 0.0, 1.0], 4.0, 0.0, 3, 1, 0));
        let (mut out, mut runs) = (Vec::new(), Vec::new());
        m.quads(100, &mut out, &mut runs);
        assert_eq!(out.len(), 8);
        assert_eq!(runs, [(3, 1, 0, 8)]);
        // Fading at the end, gone after its life.
        m.quads(MARK_LIFE_MS - 500, &mut out, &mut runs);
        assert!((out[0].rgba & 0xff) < 200);
        m.quads(MARK_LIFE_MS, &mut out, &mut runs);
        assert!(out.is_empty() && m.list.is_empty());
    }

    #[test]
    fn pool_is_capped() {
        let mut m = Marks::default();
        for i in 0..(MAX_MARKS as u32 + 3) {
            m.add(&ledge, [0.0; 3], [0.0, 0.0, 1.0], 2.0, 0.0, 0, 0, i);
        }
        assert_eq!(m.list.len(), MAX_MARKS);
        assert_eq!(m.list[0].born_ms, 3);
    }
}

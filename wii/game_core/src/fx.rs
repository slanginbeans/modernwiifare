//! Effects (`wpk::fx`, MW2's effect definitions): particles spawned by IW4's numbers and drawn as sprites.
//!
//! An effect played at a point with a direction spawns each one-shot element's particles (count, delay and life
//! drawn from IW4's ranges) and starts its runner elements' child effects. A particle moves by its element's
//! velocity graphs (local ones turned with the effect's axes, world ones as they are; IW4L
//! `fx_iw4::integrate_velocity_graph_sampled`) and gravity, and its size, colour and alpha follow the visual-state
//! samples through IW4L's `fx_iw4::evaluate_*`. Random numbers are drawn once per particle (IW4L hashes per sample,
//! too dear on the Wii). Looping elements (lingering smoke, fire) aren't played yet.

use alloc::vec::Vec;

use fx_iw4::FxElemVec3Range;
use wpk::fx::{Effect, Elem, kind};
use wpk::records::NONE;

use crate::gfx::SpriteVert;

/// Most particles alive at once (the oldest make way).
pub const MAX_PARTICLES: usize = 384;
/// Most particles one element spawns at once.
const MAX_PER_ELEM: i32 = 32;
/// Runner effects nest at most this deep.
const MAX_DEPTH: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Particle {
    effect: u16,
    elem: u8,
    /// When it appears (after its delay) and how long it lives.
    born_ms: u32,
    life_ms: u32,
    origin: [f32; 3],
    /// The effect's axes (forward, right, up).
    axis: [[f32; 3]; 3],
    /// Random numbers: size, colour, rotation, velocity x / y / z, atlas, gravity.
    r: [f32; 8],
}

/// A runner's child effect waiting for its delay.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    effect: u16,
    at_ms: u32,
    origin: [f32; 3],
    axis: [[f32; 3]; 3],
    depth: u8,
}

/// The effects playing.
#[derive(Clone, Debug, Default)]
pub struct Fx {
    particles: Vec<Particle>,
    pending: Vec<Pending>,
    rng: u32,
}

/// A run of sprites to draw with one texture and blend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    pub texture: u16,
    pub blend: u8,
    /// Vertices `first..first + count` of the sprite list.
    pub first: usize,
    pub count: usize,
}

fn axes(forward: [f32; 3]) -> [[f32; 3]; 3] {
    fx_iw4::vector_vectors(forward)
}

fn add(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

fn ranges(v: &[wpk::fx::Vec3Range], out: &mut [FxElemVec3Range; 16]) -> usize {
    let n = v.len().min(16);
    for (o, s) in out.iter_mut().zip(&v[..n]) {
        *o = FxElemVec3Range {
            base: s.base,
            amplitude: s.amplitude,
        };
    }
    n
}

impl Fx {
    pub fn new(seed: u32) -> Fx {
        Fx {
            rng: seed | 1,
            ..Fx::default()
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn rand16(&mut self) -> u16 {
        (self.rand() * 65535.0) as u16
    }

    pub fn particles(&self) -> usize {
        self.particles.len()
    }

    /// Clear everything (a new map, a respawn of the view).
    pub fn clear(&mut self) {
        self.particles.clear();
        self.pending.clear();
    }

    /// Play `effect` at `origin` pointing along `forward` from `now`.
    pub fn play(
        &mut self,
        defs: &[Effect],
        effect: u16,
        origin: [f32; 3],
        forward: [f32; 3],
        now: u32,
    ) {
        self.spawn(defs, effect, origin, axes(forward), now, 0);
    }

    fn spawn(
        &mut self,
        defs: &[Effect],
        effect: u16,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        now: u32,
        depth: u8,
    ) {
        let Some(e) = defs.get(usize::from(effect)) else {
            return;
        };
        for (k, el) in e.elems.iter().enumerate() {
            if el.looping {
                continue;
            }
            let r = self.rand16();
            let count = fx_iw4::sample_oneshot_spawn_count(el.spawn[0], el.spawn[1], r)
                .clamp(0, MAX_PER_ELEM);
            for _ in 0..count {
                let r = self.rand16();
                let delay =
                    fx_iw4::sample_life_span_msec(el.delay_ms[0], el.delay_ms[1], r).max(0) as u32;
                let o = self.spawn_point(el, origin, &axis);
                if el.kind == kind::RUNNER {
                    if depth < MAX_DEPTH && el.texture != NONE {
                        self.pending.push(Pending {
                            effect: el.texture,
                            at_ms: now.wrapping_add(delay),
                            origin: o,
                            axis,
                            depth: depth + 1,
                        });
                    }
                    continue;
                }
                let r = self.rand16();
                let life =
                    fx_iw4::sample_life_span_msec(el.life_ms[0], el.life_ms[1], r).max(1) as u32;
                let rs: [f32; 8] = core::array::from_fn(|_| self.rand());
                if self.particles.len() >= MAX_PARTICLES {
                    self.particles.remove(0);
                }
                self.particles.push(Particle {
                    effect,
                    elem: k as u8,
                    born_ms: now.wrapping_add(delay),
                    life_ms: life,
                    origin: o,
                    axis,
                    r: rs,
                });
            }
        }
    }

    /// Where a particle of `el` starts: the effect origin, the element's origin offset in the effect's axes, then its
    /// sphere or cylinder offset (IW4L `fx_iw4::offset_spawn_origin`, with this system's random numbers).
    fn spawn_point(&mut self, el: &Elem, origin: [f32; 3], axis: &[[f32; 3]; 3]) -> [f32; 3] {
        let mut o = origin;
        for (k, a) in axis.iter().enumerate() {
            let v = el.origin[k][0] + el.origin[k][1] * self.rand();
            o = add(o, *a, v);
        }
        let radius = el.offset_radius[0] + el.offset_radius[1] * self.rand();
        match fx_iw4::elem_spawn_offset_mode(el.flags) {
            Some(fx_iw4::FxSpawnOffsetMode::Sphere) => {
                let (u, w) = (
                    self.rand() * 2.0 - 1.0,
                    self.rand() * core::f32::consts::TAU,
                );
                let s = libm::sqrtf((1.0 - u * u).max(0.0));
                o = add(o, [s * libm::cosf(w), s * libm::sinf(w), u], radius);
            }
            Some(fx_iw4::FxSpawnOffsetMode::Cylinder) => {
                let yaw = self.rand() * core::f32::consts::TAU;
                o = add(o, axis[1], radius * libm::cosf(yaw));
                o = add(o, axis[2], radius * libm::sinf(yaw));
                let h = el.offset_height[0] + el.offset_height[1] * self.rand();
                o = add(o, axis[0], h);
            }
            _ => {}
        }
        o
    }

    /// Start the runners whose delay is up and drop the particles that have lived out.
    pub fn update(&mut self, defs: &[Effect], now: u32) {
        let mut i = 0;
        while i < self.pending.len() {
            let p = self.pending[i];
            if (now.wrapping_sub(p.at_ms) as i32) >= 0 {
                self.pending.swap_remove(i);
                self.spawn(defs, p.effect, p.origin, p.axis, p.at_ms, p.depth);
            } else {
                i += 1;
            }
        }
        self.particles
            .retain(|p| (now.wrapping_sub(p.born_ms) as i32) < p.life_ms as i32);
    }

    /// The sprites at `now`, facing a camera whose right and up are `right` / `up` (in the particles' space), into
    /// `out` (4 vertices each), grouped into `batches` by texture and blend.
    #[allow(clippy::too_many_arguments)]
    pub fn sprites(
        &self,
        defs: &[Effect],
        now: u32,
        right: [f32; 3],
        up: [f32; 3],
        eye: [f32; 3],
        order: &mut Vec<(u8, u16, u16)>,
        out: &mut Vec<SpriteVert>,
        batches: &mut Vec<Batch>,
    ) {
        out.clear();
        batches.clear();
        order.clear();
        for (i, p) in self.particles.iter().enumerate() {
            let Some(el) = defs
                .get(usize::from(p.effect))
                .and_then(|e| e.elems.get(usize::from(p.elem)))
            else {
                continue;
            };
            if (now.wrapping_sub(p.born_ms) as i32) < 0 {
                continue;
            }
            // Darkening (multiply) first, then see-through, then additive on top.
            let pass = match el.blend {
                4 => 0,
                3 => 2,
                _ => 1,
            };
            order.push((pass, el.texture, i as u16));
        }
        order.sort_unstable();
        for &(_, texture, i) in order.iter() {
            let p = &self.particles[usize::from(i)];
            let el = &defs[usize::from(p.effect)].elems[usize::from(p.elem)];
            let first = out.len();
            if !quad(el, p, now, right, up, eye, out) {
                continue;
            }
            match batches.last_mut() {
                Some(b) if b.texture == texture && b.blend == el.blend => {
                    b.count += out.len() - first
                }
                _ => batches.push(Batch {
                    texture,
                    blend: el.blend,
                    first,
                    count: out.len() - first,
                }),
            }
        }
    }
}

/// IW4's sprite atlas cell for particle `p` (IW4L `fx_iw4::sprite_atlas_uv`, with the particle's random start).
fn atlas_uv(atlas: &[u8; 8], r: f32, age_ms: u32, norm: f32) -> [f32; 4] {
    let count = i32::from(i16::from_le_bytes([atlas[6], atlas[7]]));
    if count <= 1 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let behaviour = atlas[0];
    let mut index = match behaviour & 3 {
        0 => i32::from(atlas[1]),
        1 => (r * count as f32) as i32,
        _ => 0,
    };
    if behaviour & 4 != 0 {
        index += (count as f32 * norm) as i32;
    } else if atlas[2] != 0 {
        index += (age_ms * u32::from(atlas[2]) / 1000) as i32;
    }
    if behaviour & 8 != 0 && index >= i32::from(atlas[3]) * count {
        index = count - 1;
    }
    let cell = fx_iw4::sprite_atlas_cell(
        (index as u32) & (count as u32 - 1),
        atlas[4],
        atlas[5],
        count as u16,
    );
    [cell.s0, cell.t0, cell.s0 + cell.ds, cell.t0 + cell.dt]
}

/// Particle `p`'s quad at `now`; false when it has nothing to show.
fn quad(
    el: &Elem,
    p: &Particle,
    now: u32,
    right: [f32; 3],
    up: [f32; 3],
    eye: [f32; 3],
    out: &mut Vec<SpriteVert>,
) -> bool {
    let age = now.wrapping_sub(p.born_ms);
    let life = p.life_ms as f32;
    let norm = fx_iw4::elem_norm_time(age as i32, p.life_ms as i32);
    let n = el.vis_intervals;
    let (Some(size0), Some(colour)) = (
        fx_iw4::evaluate_size0(el.vis, n, norm, p.r[0]),
        fx_iw4::evaluate_color_bgra(el.vis, n, norm, p.r[1]),
    ) else {
        return false;
    };
    let size1 = fx_iw4::evaluate_size1(el.vis, n, norm, p.r[0]).unwrap_or(size0);
    if colour[3] == 0 || size0 <= 0.0 {
        return false;
    }
    // Where: the velocity graphs integrated over its age, and gravity.
    let mut g = [FxElemVec3Range::default(); 16];
    let rv = [p.r[3], p.r[4], p.r[5]];
    let mut pos = p.origin;
    let k = ranges(&el.vel_local, &mut g);
    if k >= 2 {
        let d = fx_iw4::integrate_velocity_graph_sampled(&g[..k], 0.0, norm, life, rv);
        for (a, &dk) in p.axis.iter().zip(&d) {
            pos = add(pos, *a, dk);
        }
    }
    let k = ranges(&el.vel_world, &mut g);
    if k >= 2 {
        let d = fx_iw4::integrate_velocity_graph_sampled(&g[..k], 0.0, norm, life, rv);
        pos = add(pos, d, 1.0);
    }
    let t = age as f32 / 1000.0;
    let gravity = fx_iw4::elem_gravity_accel_z(el.gravity[0] + el.gravity[1] * p.r[7]);
    pos[2] -= 0.5 * gravity * t * t;
    // Its rotation (IW4: initial rotation + the integrated rotation delta; here the delta's base and amplitude at
    // the start, as a rate).
    let rot = {
        let base = f32::from_le_bytes(el.vis[4..8].try_into().unwrap_or([0; 4]));
        let amp = f32::from_le_bytes(el.vis[0x18 + 4..0x18 + 8].try_into().unwrap_or([0; 4]));
        el.initial_rotation[0]
            + el.initial_rotation[1] * p.r[2]
            + (base + amp * p.r[2]) * age as f32
    };
    let (s, c) = (libm::sinf(rot), libm::cosf(rot));
    // The quad's axes: facing the camera (billboard, cloud), across the effect's axes (oriented), or along the
    // effect's forward and facing the camera (tail).
    let (ax, ay) = match el.kind {
        kind::ORIENTED => (p.axis[1], p.axis[2]),
        kind::TAIL => {
            let f = p.axis[0];
            let to_eye = [eye[0] - pos[0], eye[1] - pos[1], eye[2] - pos[2]];
            let side = [
                f[1] * to_eye[2] - f[2] * to_eye[1],
                f[2] * to_eye[0] - f[0] * to_eye[2],
                f[0] * to_eye[1] - f[1] * to_eye[0],
            ];
            let l = libm::sqrtf(side.iter().map(|v| v * v).sum::<f32>()).max(1e-6);
            (side.map(|v| v / l), f)
        }
        _ => (right, up),
    };
    let (hx, hy) = if el.kind == kind::TAIL {
        (size0, size1 * 0.5)
    } else {
        (size0, size0)
    };
    // Rotated half extents (tails don't spin).
    let (s, c) = if el.kind == kind::TAIL {
        (0.0, 1.0)
    } else {
        (s, c)
    };
    let u: [f32; 3] = core::array::from_fn(|i| (ax[i] * c + ay[i] * s) * hx);
    let v: [f32; 3] = core::array::from_fn(|i| (ay[i] * c - ax[i] * s) * hy);
    let centre = if el.kind == kind::TAIL {
        add(pos, p.axis[0], -size1 * 0.5)
    } else {
        pos
    };
    let rgba = u32::from_be_bytes([colour[2], colour[1], colour[0], colour[3]]);
    let [s0, t0, s1, t1] = atlas_uv(&el.atlas, p.r[6], age, norm);
    for (du, dv, uv) in [
        (-1.0, 1.0, [s0, t0]),
        (1.0, 1.0, [s1, t0]),
        (1.0, -1.0, [s1, t1]),
        (-1.0, -1.0, [s0, t1]),
    ] {
        out.push(SpriteVert {
            pos: core::array::from_fn(|i| centre[i] + u[i] * du + v[i] * dv),
            rgba,
            uv,
        });
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::fx::Vec3Range;

    /// Visual samples: two fenceposts, size 4 then 8, white then faded out.
    fn vis() -> Vec<u8> {
        let mut v = vec![0u8; 2 * wpk::fx::VIS_SAMPLE];
        for (k, (size, alpha)) in [(4.0f32, 255u8), (8.0, 0)].iter().enumerate() {
            let s = &mut v[k * wpk::fx::VIS_SAMPLE..];
            s[0..4].copy_from_slice(&[255, 255, 255, *alpha]);
            s[0x0c..0x10].copy_from_slice(&size.to_le_bytes());
            // Amplitude half the same, so the random number doesn't matter.
            s[0x18..0x1c].copy_from_slice(&[255, 255, 255, *alpha]);
            s[0x18 + 0x0c..0x18 + 0x10].copy_from_slice(&0f32.to_le_bytes());
        }
        v
    }

    fn defs(vis: &[u8]) -> Vec<Effect<'_>> {
        vec![
            // 0: three sprites living 100 ms, moving 1 unit/ms along the effect's forward, and a runner of effect 1
            // after 50 ms.
            Effect {
                elems: vec![
                    Elem {
                        kind: kind::BILLBOARD,
                        flags: 0x0100_0000,
                        texture: 7,
                        blend: 3,
                        spawn: [3, 0],
                        life_ms: [100, 0],
                        vis_intervals: 1,
                        vel_local: vec![
                            Vec3Range {
                                base: [1.0, 0.0, 0.0],
                                amplitude: [0.0; 3],
                            };
                            2
                        ],
                        vis,
                        ..Elem::default()
                    },
                    Elem {
                        kind: kind::RUNNER,
                        texture: 1,
                        spawn: [1, 0],
                        delay_ms: [50, 0],
                        vis: &vis[..wpk::fx::VIS_SAMPLE],
                        ..Elem::default()
                    },
                ],
            },
            // 1: one sprite.
            Effect {
                elems: vec![Elem {
                    kind: kind::BILLBOARD,
                    texture: 2,
                    blend: 2,
                    spawn: [1, 0],
                    life_ms: [200, 0],
                    vis_intervals: 1,
                    vis,
                    ..Elem::default()
                }],
            },
        ]
    }

    #[test]
    fn plays_moves_fades_and_runs_children() {
        let v = vis();
        let d = defs(&v);
        let mut fx = Fx::new(7);
        fx.play(&d, 0, [0.0; 3], [0.0, 1.0, 0.0], 1000);
        assert_eq!(fx.particles(), 3);
        let (mut order, mut out, mut batches) = (Vec::new(), Vec::new(), Vec::new());
        let (right, up) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        // Halfway through its life: 50 units along +y, size 6, half faded.
        fx.sprites(
            &d,
            1050,
            right,
            up,
            [0.0; 3],
            &mut order,
            &mut out,
            &mut batches,
        );
        assert_eq!(out.len(), 12);
        let centre: [f32; 3] =
            core::array::from_fn(|i| out[..4].iter().map(|v| v.pos[i]).sum::<f32>() / 4.0);
        assert!((centre[1] - 50.0).abs() < 0.5, "{centre:?}");
        assert!(
            (out[1].pos[0] - out[0].pos[0] - 12.0).abs() < 0.1,
            "{:?}",
            out[..2].to_vec()
        );
        assert!(((out[0].rgba & 0xff) as i32 - 128).abs() <= 1);
        assert_eq!(
            batches,
            [Batch {
                texture: 7,
                blend: 3,
                first: 0,
                count: 12
            }]
        );
        // The runner starts effect 1 at 1050; at 1101 the first three are gone.
        fx.update(&d, 1050);
        assert_eq!(fx.particles(), 4);
        fx.update(&d, 1101);
        assert_eq!(fx.particles(), 1);
        fx.update(&d, 1300);
        assert_eq!(fx.particles(), 0);
    }

    #[test]
    fn caps_particles() {
        let v = vis();
        let d = defs(&v);
        let mut fx = Fx::new(1);
        for i in 0..400 {
            fx.play(&d, 1, [0.0; 3], [1.0, 0.0, 0.0], i);
        }
        assert_eq!(fx.particles(), MAX_PARTICLES);
    }
}

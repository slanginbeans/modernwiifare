//! The light grid for moving models (`LGRD` in a map pack): IW4's light grid sampled at convert time on a coarse
//! regular lattice (IW4L `lighting_iw4` atapoint at each lattice point, the colour a white model gets there, in
//! gamma space), looked up on the Wii with a trilinear blend of the eight points around a model's lighting origin.
//! Points the grid has no light for are marked unlit and left out of the blend.
//!
//! Layout: `f32 origin[3]`, `f32 step[3]`, `u16 dims[3]`, `u16 0`, then `dims[0] * dims[1] * dims[2]` × RGBA8 (x fastest,
//! then y, then z; RGB at half the light, as static models' tints, so up to ×2; alpha 0: unlit).

use alloc::vec::Vec;

use crate::be::{self, Push};

pub const HEADER: usize = 32;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightLattice<'a> {
    pub origin: [f32; 3],
    pub step: [f32; 3],
    pub dims: [u16; 3],
    /// RGBA8 per point.
    pub points: &'a [u8],
}

pub fn encode(origin: [f32; 3], step: [f32; 3], dims: [u16; 3], points: &[[u8; 4]]) -> Vec<u8> {
    let mut b = Vec::with_capacity(HEADER + points.len() * 4);
    origin.iter().chain(&step).for_each(|&v| b.f32(v));
    dims.iter().for_each(|&d| b.u16(d));
    b.u16(0);
    for p in points {
        b.extend_from_slice(p);
    }
    b
}

impl<'a> LightLattice<'a> {
    pub fn decode(b: &'a [u8]) -> Option<LightLattice<'a>> {
        let h = b.get(..HEADER)?;
        let dims = [be::u16(h, 24), be::u16(h, 26), be::u16(h, 28)];
        let n = dims.iter().map(|&d| usize::from(d)).product::<usize>();
        Some(LightLattice {
            origin: core::array::from_fn(|k| be::f32(h, k * 4)),
            step: core::array::from_fn(|k| be::f32(h, 12 + k * 4).max(1.0)),
            dims,
            points: b.get(HEADER..HEADER + n * 4)?,
        })
    }

    fn point(&self, x: usize, y: usize, z: usize) -> [u8; 4] {
        let [dx, dy, _] = self.dims.map(usize::from);
        let i = (z * dy + y) * dx + x;
        self.points
            .get(i * 4..i * 4 + 4)
            .map_or([0; 4], |p| [p[0], p[1], p[2], p[3]])
    }

    /// The light (RGB 0..2, gamma space, 1 = as authored) at `at`: the lit lattice points around it blended trilinearly, clamped to
    /// the lattice's edges; `None` when none of the eight is lit.
    pub fn sample(&self, at: [f32; 3]) -> Option<[f32; 3]> {
        if self.dims.contains(&0) {
            return None;
        }
        let mut cell = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let max = f32::from(self.dims[k] - 1);
            let t = ((at[k] - self.origin[k]) / self.step[k]).clamp(0.0, max);
            let c = (t as usize).min(usize::from(self.dims[k]).saturating_sub(2));
            cell[k] = c;
            frac[k] = if self.dims[k] > 1 { t - c as f32 } else { 0.0 };
        }
        let (mut sum, mut wsum) = ([0.0f32; 3], 0.0f32);
        for corner in 0..8usize {
            let pick = |k: usize| (corner >> k) & 1;
            let idx: [usize; 3] =
                core::array::from_fn(|k| (cell[k] + pick(k)).min(usize::from(self.dims[k]) - 1));
            let p = self.point(idx[0], idx[1], idx[2]);
            if p[3] == 0 {
                continue;
            }
            let w: f32 = (0..3)
                .map(|k| if pick(k) == 1 { frac[k] } else { 1.0 - frac[k] })
                .product();
            for c in 0..3 {
                sum[c] += w * f32::from(p[c]) * (2.0 / 255.0);
            }
            wsum += w;
        }
        (wsum > 1e-6).then(|| sum.map(|v| v / wsum))
    }
}

/// A light colour (gamma space, 1 = as authored) as the konst colour the shim's tint stage multiplies by two
/// (`DrawState::tint`: RGBA8, half the light; never 0, which means untinted).
pub fn tint(light: [f32; 3]) -> u32 {
    let c = |v: f32| ((v * 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (c(light[0]) << 24 | c(light[1]) << 16 | c(light[2]) << 8 | 0xff).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_blend_lit_points_and_skip_unlit() {
        // 2 x 1 x 1: dark on the left, bright on the right.
        let b = encode(
            [0.0; 3],
            [100.0; 3],
            [2, 1, 1],
            &[[0, 0, 0, 255], [255, 128, 0, 255]],
        );
        let l = LightLattice::decode(&b).unwrap();
        let mid = l.sample([50.0, 0.0, 0.0]).unwrap();
        assert!(
            (mid[0] - 1.0).abs() < 0.01 && (mid[1] - 0.502).abs() < 0.01,
            "{mid:?}"
        );
        // Clamped past the edge.
        assert!((l.sample([500.0, 0.0, 0.0]).unwrap()[0] - 2.0).abs() < 1e-3);
        // The unlit point drops out of the blend.
        let b = encode(
            [0.0; 3],
            [100.0; 3],
            [2, 1, 1],
            &[[0, 0, 0, 0], [200, 200, 200, 255]],
        );
        let l = LightLattice::decode(&b).unwrap();
        assert!((l.sample([10.0, 0.0, 0.0]).unwrap()[0] - 400.0 / 255.0).abs() < 1e-3);
        let b = encode([0.0; 3], [100.0; 3], [1, 1, 1], &[[0, 0, 0, 0]]);
        assert_eq!(LightLattice::decode(&b).unwrap().sample([0.0; 3]), None);
        assert_eq!(tint([1.0; 3]), 0x8080_80ff);
        assert_eq!(tint([0.0; 3]), 0xff);
    }
}

//! CMPR: the GameCube/Wii variant of DXT1.
//!
//! Differences from PC DXT1:
//! - 4x4 sub-blocks are grouped into 8x8 tiles in the order top-left, top-right, bottom-left,
//!   bottom-right.
//! - Endpoints are big-endian RGB565.
//! - In each index byte the *leftmost* pixel is in the two most significant bits.
//! - The two interpolated colours blend 5/8 + 3/8 rather than 2/3 + 1/3. This follows Dolphin's
//!   decoder; [`BLEND_NEAR`] is a single constant so it can be changed if hardware disagrees.
//!
//! Mode: `c0 > c1` gives four opaque colours; `c0 <= c1` gives three colours plus transparent (index 3).

/// Weight (out of 8) of the nearer endpoint in an interpolated colour.
pub const BLEND_NEAR: u32 = 5;

pub fn expand565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1f) as u8;
    let g = ((c >> 5) & 0x3f) as u8;
    let b = (c & 0x1f) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

pub fn pack565(rgb: [f32; 3]) -> u16 {
    let q = |v: f32, max: f32| ((v.clamp(0.0, 255.0) * max / 255.0) + 0.5) as u16;
    (q(rgb[0], 31.0) << 11) | (q(rgb[1], 63.0) << 5) | q(rgb[2], 31.0)
}

fn blend(near: u8, far: u8) -> u8 {
    ((near as u32 * BLEND_NEAR + far as u32 * (8 - BLEND_NEAR)) / 8) as u8
}

/// The four palette entries a block decodes to (RGBA).
pub fn palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
    let a = expand565(c0);
    let b = expand565(c1);
    let p0 = [a[0], a[1], a[2], 255];
    let p1 = [b[0], b[1], b[2], 255];
    if c0 > c1 {
        let p2 = [blend(a[0], b[0]), blend(a[1], b[1]), blend(a[2], b[2]), 255];
        let p3 = [blend(b[0], a[0]), blend(b[1], a[1]), blend(b[2], a[2]), 255];
        [p0, p1, p2, p3]
    } else {
        let mid = |i: usize| ((a[i] as u32 + b[i] as u32) / 2) as u8;
        [p0, p1, [mid(0), mid(1), mid(2), 255], [0, 0, 0, 0]]
    }
}

/// Fraction of `c0` in each palette entry (four-colour mode), for the least-squares refit.
const WEIGHT_C0: [f32; 4] = [
    1.0,
    0.0,
    BLEND_NEAR as f32 / 8.0,
    (8 - BLEND_NEAR) as f32 / 8.0,
];

fn dist2(a: [u8; 4], b: [f32; 3]) -> f32 {
    let d = [a[0] as f32 - b[0], a[1] as f32 - b[1], a[2] as f32 - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

fn rgb(p: [u8; 4]) -> [f32; 3] {
    [p[0] as f32, p[1] as f32, p[2] as f32]
}

/// Indices + total squared error for a block against the palette of (c0, c1).
fn assign(px: &[[u8; 4]; 16], transparent: &[bool; 16], c0: u16, c1: u16) -> ([u8; 16], f32) {
    let pal = palette(c0, c1);
    let usable = if c0 > c1 { 4 } else { 3 };
    let mut idx = [0u8; 16];
    let mut err = 0.0;
    for i in 0..16 {
        if transparent[i] {
            idx[i] = 3;
            continue;
        }
        let mut best = (f32::MAX, 0u8);
        for (k, entry) in pal.iter().take(usable).enumerate() {
            let d = dist2(*entry, rgb(px[i]));
            if d < best.0 {
                best = (d, k as u8);
            }
        }
        idx[i] = best.1;
        err += best.0;
    }
    (idx, err)
}

fn order(c0: u16, c1: u16, three_colour: bool) -> (u16, u16) {
    // Four-colour mode needs c0 > c1, three-colour (transparency) needs c0 <= c1.
    if three_colour == (c0 > c1) {
        (c1, c0)
    } else {
        (c0, c1)
    }
}

/// Endpoints along the principal axis of the opaque pixels.
fn principal_endpoints(opaque: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let n = opaque.len() as f32;
    let mut mean = [0.0f32; 3];
    for p in opaque {
        for c in 0..3 {
            mean[c] += p[c] / n;
        }
    }
    let mut cov = [[0.0f32; 3]; 3];
    for p in opaque {
        let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    // Power iteration for the dominant eigenvector. Start from the covariance column of the
    // highest-variance channel: a fixed start like (1,1,1) can be orthogonal to the answer
    // (e.g. red vs blue) and collapse to zero.
    let k = (0..3)
        .max_by(|&a, &b| cov[a][a].total_cmp(&cov[b][b]))
        .unwrap_or(0);
    let mut axis = [cov[0][k], cov[1][k], cov[2][k]];
    for _ in 0..8 {
        let v = [
            cov[0][0] * axis[0] + cov[0][1] * axis[1] + cov[0][2] * axis[2],
            cov[1][0] * axis[0] + cov[1][1] * axis[1] + cov[1][2] * axis[2],
            cov[2][0] * axis[0] + cov[2][1] * axis[1] + cov[2][2] * axis[2],
        ];
        let len = libm::sqrtf(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        if len < 1e-6 {
            return (mean, mean);
        }
        axis = [v[0] / len, v[1] / len, v[2] / len];
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in opaque {
        let t =
            (p[0] - mean[0]) * axis[0] + (p[1] - mean[1]) * axis[1] + (p[2] - mean[2]) * axis[2];
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let at = |t: f32| {
        [
            mean[0] + axis[0] * t,
            mean[1] + axis[1] * t,
            mean[2] + axis[2] * t,
        ]
    };
    (at(hi), at(lo))
}

/// Least-squares endpoints for fixed four-colour-mode indices.
fn refit(px: &[[u8; 4]; 16], idx: &[u8; 16]) -> Option<([f32; 3], [f32; 3])> {
    let (mut aa, mut ab, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    let mut ap = [0.0f32; 3];
    let mut bp = [0.0f32; 3];
    for i in 0..16 {
        let a = WEIGHT_C0[idx[i] as usize];
        let b = 1.0 - a;
        aa += a * a;
        ab += a * b;
        bb += b * b;
        let p = rgb(px[i]);
        for c in 0..3 {
            ap[c] += a * p[c];
            bp[c] += b * p[c];
        }
    }
    let det = aa * bb - ab * ab;
    if det.abs() < 1e-6 {
        return None;
    }
    let mut e0 = [0.0; 3];
    let mut e1 = [0.0; 3];
    for c in 0..3 {
        e0[c] = (ap[c] * bb - bp[c] * ab) / det;
        e1[c] = (bp[c] * aa - ap[c] * ab) / det;
    }
    Some((e0, e1))
}

/// Encode one 4x4 sub-block (row-major pixels) into 8 bytes.
pub fn encode_block(px: &[[u8; 4]; 16]) -> [u8; 8] {
    let mut transparent = [false; 16];
    let mut opaque = [[0.0f32; 3]; 16];
    let mut n_opaque = 0;
    for i in 0..16 {
        transparent[i] = px[i][3] < 128;
        if !transparent[i] {
            opaque[n_opaque] = rgb(px[i]);
            n_opaque += 1;
        }
    }
    let three_colour = n_opaque < 16;
    if n_opaque == 0 {
        // c0 == c1 == 0 selects three-colour mode; every index 3 = transparent.
        return [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
    }

    let (e0, e1) = principal_endpoints(&opaque[..n_opaque]);
    let (mut c0, mut c1) = order(pack565(e0), pack565(e1), three_colour);
    let (mut idx, mut err) = assign(px, &transparent, c0, c1);

    if !three_colour
        && c0 != c1
        && let Some((r0, r1)) = refit(px, &idx)
    {
        let (n0, n1) = order(pack565(r0), pack565(r1), false);
        let (nidx, nerr) = assign(px, &transparent, n0, n1);
        if nerr < err {
            (c0, c1, idx, err) = (n0, n1, nidx, nerr);
        }
    }
    let _ = err;

    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&c0.to_be_bytes());
    out[2..4].copy_from_slice(&c1.to_be_bytes());
    for row in 0..4 {
        out[4 + row] = (idx[row * 4] << 6)
            | (idx[row * 4 + 1] << 4)
            | (idx[row * 4 + 2] << 2)
            | idx[row * 4 + 3];
    }
    out
}

/// Decode one 8-byte sub-block into 16 row-major RGBA pixels.
pub fn decode_block(b: &[u8]) -> [[u8; 4]; 16] {
    let c0 = u16::from_be_bytes([b[0], b[1]]);
    let c1 = u16::from_be_bytes([b[2], b[3]]);
    let pal = palette(c0, c1);
    let mut out = [[0u8; 4]; 16];
    for row in 0..4 {
        let bits = b[4 + row];
        for col in 0..4 {
            out[row * 4 + col] = pal[((bits >> (6 - 2 * col)) & 3) as usize];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_block_is_exact_up_to_565() {
        let c = [200u8, 100, 50, 255];
        let enc = encode_block(&[c; 16]);
        let dec = decode_block(&enc);
        let want = expand565(pack565(rgb(c)));
        for p in dec {
            assert_eq!(&p[..3], &want, "{p:?}");
            assert_eq!(p[3], 255);
        }
    }

    #[test]
    fn two_colour_checker_is_exact_and_bit_order_is_leftmost_high() {
        let (a, b) = ([255u8, 0, 0, 255], [0u8, 0, 255, 255]);
        let mut px = [a; 16];
        for (i, p) in px.iter_mut().enumerate() {
            if (i % 4 + i / 4) % 2 == 1 {
                *p = b;
            }
        }
        let enc = encode_block(&px);
        assert_eq!(decode_block(&enc), px);
        // Row 0 is a,b,a,b: indices for the two endpoints alternate, leftmost in the top bits.
        let i_a = enc[4] >> 6;
        let i_b = (enc[4] >> 4) & 3;
        assert_ne!(i_a, i_b);
        assert_eq!(enc[4], (i_a << 6) | (i_b << 4) | (i_a << 2) | i_b);
    }

    #[test]
    fn transparency_uses_three_colour_mode() {
        let mut px = [[10u8, 200, 30, 255]; 16];
        px[0] = [0, 0, 0, 0];
        px[15] = [255, 255, 255, 10];
        let enc = encode_block(&px);
        let c0 = u16::from_be_bytes([enc[0], enc[1]]);
        let c1 = u16::from_be_bytes([enc[2], enc[3]]);
        assert!(c0 <= c1, "three-colour mode needs c0 <= c1");
        let dec = decode_block(&enc);
        assert_eq!(dec[0][3], 0);
        assert_eq!(dec[15][3], 0);
        assert!(dec[1..15].iter().all(|p| p[3] == 255));
    }

    #[test]
    fn fully_transparent_block() {
        let dec = decode_block(&encode_block(&[[50, 60, 70, 0]; 16]));
        assert!(dec.iter().all(|p| p[3] == 0));
    }

    #[test]
    fn gradient_error_is_small() {
        // A gentle gradient, as in real textures (a 4x4 block only has four colours).
        let mut px = [[0u8; 4]; 16];
        for (i, p) in px.iter_mut().enumerate() {
            let v = 100 + (i * 3) as u8;
            *p = [v, 255 - v, v / 2, 255];
        }
        let dec = decode_block(&encode_block(&px));
        let mse: f32 = px
            .iter()
            .zip(&dec)
            .map(|(a, b)| dist2(*b, rgb(*a)) / 3.0)
            .sum::<f32>()
            / 16.0;
        let psnr = 10.0 * libm::log10f(255.0 * 255.0 / mse);
        assert!(psnr > 30.0, "psnr {psnr}");
    }
}

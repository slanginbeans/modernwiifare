//! 4-bit DSP ADPCM sound clips (the GameCube / Wii format): 14 samples per 8-byte frame, 3.5× smaller than 16-bit
//! PCM. `SNDD` holds them when its section `count` is [`SNDD_ADPCM`] (`--adpcm`).
//!
//! A clip is a 32-byte header, then its frames, padded to a multiple of 32 bytes:
//!
//! - header: [`COEF_SETS`] predictor pairs (i16, 11 fractional bits), then the sample count (u32), big-endian.
//! - frame: one byte `predictor << 4 | scale exponent`, then 14 signed nibbles (high nibble first). A sample is
//!   `(nibble · 2^scale · 2048 + 1024 + c1 · h1 + c2 · h2) >> 11`, clamped to i16, `h1` / `h2` the two before it.
//!
//! libogc's ASND mixer only plays PCM, so the Wii decodes on the CPU a few kilobytes at a time while a clip plays
//! ([`decode`], called from the shim's voice callback). The encoder ([`encode`]) fits the predictor pairs to the clip
//! and picks, per frame, the pair and scale with the smallest error after quantisation. It is written for this
//! project (no outside encoder's code).

use alloc::vec::Vec;

use crate::be;

/// `SNDD`'s section `count` when its clips are ADPCM (0: 16-bit big-endian PCM).
pub const SNDD_ADPCM: u32 = 1;

/// Predictor pairs per clip.
pub const COEF_SETS: usize = 7;
pub const HEADER_LEN: usize = 32;
pub const FRAME_LEN: usize = 8;
pub const FRAME_SAMPLES: usize = 14;
/// Largest scale exponent the encoder uses.
const MAX_SCALE: u8 = 11;
/// Coefficients stay within ±2.0 (Q11), so the predictor sum fits in i32.
const COEF_LIMIT: i32 = 4095;

/// Samples in an ADPCM clip (from its header; 0 when it is too short to have one).
pub fn sample_count(clip: &[u8]) -> u32 {
    if clip.len() < HEADER_LEN {
        return 0;
    }
    let n = be::u32(clip, 28);
    // Never past the frames the clip has.
    let frames = (clip.len() - HEADER_LEN) / FRAME_LEN;
    n.min((frames * FRAME_SAMPLES) as u32)
}

/// Bytes of an ADPCM clip of `samples` samples, padded to 32.
pub fn clip_len(samples: usize) -> usize {
    (HEADER_LEN + samples.div_ceil(FRAME_SAMPLES) * FRAME_LEN).div_ceil(32) * 32
}

fn coefs(clip: &[u8]) -> [[i32; 2]; COEF_SETS] {
    core::array::from_fn(|i| {
        [
            i32::from(be::u16(clip, i * 4) as i16),
            i32::from(be::u16(clip, i * 4 + 2) as i16),
        ]
    })
}

#[inline]
fn predict(nib: i32, scale: i32, c: [i32; 2], h1: i32, h2: i32) -> i16 {
    let v = (nib.wrapping_mul(scale) << 11)
        .wrapping_add(1024)
        .wrapping_add(c[0].wrapping_mul(h1))
        .wrapping_add(c[1].wrapping_mul(h2))
        >> 11;
    v.clamp(-32768, 32767) as i16
}

#[inline]
fn nibble(b: u8, high: bool) -> i32 {
    let n = if high { b >> 4 } else { b & 15 };
    (i32::from(n) << 28) >> 28
}

/// Decode samples `start..` of `clip` into `out` (as many as fit, up to the clip's end), continuing from `hist`
/// (the two samples before `start`: `[h1, h2]`, zero at the start), which it updates. Returns the samples written.
/// Never panics; a damaged clip decodes to noise, not out-of-bounds reads.
pub fn decode(clip: &[u8], start: u32, hist: &mut [i16; 2], out: &mut [i16]) -> usize {
    let total = sample_count(clip) as usize;
    let start = start as usize;
    if start >= total {
        return 0;
    }
    let c = coefs(clip);
    let n = out.len().min(total - start);
    let (mut h1, mut h2) = (i32::from(hist[0]), i32::from(hist[1]));
    let mut i = start;
    let mut written = 0;
    while written < n {
        let frame = HEADER_LEN + (i / FRAME_SAMPLES) * FRAME_LEN;
        let Some(f) = clip.get(frame..frame + FRAME_LEN) else {
            break;
        };
        let pair = c[usize::from(f[0] >> 4) % COEF_SETS];
        let scale = 1i32 << (f[0] & 15).min(MAX_SCALE + 1);
        let mut k = i % FRAME_SAMPLES;
        while k < FRAME_SAMPLES && written < n {
            let s = predict(
                nibble(f[1 + k / 2], k.is_multiple_of(2)),
                scale,
                pair,
                h1,
                h2,
            );
            out[written] = s;
            h2 = h1;
            h1 = i32::from(s);
            written += 1;
            k += 1;
            i += 1;
        }
    }
    *hist = [h1 as i16, h2 as i16];
    written
}

/// Encode a frame of `x` (up to 14 samples) with one pair and scale, from history `h1`, `h2`: (frame, squared
/// error, new history).
fn encode_frame(
    x: &[i16],
    pair: [i32; 2],
    pred: u8,
    exp: u8,
    h1: i32,
    h2: i32,
) -> ([u8; 8], i64, i32, i32) {
    let scale = 1i32 << exp;
    let mut f = [0u8; FRAME_LEN];
    f[0] = (pred << 4) | exp;
    let (mut h1, mut h2) = (h1, h2);
    let mut err = 0i64;
    for (k, &s) in x.iter().enumerate() {
        let p = (pair[0] * h1 + pair[1] * h2 + 1024) >> 11;
        let d = i32::from(s) - p;
        // Round to the nearest nibble.
        let q = if d >= 0 {
            (d + scale / 2) / scale
        } else {
            -((-d + scale / 2) / scale)
        };
        let nib = q.clamp(-8, 7);
        let out = predict(nib, scale, pair, h1, h2);
        let e = i64::from(s) - i64::from(out);
        err += e * e;
        f[1 + k / 2] |= ((nib & 15) as u8) << if k.is_multiple_of(2) { 4 } else { 0 };
        h2 = h1;
        h1 = i32::from(out);
    }
    (f, err, h1, h2)
}

/// Least-squares order-2 predictor over `idx` (sample positions with two before them), Q11, limited.
fn solve(x: &[i16], idx: impl Iterator<Item = usize>) -> Option<[i32; 2]> {
    let (mut r11, mut r12, mut r22, mut r01, mut r02) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for n in idx {
        let (s0, s1, s2) = (f64::from(x[n]), f64::from(x[n - 1]), f64::from(x[n - 2]));
        r11 += s1 * s1;
        r12 += s1 * s2;
        r22 += s2 * s2;
        r01 += s0 * s1;
        r02 += s0 * s2;
    }
    let det = r11 * r22 - r12 * r12;
    let (a1, a2) = if det.abs() > 1e-9 * (r11 * r22).max(1.0) {
        ((r01 * r22 - r02 * r12) / det, (r02 * r11 - r01 * r12) / det)
    } else if r11 > 0.0 {
        (r01 / r11, 0.0)
    } else {
        return None;
    };
    let q = |a: f64| {
        let v = a * 2048.0;
        let r = if v >= 0.0 { v + 0.5 } else { v - 0.5 };
        (r as i32).clamp(-COEF_LIMIT, COEF_LIMIT)
    };
    Some([q(a1), q(a2)])
}

/// Fit [`COEF_SETS`] predictor pairs to the frames of `x`: start from a spread of fixed pairs, then alternate
/// assigning each frame to its best pair and re-fitting each pair to its frames.
fn fit_coefs(x: &[i16]) -> [[i32; 2]; COEF_SETS] {
    // Silence, first-order hold, and smooth curves of rising strength.
    let mut sets: [[i32; 2]; COEF_SETS] = [
        [0, 0],
        [2048, 0],
        [3072, -1024],
        [3584, -1600],
        [3900, -1900],
        [1024, 0],
        [4000, -2000],
    ];
    let frames = x.len().div_ceil(FRAME_SAMPLES);
    let sample = |n: usize| -> i64 { if n < x.len() { i64::from(x[n]) } else { 0 } };
    let hist = |n: usize, back: usize| -> i64 { if n >= back { sample(n - back) } else { 0 } };
    for _ in 0..6 {
        let mut members: [Vec<usize>; COEF_SETS] = Default::default();
        for f in 0..frames {
            let lo = f * FRAME_SAMPLES;
            let hi = (lo + FRAME_SAMPLES).min(x.len());
            let best = (0..COEF_SETS)
                .min_by_key(|&c| {
                    (lo..hi)
                        .map(|n| {
                            let p = (i64::from(sets[c][0]) * hist(n, 1)
                                + i64::from(sets[c][1]) * hist(n, 2))
                                >> 11;
                            let e = sample(n) - p;
                            e * e
                        })
                        .sum::<i64>()
                })
                .unwrap_or(0);
            members[best].push(f);
        }
        for (c, m) in members.iter().enumerate() {
            // Pair 0 stays the zero predictor (silence and noise-like frames).
            if c == 0 || m.is_empty() {
                continue;
            }
            let idx = m.iter().flat_map(|&f| {
                let lo = (f * FRAME_SAMPLES).max(2);
                let hi = ((f + 1) * FRAME_SAMPLES).min(x.len());
                lo..hi
            });
            if let Some(p) = solve(x, idx) {
                sets[c] = p;
            }
        }
    }
    sets
}

/// Encode 16-bit samples into a clip (header, frames, padded to 32 bytes).
pub fn encode(x: &[i16]) -> Vec<u8> {
    let sets = fit_coefs(x);
    let mut out = Vec::with_capacity(clip_len(x.len()));
    for p in sets {
        out.extend_from_slice(&(p[0] as i16).to_be_bytes());
        out.extend_from_slice(&(p[1] as i16).to_be_bytes());
    }
    out.extend_from_slice(&(x.len() as u32).to_be_bytes());
    let (mut h1, mut h2) = (0i32, 0i32);
    for chunk in x.chunks(FRAME_SAMPLES) {
        let mut best: Option<([u8; 8], i64, i32, i32)> = None;
        for (c, &pair) in sets.iter().enumerate() {
            for exp in 0..=MAX_SCALE {
                let r = encode_frame(chunk, pair, c as u8, exp, h1, h2);
                if best.as_ref().is_none_or(|b| r.1 < b.1) {
                    best = Some(r);
                }
            }
        }
        let (f, _, n1, n2) = best.unwrap_or(([0; 8], 0, 0, 0));
        out.extend_from_slice(&f);
        (h1, h2) = (n1, n2);
    }
    out.resize(out.len().div_ceil(32) * 32, 0);
    out
}

/// Big-endian 16-bit PCM bytes to samples.
pub fn samples_be(pcm: &[u8]) -> Vec<i16> {
    pcm.chunks_exact(2)
        .map(|b| i16::from_be_bytes([b[0], b[1]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(clip: &[u8]) -> Vec<i16> {
        let mut out = alloc::vec![0i16; sample_count(clip) as usize];
        let mut h = [0; 2];
        assert_eq!(decode(clip, 0, &mut h, &mut out), out.len());
        out
    }

    /// A tone sweep plus a decaying noise burst: smooth and rough parts.
    fn test_signal(n: usize) -> Vec<i16> {
        let mut seed = 12345u32;
        (0..n)
            .map(|i| {
                let t = i as f64 / 32000.0;
                let tone =
                    9000.0 * libm::sin(2.0 * core::f64::consts::PI * (200.0 + 3000.0 * t) * t);
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = f64::from((seed >> 16) as i16) * 0.3 * libm::exp(-t * 8.0);
                (tone + noise).clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }

    fn snr_db(a: &[i16], b: &[i16]) -> f64 {
        let (mut s, mut e) = (0f64, 0f64);
        for (&x, &y) in a.iter().zip(b) {
            s += f64::from(x) * f64::from(x);
            e += (f64::from(x) - f64::from(y)).powi(2);
        }
        10.0 * libm::log10(s / e.max(1.0))
    }

    #[test]
    fn round_trip_is_close_and_three_and_a_half_times_smaller() {
        let x = test_signal(32000);
        let clip = encode(&x);
        assert_eq!(clip.len(), clip_len(x.len()));
        assert_eq!(clip.len() % 32, 0);
        assert_eq!(sample_count(&clip) as usize, x.len());
        let y = decode_all(&clip);
        let snr = snr_db(&x, &y);
        assert!(snr > 28.0, "snr {snr:.1} dB");
        assert!((x.len() * 2) as f64 / clip.len() as f64 > 3.4);
    }

    #[test]
    fn decoding_in_pieces_matches_one_pass() {
        let x = test_signal(5000);
        let clip = encode(&x);
        let whole = decode_all(&clip);
        // Odd piece sizes, starting mid-frame.
        let mut h = [0; 2];
        let mut at = 0u32;
        let mut pieces = Vec::new();
        for size in [1usize, 13, 14, 15, 333, 2048].iter().cycle() {
            let mut buf = alloc::vec![0i16; *size];
            let n = decode(&clip, at, &mut h, &mut buf);
            if n == 0 {
                break;
            }
            pieces.extend_from_slice(&buf[..n]);
            at += n as u32;
        }
        assert_eq!(pieces, whole);
    }

    #[test]
    fn silence_and_damage_are_safe() {
        let clip = encode(&[0; 100]);
        assert!(decode_all(&clip).iter().all(|&s| s == 0));
        // A clip cut short reports only the frames it has; garbage decodes without panicking.
        let x = test_signal(1000);
        let clip = encode(&x);
        let cut = &clip[..HEADER_LEN + 3 * FRAME_LEN];
        assert_eq!(sample_count(cut), 3 * FRAME_SAMPLES as u32);
        let junk: Vec<u8> = (0..200u32).map(|i| (i * 37 + 11) as u8).collect();
        let mut out = [0i16; 4000];
        let mut h = [0; 2];
        let _ = decode(&junk, 0, &mut h, &mut out);
        assert_eq!(decode(&[1, 2, 3], 0, &mut h, &mut out), 0);
    }

    #[test]
    fn extreme_signals_stay_in_range() {
        // Full-scale square wave and clipping-level steps: the clamp and the coefficient limit keep it sane.
        let x: Vec<i16> = (0..3000)
            .map(|i| if (i / 7) % 2 == 0 { 32767 } else { -32768 })
            .collect();
        let y = decode_all(&encode(&x));
        assert!(snr_db(&x, &y) > 6.0);
    }
}

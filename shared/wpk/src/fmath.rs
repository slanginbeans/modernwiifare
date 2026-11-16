//! Square roots without a square root instruction. Broadway (like the 750CL it comes from) has no `fsqrt`, so
//! `libm::sqrtf` runs a software loop on the Wii; posing a body takes several hundred of them a frame (every
//! quaternion decoded, blended or turned into a matrix). These use the classic reciprocal square root estimate and
//! three Newton steps instead: multiplies and adds only, within a few units in the last place of `1/sqrt`.

/// `1 / sqrt(x)` for `x > 0` (finite); `x <= 0` gives a large finite value, never a NaN.
#[inline]
pub fn rsqrt(x: f32) -> f32 {
    let x = x.max(f32::MIN_POSITIVE);
    let half = 0.5 * x;
    let mut y = f32::from_bits(0x5f37_5a86 - (x.to_bits() >> 1));
    y *= 1.5 - half * y * y;
    y *= 1.5 - half * y * y;
    y *= 1.5 - half * y * y;
    y
}

/// `sqrt(x)` as `x * rsqrt(x)`; 0 for `x <= 0`.
#[inline]
pub fn sqrt(x: f32) -> f32 {
    if x > 0.0 { x * rsqrt(x) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsqrt_is_close_to_libm() {
        let mut worst = 0.0f64;
        let mut x = 1e-6f32;
        while x < 1e7 {
            let want = 1.0 / f64::from(x).sqrt();
            let got = f64::from(rsqrt(x));
            worst = worst.max(((got - want) / want).abs());
            x *= 1.000_37;
        }
        assert!(worst < 4e-7, "relative error {worst}");
    }

    #[test]
    fn sqrt_edges() {
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-1.0), 0.0);
        assert!((sqrt(1.0) - 1.0).abs() < 1e-6);
        assert!((sqrt(4.0) - 2.0).abs() < 1e-6);
        assert!(rsqrt(0.0).is_finite());
        // Quaternion lengths near one, where posing spends them.
        for i in 0..1000 {
            let x = 0.9 + i as f32 * 2e-4;
            assert!((sqrt(x) - libm::sqrtf(x)).abs() <= 2.0 * f32::EPSILON);
        }
    }
}

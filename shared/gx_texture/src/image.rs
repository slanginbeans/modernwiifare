//! RGBA8 images, gamma-correct resizing and mipmap chains.

use alloc::vec;
use alloc::vec::Vec;

/// Straight (non-premultiplied) RGBA8, row-major, sRGB colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
}

impl Image {
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> Image {
        Image {
            width,
            height,
            pixels: vec![fill; width as usize * height as usize],
        }
    }

    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> [u8; 4]) -> Image {
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.push(f(x, y));
            }
        }
        Image {
            width,
            height,
            pixels,
        }
    }

    /// Pixel at (x, y), clamped to the edge. Used for block padding.
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let x = x.min(self.width - 1);
        let y = y.min(self.height - 1);
        self.pixels[(y * self.width + x) as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, p: [u8; 4]) {
        self.pixels[(y * self.width + x) as usize] = p;
    }
}

/// sRGB byte -> linear light in 0..1.
fn to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        libm::powf((c + 0.055) / 1.055, 2.4)
    }
}

/// Linear light in 0..1 -> sRGB byte.
fn to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * libm::powf(l, 1.0 / 2.4) - 0.055
    };
    (c * 255.0 + 0.5) as u8
}

fn linear_table() -> [f32; 256] {
    let mut t = [0.0; 256];
    for (i, v) in t.iter_mut().enumerate() {
        *v = to_linear(i as u8);
    }
    t
}

/// Average a set of pixels: colour in linear light, weighted by alpha (so transparent texels
/// don't darken cut-out edges); alpha averaged linearly.
fn average(lin: &[f32; 256], px: impl Iterator<Item = ([u8; 4], f32)>) -> [u8; 4] {
    let (mut r, mut g, mut b, mut a, mut wsum, mut cr, mut cg, mut cb, mut n) = (
        0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32,
    );
    for (p, w) in px {
        let aw = p[3] as f32 / 255.0 * w;
        r += lin[p[0] as usize] * aw;
        g += lin[p[1] as usize] * aw;
        b += lin[p[2] as usize] * aw;
        wsum += aw;
        cr += lin[p[0] as usize] * w;
        cg += lin[p[1] as usize] * w;
        cb += lin[p[2] as usize] * w;
        a += p[3] as f32 * w;
        n += w;
    }
    if n == 0.0 {
        return [0; 4];
    }
    let (r, g, b) = if wsum > 1e-6 {
        (r / wsum, g / wsum, b / wsum)
    } else {
        (cr / n, cg / n, cb / n)
    };
    [to_srgb(r), to_srgb(g), to_srgb(b), (a / n + 0.5) as u8]
}

/// Area-average resize. Intended for downscaling; upscaling falls back to nearest neighbour.
pub fn resize(img: &Image, width: u32, height: u32) -> Image {
    assert!(width > 0 && height > 0);
    if width == img.width && height == img.height {
        return img.clone();
    }
    let lin = linear_table();
    let sx = img.width as f32 / width as f32;
    let sy = img.height as f32 / height as f32;
    Image::from_fn(width, height, |x, y| {
        if sx <= 1.0 && sy <= 1.0 {
            return img.get((x as f32 * sx) as u32, (y as f32 * sy) as u32);
        }
        // Source rectangle covered by this destination pixel, with fractional edge weights.
        let (x0, x1) = (x as f32 * sx, (x + 1) as f32 * sx);
        let (y0, y1) = (y as f32 * sy, (y + 1) as f32 * sy);
        let mut samples = Vec::new();
        let mut yy = libm::floorf(y0) as u32;
        while (yy as f32) < y1 && yy < img.height {
            let wy = (y1.min(yy as f32 + 1.0) - y0.max(yy as f32)).max(0.0);
            let mut xx = libm::floorf(x0) as u32;
            while (xx as f32) < x1 && xx < img.width {
                let wx = (x1.min(xx as f32 + 1.0) - x0.max(xx as f32)).max(0.0);
                samples.push((img.get(xx, yy), wx * wy));
                xx += 1;
            }
            yy += 1;
        }
        average(&lin, samples.into_iter())
    })
}

fn nearest_pow2(v: f32) -> u32 {
    if v <= 1.0 {
        return 1;
    }
    let lo = 1u32 << (libm::floorf(libm::log2f(v)) as u32);
    let hi = lo * 2;
    // Compare in log space so 3 -> 4 and 2.8 -> 2 (geometric midpoint).
    if v * v >= (lo * hi) as f32 { hi } else { lo }
}

/// Power-of-two size that fits in `max_dim` on the long side, keeping the aspect ratio as close
/// as powers of two allow. Never upscales beyond the source.
pub fn fit_pow2(width: u32, height: u32, max_dim: u32) -> (u32, u32) {
    let scale = (max_dim as f32 / width.max(height) as f32).min(1.0);
    let w = nearest_pow2(width as f32 * scale).min(max_dim);
    let h = nearest_pow2(height as f32 * scale).min(max_dim);
    (w, h)
}

/// Fraction of texels whose alpha is at least `threshold` (what an alpha test at that threshold lets through).
pub fn coverage(img: &Image, threshold: u8) -> f32 {
    let n = img.pixels.len();
    if n == 0 {
        return 0.0;
    }
    img.pixels.iter().filter(|p| p[3] >= threshold).count() as f32 / n as f32
}

/// Cut alpha to 0 or 255 so that `target` of the texels stay opaque.
///
/// Alpha-test coverage preservation: a downscaled cut-out has averaged (smooth) alpha, and cutting it at a fixed 50%
/// thins out branches, bars and leaves that are narrower than a texel. Picking the cut level from the full-size
/// coverage keeps the same fraction of the texture solid at every size.
///
/// Texels above the cut level become opaque. When the level itself holds more texels than needed (fine detail that
/// averaged to one even alpha), just enough of them are switched on in 4x4 Bayer order, so coverage stays right and
/// the result is deterministic.
pub fn cut_to_coverage(img: &mut Image, target: f32) {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let n = img.pixels.len();
    let want = libm::roundf(target.clamp(0.0, 1.0) * n as f32) as usize;
    let mut hist = [0usize; 256];
    for p in &img.pixels {
        hist[p[3] as usize] += 1;
    }
    // Highest cut level t whose texels (alpha >= t) reach `want`; alpha 0 texels are never drawn.
    let (mut cut, mut at_or_above) = (1usize, 0usize);
    for t in (1..=255usize).rev() {
        at_or_above += hist[t];
        cut = t;
        if at_or_above >= want {
            break;
        }
    }
    let above = at_or_above - hist[cut];
    let mut extra = want.saturating_sub(above);
    // Texels exactly at the cut level, in Bayer order.
    let w = img.width.max(1) as usize;
    let mut ties: Vec<usize> = (0..n)
        .filter(|&i| img.pixels[i][3] as usize == cut)
        .collect();
    ties.sort_by_key(|&i| (BAYER[(i / w) % 4][(i % w) % 4], i));
    let mut on = vec![false; n];
    for i in ties {
        if extra == 0 {
            break;
        }
        on[i] = true;
        extra -= 1;
    }
    for (i, p) in img.pixels.iter_mut().enumerate() {
        p[3] = if p[3] as usize > cut || on[i] { 255 } else { 0 };
    }
}

/// Mip chain from `base`: each level halves both sides (never below 1) until both are at most
/// `min_dim` or a level is 1x1. Level 0 is `base` itself.
pub fn mip_chain(base: &Image, min_dim: u32) -> Vec<Image> {
    let mut chain = vec![base.clone()];
    loop {
        let last = chain.last().unwrap();
        if (last.width <= min_dim && last.height <= min_dim)
            || (last.width == 1 && last.height == 1)
        {
            break;
        }
        let (w, h) = ((last.width / 2).max(1), (last.height / 2).max(1));
        let next = resize(last, w, h);
        chain.push(next);
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pow2_examples() {
        assert_eq!(fit_pow2(1000, 300, 256), (256, 64));
        assert_eq!(fit_pow2(512, 512, 256), (256, 256));
        assert_eq!(fit_pow2(100, 100, 256), (128, 128)); // nearest pow2 of 100 is 128
        assert_eq!(fit_pow2(90, 90, 256), (64, 64)); // geometric midpoint of 64..128 is ~90.5
        assert_eq!(fit_pow2(2048, 16, 256), (256, 2));
        assert_eq!(fit_pow2(1, 1, 256), (1, 1));
    }

    #[test]
    fn checkerboard_averages_to_mid_grey_in_linear_light() {
        let img = Image::from_fn(2, 2, |x, y| {
            if (x + y) % 2 == 0 {
                [255; 4]
            } else {
                [0, 0, 0, 255]
            }
        });
        let p = resize(&img, 1, 1).pixels[0];
        // 50% linear light is sRGB ~188, not 128.
        assert!((186..=190).contains(&p[0]), "{p:?}");
        assert_eq!(p[3], 255);
    }

    #[test]
    fn transparent_texels_do_not_darken_edges() {
        // Red opaque next to fully transparent black: the colour must stay red.
        let img = Image::from_fn(2, 1, |x, _| {
            if x == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let p = resize(&img, 1, 1).pixels[0];
        assert_eq!(&p[..3], &[255, 0, 0]);
        assert!((127..=128).contains(&p[3]), "{p:?}");
    }

    #[test]
    fn non_integer_downscale_keeps_flat_colour() {
        let img = Image::new(300, 77, [10, 200, 30, 255]);
        let small = resize(&img, 64, 16);
        assert!(small.pixels.iter().all(|&p| p == [10, 200, 30, 255]));
    }

    #[test]
    fn coverage_counts_texels_passing_the_test() {
        let img = Image::from_fn(4, 1, |x, _| [0, 0, 0, [0, 100, 128, 255][x as usize]]);
        assert_eq!(coverage(&img, 128), 0.5);
        assert_eq!(coverage(&img, 1), 0.75);
    }

    #[test]
    fn thin_bars_survive_downscale_with_coverage_kept() {
        // 1-texel-wide opaque bars every 4 texels: 25% coverage. A 4x downscale averages each bar to alpha 64,
        // which a fixed 128 cut would erase entirely.
        let src = Image::from_fn(64, 64, |x, _| {
            [200, 200, 200, if x % 4 == 0 { 255 } else { 0 }]
        });
        let want = coverage(&src, 128);
        let mut small = resize(&src, 16, 16);
        assert_eq!(coverage(&small, 128), 0.0, "naive cut loses the bars");
        cut_to_coverage(&mut small, want);
        assert!(small.pixels.iter().all(|p| p[3] == 0 || p[3] == 255));
        assert!(
            (coverage(&small, 128) - want).abs() < 0.05,
            "{}",
            coverage(&small, 128)
        );
    }

    #[test]
    fn cut_to_coverage_edges() {
        let mut img = Image::from_fn(2, 2, |x, y| [0, 0, 0, (x * 100 + y * 50) as u8]);
        cut_to_coverage(&mut img, 0.0);
        assert!(img.pixels.iter().all(|p| p[3] == 0));
        let mut img = Image::from_fn(2, 2, |x, y| [0, 0, 0, (x * 100 + y * 50 + 1) as u8]);
        cut_to_coverage(&mut img, 1.0);
        assert!(img.pixels.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn mip_chain_levels() {
        let base = Image::new(256, 64, [1, 2, 3, 255]);
        let dims: Vec<_> = mip_chain(&base, 1)
            .iter()
            .map(|i| (i.width, i.height))
            .collect();
        assert_eq!(
            dims,
            [
                (256, 64),
                (128, 32),
                (64, 16),
                (32, 8),
                (16, 4),
                (8, 2),
                (4, 1),
                (2, 1),
                (1, 1)
            ]
        );
        let short: Vec<_> = mip_chain(&base, 8)
            .iter()
            .map(|i| (i.width, i.height))
            .collect();
        assert_eq!(short.last(), Some(&(8, 2)));
    }
}

//! Encode an [`Image`] level into GX tiled bytes, and decode it back.

use alloc::vec::Vec;

use crate::cmpr;
use crate::format::TexFormat;
use crate::image::Image;

fn q(v: u8, max: u32) -> u32 {
    (v as u32 * max + 127) / 255
}
fn x4(v: u32) -> u8 {
    (v * 17) as u8
}
fn x3(v: u32) -> u8 {
    ((v << 5) | (v << 2) | (v >> 1)) as u8
}
fn x5(v: u32) -> u8 {
    ((v << 3) | (v >> 2)) as u8
}

/// Intensity: the red channel for grey images, else Rec.601 luma.
fn luma(p: [u8; 4]) -> u8 {
    if p[0] == p[1] && p[1] == p[2] {
        p[0]
    } else {
        ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32 + 128) >> 8) as u8
    }
}

fn rgb5a3(p: [u8; 4]) -> u16 {
    let a3 = q(p[3], 7);
    if a3 == 7 {
        (0x8000 | (q(p[0], 31) << 10) | (q(p[1], 31) << 5) | q(p[2], 31)) as u16
    } else {
        ((a3 << 12) | (q(p[0], 15) << 8) | (q(p[1], 15) << 4) | q(p[2], 15)) as u16
    }
}

fn from_rgb5a3(v: u16) -> [u8; 4] {
    let v = v as u32;
    if v & 0x8000 != 0 {
        [x5((v >> 10) & 31), x5((v >> 5) & 31), x5(v & 31), 255]
    } else {
        [
            x4((v >> 8) & 15),
            x4((v >> 4) & 15),
            x4(v & 15),
            x3((v >> 12) & 7),
        ]
    }
}

fn rgb565(p: [u8; 4]) -> u16 {
    ((q(p[0], 31) << 11) | (q(p[1], 63) << 5) | q(p[2], 31)) as u16
}

/// Encode one level. Pixels outside the image (block padding) repeat the edge.
pub fn encode(img: &Image, fmt: TexFormat) -> Vec<u8> {
    let (bw, bh) = fmt.block_dims();
    let (nbx, nby) = fmt.blocks(img.width, img.height);
    let mut out = Vec::with_capacity(fmt.level_size(img.width, img.height));
    for by in 0..nby {
        for bx in 0..nbx {
            let (ox, oy) = (bx * bw, by * bh);
            let px = |x: u32, y: u32| img.get(ox + x, oy + y);
            match fmt {
                TexFormat::I4 => {
                    for y in 0..8 {
                        for x in (0..8).step_by(2) {
                            out.push(
                                ((q(luma(px(x, y)), 15) << 4) | q(luma(px(x + 1, y)), 15)) as u8,
                            );
                        }
                    }
                }
                TexFormat::I8 => {
                    for y in 0..4 {
                        for x in 0..8 {
                            out.push(luma(px(x, y)));
                        }
                    }
                }
                TexFormat::IA4 => {
                    for y in 0..4 {
                        for x in 0..8 {
                            let p = px(x, y);
                            out.push(((q(p[3], 15) << 4) | q(luma(p), 15)) as u8);
                        }
                    }
                }
                TexFormat::IA8 => {
                    for y in 0..4 {
                        for x in 0..4 {
                            let p = px(x, y);
                            out.push(p[3]);
                            out.push(luma(p));
                        }
                    }
                }
                TexFormat::Rgb565 | TexFormat::Rgb5a3 => {
                    for y in 0..4 {
                        for x in 0..4 {
                            let p = px(x, y);
                            let v = if fmt == TexFormat::Rgb565 {
                                rgb565(p)
                            } else {
                                rgb5a3(p)
                            };
                            out.extend_from_slice(&v.to_be_bytes());
                        }
                    }
                }
                TexFormat::Rgba8 => {
                    for y in 0..4 {
                        for x in 0..4 {
                            let p = px(x, y);
                            out.extend_from_slice(&[p[3], p[0]]);
                        }
                    }
                    for y in 0..4 {
                        for x in 0..4 {
                            let p = px(x, y);
                            out.extend_from_slice(&[p[1], p[2]]);
                        }
                    }
                }
                TexFormat::Cmpr => {
                    for (sx, sy) in [(0, 0), (4, 0), (0, 4), (4, 4)] {
                        let mut sub = [[0u8; 4]; 16];
                        for y in 0..4 {
                            for x in 0..4 {
                                sub[(y * 4 + x) as usize] = px(sx + x, sy + y);
                            }
                        }
                        out.extend_from_slice(&cmpr::encode_block(&sub));
                    }
                }
            }
        }
    }
    debug_assert_eq!(out.len(), fmt.level_size(img.width, img.height));
    out
}

/// Decode one level of `width` x `height` from the start of `data`.
/// Returns `None` if `data` is shorter than the level.
pub fn decode(data: &[u8], width: u32, height: u32, fmt: TexFormat) -> Option<Image> {
    if data.len() < fmt.level_size(width, height) {
        return None;
    }
    let (bw, bh) = fmt.block_dims();
    let (nbx, nby) = fmt.blocks(width, height);
    let mut img = Image::new(width, height, [0; 4]);
    let mut put = |x: u32, y: u32, p: [u8; 4]| {
        if x < width && y < height {
            img.set(x, y, p);
        }
    };
    let mut o = 0usize;
    for by in 0..nby {
        for bx in 0..nbx {
            let (ox, oy) = (bx * bw, by * bh);
            let blk = &data[o..o + fmt.block_bytes()];
            o += fmt.block_bytes();
            match fmt {
                TexFormat::I4 => {
                    for (i, &b) in blk.iter().enumerate() {
                        let (x, y) = ((i as u32 % 4) * 2, i as u32 / 4);
                        let (hi, lo) = (x4((b >> 4) as u32), x4((b & 15) as u32));
                        put(ox + x, oy + y, [hi, hi, hi, hi]);
                        put(ox + x + 1, oy + y, [lo, lo, lo, lo]);
                    }
                }
                TexFormat::I8 => {
                    for (i, &v) in blk.iter().enumerate() {
                        put(ox + i as u32 % 8, oy + i as u32 / 8, [v, v, v, v]);
                    }
                }
                TexFormat::IA4 => {
                    for (i, &b) in blk.iter().enumerate() {
                        let (a, l) = (x4((b >> 4) as u32), x4((b & 15) as u32));
                        put(ox + i as u32 % 8, oy + i as u32 / 8, [l, l, l, a]);
                    }
                }
                TexFormat::IA8 => {
                    for i in 0..16u32 {
                        let (a, l) = (blk[2 * i as usize], blk[2 * i as usize + 1]);
                        put(ox + i % 4, oy + i / 4, [l, l, l, a]);
                    }
                }
                TexFormat::Rgb565 | TexFormat::Rgb5a3 => {
                    for i in 0..16u32 {
                        let v = u16::from_be_bytes([blk[2 * i as usize], blk[2 * i as usize + 1]]);
                        let p = if fmt == TexFormat::Rgb565 {
                            let c = cmpr::expand565(v);
                            [c[0], c[1], c[2], 255]
                        } else {
                            from_rgb5a3(v)
                        };
                        put(ox + i % 4, oy + i / 4, p);
                    }
                }
                TexFormat::Rgba8 => {
                    for i in 0..16u32 {
                        let (ar, gb) = (2 * i as usize, 32 + 2 * i as usize);
                        put(
                            ox + i % 4,
                            oy + i / 4,
                            [blk[ar + 1], blk[gb], blk[gb + 1], blk[ar]],
                        );
                    }
                }
                TexFormat::Cmpr => {
                    for (k, (sx, sy)) in [(0, 0), (4, 0), (0, 4), (4, 4)].into_iter().enumerate() {
                        let sub = cmpr::decode_block(&blk[k * 8..k * 8 + 8]);
                        for (i, p) in sub.into_iter().enumerate() {
                            put(ox + sx + i as u32 % 4, oy + sy + i as u32 / 4, p);
                        }
                    }
                }
            }
        }
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(w: u32, h: u32, grey: bool) -> Image {
        let mut s: u32 = 0x1234_5678;
        Image::from_fn(w, h, |_, _| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let b = s.to_be_bytes();
            if grey { [b[0], b[0], b[0], b[1]] } else { b }
        })
    }

    #[test]
    fn lossless_formats_roundtrip_exactly() {
        let img = noise(13, 7, false);
        assert_eq!(
            decode(&encode(&img, TexFormat::Rgba8), 13, 7, TexFormat::Rgba8).unwrap(),
            img
        );
        let grey = noise(13, 7, true);
        let ia8 = decode(&encode(&grey, TexFormat::IA8), 13, 7, TexFormat::IA8).unwrap();
        assert_eq!(ia8, grey);
        let i8 = decode(&encode(&grey, TexFormat::I8), 13, 7, TexFormat::I8).unwrap();
        assert!(
            i8.pixels
                .iter()
                .zip(&grey.pixels)
                .all(|(a, b)| a[0] == b[0])
        );
    }

    #[test]
    fn quantised_formats_stay_within_step() {
        let img = noise(16, 16, false);
        let grey = noise(16, 16, true);
        let cases: [(&Image, TexFormat, [i32; 4]); 5] = [
            (&img, TexFormat::Rgb565, [4, 2, 4, 255]), // no alpha channel
            (&img, TexFormat::Rgb5a3, [8, 8, 8, 18]),
            (&grey, TexFormat::I4, [8, 8, 8, 255]),
            (&grey, TexFormat::IA4, [8, 8, 8, 8]),
            (&grey, TexFormat::I8, [0, 0, 0, 255]),
        ];
        for (src, fmt, tol) in cases {
            let dec = decode(&encode(src, fmt), 16, 16, fmt).unwrap();
            for (a, b) in src.pixels.iter().zip(&dec.pixels) {
                for c in 0..4 {
                    let d = (a[c] as i32 - b[c] as i32).abs();
                    assert!(d <= tol[c], "{fmt:?} ch{c}: {a:?} -> {b:?}");
                }
            }
        }
    }

    #[test]
    fn tiling_order_matches_gx_layout() {
        // RGBA8 4x4 blocks: pixel (4,0) starts the second block; its AR pair is at byte 64.
        let img = Image::from_fn(8, 8, |x, y| [x as u8, y as u8, 0, 255]);
        let enc = encode(&img, TexFormat::Rgba8);
        assert_eq!(&enc[64..66], &[255, 4]); // A, R of (4,0)
        assert_eq!(&enc[2 * 64 + 2..2 * 64 + 4], &[255, 1]); // (1,4): third block, second pixel
        // I8 8x4 blocks: pixel (0,4) starts the second block row.
        let grey = Image::from_fn(8, 8, |x, y| {
            let v = (y * 8 + x) as u8;
            [v, v, v, 255]
        });
        let i8 = encode(&grey, TexFormat::I8);
        assert_eq!(i8[32], 32);
        assert_eq!(i8[9], 9);
    }

    #[test]
    fn cmpr_subblock_order() {
        // Four solid quadrants must come back in the right places.
        let q = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ];
        let img = Image::from_fn(8, 8, |x, y| q[((y / 4) * 2 + x / 4) as usize]);
        let dec = decode(&encode(&img, TexFormat::Cmpr), 8, 8, TexFormat::Cmpr).unwrap();
        assert_eq!(dec, img);
    }

    #[test]
    fn rgb5a3_switches_mode_on_alpha() {
        assert_eq!(rgb5a3([255, 255, 255, 255]) & 0x8000, 0x8000);
        assert_eq!(rgb5a3([255, 255, 255, 100]) & 0x8000, 0);
        assert_eq!(from_rgb5a3(rgb5a3([255, 0, 0, 255])), [255, 0, 0, 255]);
        assert_eq!(from_rgb5a3(rgb5a3([0, 0, 0, 0]))[3], 0);
    }

    #[test]
    fn short_buffer_is_rejected() {
        assert!(decode(&[0u8; 31], 8, 8, TexFormat::Cmpr).is_none());
    }
}

//! Decode IW4 image payloads (in-zone `GfxImageLoadDef` data and IWI v8 files) to RGBA8.
//!
//! Adapted from IW4L `crates/asset_material/src/material_images/decode.rs` (Apache-2.0, rev `acc57a5`): the D3D and
//! IWI format tables, the IWI v8 top-mip range, and BCn block decoding through `bcdec_rs`. That crate depends on
//! Bevy, so the small part we need is carried here instead (see docs/references.md).

use gx_texture::image::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8,
    Bgrx8,
    Rgb8,
    L8,
    La8,
    A8,
    Bc1,
    Bc2,
    Bc3,
    /// Two-channel (normal maps). Not decoded: normal maps are dropped before decoding.
    Bc5,
}

impl PixelFormat {
    pub fn name(self) -> &'static str {
        match self {
            PixelFormat::Bgra8 => "bgra8",
            PixelFormat::Bgrx8 => "bgrx8",
            PixelFormat::Rgb8 => "rgb8",
            PixelFormat::L8 => "l8",
            PixelFormat::La8 => "la8",
            PixelFormat::A8 => "a8",
            PixelFormat::Bc1 => "dxt1",
            PixelFormat::Bc2 => "dxt3",
            PixelFormat::Bc3 => "dxt5",
            PixelFormat::Bc5 => "bc5",
        }
    }
}

/// D3D9 format code stored in an in-zone `GfxImageLoadDef`.
pub fn d3d_format(format: u32) -> Result<PixelFormat, String> {
    Ok(match format {
        f if f == u32::from_le_bytes(*b"DXT1") => PixelFormat::Bc1,
        f if f == u32::from_le_bytes(*b"DXT3") => PixelFormat::Bc2,
        f if f == u32::from_le_bytes(*b"DXT5") => PixelFormat::Bc3,
        21 => PixelFormat::Bgra8,
        22 => PixelFormat::Bgrx8,
        28 => PixelFormat::A8,
        50 => PixelFormat::L8,
        51 => PixelFormat::La8,
        other => return Err(format!("unsupported D3D format {other}")),
    })
}

/// IWI v8 format byte.
pub fn iwi_format(format: u8) -> Result<PixelFormat, String> {
    Ok(match format {
        1 => PixelFormat::Bgra8,
        2 => PixelFormat::Rgb8,
        3 => PixelFormat::La8,
        4 => PixelFormat::L8,
        5 => PixelFormat::A8,
        11 => PixelFormat::Bc1,
        12 => PixelFormat::Bc2,
        13 => PixelFormat::Bc3,
        14 => PixelFormat::Bc5,
        6..=10 => return Err("wavelet-compressed IWI (not supported yet)".into()),
        other => return Err(format!("unsupported IWI format {other}")),
    })
}

/// Header facts of an IWI v8 file, and where its top mip lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IwiInfo {
    pub width: u32,
    pub height: u32,
    pub format: Result<PixelFormat, &'static str>,
    pub usage: u8,
    pub top_mip: (usize, usize),
}

/// IWI v8 stores mips smallest first; `file_size_for_picmip[0]` ends the top mip and `[1]` starts it.
pub fn iwi_info(bytes: &[u8]) -> Result<IwiInfo, String> {
    let header = asset_iw4::IwiHeader::parse(bytes).map_err(|e| format!("IWI header: {e:?}"))?;
    let end = (header.file_size_for_picmip[0] as usize).min(bytes.len());
    let start = header.file_size_for_picmip[1] as usize;
    let start = if (asset_iw4::IWI_V8_HEADER_LEN..end).contains(&start) {
        start
    } else {
        asset_iw4::IWI_V8_HEADER_LEN
    };
    Ok(IwiInfo {
        width: u32::from(header.width),
        height: u32::from(header.height),
        format: iwi_format(header.format).map_err(|_| "unsupported IWI format"),
        usage: header.usage,
        top_mip: (start, end),
    })
}

/// Decode the top mip of `data` (which starts at that mip) to RGBA8.
pub fn decode(data: &[u8], width: u32, height: u32, format: PixelFormat) -> Result<Image, String> {
    if width == 0 || height == 0 {
        return Err("zero-sized image".into());
    }
    let (w, h) = (width as usize, height as usize);
    let take = |n: usize| {
        data.get(..n)
            .ok_or_else(|| format!("truncated {} image", format.name()))
    };
    let pixels: Vec<[u8; 4]> = match format {
        PixelFormat::Bgra8 => take(w * h * 4)?
            .chunks_exact(4)
            .map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
        PixelFormat::Bgrx8 => take(w * h * 4)?
            .chunks_exact(4)
            .map(|p| [p[2], p[1], p[0], 255])
            .collect(),
        PixelFormat::Rgb8 => take(w * h * 3)?
            .chunks_exact(3)
            .map(|p| [p[2], p[1], p[0], 255])
            .collect(),
        PixelFormat::L8 => take(w * h)?.iter().map(|&v| [v, v, v, 255]).collect(),
        PixelFormat::A8 => take(w * h)?.iter().map(|&v| [255, 255, 255, v]).collect(),
        PixelFormat::La8 => take(w * h * 2)?
            .chunks_exact(2)
            .map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        PixelFormat::Bc1 | PixelFormat::Bc2 | PixelFormat::Bc3 => {
            return decode_blocks(data, width, height, format);
        }
        PixelFormat::Bc5 => return Err("BC5 (normal map) is not decoded".into()),
    };
    Ok(Image {
        width,
        height,
        pixels,
    })
}

fn decode_blocks(
    data: &[u8],
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<Image, String> {
    let block = if format == PixelFormat::Bc1 { 8 } else { 16 };
    let (bw, bh) = (width.div_ceil(4) as usize, height.div_ceil(4) as usize);
    if data.len() < bw * bh * block {
        return Err(format!("truncated {} image", format.name()));
    }
    let mut out = Image::new(width, height, [0; 4]);
    let mut tile = [0u8; 64];
    for by in 0..bh {
        for bx in 0..bw {
            let src = &data[(by * bw + bx) * block..][..block];
            match format {
                PixelFormat::Bc1 => bcdec_rs::bc1(src, &mut tile, 16),
                PixelFormat::Bc2 => bcdec_rs::bc2(src, &mut tile, 16),
                _ => bcdec_rs::bc3(src, &mut tile, 16),
            }
            for y in 0..4 {
                for x in 0..4 {
                    let (px, py) = (bx * 4 + x, by * 4 + y);
                    if px < width as usize && py < height as usize {
                        let t = &tile[(y * 4 + x) * 4..][..4];
                        out.pixels[py * width as usize + px] = [t[0], t[1], t[2], t[3]];
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d3d_codes() {
        assert_eq!(
            d3d_format(u32::from_le_bytes(*b"DXT1")),
            Ok(PixelFormat::Bc1)
        );
        assert_eq!(
            d3d_format(u32::from_le_bytes(*b"DXT5")),
            Ok(PixelFormat::Bc3)
        );
        assert_eq!(d3d_format(21), Ok(PixelFormat::Bgra8));
        assert!(d3d_format(999).is_err());
    }

    #[test]
    fn bgra_swizzles_to_rgba() {
        let img = decode(&[1, 2, 3, 4], 1, 1, PixelFormat::Bgra8).unwrap();
        assert_eq!(img.pixels, [[3, 2, 1, 4]]);
        assert!(decode(&[1, 2, 3], 1, 1, PixelFormat::Bgra8).is_err());
    }

    #[test]
    fn dxt1_solid_block() {
        // colour0 = colour1 = pure red (RGB565 0xF800), every index 0: a solid red 4x4 block.
        let block = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let img = decode(&block, 4, 4, PixelFormat::Bc1).unwrap();
        assert!(
            img.pixels.iter().all(|&p| p == [255, 0, 0, 255]),
            "{:?}",
            img.pixels[0]
        );
        // Non-multiple-of-4 sizes crop the block.
        assert_eq!(
            decode(&block, 2, 3, PixelFormat::Bc1).unwrap().pixels.len(),
            6
        );
    }

    #[test]
    fn iwi_top_mip_range() {
        // Synthetic IWI v8 header: 4x4 DXT1, mips smallest first, top mip = last 8 bytes.
        let mut f = Vec::from(*b"IWi\x08");
        f.extend_from_slice(&[0, 0, 0, 0]); // flags, ?, usage, ?
        f.push(11); // format DXT1
        f.push(0);
        f.extend_from_slice(&4u16.to_le_bytes());
        f.extend_from_slice(&4u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        let total = 32 + 8 + 8;
        for v in [total, total - 8, total - 8, total - 8] {
            f.extend_from_slice(&(v as u32).to_le_bytes());
        }
        f.resize(total, 0);
        let info = iwi_info(&f).unwrap();
        assert_eq!((info.width, info.height), (4, 4));
        assert_eq!(info.format, Ok(PixelFormat::Bc1));
        assert_eq!(info.top_mip, (40, 48));
    }
}

//! `mwf-convert texture`: convert one PNG to a Wii texture, for trying out the encoders.
//!
//! Writes `<out>/<stem>.gxt` (the Wii texture) and `<out>/<stem>_preview.png` (level 0 decoded,
//! i.e. what the Wii will show). Both go under `out/` by default, which git ignores. Point it only
//! at your own images or at textures from your own install; never commit the results.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use gx_texture::image::{Image, resize};
use gx_texture::policy::{Converted, Role, convert};
use gx_texture::{container, format::TexFormat};

pub fn read_png(path: &Path) -> Result<Image, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(file);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (info.width, info.height);
    let bytes = &buf[..info.buffer_size()];
    let px: Vec<[u8; 4]> = match info.color_type {
        png::ColorType::Rgba => bytes
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect(),
        png::ColorType::Rgb => bytes
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .chunks_exact(2)
            .map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => bytes.iter().map(|&v| [v, v, v, 255]).collect(),
        other => {
            return Err(format!(
                "{}: unsupported PNG colour type {other:?}",
                path.display()
            ));
        }
    };
    Ok(Image {
        width: w,
        height: h,
        pixels: px,
    })
}

pub fn write_png(path: &Path, img: &Image) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let flat: Vec<u8> = img.pixels.iter().flatten().copied().collect();
    w.write_image_data(&flat).map_err(|e| e.to_string())
}

/// PSNR of RGB (and alpha, weighted equally) between two same-sized images.
pub fn psnr(a: &Image, b: &Image) -> f64 {
    let mut se = 0.0f64;
    let mut n = 0.0f64;
    for (p, q) in a.pixels.iter().zip(&b.pixels) {
        for c in 0..4 {
            let d = p[c] as f64 - q[c] as f64;
            se += d * d;
            n += 1.0;
        }
    }
    if se == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / (se / n)).log10()
    }
}

pub struct Summary {
    pub converted: Converted,
    pub source_bytes: usize,
    pub psnr_db: f64,
}

pub fn convert_file(
    input: &Path,
    role: Role,
    out_dir: &Path,
) -> Result<(Summary, PathBuf, PathBuf), String> {
    let src = read_png(input)?;
    let c = convert(&src, role);
    let level0 = c.decode_level(0).ok_or("internal: level 0 missing")?;
    // Quality is measured against the source resized to the same size, so it isolates the
    // format's loss from the deliberate downscale.
    let reference = resize(&src, c.width, c.height);
    let summary = Summary {
        source_bytes: src.pixels.len() * 4,
        psnr_db: psnr(&reference, &level0),
        converted: c,
    };
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "texture".into());
    let gxt = out_dir.join(format!("{stem}.gxt"));
    let preview = out_dir.join(format!("{stem}_preview.png"));
    std::fs::write(&gxt, container::write(&summary.converted))
        .map_err(|e| format!("{}: {e}", gxt.display()))?;
    write_png(&preview, &level0)?;
    Ok((summary, gxt, preview))
}

pub fn describe(s: &Summary) -> String {
    let c = &s.converted;
    let fmt = match c.format {
        TexFormat::Cmpr => "CMPR",
        TexFormat::I4 => "I4",
        TexFormat::I8 => "I8",
        TexFormat::IA4 => "IA4",
        TexFormat::IA8 => "IA8",
        TexFormat::Rgb565 => "RGB565",
        TexFormat::Rgb5a3 => "RGB5A3",
        TexFormat::Rgba8 => "RGBA8",
    };
    format!(
        "{fmt} {}x{}, {} level(s), {} KB (source as RGBA8: {} KB, {:.1}x smaller)\n\
         content: {}, alpha {:?}\n\
         quality vs resized source: {:.1} dB PSNR",
        c.width,
        c.height,
        c.levels,
        c.data.len().div_ceil(1024),
        s.source_bytes.div_ceil(1024),
        s.source_bytes as f64 / c.data.len().max(1) as f64,
        if c.traits.grey { "grey" } else { "colour" },
        c.traits.alpha,
        s.psnr_db
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip_and_convert() {
        let dir = std::env::temp_dir().join(format!("mwf_tex_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = Image::from_fn(200, 100, |x, y| [x as u8, y as u8 * 2, 128, 255]);
        let input = dir.join("synthetic.png");
        write_png(&input, &src).unwrap();
        assert_eq!(read_png(&input).unwrap(), src);

        let (s, gxt, preview) = convert_file(&input, Role::Prop, &dir.join("out")).unwrap();
        assert_eq!(
            (s.converted.format, s.converted.width, s.converted.height),
            (TexFormat::Cmpr, 128, 64)
        );
        assert!(s.psnr_db > 30.0, "{}", s.psnr_db);
        let file = std::fs::read(&gxt).unwrap();
        let (h, data) = container::parse(&file).unwrap();
        assert_eq!((h.width, h.height), (128, 64));
        assert_eq!(data, &s.converted.data[..]);
        assert_eq!(read_png(&preview).unwrap().width, 128);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

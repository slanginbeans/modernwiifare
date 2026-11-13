//! `.gxt`: a minimal single-texture file, used until the Phase 1 pack format replaces it.
//!
//! ```text
//!  0  magic "GXT1"
//!  4  gx format id (u8)      5  mip levels (u8)    6  flags (u16 BE)
//!  8  width  (u16 BE)       10  height (u16 BE)   12  data length (u32 BE)
//! 16  data: level 0, level 1, ... (GX tiled, 32-byte aligned since the header is 16 and levels are whole blocks)
//! ```
//! The 16-byte header keeps the data 32-byte aligned when the file itself is loaded 32-byte aligned
//! (GX requires 32-byte aligned texture memory).
//!
//! Flag bit 0 ([`FLAG_ALPHA_PLANE`]): the texture is split (see [`Converted`]). A second block with the same layout
//! follows the data: a 16-byte header (format I4, same size and levels) and the alpha plane. Every level of CMPR and
//! I4 is whole 32-byte blocks, so the second block's data is 32-byte aligned too.

use alloc::vec::Vec;

use crate::format::TexFormat;
use crate::policy::{ALPHA_PLANE_FORMAT, Converted};

pub const MAGIC: [u8; 4] = *b"GXT1";
pub const HEADER_LEN: usize = 16;
/// Flag: an I4 alpha plane block follows the colour data.
pub const FLAG_ALPHA_PLANE: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: TexFormat,
    pub levels: u8,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub data_len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    TooShort,
    BadMagic,
    UnknownFormat(u8),
    BadSize,
    /// The alpha plane block is missing, or does not match the colour block.
    BadAlphaPlane,
}

fn write_block(out: &mut Vec<u8>, format: TexFormat, c: &Converted, flags: u16, data: &[u8]) {
    out.extend_from_slice(&MAGIC);
    out.push(format.gx_id());
    out.push(c.levels as u8);
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&(c.width as u16).to_be_bytes());
    out.extend_from_slice(&(c.height as u16).to_be_bytes());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

pub fn write(c: &Converted) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * HEADER_LEN + c.bytes());
    let flags = if c.alpha_plane.is_some() {
        FLAG_ALPHA_PLANE
    } else {
        0
    };
    write_block(&mut out, c.format, c, flags, &c.data);
    if let Some(plane) = &c.alpha_plane {
        write_block(&mut out, ALPHA_PLANE_FORMAT, c, 0, plane);
    }
    out
}

fn parse_block(b: &[u8]) -> Result<(Header, &[u8]), ParseError> {
    if b.len() < HEADER_LEN {
        return Err(ParseError::TooShort);
    }
    if b[0..4] != MAGIC {
        return Err(ParseError::BadMagic);
    }
    let format = TexFormat::from_gx_id(b[4]).ok_or(ParseError::UnknownFormat(b[4]))?;
    let h = Header {
        format,
        levels: b[5],
        flags: u16::from_be_bytes([b[6], b[7]]),
        width: u16::from_be_bytes([b[8], b[9]]),
        height: u16::from_be_bytes([b[10], b[11]]),
        data_len: u32::from_be_bytes([b[12], b[13], b[14], b[15]]),
    };
    let expect = format.chain_size(h.width as u32, h.height as u32, h.levels as u32);
    if h.data_len as usize != expect || b.len() < HEADER_LEN + expect {
        return Err(ParseError::BadSize);
    }
    Ok((h, &b[HEADER_LEN..HEADER_LEN + expect]))
}

/// Parse the header and return it with the (colour) texture data.
pub fn parse(b: &[u8]) -> Result<(Header, &[u8]), ParseError> {
    parse_block(b)
}

/// The alpha plane of a split texture: `Ok(None)` when the file has none.
pub fn parse_alpha_plane(b: &[u8]) -> Result<Option<(Header, &[u8])>, ParseError> {
    let (h, data) = parse_block(b)?;
    if h.flags & FLAG_ALPHA_PLANE == 0 {
        return Ok(None);
    }
    let rest = &b[HEADER_LEN + data.len()..];
    let (a, plane) = parse_block(rest).map_err(|_| ParseError::BadAlphaPlane)?;
    if a.format != ALPHA_PLANE_FORMAT
        || (a.width, a.height, a.levels) != (h.width, h.height, h.levels)
    {
        return Err(ParseError::BadAlphaPlane);
    }
    Ok(Some((a, plane)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Image;
    use crate::policy::{Role, convert};

    #[test]
    fn roundtrip_and_validation() {
        let c = convert(&Image::new(64, 32, [1, 2, 3, 255]), Role::Prop);
        let file = write(&c);
        assert_eq!(file.len() % 32, HEADER_LEN % 32 + c.data.len() % 32);
        let (h, data) = parse(&file).unwrap();
        assert_eq!(
            (h.format, h.width, h.height, h.levels as u32),
            (c.format, 64, 32, c.levels)
        );
        assert_eq!(data, &c.data[..]);

        assert_eq!(parse(&file[..10]), Err(ParseError::TooShort));
        let mut bad = file.clone();
        bad[0] = b'X';
        assert_eq!(parse(&bad), Err(ParseError::BadMagic));
        let mut bad = file.clone();
        bad[4] = 7;
        assert_eq!(parse(&bad), Err(ParseError::UnknownFormat(7)));
        assert_eq!(parse(&file[..file.len() - 1]), Err(ParseError::BadSize));
        assert_eq!(parse_alpha_plane(&file), Ok(None));
    }

    #[test]
    fn split_texture_roundtrip() {
        let src = Image::from_fn(64, 64, |x, y| [200, 100, 50, ((x + y) * 2) as u8]);
        let c = convert(&src, Role::World);
        let plane = c
            .alpha_plane
            .as_ref()
            .expect("smooth colour texture is split");
        let file = write(&c);
        assert_eq!(file.len(), 2 * HEADER_LEN + c.bytes());
        let (h, data) = parse(&file).unwrap();
        assert_eq!(h.flags, FLAG_ALPHA_PLANE);
        assert_eq!(data, &c.data[..]);
        let (a, alpha) = parse_alpha_plane(&file).unwrap().unwrap();
        assert_eq!(
            (a.format, a.width, a.height),
            (ALPHA_PLANE_FORMAT, h.width, h.height)
        );
        assert_eq!(alpha, &plane[..]);
        // The alpha plane's data starts 32-byte aligned.
        assert_eq!((2 * HEADER_LEN + c.data.len()) % 32, 0);
        // Truncated alpha plane.
        assert_eq!(
            parse_alpha_plane(&file[..file.len() - 1]),
            Err(ParseError::BadAlphaPlane)
        );
    }
}

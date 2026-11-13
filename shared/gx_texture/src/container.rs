//! `.gxt`: a minimal single-texture file, used until the Phase 1 pack format replaces it.
//!
//! ```text
//!  0  magic "GXT1"
//!  4  gx format id (u8)      5  mip levels (u8)    6  reserved (u16, 0)
//!  8  width  (u16 BE)       10  height (u16 BE)   12  data length (u32 BE)
//! 16  data: level 0, level 1, ... (GX tiled, 32-byte aligned since the header is 16 and levels are whole blocks)
//! ```
//! The 16-byte header keeps the data 32-byte aligned when the file itself is loaded 32-byte aligned
//! (GX requires 32-byte aligned texture memory).

use alloc::vec::Vec;

use crate::format::TexFormat;
use crate::policy::Converted;

pub const MAGIC: [u8; 4] = *b"GXT1";
pub const HEADER_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: TexFormat,
    pub levels: u8,
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
}

pub fn write(c: &Converted) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + c.data.len());
    out.extend_from_slice(&MAGIC);
    out.push(c.format.gx_id());
    out.push(c.levels as u8);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(c.width as u16).to_be_bytes());
    out.extend_from_slice(&(c.height as u16).to_be_bytes());
    out.extend_from_slice(&(c.data.len() as u32).to_be_bytes());
    out.extend_from_slice(&c.data);
    out
}

/// Parse the header and return it with the texture data.
pub fn parse(b: &[u8]) -> Result<(Header, &[u8]), ParseError> {
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
    }
}

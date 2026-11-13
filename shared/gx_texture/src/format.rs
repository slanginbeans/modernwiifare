//! GX texture formats and their memory layout.
//!
//! Every format is stored as a grid of fixed-size blocks, row-major across the image; each block is
//! 32 bytes (RGBA8 blocks are 64: two 32-byte halves). Images are padded up to whole blocks.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexFormat {
    /// 4-bit intensity. 8x8 blocks.
    I4,
    /// 8-bit intensity. 8x4 blocks.
    I8,
    /// 4-bit alpha + 4-bit intensity. 8x4 blocks.
    IA4,
    /// 8-bit alpha + 8-bit intensity. 4x4 blocks.
    IA8,
    /// 16-bit RGB 5:6:5. 4x4 blocks.
    Rgb565,
    /// 16-bit: RGB555 when opaque, else ARGB 3:4:4:4. 4x4 blocks.
    Rgb5a3,
    /// 32-bit RGBA, split into an AR half and a GB half per 4x4 block.
    Rgba8,
    /// 4-bit compressed (GameCube DXT1 variant). 8x8 blocks of four 4x4 sub-blocks.
    Cmpr,
}

impl TexFormat {
    pub const ALL: [TexFormat; 8] = [
        TexFormat::I4,
        TexFormat::I8,
        TexFormat::IA4,
        TexFormat::IA8,
        TexFormat::Rgb565,
        TexFormat::Rgb5a3,
        TexFormat::Rgba8,
        TexFormat::Cmpr,
    ];

    /// The `GX_TF_*` value libogc expects.
    pub const fn gx_id(self) -> u8 {
        match self {
            TexFormat::I4 => 0x0,
            TexFormat::I8 => 0x1,
            TexFormat::IA4 => 0x2,
            TexFormat::IA8 => 0x3,
            TexFormat::Rgb565 => 0x4,
            TexFormat::Rgb5a3 => 0x5,
            TexFormat::Rgba8 => 0x6,
            TexFormat::Cmpr => 0xE,
        }
    }

    pub fn from_gx_id(id: u8) -> Option<TexFormat> {
        TexFormat::ALL.into_iter().find(|f| f.gx_id() == id)
    }

    /// Block width and height in pixels.
    pub const fn block_dims(self) -> (u32, u32) {
        match self {
            TexFormat::I4 | TexFormat::Cmpr => (8, 8),
            TexFormat::I8 | TexFormat::IA4 => (8, 4),
            TexFormat::IA8 | TexFormat::Rgb565 | TexFormat::Rgb5a3 | TexFormat::Rgba8 => (4, 4),
        }
    }

    pub const fn block_bytes(self) -> usize {
        match self {
            TexFormat::Rgba8 => 64,
            _ => 32,
        }
    }

    pub const fn bits_per_pixel(self) -> u32 {
        match self {
            TexFormat::I4 | TexFormat::Cmpr => 4,
            TexFormat::I8 | TexFormat::IA4 => 8,
            TexFormat::IA8 | TexFormat::Rgb565 | TexFormat::Rgb5a3 => 16,
            TexFormat::Rgba8 => 32,
        }
    }

    /// Number of blocks across and down for a `width` x `height` level.
    pub const fn blocks(self, width: u32, height: u32) -> (u32, u32) {
        let (bw, bh) = self.block_dims();
        (width.div_ceil(bw), height.div_ceil(bh))
    }

    /// Bytes for one level, including block padding.
    pub const fn level_size(self, width: u32, height: u32) -> usize {
        let (bx, by) = self.blocks(width, height);
        bx as usize * by as usize * self.block_bytes()
    }

    /// Bytes for a mip chain starting at `width` x `height` with `levels` levels
    /// (each level halves both dimensions, never below 1).
    pub fn chain_size(self, width: u32, height: u32, levels: u32) -> usize {
        (0..levels)
            .map(|l| self.level_size((width >> l).max(1), (height >> l).max(1)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_sizes() {
        assert_eq!(TexFormat::Cmpr.level_size(256, 256), 32 * 1024);
        assert_eq!(TexFormat::I4.level_size(256, 256), 32 * 1024);
        assert_eq!(TexFormat::I8.level_size(256, 256), 64 * 1024);
        assert_eq!(TexFormat::Rgb5a3.level_size(256, 256), 128 * 1024);
        assert_eq!(TexFormat::Rgba8.level_size(256, 256), 256 * 1024);
        // Padding: 10x10 CMPR is 2x2 blocks; 1x1 RGB565 is one block.
        assert_eq!(TexFormat::Cmpr.level_size(10, 10), 4 * 32);
        assert_eq!(TexFormat::Rgb565.level_size(1, 1), 32);
        assert_eq!(TexFormat::I8.level_size(9, 5), 2 * 2 * 32);
    }

    #[test]
    fn bpp_matches_block_size() {
        for f in TexFormat::ALL {
            let (w, h) = f.block_dims();
            assert_eq!(
                (w * h * f.bits_per_pixel() / 8) as usize,
                f.block_bytes(),
                "{f:?}"
            );
        }
    }

    #[test]
    fn chain_size_sums_levels() {
        // 64x64 CMPR: 64,32,16,8 = 2048+512+128+32; 4x4 and below pad to one 8x8 block each.
        assert_eq!(TexFormat::Cmpr.chain_size(64, 64, 4), 2048 + 512 + 128 + 32);
        assert_eq!(
            TexFormat::Cmpr.chain_size(64, 64, 7),
            2048 + 512 + 128 + 32 * 4
        );
    }

    #[test]
    fn gx_ids_roundtrip() {
        for f in TexFormat::ALL {
            assert_eq!(TexFormat::from_gx_id(f.gx_id()), Some(f));
        }
        assert_eq!(TexFormat::from_gx_id(0x7), None);
    }
}

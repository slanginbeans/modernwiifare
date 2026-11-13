//! Wii (GX) texture formats for Modern Wiifare.
//!
//! - [`format`]: the GX formats, their block tiling and byte sizes.
//! - [`image`]: an RGBA image type plus gamma-correct resizing and mipmap chains.
//! - [`codec`]: encoders and decoders for every format (decoders are for tests, previews and debugging).
//! - [`cmpr`]: the CMPR (GameCube DXT1) block compressor.
//! - [`policy`]: picks size and format per texture role, following the `iw4-asset-pipeline` skill.
//! - [`container`]: a minimal file header until the pack format (Phase 1) takes over.
//!
//! `no_std` + `alloc` so the Wii runtime can share the definitions; the encoders run on the host.
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod cmpr;
pub mod codec;
pub mod container;
pub mod format;
pub mod image;
pub mod policy;

pub use codec::{decode, encode};
pub use format::TexFormat;
pub use image::Image;
pub use policy::{Converted, Role, convert};

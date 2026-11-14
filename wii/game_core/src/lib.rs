//! The hardware-free half of the Wii game: everything here is plain Rust, tested on the host, and drives the GPU only
//! through the [`gfx::Gfx`] trait (implemented over the C shim in `wii/game`, and by a recorder in tests).
//!
//! - [`camera`]: IW4 (Z-up) camera to GX view matrices, frustum test.
//! - [`world`]: a loaded pack's textures, materials and batches, and the per-frame draw order.
//! - [`sky`]: skybox face quads in D3D cube orientation.
//! - [`fly`]: free-fly camera on the GameCube pad.
//! - [`collision`]: IW4L traces over the pack's `CLIP` section.
//! - [`player`]: pad -> `UserCmd` -> IW4L `pmove`.
//! - [`hud`]: screen-space rectangles and a 3×5 debug font.
#![no_std]

extern crate alloc;

pub mod camera;
pub mod collision;
pub mod fly;
pub mod gfx;
pub mod hud;
pub mod player;
pub mod sky;
pub mod world;

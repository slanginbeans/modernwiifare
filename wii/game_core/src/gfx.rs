//! What the game asks of the GPU. `wii/game` implements this over `wii/platform/shim.h`; tests record the calls.

/// A texture in a loaded pack (pointers into the pack in memory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexDesc {
    pub data: *const u8,
    /// I4 alpha plane, or null.
    pub alpha: *const u8,
    pub width: u16,
    pub height: u16,
    pub format: u8,
    pub levels: u8,
    pub wrap: u8,
}

/// GPU state for one world draw (see `wpk::records::{Blend, Cull}` for the values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawState {
    /// Texture handle, or -1.
    pub tex: i32,
    /// Lightmap texture handle, or -1.
    pub lightmap: i32,
    pub blend: u8,
    pub alpha_ref: u8,
    pub cull: u8,
    /// Light colour RGBA8 at half intensity to multiply by (x2), or 0 for none (static models).
    pub tint: u32,
}

/// A GX 3x4 matrix, row-major, multiplying column vectors.
pub type Mtx34 = [[f32; 4]; 3];

pub trait Gfx {
    /// Create a texture; returns a handle, or -1.
    fn tex_create(&mut self, desc: &TexDesc) -> i32;
    fn frame_begin(&mut self, view: &Mtx34, fovy_deg: f32, znear: f32, zfar: f32, clear_rgba: u32);
    /// Point the world vertex arrays at `vertices` (interleaved: 28-byte float or 18-byte compact vertices).
    fn world_arrays(&mut self, vertices: *const u8, stride: u8);
    fn world_state(&mut self, state: &DrawState);
    /// Load the position matrix for the following draws (the view, or view x a model's placement).
    fn pos_matrix(&mut self, m: &Mtx34);
    fn call_dl(&mut self, dl: &[u8]);
    /// One sky face: 4 corners `[x, y, z, s, t]` around the eye, drawn with the rotation-only view.
    fn sky_face(&mut self, view_rot: &Mtx34, corners: &[[f32; 5]; 4], tex: i32);
    /// Solid screen-space rectangles (640×480, origin top left) over everything drawn so far.
    fn hud_rects(&mut self, rects: &[crate::hud::Rect], rgba: u32);
    fn frame_end(&mut self);
}

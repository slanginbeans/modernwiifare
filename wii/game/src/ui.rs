//! MW2's menu look on the Wii: the fonts and baked screens from `common.wpk` (`wpk::ui`), their textures, and
//! drawing (textured quads through `mwf_ui_quads`). Textures are (re)created after every texture reset: at start-up
//! and after each map loads (the map's load resets all texture handles).

use alloc::vec::Vec;

use game_core::gfx::{Gfx, TexDesc};
use wpk::records::{self, TextureRecord, texture_flags};
use wpk::ui::{NONE, Quad, UiPack};

use crate::ffi;

pub struct Ui {
    pub pack: UiPack,
    textures: Vec<TextureRecord>,
    txda: &'static [u8],
    handles: Vec<i32>,
    quads: Vec<Quad>,
    out: Vec<ffi::UiQuad>,
}

static mut UI: Option<Ui> = None;

/// The menu look, when `common.wpk` has one.
#[allow(static_mut_refs)]
pub fn get() -> Option<&'static mut Ui> {
    // Single-threaded: the front end and the match loop only.
    unsafe { UI.as_mut() }
}

/// Load the fonts and screens from the common pack (in MEM2 for the whole run) and create their textures.
pub fn init(common: &'static [u8], gfx: &mut impl Gfx) {
    let Ok(pack) = wpk::Pack::parse(common, false) else {
        return;
    };
    let Some(ui) = pack
        .section(wpk::tag::UI)
        .and_then(|s| UiPack::decode(s.data))
    else {
        return;
    };
    let textures = pack
        .section(wpk::tag::TEXTURES)
        .map(|s| records::iter::<TextureRecord>(s.data).collect())
        .unwrap_or_default();
    let txda = pack
        .section(wpk::tag::TEXTURE_DATA)
        .map_or(&[][..], |s| s.data);
    let mut u = Ui {
        pack: ui,
        textures,
        txda,
        handles: Vec::new(),
        quads: Vec::with_capacity(512),
        out: Vec::with_capacity(512),
    };
    u.create_textures(gfx);
    unsafe { UI = Some(u) };
}

impl Ui {
    /// Create the textures again (after `mwf_tex_reset`).
    pub fn create_textures(&mut self, gfx: &mut impl Gfx) {
        self.handles.clear();
        for r in &self.textures {
            let Some(data) = self
                .txda
                .get(r.data_off as usize..(r.data_off + r.data_len) as usize)
            else {
                self.handles.push(-1);
                continue;
            };
            let alpha = if r.flags & texture_flags::ALPHA_PLANE != 0 {
                self.txda
                    .get(r.alpha_off as usize..(r.alpha_off + r.alpha_len) as usize)
                    .map_or(core::ptr::null(), |a| a.as_ptr())
            } else {
                core::ptr::null()
            };
            self.handles.push(gfx.tex_create(&TexDesc {
                data: data.as_ptr(),
                alpha,
                width: r.width,
                height: r.height,
                format: r.format,
                levels: r.levels,
                wrap: r.wrap,
            }));
        }
    }

    /// Draw quads (consecutive quads of one texture in one call).
    pub fn draw(&mut self, quads: &[Quad]) {
        let mut i = 0;
        while i < quads.len() {
            let tex = quads[i].texture;
            let mut j = i;
            self.out.clear();
            while j < quads.len() && quads[j].texture == tex {
                let q = &quads[j];
                self.out.push(ffi::UiQuad {
                    x: q.x,
                    y: q.y,
                    w: q.w,
                    h: q.h,
                    s0: q.uv[0],
                    t0: q.uv[1],
                    s1: q.uv[2],
                    t1: q.uv[3],
                    rgba: q.rgba,
                });
                j += 1;
            }
            let handle = if tex == NONE {
                -1
            } else {
                self.handles.get(usize::from(tex)).copied().unwrap_or(-1)
            };
            unsafe { ffi::mwf_ui_quads(handle, self.out.as_ptr(), self.out.len() as u32) };
            i = j;
        }
    }

    /// Lay quads out with `f` (from the pack) and draw them.
    pub fn page(&mut self, f: impl FnOnce(&UiPack, &mut Vec<Quad>)) {
        let mut q = core::mem::take(&mut self.quads);
        q.clear();
        f(&self.pack, &mut q);
        self.draw(&q);
        self.quads = q;
    }
}

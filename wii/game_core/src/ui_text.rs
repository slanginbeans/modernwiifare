//! MW2 text on the Wii: glyph quads from a converted font (`wpk::ui::Font`), placed the way IW4 places menu text
//! (IW4L `hud_iw4::font`: the text scale times 48 virtual pixels is the em, `^N` colour codes are skipped).

use alloc::vec::Vec;

use wpk::ui::{Font, Label, Quad};

/// IW4 `R_TEXT_EM`: a text scale of 1 is 48 virtual pixels high.
pub const TEXT_EM: f32 = 48.0;

/// Font pixels to screen pixels at MW2 text scale `text_scale` (IW4L `normalized_text_scale`).
pub fn glyph_scale(font: &Font, text_scale: f32) -> f32 {
    text_scale * TEXT_EM / f32::from(font.pixel_height.max(1))
}

/// The letters drawn for `text` (IW4L `next_letter`: `^0`..`^9` colour codes are not drawn).
fn letters(text: &str) -> impl Iterator<Item = u16> + '_ {
    let mut chars = text.chars().peekable();
    core::iter::from_fn(move || {
        loop {
            let c = chars.next()?;
            if c == '^' && chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
                continue;
            }
            return Some(c as u32 as u16);
        }
    })
}

/// Width of `text` in screen pixels.
pub fn width(font: &Font, text: &str, text_scale: f32) -> f32 {
    let s = glyph_scale(font, text_scale);
    letters(text)
        .map(|l| font.glyph(l).map_or(0.0, |g| f32::from(g.dx) * s))
        .sum()
}

/// Where IW4 puts a menu item's text: the pen x and baseline y (IW4L `hud_iw4::font::item_text_origin`).
pub fn label_origin(font: &Font, l: &Label, text: &str) -> (f32, f32) {
    let w = width(font, text, l.text_scale);
    hud_iw4::font::item_text_origin(
        l.x,
        l.y,
        l.w,
        l.h,
        i32::from(l.align),
        l.text_x,
        l.text_y,
        w,
        l.text_scale * TEXT_EM,
    )
}

/// Glyph quads for `text` with its pen starting at `x` on baseline `y`.
pub fn quads(
    font: &Font,
    text: &str,
    x: f32,
    y: f32,
    text_scale: f32,
    rgba: u32,
    out: &mut Vec<Quad>,
) {
    let s = glyph_scale(font, text_scale);
    let mut pen = x;
    for l in letters(text) {
        let Some(g) = font.glyph(l) else {
            continue;
        };
        if g.w > 0 && g.h > 0 {
            out.push(Quad {
                x: pen + f32::from(g.x0) * s,
                y: y + f32::from(g.y0) * s,
                w: f32::from(g.w) * s,
                h: f32::from(g.h) * s,
                uv: [g.s0, g.t0, g.s1, g.t1],
                rgba,
                texture: font.texture,
            });
        }
        pen += f32::from(g.dx) * s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::ui::Glyph;

    fn font() -> Font {
        let g = |letter: u16, dx: u8| Glyph {
            letter,
            x0: 1,
            y0: -20,
            dx,
            w: 10,
            h: 20,
            s0: 0.0,
            t0: 0.0,
            s1: 0.5,
            t1: 0.5,
        };
        Font {
            name: "f".into(),
            pixel_height: 24,
            texture: 7,
            glyphs: vec![g(32, 6), g(65, 12), g(66, 14)],
        }
    }

    #[test]
    fn lays_text_out_on_its_baseline() {
        let f = font();
        // Text scale 0.5: 24 virtual pixels high, so font pixels map 1:1.
        assert_eq!(glyph_scale(&f, 0.5), 1.0);
        assert_eq!(width(&f, "AB", 0.5), 26.0);
        // Colour codes are skipped; unknown letters draw nothing and don't advance.
        assert_eq!(width(&f, "^3A?B", 0.5), 26.0);
        let mut q = Vec::new();
        quads(&f, "A B", 100.0, 200.0, 0.5, 0xffffffff, &mut q);
        assert_eq!(q.len(), 3);
        assert_eq!((q[0].x, q[0].y, q[0].w, q[0].h), (101.0, 180.0, 10.0, 20.0));
        assert_eq!(q[2].x, 101.0 + 12.0 + 6.0);
        assert_eq!(q[0].texture, 7);
    }
}

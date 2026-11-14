//! `mwf-convert menu-preview <common.wpk>`: a software render of the baked main menu (its images, the button list
//! in MW2's font with the selection bar), laid out as the Wii's front end does (`wii/game/src/front.rs`
//! `draw_mw2`), to check the menus without a Wii. Output: a 640 × 480 PNG.

use gx_texture::image::Image;
use wpk::records::{self, TextureRecord};
use wpk::ui::{NONE, Quad, UiPack};

/// The page title's left edge: inside MW2's 0.85 safe area (640 × 0.075).
const TITLE_X: f32 = 48.0;

/// Draw `q` over `img` (nearest texel, texture × colour, alpha blended).
fn draw(img: &mut Image, textures: &[Option<Image>], q: &Quad) {
    let Some(Some(t)) = textures.get(usize::from(q.texture)) else {
        return;
    };
    let c = [
        (q.rgba >> 24) as f32 / 255.0,
        ((q.rgba >> 16) & 0xff) as f32 / 255.0,
        ((q.rgba >> 8) & 0xff) as f32 / 255.0,
        (q.rgba & 0xff) as f32 / 255.0,
    ];
    let (x0, y0) = (q.x.max(0.0) as u32, q.y.max(0.0) as u32);
    let (x1, y1) = (
        (q.x + q.w).clamp(0.0, 640.0) as u32,
        (q.y + q.h).clamp(0.0, 480.0) as u32,
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let fx = (x as f32 + 0.5 - q.x) / q.w;
            let fy = (y as f32 + 0.5 - q.y) / q.h;
            let u = q.uv[0] + (q.uv[2] - q.uv[0]) * fx;
            let v = q.uv[1] + (q.uv[3] - q.uv[1]) * fy;
            let tx = ((u.clamp(0.0, 0.9999)) * t.width as f32) as u32;
            let ty = ((v.clamp(0.0, 0.9999)) * t.height as f32) as u32;
            let s = t.pixels[(ty * t.width + tx) as usize];
            let a = s[3] as f32 / 255.0 * c[3];
            let d = &mut img.pixels[(y * 640 + x) as usize];
            for k in 0..3 {
                let src = s[k] as f32 * c[k];
                d[k] = (src * a + d[k] as f32 * (1.0 - a)).clamp(0.0, 255.0) as u8;
            }
            d[3] = 255;
        }
    }
}

pub fn render(common: &[u8]) -> Result<Image, String> {
    let pack = wpk::Pack::parse(common, true).map_err(|e| format!("{e:?}"))?;
    let ui = pack
        .section(wpk::tag::UI)
        .and_then(|s| UiPack::decode(s.data))
        .ok_or("no menu data in this pack (convert again)")?;
    let txda = pack
        .section(wpk::tag::TEXTURE_DATA)
        .map_or(&[][..], |s| s.data);
    let textures: Vec<Option<Image>> = pack
        .section(wpk::tag::TEXTURES)
        .map(|s| {
            records::iter::<TextureRecord>(s.data)
                .map(|r| crate::preview::decode_texture(&r, txda))
                .collect()
        })
        .unwrap_or_default();
    let mut img = Image::new(640, 480, [26, 30, 36, 255]);
    let screen = ui.screen("main").ok_or("no main screen")?;
    for q in &screen.quads {
        draw(&mut img, &textures, q);
    }
    let Some(b) = screen.buttons else {
        return Ok(img);
    };
    let mut quads = Vec::new();
    let text =
        |font: usize, s: &str, x: f32, y: f32, scale: f32, rgba: u32, quads: &mut Vec<Quad>| {
            if let Some(f) = ui.fonts.get(font) {
                game_core::ui_text::quads(f, s, x, y, scale, rgba, quads);
            }
        };
    let big = ui.font("fonts/bigfont").unwrap_or(0);
    let font = usize::from(b.font);
    text(
        big,
        "MODERN WIIFARE",
        TITLE_X,
        b.y - 14.0,
        0.5,
        0xffffffff,
        &mut quads,
    );
    let text_h = b.text_scale * game_core::ui_text::TEXT_EM;
    for (row, line) in ["PLAY", "CREATE A CLASS", "QUIT"].iter().enumerate() {
        let y = b.y + row as f32 * b.step;
        if row == 0 && b.bar != NONE {
            quads.push(Quad {
                x: b.x,
                y,
                w: b.w,
                h: b.h,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: b.bar_rgba,
                texture: b.bar,
            });
        }
        let w = ui
            .fonts
            .get(font)
            .map_or(0.0, |f| game_core::ui_text::width(f, line, b.text_scale));
        let x = match b.align {
            1 => b.x + (b.w - w) * 0.5 + b.text_x,
            2 => b.x + b.w - w + b.text_x,
            _ => b.x + b.text_x,
        };
        let colour = if row == 0 { b.focus_rgba } else { b.rgba };
        text(font, line, x, y + text_h, b.text_scale, colour, &mut quads);
    }
    for q in &quads {
        draw(&mut img, &textures, q);
    }
    Ok(img)
}

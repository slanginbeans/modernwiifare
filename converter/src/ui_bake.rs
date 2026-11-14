//! Bake MW2's menus for the Wii (`wpk::ui`): the fonts (glyph tables over their atlas textures) and screens as
//! textured quads already placed on the 640 × 480 screen, plus each screen's button style.
//!
//! Placement uses IW4L's `hud_iw4::ScreenPlacement` (4:3, the default 0.85 safe area), font choice IW4L's
//! `ui_get_font_handle`. The menus' scripts and expressions don't run on the Wii: a baked screen keeps the items
//! that are always shown, plus those whose visibility only tests for the multiplayer build (`"mp"`); buttons take
//! their look from the menu's first button and drive our own menu flow.

use wpk::ui::{ButtonStyle, Font, Glyph, NONE, Quad, Screen, UiPack};

use crate::ui_capture::{Item, Menu, Rect, UiCapture};

/// The fonts the menus use (`hud_iw4::ui_get_font_handle` names; the developer and console fonts are left out).
pub const FONTS: [&str; 8] = [
    "fonts/smallfont",
    "fonts/normalfont",
    "fonts/boldfont",
    "fonts/bigfont",
    "fonts/extrabigfont",
    "fonts/objectivefont",
    "fonts/hudbigfont",
    "fonts/hudsmallfont",
];

/// Screens baked from MW2 menus: (our name, MW2 menu).
pub const SCREENS: [(&str, &str); 1] = [("main", "main_text")];

/// IW4 window style "shader": the item draws its background material.
const WINDOW_STYLE_SHADER: i32 = 3;
/// IW4 item type "button".
const ITEM_TYPE_BUTTON: i32 = 1;

#[derive(Debug, Default)]
pub struct BakeReport {
    pub fonts: usize,
    pub glyphs: usize,
    pub screens: usize,
    pub quads: usize,
    pub missing_textures: usize,
}

fn rgba(c: [f32; 4]) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (b(c[0]) << 24) | (b(c[1]) << 16) | (b(c[2]) << 8) | b(c[3])
}

fn place(p: &hud_iw4::ScreenPlacement, r: &Rect) -> hud_iw4::AppliedRect {
    p.apply_rect(
        r.x,
        r.y,
        r.w,
        r.h,
        i32::from(r.horz_align),
        i32::from(r.vert_align),
    )
}

/// Whether an item shows in the multiplayer build without running its scripts: no visibility test, or one that only
/// compares against the `"mp"` string (IW4L's expression dumps write strings as `s:<hex>`).
fn shown(item: &Item) -> bool {
    match &item.visible {
        None => true,
        Some(v) => v.split_whitespace().any(|t| t == "s:6d70"),
    }
}

/// The material name an item draws (menus prefix shared materials with ',').
fn material(name: &str) -> &str {
    name.trim_start_matches(',')
}

fn bake_screen(
    name: &str,
    menu: &Menu,
    p: &hud_iw4::ScreenPlacement,
    fonts: &[Font],
    tex: &mut impl FnMut(&str) -> Option<u16>,
    report: &mut BakeReport,
) -> Screen {
    let mut quads = Vec::new();
    for item in &menu.items {
        if item.style != WINDOW_STYLE_SHADER
            || item.background.is_empty()
            || !shown(item)
            || item.fore_color[3] <= 0.0
        {
            continue;
        }
        let Some(texture) = tex(material(&item.background)) else {
            report.missing_textures += 1;
            continue;
        };
        let r = place(p, &item.rect);
        // Negative sizes mirror the image (MW2 tiles its cloud layer this way).
        let (x, w, u0, u1) = if r.w < 0.0 {
            (r.x + r.w, -r.w, 1.0, 0.0)
        } else {
            (r.x, r.w, 0.0, 1.0)
        };
        let (y, h, v0, v1) = if r.h < 0.0 {
            (r.y + r.h, -r.h, 1.0, 0.0)
        } else {
            (r.y, r.h, 0.0, 1.0)
        };
        quads.push(Quad {
            x,
            y,
            w,
            h,
            uv: [u0, v0, u1, v1],
            rgba: rgba(item.fore_color),
            texture,
        });
    }
    report.quads += quads.len();
    // The button list: the first always-shown button, and the gap to the next one.
    let buttons: Vec<&Item> = menu
        .items
        .iter()
        .filter(|i| i.item_type == ITEM_TYPE_BUTTON && shown(i))
        .collect();
    let style = buttons.first().map(|b| {
        let r = place(p, &b.rect);
        let step = buttons
            .get(1)
            .map_or(r.h, |n| place(p, &n.rect).y - r.y)
            .max(1.0);
        let font_name =
            hud_iw4::font::ui_get_font_handle(b.font, p.scale_virtual_to_real[1], b.text_scale);
        let font = fonts
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(font_name))
            .unwrap_or(0) as u8;
        let bar = if b.background.is_empty() {
            NONE
        } else {
            tex(material(&b.background)).unwrap_or(NONE)
        };
        ButtonStyle {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            step,
            font,
            text_scale: b.text_scale,
            text_x: b.text_align_x * p.scale_virtual_to_real[0],
            align: (b.text_align_mode & 3) as u8,
            rgba: rgba(b.fore_color),
            focus_rgba: rgba(menu.focus_color),
            bar,
            bar_rgba: 0xffff_ffff,
        }
    });
    Screen {
        name: name.to_owned(),
        quads,
        buttons: style,
    }
}

/// Bake the fonts and screens. `tex(material)` adds that material's colour texture to the pack (`TEXT` index).
pub fn bake(ui: &UiCapture, mut tex: impl FnMut(&str) -> Option<u16>) -> (UiPack, BakeReport) {
    let mut report = BakeReport::default();
    let p = hud_iw4::ScreenPlacement::setup_fullscreen(640.0, 480.0, 1.0);
    let mut fonts = Vec::new();
    for name in FONTS {
        let Some(f) = ui.fonts.iter().find(|f| f.name.eq_ignore_ascii_case(name)) else {
            continue;
        };
        let Some(texture) = tex(&f.material) else {
            report.missing_textures += 1;
            continue;
        };
        let mut glyphs: Vec<Glyph> = f
            .glyphs
            .iter()
            .map(|g| Glyph {
                letter: g.letter,
                x0: g.x0,
                y0: g.y0,
                dx: g.dx,
                w: g.pixel_width,
                h: g.pixel_height,
                s0: g.s0,
                t0: g.t0,
                s1: g.s1,
                t1: g.t1,
            })
            .collect();
        glyphs.sort_by_key(|g| g.letter);
        glyphs.dedup_by_key(|g| g.letter);
        report.glyphs += glyphs.len();
        fonts.push(Font {
            name: name.to_owned(),
            pixel_height: f.pixel_height.clamp(1, 255) as u16,
            texture,
            glyphs,
        });
    }
    report.fonts = fonts.len();
    let mut screens = Vec::new();
    for (ours, theirs) in SCREENS {
        if let Some(menu) = ui.menus.get(theirs) {
            screens.push(bake_screen(ours, menu, &p, &fonts, &mut tex, &mut report));
        }
    }
    report.screens = screens.len();
    (UiPack { fonts, screens }, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_capture::Rect;

    fn item(
        name: &str,
        style: i32,
        item_type: i32,
        rect: Rect,
        bg: &str,
        visible: Option<&str>,
    ) -> Item {
        Item {
            name: name.into(),
            text: String::new(),
            item_type,
            style,
            owner_draw: 0,
            rect,
            fore_color: [1.0; 4],
            back_color: [0.0; 4],
            text_scale: 0.375,
            font: 3,
            text_align_mode: 6,
            text_align_x: -60.0,
            text_align_y: 0.0,
            text_style: 0,
            background: bg.into(),
            dvar: String::new(),
            static_flags: 0,
            visible: visible.map(str::to_owned),
            scripts: Vec::new(),
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32, ha: u8, va: u8) -> Rect {
        Rect {
            x,
            y,
            w,
            h,
            horz_align: ha,
            vert_align: va,
        }
    }

    #[test]
    fn bakes_shown_images_and_the_button_style() {
        // A made-up menu in MW2's layout conventions.
        let menu = Menu {
            name: "m".into(),
            window_background: String::new(),
            fullscreen: 1,
            rect: rect(0.0, 0.0, 640.0, 480.0, 0, 0),
            focus_color: [1.0, 0.8, 0.2, 1.0],
            items: vec![
                item(
                    "",
                    3,
                    0,
                    rect(0.0, 0.0, 640.0, 480.0, 4, 4),
                    "bg_full",
                    None,
                ),
                item(
                    "",
                    3,
                    0,
                    rect(0.0, 0.0, -100.0, -50.0, 0, 0),
                    "mirror",
                    None,
                ),
                item(
                    "",
                    3,
                    0,
                    rect(10.0, 10.0, 20.0, 20.0, 0, 0),
                    "sp_only",
                    Some("op 16 op 12 s:7370"),
                ),
                item(
                    "",
                    3,
                    0,
                    rect(10.0, 10.0, 20.0, 20.0, 0, 0),
                    "mp_only",
                    Some("op 16 op 12 s:6d70"),
                ),
                item(
                    "a",
                    1,
                    1,
                    rect(-64.0, 48.0, 336.0, 20.0, 1, 1),
                    ",bar",
                    None,
                ),
                item(
                    "b",
                    1,
                    1,
                    rect(-64.0, 68.0, 336.0, 20.0, 1, 1),
                    ",bar",
                    None,
                ),
            ],
            scripts: Vec::new(),
        };
        let mut ui = UiCapture::default();
        ui.menus.insert("main_text".into(), menu);
        let mut names = Vec::new();
        let (pack, r) = bake(&ui, |m| {
            names.push(m.to_owned());
            Some(names.len() as u16 - 1)
        });
        let s = &pack.screens[0];
        assert_eq!(s.quads.len(), 3, "full, mirrored and the mp-only image");
        assert!(names.iter().any(|n| n == "mp_only") && !names.iter().any(|n| n == "sp_only"));
        assert_eq!((s.quads[0].x, s.quads[0].w), (0.0, 640.0));
        assert_eq!(s.quads[1].uv, [1.0, 1.0, 0.0, 0.0]);
        let b = s.buttons.expect("button style");
        assert_eq!(b.step, 20.0);
        assert!(names.iter().any(|n| n == "bar"));
        assert_eq!(b.align, 2);
        assert_eq!(r.missing_textures, 0);
    }
}

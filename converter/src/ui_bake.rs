//! Bake MW2's menus for the Wii (`wpk::ui`): the fonts (glyph tables over their atlas textures) and screens as
//! textured quads already placed on the 640 × 480 screen, plus each screen's button style.
//!
//! Placement uses IW4L's `hud_iw4::ScreenPlacement` (4:3, the default 0.85 safe area), font choice IW4L's
//! `ui_get_font_handle`. The menus' scripts and expressions don't run on the Wii: a baked screen keeps the items
//! that are always shown, plus those whose visibility only tests for the multiplayer build (`"mp"`); buttons take
//! their look from the menu's first button and drive our own menu flow.

use std::collections::HashMap;

use wpk::ui::{ButtonStyle, Font, Glyph, Label, NONE, Quad, Screen, UiPack, label_kind};

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

/// Screens baked from MW2 menus: (our name, MW2 menu, x offset in virtual pixels). The scoreboard page is drawn
/// inside MW2's centred 544-wide scoreboard frame, so its centre-aligned items move left by half of that.
pub const SCREENS: [(&str, &str, f32); 5] = [
    ("main", "main_text", 0.0),
    ("class_select", "changeclass", 0.0),
    ("pause", "class", 0.0),
    ("cac", "menu_cac_assault", 0.0),
    ("scoreboard", "page_scoreboard", -272.0),
];

/// Items wider than this (virtual pixels) are layers the menu's scripts scroll across the screen (MW2's cloud
/// overlays); still, they only wash the screen out, and they are the costliest quads to fill.
const SCROLLING_LAYER_W: f32 = 960.0;

/// IW4 window styles: "empty" (text only), "filled" (back colour over the background material, or plain), "shader"
/// (the background material in the fore colour).
const WINDOW_STYLE_EMPTY: i32 = 0;
const WINDOW_STYLE_FILLED: i32 = 1;
const WINDOW_STYLE_SHADER: i32 = 3;
/// The back button's text (its rect is kept; the front end draws its own hint there).
const BACK_TEXT: &str = "@PLATFORM_BACK_CAPS";
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
        Some(v) => {
            v.split_whitespace().any(|t| t == "s:6d70")
                // A constant true test, `( 1 )`, dumps as "op 16 1".
                || v.split_whitespace().filter(|&t| t != "op" && t != "16").eq(["1"])
        }
    }
}

/// The material name an item draws (menus prefix shared materials with ',').
fn material(name: &str) -> &str {
    name.trim_start_matches(',')
}

fn font_index(p: &hud_iw4::ScreenPlacement, fonts: &[Font], font: i32, scale: f32) -> u8 {
    let name = hud_iw4::font::ui_get_font_handle(font, p.scale_virtual_to_real[1], scale);
    fonts
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case(name))
        .unwrap_or(0) as u8
}

/// A menu string: `@KEY` through the localize table (the key itself when the table lacks it), else as written.
fn text_of(text: &str, localize: &HashMap<String, String>) -> String {
    match text.strip_prefix('@') {
        Some(key) => localize.get(key).cloned().unwrap_or_else(|| key.to_owned()),
        None => text.to_owned(),
    }
}

fn label_of(p: &hud_iw4::ScreenPlacement, fonts: &[Font], item: &Item, text: String) -> Label {
    let r = place(p, &item.rect);
    Label {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        font: font_index(p, fonts, item.font, item.text_scale),
        text_scale: item.text_scale,
        align: (item.text_align_mode & 0xf) as u8,
        text_x: item.text_align_x * p.scale_virtual_to_real[0],
        text_y: item.text_align_y * p.scale_virtual_to_real[1],
        rgba: rgba(item.fore_color),
        text,
    }
}

fn bake_screen(
    name: &str,
    menu: &Menu,
    p: &hud_iw4::ScreenPlacement,
    fonts: &[Font],
    localize: &HashMap<String, String>,
    tex: &mut impl FnMut(&str) -> Option<u16>,
    report: &mut BakeReport,
) -> Screen {
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    for item in &menu.items {
        if !shown(item)
            || item.item_type == ITEM_TYPE_BUTTON
            || item.rect.w.abs() > SCROLLING_LAYER_W
        {
            continue;
        }
        // Text: MW2's own, or an empty slot for live text (a text-only item, or a filled one with a font).
        if (item.style == WINDOW_STYLE_EMPTY || item.style == WINDOW_STYLE_FILLED)
            && item.fore_color[3] > 0.0
            && (item.style == WINDOW_STYLE_EMPTY || item.font != 0 || !item.text.is_empty())
        {
            let kind = if item.text.is_empty() {
                label_kind::SLOT
            } else if !labels.iter().any(|(k, _)| *k == label_kind::TITLE) {
                label_kind::TITLE
            } else {
                label_kind::TEXT
            };
            labels.push((
                kind,
                label_of(p, fonts, item, text_of(&item.text, localize)),
            ));
        }
        let (material_name, colour) = match item.style {
            WINDOW_STYLE_SHADER if !item.background.is_empty() => {
                (material(&item.background), item.fore_color)
            }
            WINDOW_STYLE_FILLED if item.back_color[3] > 0.0 => {
                let m = if item.background.is_empty() {
                    "white"
                } else {
                    material(&item.background)
                };
                (m, item.back_color)
            }
            _ => continue,
        };
        if colour[3] <= 0.0 {
            continue;
        }
        let Some(texture) = tex(material_name) else {
            report.missing_textures += 1;
            continue;
        };
        let r = place(p, &item.rect);
        // Negative sizes mirror the image (MW2 tiles its cloud layer this way).
        // As IW4's `UI_DrawHandlePic`: the rect keeps its corner, only the texture flips.
        let (x, w, u0, u1) = if r.w < 0.0 {
            (r.x, -r.w, 1.0, 0.0)
        } else {
            (r.x, r.w, 0.0, 1.0)
        };
        let (y, h, v0, v1) = if r.h < 0.0 {
            (r.y, -r.h, 1.0, 0.0)
        } else {
            (r.y, r.h, 0.0, 1.0)
        };
        quads.push(Quad {
            x,
            y,
            w,
            h,
            uv: [u0, v0, u1, v1],
            rgba: rgba(colour),
            texture,
        });
    }
    report.quads += quads.len();
    let back = menu
        .items
        .iter()
        .find(|i| i.item_type == ITEM_TYPE_BUTTON && i.text == BACK_TEXT)
        .map(|i| {
            let r = place(p, &i.rect);
            [r.x, r.y, r.w, r.h]
        });
    // The button list: the first always-shown button, and the gap to the next one.
    let buttons: Vec<&Item> = menu
        .items
        .iter()
        .filter(|i| i.item_type == ITEM_TYPE_BUTTON && shown(i) && i.text != BACK_TEXT)
        .collect();
    let style = buttons.first().map(|b| {
        let r = place(p, &b.rect);
        let step = buttons
            .get(1)
            .map_or(r.h, |n| place(p, &n.rect).y - r.y)
            .max(1.0);
        let font = font_index(p, fonts, b.font, b.text_scale);
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
        labels,
        back,
    }
}

/// Bake the fonts and screens. `tex(material)` adds that material's colour texture to the pack (`TEXT` index);
/// `localize` gives MW2's strings for the menus' `@KEY` text.
pub fn bake(
    ui: &UiCapture,
    localize: &HashMap<String, String>,
    mut tex: impl FnMut(&str) -> Option<u16>,
) -> (UiPack, BakeReport) {
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
    for (ours, theirs, dx) in SCREENS {
        if let Some(menu) = ui.menus.get(theirs) {
            let mut menu = menu.clone();
            for item in &mut menu.items {
                item.rect.x += dx;
            }
            let menu = &menu;
            screens.push(bake_screen(
                ours,
                menu,
                &p,
                &fonts,
                localize,
                &mut tex,
                &mut report,
            ));
        }
    }
    report.screens = screens.len();
    let white = tex("white").unwrap_or(NONE);
    (
        UiPack {
            fonts,
            screens,
            white,
        },
        report,
    )
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
        let (pack, r) = bake(&ui, &HashMap::new(), |m| {
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

    #[test]
    fn bakes_labels_slots_filled_rects_and_the_back_button() {
        let mut title = item("", 0, 0, rect(0.0, 0.0, 272.0, 28.0, 1, 1), "", None);
        title.text = "@TITLE_KEY".into();
        let slot = item("", 0, 0, rect(-272.0, 48.0, 272.0, 18.0, 3, 1), "", None);
        let mut filled = item(
            "",
            1,
            0,
            rect(-272.0, 28.0, -272.0, 20.0, 3, 1),
            ",fade",
            None,
        );
        filled.back_color = [1.0, 1.0, 1.0, 0.25];
        let hidden = item(
            "",
            0,
            0,
            rect(0.0, 0.0, 10.0, 10.0, 0, 0),
            "",
            Some("op 16 op 81 s:61"),
        );
        let mut back = item(
            "",
            1,
            1,
            rect(-64.0, -20.0, 336.0, 20.0, 1, 3),
            ",bar",
            Some("op 16 s:40"),
        );
        back.text = BACK_TEXT.into();
        let button = item(
            "a",
            1,
            1,
            rect(-64.0, 28.0, 336.0, 20.0, 1, 1),
            ",bar",
            None,
        );
        let cloud = item("", 3, 0, rect(0.0, 0.0, 1708.0, 480.0, 0, 0), "cloud", None);
        let menu = Menu {
            name: "m".into(),
            window_background: String::new(),
            fullscreen: 1,
            rect: rect(0.0, 0.0, 640.0, 480.0, 0, 0),
            focus_color: [1.0; 4],
            items: vec![title, slot, filled, hidden, back, button, cloud],
            scripts: Vec::new(),
        };
        let mut ui = UiCapture::default();
        ui.menus.insert("changeclass".into(), menu);
        let localize = HashMap::from([("TITLE_KEY".to_owned(), "CHOOSE".to_owned())]);
        let mut names = Vec::new();
        let (pack, _) = bake(&ui, &localize, |m| {
            names.push(m.to_owned());
            Some(names.len() as u16 - 1)
        });
        let s = pack.screen("class_select").expect("baked");
        assert_eq!(s.title().map(|l| l.text.as_str()), Some("CHOOSE"));
        // The filled header has a font, so it is a slot too.
        assert!(s.slot(0).is_some() && s.slot(1).is_some() && s.slot(2).is_none());
        assert_eq!(
            s.quads.len(),
            1,
            "the filled header; the scrolling layer is left out"
        );
        assert_eq!(s.quads[0].rgba & 0xff, 64);
        assert!(s.back.is_some());
        let b = s.buttons.expect("buttons");
        assert!(b.y < s.back.unwrap()[1], "the back button isn't the list");
        assert_ne!(pack.white, NONE);
        assert!(!names.iter().any(|n| n == "cloud"));
    }
}

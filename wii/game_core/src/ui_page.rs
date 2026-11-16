//! A menu page laid out on one of MW2's baked screens (`wpk::ui::Screen`): its images, its labels (the title gets
//! ours, empty slots get live text, the rest is MW2's own text), a button list in its button style with the selection
//! bar, and the footer hint where MW2 has its back button. Output is textured quads; the Wii draws them
//! (`wii/game/src/ui.rs`) and the converter's `menu-preview` renders them on the PC.

use alloc::string::String;
use alloc::vec::Vec;

use wpk::ui::{ButtonStyle, Label, NONE, Quad, UiPack, label_kind};

use crate::menu;
use crate::ui_text::{self, TEXT_EM};

/// One line of a page's list.
#[derive(Clone, Copy, Debug)]
pub struct PageLine<'a> {
    pub text: &'a str,
    /// Shown right of the button on screens without a class panel.
    pub value: Option<&'a str>,
    pub enabled: bool,
}

/// What to lay out.
pub struct Page<'a> {
    pub screen: &'a str,
    pub title: &'a str,
    pub lines: &'a [PageLine<'a>],
    pub selected: usize,
    pub footer: &'a str,
    /// Live text for the screen's empty slots, in menu order.
    pub slots: &'a [String],
    /// The items the screen's images show (`wpk::ui::image_field` order; empty: none).
    pub images: &'a [String],
    /// Values drawn inside their line, right-aligned (MW2's rules and settings rows), not beside it.
    pub inline_values: bool,
    /// The screen's full-screen background (false over the frozen game).
    pub backdrop: bool,
    /// The map whose preview picture the screen shows (`mp_rust`; empty: none).
    pub preview: &'a str,
    /// The lines ticked, with a check box at each line's left (MW2's killstreak rows); empty: no boxes.
    pub checks: &'a [bool],
}

/// The killstreak rows' check box (MW2's `ks_button_check_*`), and the bar behind a ticked row.
const CHECK_SIZE: f32 = 20.0;
const CHECK_BAR_RGBA: u32 = 0xcc99_1aff;

/// MW2's main menu has no page name: ours goes above the first button, inside the 0.85 safe area (640 Ã— 0.075).
const TITLE_X: f32 = 48.0;

/// Inside a rules row: the value ends this far left of the row's right edge.
const VALUE_INSET: f32 = 8.0;
/// Slot text longer than this many bytes is a description and wraps.
const WRAP_FROM: usize = 32;

/// A quad's texture: the frame the Wii kept to show behind the in-game menus (not a pack texture).
pub const GRAB: u16 = 0xFFFE;

/// The whole screen as a quad of texture `t` in colour `rgba`.
pub fn fullscreen(t: u16, rgba: u32) -> Quad {
    Quad {
        x: 0.0,
        y: 0.0,
        w: 640.0,
        h: 480.0,
        uv: [0.0, 0.0, 1.0, 1.0],
        rgba,
        texture: t,
    }
}

/// `text` in `font` with its pen at `x` on baseline `y`.
#[allow(clippy::too_many_arguments)]
fn text(
    ui: &UiPack,
    font: usize,
    s: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: u32,
    out: &mut Vec<Quad>,
) {
    if let Some(f) = ui.fonts.get(font) {
        ui_text::quads(f, s, x, y, scale, rgba, out);
    }
}

/// `s` where IW4 puts a menu item's text.
pub fn label(ui: &UiPack, l: &Label, s: &str, out: &mut Vec<Quad>) {
    let font = usize::from(l.font);
    if let Some(f) = ui.fonts.get(font) {
        let (x, y) = ui_text::label_origin(f, l, s);
        ui_text::quads(f, s, x, y, l.text_scale, l.rgba, out);
    }
}

/// `s` in a label's rect, broken into lines that fit its width (MW2's wrapped description items); lines go down
/// from where the first would be.
pub fn wrapped_label(ui: &UiPack, l: &Label, s: &str, out: &mut Vec<Quad>) {
    let Some(f) = ui.fonts.get(usize::from(l.font)) else {
        return;
    };
    let width = (l.w - l.text_x.abs()).max(16.0);
    let step = l.text_scale * TEXT_EM * 1.15;
    for (n, line) in ui_text::wrap(f, s, l.text_scale, width).iter().enumerate() {
        let (x, y) = ui_text::label_origin(f, l, line);
        ui_text::quads(f, line, x, y + n as f32 * step, l.text_scale, l.rgba, out);
    }
}

/// A button's text x inside a rect (IW4 `item_text_origin`: aligned in the rect, then `text_x` along).
fn button_x(ui: &UiPack, b: &ButtonStyle, s: &str, x: f32, w: f32) -> f32 {
    let tw = ui
        .fonts
        .get(usize::from(b.font))
        .map_or(0.0, |f| ui_text::width(f, s, b.text_scale));
    match b.align {
        1 => x + (w - tw) * 0.5 + b.text_x,
        2 => x + w - tw + b.text_x,
        _ => x + b.text_x,
    }
}

/// Below a popup's last row, its frame goes on this far (MW2's Game Setup popup).
const POPUP_PAD: f32 = 2.0;

/// How many rows of the list fit above the back button (or the bottom of the safe area).
fn list_rows(screen: &wpk::ui::Screen, b: &ButtonStyle) -> usize {
    let bottom = screen.back.map_or(440.0, |r| r[1] - b.h * 0.5);
    (((bottom - b.y) / b.step) as usize).max(1)
}

/// The rows a popup shows for `lines` lines.
fn popup_rows(screen: &wpk::ui::Screen, b: &ButtonStyle, lines: usize) -> usize {
    lines.clamp(1, list_rows(screen, b))
}

/// Lay `page` out (falling back to the main menu's look when its screen isn't in the pack).
pub fn layout(ui: &UiPack, page: &Page<'_>, out: &mut Vec<Quad>) {
    let Some(screen) = ui.screen(page.screen).or_else(|| ui.screen("main")) else {
        return;
    };
    // The map's preview picture where MW2 shows it, at its place in MW2's item order (`od::MAP_PREVIEW`).
    let preview_tex = (!page.preview.is_empty())
        .then(|| ui.icon(&wpk::ui::map_preview_key(page.preview)))
        .flatten();
    let previews: Vec<(usize, Quad)> = preview_tex
        .map(|t| {
            screen
                .owner_draws
                .iter()
                .filter(|o| o.od == wpk::ui::od::MAP_PREVIEW)
                .map(|o| {
                    (
                        usize::from(o.texture),
                        Quad {
                            x: o.x,
                            y: o.y,
                            w: o.w,
                            h: o.h,
                            uv: [0.0, 0.0, 1.0, 1.0],
                            rgba: o.rgba,
                            texture: t,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // A popup's frame, sized to the rows it shows as MW2's scripts do (never smaller than it was baked).
    let frame = screen
        .owner_draws
        .iter()
        .find(|o| o.od == wpk::ui::od::POPUP_FRAME)
        .zip(screen.buttons)
        .map(|(o, b)| {
            let rows = popup_rows(screen, &b, page.lines.len());
            let list = b.y + rows as f32 * b.step - o.y + POPUP_PAD;
            (o.y, o.h, o.h.max(list))
        });
    for (n, q) in screen.quads.iter().enumerate() {
        out.extend(previews.iter().filter(|(at, _)| *at == n).map(|(_, p)| *p));
        if let Some((top, baked, h)) = frame
            && (q.y - top).abs() < 0.5
            && (q.h < 0.5 || (q.h - baked).abs() < 0.5)
        {
            out.push(Quad { h, ..*q });
            continue;
        }
        // Over the frozen game: the screen's own opaque full-screen background becomes a dark veil (its translucent
        // layers, like the pause menu's dimming, stay).
        let full = q.w.abs() >= 600.0 && q.h.abs() >= 440.0;
        if !page.backdrop && full && q.rgba & 0xff >= 0xc0 {
            if ui.white != NONE {
                out.push(fullscreen(ui.white, 0x0000_00a0));
            }
        } else {
            out.push(*q);
        }
    }
    out.extend(
        previews
            .iter()
            .filter(|(at, _)| *at >= screen.quads.len())
            .map(|(_, p)| *p),
    );
    // The class's pictures and icons in MW2's places.
    for im in &screen.images {
        let Some(item) = page
            .images
            .get(usize::from(im.field))
            .filter(|i| !i.is_empty())
        else {
            continue;
        };
        if let Some(t) = ui.icon(&wpk::ui::image_key(im.field, item)) {
            out.push(Quad {
                x: im.x,
                y: im.y,
                w: im.w,
                h: im.h,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: im.rgba,
                texture: t,
            });
        }
    }
    let mut slot = 0;
    let mut has_title = false;
    for (kind, l) in &screen.labels {
        let s = match *kind {
            label_kind::TITLE => {
                has_title = true;
                page.title
            }
            label_kind::SLOT => {
                slot += 1;
                page.slots.get(slot - 1).map_or("", String::as_str)
            }
            _ => l.text.as_str(),
        };
        if !s.is_empty() {
            // Long live text (a map's or mode's description) wraps in its rect; names stay on their line.
            if *kind == label_kind::SLOT && s.len() > WRAP_FROM {
                wrapped_label(ui, l, s, out);
            } else {
                label(ui, l, s, out);
            }
        }
    }
    let Some(b) = screen.buttons else {
        return;
    };
    let font = usize::from(b.font);
    if !has_title {
        let big = ui.font("fonts/bigfont").unwrap_or(0);
        text(
            ui,
            big,
            page.title,
            TITLE_X,
            b.y - 14.0,
            0.5,
            0xffff_ffff,
            out,
        );
    }
    let rows = list_rows(screen, &b);
    let text_h = b.text_scale * TEXT_EM;
    let small = ui.font("fonts/smallfont").unwrap_or(0);
    let values = page.slots.is_empty();
    for (row, i) in menu::window(page.selected, page.lines.len(), rows).enumerate() {
        let l = &page.lines[i];
        let y = b.y + row as f32 * b.step;
        if let Some(&ticked) = page.checks.get(i) {
            let icon = |name: &str, x: f32, w: f32, rgba: u32, out: &mut Vec<Quad>| {
                if let Some(t) = ui.icon(name) {
                    out.push(Quad {
                        x,
                        y,
                        w,
                        h: CHECK_SIZE,
                        uv: [0.0, 0.0, 1.0, 1.0],
                        rgba,
                        texture: t,
                    });
                }
            };
            if ticked {
                icon("ks_button_selected_bar", b.x, b.w, CHECK_BAR_RGBA, out);
            }
            icon("ks_button_check_bg", b.x, CHECK_SIZE, 0x0000_00ff, out);
            if ticked {
                icon("ks_button_check_fg", b.x, CHECK_SIZE, 0xffff_ffff, out);
            }
        }
        if i == page.selected && b.bar != NONE {
            out.push(Quad {
                x: b.x,
                y,
                w: b.w,
                h: b.h,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: b.bar_rgba,
                texture: b.bar,
            });
        }
        let colour = if i == page.selected {
            b.focus_rgba
        } else if l.enabled {
            b.rgba
        } else {
            (b.rgba & 0xffff_ff00) | 0x60
        };
        let x = button_x(ui, &b, l.text, b.x, b.w);
        text(ui, font, l.text, x, y + text_h, b.text_scale, colour, out);
        match l.value {
            // MW2's rules rows: the value at the right of the row, in the button's own font.
            Some(v) if page.inline_values => {
                let w = ui
                    .fonts
                    .get(font)
                    .map_or(0.0, |f| ui_text::width(f, v, b.text_scale));
                text(
                    ui,
                    font,
                    v,
                    b.x + b.w - VALUE_INSET - w,
                    y + text_h,
                    b.text_scale,
                    colour,
                    out,
                );
            }
            Some(v) if values => {
                text(ui, small, v, b.x + b.w + 12.0, y + text_h, 0.3, colour, out);
            }
            _ => {}
        }
    }
    match screen.back {
        Some(r) => {
            // Inside the title-safe area even when the hint is longer than MW2's own.
            let x = button_x(ui, &b, page.footer, r[0], r[2]).max(TITLE_X);
            text(
                ui,
                font,
                page.footer,
                x,
                r[1] + text_h,
                b.text_scale,
                b.rgba,
                out,
            );
        }
        None => text(
            ui,
            small,
            page.footer,
            TITLE_X,
            456.0,
            0.3,
            0xc0c8_d0ff,
            out,
        ),
    }
}

/// One player of the lobby list: name, what is shown right of it (team, "BOT"), and its text colour.
#[derive(Clone, Debug)]
pub struct LobbyRow {
    pub name: String,
    pub info: String,
    pub rgba: u32,
}

/// The lobby's players in MW2's player rows (the lobby screen's `od::LOBBY_ROW` rects, top down).
pub fn lobby_rows(ui: &UiPack, rows: &[LobbyRow], out: &mut Vec<Quad>) {
    let Some(screen) = ui.screen("lobby") else {
        return;
    };
    let Some(b) = screen.buttons else {
        return;
    };
    let mut rects: Vec<&wpk::ui::OwnerDraw> = screen
        .owner_draws
        .iter()
        .filter(|o| o.od == wpk::ui::od::LOBBY_ROW && o.y >= b.y - 1.0)
        .collect();
    rects.sort_by(|a, c| a.y.total_cmp(&c.y));
    let font = usize::from(b.font);
    let Some(f) = ui.fonts.get(font) else {
        return;
    };
    let scale = b.text_scale;
    for (r, row) in rects.iter().zip(rows) {
        if r.texture != NONE {
            out.push(Quad {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: r.rgba,
                texture: r.texture,
            });
        }
        let base = r.y + (r.h + scale * TEXT_EM * 0.75) * 0.5;
        ui_text::quads(f, &row.name, r.x + 8.0, base, scale, row.rgba, out);
        let w = ui_text::width(f, &row.info, scale);
        ui_text::quads(
            f,
            &row.info,
            r.x + r.w - 8.0 - w,
            base,
            scale,
            row.rgba,
            out,
        );
    }
}

/// One scoreboard row: a name and MW2's columns (score, kills, assists, deaths).
#[derive(Clone, Debug)]
pub struct ScoreRow {
    pub name: String,
    pub cols: [i32; 4],
    pub you: bool,
}

/// Rows under the scoreboard page's column headers (its first four text labels), one header height apart, MW2's
/// white bar behind each row and the yellow one behind yours.
pub fn score_rows(ui: &UiPack, rows: &[ScoreRow], out: &mut Vec<Quad>) {
    let Some(page) = ui.screen("scoreboard") else {
        return;
    };
    let mut heads = page
        .labels
        .iter()
        .filter(|(k, _)| *k == label_kind::TEXT)
        .map(|(_, l)| l)
        .take(4);
    let Some(first) = heads.next() else {
        return;
    };
    let heads: Vec<&Label> = core::iter::once(first).chain(heads).collect();
    let right = heads.iter().map(|l| l.x + l.w).fold(first.x, f32::max);
    let max_rows = ((440.0 - first.y) / first.h.max(1.0)) as usize;
    for (n, r) in rows.iter().take(max_rows.saturating_sub(1)).enumerate() {
        let y = first.y + first.h * (n + 1) as f32;
        if ui.white != NONE {
            out.push(Quad {
                x: first.x,
                y,
                w: right - first.x + 3.0,
                h: first.h,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: if r.you { 0xffff_0054 } else { 0xffff_ff54 },
                texture: ui.white,
            });
        }
        let name = Label {
            y,
            align: 8, // left, centred vertically
            text_x: 3.0,
            rgba: 0xffff_ffff,
            ..first.clone()
        };
        label(ui, &name, &r.name, out);
        for (h, v) in heads.iter().zip(r.cols) {
            let cell = Label {
                y,
                rgba: 0xffff_ffff,
                ..(*h).clone()
            };
            label(ui, &cell, &alloc::format!("{v}"), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::ui::{Font, Glyph, Screen};

    fn pack() -> UiPack {
        let g = |letter: u16| Glyph {
            letter,
            x0: 0,
            y0: -10,
            dx: 10,
            w: 10,
            h: 10,
            s0: 0.0,
            t0: 0.0,
            s1: 1.0,
            t1: 1.0,
        };
        let l = |x: f32, text: &str| Label {
            x,
            y: 50.0,
            w: 100.0,
            h: 20.0,
            font: 0,
            text_scale: 0.5,
            align: 4,
            text_x: 0.0,
            text_y: 0.0,
            rgba: 0xffff_ffff,
            text: text.into(),
        };
        UiPack {
            fonts: vec![Font {
                name: "fonts/smallfont".into(),
                pixel_height: 24,
                texture: 9,
                glyphs: (32..91).map(g).collect(),
            }],
            screens: vec![Screen {
                name: "s".into(),
                quads: vec![Quad::default()],
                buttons: Some(ButtonStyle {
                    x: 0.0,
                    y: 30.0,
                    w: 200.0,
                    h: 20.0,
                    step: 20.0,
                    text_scale: 0.375,
                    bar: 3,
                    ..ButtonStyle::default()
                }),
                labels: vec![
                    (label_kind::TITLE, l(0.0, "MW2")),
                    (label_kind::SLOT, l(300.0, "")),
                    (label_kind::TEXT, l(400.0, "AB")),
                ],
                back: Some([0.0, 400.0, 200.0, 20.0]),
                owner_draws: Vec::new(),
                images: Vec::new(),
            }],
            white: 1,
            icons: Vec::new(),
            hud: Vec::new(),
            strings: Vec::new(),
        }
    }

    #[test]
    fn lays_out_title_slots_buttons_and_footer() {
        let ui = pack();
        let lines = [
            PageLine {
                text: "A",
                value: None,
                enabled: true,
            },
            PageLine {
                text: "BB",
                value: None,
                enabled: true,
            },
        ];
        let slots = [String::from("XYZ")];
        let page = Page {
            screen: "s",
            title: "T",
            lines: &lines,
            selected: 1,
            footer: "F",
            slots: &slots,
            images: &[],
            inline_values: false,
            backdrop: true,
            preview: "",
            checks: &[],
        };
        let mut q = Vec::new();
        layout(&ui, &page, &mut q);
        // Image, title "T", slot "XYZ", MW2 text "AB", bar, "A", "BB", footer "F".
        assert_eq!(q.len(), 1 + 1 + 3 + 2 + 1 + 1 + 2 + 1);
        let bar = q
            .iter()
            .position(|q| q.texture == 3)
            .expect("selection bar");
        assert_eq!(q[bar].y, 50.0, "on the second line");
        // The slot is drawn at its own rect.
        assert!(q.iter().any(|q| q.x == 300.0));
        // The footer sits in the back button's rect.
        assert!(q.iter().any(|q| q.y >= 400.0));
    }

    fn lines<'a>(texts: &'a [&'a str]) -> Vec<PageLine<'a>> {
        texts
            .iter()
            .map(|t| PageLine {
                text: t,
                value: None,
                enabled: true,
            })
            .collect()
    }

    #[test]
    fn a_popup_frame_fits_its_rows_and_rows_get_check_boxes() {
        let mut ui = pack();
        let s = &mut ui.screens[0];
        // At the popup's top: a frame quad baked without a height (sized by script), and a taller panel.
        s.quads = vec![
            Quad {
                y: 26.0,
                h: 0.0,
                texture: 4,
                ..Quad::default()
            },
            Quad {
                y: 26.0,
                h: 120.0,
                texture: 5,
                ..Quad::default()
            },
        ];
        s.owner_draws.push(wpk::ui::OwnerDraw {
            od: wpk::ui::od::POPUP_FRAME,
            y: 26.0,
            h: 0.0,
            ..wpk::ui::OwnerDraw::default()
        });
        ui.icons = vec![
            ("ks_button_check_bg".into(), 7),
            ("ks_button_check_fg".into(), 8),
        ];
        let rows = lines(&["A", "B", "C"]);
        let page = Page {
            screen: "s",
            title: "",
            lines: &rows,
            selected: 0,
            footer: "",
            slots: &[],
            images: &[],
            inline_values: false,
            backdrop: true,
            preview: "",
            checks: &[true, false, false],
        };
        let mut q = Vec::new();
        layout(&ui, &page, &mut q);
        // Rows from y 30, 20 apart: three end at 90, the frame 2 below (64 from its top).
        let h = |t: u16| q.iter().find(|q| q.texture == t).map(|q| q.h);
        assert_eq!(h(4), Some(30.0 + 60.0 - 26.0 + POPUP_PAD));
        // A quad of another height than the baked frame's stays as it is.
        assert_eq!(h(5), Some(120.0));
        assert_eq!(q.iter().filter(|q| q.texture == 7).count(), 3);
        assert_eq!(q.iter().filter(|q| q.texture == 8).count(), 1);
    }

    #[test]
    fn missing_screen_falls_back_to_main() {
        let mut ui = pack();
        ui.screens[0].name = "main".into();
        let page = Page {
            screen: "nope",
            title: "",
            lines: &[],
            selected: 0,
            footer: "",
            slots: &[],
            images: &[],
            inline_values: false,
            backdrop: true,
            preview: "",
            checks: &[],
        };
        let mut q = Vec::new();
        layout(&ui, &page, &mut q);
        assert!(!q.is_empty());
    }
}

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
}

/// MW2's main menu has no page name: ours goes above the first button, inside the 0.85 safe area (640 × 0.075).
const TITLE_X: f32 = 48.0;

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

/// Lay `page` out (falling back to the main menu's look when its screen isn't in the pack).
pub fn layout(ui: &UiPack, page: &Page<'_>, out: &mut Vec<Quad>) {
    let Some(screen) = ui.screen(page.screen).or_else(|| ui.screen("main")) else {
        return;
    };
    out.extend_from_slice(&screen.quads);
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
            label(ui, l, s, out);
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
    let bottom = screen.back.map_or(440.0, |r| r[1] - b.h * 0.5);
    let rows = (((bottom - b.y) / b.step) as usize).max(1);
    let text_h = b.text_scale * TEXT_EM;
    let small = ui.font("fonts/smallfont").unwrap_or(0);
    let values = page.slots.is_empty();
    for (row, i) in menu::window(page.selected, page.lines.len(), rows).enumerate() {
        let l = &page.lines[i];
        let y = b.y + row as f32 * b.step;
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
        if values && let Some(v) = l.value {
            text(ui, small, v, b.x + b.w + 12.0, y + text_h, 0.3, colour, out);
        }
    }
    match screen.back {
        Some(r) => {
            let x = button_x(ui, &b, page.footer, r[0], r[2]);
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
            }],
            white: 1,
            icons: Vec::new(),
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
        };
        let mut q = Vec::new();
        layout(&ui, &page, &mut q);
        assert!(!q.is_empty());
    }
}

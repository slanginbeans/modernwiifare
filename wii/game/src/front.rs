//! Front-end menus on the HUD font: main menu, map choice, class choice, Create-a-Class and the in-game pause menu.
//! Every screen is a list (D-pad or main stick up / down, A choose, B back). Custom classes are saved to
//! `<app folder>/classes.dat` by item name.

use alloc::string::String;
use alloc::vec::Vec;

use game_core::camera::Camera;
use game_core::gfx::Gfx;
use game_core::hud::{self, Rect};
use game_core::loadout::{self, FIELDS};
use game_core::menu::{self, Nav, Step};
use wpk::classes::{Catalog, Loadout};

use crate::{AppPath, Shim, ffi};

/// Edge-triggered pad input for menus (the stick acts like the D-pad, one line per push).
#[derive(Default)]
pub struct Input {
    held_y: i8,
}

impl Input {
    pub fn poll(&mut self) -> Nav {
        let pad = ffi::pad0();
        let stick = if pad.sy > 60 {
            1
        } else if pad.sy < -60 {
            -1
        } else {
            0
        };
        let nav = Nav {
            up: pad.down & ffi::BTN_UP != 0 || (stick == 1 && self.held_y != 1),
            down: pad.down & ffi::BTN_DOWN != 0 || (stick == -1 && self.held_y != -1),
            accept: pad.down & ffi::BTN_A != 0,
            back: pad.down & (ffi::BTN_B | ffi::BTN_START) != 0 || ffi::should_exit(),
        };
        self.held_y = stick;
        nav
    }
}

/// One line of a list: text, an optional right-hand value, and whether it can be chosen.
pub struct Line {
    pub text: String,
    pub value: Option<String>,
    pub enabled: bool,
}

impl Line {
    pub fn new(text: &str) -> Line {
        Line {
            text: String::from(text),
            value: None,
            enabled: true,
        }
    }
}

const ROWS: usize = 15;
const TITLE: u32 = 0xf0e8d0ff;
const NORMAL: u32 = 0xb0b8c0ff;
const SELECTED: u32 = 0xffd040ff;
const DISABLED: u32 = 0x505860ff;

/// Show a list until a line is chosen (`Accept`) or B (`Back`). `selected` keeps the cursor between calls.
pub fn list(
    gfx: &mut Shim,
    input: &mut Input,
    title: &str,
    lines: &[Line],
    selected: &mut usize,
    footer: &str,
) -> Step {
    let cam = Camera::new([0.0; 3], 0.0, 0.0);
    let mut rects: Vec<Rect> = Vec::with_capacity(4096);
    loop {
        let step = menu::step(selected, lines.len(), input.poll());
        match step {
            Step::Accept(i) if !lines[i].enabled => {}
            Step::Stay => {}
            other => return other,
        }
        unsafe { ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, 0x1a1e24ff) };
        if let Some(u) = crate::ui::get() {
            draw_mw2(u, title, lines, *selected, footer);
            gfx.frame_end();
            continue;
        }
        rects.clear();
        hud::text(&mut rects, 48, 40, 5, title);
        gfx.hud_rects(&rects, TITLE);
        for (row, i) in menu::window(*selected, lines.len(), ROWS).enumerate() {
            let l = &lines[i];
            let y = 96 + row as i16 * 22;
            let colour = if i == *selected {
                SELECTED
            } else if l.enabled {
                NORMAL
            } else {
                DISABLED
            };
            rects.clear();
            if i == *selected {
                hud::text(&mut rects, 32, y, 3, ">");
            }
            hud::text(&mut rects, 52, y, 3, &l.text);
            if let Some(v) = &l.value {
                hud::text(&mut rects, 292, y + 2, 2, v);
            }
            gfx.hud_rects(&rects, colour);
        }
        rects.clear();
        hud::text(&mut rects, 48, 446, 2, footer);
        gfx.hud_rects(&rects, 0x808890ff);
        gfx.frame_end();
    }
}

/// A list in MW2's main-menu style: its background images, its button rects and font, the selection bar on the
/// focused line (from the user's own `main_text` menu, baked by the converter).
fn draw_mw2(u: &mut crate::ui::Ui, title: &str, lines: &[Line], selected: usize, footer: &str) {
    u.screen("main");
    let style = u.pack.screen("main").and_then(|s| s.buttons);
    let Some(b) = style else {
        return;
    };
    let big = u.font("fonts/bigfont");
    let small = u.font("fonts/smallfont");
    let font = usize::from(b.font);
    // Title above the first button (MW2's menus put the page name there).
    // Inside MW2's 0.85 safe area (640 × 0.075).
    u.text(big, title, 48.0, b.y - 14.0, 0.5, 0xffffffff);
    let rows = (((440.0 - b.y) / b.step) as usize).max(1);
    let text_h = b.text_scale * game_core::ui_text::TEXT_EM;
    for (row, i) in menu::window(selected, lines.len(), rows).enumerate() {
        let l = &lines[i];
        let y = b.y + row as f32 * b.step;
        if i == selected && b.bar != wpk::ui::NONE {
            u.draw(&[wpk::ui::Quad {
                x: b.x,
                y,
                w: b.w,
                h: b.h,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: b.bar_rgba,
                texture: b.bar,
            }]);
        }
        let colour = if i == selected {
            b.focus_rgba
        } else if l.enabled {
            b.rgba
        } else {
            (b.rgba & 0xffff_ff00) | 0x60
        };
        // IW4 text placement (`hud_iw4::item_text_origin`): right-aligned buttons sit text_x from the rect's right edge;
        // the baseline is one text height below the rect's top.
        let w = u.text_width(font, &l.text, b.text_scale);
        let x = match b.align {
            1 => b.x + (b.w - w) * 0.5 + b.text_x,
            2 => b.x + b.w - w + b.text_x,
            _ => b.x + b.text_x,
        };
        u.text(font, &l.text, x, y + text_h, b.text_scale, colour);
        if let Some(v) = &l.value {
            u.text(small, v, b.x + b.w + 12.0, y + text_h, 0.3, colour);
        }
    }
    u.text(small, footer, 48.0, 456.0, 0.3, 0xc0c8d0ff);
}

pub enum Main {
    Play,
    CreateAClass,
    Quit,
}

pub fn main_menu(
    gfx: &mut Shim,
    input: &mut Input,
    selected: &mut usize,
    has_classes: bool,
) -> Main {
    let mut lines = [
        Line::new("PLAY"),
        Line::new("CREATE A CLASS"),
        Line::new("QUIT"),
    ];
    if !has_classes {
        lines[1].value = Some(String::from("NEEDS COMMON.WPK"));
        lines[1].enabled = false;
    }
    loop {
        match list(gfx, input, "MODERN WIIFARE", &lines, selected, "A SELECT") {
            Step::Accept(0) => return Main::Play,
            Step::Accept(1) => return Main::CreateAClass,
            Step::Accept(_) => return Main::Quit,
            // B on the main menu does nothing (QUIT is a line); RESET still quits.
            _ if ffi::should_exit() => return Main::Quit,
            _ => {}
        }
    }
}

/// Pick a map from `maps`; None = back.
pub fn pick_map(
    gfx: &mut Shim,
    input: &mut Input,
    maps: &[&str],
    selected: &mut usize,
) -> Option<usize> {
    let mut lines: Vec<Line> = maps.iter().map(|m| Line::new(m)).collect();
    if lines.is_empty() {
        let mut l = Line::new("NO MAPS IN THE APP FOLDER'S MAPS/");
        l.enabled = false;
        lines.push(l);
    }
    match list(
        gfx,
        input,
        "CHOOSE A MAP",
        &lines,
        selected,
        "A PLAY   B BACK",
    ) {
        Step::Accept(i) if !maps.is_empty() => Some(i),
        _ => None,
    }
}

/// Pick one of `classes` (custom first, then MW2's defaults); None = back.
pub fn pick_class(
    gfx: &mut Shim,
    input: &mut Input,
    title: &str,
    classes: &[Loadout],
    selected: &mut usize,
) -> Option<usize> {
    let lines: Vec<Line> = classes.iter().map(|c| Line::new(&c.name)).collect();
    match list(gfx, input, title, &lines, selected, "A CHOOSE   B BACK") {
        Step::Accept(i) => Some(i),
        _ => None,
    }
}

/// Edit a class field by field until B.
pub fn edit_class(gfx: &mut Shim, input: &mut Input, cat: &Catalog, class: &mut Loadout) {
    let mut field = 0usize;
    loop {
        let lines: Vec<Line> = FIELDS
            .iter()
            .map(|&f| Line {
                text: String::from(loadout::label(f)),
                value: Some(loadout::value(cat, class, f)),
                enabled: loadout::enabled(cat, class, f),
            })
            .collect();
        let title = alloc::format!("EDIT {}", class.name);
        let Step::Accept(i) = list(gfx, input, &title, &lines, &mut field, "A CHANGE   B DONE")
        else {
            return;
        };
        let f = FIELDS[i];
        let options = loadout::choices(cat, class, f);
        if options.is_empty() {
            continue;
        }
        let names: Vec<Line> = options
            .iter()
            .map(|&v| {
                let mut probe = class.clone();
                loadout::set(cat, &mut probe, f, v);
                Line::new(&loadout::value(cat, &probe, f))
            })
            .collect();
        let current = options
            .iter()
            .position(|&v| {
                let mut probe = class.clone();
                loadout::set(cat, &mut probe, f, v);
                loadout::value(cat, &probe, f) == loadout::value(cat, class, f)
            })
            .unwrap_or(0);
        let mut pick = current;
        if let Step::Accept(k) = list(
            gfx,
            input,
            loadout::label(f).trim_start(),
            &names,
            &mut pick,
            "A CHOOSE   B BACK",
        ) {
            loadout::set(cat, class, f, options[k]);
        }
    }
}

/// The custom classes: `classes.dat` when it is there and readable, else the catalog's starting classes.
pub fn load_classes(cat: &Catalog) -> Vec<Loadout> {
    let mut out = cat.customs.clone();
    if let Some(path) = AppPath::new("classes.dat") {
        let mut buf = alloc::vec![0u8; 4096];
        let n = unsafe { ffi::mwf_file_read(path.as_ptr(), 0, buf.len() as u32, buf.as_mut_ptr()) };
        if n > 0
            && let Ok(saved) = cat.decode_loadouts(&buf[..n as usize])
        {
            for (slot, l) in out.iter_mut().zip(saved) {
                *slot = l;
            }
        }
    }
    for l in &mut out {
        loadout::fix(cat, l);
    }
    out
}

/// Save the custom classes to `classes.dat`; false when the write failed.
pub fn save_classes(cat: &Catalog, classes: &[Loadout]) -> bool {
    let bytes = cat.encode_loadouts(classes);
    let Some(path) = AppPath::new("classes.dat") else {
        return false;
    };
    unsafe {
        ffi::mwf_file_write(path.as_ptr(), bytes.as_ptr(), bytes.len() as u32) == bytes.len() as i32
    }
}

pub enum Pause {
    Resume,
    ChangeClass(usize),
    EndGame,
}

/// The in-game pause menu (the match stops while it is open: there is no one else to play on).
pub fn pause(
    gfx: &mut Shim,
    input: &mut Input,
    classes: Option<&[Loadout]>,
    class_cursor: &mut usize,
) -> Pause {
    let mut sel = 0usize;
    loop {
        let mut lines = [
            Line::new("RESUME"),
            Line::new("CHANGE CLASS"),
            Line::new("END GAME"),
        ];
        lines[1].enabled = classes.is_some();
        match list(
            gfx,
            input,
            "PAUSED",
            &lines,
            &mut sel,
            "A SELECT   B RESUME",
        ) {
            Step::Accept(1) => {
                if let Some(c) = classes
                    && let Some(i) = pick_class(gfx, input, "CHANGE CLASS", c, class_cursor)
                {
                    return Pause::ChangeClass(i);
                }
            }
            Step::Accept(2) => return Pause::EndGame,
            _ => return Pause::Resume,
        }
    }
}

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

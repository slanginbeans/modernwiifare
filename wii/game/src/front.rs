//! Front-end menus on the HUD font: main menu, map choice, class choice, Create-a-Class and the in-game pause menu.
//! Every screen is a list (D-pad or main stick up / down, A choose, B back). Custom classes are saved to
//! `<app folder>/classes.dat` by item name.

use alloc::string::String;
use alloc::vec::Vec;

use game_core::arena::{Limits, Mode};
use game_core::camera::Camera;
use game_core::gfx::Gfx;
use game_core::hud::{self, Rect};
use game_core::loadout::{self, FIELDS};
use game_core::menu::{self, Nav, Step};
pub use game_core::ui_page::ScoreRow;
use game_core::ui_page::{self, PageLine};
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

/// Live text for a screen's empty slots (`wpk::ui::label_kind::SLOT`, in menu order) for the focused line.
pub type Panel<'a> = &'a dyn Fn(usize, &mut Vec<String>);

/// Which baked MW2 screen a list is drawn on, and what fills its slots.
pub struct Page<'a> {
    pub screen: &'a str,
    pub panel: Option<Panel<'a>>,
}

impl Page<'_> {
    pub const MAIN: Page<'static> = Page {
        screen: "main",
        panel: None,
    };
}

/// Show a list until a line is chosen (`Accept`) or B (`Back`). `selected` keeps the cursor between calls.
pub fn list(
    gfx: &mut Shim,
    input: &mut Input,
    title: &str,
    lines: &[Line],
    selected: &mut usize,
    footer: &str,
) -> Step {
    list_on(gfx, input, &Page::MAIN, title, lines, selected, footer)
}

/// `list` on a given MW2 screen.
pub fn list_on(
    gfx: &mut Shim,
    input: &mut Input,
    page: &Page<'_>,
    title: &str,
    lines: &[Line],
    selected: &mut usize,
    footer: &str,
) -> Step {
    let cam = Camera::new([0.0; 3], 0.0, 0.0);
    let mut rects: Vec<Rect> = Vec::with_capacity(4096);
    let mut slots: Vec<String> = Vec::new();
    loop {
        let step = menu::step(selected, lines.len(), input.poll());
        match step {
            Step::Accept(i) if !lines[i].enabled => {}
            Step::Stay => {}
            other => return other,
        }
        unsafe { ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, 0x1a1e24ff) };
        if let Some(u) = crate::ui::get() {
            slots.clear();
            if let Some(p) = page.panel {
                p(*selected, &mut slots);
            }
            draw_mw2(u, page.screen, title, lines, *selected, footer, &slots);
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

/// A list drawn on one of MW2's screens (`game_core::ui_page`).
fn draw_mw2(
    u: &mut crate::ui::Ui,
    screen: &str,
    title: &str,
    lines: &[Line],
    selected: usize,
    footer: &str,
    slots: &[String],
) {
    let lines: Vec<PageLine<'_>> = lines
        .iter()
        .map(|l| PageLine {
            text: &l.text,
            value: l.value.as_deref(),
            enabled: l.enabled,
        })
        .collect();
    let page = ui_page::Page {
        screen,
        title,
        lines: &lines,
        selected,
        footer,
        slots,
    };
    u.page(|ui, q| ui_page::layout(ui, &page, q));
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

/// MW2's private-match game setup: the game type, score limit and time limit (each starts at the user's scripts'
/// value for the mode; A cycles through the choices), then START. False on B.
pub fn game_setup(
    gfx: &mut Shim,
    input: &mut Input,
    rules: &wpk::rules::Rules,
    mode: &mut Mode,
    limits: &mut Limits,
) -> bool {
    let mut sel = 3usize;
    loop {
        let score = if limits.score_limit == 0 {
            String::from("UNLIMITED")
        } else {
            alloc::format!("{}", limits.score_limit)
        };
        let time = if limits.time_limit_ms == 0 {
            String::from("UNLIMITED")
        } else {
            alloc::format!("{} MIN", limits.time_limit_ms / 60_000)
        };
        let value = |text: &str, v: String| Line {
            text: String::from(text),
            value: Some(v),
            enabled: true,
        };
        let lines = [
            value(
                "GAME TYPE",
                String::from(match mode {
                    Mode::Ffa => "FREE-FOR-ALL",
                    Mode::Tdm => "TEAM DEATHMATCH",
                }),
            ),
            value("SCORE LIMIT", score),
            value("TIME LIMIT", time),
            Line::new("START MATCH"),
        ];
        match list(
            gfx,
            input,
            "GAME SETUP",
            &lines,
            &mut sel,
            "A CHANGE / START   B BACK",
        ) {
            Step::Accept(0) => {
                *mode = match mode {
                    Mode::Ffa => Mode::Tdm,
                    Mode::Tdm => Mode::Ffa,
                };
                *limits = Limits::of(rules, *mode);
            }
            Step::Accept(1) => {
                // The scripts' limit, then whole numbers of kills, then none.
                let default = Limits::of(rules, *mode).score_limit;
                let mut options: Vec<u32> = alloc::vec![default];
                for kills in [10, 20, 30, 50, 75, 100, 150] {
                    let v = kills * limits.kill_score.max(1);
                    if !options.contains(&v) {
                        options.push(v);
                    }
                }
                options.push(0);
                let at = options
                    .iter()
                    .position(|&v| v == limits.score_limit)
                    .unwrap_or(0);
                limits.score_limit = options[(at + 1) % options.len()];
            }
            Step::Accept(2) => {
                let default = Limits::of(rules, *mode).time_limit_ms;
                let mut options: Vec<u32> = alloc::vec![default];
                for min in [5, 10, 15, 20, 30] {
                    let v = min * 60_000;
                    if !options.contains(&v) {
                        options.push(v);
                    }
                }
                options.push(0);
                let at = options
                    .iter()
                    .position(|&v| v == limits.time_limit_ms)
                    .unwrap_or(0);
                limits.time_limit_ms = options[(at + 1) % options.len()];
            }
            Step::Accept(_) => return true,
            _ => return false,
        }
    }
}
/// Pick one of `classes` (custom first, then MW2's defaults) on MW2's class screen, the focused class shown in its
/// panel; None = back.
pub fn pick_class(
    gfx: &mut Shim,
    input: &mut Input,
    cat: Option<&Catalog>,
    title: &str,
    classes: &[Loadout],
    selected: &mut usize,
) -> Option<usize> {
    let lines: Vec<Line> = classes.iter().map(|c| Line::new(&c.name)).collect();
    let panel = |i: usize, out: &mut Vec<String>| {
        if let (Some(cat), Some(c)) = (cat, classes.get(i)) {
            loadout::panel(cat, c, out);
        }
    };
    let page = Page {
        screen: "class_select",
        panel: Some(&panel),
    };
    match list_on(
        gfx,
        input,
        &page,
        title,
        &lines,
        selected,
        "A CHOOSE   B BACK",
    ) {
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
        let shown = class.clone();
        let panel = |_: usize, out: &mut Vec<String>| loadout::panel(cat, &shown, out);
        let page = Page {
            screen: "cac",
            panel: Some(&panel),
        };
        let Step::Accept(i) = list_on(
            gfx,
            input,
            &page,
            &title,
            &lines,
            &mut field,
            "A CHANGE   B DONE",
        ) else {
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
        // The panel previews the focused choice.
        let base = class.clone();
        let preview = |i: usize, out: &mut Vec<String>| {
            let mut probe = base.clone();
            if let Some(&v) = options.get(i) {
                loadout::set(cat, &mut probe, f, v);
            }
            loadout::panel(cat, &probe, out);
        };
        let page = Page {
            screen: "cac",
            panel: Some(&preview),
        };
        if let Step::Accept(k) = list_on(
            gfx,
            input,
            &page,
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
    map: &str,
    mode: &str,
    cat: Option<&Catalog>,
    classes: Option<&[Loadout]>,
    class_cursor: &mut usize,
) -> Pause {
    // MW2's pause screen names the map and the mode in its right-hand slots.
    let info = |_: usize, out: &mut Vec<String>| {
        out.push(map.trim_start_matches("mp_").to_ascii_uppercase());
        out.push(String::from(mode));
    };
    let page = Page {
        screen: "pause",
        panel: Some(&info),
    };
    let mut sel = 0usize;
    loop {
        let mut lines = [
            Line::new("RESUME"),
            Line::new("CHANGE CLASS"),
            Line::new("END GAME"),
        ];
        lines[1].enabled = classes.is_some();
        match list_on(
            gfx,
            input,
            &page,
            "PAUSED",
            &lines,
            &mut sel,
            "A SELECT   B RESUME",
        ) {
            Step::Accept(1) => {
                if let Some(c) = classes
                    && let Some(i) = pick_class(gfx, input, cat, "CHANGE CLASS", c, class_cursor)
                {
                    return Pause::ChangeClass(i);
                }
            }
            Step::Accept(2) => return Pause::EndGame,
            _ => return Pause::Resume,
        }
    }
}

/// The end-of-match scoreboard on MW2's scoreboard page (its frame and column headers), until A or B. False when
/// the pack has no scoreboard page (the caller shows a plain list).
pub fn scoreboard(gfx: &mut Shim, input: &mut Input, title: &str, rows: &[ScoreRow]) -> bool {
    let has_page = crate::ui::get().is_some_and(|u| u.pack.screen("scoreboard").is_some());
    if !has_page {
        return false;
    }
    let cam = Camera::new([0.0; 3], 0.0, 0.0);
    let footer = [String::from("A CONTINUE")];
    loop {
        let nav = input.poll();
        if nav.accept || nav.back {
            return true;
        }
        unsafe { ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, 0x1a1e24ff) };
        if let Some(u) = crate::ui::get() {
            draw_mw2(u, "scoreboard", title, &[], 0, "", &footer);
            u.page(|ui, q| ui_page::score_rows(ui, rows, q));
        }
        gfx.frame_end();
    }
}

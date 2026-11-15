//! Front-end menus in the Xbox 360 MW2's flow: the main menu, the private match lobby (start, game setup, Create-a-
//! Class, the players), the Game Setup popup (map, game mode, game rules), and in a match the START menu over the
//! frozen game (choose class, options, end game with MW2's yes / no question). Every screen is drawn on one of
//! MW2's own baked screens (`game_core::ui_page`) and driven as a list: D-pad or stick up / down (held, it repeats),
//! left / right change a value, A chooses, B goes back. Custom classes are saved to `<app folder>/classes.dat` by
//! item name.

use alloc::string::String;
use alloc::vec::Vec;

use game_core::arena::{Limits, Mode};
use game_core::camera::Camera;
use game_core::gfx::Gfx;
use game_core::hud::{self, Rect};
use game_core::loadout::{self, FIELDS};
use game_core::menu::{self, Nav, Repeat, Step};
use game_core::ui_page::{self, PageLine};
pub use game_core::ui_page::{LobbyRow, ScoreRow};
use wpk::classes::{Catalog, Loadout};
use wpk::ui::{Quad, UiPack, menu_sound};

use crate::{AppPath, Shim, ffi};

/// Pad input for menus: edges, with a held direction repeating (`game_core::menu::Repeat`). Either pad drives the
/// menus: the GameCube pad, or the USB Xbox 360 pad (D-pad or left stick, A accepts, B / START / BACK go back).
#[derive(Default)]
pub struct Input {
    prev_x360: u16,
    rep_y: Repeat,
    rep_x: Repeat,
    /// Ignore A and B until both are let go (a menu opened by a press doesn't take that press).
    settle: bool,
}

/// A stick axis as a direction: past 60 of 127 (the GameCube stick's gate is about 100).
fn stick_dir(v: i32, threshold: i32) -> i8 {
    if v > threshold {
        1
    } else if v < -threshold {
        -1
    } else {
        0
    }
}

impl Input {
    /// Ignore A, B and START until they are let go (after a press opened this menu, or a match ended).
    pub fn settle(&mut self) {
        self.settle = true;
    }

    pub fn poll(&mut self) -> Nav {
        use game_core::controls::{gc, x360, x360_axis};
        let (pad, x) = ffi::pads();
        let xb = if x.connected { x.buttons } else { 0 };
        let xd = xb & !self.prev_x360;
        self.prev_x360 = xb;
        let (xx, xy) = if x.connected {
            (i32::from(x360_axis(x.lx)), i32::from(x360_axis(x.ly)))
        } else {
            (0, 0)
        };
        let held = |g: u16, b: u16| pad.held & g != 0 || xb & b != 0;
        let mut dy = stick_dir(i32::from(pad.sy), 60);
        if dy == 0 {
            dy = stick_dir(xy, 60);
        }
        if held(gc::UP, x360::UP) {
            dy = 1;
        } else if held(gc::DOWN, x360::DOWN) {
            dy = -1;
        }
        let mut dx = stick_dir(i32::from(pad.sx), 60);
        if dx == 0 {
            dx = stick_dir(xx, 60);
        }
        if held(gc::RIGHT, x360::RIGHT) {
            dx = 1;
        } else if held(gc::LEFT, x360::LEFT) {
            dx = -1;
        }
        let now = ffi::ms();
        let fy = self.rep_y.update(dy, now);
        let fx = self.rep_x.update(dx, now);
        let buttons = pad.held & (gc::A | gc::B | gc::START) != 0
            || xb & (x360::A | x360::B | x360::START | x360::BACK) != 0;
        if self.settle {
            if buttons {
                return Nav {
                    up: fy && dy > 0,
                    down: fy && dy < 0,
                    left: fx && dx < 0,
                    right: fx && dx > 0,
                    ..Nav::default()
                };
            }
            self.settle = false;
        }
        Nav {
            up: fy && dy > 0,
            down: fy && dy < 0,
            left: fx && dx < 0,
            right: fx && dx > 0,
            accept: pad.down & gc::A != 0 || xd & x360::A != 0,
            back: pad.down & (gc::B | gc::START) != 0
                || xd & (x360::B | x360::START | x360::BACK) != 0
                || ffi::should_exit(),
        }
    }
}

/// One line of a list: text, an optional value, and whether it can be chosen.
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

    pub fn value(text: &str, value: String) -> Line {
        Line {
            text: String::from(text),
            value: Some(value),
            enabled: true,
        }
    }
}

const ROWS: usize = 15;
const TITLE: u32 = 0xf0e8d0ff;
const NORMAL: u32 = 0xb0b8c0ff;
const SELECTED: u32 = 0xffd040ff;
const DISABLED: u32 = 0x505860ff;
/// Footer hints (MW2's console prompts name the buttons the same way).
pub const FOOTER_SELECT: &str = "A SELECT   B BACK";
const FOOTER_CHANGE: &str = "A / LEFT / RIGHT CHANGE   B BACK";

/// Live text for a screen's empty slots (`wpk::ui::label_kind::SLOT`, in menu order) and the items its images show
/// (`wpk::ui::image_field` order) for the focused line.
pub type Panel<'a> = &'a dyn Fn(usize, &mut Vec<String>, &mut Vec<String>);
/// Quads drawn under or over a page (the frozen game, the lobby under a popup, the lobby's players).
pub type Layer<'a> = &'a dyn Fn(&UiPack, &mut Vec<Quad>);
/// The map whose preview picture a page shows for the focused line.
pub type Preview<'a> = &'a dyn Fn(usize) -> String;

/// Which baked MW2 screen a list is drawn on, and what goes with it.
#[derive(Default)]
pub struct Page<'a> {
    pub screen: &'a str,
    pub panel: Option<Panel<'a>>,
    /// Values inside their rows (MW2's rules rows); left / right change them.
    pub values: bool,
    pub under: Option<Layer<'a>>,
    pub over: Option<Layer<'a>>,
    pub preview: Option<Preview<'a>>,
}

impl<'a> Page<'a> {
    pub fn on(screen: &'a str) -> Page<'a> {
        Page {
            screen,
            ..Page::default()
        }
    }
}

/// MW2's menu sounds (`UISN` in `common.wpk`) and the bank they play from.
struct MenuSounds {
    bank: crate::sound::Sounds<'static>,
    aliases: [u16; 3],
}

static mut MENU_SOUNDS: Option<MenuSounds> = None;

/// Load the menu sounds from the common pack (none with an older pack).
pub fn init_menu_sounds(common: &'static [u8]) {
    let Some(bank) = crate::sound::Sounds::load(common) else {
        return;
    };
    let Some(s) = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::MENU_SOUNDS))
    else {
        return;
    };
    let mut aliases = [wpk::sound::NONE; 3];
    for (k, a) in aliases.iter_mut().enumerate() {
        if let Some(b) = s.data.get(k * 2..k * 2 + 2) {
            *a = u16::from_be_bytes([b[0], b[1]]);
        }
    }
    // Single-threaded: set once at start, before any menu.
    unsafe { MENU_SOUNDS = Some(MenuSounds { bank, aliases }) };
}

/// Play menu sound `which` (`wpk::ui::menu_sound`).
#[allow(static_mut_refs)]
fn play_menu_sound(which: usize) {
    // Single-threaded: the menus only.
    if let Some(m) = unsafe { MENU_SOUNDS.as_mut() } {
        let a = m.aliases[which];
        m.bank.play(a);
    }
}

/// The frozen game (kept when the menu opened), as the in-game menus' background.
pub fn grabbed(_: &UiPack, q: &mut Vec<Quad>) {
    q.push(ui_page::fullscreen(ui_page::GRAB, 0xffff_ffff));
}

/// Keep the frame on screen for the in-game menus.
pub fn grab_screen() -> bool {
    unsafe { ffi::mwf_grab_screen() != 0 }
}

/// Show a list until a line is chosen (`Accept`), changed (`Adjust`, on pages with values) or B (`Back`).
/// `selected` keeps the cursor between calls.
pub fn list(
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
    let mut images: Vec<String> = Vec::new();
    // Popups (over another page) use MW2's submenu focus sound.
    let over_sound = if page.under.is_some() {
        menu_sound::SUBMENU_OVER
    } else {
        menu_sound::OVER
    };
    loop {
        let before = *selected;
        let step = menu::step(selected, lines.len(), input.poll());
        if *selected != before {
            play_menu_sound(over_sound);
        }
        match step {
            Step::Accept(i) if !lines[i].enabled => {}
            Step::Adjust(i, _) if !page.values || !lines[i].enabled || lines[i].value.is_none() => {
            }
            Step::Stay => {}
            other => {
                play_menu_sound(if let Step::Adjust(..) = other {
                    over_sound
                } else {
                    menu_sound::CLICK
                });
                return other;
            }
        }
        unsafe { ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, 0x1a1e24ff) };
        if let Some(u) = crate::ui::get() {
            slots.clear();
            images.clear();
            if let Some(p) = page.panel {
                p(*selected, &mut slots, &mut images);
            }
            let preview = page.preview.map(|f| f(*selected)).unwrap_or_default();
            let page_lines: Vec<PageLine<'_>> = lines
                .iter()
                .map(|l| PageLine {
                    text: &l.text,
                    value: l.value.as_deref(),
                    enabled: l.enabled,
                })
                .collect();
            let p = ui_page::Page {
                screen: page.screen,
                title,
                lines: &page_lines,
                selected: *selected,
                footer,
                slots: &slots,
                images: &images,
                inline_values: page.values,
                // Over something (the frozen game, the lobby) the screen's opaque background is left out.
                backdrop: page.under.is_none(),
                preview: &preview,
            };
            u.page(|ui, q| {
                if let Some(f) = page.under {
                    f(ui, q);
                }
                ui_page::layout(ui, &p, q);
                if let Some(f) = page.over {
                    f(ui, q);
                }
            });
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

/// A page's string from MW2's localized text, else ours.
fn mw2(key: &str, ours: &str) -> String {
    crate::ui::get()
        .and_then(|u| u.pack.string(key).map(String::from))
        .unwrap_or_else(|| String::from(ours))
}

pub enum Main {
    PrivateMatch,
    CreateAClass,
    Options,
    Debug,
    Quit,
}

pub fn main_menu(
    gfx: &mut Shim,
    input: &mut Input,
    selected: &mut usize,
    has_classes: bool,
) -> Main {
    let mut lines = [
        Line::new("PRIVATE MATCH"),
        Line::new("CREATE A CLASS"),
        Line::new("OPTIONS"),
        Line::new("DEBUG"),
        Line::new("QUIT"),
    ];
    if !has_classes {
        lines[1].value = Some(String::from("NEEDS COMMON.WPK"));
        lines[1].enabled = false;
    }
    loop {
        match list(
            gfx,
            input,
            &Page::on("main"),
            "MODERN WIIFARE",
            &lines,
            selected,
            "A SELECT",
        ) {
            Step::Accept(0) => return Main::PrivateMatch,
            Step::Accept(1) => return Main::CreateAClass,
            Step::Accept(2) => return Main::Options,
            Step::Accept(3) => return Main::Debug,
            Step::Accept(_) => return Main::Quit,
            // B on the main menu does nothing (QUIT is a line); RESET still quits.
            _ if ffi::should_exit() => return Main::Quit,
            _ => {}
        }
    }
}

/// The match being set up: map, mode, limits, bots.
pub struct Setup<'a> {
    pub maps: &'a [&'a str],
    pub map: &'a mut usize,
    pub mode: &'a mut Mode,
    pub limits: &'a mut Limits,
    pub rules: &'a wpk::rules::Rules,
    pub debug: &'a mut game_core::debug::DebugSettings,
}

/// A map's name as MW2 shows it (`MPUI_<MAP>`), else the file name without `mp_`.
pub fn map_name(file: &str) -> String {
    let short = file.trim_start_matches("mp_").to_ascii_uppercase();
    mw2(&alloc::format!("MPUI_{short}"), &short)
}

fn map_description(file: &str) -> String {
    let short = file.trim_start_matches("mp_").to_ascii_uppercase();
    mw2(&alloc::format!("MPUI_DESC_MAP_{short}"), "")
}

/// A game mode's name and description (MW2's game type table's strings).
pub fn mode_name(m: Mode) -> String {
    match m {
        Mode::Ffa => mw2("MPUI_DEATHMATCH", "Free-for-all"),
        Mode::Tdm => mw2("MPUI_WAR", "Team Deathmatch"),
        Mode::Dom => mw2("MPUI_DOMINATION", "Domination"),
    }
}

fn mode_description(m: Mode) -> String {
    match m {
        Mode::Ffa => mw2("MENU_KILL_EVERYONE_FIRST_PLAYER", ""),
        Mode::Tdm => mw2("MENU_KILL_PLAYERS_ON_THE_OPPOSING", ""),
        Mode::Dom => mw2("MENU_CAPTURE_AND_HOLD_THE", ""),
    }
}

const MODES: [Mode; 3] = [Mode::Ffa, Mode::Tdm, Mode::Dom];

pub enum Lobby {
    Start,
    CreateAClass,
    Back,
}

/// The private match lobby: START GAME, GAME SETUP (the popup with the map, the mode and the rules), CREATE A CLASS,
/// with the players on the right and the map and mode in the lobby's slots.
pub fn lobby(
    gfx: &mut Shim,
    input: &mut Input,
    setup: &mut Setup<'_>,
    players: &dyn Fn(Mode, usize) -> Vec<LobbyRow>,
    selected: &mut usize,
) -> Lobby {
    let lines = [
        Line::new("START GAME"),
        Line::new("GAME SETUP"),
        Line::new("CREATE A CLASS"),
    ];
    loop {
        let rows = players(*setup.mode, usize::from(setup.debug.bots));
        let map = setup.maps.get(*setup.map).copied().unwrap_or("");
        // The lobby's slots: the player count (bottom right), the map and the game mode (bottom left).
        let info = [
            alloc::format!("{} PLAYERS", rows.len()),
            map_name(map),
            mode_name(*setup.mode),
        ];
        let panel = |_: usize, out: &mut Vec<String>, _: &mut Vec<String>| {
            out.extend(info.iter().cloned());
        };
        let over = |ui: &UiPack, q: &mut Vec<Quad>| ui_page::lobby_rows(ui, &rows, q);
        let preview = |_: usize| String::from(map);
        let page = Page {
            screen: "lobby",
            panel: Some(&panel),
            over: Some(&over),
            preview: Some(&preview),
            ..Page::default()
        };
        match list(
            gfx,
            input,
            &page,
            "PRIVATE MATCH",
            &lines,
            selected,
            FOOTER_SELECT,
        ) {
            Step::Accept(0) if !setup.maps.is_empty() => return Lobby::Start,
            Step::Accept(1) => game_setup(gfx, input, setup, &rows, &info),
            Step::Accept(2) => return Lobby::CreateAClass,
            Step::Back => return Lobby::Back,
            _ => {}
        }
    }
}

/// MW2's Game Setup popup over the lobby: CHANGE MAP, CHANGE GAME MODE, GAME RULES, until B.
fn game_setup(
    gfx: &mut Shim,
    input: &mut Input,
    setup: &mut Setup<'_>,
    rows: &[LobbyRow],
    info: &[String; 3],
) {
    let lines = [
        Line::new("CHANGE MAP"),
        Line::new("CHANGE GAME MODE"),
        Line::new("GAME RULES"),
    ];
    let mut sel = 0usize;
    loop {
        let map = setup.maps.get(*setup.map).copied().unwrap_or("");
        // The lobby, dimmed, under the popup.
        let lobby_lines = [
            PageLine {
                text: "START GAME",
                value: None,
                enabled: true,
            },
            PageLine {
                text: "GAME SETUP",
                value: None,
                enabled: true,
            },
            PageLine {
                text: "CREATE A CLASS",
                value: None,
                enabled: true,
            },
        ];
        let under = |ui: &UiPack, q: &mut Vec<Quad>| {
            ui_page::layout(
                ui,
                &ui_page::Page {
                    screen: "lobby",
                    title: "PRIVATE MATCH",
                    lines: &lobby_lines,
                    selected: 1,
                    footer: "",
                    slots: info,
                    images: &[],
                    inline_values: false,
                    backdrop: true,
                    preview: map,
                },
                q,
            );
            ui_page::lobby_rows(ui, rows, q);
            q.push(ui_page::fullscreen(ui.white, 0x0000_0099));
        };
        let page = Page {
            screen: "setup",
            under: Some(&under),
            ..Page::default()
        };
        match list(gfx, input, &page, "GAME SETUP", &lines, &mut sel, "") {
            Step::Accept(0) => pick_map(gfx, input, setup.maps, setup.map),
            Step::Accept(1) => {
                if pick_mode(gfx, input, setup.mode) {
                    *setup.limits = Limits::of(setup.rules, *setup.mode);
                }
            }
            Step::Accept(2) => game_rules(gfx, input, setup),
            Step::Back => return,
            _ => {}
        }
    }
}

/// MW2's map page: the maps by name, the focused one's name and description on the right.
pub fn pick_map(gfx: &mut Shim, input: &mut Input, maps: &[&str], selected: &mut usize) {
    let mut lines: Vec<Line> = maps.iter().map(|m| Line::new(&map_name(m))).collect();
    if lines.is_empty() {
        let mut l = Line::new("NO MAPS IN THE APP FOLDER'S MAPS/");
        l.enabled = false;
        lines.push(l);
    }
    let panel = |i: usize, out: &mut Vec<String>, _: &mut Vec<String>| {
        if let Some(m) = maps.get(i) {
            out.push(map_name(m));
            out.push(map_description(m));
        }
    };
    let preview = |i: usize| maps.get(i).map_or_else(String::new, |m| String::from(*m));
    let page = Page {
        screen: "maps",
        panel: Some(&panel),
        preview: Some(&preview),
        ..Page::default()
    };
    let mut cursor = *selected;
    if let Step::Accept(i) = list(
        gfx,
        input,
        &page,
        "CHOOSE MAP",
        &lines,
        &mut cursor,
        FOOTER_SELECT,
    ) && i < maps.len()
    {
        *selected = i;
    }
}

/// MW2's game mode page. True when the mode changed.
fn pick_mode(gfx: &mut Shim, input: &mut Input, mode: &mut Mode) -> bool {
    let lines: Vec<Line> = MODES.iter().map(|&m| Line::new(&mode_name(m))).collect();
    let panel = |i: usize, out: &mut Vec<String>, _: &mut Vec<String>| {
        if let Some(&m) = MODES.get(i) {
            out.push(mode_name(m));
            out.push(mode_description(m));
        }
    };
    let page = Page {
        screen: "modes",
        panel: Some(&panel),
        ..Page::default()
    };
    let mut cursor = MODES.iter().position(|m| m == mode).unwrap_or(0);
    match list(
        gfx,
        input,
        &page,
        "CHOOSE GAME MODE",
        &lines,
        &mut cursor,
        FOOTER_SELECT,
    ) {
        Step::Accept(i) if MODES[i] != *mode => {
            *mode = MODES[i];
            true
        }
        _ => false,
    }
}

/// The choices of a limit: the scripts' value for the mode first, then MW2's usual steps, then none (0).
fn limit_choices(default: u32, steps: &[u32]) -> Vec<u32> {
    let mut v = alloc::vec![default];
    for &s in steps {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v.sort_unstable();
    v.retain(|&x| x != 0);
    v.push(0);
    v
}

fn step_choice(options: &[u32], current: u32, dir: i8) -> u32 {
    let at = options.iter().position(|&v| v == current).unwrap_or(0);
    let n = options.len();
    options[if dir > 0 {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    }]
}

/// MW2's Game Rules page (its rules rows): time limit, score limit, the bots and their difficulty.
fn game_rules(gfx: &mut Shim, input: &mut Input, s: &mut Setup<'_>) {
    use game_core::debug::Item;
    let mut sel = 0usize;
    loop {
        let score = if s.limits.score_limit == 0 {
            String::from("UNLIMITED")
        } else {
            alloc::format!("{}", s.limits.score_limit)
        };
        let time = if s.limits.time_limit_ms == 0 {
            String::from("UNLIMITED")
        } else {
            alloc::format!("{} MINUTES", s.limits.time_limit_ms / 60_000)
        };
        let lines = [
            Line::value("TIME LIMIT", time),
            Line::value("SCORE LIMIT", score),
            Line::value(
                "BOTS",
                s.debug.value(Item::Bots, |_| None).unwrap_or_default(),
            ),
            Line::value(
                "BOT DIFFICULTY",
                s.debug.value(Item::BotSkill, |_| None).unwrap_or_default(),
            ),
        ];
        let page = Page {
            screen: "rules",
            values: true,
            ..Page::default()
        };
        let (i, dir) = match list(
            gfx,
            input,
            &page,
            &mode_name(*s.mode).to_ascii_uppercase(),
            &lines,
            &mut sel,
            FOOTER_CHANGE,
        ) {
            Step::Accept(i) => (i, 1),
            Step::Adjust(i, d) => (i, d),
            _ => return,
        };
        let defaults = Limits::of(s.rules, *s.mode);
        match i {
            0 => {
                let options = limit_choices(
                    defaults.time_limit_ms,
                    &[5, 10, 15, 20, 30].map(|m| m * 60_000),
                );
                s.limits.time_limit_ms = step_choice(&options, s.limits.time_limit_ms, dir);
            }
            1 => {
                let k = s.limits.kill_score.max(1);
                let options = limit_choices(
                    defaults.score_limit,
                    &[10, 20, 30, 50, 75, 100, 150].map(|n| n * k),
                );
                s.limits.score_limit = step_choice(&options, s.limits.score_limit, dir);
            }
            2 => cycle_debug(s.debug, Item::Bots, dir),
            _ => cycle_debug(s.debug, Item::BotSkill, dir),
        }
    }
}

/// A debug setting one step forward, or back (its cycle wraps: back is all the way round less one).
fn cycle_debug(d: &mut game_core::debug::DebugSettings, item: game_core::debug::Item, dir: i8) {
    if dir > 0 {
        d.cycle(item, 0);
        return;
    }
    let start = d.encode();
    let mut prev = start.clone();
    for _ in 0..64 {
        d.cycle(item, 0);
        if d.encode() == start {
            if let Some(p) = game_core::debug::DebugSettings::decode(&prev) {
                *d = p;
            }
            return;
        }
        prev = d.encode();
    }
}

/// Pick one of `classes` (custom first, then MW2's defaults) on MW2's class screen, the focused class shown in its
/// panel; None = back. `under`: the frozen game, in a match.
#[allow(clippy::too_many_arguments)]
pub fn pick_class(
    gfx: &mut Shim,
    input: &mut Input,
    cat: Option<&Catalog>,
    title: &str,
    classes: &[Loadout],
    selected: &mut usize,
    under: Option<Layer<'_>>,
) -> Option<usize> {
    let lines: Vec<Line> = classes.iter().map(|c| Line::new(&c.name)).collect();
    let panel = |i: usize, out: &mut Vec<String>, images: &mut Vec<String>| {
        if let (Some(cat), Some(c)) = (cat, classes.get(i)) {
            loadout::panel(cat, c, out);
            images.extend(loadout::images(cat, c));
        }
    };
    let page = Page {
        screen: "class_select",
        panel: Some(&panel),
        under,
        ..Page::default()
    };
    match list(gfx, input, &page, title, &lines, selected, FOOTER_SELECT) {
        Step::Accept(i) => Some(i),
        _ => None,
    }
}

/// Create-a-Class: pick a custom class, edit it field by field, saved when leaving it.
pub fn create_a_class(
    gfx: &mut Shim,
    input: &mut Input,
    cat: &Catalog,
    custom: &mut [Loadout],
    selected: &mut usize,
) {
    while let Some(i) = pick_class(
        gfx,
        input,
        Some(cat),
        "CREATE A CLASS",
        custom,
        selected,
        None,
    ) {
        edit_class(gfx, input, cat, &mut custom[i]);
        save_classes(cat, custom);
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
        let title = class.name.to_ascii_uppercase();
        let shown = class.clone();
        let panel = |_: usize, out: &mut Vec<String>, images: &mut Vec<String>| {
            loadout::panel(cat, &shown, out);
            images.extend(loadout::images(cat, &shown));
        };
        let page = Page {
            screen: "cac",
            panel: Some(&panel),
            ..Page::default()
        };
        let Step::Accept(i) = list(gfx, input, &page, &title, &lines, &mut field, FOOTER_SELECT)
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
        // The panel previews the focused choice.
        let base = class.clone();
        let preview = |i: usize, out: &mut Vec<String>, images: &mut Vec<String>| {
            let mut probe = base.clone();
            if let Some(&v) = options.get(i) {
                loadout::set(cat, &mut probe, f, v);
            }
            loadout::panel(cat, &probe, out);
            images.extend(loadout::images(cat, &probe));
        };
        let page = Page {
            screen: "cac",
            panel: Some(&preview),
            ..Page::default()
        };
        if let Step::Accept(k) = list(
            gfx,
            input,
            &page,
            loadout::label(f).trim_start(),
            &names,
            &mut pick,
            FOOTER_SELECT,
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
    Options,
    Debug,
    EndGame,
}

/// MW2's yes / no popup (its End Game question) over `under`. True for YES.
pub fn confirm(gfx: &mut Shim, input: &mut Input, title: &str, under: Layer<'_>) -> bool {
    let lines = [
        Line::new(&mw2("MENU_YES", "Yes")),
        Line::new(&mw2("MENU_NO", "No")),
    ];
    let dim = |ui: &UiPack, q: &mut Vec<Quad>| {
        under(ui, q);
        q.push(ui_page::fullscreen(ui.white, 0x0000_0099));
    };
    let page = Page {
        screen: "yesno",
        under: Some(&dim),
        ..Page::default()
    };
    // NO is focused first, as in MW2.
    let mut sel = 1usize;
    matches!(
        list(gfx, input, &page, title, &lines, &mut sel, ""),
        Step::Accept(0)
    )
}

/// The in-game START menu over the frozen game (the match stops while it is open: there is no one else to play on):
/// CHOOSE CLASS, OPTIONS, END GAME (asked first), DEBUG; B or START resumes. `sel` keeps the cursor (the caller
/// reopens the menu after Options).
#[allow(clippy::too_many_arguments)]
pub fn pause(
    gfx: &mut Shim,
    input: &mut Input,
    map: &str,
    mode: &str,
    cat: Option<&Catalog>,
    classes: Option<&[Loadout]>,
    class_cursor: &mut usize,
    sel: &mut usize,
) -> Pause {
    // MW2's pause screen names the map and the mode in its right-hand slots.
    let info = |_: usize, out: &mut Vec<String>, _: &mut Vec<String>| {
        out.push(map_name(map).to_ascii_uppercase());
        out.push(String::from(mode));
    };
    let page = Page {
        screen: "pause",
        panel: Some(&info),
        under: Some(&grabbed),
        ..Page::default()
    };
    // START opened this menu: it doesn't also close it.
    input.settle();
    loop {
        let mut lines = [
            Line::new("CHOOSE CLASS"),
            Line::new("OPTIONS"),
            Line::new("END GAME"),
            Line::new("DEBUG"),
        ];
        lines[0].enabled = classes.is_some();
        match list(
            gfx,
            input,
            &page,
            "OPTIONS",
            &lines,
            sel,
            "A SELECT   B RESUME",
        ) {
            Step::Accept(0) => {
                if let Some(c) = classes
                    && let Some(i) = pick_class(
                        gfx,
                        input,
                        cat,
                        "CHOOSE CLASS",
                        c,
                        class_cursor,
                        Some(&grabbed),
                    )
                {
                    return Pause::ChangeClass(i);
                }
            }
            Step::Accept(1) => return Pause::Options,
            Step::Accept(2) => {
                if confirm(gfx, input, &mw2("MPUI_END_GAME2", "End game?"), &grabbed) {
                    return Pause::EndGame;
                }
            }
            Step::Accept(3) => return Pause::Debug,
            Step::Back => return Pause::Resume,
            _ => {}
        }
    }
}

/// The end-of-match scoreboard on MW2's scoreboard page (its frame and column headers) over the last frame of the
/// match, until A or B. False when the pack has no scoreboard page (the caller shows a plain list).
pub fn scoreboard(gfx: &mut Shim, input: &mut Input, title: &str, rows: &[ScoreRow]) -> bool {
    let has_page = crate::ui::get().is_some_and(|u| u.pack.screen("scoreboard").is_some());
    if !has_page {
        return false;
    }
    let grabbed_ok = grab_screen();
    input.settle();
    let cam = Camera::new([0.0; 3], 0.0, 0.0);
    let footer = [String::from("A CONTINUE")];
    loop {
        let nav = input.poll();
        if nav.accept || nav.back {
            return true;
        }
        unsafe { ffi::mwf_frame_begin(&cam.view(), cam.fovy, 4.0, 100.0, 0x1a1e24ff) };
        if let Some(u) = crate::ui::get() {
            let p = ui_page::Page {
                screen: "scoreboard",
                title,
                lines: &[],
                selected: 0,
                footer: "",
                slots: &footer,
                images: &[],
                inline_values: false,
                backdrop: true,
                preview: "",
            };
            u.page(|ui, q| {
                if grabbed_ok {
                    grabbed(ui, q);
                    q.push(ui_page::fullscreen(ui.white, 0x0000_0080));
                }
                ui_page::layout(ui, &p, q);
                ui_page::score_rows(ui, rows, q);
            });
        }
        gfx.frame_end();
    }
}

/// The debug menu: every setting (A or left / right changes it) and, in a match, the sandbox actions. Returns an
/// action to carry out (the menu closes for it), or None on B. Settings that only apply when a match starts are
/// greyed out in a match. `weapon_name` names catalog weapon `i` of `weapons`.
pub fn debug_menu(
    gfx: &mut Shim,
    input: &mut Input,
    s: &mut game_core::debug::DebugSettings,
    weapon_name: &dyn Fn(u8) -> Option<String>,
    weapons: u8,
    in_match: bool,
    sel: &mut usize,
) -> Option<game_core::debug::Action> {
    use game_core::debug::{ACTIONS, ITEMS};
    let page = Page {
        screen: "rules",
        values: true,
        under: in_match.then_some(&grabbed as Layer<'_>),
        ..Page::default()
    };
    loop {
        let mut lines: Vec<Line> = ITEMS
            .iter()
            .map(|&it| Line {
                text: String::from(it.label()),
                value: s.value(it, weapon_name),
                enabled: !(in_match && it.at_start_only()),
            })
            .collect();
        if in_match {
            lines.extend(ACTIONS.iter().map(|a| Line::new(a.label())));
        }
        match list(gfx, input, &page, "DEBUG", &lines, sel, FOOTER_CHANGE) {
            Step::Accept(i) | Step::Adjust(i, _) if i < ITEMS.len() => s.cycle(ITEMS[i], weapons),
            Step::Accept(i) => return ACTIONS.get(i - ITEMS.len()).copied(),
            Step::Adjust(..) => {}
            _ => return None,
        }
    }
}

/// The debug settings in `debug.dat` (the defaults when it isn't there).
pub fn load_debug() -> game_core::debug::DebugSettings {
    let Some(path) = AppPath::new("debug.dat") else {
        return Default::default();
    };
    let mut buf = [0u8; 64];
    let n = unsafe { ffi::mwf_file_read(path.as_ptr(), 0, buf.len() as u32, buf.as_mut_ptr()) };
    if n <= 0 {
        return Default::default();
    }
    game_core::debug::DebugSettings::decode(&buf[..n as usize]).unwrap_or_default()
}

/// Save the debug settings to `debug.dat`; false when the write failed.
pub fn save_debug(s: &game_core::debug::DebugSettings) -> bool {
    let bytes = s.encode();
    let Some(path) = AppPath::new("debug.dat") else {
        return false;
    };
    unsafe {
        ffi::mwf_file_write(path.as_ptr(), bytes.as_ptr(), bytes.len() as u32) == bytes.len() as i32
    }
}

/// MW2's Options > Controls (and the HUD's safe area) on its rules rows: A or left / right changes a line, until
/// B. The caller saves them. `in_match`: over the frozen game.
pub fn options_menu(
    gfx: &mut Shim,
    input: &mut Input,
    o: &mut game_core::options::Options,
    in_match: bool,
) {
    use game_core::options::{ITEMS, Item};
    let page = Page {
        screen: "rules",
        values: true,
        under: in_match.then_some(&grabbed as Layer<'_>),
        ..Page::default()
    };
    let mut sel = 0usize;
    loop {
        let lines: Vec<Line> = ITEMS
            .iter()
            .map(|&it| Line {
                text: String::from(it.label()),
                value: o.value(it),
                enabled: true,
            })
            .collect();
        match list(
            gfx,
            input,
            &page,
            "OPTIONS",
            &lines,
            &mut sel,
            FOOTER_CHANGE,
        ) {
            Step::Accept(i) => o.cycle(ITEMS[i]),
            Step::Adjust(i, d) if ITEMS[i] != Item::Defaults => o.adjust(ITEMS[i], d, false),
            Step::Adjust(..) => {}
            _ => return,
        }
    }
}

/// The options in `options.dat` (the defaults when it isn't there).
pub fn load_options() -> game_core::options::Options {
    let Some(path) = AppPath::new("options.dat") else {
        return Default::default();
    };
    let mut buf = [0u8; 32];
    let n = unsafe { ffi::mwf_file_read(path.as_ptr(), 0, buf.len() as u32, buf.as_mut_ptr()) };
    if n <= 0 {
        return Default::default();
    }
    game_core::options::Options::decode(&buf[..n as usize]).unwrap_or_default()
}

/// Save the options to `options.dat`; false when the write failed.
pub fn save_options(o: &game_core::options::Options) -> bool {
    let bytes = o.encode();
    let Some(path) = AppPath::new("options.dat") else {
        return false;
    };
    unsafe {
        ffi::mwf_file_write(path.as_ptr(), bytes.as_ptr(), bytes.len() as u32) == bytes.len() as i32
    }
}

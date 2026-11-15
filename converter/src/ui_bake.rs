//! Bake MW2's menus for the Wii (`wpk::ui`): the fonts (glyph tables over their atlas textures) and screens as
//! textured quads already placed on the 640 × 480 screen, plus each screen's button style.
//!
//! Placement uses IW4L's `hud_iw4::ScreenPlacement` (4:3, the default 0.85 safe area), font choice IW4L's
//! `ui_get_font_handle`. The menus' scripts and expressions don't run on the Wii: a baked screen keeps the items
//! that are always shown, plus those whose visibility only tests for the multiplayer build (`"mp"`); buttons take
//! their look from the menu's first button and drive our own menu flow.

use std::collections::HashMap;

use wpk::ui::{ButtonStyle, Font, Glyph, Label, NONE, OwnerDraw, Quad, Screen, UiPack, label_kind};

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
/// The console's private match: its lobby, the Game Setup popup over it, the map, game mode and rules pages, and
/// the yes / no popup (MW2's End Game question).
pub const SCREENS: [(&str, &str, f32); 12] = [
    ("minimap", "minimap_fullscreen", 0.0),
    ("main", "main_text", 0.0),
    ("class_select", "changeclass", 0.0),
    ("pause", "class", 0.0),
    ("cac", "menu_cac_assault", 0.0),
    ("scoreboard", "page_scoreboard", -272.0),
    ("lobby", "menu_xboxlive_privatelobby", 0.0),
    ("setup", "popup_gamesetup", 0.0),
    ("maps", "settings_map", 0.0),
    ("modes", "settings_gametype", 0.0),
    ("rules", "settings_quick_dm", 0.0),
    ("yesno", "popup_endgame", 0.0),
];

/// MW2's popups size their frame from the number of buttons in a script (their rects have no height): the Game
/// Setup popup's frame covers its title bar and three buttons.
const POPUP_HEIGHT: f32 = 86.0;

/// The SD (480-line) HUD menus, laid out at run time inside the safe area: (our name, MW2 menu).
pub const HUDS: [(&str, &str); 3] = [
    ("weaponbar", "weaponbar_sd"),
    ("scorebar", "scorebar_sd"),
    ("dpad", "dpad_sd"),
];

/// Localized strings the front end shows by key (map names and descriptions are added by prefix).
const STRINGS: [&str; 12] = [
    "MPUI_DEATHMATCH",
    "MPUI_WAR",
    "MPUI_DOMINATION",
    "MENU_KILL_EVERYONE_FIRST_PLAYER",
    "MENU_KILL_PLAYERS_ON_THE_OPPOSING",
    "MENU_CAPTURE_AND_HOLD_THE",
    "MPUI_WINNING_CAPS",
    "MPUI_LOSING_CAPS",
    "MPUI_TIED_CAPS",
    "MPUI_END_GAME2",
    "MENU_YES",
    "MENU_NO",
];

/// Items wider than this (virtual pixels) are layers the menu's scripts scroll across the screen (MW2's cloud
/// overlays); still, they only wash the screen out, and they are the costliest quads to fill.
const SCROLLING_LAYER_W: f32 = 960.0;

/// IW4 window styles: "empty" (text only), "filled" (back colour over the background material, or plain), "shader"
/// (the background material in the fore colour).
const WINDOW_STYLE_EMPTY: i32 = 0;
const WINDOW_STYLE_FILLED: i32 = 1;
const WINDOW_STYLE_SHADER: i32 = 3;
/// The image of a lobby's player row.
const LOBBY_ROW_MATERIAL: &str = "playercard_short_bg";
/// The back button's text (its rect is kept; the front end draws its own hint there).
const BACK_TEXT: &str = "@PLATFORM_BACK_CAPS";
/// IW4 item types "button" and "owner draw".
const ITEM_TYPE_BUTTON: i32 = 1;
const ITEM_TYPE_OWNERDRAW: i32 = 8;
/// HUD images the game draws by name (`hud_iw4` compass pings and the radar sweep, the magazine's rounds), and the
/// lobby's player rows.
pub const ICONS: [&str; 13] = [
    "compassping_player",
    "compassping_friendly_mp",
    "compassping_grenade",
    "compassping_explosion",
    "compassping_enemy",
    "compassping_enemyfiring",
    "compass_radarline",
    "ammo_counter_bullet_mp",
    "ammo_counter_riflebullet_mp",
    "ammo_counter_shotgunshell_mp",
    "ammo_counter_rocket_mp",
    "ammo_counter_beltbullet_mp",
    "playercard_short_bg",
];

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
    let mut owner_draws = Vec::new();
    let mut images: Vec<wpk::ui::ImageSlot> = Vec::new();
    for item in &menu.items {
        // An image chosen from the class (whatever its visibility: MW2 shows a perk's icon either way, Pro or not).
        if let Some(field) = item.material_exp.as_deref().and_then(image_field) {
            let r = place(p, &item.rect);
            if !images
                .iter()
                .any(|i| i.field == field && i.x == r.x && i.y == r.y)
            {
                images.push(wpk::ui::ImageSlot {
                    field,
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    rgba: rgba(item.fore_color),
                });
            }
            continue;
        }
        // The map's preview picture (MW2 builds `preview_<map>` or reads `ui_info_icon` by script): its rect.
        if item.material_exp.as_deref().is_some_and(shows_map_preview) {
            let r = place(p, &item.rect);
            owner_draws.push(OwnerDraw {
                od: wpk::ui::od::MAP_PREVIEW,
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                // Its place in MW2's item order: the screen's images drawn before it.
                texture: quads.len() as u16,
                rgba: rgba(item.fore_color),
            });
            continue;
        }
        // The lobby's player rows (shown per player by script): their rects for the front end's list.
        if material(&item.background) == LOBBY_ROW_MATERIAL && item.style == WINDOW_STYLE_SHADER {
            let r = place(p, &item.rect);
            owner_draws.push(OwnerDraw {
                od: wpk::ui::od::LOBBY_ROW,
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                texture: tex(LOBBY_ROW_MATERIAL).unwrap_or(NONE),
                rgba: rgba(item.fore_color),
            });
            continue;
        }
        if item.item_type == ITEM_TYPE_OWNERDRAW && shown(item) && item.owner_draw > 0 {
            let r = place(p, &item.rect);
            let texture = if item.background.is_empty() {
                NONE
            } else {
                tex(material(&item.background)).unwrap_or(NONE)
            };
            owner_draws.push(OwnerDraw {
                od: item.owner_draw as u16,
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                texture,
                rgba: rgba(item.fore_color),
            });
            continue;
        }
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
        // MW2's "black" panels: the white image in black.
        let (material_name, colour) = if material_name == "black" {
            ("white", [0.0, 0.0, 0.0, colour[3]])
        } else {
            (material_name, colour)
        };
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
        owner_draws,
        images,
    }
}

/// The strings of an expression dump (`s:<hex>` tokens).
fn dump_strings(dump: &str) -> Vec<String> {
    dump.split_whitespace()
        .filter_map(|t| t.strip_prefix("s:"))
        .filter_map(|h| {
            let bytes: Option<Vec<u8>> = (0..h.len() / 2)
                .map(|i| u8::from_str_radix(h.get(i * 2..i * 2 + 2)?, 16).ok())
                .collect();
            String::from_utf8(bytes?).ok()
        })
        .collect()
}

/// Whether a material expression shows the map's preview: `"preview_" + mapname` (the lobby) or the map
/// table's `ui_info_icon` (the map page).
fn shows_map_preview(dump: &str) -> bool {
    dump_strings(dump)
        .iter()
        .any(|s| s.eq_ignore_ascii_case("preview_") || s.eq_ignore_ascii_case("ui_info_icon"))
}

/// The loadout field an item's material expression shows: MW2's
/// `tablelookup("<table>", <key column>, dvarString("loadout<...>"), <image column>)`, as its dump spells it
/// (`op 16 op 74 s:<table> op 17 <n> op 17 op 81 s:<dvar> op 1 op 17 <n> op 1`). Camouflage and other lookups:
/// `None`.
fn image_field(dump: &str) -> Option<u8> {
    let strings = dump_strings(dump);
    let [table, dvar] = strings.as_slice() else {
        return None;
    };
    let table = table.to_ascii_lowercase();
    if !(table.ends_with("statstable.csv")
        || table.ends_with("attachmenttable.csv")
        || table.ends_with("perktable.csv"))
    {
        return None;
    }
    wpk::ui::image_field::from_dvar(dvar)
}

/// Bake the fonts and screens. `tex(material)` adds that material's colour texture to the pack (`TEXT` index);
/// `localize` gives MW2's strings for the menus' `@KEY` text.
pub fn bake(
    ui: &UiCapture,
    localize: &HashMap<String, String>,
    extra_icons: &[(String, String)],
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
                if ours == "setup" && item.rect.h == 0.0 && item.rect.w != 0.0 {
                    item.rect.h = POPUP_HEIGHT;
                }
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
    let mut icons: Vec<(String, u16)> = ICONS
        .iter()
        .filter_map(|&n| tex(n).map(|t| (n.to_owned(), t)))
        .collect();
    // The HUD's own images (`key` -> material): the hitmarker, weapons' HUD icons, killstreak icons.
    for (key, material) in extra_icons {
        if let Some(t) = tex(material) {
            icons.push((key.clone(), t));
        }
    }
    let hud = HUDS
        .iter()
        .filter_map(|(ours, theirs)| {
            ui.menus
                .get(*theirs)
                .map(|m| bake_hud(ours, m, &p, &fonts, localize, &mut tex))
        })
        .collect();
    let mut strings: Vec<(String, String)> = localize
        .iter()
        .filter(|(k, _)| {
            STRINGS.contains(&k.as_str())
                || k.starts_with("MPUI_DESC_MAP_")
                || k.strip_prefix("MPUI_")
                    .is_some_and(|m| localize.contains_key(&format!("MPUI_DESC_MAP_{m}")))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    strings.sort();
    (
        UiPack {
            fonts,
            screens,
            white,
            icons,
            hud,
            strings,
        },
        report,
    )
}

/// A HUD menu's items in MW2's virtual space with their alignment (placed at run time): images, owner draws and
/// text, whatever their visibility test (the game decides what shows).
fn bake_hud(
    name: &str,
    menu: &Menu,
    p: &hud_iw4::ScreenPlacement,
    fonts: &[Font],
    localize: &HashMap<String, String>,
    tex: &mut impl FnMut(&str) -> Option<u16>,
) -> wpk::ui::HudLayout {
    let mut items = Vec::new();
    for i in &menu.items {
        let material = material(&i.background);
        let texture = if i.style == WINDOW_STYLE_SHADER && !material.is_empty() {
            tex(material).unwrap_or(NONE)
        } else {
            NONE
        };
        let od = if i.item_type == ITEM_TYPE_OWNERDRAW {
            i.owner_draw.max(0) as u16
        } else {
            0
        };
        if texture == NONE && od == 0 && i.text.is_empty() {
            continue;
        }
        items.push(wpk::ui::HudItem {
            od,
            x: i.rect.x,
            y: i.rect.y,
            w: i.rect.w,
            h: i.rect.h,
            halign: i.rect.horz_align,
            valign: i.rect.vert_align,
            font: font_index(p, fonts, i.font, i.text_scale),
            text_scale: i.text_scale,
            align: (i.text_align_mode & 0xf) as u8,
            rgba: rgba(i.fore_color),
            texture,
            // The item's own name (MW2's "Pass1" compass passes are told apart by it), else its image's.
            name: if i.name.is_empty() {
                material.to_owned()
            } else {
                i.name.clone()
            },
            text: if i.text.is_empty() {
                String::new()
            } else {
                text_of(&i.text, localize)
            },
        });
    }
    wpk::ui::HudLayout {
        name: name.to_owned(),
        items,
    }
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
            material_exp: None,
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
        let (pack, r) = bake(&ui, &HashMap::new(), &[], |m| {
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
    fn finds_map_preview_expressions() {
        // `"preview_" + dvarString(...)` and `dvarString("ui_info_icon")`, as IW4L dumps them.
        assert!(shows_map_preview(
            "op 16 s:707265766965775f op 5 op 26 26 op 1"
        ));
        assert!(shows_map_preview(
            "op 16 op 81 s:75695f696e666f5f69636f6e op 1"
        ));
        assert!(!shows_map_preview("op 16 op 81 s:61 op 1"));
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
        let (pack, _) = bake(&ui, &localize, &[], |m| {
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

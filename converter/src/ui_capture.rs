//! MW2's fonts and menus as the zone loader hands them over (IW4L `fastfile_iw4` `capture_font`,
//! `capture_menu_def`, `capture_menu_item_layout`, `capture_item_visible_exp`, `capture_menu_script`), kept as plain
//! data for baking the Wii's menu screens (`ui_bake.rs`). The capture follows IW4L's own menu catalog
//! (`crates/asset_game/src/menu_catalog.rs`, Apache-2.0, rev `acc57a5`): same hooks, same zones.

use std::collections::BTreeMap;

use fastfile_iw4::{
    FontCapture, GlyphCapture, MenuDefCapture, MenuItemLayout, MenuRectCapture, MenuScriptKind,
};

/// The zones IW4L reads MP menus from (`asset_game::menu_catalog::UI_MENU_ZONES`).
pub const UI_ZONES: [&str; 5] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "localized_ui_mp",
    "common_mp",
    "patch_mp",
];

#[derive(Clone, Debug)]
pub struct Font {
    pub name: String,
    pub pixel_height: i32,
    pub material: String,
    pub glyphs: Vec<GlyphCapture>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub horz_align: u8,
    pub vert_align: u8,
}

impl From<MenuRectCapture> for Rect {
    fn from(r: MenuRectCapture) -> Rect {
        Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            horz_align: r.horz_align,
            vert_align: r.vert_align,
        }
    }
}

// Kept whole for the screens still to bake (texts, owner draws, scripts).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    /// Text or localize reference (`@MENU_...`).
    pub text: String,
    pub item_type: i32,
    pub style: i32,
    pub owner_draw: i32,
    pub rect: Rect,
    pub fore_color: [f32; 4],
    pub back_color: [f32; 4],
    pub text_scale: f32,
    pub font: i32,
    pub text_align_mode: i32,
    pub text_align_x: f32,
    pub text_align_y: f32,
    pub text_style: i32,
    pub background: String,
    pub dvar: String,
    pub static_flags: i32,
    /// The visibility expression (IW4L text dump), when the item has one.
    pub visible: Option<String>,
    /// Scripts by kind (action, focus ...).
    pub scripts: Vec<(MenuScriptKind, String)>,
}

// Kept whole for the screens still to bake (texts, owner draws, scripts).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Menu {
    pub name: String,
    pub window_background: String,
    pub fullscreen: i32,
    pub rect: Rect,
    pub focus_color: [f32; 4],
    pub items: Vec<Item>,
    pub scripts: Vec<(MenuScriptKind, String)>,
}

#[derive(Default)]
pub struct UiCapture {
    pub fonts: Vec<Font>,
    /// By name (the first zone that has a menu wins, as zones load in order).
    pub menus: BTreeMap<String, Menu>,
}

impl UiCapture {
    pub fn font(&mut self, rec: &FontCapture<'_>) {
        if self.fonts.iter().any(|f| f.name == rec.name) {
            return;
        }
        let glyphs = rec
            .glyphs
            .chunks_exact(asset_iw4::size::GLYPH)
            .filter_map(|c| c.try_into().ok().map(GlyphCapture::from_row))
            .collect();
        self.fonts.push(Font {
            name: rec.name.to_owned(),
            pixel_height: rec.pixel_height,
            material: rec.material.to_owned(),
            glyphs,
        });
    }

    pub fn menu_def(&mut self, rec: &MenuDefCapture<'_>) {
        self.menus
            .entry(rec.name.to_owned())
            .or_insert_with(|| Menu {
                name: rec.name.to_owned(),
                window_background: rec.window_background.to_owned(),
                fullscreen: rec.fullscreen,
                rect: rec.rect.into(),
                focus_color: rec.focus_color,
                items: Vec::new(),
                scripts: Vec::new(),
            });
    }

    fn menu_mut(&mut self, name: &str) -> &mut Menu {
        self.menus.entry(name.to_owned()).or_insert_with(|| Menu {
            name: name.to_owned(),
            window_background: String::new(),
            fullscreen: 0,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 640.0,
                h: 480.0,
                horz_align: 0,
                vert_align: 0,
            },
            focus_color: [1.0; 4],
            items: Vec::new(),
            scripts: Vec::new(),
        })
    }

    pub fn item(&mut self, rec: &MenuItemLayout<'_>) {
        let m = self.menu_mut(rec.menu);
        m.items.push(Item {
            name: rec.name.to_owned(),
            text: String::from_utf8_lossy(rec.text).into_owned(),
            item_type: rec.item_type,
            style: rec.style,
            owner_draw: rec.owner_draw,
            rect: rec.rect.into(),
            fore_color: rec.fore_color,
            back_color: rec.back_color,
            text_scale: rec.text_scale,
            font: rec.font_enum,
            text_align_mode: rec.text_align_mode,
            text_align_x: rec.text_align_x,
            text_align_y: rec.text_align_y,
            text_style: rec.text_style,
            background: rec.background.to_owned(),
            dvar: rec.dvar.to_owned(),
            static_flags: rec.static_flags,
            visible: None,
            scripts: Vec::new(),
        });
    }

    /// The item named `item` of `menu` (the last one with that name: items are captured in order).
    fn item_mut(&mut self, menu: &str, item: &str) -> Option<&mut Item> {
        self.menus
            .get_mut(menu)?
            .items
            .iter_mut()
            .rev()
            .find(|i| i.name == item)
    }

    pub fn visible(&mut self, menu: &str, item: &str, dump: &str) {
        if let Some(i) = self.item_mut(menu, item) {
            i.visible = Some(dump.to_owned());
        }
    }

    pub fn script(&mut self, menu: &str, item: &str, kind: MenuScriptKind, script: &str) {
        if item.is_empty() {
            self.menu_mut(menu).scripts.push((kind, script.to_owned()));
        } else if let Some(i) = self.item_mut(menu, item) {
            i.scripts.push((kind, script.to_owned()));
        }
    }

    /// Add another zone's capture (fonts and menus already here win).
    pub fn absorb(&mut self, other: UiCapture) {
        for f in other.fonts {
            if !self.fonts.iter().any(|x| x.name == f.name) {
                self.fonts.push(f);
            }
        }
        for (k, v) in other.menus {
            self.menus.entry(k).or_insert(v);
        }
    }
}

//! `mwf-convert menu-preview <common.wpk>`: software renders of the baked menu screens with sample content, laid out
//! by the same code as the Wii's front end (`game_core::ui_page`), to check the menus without a Wii. Output: one
//! 640 × 480 PNG per screen.

use game_core::loadout::{self, FIELDS};
use game_core::ui_page::{self, Page, PageLine, ScoreRow};
use gx_texture::image::Image;
use wpk::classes::Catalog;
use wpk::records::{self, TextureRecord};
use wpk::ui::{Quad, UiPack};

/// The screens rendered (the names `ui_bake::SCREENS` gives them).
pub const SCREENS: [&str; 5] = ["main", "class_select", "pause", "cac", "scoreboard"];

/// Draw `q` over `img` (nearest texel, texture × colour, alpha blended).
fn draw(img: &mut Image, textures: &[Option<Image>], q: &Quad) {
    let Some(Some(t)) = textures.get(usize::from(q.texture)) else {
        return;
    };
    let c = [
        (q.rgba >> 24) as f32 / 255.0,
        ((q.rgba >> 16) & 0xff) as f32 / 255.0,
        ((q.rgba >> 8) & 0xff) as f32 / 255.0,
        (q.rgba & 0xff) as f32 / 255.0,
    ];
    let (x0, y0) = (q.x.max(0.0) as u32, q.y.max(0.0) as u32);
    let (x1, y1) = (
        (q.x + q.w).clamp(0.0, 640.0) as u32,
        (q.y + q.h).clamp(0.0, 480.0) as u32,
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let fx = (x as f32 + 0.5 - q.x) / q.w;
            let fy = (y as f32 + 0.5 - q.y) / q.h;
            let u = q.uv[0] + (q.uv[2] - q.uv[0]) * fx;
            let v = q.uv[1] + (q.uv[3] - q.uv[1]) * fy;
            let tx = ((u.clamp(0.0, 0.9999)) * t.width as f32) as u32;
            let ty = ((v.clamp(0.0, 0.9999)) * t.height as f32) as u32;
            let s = t.pixels[(ty * t.width + tx) as usize];
            let a = s[3] as f32 / 255.0 * c[3];
            let d = &mut img.pixels[(y * 640 + x) as usize];
            for k in 0..3 {
                let src = s[k] as f32 * c[k];
                d[k] = (src * a + d[k] as f32 * (1.0 - a)).clamp(0.0, 255.0) as u8;
            }
            d[3] = 255;
        }
    }
}

/// Draw a quad whose corners carry their own texture coordinates (`v`: top left, top right, bottom right, bottom
/// left of an axis-aligned rect), sampling bilinearly between them.
fn draw_verts(img: &mut Image, t: &Image, v: &[game_core::minimap::Vert; 4]) {
    let (x0, y0, x1, y1) = (v[0].x, v[0].y, v[2].x, v[2].y);
    for y in (y0.max(0.0) as u32)..(y1.clamp(0.0, 480.0) as u32) {
        for x in (x0.max(0.0) as u32)..(x1.clamp(0.0, 640.0) as u32) {
            let fx = (x as f32 + 0.5 - x0) / (x1 - x0);
            let fy = (y as f32 + 0.5 - y0) / (y1 - y0);
            let lerp = |a: f32, b: f32, f: f32| a + (b - a) * f;
            let top = [lerp(v[0].s, v[1].s, fx), lerp(v[0].t, v[1].t, fx)];
            let bot = [lerp(v[3].s, v[2].s, fx), lerp(v[3].t, v[2].t, fx)];
            let (s, tt) = (lerp(top[0], bot[0], fy), lerp(top[1], bot[1], fy));
            let tx = (s.clamp(0.0, 0.9999) * t.width as f32) as u32;
            let ty = (tt.clamp(0.0, 0.9999) * t.height as f32) as u32;
            let p = t.pixels[(ty * t.width + tx) as usize];
            let a = f32::from(p[3]) / 255.0;
            let d = &mut img.pixels[(y * 640 + x) as usize];
            for k in 0..3 {
                d[k] = (f32::from(p[k]) * a + f32::from(d[k]) * (1.0 - a)) as u8;
            }
        }
    }
}

/// The HUD minimap of map pack `map` for a player at its first free-for-all spawn, with an enemy firing ahead and
/// one marked by the radar, over a grey background.
pub fn render_minimap(common: &[u8], map: &[u8]) -> Result<Image, String> {
    let pack = wpk::Pack::parse(common, true).map_err(|e| format!("{e:?}"))?;
    let ui = pack
        .section(wpk::tag::UI)
        .and_then(|s| UiPack::decode(s.data))
        .ok_or("no menu data in this pack")?;
    let txda = pack
        .section(wpk::tag::TEXTURE_DATA)
        .map_or(&[][..], |s| s.data);
    let textures: Vec<Option<Image>> = pack
        .section(wpk::tag::TEXTURES)
        .map(|s| {
            records::iter::<TextureRecord>(s.data)
                .map(|r| crate::preview::decode_texture(&r, txda))
                .collect()
        })
        .unwrap_or_default();
    let mp = wpk::Pack::parse(map, true).map_err(|e| format!("{e:?}"))?;
    let m = mp
        .section(wpk::tag::MINIMAP)
        .and_then(|s| records::get::<records::Minimap>(s.data, 0))
        .ok_or("this map pack has no minimap (convert again)")?;
    let map_txda = mp
        .section(wpk::tag::TEXTURE_DATA)
        .map_or(&[][..], |s| s.data);
    let map_tex = mp
        .section(wpk::tag::TEXTURES)
        .and_then(|s| records::get::<TextureRecord>(s.data, usize::from(m.texture)))
        .and_then(|r| crate::preview::decode_texture(&r, map_txda))
        .ok_or("minimap texture unreadable")?;
    let spawn = mp
        .section(wpk::tag::SPAWNS)
        .and_then(|s| {
            records::iter::<records::SpawnRecord>(s.data)
                .find(|s| s.kind == records::spawn_kind::DM)
        })
        .ok_or("no spawns")?;
    let screen = ui.screen("minimap").ok_or("no minimap layout")?;
    let rect = screen
        .owner_draw(wpk::ui::od::MINIMAP_MAP)
        .map(|o| [o.x, o.y, o.w, o.h])
        .ok_or("no minimap rect")?;
    // Shown four times larger than on screen, so the map can be checked.
    let big = [20.0, 20.0, rect[2] * 4.0, rect[3] * 4.0];
    let f = game_core::minimap::Frame::new(&m, big, spawn.origin, spawn.yaw)
        .ok_or("bad minimap frame")?;
    let mut img = Image::new(640, 480, [60, 60, 60, 255]);
    draw_verts(&mut img, &map_tex, &f.map_quad(0xffffffff));
    let r = spawn.yaw.to_radians();
    let ahead = [
        spawn.origin[0] + r.cos() * 800.0,
        spawn.origin[1] + r.sin() * 800.0,
        spawn.origin[2],
    ];
    let mut quads = Vec::new();
    for (name, at) in [
        ("compassping_enemyfiring", ahead),
        (
            "compassping_enemy",
            [spawn.origin[0] + 400.0, spawn.origin[1] + 400.0, 0.0],
        ),
    ] {
        if let Some(t) = ui.icon(name) {
            let mut q = game_core::minimap::Frame::icon(
                f.to_screen(at),
                game_core::minimap::Frame::ENEMY_SIZE,
                t,
                0xffffffff,
            );
            q.x -= q.w * 1.5;
            q.y -= q.h * 1.5;
            q.w *= 4.0;
            q.h *= 4.0;
            quads.push(q);
        }
    }
    if let Some(t) = ui.icon("compassping_player") {
        let mut q = f.player_icon(t);
        q.x -= q.w * 1.5;
        q.y -= q.h * 1.5;
        q.w *= 4.0;
        q.h *= 4.0;
        quads.push(q);
    }
    for q in &quads {
        draw(&mut img, &textures, q);
    }
    Ok(img)
}

fn lines<'a>(texts: &'a [String], values: &'a [Option<String>]) -> Vec<PageLine<'a>> {
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| PageLine {
            text: t,
            value: values.get(i).and_then(|v| v.as_deref()),
            enabled: true,
        })
        .collect()
}

/// Render `screen` with sample content (the pack's own classes where the screen shows them).
pub fn render(common: &[u8], screen: &str) -> Result<Image, String> {
    let pack = wpk::Pack::parse(common, true).map_err(|e| format!("{e:?}"))?;
    let ui = pack
        .section(wpk::tag::UI)
        .and_then(|s| UiPack::decode(s.data))
        .ok_or("no menu data in this pack (convert again)")?;
    let cat = pack
        .section(wpk::tag::CLASSES)
        .and_then(|s| Catalog::decode(s.data).ok());
    let txda = pack
        .section(wpk::tag::TEXTURE_DATA)
        .map_or(&[][..], |s| s.data);
    let textures: Vec<Option<Image>> = pack
        .section(wpk::tag::TEXTURES)
        .map(|s| {
            records::iter::<TextureRecord>(s.data)
                .map(|r| crate::preview::decode_texture(&r, txda))
                .collect()
        })
        .unwrap_or_default();
    let classes: Vec<wpk::classes::Loadout> = cat
        .as_ref()
        .map(|c| c.customs.iter().chain(&c.defaults).cloned().collect())
        .unwrap_or_default();
    let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<String>>();
    let mut slots = Vec::new();
    let mut quads = Vec::new();
    let (title, texts, values, footer) = match screen {
        "class_select" => {
            if let (Some(c), Some(l)) = (&cat, classes.first()) {
                loadout::panel(c, l, &mut slots);
            }
            let names = classes.iter().map(|c| c.name.clone()).collect();
            ("CHOOSE CLASS", names, Vec::new(), "A CHOOSE   B BACK")
        }
        "pause" => {
            let rules = pack
                .section(wpk::tag::RULES)
                .and_then(|s| wpk::rules::Rules::decode(s.data))
                .unwrap_or_default();
            slots.push("RUST".to_owned());
            slots.push(format!(
                "FREE-FOR-ALL: FIRST TO {} POINTS",
                rules.score_limit
            ));
            (
                "PAUSED",
                s(&["RESUME", "CHANGE CLASS", "END GAME"]),
                Vec::new(),
                "A SELECT   B RESUME",
            )
        }
        "cac" => match (&cat, classes.first()) {
            (Some(c), Some(l)) => {
                loadout::panel(c, l, &mut slots);
                let texts = FIELDS
                    .iter()
                    .map(|&f| loadout::label(f).to_owned())
                    .collect();
                let values = FIELDS
                    .iter()
                    .map(|&f| Some(loadout::value(c, l, f)))
                    .collect();
                ("EDIT CLASS", texts, values, "A CHANGE   B DONE")
            }
            _ => ("EDIT CLASS", Vec::new(), Vec::new(), "A CHANGE   B DONE"),
        },
        "scoreboard" => {
            slots.push("A CONTINUE".to_owned());
            ("YOU WIN", Vec::new(), Vec::new(), "")
        }
        _ => (
            "MODERN WIIFARE",
            s(&["PLAY", "CREATE A CLASS", "QUIT"]),
            Vec::new(),
            "A SELECT",
        ),
    };
    let page_lines = lines(&texts, &values);
    ui_page::layout(
        &ui,
        &Page {
            screen,
            title,
            lines: &page_lines,
            selected: 0,
            footer,
            slots: &slots,
        },
        &mut quads,
    );
    if screen == "scoreboard" {
        let rows: Vec<ScoreRow> = ["YOU", "BOT 1", "BOT 2", "BOT 3", "BOT 4", "BOT 5"]
            .iter()
            .enumerate()
            .map(|(i, n)| ScoreRow {
                name: (*n).to_owned(),
                cols: [(30 - i as i32 * 4) * 50, 30 - i as i32 * 4, 0, 5 + i as i32],
                you: i == 0,
            })
            .collect();
        ui_page::score_rows(&ui, &rows, &mut quads);
    }
    let mut img = Image::new(640, 480, [26, 30, 36, 255]);
    for q in &quads {
        draw(&mut img, &textures, q);
    }
    Ok(img)
}

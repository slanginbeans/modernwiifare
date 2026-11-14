//! The menus' look (`UIPK` in `common.wpk`): MW2's fonts (glyph tables over atlas textures in `TEXT`) and baked
//! screens: textured quads already placed on the 640 × 480 screen, and the style of a screen's button list (position,
//! spacing, font, text placement, the selection bar). Built at convert time from the user's own menus
//! (`converter/src/ui_bake.rs`); drawn by the Wii's front end.

use alloc::string::String;
use alloc::vec::Vec;

use crate::be::{self, Push};

pub const NONE: u16 = 0xFFFF;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glyph {
    pub letter: u16,
    /// Offset of the glyph's top left from the pen (font pixels), and the advance.
    pub x0: i8,
    pub y0: i8,
    pub dx: u8,
    pub w: u8,
    pub h: u8,
    /// Atlas coordinates (0..1).
    pub s0: f32,
    pub t0: f32,
    pub s1: f32,
    pub t1: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Font {
    pub name: String,
    pub pixel_height: u16,
    /// Atlas texture (index into `TEXT`).
    pub texture: u16,
    /// Sorted by letter.
    pub glyphs: Vec<Glyph>,
}

impl Font {
    pub fn glyph(&self, letter: u16) -> Option<&Glyph> {
        self.glyphs
            .binary_search_by_key(&letter, |g| g.letter)
            .ok()
            .map(|i| &self.glyphs[i])
    }
}

/// A textured rectangle on the 640 × 480 screen (negative sizes flip the texture, as MW2's mirrored items do).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub uv: [f32; 4],
    pub rgba: u32,
    pub texture: u16,
}

/// How a screen lays out its buttons (from the menu's first button and the gap to the next).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ButtonStyle {
    /// First button's rect (screen pixels) and the step to the next.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub step: f32,
    /// Font index into `UiPack::fonts`, MW2 text scale, text x inside the rect, alignment (0 left, 1 centre, 2 right).
    pub font: u8,
    pub text_scale: f32,
    pub text_x: f32,
    pub align: u8,
    pub rgba: u32,
    pub focus_rgba: u32,
    /// The focused button's bar (texture into `TEXT`, or `NONE`).
    pub bar: u16,
    pub bar_rgba: u32,
}

/// A text item of a menu, placed on the screen: MW2's own text (`text`), or an empty slot the front end fills with
/// live text (the class panel's weapon and perk names, the map name).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Label {
    /// The item's rect (screen pixels).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub font: u8,
    pub text_scale: f32,
    /// IW4 text alignment: bits 0..1 horizontal (0 left, 1 centre, 2 right), bits 2..3 vertical.
    pub align: u8,
    pub text_x: f32,
    pub text_y: f32,
    pub rgba: u32,
    pub text: String,
}

/// What a label is for on its screen.
pub mod label_kind {
    /// MW2's text, drawn as is.
    pub const TEXT: u8 = 0;
    /// The page title (the front end may draw its own title in it).
    pub const TITLE: u8 = 1;
    /// An empty slot for live text.
    pub const SLOT: u8 = 2;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Screen {
    pub name: String,
    pub quads: Vec<Quad>,
    pub buttons: Option<ButtonStyle>,
    /// Labels with their kind (`label_kind`), in menu order.
    pub labels: Vec<(u8, Label)>,
    /// The back button's rect (bottom left in MW2's menus), when the menu has one.
    pub back: Option<[f32; 4]>,
    /// Items the game draws itself (IW4 owner draws: the minimap's map, player and enemies ...).
    pub owner_draws: Vec<OwnerDraw>,
}

/// An owner-draw item: its IW4 owner-draw number, rect (screen pixels), material (texture or `NONE`) and colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OwnerDraw {
    pub od: u16,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub texture: u16,
    pub rgba: u32,
}

/// IW4 owner-draw numbers of the HUD minimap (`minimap_fullscreen`).
pub mod od {
    pub const MINIMAP_PLAYER: u16 = 150;
    pub const MINIMAP_MAP: u16 = 159;
    pub const MINIMAP_ENEMIES: u16 = 175;
}

impl Screen {
    pub fn title(&self) -> Option<&Label> {
        self.labels
            .iter()
            .find(|(k, _)| *k == label_kind::TITLE)
            .map(|(_, l)| l)
    }

    /// The `n`th empty slot.
    pub fn owner_draw(&self, od: u16) -> Option<&OwnerDraw> {
        self.owner_draws.iter().find(|o| o.od == od)
    }

    pub fn slot(&self, n: usize) -> Option<&Label> {
        self.labels
            .iter()
            .filter(|(k, _)| *k == label_kind::SLOT)
            .nth(n)
            .map(|(_, l)| l)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiPack {
    pub fonts: Vec<Font>,
    pub screens: Vec<Screen>,
    /// The plain white texture (`TEXT` index or `NONE`), for filled rects.
    pub white: u16,
    /// Named HUD images (the compass pings), by material name.
    pub icons: Vec<(String, u16)>,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(be::u16(self.take(2)?, 0))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(be::u32(self.take(4)?, 0))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(be::f32(self.take(4)?, 0))
    }
    fn str(&mut self) -> Option<String> {
        let n = usize::from(self.u8()?);
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

fn put_str(o: &mut Vec<u8>, s: &str) {
    let mut n = s.len().min(255);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    o.push(n as u8);
    o.extend_from_slice(&s.as_bytes()[..n]);
}

fn put_style(o: &mut Vec<u8>, s: &ButtonStyle) {
    for v in [s.x, s.y, s.w, s.h, s.step] {
        o.f32(v);
    }
    o.push(s.font);
    o.f32(s.text_scale);
    o.f32(s.text_x);
    o.push(s.align);
    o.u32(s.rgba);
    o.u32(s.focus_rgba);
    o.u16(s.bar);
    o.u32(s.bar_rgba);
}

fn put_label(o: &mut Vec<u8>, kind: u8, l: &Label) {
    o.push(kind);
    for v in [l.x, l.y, l.w, l.h] {
        o.f32(v);
    }
    o.push(l.font);
    o.f32(l.text_scale);
    o.push(l.align);
    o.f32(l.text_x);
    o.f32(l.text_y);
    o.u32(l.rgba);
    put_str(o, &l.text);
}

fn get_label(r: &mut Reader<'_>) -> Option<(u8, Label)> {
    let kind = r.u8()?;
    Some((
        kind,
        Label {
            x: r.f32()?,
            y: r.f32()?,
            w: r.f32()?,
            h: r.f32()?,
            font: r.u8()?,
            text_scale: r.f32()?,
            align: r.u8()?,
            text_x: r.f32()?,
            text_y: r.f32()?,
            rgba: r.u32()?,
            text: r.str()?,
        },
    ))
}

fn get_style(r: &mut Reader<'_>) -> Option<ButtonStyle> {
    Some(ButtonStyle {
        x: r.f32()?,
        y: r.f32()?,
        w: r.f32()?,
        h: r.f32()?,
        step: r.f32()?,
        font: r.u8()?,
        text_scale: r.f32()?,
        text_x: r.f32()?,
        align: r.u8()?,
        rgba: r.u32()?,
        focus_rgba: r.u32()?,
        bar: r.u16()?,
        bar_rgba: r.u32()?,
    })
}

impl UiPack {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.u16(self.fonts.len() as u16);
        for f in &self.fonts {
            put_str(&mut o, &f.name);
            o.u16(f.pixel_height);
            o.u16(f.texture);
            o.u16(f.glyphs.len() as u16);
            for g in &f.glyphs {
                o.u16(g.letter);
                o.push(g.x0 as u8);
                o.push(g.y0 as u8);
                o.push(g.dx);
                o.push(g.w);
                o.push(g.h);
                o.push(0);
                for v in [g.s0, g.t0, g.s1, g.t1] {
                    o.f32(v);
                }
            }
        }
        o.u16(self.screens.len() as u16);
        for s in &self.screens {
            put_str(&mut o, &s.name);
            o.u16(s.quads.len() as u16);
            for q in &s.quads {
                for v in [q.x, q.y, q.w, q.h] {
                    o.f32(v);
                }
                for v in q.uv {
                    o.f32(v);
                }
                o.u32(q.rgba);
                o.u16(q.texture);
            }
            match &s.buttons {
                Some(b) => {
                    o.push(1);
                    put_style(&mut o, b);
                }
                None => o.push(0),
            }
            o.u16(s.labels.len() as u16);
            for (k, l) in &s.labels {
                put_label(&mut o, *k, l);
            }
            match s.back {
                Some(b) => {
                    o.push(1);
                    for v in b {
                        o.f32(v);
                    }
                }
                None => o.push(0),
            }
            o.u16(s.owner_draws.len() as u16);
            for d in &s.owner_draws {
                o.u16(d.od);
                for v in [d.x, d.y, d.w, d.h] {
                    o.f32(v);
                }
                o.u16(d.texture);
                o.u32(d.rgba);
            }
        }
        o.u16(self.white);
        o.u16(self.icons.len() as u16);
        for (n, t) in &self.icons {
            put_str(&mut o, n);
            o.u16(*t);
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<UiPack> {
        let mut r = Reader { b, at: 0 };
        let nf = r.u16()?;
        let mut fonts = Vec::with_capacity(usize::from(nf));
        for _ in 0..nf {
            let name = r.str()?;
            let pixel_height = r.u16()?;
            let texture = r.u16()?;
            let ng = r.u16()?;
            let mut glyphs = Vec::with_capacity(usize::from(ng));
            for _ in 0..ng {
                let letter = r.u16()?;
                let x0 = r.u8()? as i8;
                let y0 = r.u8()? as i8;
                let dx = r.u8()?;
                let w = r.u8()?;
                let h = r.u8()?;
                r.u8()?;
                glyphs.push(Glyph {
                    letter,
                    x0,
                    y0,
                    dx,
                    w,
                    h,
                    s0: r.f32()?,
                    t0: r.f32()?,
                    s1: r.f32()?,
                    t1: r.f32()?,
                });
            }
            fonts.push(Font {
                name,
                pixel_height,
                texture,
                glyphs,
            });
        }
        let ns = r.u16()?;
        let mut screens = Vec::with_capacity(usize::from(ns));
        for _ in 0..ns {
            let name = r.str()?;
            let nq = r.u16()?;
            let mut quads = Vec::with_capacity(usize::from(nq));
            for _ in 0..nq {
                quads.push(Quad {
                    x: r.f32()?,
                    y: r.f32()?,
                    w: r.f32()?,
                    h: r.f32()?,
                    uv: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
                    rgba: r.u32()?,
                    texture: r.u16()?,
                });
            }
            let buttons = match r.u8()? {
                1 => Some(get_style(&mut r)?),
                _ => None,
            };
            let nl = r.u16()?;
            let mut labels = Vec::with_capacity(usize::from(nl));
            for _ in 0..nl {
                labels.push(get_label(&mut r)?);
            }
            let back = match r.u8()? {
                1 => Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?]),
                _ => None,
            };
            let nd = r.u16()?;
            let mut owner_draws = Vec::with_capacity(usize::from(nd));
            for _ in 0..nd {
                owner_draws.push(OwnerDraw {
                    od: r.u16()?,
                    x: r.f32()?,
                    y: r.f32()?,
                    w: r.f32()?,
                    h: r.f32()?,
                    texture: r.u16()?,
                    rgba: r.u32()?,
                });
            }
            screens.push(Screen {
                name,
                quads,
                buttons,
                labels,
                back,
                owner_draws,
            });
        }
        let white = r.u16()?;
        let ni = r.u16()?;
        let mut icons = Vec::with_capacity(usize::from(ni));
        for _ in 0..ni {
            icons.push((r.str()?, r.u16()?));
        }
        Some(UiPack {
            fonts,
            screens,
            white,
            icons,
        })
    }

    /// The texture of HUD image `name`.
    pub fn icon(&self, name: &str) -> Option<u16> {
        self.icons
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, t)| *t)
    }

    pub fn screen(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.name == name)
    }

    pub fn font(&self, name: &str) -> Option<usize> {
        self.fonts
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trips() {
        let p = UiPack {
            fonts: vec![Font {
                name: "fonts/test".into(),
                pixel_height: 20,
                texture: 3,
                glyphs: vec![
                    Glyph {
                        letter: 65,
                        x0: -1,
                        y0: -14,
                        dx: 12,
                        w: 11,
                        h: 15,
                        s0: 0.1,
                        t0: 0.2,
                        s1: 0.3,
                        t1: 0.4,
                    },
                    Glyph {
                        letter: 66,
                        ..Glyph::default()
                    },
                ],
            }],
            screens: vec![Screen {
                name: "main".into(),
                quads: vec![Quad {
                    x: -10.0,
                    y: 0.0,
                    w: 660.0,
                    h: 480.0,
                    uv: [0.0, 0.0, 1.0, 1.0],
                    rgba: 0xffffff80,
                    texture: 1,
                }],
                buttons: Some(ButtonStyle {
                    x: 40.0,
                    y: 60.0,
                    w: 300.0,
                    h: 20.0,
                    step: 20.0,
                    font: 0,
                    text_scale: 0.375,
                    text_x: 4.0,
                    align: 2,
                    rgba: 0xffffffff,
                    focus_rgba: 0xffd040ff,
                    bar: 2,
                    bar_rgba: 0xffffffff,
                }),
                labels: vec![
                    (
                        label_kind::TITLE,
                        Label {
                            x: 0.0,
                            y: 0.0,
                            w: 272.0,
                            h: 28.0,
                            font: 1,
                            text_scale: 0.5,
                            align: 6,
                            text_x: -60.0,
                            text_y: 0.0,
                            rgba: 0xffffffff,
                            text: "TITLE".into(),
                        },
                    ),
                    (label_kind::SLOT, Label::default()),
                ],
                back: Some([1.0, 2.0, 3.0, 4.0]),
                owner_draws: vec![OwnerDraw {
                    od: od::MINIMAP_MAP,
                    x: 6.0,
                    w: 100.0,
                    texture: NONE,
                    ..OwnerDraw::default()
                }],
            }],
            white: 5,
            icons: vec![("compassping_player".into(), 7)],
        };
        let back = UiPack::decode(&p.encode()).expect("decodes");
        assert_eq!(back, p);
        assert_eq!(back.fonts[0].glyph(65).map(|g| g.dx), Some(12));
        assert!(back.fonts[0].glyph(67).is_none());
        assert_eq!(
            back.screens[0].title().map(|l| l.text.as_str()),
            Some("TITLE")
        );
        assert!(back.screens[0].slot(0).is_some() && back.screens[0].slot(1).is_none());
        assert_eq!(
            back.screens[0].owner_draw(od::MINIMAP_MAP).map(|o| o.w),
            Some(100.0)
        );
        assert_eq!(back.icon("CompassPing_Player"), Some(7));
        assert_eq!(back.font("FONTS/TEST"), Some(0));
        assert!(UiPack::decode(&p.encode()[..10]).is_none());
    }
}

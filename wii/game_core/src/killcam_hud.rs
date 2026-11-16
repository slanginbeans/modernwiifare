//! The killcam's HUD and the perks' notices, in MW2's console look (MW2's fonts and strings from `common.wpk`'s
//! `UIPK`, placed with IW4L's `hud_iw4::scrplace` inside the safe area as `crate::hud_mw2`):
//!
//! - the killcam (`_killcam.gsc` `initKCElements`): "KILLCAM" (or the final killcam's title) at the top
//!   (`kc_skiptext`'s place: centre, 32 down the adjustable top, `default` at 1.8), the camtime countdown under it
//!   (`kc_timer`: `hudbig`, 42 down, tenths), who killed you with what, the lower message ("Press X to respawn",
//!   `setLowerMessage( "kc_info" )`), and Copycat's offer (`kc_icon` 48 × 48 at 16 right of and 16 below the centre,
//!   `kc_othertext` beside it in `small` at 1.6);
//! - last stand's title and its give-up hint (`notifyMessage` "Last Stand!" / "Final Stand!",
//!   `PLATFORM_COWARDS_WAY_OUT`), One Man Army's "Changing Kit" bar (`omaUseBar`), Painkiller's icon.

use alloc::string::String;
use alloc::vec::Vec;

use wpk::ui::{NONE, Quad, UiPack};

/// IW4 horizontal / vertical alignments (`HORIZONTAL_ALIGN_*` / `VERTICAL_ALIGN_*`): centre (not safe-area
/// adjusted), middle, and the user-adjustable top / centre / bottom.
const H_CENTER: i32 = 2;
const V_MIDDLE: i32 = 2;
const USER_TOP: i32 = 8;
const USER_CENTER: i32 = 9;
const USER_BOTTOM: i32 = 10;
/// IW4 hud elem `fontscale` to a text scale on the 640 × 480 screen (MW2's HUD fonts are 48-pixel fonts drawn at
/// `fontscale × 0.25`... the `default` font at 1.0 is 12 pixels high).
const FONT_SCALE: f32 = 0.25;

const WHITE: u32 = 0xffff_ffff;
/// `kc_timer.color = (0.85,0.85,0.85)`.
const TIMER: u32 = 0xd9d9_d9ff;
const YELLOW: u32 = 0xffd8_40ff;
const RED: u32 = 0xff40_30ff;

/// What the killcam shows this frame.
pub struct Killcam<'a> {
    pub safe_area: f32,
    /// The final killcam (its own title, no skip).
    pub final_kill: bool,
    /// The camtime countdown, tenths of a second.
    pub timer_tenths: u32,
    /// Who killed you, and with what (display names).
    pub killer: &'a str,
    pub weapon: &'a str,
    /// Copycat's offer (`waitKCCopyCatButton`).
    pub copycat: bool,
}

/// What a living player's perk notices show this frame.
pub struct Notices<'a> {
    pub safe_area: f32,
    /// In last stand (Final Stand: `final_stand`) since `ms` ago.
    pub last_stand: Option<(bool, u32)>,
    /// One Man Army changing the class: done of 0..1.
    pub changing_kit: Option<f32>,
    /// Painkiller's time left, ms.
    pub painkiller_ms: Option<u32>,
    /// A notice at the centre (Copycat's class taken ...).
    pub note: Option<&'a str>,
}

/// MW2's string `key` with its binds as the GameCube pad's buttons (`[{+activate}]` X, `[{weapnext}]` Y) and its
/// colour codes dropped; `fallback` without the string.
pub fn string(ui: &UiPack, key: &str, fallback: &str) -> String {
    let s = ui.string(key).unwrap_or(fallback);
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '^' {
            let mut it = rest.chars();
            it.next();
            it.next();
            rest = it.as_str();
            continue;
        }
        if c == '['
            && let Some(end) = rest.find("}]")
        {
            let bind = &rest[1..end + 1];
            out.push_str(match bind.trim_matches(|c| c == '{' || c == '}') {
                "+activate" | "+usereload" => "X",
                "weapnext" => "Y",
                "+melee" => "R3",
                "+gostand" => "A",
                _ => "?",
            });
            rest = &rest[end + 2..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

struct Page<'a> {
    ui: &'a UiPack,
    p: hud_iw4::ScreenPlacement,
    q: &'a mut Vec<Quad>,
}

impl Page<'_> {
    fn at(&self, x: f32, y: f32, h: i32, v: i32) -> [f32; 2] {
        let r = self.p.apply_rect(x, y, 0.0, 0.0, h, v);
        [r.x, r.y]
    }

    /// `s` in `font` at hud elem `fontscale` with its pen's top at `y` (alignX 0 left, 1 centre).
    fn text(&mut self, font: &str, s: &str, at: [f32; 2], fontscale: f32, rgba: u32, center: bool) {
        let ui = self.ui;
        let Some(f) = ui
            .font(font)
            .and_then(|i| ui.fonts.get(i))
            .or_else(|| ui.fonts.first())
        else {
            return;
        };
        let scale = fontscale * FONT_SCALE;
        let w = crate::ui_text::width(f, s, scale);
        let x = if center { at[0] - w * 0.5 } else { at[0] };
        // The baseline under the top (the font's height at this scale).
        let y = at[1] + f32::from(f.pixel_height) * crate::ui_text::glyph_scale(f, scale);
        let shadow = ((rgba & 0xff) * 3 / 4).min(0xc0);
        crate::ui_text::quads(f, s, x + 1.0, y + 1.0, scale, shadow, self.q);
        crate::ui_text::quads(f, s, x, y, scale, rgba, self.q);
    }

    fn icon(&mut self, key: &str, r: [f32; 4], rgba: u32) {
        if let Some(t) = self.ui.icon(key).filter(|&t| t != NONE) {
            self.q.push(Quad {
                x: r[0],
                y: r[1],
                w: r[2],
                h: r[3],
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba,
                texture: t,
            });
        }
    }

    fn bar(&mut self, r: [f32; 4], done: f32) {
        let white = self.ui.icon("white").unwrap_or(NONE);
        let Some(t) = self.ui.icon("ks_button_selected_bar").or(Some(white)) else {
            return;
        };
        if t == NONE {
            return;
        }
        self.q.push(Quad {
            x: r[0],
            y: r[1],
            w: r[2],
            h: r[3],
            uv: [0.0, 0.0, 1.0, 1.0],
            rgba: 0x0000_0090,
            texture: t,
        });
        self.q.push(Quad {
            x: r[0],
            y: r[1],
            w: r[2] * done.clamp(0.0, 1.0),
            h: r[3],
            uv: [0.0, 0.0, 1.0, 1.0],
            rgba: WHITE,
            texture: t,
        });
    }
}

/// The killcam's HUD.
pub fn killcam(ui: &UiPack, s: &Killcam<'_>, q: &mut Vec<Quad>) {
    let mut h = Page {
        ui,
        p: crate::hud_mw2::placement(s.safe_area),
        q,
    };
    let title = if s.final_kill {
        string(ui, "MP_FINAL_KILLCAM", "GAME WINNING KILL")
    } else {
        string(ui, "MP_KILLCAM", "KILLCAM")
    };
    let top = h.at(0.0, 32.0, USER_CENTER, USER_TOP);
    h.text("fonts/normalfont", &title, top, 1.8, WHITE, true);
    // `setTenthsTimer`: seconds and tenths.
    let mut t = String::new();
    let _ = core::fmt::write(
        &mut t,
        format_args!("{}.{}", s.timer_tenths / 10, s.timer_tenths % 10),
    );
    let timer = h.at(0.0, 42.0 + 16.0, USER_CENTER, USER_TOP);
    h.text("fonts/hudbigfont", &t, timer, 1.0, TIMER, true);
    // Who killed you (MW2's killed-by card at the bottom).
    let mut by = String::from(s.killer);
    if !s.weapon.is_empty() {
        by.push_str("  ");
        by.push_str(s.weapon);
    }
    let card = h.at(0.0, -120.0, USER_CENTER, USER_BOTTOM);
    h.text("fonts/normalfont", &by, card, 1.4, YELLOW, true);
    if !s.final_kill {
        let lower = string(ui, "PLATFORM_PRESS_TO_RESPAWN", "Press X to respawn");
        let at = h.at(0.0, -90.0, USER_CENTER, USER_BOTTOM);
        h.text("fonts/normalfont", &lower, at, 1.4, WHITE, true);
    }
    if s.copycat {
        let at = h.at(16.0, 16.0, H_CENTER, V_MIDDLE);
        h.icon(
            "img:perk:specialty_copycat",
            [at[0], at[1], 48.0, 48.0],
            WHITE,
        );
        let text = string(ui, "PLATFORM_PRESS_TO_COPYCAT", "Press Y to copycat");
        let at = h.at(62.0, 18.0, H_CENTER, V_MIDDLE);
        h.text("fonts/smallfont", &text, at, 1.6, WHITE, false);
    }
}

/// The perks' notices over the match HUD.
pub fn notices(ui: &UiPack, s: &Notices<'_>, q: &mut Vec<Quad>) {
    let mut h = Page {
        ui,
        p: crate::hud_mw2::placement(s.safe_area),
        q,
    };
    if let Some((final_stand, since)) = s.last_stand {
        // `notifyMessage` for 2 s, red-glowing; then the give-up hint stays (not in Final Stand).
        if since < 2000 {
            let (key, fallback, icon) = if final_stand {
                (
                    "MPUI_FINAL_STAND",
                    "Final Stand!",
                    "img:perk:specialty_finalstand",
                )
            } else {
                (
                    "MPUI_LAST_STAND",
                    "Last Stand!",
                    "img:perk:specialty_pistoldeath",
                )
            };
            let title = string(ui, key, fallback);
            let at = h.at(0.0, 60.0, USER_CENTER, USER_TOP);
            h.text("fonts/normalfont", &title, at, 1.8, RED, true);
            let at = h.at(-24.0, 90.0, USER_CENTER, USER_TOP);
            h.icon(icon, [at[0], at[1], 48.0, 48.0], WHITE);
        }
        if !final_stand {
            let hint = string(ui, "PLATFORM_COWARDS_WAY_OUT", "Hold X to give up");
            let at = h.at(0.0, -90.0, USER_CENTER, USER_BOTTOM);
            h.text("fonts/normalfont", &hint, at, 1.4, WHITE, true);
        }
    }
    if let Some(done) = s.changing_kit {
        // `createPrimaryProgressBar( 25 )` with its text over it.
        let text = string(ui, "MPUI_CHANGING_KIT", "Changing Kit");
        let at = h.at(0.0, -25.0 - 24.0, USER_CENTER, USER_CENTER);
        h.text("fonts/normalfont", &text, at, 1.4, WHITE, true);
        let bar = h.at(-60.0, -25.0, USER_CENTER, USER_CENTER);
        h.bar([bar[0], bar[1], 120.0, 8.0], done);
    }
    if let Some(left) = s.painkiller_ms {
        // `setCombatHigh`: the icon over a timer, 112 below the centre; it fades over its last 2 s.
        let a = (left as f32 / 2000.0).min(1.0);
        let alpha = (a * 0.85 * 255.0) as u32;
        let at = h.at(-16.0, 112.0 - 40.0, USER_CENTER, USER_CENTER);
        h.icon(
            "img:perk:specialty_painkiller",
            [at[0], at[1], 32.0, 32.0],
            0xffff_ff00 | alpha,
        );
        let mut t = String::new();
        let _ = core::fmt::write(&mut t, format_args!("{}", left.div_ceil(1000)));
        let at = h.at(0.0, 112.0 - 6.0, USER_CENTER, USER_CENTER);
        h.text(
            "fonts/hudsmallfont",
            &t,
            at,
            1.0,
            0xcccc_0000 | alpha.max(1),
            true,
        );
    }
    if let Some(n) = s.note {
        let at = h.at(0.0, 60.0, USER_CENTER, USER_TOP);
        h.text("fonts/normalfont", n, at, 1.6, YELLOW, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::ui::{Font, Glyph};

    fn ui() -> UiPack {
        let g = |letter: u16| Glyph {
            letter,
            x0: 0,
            y0: -10,
            dx: 8,
            w: 8,
            h: 10,
            s0: 0.0,
            t0: 0.0,
            s1: 1.0,
            t1: 1.0,
        };
        UiPack {
            fonts: vec![Font {
                name: "fonts/normalfont".into(),
                pixel_height: 12,
                texture: 1,
                glyphs: (32..127).map(g).collect(),
            }],
            strings: vec![
                ("MP_KILLCAM".into(), "KILLCAM".into()),
                (
                    "PLATFORM_PRESS_TO_SKIP".into(),
                    "Press ^3[{+activate}]^7 to skip.".into(),
                ),
            ],
            ..UiPack::default()
        }
    }

    #[test]
    fn strings_take_the_pads_buttons() {
        let u = ui();
        assert_eq!(string(&u, "PLATFORM_PRESS_TO_SKIP", ""), "Press X to skip.");
        assert_eq!(string(&u, "NOPE", "Fallback"), "Fallback");
    }

    #[test]
    fn the_killcam_hud_has_its_title_timer_and_card() {
        let u = ui();
        let mut q = Vec::new();
        killcam(
            &u,
            &Killcam {
                safe_area: 0.9,
                final_kill: false,
                timer_tenths: 49,
                killer: "BOT 3",
                weapon: "M4A1",
                copycat: false,
            },
            &mut q,
        );
        // Title (7 letters), timer (3), card, lower message, each with a shadow: all on screen.
        assert!(q.len() > 2 * (7 + 3), "{}", q.len());
        assert!(
            q.iter()
                .all(|g| g.x >= 0.0 && g.x < 640.0 && g.y >= 0.0 && g.y < 480.0)
        );
        // The title is near the top, centred.
        let top = q.iter().map(|g| g.y).fold(f32::MAX, f32::min);
        assert!(top < 80.0, "{top}");
        let mut n = Vec::new();
        notices(
            &u,
            &Notices {
                safe_area: 0.9,
                last_stand: Some((false, 500)),
                changing_kit: Some(0.5),
                painkiller_ms: None,
                note: None,
            },
            &mut n,
        );
        assert!(!n.is_empty());
    }
}

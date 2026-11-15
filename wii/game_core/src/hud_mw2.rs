//! The match HUD in MW2's console look, laid out by MW2's own SD-TV HUD menus (`common.wpk`'s `UIPK` HUD layouts
//! `weaponbar` (`weaponbar_sd`), `scorebar` (`scorebar_sd`) and `dpad` (`dpad_sd`)) and placed with IW4L's
//! `hud_iw4::scrplace` inside the player's safe area, so nothing falls off a TV's overscan:
//!
//! - Bottom right, the weapon bar: the magazine as rounds (`hud_iw4::ammo` pips), the reserve, the weapon's name, the
//!   lethal and tactical grenades stacked by count, MW2's reload / low ammo warning at the centre.
//! - Bottom left, the score bar: the clock (amber under a minute, red under 30 s), your score's bar and the leader's
//!   (or the enemy team's) against the score limit, and "Winning" / "Losing" / "Tied" in turn with the game mode
//!   (`hud_iw4::scorebar`).
//! - The d-pad's killstreak, the kill feed over the score bar, the hitmarker and points at the crosshair, notices
//!   at the top.
//!
//! Without the layouts in the pack (an older `common.wpk`) the same items go to fixed places.

use alloc::vec::Vec;

use hud_iw4::ScreenPlacement;
use wpk::ui::{HudItem, NONE, Quad, UiPack, od};

use crate::hud::heapless_str::Str;

/// The weapon in hand.
#[derive(Clone, Copy, Debug)]
pub struct Weapon<'a> {
    /// Script name (`m4_mp`) and display name.
    pub name: &'a str,
    pub display: &'a str,
    pub clip: i32,
    pub stock: i32,
    pub clip_size: i32,
    /// IW4 weapon class (rifle 0, sniper 1, MG 2, SMG 3, spread 4, pistol 5, grenade 6, launcher 7 ...).
    pub weap_class: i32,
}

/// What the HUD shows this frame.
pub struct State<'a> {
    pub now: u32,
    /// Time into the match (the score bar's status cycles with it).
    pub match_ms: u32,
    /// The player's safe area (0.8 .. 1.0 of the screen; MW2's `safeArea_adjusted`).
    pub safe_area: f32,
    pub weapon: Option<Weapon<'a>>,
    /// The lethal and tactical grenades: weapon name and count.
    pub grenades: [Option<(&'a str, i32)>; 2],
    /// Your score (or your team's), the leader's (or the enemy team's), the score limit (0: none).
    pub score: (i32, i32, u32),
    pub teams: bool,
    /// The game mode's name (shown in turn with the standing).
    pub mode_name: &'a str,
    /// Objective status (Domination's flags), over the kill feed.
    pub objectives: &'a str,
    /// Time left, when the match has a time limit.
    pub clock_ms: Option<u32>,
    /// Kill feed lines (newest last) with whether they involve you.
    pub feed: &'a [(&'a str, bool)],
    pub hitmarker: bool,
    /// Points just scored and when.
    pub popup: Option<(u32, u32)>,
    /// The newest killstreak reward ready to call in.
    pub reward: Option<&'a str>,
    /// A notice at the top (killstreak earned, UAV online ...) and when it came.
    pub notice: Option<(&'a str, u32)>,
    /// Dead: seconds to respawn.
    pub respawn_in: Option<u32>,
    /// A nuke's countdown, seconds.
    pub nuke_in: Option<u32>,
}

/// MW2's console default for the HUD's safe area on an SD TV, the share of the screen the HUD keeps inside.
pub const SAFE_AREA_DEFAULT: f32 = 0.9;

const WHITE: u32 = 0xffff_ffff;
const DIM: u32 = 0xd0d8e0e0;
const YELLOW: u32 = 0xffd8_40ff;
const RED: u32 = 0xff50_40ff;
/// IW4 `ALIGN_USER_MIN` / `ALIGN_USER_CENTER` / `ALIGN_USER_MAX`: inside the adjustable safe area.
const USER_MIN: i32 = 8;
const USER_CENTER: i32 = 9;
const USER_MAX: i32 = 10;
/// MW2's low-ammo threshold (`lowAmmoWarningThreshold`'s default).
const LOW_AMMO: f32 = 0.33;

fn rgba_of(c: [f32; 4]) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (b(c[0]) << 24) | (b(c[1]) << 16) | (b(c[2]) << 8) | b(c[3])
}

fn alpha(rgba: u32, a: f32) -> u32 {
    (rgba & 0xffff_ff00) | ((rgba & 0xff) as f32 * a.clamp(0.0, 1.0)) as u32
}

/// Fade from 1 to 0 over the last `fade` of `life` ms after `at`.
fn fade(now: u32, at: u32, life: u32, fade: u32) -> Option<f32> {
    let age = now.wrapping_sub(at);
    (age < life).then(|| ((life - age) as f32 / fade.max(1) as f32).min(1.0))
}

/// The screen placement for a safe area (the title-safe area stays IW4's 0.85).
pub fn placement(safe_area: f32) -> ScreenPlacement {
    let s = safe_area.clamp(0.7, 1.0);
    ScreenPlacement::setup(
        0.0,
        0.0,
        640.0,
        480.0,
        640.0,
        480.0,
        hud_iw4::SAFE_AREA_DEFAULT,
        hud_iw4::SAFE_AREA_DEFAULT,
        s,
        s,
        1.0,
    )
}

struct Hud<'a> {
    ui: &'a UiPack,
    p: ScreenPlacement,
    q: &'a mut Vec<Quad>,
}

impl Hud<'_> {
    /// A virtual rect placed on the screen.
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, ha: i32, va: i32) -> [f32; 4] {
        let r = self.p.apply_rect(x, y, w, h, ha, va);
        [r.x, r.y, r.w, r.h]
    }

    fn item_rect(&self, i: &HudItem) -> [f32; 4] {
        self.rect(i.x, i.y, i.w, i.h, i32::from(i.halign), i32::from(i.valign))
    }

    fn quad(&mut self, r: [f32; 4], uv: [f32; 4], rgba: u32, texture: u16) {
        if texture != NONE {
            self.q.push(Quad {
                x: r[0],
                y: r[1],
                w: r[2],
                h: r[3],
                uv,
                rgba,
                texture,
            });
        }
    }

    fn icon(&mut self, key: &str, r: [f32; 4], rgba: u32) -> bool {
        match self.ui.icon(key) {
            Some(t) => {
                self.quad(r, [0.0, 0.0, 1.0, 1.0], rgba, t);
                true
            }
            None => false,
        }
    }

    /// A weapon's HUD icon: its own, else its base weapon's (`m4_reflex_mp` -> `m4_mp`).
    fn weapon_icon(&mut self, name: &str, r: [f32; 4], rgba: u32) -> bool {
        let mut key = alloc::string::String::from("weapon:");
        key.push_str(name);
        if self.icon(&key, r, rgba) {
            return true;
        }
        key.truncate("weapon:".len());
        key.push_str(name.split('_').next().unwrap_or(name));
        key.push_str("_mp");
        self.icon(&key, r, rgba)
    }

    /// `s` in font `font` (an index, else the named fallback), its pen at `x` (left, centre or right of it by
    /// `align` 0, 1, 2) on baseline `y`.
    #[allow(clippy::too_many_arguments)]
    fn text_in(
        &mut self,
        font: Option<usize>,
        fallback: &str,
        s: &str,
        x: f32,
        y: f32,
        scale: f32,
        rgba: u32,
        align: u8,
    ) {
        let ui = self.ui;
        let Some(f) = font
            .and_then(|i| ui.fonts.get(i))
            .or_else(|| ui.font(fallback).and_then(|i| ui.fonts.get(i)))
            .or_else(|| ui.fonts.first())
        else {
            return;
        };
        let w = crate::ui_text::width(f, s, scale);
        let x = match align {
            1 => x - w * 0.5,
            2 => x - w,
            _ => x,
        };
        // MW2's HUD text is shadowed (`ITEM_TEXTSTYLE_SHADOWED`): readable over bright scenes and backdrops.
        let shadow = ((rgba & 0xff) * 3 / 4).min(0xc0);
        crate::ui_text::quads(f, s, x + 1.0, y + 1.0, scale, shadow, self.q);
        crate::ui_text::quads(f, s, x, y, scale, rgba, self.q);
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, font: &str, s: &str, x: f32, y: f32, scale: f32, rgba: u32, align: u8) {
        self.text_in(None, font, s, x, y, scale, rgba, align);
    }

    /// An owner-draw item's text: in its rect, aligned by its text alignment, on the rect's bottom.
    fn item_text(&mut self, i: &HudItem, s: &str, rgba: u32) {
        let r = self.item_rect(i);
        let (x, align) = match i.align & 3 {
            1 => (r[0] + r[2] * 0.5, 1),
            2 => (r[0] + r[2], 2),
            _ => (r[0], 0),
        };
        self.text_in(
            Some(usize::from(i.font)),
            "fonts/hudsmallfont",
            s,
            x,
            r[1] + r[3],
            i.text_scale,
            rgba,
            align,
        );
    }
}

/// The ammo counter's rounds for an IW4 weapon class (MW2 sets `ammoCounterClip` per weapon; this is each class's).
fn clip_kind(weap_class: i32) -> hud_iw4::AmmoCounterClipKind {
    use hud_iw4::AmmoCounterClipKind as K;
    match weap_class {
        0 | 3 | 5 => K::Magazine,
        1 => K::ShortMagazine,
        2 => K::Beltfed,
        4 => K::Shotgun,
        7 => K::Rocket,
        _ => K::None,
    }
}

/// The weapon bar (bottom right).
fn weaponbar(h: &mut Hud<'_>, s: &State<'_>) {
    let Some(layout) = h.ui.hud("weaponbar") else {
        return weaponbar_plain(h, s);
    };
    let mut frag: Vec<&HudItem> = Vec::new();
    let mut smoke: Vec<&HudItem> = Vec::new();
    let mut stock_done = false;
    for i in &layout.items {
        // The compass passes are an SP leftover (not drawn in MW2's multiplayer).
        if i.name.contains("Pass") {
            continue;
        }
        if i.od == 0 {
            if i.texture != NONE && i.text.is_empty() {
                let r = h.item_rect(i);
                h.quad(r, [0.0, 0.0, 1.0, 1.0], i.rgba, i.texture);
            }
            continue;
        }
        let Some(w) = s.weapon else {
            continue;
        };
        match i.od {
            od::AMMO_STOCK if !stock_done => {
                stock_done = true;
                let mut n = Str::default();
                n.num(w.stock.max(0) as u32);
                h.item_text(i, n.as_str(), if w.stock > 0 { WHITE } else { RED });
            }
            od::WEAPON_NAME => h.item_text(i, w.display, i.rgba),
            od::AMMO_CLIP_GRAPHIC => clip_pips(h, i, &w),
            od::OFFHAND_FRAG_ICON => frag.push(i),
            od::OFFHAND_SMOKE_ICON => smoke.push(i),
            od::LOW_AMMO_WARNING => low_ammo(h, i, &w, s.now),
            _ => {}
        }
    }
    // Grenades: one icon a grenade, the copies stacked right to left (brightest on top), one dim one when out.
    for (slots, g) in [(frag, s.grenades[0]), (smoke, s.grenades[1])] {
        let Some((name, n)) = g else { continue };
        let mut slots = slots;
        slots.sort_by(|a, b| b.x.total_cmp(&a.x));
        let shown = (n.max(1) as usize).min(slots.len());
        for (k, i) in slots.iter().take(shown).enumerate().rev() {
            let r = h.item_rect(i);
            let c = if n > 0 { i.rgba } else { 0xffff_ff50 };
            if !h.weapon_icon(name, r, c) && k == 0 {
                h.text("fonts/hudsmallfont", "G", r[0], r[1] + r[3], 0.3, c, 0);
            }
        }
    }
}

/// The magazine as rounds from the weapon bar's clip graphic point (right-aligned), spent rounds dimmed.
fn clip_pips(h: &mut Hud<'_>, i: &HudItem, w: &Weapon<'_>) {
    use hud_iw4 as ammo;
    let Some(m) = ammo::clip_pip_metrics(clip_kind(w.weap_class)) else {
        return;
    };
    let Some(t) = h.ui.icon(m.image) else {
        // No pip image: the magazine as a number.
        let mut n = Str::default();
        n.num(w.clip.max(0) as u32);
        h.item_text(
            &HudItem {
                text_scale: 0.5,
                align: 2,
                ..i.clone()
            },
            n.as_str(),
            WHITE,
        );
        return;
    };
    let base = h.item_rect(i);
    let sx = h.p.scale_virtual_to_real[0];
    let sy = h.p.scale_virtual_to_real[1];
    let size = w.clip_size.clamp(0, 200);
    let empty = rgba_of([
        ammo::CLIP_PIP_EMPTY_RGB,
        ammo::CLIP_PIP_EMPTY_RGB,
        ammo::CLIP_PIP_EMPTY_RGB,
        ammo::CLIP_PIP_EMPTY_ALPHA,
    ]);
    let scaled = ammo::ClipPipMetrics {
        width: m.width * sx,
        height: m.height * sy,
        step_x: m.step_x * sx,
        step_y: m.step_y * sy,
        ..m
    };
    for k in 0..size {
        let [x, y] = ammo::clip_pip_grid_xy(scaled, [base[0], base[1]], k, i32::from(i.halign));
        let c = if k < w.clip { i.rgba | 0xff } else { empty };
        h.quad(
            [x, y, scaled.width, scaled.height],
            [0.0, 0.0, 1.0, 1.0],
            c,
            t,
        );
    }
}

/// MW2's "Reload" / "Low ammo" / "No ammo" at the centre, pulsing (`hud_iw4::ammo`).
fn low_ammo(h: &mut Hud<'_>, i: &HudItem, w: &Weapon<'_>, now: u32) {
    use hud_iw4::{self as ammo, LowAmmoWarningKind as K};
    if w.clip_size <= 0 || !ammo::check_player_for_low_clip(w.clip, w.clip_size, LOW_AMMO, false) {
        return;
    }
    let Some(kind) = ammo::low_ammo_warning_kind(w.stock, w.clip, w.clip_size) else {
        return;
    };
    let (a, b) = ammo::low_ammo_warning_color_pair(kind);
    let t = ammo::low_ammo_warning_pulse_frac(now as i32) / ammo::LOW_AMMO_WARNING_PULSE_MAX;
    let c = rgba_of(ammo::vec4_lerp(a, b, t));
    let s = match kind {
        K::Reload => "RELOAD",
        K::LowAmmo => "LOW AMMO",
        K::NoAmmo => "NO AMMO",
    };
    let r = h.item_rect(i);
    h.text_in(
        Some(usize::from(i.font)),
        "fonts/hudsmallfont",
        s,
        r[0] + r[2] * 0.5,
        r[1] + 16.0,
        i.text_scale,
        c,
        1,
    );
}

/// The weapon bar's items at fixed places (no layout in the pack).
fn weaponbar_plain(h: &mut Hud<'_>, s: &State<'_>) {
    let Some(w) = s.weapon else {
        return;
    };
    let r = h.rect(-150.0, -60.0, 96.0, 48.0, USER_MAX, USER_MAX);
    h.weapon_icon(w.name, r, WHITE);
    let br = h.rect(-8.0, -12.0, 0.0, 0.0, USER_MAX, USER_MAX);
    let mut n = Str::default();
    n.num(w.clip.max(0) as u32);
    n.push_str(" / ");
    n.num(w.stock.max(0) as u32);
    h.text("fonts/hudbigfont", n.as_str(), br[0], br[1], 0.45, WHITE, 2);
    h.text(
        "fonts/smallfont",
        w.display,
        br[0],
        br[1] - 30.0,
        0.3,
        DIM,
        2,
    );
}

/// The score bar (bottom left): clock, both scores' bars against the limit, the standing in turn with the mode.
fn scorebar(h: &mut Hud<'_>, s: &State<'_>) {
    use hud_iw4 as sb;
    let (mine, other, limit) = s.score;
    let frac = |v: i32| {
        if limit == 0 {
            1.0
        } else {
            sb::scorebar_track_frac(v, limit as i32)
        }
    };
    let rect = |h: &Hud<'_>, r: sb::ScorebarRect| h.rect(r.x, r.y, r.w, r.h, USER_MIN, USER_MAX);
    if let Some(layout) = h.ui.hud("scorebar") {
        for i in &layout.items {
            if i.texture == NONE || !i.text.is_empty() {
                continue;
            }
            let r = h.item_rect(i);
            match i.name.as_str() {
                // The fills follow the scores; their caps sit at the fill's end.
                "hud_scorebar_topbar" | "hud_scorebar_bottombar" => {
                    let f = if i.name.contains("top") {
                        frac(mine)
                    } else {
                        frac(other)
                    };
                    h.quad(
                        [r[0], r[1], r[2] * f, r[3]],
                        [0.0, 0.0, f, 1.0],
                        i.rgba,
                        i.texture,
                    );
                }
                "hud_scorebar_topcap" | "hud_scorebar_bottomcap" => {
                    let (fill, f) = if i.name.contains("top") {
                        (sb::SCOREBAR_TOP_FILL, frac(mine))
                    } else {
                        (sb::SCOREBAR_BOTTOM_FILL, frac(other))
                    };
                    let x = sb::scorebar_fill_cap_x(fill, f) - i.x;
                    let c = h.rect(i.x + x, i.y, i.w, i.h, USER_MIN, USER_MAX);
                    h.quad(c, [0.0, 0.0, 1.0, 1.0], i.rgba, i.texture);
                }
                // The caret marks your team's bar.
                "hud_teamcaret" if !s.teams => {}
                _ => h.quad(r, [0.0, 0.0, 1.0, 1.0], i.rgba, i.texture),
            }
        }
    }
    // The scores beside their bars.
    for (v, bar) in [(mine, sb::SCOREBAR_TOP_BG), (other, sb::SCOREBAR_BOTTOM_BG)] {
        let r = rect(h, bar);
        let mut n = Str::default();
        n.num(v.max(0) as u32);
        h.text(
            "fonts/hudsmallfont",
            n.as_str(),
            r[0] + r[2] + 20.0,
            r[1] + r[3],
            0.3,
            WHITE,
            0,
        );
    }
    // Winning / Losing / Tied, in turn with the mode's name.
    let standing = if mine > other {
        sb::ScorebarStatus::Winning
    } else if mine < other {
        sb::ScorebarStatus::Losing
    } else {
        sb::ScorebarStatus::Tied
    };
    let status = match sb::scorebar_cycle_slot(s.match_ms) {
        sb::ScorebarCycleSlot::Blank => None,
        sb::ScorebarCycleSlot::Gametype => Some(sb::ScorebarStatus::Gametype),
        sb::ScorebarCycleSlot::Standing => Some(standing),
    };
    if let Some(st) = status {
        let label = match st {
            sb::ScorebarStatus::Gametype => alloc::string::String::from(s.mode_name),
            _ => sb::scorebar_status_loc_key(st, "")
                .and_then(|k| h.ui.string(k))
                .map_or_else(
                    || {
                        alloc::string::String::from(match st {
                            sb::ScorebarStatus::Winning => "WINNING",
                            sb::ScorebarStatus::Losing => "LOSING",
                            _ => "TIED",
                        })
                    },
                    alloc::string::String::from,
                ),
        };
        let r = rect(h, sb::SCOREBAR_TOP_BG);
        h.text(
            "fonts/hudsmallfont",
            &label,
            r[0],
            r[1] - 3.0,
            sb::SCOREBAR_STATUS_TEXTSCALE,
            rgba_of(sb::scorebar_status_forecolor(st)),
            0,
        );
    }
    // The clock in its dial.
    if let Some(ms) = s.clock_ms {
        let left = (ms / 1000) as i32;
        let c = sb::scorebar_clock_forecolor(left).unwrap_or([0.85, 0.4, 0.4, 1.0]);
        let (m, sec) = sb::mm_ss_nonneg(ms as i32);
        let mut t = Str::default();
        t.num(m);
        t.push(':');
        if sec < 10 {
            t.push('0');
        }
        t.num(sec);
        let r = rect(h, sb::SCOREBAR_CLOCK);
        h.text(
            "fonts/hudbigfont",
            t.as_str(),
            r[0] + r[2] * 0.5,
            r[1] + r[3] * 0.5 + 7.0,
            0.3,
            rgba_of(c),
            1,
        );
    }
}

/// Lay the HUD out.
pub fn layout(ui: &UiPack, s: &State<'_>, q: &mut Vec<Quad>) {
    let mut h = Hud {
        ui,
        p: placement(s.safe_area),
        q,
    };
    weaponbar(&mut h, s);
    scorebar(&mut h, s);
    // The d-pad's killstreak, ready to call in.
    if let Some(r) = s.reward {
        let rect = ui
            .hud("dpad")
            .and_then(|l| l.items.iter().find(|i| i.od == od::DPAD_KILLSTREAK))
            .map_or_else(
                || h.rect(-61.0, -45.0, 32.0, 32.0, USER_MAX, USER_MAX),
                |i| h.item_rect(i),
            );
        let mut key = alloc::string::String::from("streak:");
        key.push_str(r);
        h.icon(&key, rect, WHITE);
    }
    // Over the score bar: the objectives and the kill feed.
    let mut y = h.rect(0.0, -70.0, 0.0, 0.0, USER_MIN, USER_MAX)[1];
    let x = h.rect(4.0, 0.0, 0.0, 0.0, USER_MIN, USER_MAX)[0];
    if !s.objectives.is_empty() {
        h.text("fonts/hudsmallfont", s.objectives, x, y, 0.3, DIM, 0);
        y -= 16.0;
    }
    for (line, mine) in s.feed.iter().rev() {
        h.text(
            "fonts/hudsmallfont",
            line,
            x,
            y,
            0.3,
            if *mine { YELLOW } else { DIM },
            0,
        );
        y -= 15.0;
    }
    // The crosshair: MW2's hitmarker image, and the points just scored.
    if s.hitmarker {
        h.icon("damage_feedback", [308.0, 228.0, 24.0, 24.0], WHITE);
    }
    if let Some((pts, at)) = s.popup
        && let Some(a) = fade(s.now, at, 1200, 400)
    {
        let mut p = Str::default();
        p.push('+');
        p.num(pts);
        h.text(
            "fonts/hudbigfont",
            p.as_str(),
            340.0,
            214.0,
            0.38,
            alpha(YELLOW, a),
            0,
        );
    }
    // Top centre, inside the safe area: notices and the nuke's countdown.
    let top = h.rect(0.0, 0.0, 0.0, 0.0, USER_CENTER, USER_MIN);
    if let Some((n, at)) = s.notice
        && let Some(a) = fade(s.now, at, 3000, 600)
    {
        h.text(
            "fonts/objectivefont",
            n,
            top[0],
            top[1] + 90.0,
            0.42,
            alpha(YELLOW, a),
            1,
        );
    }
    if let Some(t) = s.nuke_in {
        let mut l = Str::default();
        l.push_str("TACTICAL NUKE: ");
        l.num(t);
        h.text(
            "fonts/hudbigfont",
            l.as_str(),
            top[0],
            top[1] + 60.0,
            0.4,
            RED,
            1,
        );
    }
    // Dead: the respawn countdown.
    if let Some(t) = s.respawn_in {
        h.text(
            "fonts/bigfont",
            "YOU WERE KILLED",
            320.0,
            220.0,
            0.5,
            WHITE,
            1,
        );
        let mut l = Str::default();
        l.push_str("RESPAWNING IN ");
        l.num(t);
        h.text("fonts/normalfont", l.as_str(), 320.0, 250.0, 0.3, DIM, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_safe_area_pulls_the_corners_in() {
        let full = placement(1.0).apply_rect(0.0, 0.0, 0.0, 0.0, USER_MAX, USER_MAX);
        let tv = placement(0.9).apply_rect(0.0, 0.0, 0.0, 0.0, USER_MAX, USER_MAX);
        assert!((full.x - 640.0).abs() < 0.5 && (full.y - 480.0).abs() < 0.5);
        assert!((tv.x - 608.0).abs() < 0.5, "5% in from the right: {}", tv.x);
        assert!(
            (tv.y - 456.0).abs() < 0.5,
            "5% up from the bottom: {}",
            tv.y
        );
    }

    #[test]
    fn classes_pick_their_rounds() {
        use hud_iw4::AmmoCounterClipKind as K;
        assert_eq!(clip_kind(0), K::Magazine);
        assert_eq!(clip_kind(1), K::ShortMagazine);
        assert_eq!(clip_kind(4), K::Shotgun);
        assert_eq!(clip_kind(6), K::None);
    }
}

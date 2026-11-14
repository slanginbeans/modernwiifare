//! The match HUD in MW2's look: its fonts (`fonts/hudbigfont`, `fonts/smallfont` ... from `common.wpk`'s `UIPK`) and
//! its HUD images (each weapon's HUD icon, the killstreak icons, the hitmarker), laid out where MW2's console HUD
//! puts them on a 640 × 480 screen: ammo and weapon bottom right, grenades above them, the score and clock bottom left
//! with the kill feed over them, the hitmarker and score popups at the crosshair, notices at the top.
//!
//! Positions and sizes are this build's (MW2's HUD is laid out by its `hud.menu` scripts, which are not baked yet).

use alloc::vec::Vec;

use crate::hud::heapless_str::Str;
use wpk::ui::{Quad, UiPack};

/// What the HUD shows this frame.
pub struct State<'a> {
    pub now: u32,
    /// The weapon in hand: its script name, its display name, magazine, reserve.
    pub weapon: Option<(&'a str, &'a str, i32, i32)>,
    /// The lethal and tactical grenades: weapon name and count.
    pub grenades: [Option<(&'a str, i32)>; 2],
    /// Score line(s) bottom left (team scores or your score and the leader's).
    pub score: &'a str,
    pub objectives: &'a str,
    /// Match clock, "m:ss".
    pub clock: &'a str,
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

const WHITE: u32 = 0xffff_ffff;
const DIM: u32 = 0xd0d8e0e0;
const YELLOW: u32 = 0xffd8_40ff;
const RED: u32 = 0xff50_40ff;

/// `s` in `font` at MW2 text scale `scale`, its pen at `x` (left, centre or right of it by `align` 0, 1, 2) on
/// baseline `y`.
#[allow(clippy::too_many_arguments)]
fn text(
    ui: &UiPack,
    font: &str,
    s: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: u32,
    align: u8,
    q: &mut Vec<Quad>,
) {
    let Some(f) = ui
        .font(font)
        .and_then(|i| ui.fonts.get(i))
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
    crate::ui_text::quads(f, s, x, y, scale, rgba, q);
}

/// HUD image `key` (a `UIPK` icon) in the rect.
#[allow(clippy::too_many_arguments)]
fn icon(
    ui: &UiPack,
    key: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rgba: u32,
    q: &mut Vec<Quad>,
) -> bool {
    let Some(t) = ui.icon(key) else {
        return false;
    };
    q.push(Quad {
        x,
        y,
        w,
        h,
        uv: [0.0, 0.0, 1.0, 1.0],
        rgba,
        texture: t,
    });
    true
}

#[allow(clippy::too_many_arguments)]
/// A weapon's HUD icon key: its own, else its base weapon's (`m4_reflex_mp` -> `m4_mp`).
fn weapon_icon(
    ui: &UiPack,
    name: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rgba: u32,
    q: &mut Vec<Quad>,
) -> bool {
    let mut key = alloc::string::String::from("weapon:");
    key.push_str(name);
    if icon(ui, &key, x, y, w, h, rgba, q) {
        return true;
    }
    let base = name.split('_').next().unwrap_or(name);
    key.truncate("weapon:".len());
    key.push_str(base);
    key.push_str("_mp");
    icon(ui, &key, x, y, w, h, rgba, q)
}

/// Fade from 1 to 0 over the last `fade` of `life` ms after `at`.
fn fade(now: u32, at: u32, life: u32, fade: u32) -> Option<f32> {
    let age = now.wrapping_sub(at);
    (age < life).then(|| ((life - age) as f32 / fade.max(1) as f32).min(1.0))
}

fn alpha(rgba: u32, a: f32) -> u32 {
    (rgba & 0xffff_ff00) | ((rgba & 0xff) as f32 * a.clamp(0.0, 1.0)) as u32
}

/// Lay the HUD out.
pub fn layout(ui: &UiPack, s: &State<'_>, q: &mut Vec<Quad>) {
    // Bottom right: the weapon's HUD icon, the magazine (big) and the reserve, the weapon's name.
    if let Some((name, display, clip, stock)) = s.weapon {
        weapon_icon(ui, name, 432.0, 418.0, 96.0, 48.0, WHITE, q);
        let mut n = Str::default();
        n.num(clip.max(0) as u32);
        text(
            ui,
            "fonts/hudbigfont",
            n.as_str(),
            584.0,
            458.0,
            0.5,
            if clip > 0 { WHITE } else { RED },
            2,
            q,
        );
        let mut r = Str::default();
        r.push_str("/ ");
        r.num(stock.max(0) as u32);
        text(
            ui,
            "fonts/hudbigfont",
            r.as_str(),
            588.0,
            458.0,
            0.3,
            DIM,
            0,
            q,
        );
        text(ui, "fonts/smallfont", display, 616.0, 412.0, 0.3, DIM, 2, q);
    }
    // Above them: the lethal and tactical grenades with their counts.
    for (k, g) in s.grenades.iter().enumerate() {
        let Some((name, n)) = g else { continue };
        let x = 548.0 + k as f32 * 40.0;
        let rgba = if *n > 0 { WHITE } else { 0xffff_ff60 };
        weapon_icon(ui, name, x, 368.0, 24.0, 24.0, rgba, q);
        let mut c = Str::default();
        c.num((*n).max(0) as u32);
        text(
            ui,
            "fonts/smallfont",
            c.as_str(),
            x + 26.0,
            390.0,
            0.3,
            rgba,
            0,
            q,
        );
    }
    // Right: the killstreak ready to call in.
    if let Some(r) = s.reward {
        let mut key = alloc::string::String::from("streak:");
        key.push_str(r);
        icon(ui, &key, 584.0, 300.0, 40.0, 40.0, WHITE, q);
        text(
            ui,
            "fonts/smallfont",
            "D-PAD LEFT",
            616.0,
            354.0,
            0.28,
            YELLOW,
            2,
            q,
        );
    }
    // Bottom left: the kill feed over the score and the clock.
    for (row, (line, mine)) in s.feed.iter().rev().enumerate() {
        let y = 388.0 - row as f32 * 16.0;
        text(
            ui,
            "fonts/normalfont",
            line,
            24.0,
            y,
            0.3,
            if *mine { YELLOW } else { DIM },
            0,
            q,
        );
    }
    text(
        ui,
        "fonts/hudbigfont",
        s.score,
        24.0,
        428.0,
        0.32,
        WHITE,
        0,
        q,
    );
    if !s.objectives.is_empty() {
        text(
            ui,
            "fonts/smallfont",
            s.objectives,
            24.0,
            446.0,
            0.34,
            DIM,
            0,
            q,
        );
    }
    text(
        ui,
        "fonts/smallfont",
        s.clock,
        24.0,
        466.0,
        0.36,
        WHITE,
        0,
        q,
    );
    // The crosshair: MW2's hitmarker image, and the points just scored.
    if s.hitmarker {
        icon(ui, "damage_feedback", 308.0, 228.0, 24.0, 24.0, WHITE, q);
    }
    if let Some((pts, at)) = s.popup
        && let Some(a) = fade(s.now, at, 1200, 400)
    {
        let mut p = Str::default();
        p.push('+');
        p.num(pts);
        text(
            ui,
            "fonts/hudbigfont",
            p.as_str(),
            352.0,
            268.0,
            0.38,
            alpha(YELLOW, a),
            0,
            q,
        );
    }
    // Top: notices and the nuke's countdown.
    if let Some((n, at)) = s.notice
        && let Some(a) = fade(s.now, at, 3000, 600)
    {
        text(
            ui,
            "fonts/objectivefont",
            n,
            320.0,
            116.0,
            0.42,
            alpha(YELLOW, a),
            1,
            q,
        );
    }
    if let Some(t) = s.nuke_in {
        let mut l = Str::default();
        l.push_str("TACTICAL NUKE: ");
        l.num(t);
        text(
            ui,
            "fonts/hudbigfont",
            l.as_str(),
            320.0,
            86.0,
            0.4,
            RED,
            1,
            q,
        );
    }
    // Dead: the respawn countdown.
    if let Some(t) = s.respawn_in {
        text(
            ui,
            "fonts/bigfont",
            "YOU WERE KILLED",
            320.0,
            220.0,
            0.5,
            WHITE,
            1,
            q,
        );
        let mut l = Str::default();
        l.push_str("RESPAWNING IN ");
        l.num(t);
        text(
            ui,
            "fonts/normalfont",
            l.as_str(),
            320.0,
            250.0,
            0.3,
            DIM,
            1,
            q,
        );
    }
}

//! The match HUD's effects over the play view, after IW4L (acc57a5; ported, see `docs/runtime.md` "HUD"):
//!
//! - [`Crosshair`]: MW2's hip crosshair from the weapon's reticle materials (`hud_iw4::crosshair`: spread gap, ADS
//!   transition, alpha by aim spread; the four side quads turned as IW4L's `hud::reticle` turns them).
//! - [`scope`]: the sniper scope overlay (`hud_iw4::iris`: overlay zoom, one or four mirrored quads, black letterbox
//!   around them; the gun is hidden while it is up).
//! - [`DamageDirections`]: the hit-direction arrows (IW4L `hud::damage_direction`: 12 slots by the hit's yaw, MW2's
//!   `hit_direction`).
//! - [`Blood`]: the low-health splatter (`hud_iw4::blood` lerp; MW2's `splatter_alt`).
//! - [`Feed`]: the kill feed with weapon icons (`hud_iw4::gamemsg`: window 0's line times, fades and kill icon size).
//! - [`Splash`]: the killstreak-earned splash (`hud_iw4::splash` slots; MW2's splash table).
//! - [`Overhead`]: teammates' names over their heads (`hud_iw4::overhead_names`).
//! - EMP: the static over the screen while jammed and the flash when one goes off.
//! - [`Rumbler`]: the pad's rumble (IW4L `audio::rumble`), as a duty cycle for the GameCube pad's on / off motor.
//!
//! Everything here is laid out in MW2's virtual 640 × 480 (the Wii's screen) and emits quads: axis-aligned
//! [`Quad`]s and turned [`FreeQuad`]s.

use alloc::vec::Vec;

use wpk::ui::{HudWeapon, NONE, Quad, Rumble};

use crate::minimap::Vert;

/// A turned textured quad (4 corners clockwise from top left) for `mwf_ui_verts`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreeQuad {
    pub texture: u16,
    pub v: [Vert; 4],
}

const CX: f32 = 320.0;
const CY: f32 = 240.0;

fn with_alpha(rgba: u32, a: f32) -> u32 {
    (rgba & 0xffff_ff00) | ((rgba & 0xff) as f32 * a.clamp(0.0, 1.0) + 0.5) as u32
}

/// A `w` × `h` quad centred on `c`, its local axes turned by (`sin`, `cos`) (screen y down).
fn turned(texture: u16, c: [f32; 2], w: f32, h: f32, sin: f32, cos: f32, rgba: u32) -> FreeQuad {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let at = |x: f32, y: f32, s: f32, t: f32| Vert {
        x: c[0] + cos * x - sin * y,
        y: c[1] + sin * x + cos * y,
        s,
        t,
        rgba,
    };
    FreeQuad {
        texture,
        v: [
            at(-hw, -hh, 0.0, 0.0),
            at(hw, -hh, 1.0, 0.0),
            at(hw, hh, 1.0, 1.0),
            at(-hw, hh, 0.0, 1.0),
        ],
    }
}

/// Which way the gun last moved: towards ADS or away (IW4L `ReticleAdsLatch`).
#[derive(Clone, Copy, Debug, Default)]
pub struct AdsLatch {
    last: f32,
    pub to_ads: bool,
}

impl AdsLatch {
    pub fn update(&mut self, frac: f32) {
        if frac > self.last + 1e-4 {
            self.to_ads = true;
        } else if frac + 1e-4 < self.last {
            self.to_ads = false;
        }
        self.last = frac;
    }
}

fn overlay_facts(w: &HudWeapon) -> hud_iw4::WeaponAdsOverlayFacts {
    hud_iw4::WeaponAdsOverlayFacts {
        ads_zoom_in_frac: w.ads_zoom_in_frac,
        ads_zoom_out_frac: w.ads_zoom_out_frac,
        overlay_material: u32::from(w.overlay != NONE),
        overlay_reticle: w.overlay_reticle,
        ads_overlay_width: w.ads_overlay_width,
        ads_overlay_height: w.ads_overlay_height,
        ..Default::default()
    }
}

/// The scope overlay this frame (IW4L `draw_weap_reticle`): the hip reticle's alpha and the overlay's (None: down).
pub fn reticle(w: &HudWeapon, frac: f32, to_ads: bool) -> hud_iw4::ReticleDraw {
    hud_iw4::draw_weap_reticle(
        frac,
        to_ads,
        hud_iw4::LOW_RES_VIEWPORT_MAX_HEIGHT,
        0,
        &overlay_facts(w),
    )
}

/// Whether the gun is drawn (IW4L `viewweapon_drawgun`: hidden behind an active scope overlay).
pub fn draws_gun(w: &HudWeapon, frac: f32, to_ads: bool) -> bool {
    hud_iw4::viewweapon_drawgun(
        false,
        true,
        hud_iw4::get_weap_reticle_zoom(frac, to_ads, &overlay_facts(w)),
    )
}

/// The scope overlay: the material's quads round the centre (four mirrored ones for a small overlay) and black
/// filling the screen outside them, at the overlay's alpha.
pub fn scope(w: &HudWeapon, frac: f32, to_ads: bool, white: u16, q: &mut Vec<Quad>) -> bool {
    let d = reticle(w, frac, to_ads);
    let (Some(a), true) = (d.overlay_alpha, w.overlay != NONE) else {
        return false;
    };
    let l = hud_iw4::draw_ads_overlay_layout(w.ads_overlay_width, w.ads_overlay_height);
    let rgba = with_alpha(0xffff_ffff, a);
    for o in l.live_quads() {
        let (s0, s1) = if o.flip_s { (1.0, 0.0) } else { (0.0, 1.0) };
        let (t0, t1) = if o.flip_t { (1.0, 0.0) } else { (0.0, 1.0) };
        q.push(Quad {
            x: CX + o.x,
            y: CY + o.y,
            w: o.w,
            h: o.h,
            uv: [s0, t0, s1, t1],
            rgba,
            texture: w.overlay,
        });
    }
    // The letterbox: black outside the overlay's rect.
    let (x0, y0) = (CX + l.inner_x, CY + l.inner_y);
    let (x1, y1) = (x0 + l.inner_w, y0 + l.inner_h);
    let black = with_alpha(0x0000_00ff, a);
    for r in [
        [0.0, 0.0, 640.0, y0],
        [0.0, y1, 640.0, 480.0 - y1],
        [0.0, y0, x0, y1 - y0],
        [x1, y0, 640.0 - x1, y1 - y0],
    ] {
        if r[2] > 0.0 && r[3] > 0.0 {
            q.push(Quad {
                x: r[0],
                y: r[1],
                w: r[2],
                h: r[3],
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: black,
                texture: white,
            });
        }
    }
    true
}

/// MW2's hip crosshair for one frame.
pub struct Crosshair<'a> {
    pub weapon: &'a HudWeapon,
    /// The weapon's spread cone (min, max) in degrees for the stance (`get_spread_for_weapon`).
    pub cone: (f32, f32),
    /// IW4 aim spread scale (0..255).
    pub aim_spread_scale: f32,
    pub frac: f32,
    pub to_ads: bool,
    pub tan_half_fov_y: f32,
    /// IW4L `hip_crosshair_visible`'s gate (weapon state, flash, EMP).
    pub gate: hud_iw4::HipCrosshairGate,
}

impl Crosshair<'_> {
    /// The quads: the four sides at the spread's gap (top, right, bottom, left, turned a quarter each) and the centre.
    pub fn quads(&self, out: &mut Vec<FreeQuad>) {
        use hud_iw4 as h;
        if !h::hip_crosshair_visible(&self.gate) {
            return;
        }
        let w = self.weapon;
        let facts = h::WeaponReticleFacts {
            i_reticle_min_ofs: w.reticle_min_ofs,
            hip_reticle_side_pos: w.hip_reticle_side_pos,
            i_reticle_side_size: w.reticle_side_size,
        };
        let ads = h::WeaponAdsCrosshairFacts {
            ads_aim_pitch: w.ads_aim_pitch,
            ads_crosshair_in_frac: w.ads_crosshair_in_frac,
            ads_crosshair_out_frac: w.ads_crosshair_out_frac,
        };
        let trans = h::hip_crosshair_trans_scale(self.frac, self.to_ads, &ads, self.tan_half_fov_y);
        let size = h::reticle_draw_size(&facts, trans);
        let gap = h::calc_reticle_spread(
            self.cone.0,
            self.cone.1,
            self.aim_spread_scale,
            trans,
            self.tan_half_fov_y,
            &facts,
            size,
        );
        let alpha = h::calc_reticle_alpha(
            1.0,
            h::CG_CROSSHAIR_ALPHA_DEFAULT,
            h::CG_CROSSHAIR_ALPHA_MIN_DEFAULT,
            self.aim_spread_scale,
        ) * reticle(w, self.frac, self.to_ads).hip_reticle_alpha;
        if alpha < h::RETICLE_SIDES_ALPHA_DRAW_MIN {
            return;
        }
        let rgba = with_alpha(0xffff_ffff, alpha);
        if w.reticle_side != NONE && size[0] > 0.0 {
            for (dx, dy, sin, cos) in [
                (0.0, -gap[1], 0.0, 1.0),
                (gap[0], 0.0, 1.0, 0.0),
                (0.0, gap[1], 0.0, -1.0),
                (-gap[0], 0.0, -1.0, 0.0),
            ] {
                out.push(turned(
                    w.reticle_side,
                    [CX + dx, CY + dy],
                    size[0],
                    size[1],
                    sin,
                    cos,
                    rgba,
                ));
            }
        }
        let c = w.reticle_center_size as f32 * trans;
        if w.reticle_center != NONE && c > 0.0 {
            out.push(turned(w.reticle_center, [CX, CY], c, c, 0.0, 1.0, rgba));
        }
    }
}

/// `cg_hudDamageIconTime`, `cg_hudDamageIconWidth`, `..Offset`, `..Height` (MW2's defaults, as IW4L reads them).
pub const DAMAGE_ICON_TIME_MS: i32 = 6000;
pub const DAMAGE_ICON_WIDTH: f32 = 128.0;
pub const DAMAGE_ICON_OFFSET: f32 = 128.0;
pub const DAMAGE_ICON_HEIGHT: f32 = 64.0;

#[derive(Clone, Copy, Debug, Default)]
struct DamageHit {
    time: i32,
    duration: i32,
    yaw: f32,
}

/// The hit-direction arrows (IW4L `hud::damage_direction::DamageDirections`): 12 slots, the oldest taken.
#[derive(Clone, Debug, Default)]
pub struct DamageDirections {
    hits: [DamageHit; 12],
    random: u32,
}

/// The yaw (degrees) a hit came along: from `from` towards `to` (IW4's `damageYaw`, kept in a byte).
pub fn damage_yaw(from: [f32; 3], to: [f32; 3]) -> u8 {
    let yaw = libm::atan2f(to[1] - from[1], to[0] - from[0]).to_degrees();
    let yaw = if yaw < 0.0 { yaw + 360.0 } else { yaw };
    (libm::floorf(yaw * 256.0 / 360.0 + 0.5) as i32 & 0xff) as u8
}

impl DamageDirections {
    pub fn clear(&mut self) {
        *self = DamageDirections::default();
    }

    /// A hit at `now` whose `damage_yaw` byte is `yaw` (jittered ±10° as IW4L does).
    pub fn hit(&mut self, now: i32, yaw: u8) {
        let slot = self
            .hits
            .iter()
            .enumerate()
            .min_by_key(|(_, h)| h.time)
            .map_or(0, |(i, _)| i);
        self.random = self.random.wrapping_mul(214013).wrapping_add(2531011);
        let jitter = (((self.random >> 16) & 0x7fff) as f32 / 32768.0 - 0.5) * 20.0;
        self.hits[slot] = DamageHit {
            time: now,
            duration: DAMAGE_ICON_TIME_MS,
            yaw: f32::from(yaw) * (360.0 / 256.0) + jitter,
        };
    }

    /// The arrows round the crosshair for a view yawed `view_yaw` degrees, fading over their last half.
    pub fn quads(&self, now: i32, view_yaw: f32, texture: u16, out: &mut Vec<FreeQuad>) {
        if texture == NONE {
            return;
        }
        let (hw, offset) = (DAMAGE_ICON_WIDTH * 0.5, DAMAGE_ICON_OFFSET);
        let outer = offset + DAMAGE_ICON_HEIGHT;
        for h in &self.hits {
            let age = now.saturating_sub(h.time);
            if age <= 0 || age >= h.duration {
                continue;
            }
            let (sin, cos) = libm::sincosf((view_yaw - h.yaw).to_radians());
            let a = (2.0 - 2.0 * age as f32 / h.duration as f32).min(1.0);
            let rgba = with_alpha(0xffff_ffff, a);
            let at = |x: f32, y: f32, s: f32, t: f32| Vert {
                x: CX + cos * x - sin * y,
                y: CY + sin * x + cos * y,
                s,
                t,
                rgba,
            };
            out.push(FreeQuad {
                texture,
                v: [
                    at(hw, outer, 0.0, 0.0),
                    at(-hw, outer, 1.0, 0.0),
                    at(-hw, offset, 1.0, 1.0),
                    at(hw, offset, 0.0, 1.0),
                ],
            });
        }
    }
}

/// The low-health splatter's intensity (IW4L `hud::blood`: `blood_overlay_lerp` towards 1 - health).
#[derive(Clone, Copy, Debug, Default)]
pub struct Blood {
    pub intensity: f32,
}

impl Blood {
    pub fn update(&mut self, health: i32, max_health: i32, alive: bool, frametime_ms: i32) {
        if !alive {
            self.intensity = 0.0;
            return;
        }
        let frac = hud_iw4::get_health_fraction(health, max_health, 0);
        self.intensity = hud_iw4::blood_overlay_lerp(
            self.intensity,
            frac,
            frametime_ms,
            hud_iw4::HUD_BLOOD_OVERLAY_LERP_RATE_DEFAULT,
        );
    }

    pub fn quad(&self, texture: u16, q: &mut Vec<Quad>) {
        if self.intensity > 0.0 && texture != NONE {
            q.push(Quad {
                x: 0.0,
                y: 0.0,
                w: 640.0,
                h: 480.0,
                uv: [0.0, 0.0, 1.0, 1.0],
                rgba: with_alpha(0xffff_ffff, self.intensity),
                texture,
            });
        }
    }
}

/// MW2's EMP static (`hud_fullscreen`'s `ac130_overlay_grain` item: forecolor 0.9 1 1 0.3, shown while jammed),
/// its grain moved each frame.
pub fn emp_static(texture: u16, frame: u32, q: &mut Vec<Quad>) {
    if texture == NONE {
        return;
    }
    let h = frame.wrapping_mul(2_654_435_761);
    let (s, t) = (
        (h >> 8 & 0xff) as f32 / 256.0,
        (h >> 16 & 0xff) as f32 / 256.0,
    );
    q.push(Quad {
        x: 0.0,
        y: 0.0,
        w: 640.0,
        h: 480.0,
        uv: [s, t, s + 2.0, t + 1.5],
        rgba: 0xe6ff_ff4d,
        texture,
    });
}

/// How long the EMP's flash lasts, ms.
pub const EMP_FLASH_MS: u32 = 3100;

/// The EMP's flash, when one goes off (`_emp.gsc`: `coup_sunblind` in over 0.1 s, back to the map's over 3 s); a
/// white fade in its place until vision sets are ported. 0 when over.
pub fn emp_flash(age_ms: u32) -> f32 {
    const PEAK: f32 = 0.85;
    match age_ms {
        0..100 => PEAK * age_ms as f32 / 100.0,
        100..3100 => PEAK * (1.0 - (age_ms - 100) as f32 / 3000.0),
        _ => 0.0,
    }
}

/// A kill feed line.
#[derive(Clone, Copy, Debug)]
pub struct FeedLine<'a> {
    pub killer: Option<&'a str>,
    pub victim: &'a str,
    /// The killer's weapon (its kill icon) or a means-of-death icon (`killiconheadshot` ...).
    pub icon: Option<&'a HudWeapon>,
    pub headshot: u16,
    pub mine: bool,
    pub at: u32,
}

/// Window 0's alpha for a line `age` ms old (fade in, hold, fade out over `GAME_MSG_WIN0_MSG_TIME_MS`).
pub fn feed_alpha(age: u32) -> Option<f32> {
    use hud_iw4::{GAME_MSG_WIN0_FADE_IN_TIME_MS as IN, GAME_MSG_WIN0_FADE_OUT_TIME_MS as OUT};
    let life = hud_iw4::GAME_MSG_WIN0_MSG_TIME_MS as u32;
    if age >= life {
        return None;
    }
    let a = if age < IN as u32 {
        age as f32 / IN as f32
    } else if age > life - OUT as u32 {
        (life - age) as f32 / OUT as f32
    } else {
        1.0
    };
    Some(a)
}

/// A kill icon's virtual size (`killicon_virtual_size` by the weapon's ratio) and its s range (flipped or not).
pub fn kill_icon(w: &HudWeapon) -> ((f32, f32), (f32, f32)) {
    (
        hud_iw4::killicon_virtual_size(i32::from(w.kill_icon_ratio)),
        hud_iw4::killicon_stretch_uv(w.flip_kill_icon),
    )
}

/// The killstreak splash in its slot (`hud_iw4::splash`, slot 0).
#[derive(Clone, Copy, Debug, Default)]
pub struct Splash {
    pub slot: hud_iw4::SplashSlot,
}

/// The splash's alpha `age` ms into a `duration` ms splash: in over 250 ms, out over the last 250.
pub fn splash_alpha(age: i32, duration: i32) -> f32 {
    const FADE: f32 = 250.0;
    let a = (age as f32 / FADE).min((duration - age) as f32 / FADE);
    a.clamp(0.0, 1.0)
}

/// Teammates' names over their heads: when each was first and last seen (IW4L `overhead_fade_alpha` with the
/// friendly fade times).
#[derive(Clone, Debug)]
pub struct Overhead {
    seen: [(i32, i32); 4],
}

impl Default for Overhead {
    fn default() -> Self {
        Overhead {
            seen: [(i32::MIN / 2, i32::MIN / 2); 4],
        }
    }
}

impl Overhead {
    /// Fighter `i` is `visible` at `now`: its name's alpha.
    pub fn alpha(&mut self, i: usize, now: i32, visible: bool) -> f32 {
        use hud_iw4::{
            FRIENDLY_NAME_FADE_IN_DEFAULT_MS as IN, FRIENDLY_NAME_FADE_OUT_DEFAULT_MS as OUT,
        };
        let Some(s) = self.seen.get_mut(i) else {
            return 0.0;
        };
        if visible {
            if now.wrapping_sub(s.1) > OUT {
                s.0 = now;
            }
            s.1 = now;
        }
        if IN == 0 && now.wrapping_sub(s.1) <= OUT {
            // Fade in of 0 ms: full at once, then the fade out.
            let since = now.wrapping_sub(s.1);
            return if since < OUT {
                (OUT - since) as f32 / OUT as f32
            } else {
                0.0
            };
        }
        hud_iw4::overhead_fade_alpha(now, s.0, s.1, IN, OUT)
    }
}

/// Where a name goes over a head at `anchor`: screen position and the distance scale (IW4L `overhead_anchor`'s
/// fallback lift above the origin, `overhead_distance_scale` near 64 / far 512 at 0.7).
pub fn overhead_place(cam: &crate::camera::Camera, origin: [f32; 3]) -> Option<([f32; 2], f32)> {
    use hud_iw4::{
        OVERHEAD_FAR_DISTANCE_DEFAULT as FAR, OVERHEAD_FAR_SCALE_DEFAULT as FS,
        OVERHEAD_NEAR_DISTANCE_DEFAULT as NEAR,
    };
    let anchor = hud_iw4::overhead_anchor(hud_iw4::OverheadHeadResult::NoDObjOrHead, origin);
    let d = crate::camera::sub(anchor, cam.eye);
    if crate::camera::dot(d, d)
        > hud_iw4::OVERHEAD_MAX_DISTANCE_DEFAULT * hud_iw4::OVERHEAD_MAX_DISTANCE_DEFAULT
    {
        return None;
    }
    let p = cam.project(anchor)?;
    Some((
        p,
        hud_iw4::overhead_distance_scale(cam.eye, anchor, NEAR, FAR, FS),
    ))
}

/// Playing rumbles (IW4L `audio::rumble`): each one's strength over its life, the strongest wins, both motors.
#[derive(Clone, Debug, Default)]
pub struct Rumbler {
    active: Vec<(u32, u16, f32)>,
    duty: f32,
}

/// A graph's strength at `fraction` of the rumble's life (IW4L `audio::rumble::sample`).
pub fn sample(points: &[(f32, f32)], fraction: f32) -> f32 {
    let Some(&first) = points.first() else {
        return 0.0;
    };
    let mut previous = first;
    if fraction <= previous.0 {
        return previous.1.clamp(0.0, 1.0);
    }
    for &next in &points[1..] {
        if fraction <= next.0 {
            let t = if next.0 > previous.0 {
                (fraction - previous.0) / (next.0 - previous.0)
            } else {
                1.0
            };
            return (previous.1 + (next.1 - previous.1) * t).clamp(0.0, 1.0);
        }
        previous = next;
    }
    previous.1.clamp(0.0, 1.0)
}

/// A rumble's (strong, weak) motor strength `elapsed` ms in.
pub fn intensity(r: &Rumble, elapsed: u32) -> (f32, f32) {
    let f = (elapsed as f32 / r.duration_ms.max(1) as f32).clamp(0.0, 1.0);
    (sample(&r.low, f), sample(&r.high, f))
}

impl Rumbler {
    /// Start rumble `index` at `now`, at `gain` (1 for your own; less with distance for broadcast ones).
    pub fn play(&mut self, index: u16, now: u32, gain: f32) {
        if index == NONE || gain <= 0.0 {
            return;
        }
        if self.active.len() >= 32 {
            self.active.remove(0);
        }
        self.active.push((now, index, gain.min(1.0)));
    }

    pub fn stop(&mut self) {
        self.active.clear();
        self.duty = 0.0;
    }

    /// Both motors' strength at `now` (the strongest playing rumble per motor); ended rumbles are dropped.
    pub fn strength(&mut self, now: u32, rumbles: &[Rumble]) -> (f32, f32) {
        self.active.retain(|&(t, i, _)| {
            rumbles
                .get(usize::from(i))
                .is_some_and(|r| now.wrapping_sub(t) < r.duration_ms)
        });
        let mut out = (0.0f32, 0.0f32);
        for &(t, i, g) in &self.active {
            if let Some(r) = rumbles.get(usize::from(i)) {
                let (a, b) = intensity(r, now.wrapping_sub(t));
                out.0 = out.0.max(a * g);
                out.1 = out.1.max(b * g);
            }
        }
        out
    }

    /// The GameCube pad's motor this frame (it is on or off): on for a share of frames matching the strength.
    pub fn motor(&mut self, strength: (f32, f32)) -> bool {
        let s = strength.0.max(strength.1 * 0.5).clamp(0.0, 1.0);
        if s <= 0.0 {
            self.duty = 0.0;
            return false;
        }
        self.duty += s;
        if self.duty >= 1.0 {
            self.duty -= 1.0;
            true
        } else {
            false
        }
    }
}

/// A broadcast rumble's gain at `distance` (IW4L `spatial_gain`): 0 out of range, faded with distance if it says so.
pub fn spatial_gain(r: &Rumble, distance: f32) -> f32 {
    if !r.broadcast {
        return 1.0;
    }
    if !distance.is_finite() || distance > r.range {
        return 0.0;
    }
    if r.fade_with_distance {
        (1.0 - distance / r.range).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sniper() -> HudWeapon {
        HudWeapon {
            name: "cheytac_mp".into(),
            overlay: 3,
            reticle_side: 4,
            reticle_center: NONE,
            ads_zoom_in_frac: 0.05,
            ads_zoom_out_frac: 0.1,
            ads_overlay_width: 220.0,
            ads_overlay_height: 220.0,
            overlay_reticle: 1,
            reticle_side_size: 8,
            reticle_min_ofs: 8,
            hip_reticle_side_pos: 0.5,
            ads_crosshair_in_frac: 1.0,
            ads_crosshair_out_frac: 0.2,
            ..HudWeapon::default()
        }
    }

    #[test]
    fn the_scope_comes_up_late_in_the_ads_move_and_hides_the_gun() {
        let w = sniper();
        // IW4L calc_ads_overlay_zoom: (frac - (1 - window)) / window.
        assert!(reticle(&w, 0.9, true).overlay_alpha.is_none());
        let d = reticle(&w, 0.975, true);
        assert!((d.overlay_alpha.unwrap() - 0.5).abs() < 1e-4);
        assert!((d.hip_reticle_alpha - 0.5).abs() < 1e-4);
        assert!(draws_gun(&w, 0.9, true) && !draws_gun(&w, 1.0, true));
        let mut q = Vec::new();
        assert!(scope(&w, 1.0, true, 9, &mut q));
        // A 220 overlay is under 320 × 240: four mirrored quadrants, then four letterbox bars.
        assert_eq!(q.len(), 8);
        assert_eq!((q[0].x, q[0].y, q[0].w), (100.0, 20.0, 220.0));
        assert_eq!(q[1].uv[0], 1.0);
        assert_eq!(q[4].texture, 9);
        assert_eq!(q[4].h, 20.0);
        assert_eq!(q[6].w, 100.0);
    }

    #[test]
    fn the_crosshair_opens_with_spread_and_turns_its_sides() {
        let w = sniper();
        let t = libm::tanf(25.54f32.to_radians());
        let gate = hud_iw4::HipCrosshairGate::default();
        let mut out = Vec::new();
        Crosshair {
            weapon: &w,
            cone: (2.0, 8.0),
            aim_spread_scale: 0.0,
            frac: 0.0,
            to_ads: false,
            tan_half_fov_y: t,
            gate,
        }
        .quads(&mut out);
        assert_eq!(out.len(), 4);
        // Gap: tan(2°) · 240 / tan(fov/2) - 0.5 · 8 (IW4L calc_reticle_spread).
        let gap = libm::tanf(2.0f32.to_radians()) * 240.0 / t - 4.0;
        let top = &out[0].v;
        let cy = (top[0].y + top[2].y) * 0.5;
        assert!((240.0 - cy - gap).abs() < 1e-3, "{cy} {gap}");
        // The right side is turned a quarter: its texture's top edge runs down the screen.
        let r = &out[1].v;
        assert!((r[0].x - r[1].x).abs() < 1e-4 && r[1].y > r[0].y);
        // Full spread opens wider and fades to the minimum alpha 0.5.
        let mut wide = Vec::new();
        Crosshair {
            weapon: &w,
            cone: (2.0, 8.0),
            aim_spread_scale: 255.0,
            frac: 0.0,
            to_ads: false,
            tan_half_fov_y: t,
            gate,
        }
        .quads(&mut wide);
        assert!(wide[0].v[0].y < top[0].y);
        assert_eq!(wide[0].v[0].rgba & 0xff, 128);
        // Reloading (weapon state 0x2) hides it.
        let mut none = Vec::new();
        Crosshair {
            weapon: &w,
            cone: (2.0, 8.0),
            aim_spread_scale: 0.0,
            frac: 0.0,
            to_ads: false,
            tan_half_fov_y: t,
            gate: hud_iw4::HipCrosshairGate {
                weaponstate_primary: 2,
                ..gate
            },
        }
        .quads(&mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn damage_arrows_point_at_the_shooter_and_fade() {
        let mut d = DamageDirections::default();
        // A shooter on your right (you face +x, yaw 0): the bullet travels towards -y... i.e. from (0,-100) to you.
        d.hit(1000, damage_yaw([0.0, -100.0, 0.0], [0.0, 0.0, 0.0]));
        let mut out = Vec::new();
        d.quads(1001, 0.0, 5, &mut out);
        assert_eq!(out.len(), 1);
        let mx = out[0].v.iter().map(|v| v.x).sum::<f32>() / 4.0;
        let my = out[0].v.iter().map(|v| v.y).sum::<f32>() / 4.0;
        assert!(mx > 320.0 + 140.0 && (my - 240.0).abs() < 40.0, "{mx} {my}");
        // A shooter behind: below the crosshair.
        let mut b = DamageDirections::default();
        b.hit(1000, damage_yaw([-100.0, 0.0, 0.0], [0.0, 0.0, 0.0]));
        let mut out = Vec::new();
        b.quads(1001, 0.0, 5, &mut out);
        let my = out[0].v.iter().map(|v| v.y).sum::<f32>() / 4.0;
        assert!(my > 240.0 + 140.0);
        // Full for the first half, half alpha at 3/4, gone at 6 s.
        let mut out = Vec::new();
        d.quads(1000 + 4500, 0.0, 5, &mut out);
        assert_eq!(out[0].v[0].rgba & 0xff, 128);
        let mut out = Vec::new();
        d.quads(1000 + 6000, 0.0, 5, &mut out);
        assert!(out.is_empty());
        // The 13th hit takes the oldest slot.
        for k in 0..13 {
            d.hit(2000 + k, 0);
        }
        let mut out = Vec::new();
        d.quads(2100, 0.0, 5, &mut out);
        assert_eq!(out.len(), 12);
        assert_eq!(damage_yaw([0.0; 3], [1.0, 0.0, 0.0]), 0);
        assert_eq!(damage_yaw([0.0; 3], [0.0, 1.0, 0.0]), 64);
    }

    #[test]
    fn blood_rises_at_once_and_ebbs_at_iw4ls_rate() {
        let mut b = Blood::default();
        b.update(40, 100, true, 16);
        assert!((b.intensity - 0.6).abs() < 1e-5);
        // Healed: back down at 0.3 a second.
        b.update(100, 100, true, 1000);
        assert!((b.intensity - 0.3).abs() < 1e-5);
        b.update(100, 100, false, 16);
        assert_eq!(b.intensity, 0.0);
    }

    #[test]
    fn feed_lines_fade_in_hold_and_out() {
        assert_eq!(feed_alpha(0), Some(0.0));
        assert_eq!(feed_alpha(125), Some(0.5));
        assert_eq!(feed_alpha(2000), Some(1.0));
        assert_eq!(feed_alpha(4750), Some(0.5));
        assert_eq!(feed_alpha(5000), None);
        let w = HudWeapon {
            kill_icon_ratio: 1,
            flip_kill_icon: true,
            ..HudWeapon::default()
        };
        let ((iw, ih), (s0, s1)) = kill_icon(&w);
        // A wide icon (ratio 1) is 2.8 em by 1.4 em at the window's 0.375 scale.
        assert_eq!(
            (iw, ih),
            (
                hud_iw4::killicon_virtual_size(1).0,
                hud_iw4::killicon_virtual_size(1).1
            )
        );
        assert!(iw > ih);
        assert_eq!((s0, s1), (1.0, 0.0));
    }

    #[test]
    fn splashes_and_names_fade() {
        assert_eq!(splash_alpha(0, 2500), 0.0);
        assert_eq!(splash_alpha(1000, 2500), 1.0);
        assert_eq!(splash_alpha(2375, 2500), 0.5);
        let mut o = Overhead::default();
        assert_eq!(o.alpha(1, 1000, true), 1.0);
        assert_eq!(o.alpha(1, 1750, false), 0.5);
        assert_eq!(o.alpha(1, 2600, false), 0.0);
        assert_eq!(o.alpha(2, 2600, false), 0.0);
    }

    #[test]
    fn rumble_follows_its_graph_and_duty_cycles_the_motor() {
        let r = Rumble {
            name: "assault_fire".into(),
            duration_ms: 200,
            low: vec![(0.0, 1.0), (1.0, 0.0)],
            high: vec![(0.0, 0.5), (0.5, 0.5), (1.0, 0.0)],
            ..Rumble::default()
        };
        assert_eq!(intensity(&r, 100), (0.5, 0.5));
        assert_eq!(sample(&r.high, 0.75), 0.25);
        let rumbles = vec![r];
        let mut m = Rumbler::default();
        m.play(0, 1000, 1.0);
        let s = m.strength(1100, &rumbles);
        assert_eq!(s, (0.5, 0.5));
        // Half strength: on every other frame.
        let on: usize = (0..10).filter(|_| m.motor((0.5, 0.0))).count();
        assert_eq!(on, 5);
        assert_eq!(m.strength(1200, &rumbles), (0.0, 0.0));
        assert!(!m.motor((0.0, 0.0)));
        let b = Rumble {
            broadcast: true,
            fade_with_distance: true,
            range: 800.0,
            ..Rumble::default()
        };
        assert_eq!(spatial_gain(&b, 400.0), 0.5);
        assert_eq!(spatial_gain(&b, 900.0), 0.0);
    }

    #[test]
    fn emp_flash_and_static() {
        assert_eq!(emp_flash(0), 0.0);
        assert!((emp_flash(100) - 0.85).abs() < 1e-6);
        assert!(emp_flash(1600) < 0.5 && emp_flash(1600) > 0.4);
        assert_eq!(emp_flash(3200), 0.0);
        let mut q = Vec::new();
        emp_static(7, 3, &mut q);
        assert_eq!(q[0].rgba & 0xff, 0x4d);
    }
}

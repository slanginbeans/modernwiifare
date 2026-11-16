//! The killstreaks on the Wii side (`game_core::killstreaks` does the work): the match setup, the player's call-ins,
//! crate capture with X held, the notes it raises, stand-in boxes for the world models not converted yet (crates,
//! the Little Bird, the C-130) and the HUD lines.

use alloc::string::String;
use alloc::vec::Vec;

use game_core::arena::Arena;
use game_core::killstreaks::{MapInfo, Note, Streak, Streaks, Weapons, airdrop};

/// The killstreak HUD's state between frames.
#[derive(Default)]
pub struct Shell {
    /// A line and when it came up.
    pub note: Option<(String, u32)>,
    /// A crate in reach: the capture's progress (0 until X is held).
    pub capture: Option<f32>,
    /// Picking an air strike's target on the map.
    pub targeting: Option<Targeting>,
    /// Fire held last time [`Shell::fire_pressed`] looked (the trigger has no press of its own).
    fire_was: bool,
}

/// The target screen (MW2's `beginLocationSelection`): the strike, the cursor on the map and the run's heading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Targeting {
    pub streak: Streak,
    pub at: [f32; 2],
    pub yaw: f32,
}

/// The target screen's map rect (x, y, w, h) on the 640 × 480 screen.
const TARGET_RECT: [f32; 4] = [120.0, 56.0, 400.0, 380.0];
/// How fast the cursor crosses the map at full stick (map widths a second).
const CURSOR_SPEED: f32 = 0.45;

/// Give the arena its killstreak rules (the common pack's `KSRL`, stock values with an older pack), weapons and the
/// map's middle (the minimap's, else the spawns').
pub fn setup(
    a: &mut Arena,
    common: &[u8],
    weapons: &[u8],
    equips: &[u8],
    spawns: &[([f32; 3], f32)],
    minimap: Option<&wpk::records::Minimap>,
) {
    let rules = wpk::Pack::parse(common, false)
        .ok()
        .and_then(|p| p.section(wpk::tag::STREAK_RULES))
        .and_then(|s| wpk::streaks::StreakRules::decode(s.data))
        .unwrap_or_default();
    let n = spawns.len().max(1) as f32;
    let mut center = [0.0f32; 3];
    for (o, _) in spawns {
        for k in 0..3 {
            center[k] += o[k] / n;
        }
    }
    let mut radius = spawns
        .iter()
        .map(|(o, _)| libm::hypotf(o[0] - center[0], o[1] - center[1]))
        .fold(0.0f32, f32::max);
    if let Some(m) = minimap {
        center[0] = (m.northwest[0] + m.southeast[0]) * 0.5;
        center[1] = (m.northwest[1] + m.southeast[1]) * 0.5;
        radius = radius.max(
            libm::hypotf(
                m.northwest[0] - m.southeast[0],
                m.northwest[1] - m.southeast[1],
            ) * 0.35,
        );
    }
    a.streaks = Streaks::new(
        rules,
        Weapons::from_records(weapons, equips),
        MapInfo {
            center,
            radius: radius.max(512.0),
        },
    );
}

impl Shell {
    fn say(&mut self, text: &str, now: u32) {
        self.note = Some((String::from(text), now));
    }

    /// The player calls in `name` (a killstreak the radar code doesn't handle). Returns false when it is not one of
    /// these (the caller keeps its own handling).
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &mut self,
        a: &mut Arena,
        clip: &game_core::collision::MapClip<'_>,
        name: &str,
        eye: [f32; 3],
        forward: [f32; 3],
        vel: [f32; 3],
        now: u32,
    ) -> bool {
        let Some(streak) = Streak::from_name(name) else {
            return false;
        };
        match streak {
            Streak::Airdrop | Streak::Mega => {
                // Thrown a little up from the view (the marker's own throw numbers fly it).
                let up = [forward[0], forward[1], forward[2] + 0.25];
                let l = libm::sqrtf(up.iter().map(|x| x * x).sum()).max(1e-6);
                let dir = up.map(|x| x / l);
                let ahead = [
                    eye[0] + forward[0] * 256.0,
                    eye[1] + forward[1] * 256.0,
                    eye[2],
                ];
                let t = clip.trace(
                    eye,
                    ahead,
                    [0.0; 3],
                    [0.0; 3],
                    game_core::collision::MASK_PLAYER_SOLID,
                );
                let at: [f32; 3] =
                    core::array::from_fn(|k| eye[k] + (ahead[k] - eye[k]) * t.fraction);
                let down = [at[0], at[1], at[2] - 4096.0];
                let g = clip.trace(
                    at,
                    down,
                    [0.0; 3],
                    [0.0; 3],
                    game_core::collision::MASK_PLAYER_SOLID,
                );
                let ground = [at[0], at[1], at[2] - 4096.0 * g.fraction];
                a.throw_marker(0, streak, eye, dir, vel, ground, now);
            }
            Streak::Predator => a.launch_predator(0),
            Streak::Precision | Streak::Harrier | Streak::Stealth => {
                let o = a.fighters[0].player.ps.origin;
                self.targeting = Some(Targeting {
                    streak,
                    at: [o[0], o[1]],
                    yaw: a.fighters[0].player.view().1,
                });
                self.say("PICK THE TARGET", now);
                return true;
            }
            Streak::Sentry => a.give_sentry(0, now),
            Streak::Helicopter | Streak::PaveLow | Streak::ChopperGunner | Streak::Ac130 => {
                a.call_air(0, streak, now)
            }
            _ => return false,
        }
        self.say(streak.called(), now);
        true
    }

    /// Whether fire went down since the last call.
    pub fn fire_pressed(&mut self, held: bool) -> bool {
        let p = held && !self.fire_was;
        self.fire_was = held;
        p
    }

    /// Whether the player is busy with a killstreak screen (the body stands still).
    pub fn busy(&self) -> bool {
        self.targeting.is_some()
    }

    /// The target screen's input: the main stick moves the cursor (screen up is north), the C-stick turns the
    /// heading, A or R calls the strike, B gives the reward back (`cancel` returns its name).
    #[allow(clippy::too_many_arguments)]
    pub fn target_input(
        &mut self,
        a: &mut Arena,
        clip: &game_core::collision::MapClip<'_>,
        minimap: Option<&wpk::records::Minimap>,
        move_stick: (i8, i8),
        turn_stick: (i8, i8),
        confirm: bool,
        cancel: bool,
        dt: f32,
        now: u32,
    ) -> Option<&'static str> {
        let t = self.targeting.as_mut()?;
        if cancel {
            let name = t.streak.name();
            self.targeting = None;
            return Some(name);
        }
        let frame = minimap.and_then(|m| {
            let c = game_core::minimap::Frame::map_centre(m)?;
            game_core::minimap::Frame::whole(m, TARGET_RECT, c)
        });
        let (right, up) = frame
            .as_ref()
            .map_or(([1.0, 0.0], [0.0, 1.0]), |f| f.axes());
        let span = a.streaks.map.radius * 2.0;
        let k = |v: i8| {
            let v = f32::from(v) / 127.0;
            if libm::fabsf(v) < 0.15 { 0.0 } else { v }
        };
        let (sx, sy) = (k(move_stick.0), k(move_stick.1));
        for i in 0..2 {
            t.at[i] += (right[i] * sx + up[i] * sy) * span * CURSOR_SPEED * dt;
        }
        // Kept on the map.
        let c = a.streaks.map.center;
        let r = a.streaks.map.radius * 1.2;
        let d = libm::hypotf(t.at[0] - c[0], t.at[1] - c[1]);
        if d > r {
            t.at = [
                c[0] + (t.at[0] - c[0]) * r / d,
                c[1] + (t.at[1] - c[1]) * r / d,
            ];
        }
        t.yaw -= k(turn_stick.0) * 120.0 * dt;
        if !confirm {
            return None;
        }
        // The ground under the cursor.
        let top = [t.at[0], t.at[1], c[2] + 8000.0];
        let tr = clip.trace(
            top,
            [top[0], top[1], top[2] - 16_000.0],
            [0.0; 3],
            [0.0; 3],
            game_core::collision::MASK_PLAYER_SOLID,
        );
        let target = [t.at[0], t.at[1], top[2] - 16_000.0 * tr.fraction];
        let (streak, yaw) = (t.streak, t.yaw);
        self.targeting = None;
        a.call_strike(0, streak, target, yaw, now);
        crate::devlog::event(
            now,
            "strike",
            format_args!(
                "kind={} x={:.0} y={:.0} z={:.0} yaw={yaw:.0}",
                streak.name(),
                target[0],
                target[1],
                target[2]
            ),
        );
        self.say(streak.called(), now);
        None
    }

    /// The target screen over the game: the whole minimap, you, the cursor and the heading.
    pub fn draw_targeting(
        &self,
        gfx: &mut impl game_core::gfx::Gfx,
        rects: &mut Vec<game_core::hud::Rect>,
        a: &Arena,
        minimap: Option<&(wpk::records::Minimap, i32)>,
    ) {
        let Some(t) = self.targeting else {
            return;
        };
        let r = TARGET_RECT;
        let ri = |v: f32| v as i16;
        gfx.hud_rects(&[[0, 0, 640, 480]], 0x0000_00b0);
        let frame = minimap.and_then(|(m, h)| {
            let c = game_core::minimap::Frame::map_centre(m)?;
            Some((game_core::minimap::Frame::whole(m, r, c)?, *h))
        });
        let to_screen = |p: [f32; 2]| -> [f32; 2] {
            match &frame {
                Some((f, _)) => f.to_screen([p[0], p[1], 0.0]),
                None => {
                    let c = a.streaks.map.center;
                    let s = r[3] / (a.streaks.map.radius * 2.4).max(1.0);
                    [
                        r[0] + r[2] * 0.5 + (p[0] - c[0]) * s,
                        r[1] + r[3] * 0.5 - (p[1] - c[1]) * s,
                    ]
                }
            }
        };
        if let Some((f, h)) = &frame {
            let verts = f.map_quad(0xffff_ffff);
            unsafe { crate::ffi::mwf_ui_verts(*h, verts.as_ptr(), 1) };
        } else {
            gfx.hud_rects(&[[ri(r[0]), ri(r[1]), ri(r[2]), ri(r[3])]], 0x2a3a2aff);
        }
        // You (white), the other players you can see (allies blue), the cursor (a red cross), the heading.
        for (i, f) in a.fighters.iter().enumerate() {
            if !f.life.alive() || (i != 0 && a.enemies(0, i)) {
                continue;
            }
            let p = to_screen([f.player.ps.origin[0], f.player.ps.origin[1]]);
            let col = if i == 0 { 0xffff_ffff } else { 0x60a0_ffff };
            gfx.hud_rects(&[[ri(p[0]) - 3, ri(p[1]) - 3, 6, 6]], col);
        }
        let c = to_screen(t.at);
        let (x, y) = (ri(c[0]), ri(c[1]));
        gfx.hud_rects(
            &[[x - 12, y - 1, 24, 3], [x - 1, y - 12, 3, 24]],
            0xff30_30ff,
        );
        if t.streak != Streak::Harrier {
            let d = game_core::killstreaks::yaw_dir(t.yaw);
            let ahead = to_screen([t.at[0] + d[0] * 1200.0, t.at[1] + d[1] * 1200.0]);
            for k in 1..8 {
                let f = k as f32 / 8.0;
                let p = [c[0] + (ahead[0] - c[0]) * f, c[1] + (ahead[1] - c[1]) * f];
                gfx.hud_rects(&[[ri(p[0]) - 2, ri(p[1]) - 2, 4, 4]], 0xffd0_40ff);
            }
        }
        rects.clear();
        let title = match t.streak {
            Streak::Precision => "PRECISION AIRSTRIKE",
            Streak::Harrier => "HARRIER STRIKE",
            _ => "STEALTH BOMBER",
        };
        game_core::hud::text(rects, 120, 24, 2, title);
        game_core::hud::text(
            rects,
            60,
            446,
            1,
            "STICK: TARGET   C-STICK: HEADING   A: CALL IT IN   B: CANCEL",
        );
        gfx.hud_rects(rects, 0xf0f0_f0ff);
    }

    /// After the arena's update: crate rewards and ammo for the player.
    pub fn after_update(
        &mut self,
        a: &mut Arena,
        rewards: &mut Vec<String>,
        mut refill: impl FnMut(),
        now: u32,
    ) {
        for n in a.streaks.take_notes() {
            match n {
                Note::Reward { fighter: 0, streak } => {
                    // The player's list is the shell's.
                    a.streaks.rewards[0].clear();
                    rewards.push(String::from(streak.name()));
                    let mut s = String::from("CARE PACKAGE: ");
                    s.push_str(&streak.name().replace('_', " ").to_ascii_uppercase());
                    s.push_str(" (D-PAD LEFT)");
                    self.say(&s, now);
                    crate::devlog::event(now, "crate", format_args!("reward={}", streak.name()));
                }
                Note::Ammo { fighter: 0 } => {
                    refill();
                    self.say("AMMO", now);
                    crate::devlog::event(now, "crate", format_args!("ammo"));
                }
                Note::CrateLanded { origin } => crate::devlog::event(
                    now,
                    "crate_landed",
                    format_args!("x={:.0} y={:.0} z={:.0}", origin[0], origin[1], origin[2]),
                ),
                _ => {}
            }
        }
    }

    /// X held by a crate takes it. Returns whether a crate is in reach (X captures, not reloads).
    pub fn capture(&mut self, a: &mut Arena, origin: [f32; 3], held: bool, now: u32) -> bool {
        self.capture = a.use_crate(0, origin, held, now);
        self.capture.is_some()
    }

    /// The killstreak lines: a capture bar, the last note.
    pub fn draw_hud(
        &self,
        gfx: &mut impl game_core::gfx::Gfx,
        rects: &mut Vec<game_core::hud::Rect>,
        now: u32,
    ) {
        if let Some(p) = self.capture {
            rects.clear();
            game_core::hud::text(rects, 236, 300, 2, "HOLD X FOR THE CRATE");
            gfx.hud_rects(rects, 0xf0f0f0e0);
            gfx.hud_rects(&[[220, 320, 200, 8]], 0x00000090);
            if p > 0.0 {
                gfx.hud_rects(&[[220, 320, (200.0 * p) as i16, 8]], 0xf0f0f0e0);
            }
        }
        if let Some((text, at)) = self
            .note
            .as_ref()
            .filter(|(_, at)| now.wrapping_sub(*at) < 3000)
        {
            let _ = at;
            rects.clear();
            game_core::hud::text(rects, 160, 120, 2, text);
            gfx.hud_rects(rects, 0xffd040e0);
        }
    }
}

/// The AC-130's view by gun (vertical degrees: the 105's wide thermal view, the 40's and 25's zoom).
const AC130_FOVY: [f32; 3] = [40.0, 25.0, 15.0];
/// How fast the remote views turn at full stick (degrees a second).
const REMOTE_LOOK: f32 = 60.0;

/// The player's remote killstreak view, if one is up: the Predator's nose (MW2's missile camera), the Chopper
/// Gunner's turret or the AC-130's guns.
pub fn remote(a: &Arena) -> Option<game_core::camera::Camera> {
    let s = &a.streaks;
    if let Some(m) = s.missiles.iter().find(|m| m.owner == 0 && m.linked) {
        return Some(game_core::camera::Camera::new(
            m.origin,
            m.angles[1],
            m.angles[0],
        ));
    }
    if let Some(g) = s.gunship.filter(|g| g.owner == 0) {
        let mut c = game_core::camera::Camera::new(g.eye(), g.aim[1], g.aim[0]);
        c.fovy = AC130_FOVY[g.gun];
        return Some(c);
    }
    let h = s
        .helis
        .iter()
        .find(|h| h.owner == 0 && h.kind == game_core::killstreaks::air::HeliKind::Gunner)?;
    let mut c = game_core::camera::Camera::new(h.origin_eye(), h.aim[1], h.aim[0]);
    c.fovy = 45.0;
    Some(c)
}

/// Whether the player is up in a killstreak (Chopper Gunner, AC-130): out of the match below meanwhile.
pub fn in_vehicle(a: &Arena) -> bool {
    let s = &a.streaks;
    s.gunship.is_some_and(|g| g.owner == 0)
        || s.helis
            .iter()
            .any(|h| h.owner == 0 && h.kind == game_core::killstreaks::air::HeliKind::Gunner)
}

/// The C-stick steers the player's remote killstreak: the Predator (up raises the nose, right turns right; fire
/// boosts) or the Chopper Gunner / AC-130 view (fire shoots, Y changes the AC-130's gun).
pub fn steer(a: &mut Arena, stick: (i8, i8), fire: bool, switch: bool, dt: f32) {
    if a.streaks.missiles.iter().any(|m| m.owner == 0 && m.linked) {
        a.steer_predator(0, stick.1, stick.0, fire, dt);
        return;
    }
    let k = |v: i8| {
        let v = f32::from(v) / 127.0;
        if libm::fabsf(v) < 0.15 { 0.0 } else { v }
    };
    let look = [
        -k(stick.1) * REMOTE_LOOK * dt,
        -k(stick.0) * REMOTE_LOOK * dt,
    ];
    a.steer_air(0, look, fire, switch);
}

/// The remote views' HUD: a crosshair and what is flying.
pub fn draw_remote_hud(
    gfx: &mut impl game_core::gfx::Gfx,
    rects: &mut Vec<game_core::hud::Rect>,
    a: &Arena,
    now: u32,
) {
    let s = &a.streaks;
    let line = if s.missiles.iter().any(|m| m.owner == 0 && m.linked) {
        "PREDATOR MISSILE: C-STICK STEERS, R BOOSTS"
    } else if let Some(g) = s.gunship.filter(|g| g.owner == 0) {
        let left = g.leave_ms.wrapping_sub(now) / 1000;
        let _ = left;
        [
            "AC-130: 105 MM  (Y: NEXT GUN)",
            "AC-130: 40 MM  (Y: NEXT GUN)",
            "AC-130: 25 MM  (Y: NEXT GUN)",
        ][g.gun]
    } else if in_vehicle(a) {
        "CHOPPER GUNNER: R FIRES"
    } else {
        return;
    };
    gfx.hud_rects(&[[300, 239, 40, 2], [319, 220, 2, 40]], 0xf0f0_f0c0);
    rects.clear();
    game_core::hud::text(rects, 140, 420, 2, line);
    gfx.hud_rects(rects, 0xf0f0_f0e0);
}

/// Stand-ins for the killstreaks' world models (not converted yet): crates, the Little Bird and the C-130.
pub fn stand_ins(a: &Arena, boxes: &mut Vec<[f32; 6]>, colours: &mut Vec<u32>) {
    let s = &a.streaks;
    for c in &s.crates {
        let o = c.origin;
        let (hx, hy) = (airdrop::CRATE_HALF[0], airdrop::CRATE_HALF[1]);
        boxes.push([
            o[0] - hx,
            o[1] - hy,
            o[2],
            o[0] + hx,
            o[1] + hy,
            o[2] + airdrop::CRATE_HEIGHT,
        ]);
        colours.push(if a.enemies(0, c.owner) {
            0x8a5a3aff
        } else {
            0x6a7a4aff
        });
    }
    // The Predator: a long thin box (others see it fall).
    for m in &s.missiles {
        let o = m.origin;
        boxes.push([
            o[0] - 12.0,
            o[1] - 12.0,
            o[2] - 12.0,
            o[0] + 12.0,
            o[1] + 12.0,
            o[2] + 12.0,
        ]);
        colours.push(0xd0d0d0ff);
    }
    // Sentries: a base and a gun box along its aim (a carried one green where it may stand, red where not).
    for t in &s.sentries {
        let o = t.origin;
        boxes.push([
            o[0] - 16.0,
            o[1] - 16.0,
            o[2],
            o[0] + 16.0,
            o[1] + 16.0,
            o[2] + 32.0,
        ]);
        colours.push(match (t.carried, t.preview_ok) {
            (true, true) => 0x40c04090,
            (true, false) => 0xc0404090,
            _ => 0x505858ff,
        });
        let m = [
            o[0] + t.aim[0] * 16.0,
            o[1] + t.aim[1] * 16.0,
            o[2] + 40.0 + t.aim[2] * 16.0,
        ];
        boxes.push([
            m[0] - 8.0,
            m[1] - 8.0,
            m[2] - 6.0,
            m[0] + 8.0,
            m[1] + 8.0,
            m[2] + 6.0,
        ]);
        colours.push(if t.overheated { 0xc06030ff } else { 0x303838ff });
    }
    // Helicopters and the Harrier, the strike planes and their bombs.
    for h in &s.helis {
        let o = h.origin;
        let (l, ht, col) = match h.kind {
            game_core::killstreaks::air::HeliKind::Harrier => (160.0, 40.0, 0x606870ff),
            game_core::killstreaks::air::HeliKind::PaveLow => (260.0, 90.0, 0x3a4436ff),
            _ => (200.0, 70.0, 0x2e3a2eff),
        };
        boxes.push([
            o[0] - l,
            o[1] - l * 0.3,
            o[2] - ht * 0.5,
            o[0] + l,
            o[1] + l * 0.3,
            o[2] + ht * 0.5,
        ]);
        colours.push(col);
    }
    for p in &s.planes {
        let o = p.origin;
        boxes.push([
            o[0] - 200.0,
            o[1] - 200.0,
            o[2] - 30.0,
            o[0] + 200.0,
            o[1] + 200.0,
            o[2] + 30.0,
        ]);
        colours.push(0x505860ff);
    }
    for b in &s.bombs {
        let o = b.origin;
        boxes.push([
            o[0] - 10.0,
            o[1] - 10.0,
            o[2] - 10.0,
            o[0] + 10.0,
            o[1] + 10.0,
            o[2] + 10.0,
        ]);
        colours.push(0x303030ff);
    }
    for d in &s.drops {
        let o = d.origin;
        let (l, w, h) = match d.carrier {
            airdrop::Carrier::LittleBird => (90.0, 30.0, 40.0),
            airdrop::Carrier::C130 => (500.0, 300.0, 90.0),
        };
        boxes.push([
            o[0] - l,
            o[1] - l,
            o[2] - h * 0.5,
            o[0] + l,
            o[1] + l,
            o[2] + h,
        ]);
        colours.push(0x404850ff);
        let _ = w;
    }
}

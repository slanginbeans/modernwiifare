//! The local player: GameCube pad -> IW4 `UserCmd` -> IW4L `pmove` against the map's collision.
//!
//! Movement is IW4L's, unchanged. The pmove context below uses MW2's multiplayer dvar defaults (as in
//! `iw4l_probe::movement_trace`); weapon speed scales are 1.0 until weapons exist. Look speed on the C-stick is a
//! first guess and must be tuned against the 360 reference (feel-parity skill) before anyone calls it right.

use movement_iw4::{
    AdsFracContext, AdsIntentContext, AirMoveContext, CmdScaleWalkContext,
    CreateAnimsMantleRootDelta, FlatMantleAnimLength, JumpLaunchContext, MeleeChargeWeaponDelays,
    MoveBounds, PmoveSingleContext, SprintContext, ViewAngleClamp, WalkMoveContext, pmove,
};
use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons};

use crate::camera::Camera;
use crate::collision::{MASK_PLAYER_SOLID, MapClip, PLAYER_MAXS, PLAYER_MINS};

/// One frame of movement with absolute view angles (degrees) and IW4 move amounts (-127..127).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Steer {
    pub yaw: f32,
    pub pitch: f32,
    pub forward: i8,
    pub right: i8,
    pub jump: bool,
    pub crouch: bool,
    /// Prone (IW4 `BUTTON_PRONE`; wins over crouch).
    pub prone: bool,
    pub sprint: bool,
    pub ads: bool,
    /// Fire and reload held: pmove sees them (they end a sprint, IW4 `SPRINT_INTERFERING`).
    pub fire: bool,
    pub reload: bool,
    /// Hold breath (IW4 `BUTTON_BREATH`): steadies a scope while fully aimed (`movement_iw4` breath).
    pub breath: bool,
}

/// One step of the player's input, already shaped (`look`): IW4 move amounts and the view change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MovePad {
    /// IW4 move amounts, -127..127 (forward, right).
    pub forward: i8,
    pub right: i8,
    /// View change this step, degrees (pitch down positive, yaw left positive), from [`crate::look::look_frame`].
    pub look: [f32; 2],
    pub jump: bool,
    /// Stance bits from [`StanceButton`] (`buttons::CROUCH` / `buttons::PRONE`).
    pub stance: u32,
    pub sprint: bool,
    pub ads: bool,
    pub fire: bool,
    pub reload: bool,
    /// Hold breath: MW2's console `+breath_sprint` puts it on the sprint button ([`crate::controls::act::BREATH`]).
    pub breath: bool,
}

/// The console stance button (IW4L `input_iw4` `stance_button` / `stance_hold`): a tap toggles crouch; held
/// [`STANCE_HOLD_MS`] on the ground it goes prone (or stands up from prone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StanceButton {
    /// `buttons::CROUCH`, `buttons::PRONE` or 0.
    pub latch: u32,
    held: Option<(u32, i32)>,
    was_down: bool,
}

/// IW4L `STANCE_HOLD_MS`.
pub const STANCE_HOLD_MS: i32 = 300;

impl StanceButton {
    /// The button's state at `now_ms` (`airborne`: the hold waits for the ground). Returns the stance bits.
    pub fn update(&mut self, down: bool, airborne: bool, now_ms: i32) -> u32 {
        if down != self.was_down {
            self.was_down = down;
            if down {
                self.held = Some((self.latch, now_ms));
                self.latch = buttons::CROUCH;
            } else if let Some((from, _)) = self.held.take()
                && from == buttons::CROUCH
            {
                self.latch = 0;
            }
        }
        if let Some((from, since)) = self.held {
            if airborne {
                self.held = Some((from, now_ms));
            } else if now_ms.wrapping_sub(since) >= STANCE_HOLD_MS {
                self.latch = if from == buttons::PRONE {
                    0
                } else {
                    buttons::PRONE
                };
                self.held = None;
            }
        }
        self.latch
    }

    /// Stand (a jump from crouch, a spawn).
    pub fn reset(&mut self) {
        *self = StanceButton {
            was_down: self.was_down,
            ..StanceButton::default()
        };
    }
}

/// Units per degree in IW4's 16-bit angles.
const ANGLE2SHORT: f32 = 65536.0 / 360.0;

#[derive(Clone)]
pub struct Player {
    pub ps: PlayerState,
    server_time: i32,
    old_buttons: u32,
    /// The last command's view angles (IW4 shorts), the one before, and its move (for aim spread).
    pub cmd_angles: [i32; 3],
    pub old_cmd_angles: [i32; 3],
    pub cmd_move: (i8, i8),
    /// View angles the pad has built, in degrees (pitch down positive, as IW4).
    yaw: f32,
    pitch: f32,
}

/// What the held weapon changes in movement (IW4L `sim::step::pmove_context` inputs): move-speed scales and sprint
/// time, ADS rates and whether ADS is allowed, melee delays, scope and breath.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponMove {
    pub move_speed_scale: f32,
    pub ads_move_speed_scale: f32,
    pub sprint_duration_scale: f32,
    pub ads_allowed: bool,
    pub aim_down_sight: bool,
    pub ads_in_rate: f32,
    pub ads_out_rate: f32,
    pub ads_reload_trans_time_ms: i32,
    pub segmented_reload: bool,
    pub rechamber_while_ads: bool,
    pub ads_fire_only: bool,
    pub melee_delay_ms: i32,
    pub melee_charge_delay_ms: i32,
    pub scope: bool,
    pub can_hold_breath: bool,
}

impl Default for WeaponMove {
    /// No weapon: IW4L's fallbacks (200 ms ADS, no ADS allowed, full speed).
    fn default() -> Self {
        WeaponMove {
            move_speed_scale: 1.0,
            ads_move_speed_scale: 0.5,
            sprint_duration_scale: 1.0,
            ads_allowed: false,
            aim_down_sight: false,
            ads_in_rate: 1.0 / 200.0,
            ads_out_rate: 1.0 / 200.0,
            ads_reload_trans_time_ms: 0,
            segmented_reload: false,
            rechamber_while_ads: true,
            ads_fire_only: false,
            melee_delay_ms: 0,
            melee_charge_delay_ms: 0,
            scope: false,
            can_hold_breath: false,
        }
    }
}

/// The mantle's root motion: IW4L's `CreateAnimsMantleRootDelta`, a straight line from the start to each clip's end
/// (forward 16 units and up by the transition height, then over the ledge), over `FlatMantleAnimLength`'s 400 / 200
/// ms. MW2 moves along the mantle clips' own root tracks (IW4L `sim::mantle_xanim`); those curves are not converted
/// yet, so the path is straight where MW2's bends. Before this the root delta was zero: `mantle::enter` put the
/// player at the ledge's end at once and the mantle "played" standing still there.
const MANTLE_ROOT: CreateAnimsMantleRootDelta = CreateAnimsMantleRootDelta;

/// IW4 `player_sprintTime` (seconds).
const PLAYER_SPRINT_TIME: f32 = 4.0;

fn context(old_buttons: u32, w: &WeaponMove) -> PmoveSingleContext {
    let air = AirMoveContext {
        player_spectate_speed_scale: 1.0,
        shellshock_gravity_scale: 1.0,
        shellshock_gravity_bias: 0.0,
    };
    PmoveSingleContext {
        walk: WalkMoveContext {
            cmd_scale: CmdScaleWalkContext {
                player_back_speed_scale: 0.7,
                player_strafe_speed_scale: 0.8,
                player_sprint_speed_scale: 1.5,
                player_last_stand_crawl_speed_scale: 0.15,
                weapon_move_speed_scale: w.move_speed_scale,
                weapon_ads_move_speed_scale: w.ads_move_speed_scale,
                // A shell shock whose `.shock` file says so slows movement (the arena sets `SHELLSHOCKED` only then).
                shellshock_affects_movement: true,
            },
            weapon_move_scale: 1.0,
            old_buttons,
            jump: JumpLaunchContext {
                jump_height: 39.0,
                dive: false,
                crouch_jump_scale: 1.0,
                jump_ladder_push_vel: 128.0,
            },
            air,
        },
        air,
        bounds: MoveBounds {
            mins: PLAYER_MINS,
            maxs: PLAYER_MAXS,
            tracemask: MASK_PLAYER_SOLID,
        },
        view_angles: ViewAngleClamp {
            pitch_up: 85.0,
            pitch_down: 85.0,
            unclamped_pitch_bit: false,
        },
        sprint: SprintContext {
            weapon_max_sprint_time: movement_iw4::get_max_sprint_time(
                w.sprint_duration_scale,
                PLAYER_SPRINT_TIME,
            ),
            sprint_forever: false,
            min_sprint_time_seconds: 1.0,
            sprint_delay_seconds: 0.0,
            sprint_forward_minimum: 105,
            stand_up_clear: true,
            sprint_recharge_pause_seconds: 0.0,
        },
        ads_intent: AdsIntentContext {
            ads_allowed: w.ads_allowed,
            weapon_def_scope: w.scope,
            sprint_hold_ads: false,
        },
        ads_frac: AdsFracContext {
            aim_down_sight: w.aim_down_sight,
            ads_in_rate: w.ads_in_rate,
            ads_out_rate: w.ads_out_rate,
            ads_reload_trans_time_ms: w.ads_reload_trans_time_ms,
            segmented_reload: w.segmented_reload,
            rechamber_while_ads: w.rechamber_while_ads,
            ads_fire_only: w.ads_fire_only,
        },
        melee_charge: MeleeChargeWeaponDelays {
            melee_delay_ms: w.melee_delay_ms,
            melee_charge_delay_ms: w.melee_charge_delay_ms,
        },
        player_melee_range: 64.0,
        old_buttons,
        weapon_blocks_prone: false,
        can_hold_breath: w.can_hold_breath,
    }
}

impl Player {
    /// A player standing at `origin` (dropped onto the floor below it, if any) facing `yaw` degrees.
    pub fn spawn(clip: &MapClip<'_>, origin: [f32; 3], yaw: f32) -> Player {
        let start = [origin[0], origin[1], origin[2] + 16.0];
        let end = [origin[0], origin[1], origin[2] - 256.0];
        let t = clip.trace(start, end, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYER_SOLID);
        let feet = if t.startsolid == 0 && t.fraction < 1.0 {
            let f = t.fraction;
            [
                start[0],
                start[1],
                start[2] + (end[2] - start[2]) * f + 0.125,
            ]
        } else {
            origin
        };
        let mut ps = PlayerState::ZERO;
        ps.origin = feet;
        ps.speed = 190;
        ps.gravity = 800;
        ps.health = 100;
        ps.max_health = 100;
        ps.move_speed_scale_multiplier = 1.0;
        ps.view_height_target = 60;
        ps.view_height_current = 60.0;
        ps.ground_entity_num = ENTITYNUM_NONE;
        ps.viewangles = [0.0, yaw, 0.0];
        let server_time = 1000;
        ps.command_time = server_time;
        Player {
            ps,
            server_time,
            old_buttons: 0,
            cmd_angles: [0; 3],
            old_cmd_angles: [0; 3],
            cmd_move: (0, 0),
            yaw,
            pitch: 0.0,
        }
    }

    /// Run one frame of `msec` milliseconds. Returns the fall damage of a landing this frame (IW4L's crash land).
    pub fn update(&mut self, clip: &MapClip<'_>, pad: &MovePad, w: &WeaponMove, msec: i32) -> i32 {
        self.steer(
            clip,
            &Steer {
                yaw: self.yaw + pad.look[1],
                pitch: self.pitch + pad.look[0],
                forward: pad.forward,
                right: pad.right,
                jump: pad.jump,
                crouch: pad.stance & buttons::CROUCH != 0,
                prone: pad.stance & buttons::PRONE != 0,
                sprint: pad.sprint,
                ads: pad.ads,
                fire: pad.fire,
                reload: pad.reload,
                breath: pad.breath,
            },
            w,
            msec,
        )
    }

    /// Run one frame with absolute view angles and move amounts (bots). Same as [`Player::update`] otherwise.
    pub fn steer(&mut self, clip: &MapClip<'_>, st: &Steer, w: &WeaponMove, msec: i32) -> i32 {
        self.yaw = st.yaw - libm::floorf(st.yaw / 360.0) * 360.0;
        self.pitch = st.pitch.clamp(-85.0, 85.0);
        let mut btn = 0;
        if st.jump {
            btn |= buttons::JUMP;
        }
        if st.prone {
            btn |= buttons::PRONE;
        } else if st.crouch {
            btn |= buttons::CROUCH;
        }
        if st.sprint {
            btn |= buttons::SPRINT;
        }
        if st.ads {
            btn |= buttons::ADS;
        }
        if st.fire {
            btn |= buttons::ATTACK;
        }
        if st.reload {
            btn |= buttons::RELOAD;
        }
        if st.breath {
            btn |= buttons::BREATH;
        }
        self.server_time += msec;
        let short = |deg: f32| (deg * ANGLE2SHORT) as i32 & 0xffff;
        let mut cmd = UserCmd {
            server_time: self.server_time,
            buttons: btn,
            angles: [
                short(self.pitch).wrapping_sub(short(self.ps.delta_angles[0])),
                short(self.yaw).wrapping_sub(short(self.ps.delta_angles[1])),
                0,
            ],
            forwardmove: st.forward,
            rightmove: st.right,
            ..UserCmd::default()
        };
        self.old_cmd_angles = self.cmd_angles;
        self.cmd_angles = cmd.angles;
        self.cmd_move = (cmd.forwardmove, cmd.rightmove);
        let result = pmove(
            &mut self.ps,
            &mut cmd,
            context(self.old_buttons, w),
            clip,
            &FlatMantleAnimLength::default(),
            &MANTLE_ROOT,
        );
        self.old_buttons = btn;
        result.pml.fall_damage
    }

    /// First-person camera at the player's eye.
    pub fn camera(&self, aspect: f32) -> Camera {
        let o = self.ps.origin;
        let mut cam = Camera::new(
            [o[0], o[1], o[2] + self.ps.view_height_current],
            self.ps.viewangles[1],
            self.ps.viewangles[0],
        );
        cam.aspect = aspect;
        cam
    }

    /// View angles (pitch, yaw) in degrees.
    pub fn view(&self) -> (f32, f32) {
        (self.pitch, self.yaw)
    }

    /// Eye position.
    pub fn eye(&self) -> [f32; 3] {
        let o = self.ps.origin;
        [o[0], o[1], o[2] + self.ps.view_height_current]
    }

    /// The top of the player's hull by stance (IW4: 70 standing, 50 crouched, 30 prone).
    pub fn hull_height(&self) -> f32 {
        if self.prone() {
            30.0
        } else if self.crouched() {
            50.0
        } else {
            70.0
        }
    }

    /// Whether prone (IW4 `PMF_PRONE`).
    pub fn prone(&self) -> bool {
        self.ps.pm_flags & playerstate_iw4::pm_flags::PRONE != 0
    }

    /// Whether crouched (IW4 `PMF_DUCKED`).
    pub fn crouched(&self) -> bool {
        self.ps.pm_flags & playerstate_iw4::pm_flags::CROUCH != 0
    }

    pub fn on_ground(&self) -> bool {
        self.ps.ground_entity_num != ENTITYNUM_NONE
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use wpk::clip::Clip;

    fn floor_clip() -> Clip {
        let (min, max) = ([-4096.0, -4096.0, -64.0], [4096.0, 4096.0, 0.0]);
        let planes = [
            [1.0, 0.0, 0.0, max[0]],
            [-1.0, 0.0, 0.0, -min[0]],
            [0.0, 1.0, 0.0, max[1]],
            [0.0, -1.0, 0.0, -min[1]],
            [0.0, 0.0, 1.0, max[2]],
            [0.0, 0.0, -1.0, -min[2]],
        ];
        let mut c = Clip::default();
        c.push_brush(&planes, &[0; 6], 1, 0);
        c
    }

    fn run(p: &mut Player, clip: &MapClip<'_>, pad: MovePad, ms: i32) {
        for _ in 0..ms / 16 {
            p.update(clip, &pad, &WeaponMove::default(), 16);
        }
    }

    #[test]
    fn falls_lands_and_runs_on_a_floor() {
        let c = floor_clip();
        let clip = MapClip::new(&c);
        let mut p = Player::spawn(&clip, [0.0, 0.0, 100.0], 0.0);
        // Spawn drops onto the floor below (within the 256-unit probe).
        assert!(p.ps.origin[2] < 1.0, "{:?}", p.ps.origin);
        run(&mut p, &clip, MovePad::default(), 500);
        assert!(p.on_ground());
        assert!(p.ps.origin[2].abs() < 1.0);
        let x0 = p.ps.origin[0];
        run(
            &mut p,
            &clip,
            MovePad {
                forward: 127,
                ..MovePad::default()
            },
            2000,
        );
        let v = (p.ps.origin[0] - x0) / 2.0;
        // MW2 base run speed is 190 u/s (acceleration eats a little).
        assert!((150.0..=195.0).contains(&v), "speed {v}");
        assert!(p.ps.origin[1].abs() < 1.0, "yaw 0 runs along +x");
        // Jump: leaves the ground and comes back.
        p.update(
            &clip,
            &MovePad {
                jump: true,
                ..MovePad::default()
            },
            &WeaponMove::default(),
            16,
        );
        run(&mut p, &clip, MovePad::default(), 48);
        assert!(!p.on_ground() && p.ps.origin[2] > 1.0, "{:?}", p.ps.origin);
        run(&mut p, &clip, MovePad::default(), 1500);
        assert!(p.on_ground());
    }

    /// Walks every spawn of a locally converted `out/packs/mp_rust.wpk` (never in git; skipped when absent).
    #[test]
    fn spawns_stand_on_the_real_map() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/mp_rust.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local pack, skipped");
            return;
        };
        if let Err(wpk::ParseError::UnsupportedVersion(v)) = wpk::Pack::parse(&bytes, false) {
            std::println!("local pack is version {v}, re-run `mwf-convert pack`; skipped");
            return;
        }
        let pack = wpk::Pack::parse(&bytes, true).unwrap();
        let c = Clip::decode(pack.section(wpk::tag::CLIP).unwrap().data).unwrap();
        let clip = MapClip::new(&c);
        let spawns: std::vec::Vec<wpk::records::SpawnRecord> =
            wpk::records::iter(pack.section(wpk::tag::SPAWNS).unwrap().data).collect();
        assert!(!spawns.is_empty());
        let mut moved = 0;
        for (i, s) in spawns.iter().enumerate() {
            let mut p = Player::spawn(&clip, s.origin, s.yaw);
            run(&mut p, &clip, MovePad::default(), 1000);
            assert!(p.on_ground(), "spawn {i} not grounded at {:?}", p.ps.origin);
            assert!(
                (p.ps.origin[2] - s.origin[2]).abs() < 40.0,
                "spawn {i}: {:?} vs {:?}",
                p.ps.origin,
                s.origin
            );
            let start = p.ps.origin;
            run(
                &mut p,
                &clip,
                MovePad {
                    forward: 127,
                    ..MovePad::default()
                },
                2000,
            );
            let o = p.ps.origin;
            assert!(
                o[2] > start[2] - 300.0,
                "spawn {i} fell through: {start:?} -> {o:?}"
            );
            if libm::hypotf(o[0] - start[0], o[1] - start[1]) > 50.0 {
                moved += 1;
            }
        }
        std::println!(
            "{} spawns, {moved} ran more than 50 units forward",
            spawns.len()
        );
        assert!(
            moved * 2 > spawns.len(),
            "most spawns should have room to run"
        );
    }
    #[test]
    fn the_stance_button_taps_to_crouch_and_holds_to_go_prone() {
        let mut s = StanceButton::default();
        let crouch = buttons::CROUCH;
        let prone = buttons::PRONE;
        // Tap: crouch, stays crouched; tap again: stand.
        assert_eq!(s.update(true, false, 0), crouch);
        assert_eq!(s.update(false, false, 100), crouch);
        assert_eq!(s.update(true, false, 1000), crouch);
        assert_eq!(s.update(false, false, 1100), 0);
        // Hold 300 ms: prone; hold again from prone: stand.
        s.update(true, false, 2000);
        assert_eq!(s.update(true, false, 2300), prone);
        assert_eq!(s.update(false, false, 2400), prone);
        s.update(true, false, 3000);
        assert_eq!(s.update(true, false, 3300), 0);
        s.update(false, false, 3400);
        // In the air the hold waits for the ground.
        s.update(true, true, 4000);
        assert_eq!(s.update(true, true, 4500), crouch);
        assert_eq!(s.update(true, false, 4600), crouch);
        assert_eq!(s.update(true, false, 4900), prone);
    }

    #[test]
    fn going_prone_lowers_the_view_and_hull() {
        let clip = floor_clip();
        let mc = MapClip::new(&clip);
        let mut p = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let pad = MovePad {
            stance: buttons::PRONE,
            ..MovePad::default()
        };
        for _ in 0..120 {
            p.update(&mc, &pad, &WeaponMove::default(), 16);
        }
        assert!(p.prone(), "prone");
        assert_eq!(p.hull_height(), 30.0);
        assert!(
            p.ps.view_height_current < 20.0,
            "{}",
            p.ps.view_height_current
        );
    }
    #[test]
    fn running_steps_about_twice_a_second() {
        let clip = floor_clip();
        let mc = MapClip::new(&clip);
        let mut p = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let pad = MovePad {
            forward: 127,
            ..MovePad::default()
        };
        let mut steps = 0;
        let mut last = p.ps.bob_cycle as u8;
        for _ in 0..120 {
            p.update(&mc, &pad, &WeaponMove::default(), 16);
            let bob = p.ps.bob_cycle as u8;
            if movement_iw4::bob_cycle_wrapped(last, bob) {
                steps += 1;
            }
            last = bob;
        }
        std::println!("{steps} steps in 1.9 s");
        assert!((2..=8).contains(&steps), "{steps}");
    }

    /// A box with a mantle face on +x: 40 units high, solid and `CONTENTS_MANTLE`, all sides `SURF_MANTLE_ON`.
    fn ledge_clip(height: f32) -> Clip {
        let mut c = floor_clip();
        let (min, max) = ([64.0, -64.0, 0.0], [192.0, 64.0, height]);
        let planes = [
            [1.0, 0.0, 0.0, max[0]],
            [-1.0, 0.0, 0.0, -min[0]],
            [0.0, 1.0, 0.0, max[1]],
            [0.0, -1.0, 0.0, -min[1]],
            [0.0, 0.0, 1.0, max[2]],
            [0.0, 0.0, -1.0, -min[2]],
        ];
        let on = 0x0200_0000;
        c.push_brush(&planes, &[on; 6], 1 | movement_iw4::CONTENTS_MANTLE, 0);
        c
    }

    /// Runs at a ledge holding jump; returns (mantle seen, frames in the mantle, final origin).
    fn try_mantle(p: &mut Player, clip: &MapClip<'_>, frames: usize) -> (bool, usize, [f32; 3]) {
        let mut seen = 0;
        for _ in 0..frames {
            p.update(
                clip,
                &MovePad {
                    forward: 127,
                    jump: true,
                    ..MovePad::default()
                },
                &WeaponMove::default(),
                16,
            );
            if p.ps.pm_flags & playerstate_iw4::pm_flags::MANTLE != 0 {
                seen += 1;
            }
        }
        (seen > 0, seen, p.ps.origin)
    }

    #[test]
    fn a_mantle_climbs_the_ledge_over_time() {
        let c = ledge_clip(40.0);
        let clip = MapClip::new(&c);
        let mut p = Player::spawn(&clip, [20.0, 0.0, 10.0], 0.0);
        run(&mut p, &clip, MovePad::default(), 300);
        let mut path = std::vec::Vec::new();
        let mut entered = None;
        for i in 0..90 {
            p.update(
                &clip,
                &MovePad {
                    forward: 127,
                    jump: true,
                    ..MovePad::default()
                },
                &WeaponMove::default(),
                16,
            );
            if p.ps.pm_flags & playerstate_iw4::pm_flags::MANTLE != 0 {
                entered.get_or_insert(i);
                path.push(p.ps.origin);
            }
        }
        let first = entered.expect("the ledge is mantled");
        // 400 ms up (CreateAnims: straight to the ledge top), MW2's 25 frames at 16 ms.
        assert!(
            (20..=27).contains(&path.len()),
            "{} mantle frames",
            path.len()
        );
        // The climb is spread over the mantle: the first frame is well below the top, the heights rise.
        assert!(
            path[0][2] < 30.0,
            "first mantle frame {:?} (entered at {first})",
            path[0]
        );
        for w in path.windows(2) {
            assert!(w[1][2] >= w[0][2] - 0.01, "{:?}", w);
        }
        let end = p.ps.origin;
        assert!((end[2] - 40.0).abs() < 2.0, "on the ledge: {end:?}");
        assert!(end[0] > 64.0, "over the edge: {end:?}");
    }

    /// Looks for mantle brushes in a locally converted `out/packs/mp_rust.wpk` and mantles one (skipped when absent).
    /// Prints the spot for `tools/autoplay/mantle.txt`.
    #[test]
    fn mantles_on_the_real_map() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/mp_rust.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local pack, skipped");
            return;
        };
        let Ok(pack) = wpk::Pack::parse(&bytes, true) else {
            std::println!("local pack is another version; skipped");
            return;
        };
        let c = Clip::decode(pack.section(wpk::tag::CLIP).unwrap().data).unwrap();
        let clip = MapClip::new(&c);
        let mut brushes = 0;
        let mut tried = 0;
        let mut worked = std::vec::Vec::new();
        for b in &c.brushes {
            if b.contents & movement_iw4::CONTENTS_MANTLE == 0 {
                continue;
            }
            brushes += 1;
            let r = b.first_plane as usize..b.first_plane as usize + b.plane_count as usize;
            let planes = &c.planes[r.clone()];
            let flags = &c.plane_flags[r];
            let mut lo = [f32::MIN; 3];
            let mut hi = [f32::MAX; 3];
            for pl in planes.iter().take(6) {
                for k in 0..3 {
                    if pl[k] == 1.0 {
                        hi[k] = pl[3];
                    } else if pl[k] == -1.0 {
                        lo[k] = -pl[3];
                    }
                }
            }
            // Each vertical side of the box: stand 30 units out, facing in.
            for (side, (axis, sign)) in [(0, 1.0f32), (0, -1.0), (1, 1.0), (1, -1.0)]
                .iter()
                .enumerate()
            {
                let fi = side; // planes 0..3 are +x, -x, +y, -y
                if flags.get(fi).copied().unwrap_or(0) & movement_iw4::SURF_MANTLE_ON_OR_OVER == 0 {
                    continue;
                }
                let mut o = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, hi[2]];
                o[*axis] = if *sign > 0.0 {
                    hi[*axis] + 30.0
                } else {
                    lo[*axis] - 30.0
                };
                let yaw = match (axis, *sign > 0.0) {
                    (0, true) => 180.0,
                    (0, false) => 0.0,
                    (_, true) => 270.0,
                    (_, false) => 90.0,
                };
                let mut p = Player::spawn(&clip, o, yaw);
                run(&mut p, &clip, MovePad::default(), 300);
                if !p.on_ground() {
                    continue;
                }
                tried += 1;
                let start = p.ps.origin;
                let (seen, frames, end) = try_mantle(&mut p, &clip, 90);
                if seen && end[2] > start[2] + 10.0 {
                    worked.push((start, yaw, end, frames));
                }
            }
        }
        std::println!(
            "{brushes} mantle brushes, {tried} sides tried, {} mantled",
            worked.len()
        );
        for (s, y, e, f) in worked.iter().take(8) {
            std::println!("  from {s:?} yaw {y} -> {e:?} ({f} frames)");
        }
        assert!(brushes > 0, "the converter keeps mantle brushes");
        assert!(
            !worked.is_empty(),
            "at least one mantle works on our collision"
        );
    }
}

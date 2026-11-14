//! The local player: GameCube pad -> IW4 `UserCmd` -> IW4L `pmove` against the map's collision.
//!
//! Movement is IW4L's, unchanged. The pmove context below uses MW2's multiplayer dvar defaults (as in
//! `iw4l_probe::movement_trace`); weapon speed scales are 1.0 until weapons exist. Look speed on the C-stick is a
//! first guess and must be tuned against the 360 reference (feel-parity skill) before anyone calls it right.

use movement_iw4::{
    AdsFracContext, AdsIntentContext, AirMoveContext, CmdScaleWalkContext, FlatMantleAnimLength,
    JumpLaunchContext, MeleeChargeWeaponDelays, MoveBounds, PmoveSingleContext, SprintContext,
    ViewAngleClamp, WalkMoveContext, ZeroMantleRootDelta, pmove,
};
use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons};

use crate::camera::Camera;
use crate::collision::{MASK_PLAYER_SOLID, MapClip, PLAYER_MAXS, PLAYER_MINS};
use crate::fly::axis;

/// One frame of movement with absolute view angles (degrees) and IW4 move amounts (-127..127).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Steer {
    pub yaw: f32,
    pub pitch: f32,
    pub forward: i8,
    pub right: i8,
    pub jump: bool,
    pub crouch: bool,
    pub sprint: bool,
    pub ads: bool,
    /// Fire and reload held: pmove sees them (they end a sprint, IW4 `SPRINT_INTERFERING`).
    pub fire: bool,
    pub reload: bool,
}

/// Pad state for one frame, already mapped from the GameCube controller.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MovePad {
    /// Main stick (move) and C-stick (look), −128..127, up / right positive.
    pub sx: i8,
    pub sy: i8,
    pub cx: i8,
    pub cy: i8,
    pub jump: bool,
    pub crouch: bool,
    pub sprint: bool,
    pub ads: bool,
    pub fire: bool,
    pub reload: bool,
}

/// Degrees per second at full C-stick deflection (placeholder, see the module note).
pub const LOOK_YAW_RATE: f32 = 200.0;
pub const LOOK_PITCH_RATE: f32 = 140.0;
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
                shellshock_affects_movement: false,
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
        let dt = msec as f32 / 1000.0;
        let yaw = self.yaw - axis(pad.cx) * LOOK_YAW_RATE * dt;
        let pitch = self.pitch - axis(pad.cy) * LOOK_PITCH_RATE * dt;
        let to_move = |v: i8| (axis(v) * 127.0) as i8;
        self.steer(
            clip,
            &Steer {
                yaw,
                pitch,
                forward: to_move(pad.sy),
                right: to_move(pad.sx),
                jump: pad.jump,
                crouch: pad.crouch,
                sprint: pad.sprint,
                ads: pad.ads,
                fire: pad.fire,
                reload: pad.reload,
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
        if st.crouch {
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
            &ZeroMantleRootDelta,
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
                sy: 127,
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
                    sy: 127,
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
}

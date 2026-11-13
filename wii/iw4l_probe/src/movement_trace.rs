use movement_iw4::{
    AdsFracContext, AdsIntentContext, AirMoveContext, CmdScaleWalkContext, CollisionBackend,
    FlatMantleAnimLength, GroundTraceInput, JumpLaunchContext, MeleeChargeWeaponDelays, MoveBounds,
    PmoveSingleContext, SprintContext, ViewAngleClamp, WalkMoveContext, ZeroMantleRootDelta, pmove,
};
use playerstate_iw4::{PlayerState, UserCmd, buttons};
use trace_iw4::{BrushRef, Trace, trace_box};

/// Number of ticks the standard trace runs.
pub const TRACE_TICKS: u32 = 1200;

/// `movement_trace(TRACE_TICKS).hash` as produced on x86-64 and on PPC 750CL (qemu), with IW4L at
/// the rev pinned in Cargo.toml. A change here means movement behaviour changed: treat it as a
/// feel change (see the feel-parity skill), never just update the number.
pub const REFERENCE_HASH: u64 = 0x98b5_d201_ad27_41d5;

const SOLID: u32 = 1;
const NO_FLAGS: [u32; 6] = [0; 6];

/// A convex brush with exactly 6 planes `[nx, ny, nz, dist]`, outward normals.
struct Brush([[f32; 4]; 6]);

const fn aabb(min: [f32; 3], max: [f32; 3]) -> Brush {
    Brush([
        [1.0, 0.0, 0.0, max[0]],
        [-1.0, 0.0, 0.0, -min[0]],
        [0.0, 1.0, 0.0, max[1]],
        [0.0, -1.0, 0.0, -min[1]],
        [0.0, 0.0, 1.0, max[2]],
        [0.0, 0.0, -1.0, -min[2]],
    ])
}

/// Test room: floor, an 16-unit step, a ramp rising along +x, a low ledge, and four walls.
fn room() -> [Brush; 9] {
    // Ramp: x 600..900, rising 0 -> 90 units (slope ~16.7 deg). Top plane through (600,0,0)
    // with normal proportional to (-0.3, 0, 1).
    let len = libm::sqrtf(0.3 * 0.3 + 1.0);
    let (nx, nz) = (-0.3 / len, 1.0 / len);
    let ramp = Brush([
        [nx, 0.0, nz, nx * 600.0],
        [0.0, 0.0, -1.0, 0.0],
        [1.0, 0.0, 0.0, 900.0],
        [-1.0, 0.0, 0.0, -600.0],
        [0.0, 1.0, 0.0, 128.0],
        [0.0, -1.0, 0.0, 128.0],
    ]);
    [
        aabb([-2048.0, -2048.0, -64.0], [2048.0, 2048.0, 0.0]),
        aabb([200.0, -128.0, 0.0], [400.0, 128.0, 16.0]),
        ramp,
        aabb([900.0, -128.0, 0.0], [1100.0, 128.0, 90.0]),
        aabb([-400.0, 300.0, 0.0], [-200.0, 500.0, 40.0]),
        aabb([2048.0, -2048.0, -64.0], [2112.0, 2048.0, 512.0]),
        aabb([-2112.0, -2048.0, -64.0], [-2048.0, 2048.0, 512.0]),
        aabb([-2048.0, 2048.0, -64.0], [2048.0, 2112.0, 512.0]),
        aabb([-2048.0, -2112.0, -64.0], [2048.0, -2048.0, 512.0]),
    ]
}

struct Room([Brush; 9]);

impl CollisionBackend for Room {
    fn trace(&self, i: GroundTraceInput) -> Trace {
        trace_box(
            self.0.iter().map(|b| BrushRef {
                planes: &b.0,
                contents: SOLID,
                plane_surface_flags: &NO_FLAGS,
                glass_encoded: 0,
            }),
            i.start,
            i.end,
            i.mins,
            i.maxs,
            i.tracemask,
        )
    }
}

fn context(old_buttons: u32) -> PmoveSingleContext {
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
                player_last_stand_crawl_speed_scale: 0.2,
                weapon_move_speed_scale: 1.0,
                weapon_ads_move_speed_scale: 0.5,
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
            mins: [-15.0, -15.0, 0.0],
            maxs: [15.0, 15.0, 70.0],
            tracemask: SOLID,
        },
        view_angles: ViewAngleClamp {
            pitch_up: 85.0,
            pitch_down: 85.0,
            unclamped_pitch_bit: false,
        },
        sprint: SprintContext {
            weapon_max_sprint_time: 4000,
            sprint_forever: false,
            min_sprint_time_seconds: 1.0,
            sprint_delay_seconds: 0.25,
            sprint_forward_minimum: 105,
            stand_up_clear: true,
            sprint_recharge_pause_seconds: 0.0,
        },
        ads_intent: AdsIntentContext {
            ads_allowed: true,
            weapon_def_scope: false,
            sprint_hold_ads: false,
        },
        ads_frac: AdsFracContext::default(),
        melee_charge: MeleeChargeWeaponDelays::default(),
        player_melee_range: 64.0,
        old_buttons,
        weapon_blocks_prone: false,
        can_hold_breath: false,
    }
}

/// Scripted input for tick `t`: run, strafe, turn, sprint, jump, crouch, ADS, back-pedal, with
/// uneven frame times (16/17/33/50 ms) so the clamping and integration paths are exercised.
fn script(t: u32) -> (i32, i8, i8, u32, i32) {
    let msec = [16, 17, 33, 16, 50, 17][(t % 6) as usize];
    let phase = t / 100;
    let (fwd, right, mut btn) = match phase {
        0 | 1 => (127, 0, 0),
        2 => (127, 0, buttons::SPRINT),
        3 => (127, 64, 0),
        4 => (90, -127, buttons::ADS),
        5 => (-127, 0, 0),
        6 => (127, 0, buttons::CROUCH),
        7 => (0, 127, 0),
        8 => (127, 30, buttons::SPRINT),
        9 => (127, 0, 0),
        10 => (-60, -60, buttons::ADS),
        _ => (127, 0, 0),
    };
    if t.is_multiple_of(37) {
        btn |= buttons::JUMP;
    }
    // Yaw sweeps as a slow triangle wave; units are IW "short angles" (65536 per turn).
    let yaw = ((t as i32 % 400) - 200).abs() * 40 - 4000;
    (msec, fwd, right, btn, yaw)
}

/// Result of a trace run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovementTrace {
    /// FNV-1a 64 over (origin, velocity, viewangles, pm_flags, ground entity) of every tick.
    pub hash: u64,
    /// Final origin as raw f32 bits, for a quick look on screen.
    pub final_origin_bits: [u32; 3],
}

fn mix(h: &mut u64, v: u32) {
    for b in v.to_be_bytes() {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

/// Run IW4L `pmove` for `ticks` ticks in the test room and hash the player state.
pub fn movement_trace(ticks: u32) -> MovementTrace {
    let world = Room(room());
    let mut ps = PlayerState::ZERO;
    ps.origin = [-100.0, 0.0, 1.0];
    ps.speed = 190;
    ps.health = 100;
    ps.max_health = 100;
    ps.move_speed_scale_multiplier = 1.0;
    ps.view_height_target = 60;
    ps.view_height_current = 60.0;
    ps.gravity = 800;
    ps.ground_entity_num = 0x7fe;

    let mut server_time = 1_000;
    ps.command_time = server_time;
    let mut old_buttons = 0;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;

    for t in 0..ticks {
        let (msec, fwd, right, btn, yaw) = script(t);
        server_time += msec;
        let mut cmd = UserCmd {
            server_time,
            buttons: btn,
            angles: [0, yaw, 0],
            forwardmove: fwd,
            rightmove: right,
            ..UserCmd::default()
        };
        let _ = pmove(
            &mut ps,
            &mut cmd,
            context(old_buttons),
            &world,
            &FlatMantleAnimLength::default(),
            &ZeroMantleRootDelta,
        );
        old_buttons = btn;
        for v in ps.origin.iter().chain(&ps.velocity).chain(&ps.viewangles) {
            mix(&mut h, v.to_bits());
        }
        mix(&mut h, ps.pm_flags);
        mix(&mut h, ps.ground_entity_num as u32);
    }

    MovementTrace {
        hash: h,
        final_origin_bits: ps.origin.map(f32::to_bits),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn trace_is_stable_and_physical() {
        let a = movement_trace(TRACE_TICKS);
        let b = movement_trace(TRACE_TICKS);
        assert_eq!(a, b, "same inputs must give the same trace");
        assert_eq!(
            a.hash, REFERENCE_HASH,
            "movement differs from the cross-arch reference"
        );
        let o = a.final_origin_bits.map(f32::from_bits);
        std::println!("hash {:016x} final origin {:?}", a.hash, o);
        // Still inside the room, never below the floor.
        assert!(o.iter().all(|v| v.is_finite()));
        assert!(
            o[0].abs() < 2048.0 && o[1].abs() < 2048.0 && o[2] >= 0.0,
            "{o:?}"
        );
    }

    #[test]
    fn player_walks_and_climbs() {
        // Sample mid-run state: after the first 200 ticks of running forward the player
        // should have moved well along the room.
        let early = movement_trace(200).final_origin_bits.map(f32::from_bits);
        std::println!("after 200 ticks: {early:?}");
        assert!(
            (early[0] - -100.0).abs() > 200.0 || early[1].abs() > 200.0,
            "{early:?}"
        );
    }
}

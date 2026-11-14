//! Match rules read from the user's own MW2 game-type scripts at convert time (`RULE` in `common.wpk`): health and
//! its regeneration (`_tweakables`, `_healthoverlay`), free-for-all's limits and points (`dm`), and move speed by the
//! primary weapon's class and with Lightweight (`_weapons`, `_perkfunctions`). The converter records which values it
//! found; the rest keep MW2's stock values ([`Rules::default`]).

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Bits of [`Rules::found`]: which values came from the scripts.
pub mod found {
    pub const MAX_HEALTH: u16 = 1 << 0;
    pub const REGEN_DELAY: u16 = 1 << 1;
    pub const REGEN_STEP: u16 = 1 << 2;
    pub const REGEN_INTERVAL: u16 = 1 << 3;
    pub const HURT_CUTOFF: u16 = 1 << 4;
    pub const VERY_HURT_WAIT: u16 = 1 << 5;
    pub const TIME_LIMIT: u16 = 1 << 6;
    pub const SCORE_LIMIT: u16 = 1 << 7;
    pub const KILL_SCORE: u16 = 1 << 8;
    pub const HEADSHOT_SCORE: u16 = 1 << 9;
    pub const ASSIST_SCORE: u16 = 1 << 10;
    pub const CLASS_SPEED: u16 = 1 << 11;
    pub const LIGHTWEIGHT_SPEED: u16 = 1 << 12;
    pub const UAV_TIME: u16 = 1 << 13;
    pub const TDM_TIME_LIMIT: u16 = 1 << 14;
    pub const TDM_SCORE_LIMIT: u16 = 1 << 15;
    /// Team points (in `found_team`).
    pub const TEAM_KILL_SCORE: u16 = 1;
    pub const TEAM_HEADSHOT_SCORE: u16 = 1 << 1;
    pub const ALL: u16 = u16::MAX;
    pub const ALL_TEAM: u16 = (1 << 2) - 1;
    /// Flash and stun (in `found_fx`).
    pub const FLASH: u16 = 1;
    pub const STUN: u16 = 1 << 1;
    pub const SHOCK_FLASH: u16 = 1 << 2;
    pub const SHOCK_STUN: u16 = 1 << 3;
    pub const CLAYMORE: u16 = 1 << 4;
    pub const EXPLOSIVES: u16 = 1 << 5;
    pub const ALL_FX: u16 = (1 << 6) - 1;
}

/// A shell shock's screen and movement (an IW4 `.shock` file: `bg_shock_screenType`, blur blend / fade, white and
/// shot fade, `bg_shock_movement`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShockFx {
    /// `hud_iw4::SCREEN_BLEND_*`.
    pub screen_type: i32,
    pub blur_blend_ms: i32,
    pub blur_fade_ms: i32,
    pub white_fade_ms: i32,
    pub shot_fade_ms: i32,
    /// The shock slows movement.
    pub movement: bool,
}

/// The weapon classes of [`Rules::class_speed`], in order (`_weapons.gsc` `updateMoveSpeedScale`).
pub const SPEED_CLASSES: [&str; 8] = [
    "rifle",
    "pistol",
    "mg",
    "smg",
    "spread",
    "rocketlauncher",
    "sniper",
    "riotshield",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    pub max_health: i32,
    /// No regeneration until this long after the last hit (`healthregentime`; 0 = none).
    pub regen_delay_ms: u32,
    /// Health fraction regained per regeneration tick once badly hurt.
    pub regen_step: f32,
    /// The regeneration loop's tick.
    pub regen_interval_ms: u32,
    /// At or under this fraction of health a player is "very hurt" (regenerates step by step, not at once).
    pub hurt_cutoff: f32,
    /// A very hurt player's steps start this long after the hit.
    pub very_hurt_wait_ms: u32,
    /// Free-for-all: time limit and score limit (0 = none), points per kill / headshot bonus / assist.
    pub time_limit_ms: u32,
    pub score_limit: u32,
    pub kill_score: u32,
    pub headshot_score: u32,
    pub assist_score: u32,
    /// Move speed scale by the primary weapon's class ([`SPEED_CLASSES`]).
    pub class_speed: [f32; 8],
    /// Lightweight's move speed scaler (multiplies the class's).
    pub lightweight_speed: f32,
    /// How long a UAV keeps the radar up (`_uav.gsc` `level.radarViewTime`).
    pub uav_time_ms: u32,
    /// Team deathmatch (`war.gsc`): time and score limits; points a kill and a headshot bonus in team modes
    /// (`_rank.gsc`), which also go to the team's score.
    pub tdm_time_limit_ms: u32,
    pub tdm_score_limit: u32,
    pub team_kill_score: u32,
    pub team_headshot_score: u32,
    /// Flashbang (`_flashgrenades.gsc`): seconds = distance amount × look amount × `flash_seconds`, the look amount
    /// raised to `flash_angle_min` below it and to 1 above `flash_angle_full`; under `flash_min_seconds` nothing.
    pub flash_seconds: f32,
    pub flash_angle_min: f32,
    pub flash_angle_full: f32,
    pub flash_min_seconds: f32,
    /// Stun (`_weapons.gsc` `onWeaponDamage`): seconds = `stun_base + stun_extra × (1 - distance / stun_radius)`.
    pub stun_radius: f32,
    pub stun_base: f32,
    pub stun_extra: f32,
    /// The flashbang's and the stun grenade's shocks.
    pub shock_flash: ShockFx,
    pub shock_stun: ShockFx,
    /// Claymores (`_weapons.gsc`): the trip cone's half angle (degrees) and minimum distance ahead, the delay
    /// between the trip and the blast, and the trip trigger's radius (a cylinder as high as twice that).
    pub claymore_cone_deg: f32,
    pub claymore_min_dist: f32,
    pub claymore_grace_ms: u32,
    pub claymore_radius: f32,
    /// How many C4 / claymores a player may have out (`scr_maxPerPlayerExplosives`): a new one blows the oldest.
    pub max_explosives: u32,
    pub found: u16,
    pub found_team: u16,
    pub found_fx: u16,
}

impl Default for Rules {
    /// MW2's stock values.
    fn default() -> Self {
        Rules {
            max_health: 100,
            regen_delay_ms: 5000,
            regen_step: 0.1,
            regen_interval_ms: 50,
            hurt_cutoff: 0.55,
            very_hurt_wait_ms: 3000,
            time_limit_ms: 600_000,
            score_limit: 1000,
            kill_score: 50,
            headshot_score: 50,
            assist_score: 10,
            class_speed: [0.95, 1.0, 0.875, 1.0, 0.95, 0.8, 1.0, 0.8],
            lightweight_speed: 1.07,
            uav_time_ms: 30_000,
            tdm_time_limit_ms: 600_000,
            tdm_score_limit: 500,
            team_kill_score: 100,
            team_headshot_score: 100,
            flash_seconds: 6.0,
            flash_angle_min: 0.5,
            flash_angle_full: 0.8,
            flash_min_seconds: 0.25,
            stun_radius: 512.0,
            stun_base: 2.0,
            stun_extra: 4.0,
            shock_flash: ShockFx {
                screen_type: 1,
                blur_blend_ms: 400,
                blur_fade_ms: 1000,
                white_fade_ms: 3500,
                shot_fade_ms: 1000,
                movement: false,
            },
            shock_stun: ShockFx {
                screen_type: 1,
                blur_blend_ms: 400,
                blur_fade_ms: 12_000,
                white_fade_ms: 7500,
                shot_fade_ms: 15_000,
                movement: true,
            },
            claymore_cone_deg: 70.0,
            claymore_min_dist: 20.0,
            claymore_grace_ms: 750,
            claymore_radius: 192.0,
            max_explosives: 2,
            found: 0,
            found_team: 0,
            found_fx: 0,
        }
    }
}

impl Rules {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.u32(self.max_health as u32);
        o.u32(self.regen_delay_ms);
        o.f32(self.regen_step);
        o.u32(self.regen_interval_ms);
        o.f32(self.hurt_cutoff);
        o.u32(self.very_hurt_wait_ms);
        o.u32(self.time_limit_ms);
        o.u32(self.score_limit);
        o.u32(self.kill_score);
        o.u32(self.headshot_score);
        o.u32(self.assist_score);
        for v in self.class_speed {
            o.f32(v);
        }
        o.f32(self.lightweight_speed);
        o.u32(self.uav_time_ms);
        o.u32(self.tdm_time_limit_ms);
        o.u32(self.tdm_score_limit);
        o.u32(self.team_kill_score);
        o.u32(self.team_headshot_score);
        for v in [
            self.flash_seconds,
            self.flash_angle_min,
            self.flash_angle_full,
            self.flash_min_seconds,
            self.stun_radius,
            self.stun_base,
            self.stun_extra,
        ] {
            o.f32(v);
        }
        for s in [&self.shock_flash, &self.shock_stun] {
            for v in [
                s.screen_type,
                s.blur_blend_ms,
                s.blur_fade_ms,
                s.white_fade_ms,
                s.shot_fade_ms,
                i32::from(s.movement),
            ] {
                o.u32(v as u32);
            }
        }
        o.u16(self.found);
        o.u16(self.found_team);
        o.u16(self.found_fx);
        o.f32(self.claymore_cone_deg);
        o.f32(self.claymore_min_dist);
        o.u32(self.claymore_grace_ms);
        o.f32(self.claymore_radius);
        o.u32(self.max_explosives);
        o
    }

    pub fn decode(b: &[u8]) -> Option<Rules> {
        if b.len() < 182 {
            return None;
        }
        let u = |i: usize| be::u32(b, i * 4);
        let shock = |i: usize| ShockFx {
            screen_type: u(i) as i32,
            blur_blend_ms: u(i + 1) as i32,
            blur_fade_ms: u(i + 2) as i32,
            white_fade_ms: u(i + 3) as i32,
            shot_fade_ms: u(i + 4) as i32,
            movement: u(i + 5) != 0,
        };
        let f = |i: usize| be::f32(b, i * 4);
        // Claymore and explosives values follow the found bits (packs before them keep the defaults).
        let d = Rules::default();
        let tail = b.len() >= 202;
        let tf = |o: usize, def: f32| if tail { be::f32(b, 182 + o) } else { def };
        let tu = |o: usize, def: u32| if tail { be::u32(b, 182 + o) } else { def };
        Some(Rules {
            max_health: u(0) as i32,
            regen_delay_ms: u(1),
            regen_step: f(2),
            regen_interval_ms: u(3),
            hurt_cutoff: f(4),
            very_hurt_wait_ms: u(5),
            time_limit_ms: u(6),
            score_limit: u(7),
            kill_score: u(8),
            headshot_score: u(9),
            assist_score: u(10),
            class_speed: core::array::from_fn(|k| f(11 + k)),
            lightweight_speed: f(19),
            uav_time_ms: u(20),
            tdm_time_limit_ms: u(21),
            tdm_score_limit: u(22),
            team_kill_score: u(23),
            team_headshot_score: u(24),
            flash_seconds: f(25),
            flash_angle_min: f(26),
            flash_angle_full: f(27),
            flash_min_seconds: f(28),
            stun_radius: f(29),
            stun_base: f(30),
            stun_extra: f(31),
            shock_flash: shock(32),
            shock_stun: shock(38),
            claymore_cone_deg: tf(0, d.claymore_cone_deg),
            claymore_min_dist: tf(4, d.claymore_min_dist),
            claymore_grace_ms: tu(8, d.claymore_grace_ms),
            claymore_radius: tf(12, d.claymore_radius),
            max_explosives: tu(16, d.max_explosives),
            found: be::u16(b, 176),
            found_team: be::u16(b, 178),
            found_fx: be::u16(b, 180),
        })
    }

    /// The move speed scale for a primary weapon of speed class `class` (None: 1.0), with Lightweight or not.
    pub fn move_speed(&self, class: Option<usize>, lightweight: bool) -> f32 {
        let base = class
            .and_then(|c| self.class_speed.get(c).copied())
            .unwrap_or(1.0);
        base * if lightweight {
            self.lightweight_speed
        } else {
            1.0
        }
    }

    /// The rules of `mode`: (time limit ms, score limit, points a kill, headshot bonus).
    pub fn of_mode(&self, team: bool) -> (u32, u32, u32, u32) {
        if team {
            (
                self.tdm_time_limit_ms,
                self.tdm_score_limit,
                self.team_kill_score,
                self.team_headshot_score,
            )
        } else {
            (
                self.time_limit_ms,
                self.score_limit,
                self.kill_score,
                self.headshot_score,
            )
        }
    }

    /// Kills to win at `kill_score` points each (no limit: `u32::MAX`).
    pub fn kill_limit(&self) -> u32 {
        if self.score_limit == 0 || self.kill_score == 0 {
            u32::MAX
        } else {
            self.score_limit.div_ceil(self.kill_score)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let r = Rules {
            max_health: 30,
            score_limit: 1500,
            found: found::MAX_HEALTH | found::SCORE_LIMIT,
            ..Rules::default()
        };
        assert_eq!(Rules::decode(&r.encode()), Some(r));
        assert!(Rules::decode(&r.encode()[..20]).is_none());
        assert_eq!(r.kill_limit(), 30);
        assert_eq!(r.move_speed(Some(2), true), 0.875 * 1.07);
        assert_eq!(r.move_speed(None, false), 1.0);
        assert_eq!(Rules::default().kill_limit(), 20);
        let none = Rules {
            score_limit: 0,
            ..Rules::default()
        };
        assert_eq!(none.kill_limit(), u32::MAX);
    }
}

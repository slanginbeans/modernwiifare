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
    pub const ALL: u16 = (1 << 13) - 1;
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
    pub found: u16,
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
            found: 0,
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
        o.u16(self.found);
        o
    }

    pub fn decode(b: &[u8]) -> Option<Rules> {
        if b.len() < 82 {
            return None;
        }
        let u = |i: usize| be::u32(b, i * 4);
        let f = |i: usize| be::f32(b, i * 4);
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
            found: be::u16(b, 80),
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

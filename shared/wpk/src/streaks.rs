//! Killstreak rules read from the user's own MW2 killstreak scripts at convert time (`KSRL` in `common.wpk`, ADR
//! 0007): the care package's crate odds and capture times, the Predator's launch, the sentry's bursts and heat, the
//! air strikes' runs and bombs, the helicopters' health, guns and stay, the AC-130's stay and reloads. The converter
//! records which values it found ([`StreakRules::found`]); the rest keep MW2's stock values ([`StreakRules::default`]).
//!
//! The section is a value count, the found bits and the values as `f32` in [`StreakRules::values`] order, so values
//! appended later read as their defaults from older packs.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// MW2's killstreak names (`killstreakTable`, `_killstreaks.gsc`), in the order the crate odds use.
pub const NAMES: [&str; 15] = [
    "uav",
    "counter_uav",
    "airdrop",
    "sentry",
    "predator_missile",
    "precision_airstrike",
    "harrier_airstrike",
    "helicopter",
    "airdrop_mega",
    "stealth_airstrike",
    "helicopter_flares",
    "helicopter_minigun",
    "ac130",
    "emp",
    "nuke",
];

/// Index of `name` in [`NAMES`].
pub fn index(name: &str) -> Option<usize> {
    NAMES.iter().position(|n| *n == name)
}

/// Bits of [`StreakRules::found`].
pub mod found {
    pub const CRATE_ODDS: u64 = 1 << 0;
    pub const MEGA_ODDS: u64 = 1 << 1;
    pub const CRATE_USE: u64 = 1 << 2;
    pub const CRATE_TIMEOUT: u64 = 1 << 3;
    pub const DROP_FLIGHT: u64 = 1 << 4;
    pub const C130: u64 = 1 << 5;
    pub const PREDATOR: u64 = 1 << 6;
    pub const SENTRY: u64 = 1 << 7;
    pub const SENTRY_BURST: u64 = 1 << 8;
    pub const AIRSTRIKE: u64 = 1 << 9;
    pub const AIRSTRIKE_BOMBS: u64 = 1 << 10;
    pub const STEALTH: u64 = 1 << 11;
    pub const STEALTH_BOMBS: u64 = 1 << 12;
    pub const HARRIER: u64 = 1 << 13;
    pub const HELI: u64 = 1 << 14;
    pub const HELI_STAY: u64 = 1 << 15;
    pub const AC130: u64 = 1 << 16;
    pub const ALL: u64 = (1 << 17) - 1;
}

/// What the killstreaks do, in the scripts' units (seconds, inches, inches a second, damage points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreakRules {
    pub found: u64,
    /// `_airdrop.gsc` `addCrateType( "airdrop", ... )`: the weights, ammo then by [`NAMES`] (0 = never).
    pub crate_ammo: f32,
    pub crate_odds: [f32; 15],
    /// The same for the emergency airdrop (`"airdrop_mega"`).
    pub mega_ammo: f32,
    pub mega_odds: [f32; 15],
    /// `crateOwnerCaptureThink`: the owner holds use this long; `useHoldThink`'s default for anyone else.
    pub crate_owner_use_s: f32,
    pub crate_other_use_s: f32,
    /// `dropTimeOut`: an unclaimed crate goes after this long.
    pub crate_timeout_s: f32,
    /// The Little Bird's run (`getPathStart` `lbHalfDistance`, `getFlyHeightOffset` `lbFlyHeight`, `heliSetup` /
    /// `doFlyBy` speeds in mph and the wait before it slows).
    pub drop_distance: f32,
    pub drop_height: f32,
    pub drop_fast_mph: f32,
    pub drop_slow_mph: f32,
    pub drop_fast_s: f32,
    /// The emergency airdrop's C-130 (`doC130FlyBy`: `planeHalfDistance`, `planeFlySpeed`).
    pub c130_distance: f32,
    pub c130_speed: f32,
    /// `_remotemissile.gsc` `init`: the launch above, behind and the aim ahead of the caller.
    pub predator_up: f32,
    pub predator_back: f32,
    pub predator_ahead: f32,
    /// `_autosentry.gsc`: `SENTRY_TIME_OUT`, `SENTRY_OVERHEAT_TIME`, health, bursts and pauses.
    pub sentry_timeout_s: f32,
    pub sentry_overheat_s: f32,
    pub sentry_health: f32,
    pub sentry_burst_min: f32,
    pub sentry_burst_max: f32,
    pub sentry_pause_min: f32,
    pub sentry_pause_max: f32,
    /// `_airstrike.gsc` `callStrike` (precision and Harrier): `planeHalfDistance`, `planeFlySpeed`,
    /// `planeFlyHeight` (old system: above the target) and `planeBombExplodeDistance`.
    pub strike_distance: f32,
    pub strike_speed: f32,
    pub strike_height: f32,
    pub strike_explode_distance: f32,
    /// `callStrike_bombEffect`: the bomb's traces (`repeat`, `minAngles`, `maxAngles`) and each one's `losRadiusDamage`.
    pub strike_traces: f32,
    pub strike_min_angle: f32,
    pub strike_max_angle: f32,
    pub strike_radius: f32,
    pub strike_max_damage: f32,
    pub strike_min_damage: f32,
    /// The stealth bomber's run and carpet (`callStrike_bomb`: `accuracyRadius`, `losRadiusDamage`).
    pub stealth_distance: f32,
    pub stealth_speed: f32,
    pub stealth_height: f32,
    pub stealth_accuracy: f32,
    pub stealth_radius: f32,
    pub stealth_max_damage: f32,
    pub stealth_min_damage: f32,
    /// `_harrier.gsc`: health, how long it defends, its 20 mm bursts.
    pub harrier_health: f32,
    pub harrier_stay_s: f32,
    pub harrier_burst: f32,
    pub harrier_range: f32,
    /// `_helicopter.gsc` `init`: `heli_maxhealth` (the Pave Low's twice), `heli_visual_range`,
    /// `heli_turretClipSize`, `heli_turretReloadTime`, `heli_targeting_delay`.
    pub heli_health: f32,
    pub heli_range: f32,
    pub heli_clip: f32,
    pub heli_reload_s: f32,
    pub heli_targeting_s: f32,
    /// `heli_leave_on_timeout`: the attack helicopter's and Pave Low's stay, and the Chopper Gunner's.
    pub heli_stay_s: f32,
    pub gunner_stay_s: f32,
    /// `_ac130.gsc`: `ac130_use_duration`, a turn of the rig (`ac130_Speed["rotate"]`), reloads by gun (105, 40,
    /// 25 mm).
    pub ac130_stay_s: f32,
    pub ac130_turn_s: f32,
    pub ac130_reload_s: [f32; 3],
}

impl Default for StreakRules {
    fn default() -> Self {
        StreakRules {
            found: 0,
            crate_ammo: 17.0,
            crate_odds: [
                17.0, 15.0, 0.0, 12.0, 12.0, 11.0, 7.0, 7.0, 0.0, 5.0, 5.0, 3.0, 3.0, 1.0, 0.0,
            ],
            mega_ammo: 12.0,
            mega_odds: [
                12.0, 16.0, 0.0, 16.0, 14.0, 10.0, 5.0, 5.0, 0.0, 3.0, 3.0, 2.0, 2.0, 0.0, 0.0,
            ],
            crate_owner_use_s: 0.5,
            crate_other_use_s: 3.0,
            crate_timeout_s: 90.0,
            drop_distance: 15000.0,
            drop_height: 850.0,
            drop_fast_mph: 250.0,
            drop_slow_mph: 75.0,
            drop_fast_s: 2.0,
            c130_distance: 24000.0,
            c130_speed: 2000.0,
            predator_up: 14000.0,
            predator_back: 7000.0,
            predator_ahead: 1500.0,
            sentry_timeout_s: 90.0,
            sentry_overheat_s: 8.0,
            sentry_health: 1000.0,
            sentry_burst_min: 20.0,
            sentry_burst_max: 120.0,
            sentry_pause_min: 0.15,
            sentry_pause_max: 0.35,
            strike_distance: 24000.0,
            strike_speed: 7000.0,
            strike_height: 850.0,
            strike_explode_distance: 1500.0,
            strike_traces: 12.0,
            strike_min_angle: 5.0,
            strike_max_angle: 55.0,
            strike_radius: 512.0,
            strike_max_damage: 200.0,
            strike_min_damage: 30.0,
            stealth_distance: 12000.0,
            stealth_speed: 2000.0,
            stealth_height: 950.0,
            stealth_accuracy: 512.0,
            stealth_radius: 896.0,
            stealth_max_damage: 300.0,
            stealth_min_damage: 50.0,
            harrier_health: 3000.0,
            harrier_stay_s: 45.0,
            harrier_burst: 25.0,
            harrier_range: 8192.0,
            heli_health: 1500.0,
            heli_range: 3500.0,
            heli_clip: 40.0,
            heli_reload_s: 1.5,
            heli_targeting_s: 0.5,
            heli_stay_s: 60.0,
            gunner_stay_s: 40.0,
            ac130_stay_s: 40.0,
            ac130_turn_s: 70.0,
            ac130_reload_s: [5.0, 3.0, 1.5],
        }
    }
}

impl StreakRules {
    /// Every value in section order.
    pub fn values(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(96);
        v.push(self.crate_ammo);
        v.extend_from_slice(&self.crate_odds);
        v.push(self.mega_ammo);
        v.extend_from_slice(&self.mega_odds);
        v.extend_from_slice(&[
            self.crate_owner_use_s,
            self.crate_other_use_s,
            self.crate_timeout_s,
            self.drop_distance,
            self.drop_height,
            self.drop_fast_mph,
            self.drop_slow_mph,
            self.drop_fast_s,
            self.c130_distance,
            self.c130_speed,
            self.predator_up,
            self.predator_back,
            self.predator_ahead,
            self.sentry_timeout_s,
            self.sentry_overheat_s,
            self.sentry_health,
            self.sentry_burst_min,
            self.sentry_burst_max,
            self.sentry_pause_min,
            self.sentry_pause_max,
            self.strike_distance,
            self.strike_speed,
            self.strike_height,
            self.strike_explode_distance,
            self.strike_traces,
            self.strike_min_angle,
            self.strike_max_angle,
            self.strike_radius,
            self.strike_max_damage,
            self.strike_min_damage,
            self.stealth_distance,
            self.stealth_speed,
            self.stealth_height,
            self.stealth_accuracy,
            self.stealth_radius,
            self.stealth_max_damage,
            self.stealth_min_damage,
            self.harrier_health,
            self.harrier_stay_s,
            self.harrier_burst,
            self.harrier_range,
            self.heli_health,
            self.heli_range,
            self.heli_clip,
            self.heli_reload_s,
            self.heli_targeting_s,
            self.heli_stay_s,
            self.gunner_stay_s,
            self.ac130_stay_s,
            self.ac130_turn_s,
        ]);
        v.extend_from_slice(&self.ac130_reload_s);
        v
    }

    /// The rules from values in section order (missing ones keep their defaults).
    pub fn from_values(found: u64, v: &[f32]) -> StreakRules {
        let d = StreakRules::default();
        let dv = d.values();
        let g = |i: usize| v.get(i).copied().unwrap_or(dv[i]);
        let arr = |at: usize| -> [f32; 15] { core::array::from_fn(|k| g(at + k)) };
        let mut i = 32;
        let mut n = || {
            i += 1;
            g(i - 1)
        };
        StreakRules {
            found,
            crate_ammo: g(0),
            crate_odds: arr(1),
            mega_ammo: g(16),
            mega_odds: arr(17),
            crate_owner_use_s: n(),
            crate_other_use_s: n(),
            crate_timeout_s: n(),
            drop_distance: n(),
            drop_height: n(),
            drop_fast_mph: n(),
            drop_slow_mph: n(),
            drop_fast_s: n(),
            c130_distance: n(),
            c130_speed: n(),
            predator_up: n(),
            predator_back: n(),
            predator_ahead: n(),
            sentry_timeout_s: n(),
            sentry_overheat_s: n(),
            sentry_health: n(),
            sentry_burst_min: n(),
            sentry_burst_max: n(),
            sentry_pause_min: n(),
            sentry_pause_max: n(),
            strike_distance: n(),
            strike_speed: n(),
            strike_height: n(),
            strike_explode_distance: n(),
            strike_traces: n(),
            strike_min_angle: n(),
            strike_max_angle: n(),
            strike_radius: n(),
            strike_max_damage: n(),
            strike_min_damage: n(),
            stealth_distance: n(),
            stealth_speed: n(),
            stealth_height: n(),
            stealth_accuracy: n(),
            stealth_radius: n(),
            stealth_max_damage: n(),
            stealth_min_damage: n(),
            harrier_health: n(),
            harrier_stay_s: n(),
            harrier_burst: n(),
            harrier_range: n(),
            heli_health: n(),
            heli_range: n(),
            heli_clip: n(),
            heli_reload_s: n(),
            heli_targeting_s: n(),
            heli_stay_s: n(),
            gunner_stay_s: n(),
            ac130_stay_s: n(),
            ac130_turn_s: n(),
            ac130_reload_s: [n(), n(), n()],
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let v = self.values();
        let mut o = Vec::with_capacity(12 + v.len() * 4);
        o.u32(v.len() as u32);
        o.u32((self.found >> 32) as u32);
        o.u32(self.found as u32);
        for x in v {
            o.f32(x);
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<StreakRules> {
        if b.len() < 12 {
            return None;
        }
        let n = (be::u32(b, 0) as usize).min((b.len() - 12) / 4);
        let found = (u64::from(be::u32(b, 4)) << 32) | u64::from(be::u32(b, 8));
        let v: Vec<f32> = (0..n).map(|i| be::f32(b, 12 + i * 4)).collect();
        Some(StreakRules::from_values(found, &v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_old_packs_keep_defaults() {
        let mut r = StreakRules {
            found: found::SENTRY | found::AC130,
            sentry_timeout_s: 77.0,
            ..StreakRules::default()
        };
        r.ac130_reload_s[2] = 1.25;
        r.mega_odds[14] = 3.0;
        assert_eq!(StreakRules::decode(&r.encode()), Some(r));
        // A pack with only the first 20 values: the rest are the defaults.
        let mut short = r.encode();
        short.truncate(12 + 20 * 4);
        short[..4].copy_from_slice(&20u32.to_be_bytes());
        let d = StreakRules::decode(&short).unwrap();
        assert_eq!(d.mega_odds[14], StreakRules::default().mega_odds[14]);
        assert_eq!(d.crate_odds, r.crate_odds);
        assert_eq!(d.sentry_timeout_s, 90.0);
        assert_eq!(StreakRules::default().values().len(), 85);
    }
}

//! One player's life in a match: health, damage (falls today; bullets once there are other players), death and
//! respawn, and the match clock.
//!
//! Sources: fall damage is IW4L `movement_iw4` (128 units safe, 300 fatal); the clock is IW4L `gamemode_iw4::ffa`'s
//! time limit. MW2's health regeneration, killcam and per-mode respawn rules live in its game-type scripts and come
//! with the GSC runtime (ADR 0002); until then a death waits [`RESPAWN_DELAY_MS`] (a placeholder, not MW2's flow) and
//! health does not regenerate.

/// Placeholder until the scripted death / killcam flow: time from death to respawn.
pub const RESPAWN_DELAY_MS: u32 = 3000;
/// Falling this far below the lowest spawn point kills (MW2 maps use trigger_hurt volumes; this stands in).
pub const OUT_OF_WORLD_DEPTH: f32 = 2048.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifeState {
    Alive,
    Dead { at_ms: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Life {
    pub health: i32,
    pub max_health: i32,
    pub state: LifeState,
    pub deaths: u32,
    /// When damage was last taken (for the damage vignette).
    pub last_damage_ms: u32,
}

impl Default for Life {
    fn default() -> Self {
        Life {
            health: 100,
            max_health: 100,
            state: LifeState::Alive,
            deaths: 0,
            last_damage_ms: 0,
        }
    }
}

impl Life {
    pub fn alive(&self) -> bool {
        self.state == LifeState::Alive
    }

    /// Take `amount` damage at `now_ms`. Returns true when this kills.
    pub fn damage(&mut self, amount: i32, now_ms: u32) -> bool {
        if !self.alive() || amount <= 0 {
            return false;
        }
        self.health = (self.health - amount).max(0);
        self.last_damage_ms = now_ms;
        if self.health == 0 {
            self.state = LifeState::Dead { at_ms: now_ms };
            self.deaths += 1;
            return true;
        }
        false
    }

    /// Dead long enough to respawn.
    pub fn ready_to_respawn(&self, now_ms: u32) -> bool {
        matches!(self.state, LifeState::Dead { at_ms } if now_ms.wrapping_sub(at_ms) >= RESPAWN_DELAY_MS)
    }

    /// Milliseconds until respawn (0 when alive or ready).
    pub fn respawn_in(&self, now_ms: u32) -> u32 {
        match self.state {
            LifeState::Dead { at_ms } => {
                RESPAWN_DELAY_MS.saturating_sub(now_ms.wrapping_sub(at_ms))
            }
            LifeState::Alive => 0,
        }
    }

    pub fn respawn(&mut self) {
        self.health = self.max_health;
        self.state = LifeState::Alive;
    }

    /// How strongly to tint the screen red, 0..1: missing health, flashing up right after a hit.
    pub fn hurt(&self, now_ms: u32) -> f32 {
        let missing = 1.0 - self.health as f32 / self.max_health.max(1) as f32;
        let since = now_ms.wrapping_sub(self.last_damage_ms) as f32;
        let flash = (1.0 - since / 500.0).max(0.0) * 0.5;
        (missing * 0.6 + flash).min(1.0)
    }
}

/// The match clock (IW4L `gamemode_iw4::ffa::TIME_LIMIT_MS`, 10 minutes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchClock {
    start_ms: u32,
    pub limit_ms: u32,
}

impl MatchClock {
    pub fn start(now_ms: u32) -> MatchClock {
        MatchClock {
            start_ms: now_ms,
            limit_ms: gamemode_iw4::ffa::TIME_LIMIT_MS,
        }
    }

    pub fn remaining_ms(&self, now_ms: u32) -> u32 {
        self.limit_ms
            .saturating_sub(now_ms.wrapping_sub(self.start_ms))
    }

    pub fn over(&self, now_ms: u32) -> bool {
        self.remaining_ms(now_ms) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dies_and_respawns_after_the_delay() {
        let mut l = Life::default();
        assert!(!l.damage(40, 1000));
        assert_eq!(l.health, 60);
        assert!(l.hurt(1000) > l.hurt(5000));
        assert!(l.damage(70, 2000));
        assert_eq!((l.health, l.deaths), (0, 1));
        assert!(!l.damage(10, 2100), "no damage while dead");
        assert!(!l.ready_to_respawn(2000 + RESPAWN_DELAY_MS - 1));
        assert_eq!(l.respawn_in(2000 + 1000), RESPAWN_DELAY_MS - 1000);
        assert!(l.ready_to_respawn(2000 + RESPAWN_DELAY_MS));
        l.respawn();
        assert!(l.alive() && l.health == 100);
    }

    #[test]
    fn fall_damage_is_iw4ls() {
        // Under 128 units nothing; past 300 always lethal (IW4L movement_iw4::fall_damage).
        assert_eq!(movement_iw4::fall_damage(100.0, false), 0);
        assert!(movement_iw4::fall_damage(400.0, false) >= 100);
        let c = MatchClock::start(0);
        assert_eq!(c.remaining_ms(60_000), 540_000);
        assert!(c.over(600_000));
    }
}

//! One player's life in a match: health, damage (falls today; bullets once there are other players), death and
//! respawn, and the match clock.
//!
//! Sources: fall damage is IW4L `movement_iw4` (128 units safe, 300 fatal). Health and its regeneration follow MW2's
//! `_healthoverlay.gsc` loop step by step ([`Life::regen`]), with the numbers read from the user's scripts at convert
//! time (`wpk::rules::Rules`); the clock's limit too. MW2's killcam and per-mode respawn rules come with the GSC
//! runtime (ADR 0002); until then a death waits [`RESPAWN_DELAY_MS`] (a placeholder, not MW2's flow).

use wpk::rules::Rules;

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
    regen: Regen,
}

/// The state of MW2's regeneration loop (`playerHealthRegen`): its next tick, the health it saw last tick, whether
/// the player is very hurt, and when the hurt began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Regen {
    next_ms: Option<u32>,
    old_health: i32,
    very_hurt: bool,
    hurt_ms: u32,
}

impl Regen {
    const fn new(max_health: i32) -> Regen {
        Regen {
            next_ms: None,
            old_health: max_health,
            very_hurt: false,
            hurt_ms: 0,
        }
    }
}

impl Default for Life {
    fn default() -> Self {
        Life {
            health: 100,
            max_health: 100,
            state: LifeState::Alive,
            deaths: 0,
            last_damage_ms: 0,
            regen: Regen::new(100),
        }
    }
}

impl Life {
    /// Full health under `rules`.
    pub fn with_rules(rules: &Rules) -> Life {
        let max = rules.max_health.max(1);
        Life {
            health: max,
            max_health: max,
            regen: Regen::new(max),
            ..Life::default()
        }
    }

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
        self.regen = Regen::new(self.max_health);
    }

    /// Run MW2's regeneration loop up to `now`: a tick every `regen_interval_ms`. A hit (health below last tick's)
    /// restarts the wait; after `regen_delay_ms` without one, a lightly hurt player is healed at once, a very hurt
    /// one (at or under `hurt_cutoff`) regains `regen_step` of full health a tick once `very_hurt_wait_ms` passed.
    pub fn regen(&mut self, now: u32, rules: &Rules) {
        if !self.alive() || rules.regen_delay_ms == 0 {
            return;
        }
        let interval = rules.regen_interval_ms.max(1);
        let mut t = *self.regen.next_ms.get_or_insert(now.wrapping_add(interval));
        if (now.wrapping_sub(t) as i32) > (8 * interval) as i32 {
            // A long stall (a menu, a load): don't replay it.
            t = now;
        }
        while (now.wrapping_sub(t) as i32) >= 0 {
            self.regen_tick(t, rules);
            t = t.wrapping_add(interval);
        }
        self.regen.next_ms = Some(t);
    }

    fn regen_tick(&mut self, now: u32, rules: &Rules) {
        let r = &mut self.regen;
        if self.health >= self.max_health {
            r.old_health = self.max_health;
            r.very_hurt = false;
            return;
        }
        let ratio = self.health as f32 / self.max_health.max(1) as f32;
        if ratio <= rules.hurt_cutoff {
            if !r.very_hurt {
                r.hurt_ms = now;
            }
            r.very_hurt = true;
        }
        if self.health < r.old_health {
            // Hurt since the last tick.
            r.old_health = self.health;
            r.hurt_ms = now;
            return;
        }
        if now.wrapping_sub(r.hurt_ms) < rules.regen_delay_ms {
            return;
        }
        let mut new = 1.0;
        if r.very_hurt {
            new = ratio;
            if now.wrapping_sub(r.hurt_ms) > rules.very_hurt_wait_ms {
                new += rules.regen_step;
            }
        }
        let new = new.min(1.0);
        // `setNormalHealth`: a fraction of full health (the small bias keeps 0.1 steps from losing a point).
        self.health = ((new * self.max_health as f32 + 1e-3) as i32).clamp(1, self.max_health);
        r.old_health = self.health;
    }

    /// How strongly to tint the screen red, 0..1: missing health, flashing up right after a hit.
    pub fn hurt(&self, now_ms: u32) -> f32 {
        let missing = 1.0 - self.health as f32 / self.max_health.max(1) as f32;
        let since = now_ms.wrapping_sub(self.last_damage_ms) as f32;
        let flash = (1.0 - since / 500.0).max(0.0) * 0.5;
        (missing * 0.6 + flash).min(1.0)
    }
}

/// The match clock: the time limit from the rules (`dm.gsc`; 0 = none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchClock {
    start_ms: u32,
    pub limit_ms: u32,
}

impl MatchClock {
    pub fn start(now_ms: u32) -> MatchClock {
        Self::with_limit(now_ms, gamemode_iw4::ffa::TIME_LIMIT_MS)
    }

    pub fn with_limit(now_ms: u32, limit_ms: u32) -> MatchClock {
        MatchClock {
            start_ms: now_ms,
            limit_ms: if limit_ms == 0 { u32::MAX } else { limit_ms },
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
    fn regenerates_as_mw2s_script() {
        let r = Rules::default();
        // Lightly hurt (60 of 100 > the 0.55 cutoff): full health at once, 5 s after the hit.
        let mut l = Life::with_rules(&r);
        l.regen(0, &r);
        l.damage(40, 1000);
        let mut t = 1000;
        while t < 1000 + r.regen_delay_ms - 50 {
            t += 16;
            l.regen(t, &r);
        }
        assert_eq!(l.health, 60, "nothing before the delay");
        for _ in 0..10 {
            t += 16;
            l.regen(t, &r);
        }
        assert_eq!(l.health, 100);
        // Very hurt (30): a tenth of full health a tick after the delay.
        l.damage(70, t);
        let hit = t;
        while l.health < 100 && t < hit + 10_000 {
            t += 16;
            l.regen(t, &r);
            assert!(l.health == 30 || t >= hit + r.regen_delay_ms);
        }
        assert_eq!(l.health, 100);
        let took = t - hit;
        // 5 s, then seven steps of 0.1 (30 -> 100) 50 ms apart.
        assert!((5_330..5_420).contains(&took), "{took}");
        // A new hit during the wait restarts it.
        l.damage(20, t);
        let mut later = t;
        for _ in 0..200 {
            later += 16;
            l.regen(later, &r);
            if later == t + 2000 {
                l.damage(10, later);
            }
        }
        assert_eq!(l.health, 70, "3.2 s after the second hit: still waiting");
        // No regeneration when the rules turn it off, or when dead.
        let off = Rules {
            regen_delay_ms: 0,
            ..r
        };
        let mut l = Life::with_rules(&off);
        l.damage(50, 0);
        l.regen(60_000, &off);
        assert_eq!(l.health, 50);
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

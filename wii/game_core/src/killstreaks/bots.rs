//! Bots calling the killstreaks they earn (a second and a half after earning one, oldest first; MW2 has no bots, so
//! how they use them is ours): the radar ones (UAV, Counter-UAV, EMP) and the nuke at once; the care packages thrown
//! ahead of them (and their crates taken: `airdrop::update`); the sentry set down in front once its placement is valid;
//! the Predator flown onto the nearest enemy and boosted once lined up; the air strikes on the nearest enemy, heading
//! from the bot to it; the helicopters and the AC-130 called (their gunners aim for them, `air`).

use super::Streak;
use crate::arena::{Arena, Nuke};
use crate::collision::MapClip;

/// How long a bot holds a new reward before calling it.
pub const CALL_DELAY_MS: u32 = 1500;
/// How long a bot tries to find a place for its sentry before giving up on it.
const SENTRY_TRIES_MS: u32 = 5000;

/// The nearest living enemy of `i` (where it stands).
pub fn nearest_enemy(a: &Arena, i: usize) -> Option<[f32; 3]> {
    let me = a.fighters[i].player.ps.origin;
    a.fighters
        .iter()
        .enumerate()
        .filter(|(j, f)| f.life.alive() && (*j != 0 || a.human_present) && a.enemies(i, *j))
        .map(|(_, f)| f.player.ps.origin)
        .min_by(|p, q| super::dist2(*p, me).total_cmp(&super::dist2(*q, me)))
}

impl Arena {
    /// Bots call their rewards, place their sentries and fly their missiles (before the killstreaks update).
    pub fn bots_use_killstreaks(&mut self, _clip: &MapClip<'_>, now: u32) {
        let n = self.fighters.len();
        self.streaks.fit(n);
        if self.streaks.bot_next_ms.len() < n {
            self.streaks.bot_next_ms.resize(n, None);
        }
        for i in 1..n {
            if self.fighters[i].brain.is_none() || !self.fighters[i].life.alive() {
                continue;
            }
            // Sentries: down as soon as one may stand there; given up after a while.
            if self.carries_sentry(i) && !self.place_sentry(i, now) {
                let given = self
                    .streaks
                    .sentries
                    .iter()
                    .find(|s| s.owner == i && s.carried)
                    .map_or(now, |s| s.expires_ms);
                if now.wrapping_sub(given) > SENTRY_TRIES_MS {
                    self.streaks
                        .sentries
                        .retain(|s| !(s.owner == i && s.carried));
                }
            }
            if self.streaks.rewards[i].is_empty() {
                self.streaks.bot_next_ms[i] = None;
                continue;
            }
            let due = *self.streaks.bot_next_ms[i].get_or_insert(now.wrapping_add(CALL_DELAY_MS));
            if (now.wrapping_sub(due) as i32) < 0 {
                continue;
            }
            self.streaks.bot_next_ms[i] = None;
            let streak = self.streaks.rewards[i].remove(0);
            self.bot_call(i, streak, now);
        }
        // Bots' Predators: steer onto the nearest enemy, boost once lined up.
        for k in 0..self.streaks.missiles.len() {
            let m = self.streaks.missiles[k];
            if !m.linked || self.fighters[m.owner].brain.is_none() {
                continue;
            }
            let Some(t) = nearest_enemy(self, m.owner) else {
                continue;
            };
            let want = math_iw4::vect_to_angles(core::array::from_fn(|j| t[j] - m.origin[j]));
            let (dp, dy) = (
                math_iw4::angle_subtract(want[0], m.angles[0]),
                math_iw4::angle_subtract(want[1], m.angles[1]),
            );
            // Positive pitch input raises the nose, positive yaw turns right (IW4L `steer`).
            let stick = |err: f32| (-err * 12.0).clamp(-127.0, 127.0) as i8;
            let lined_up = libm::fabsf(dp) < 3.0 && libm::fabsf(dy) < 3.0;
            self.steer_predator(m.owner, stick(dp), stick(dy), lined_up, 0.05);
        }
    }

    /// Bot `i` calls `streak`.
    pub fn bot_call(&mut self, i: usize, streak: Streak, now: u32) {
        let rules = self.rules;
        let f = &mut self.fighters[i];
        match streak {
            Streak::Uav => f.uav_until = Some(now.wrapping_add(rules.uav_time_ms)),
            Streak::CounterUav => f.counter_uav_until = Some(now.wrapping_add(rules.uav_time_ms)),
            Streak::Emp => f.emp_until = Some(now.wrapping_add(rules.emp_ms)),
            Streak::Nuke => {
                if self.nuke.is_none() {
                    self.nuke = Some(Nuke {
                        owner: i,
                        detonate_ms: now.wrapping_add(rules.nuke_timer_ms),
                        done: false,
                    });
                }
            }
            Streak::Airdrop | Streak::Mega => {
                let p = &self.fighters[i].player;
                let eye = p.eye();
                let (_, yaw) = p.view();
                let f = super::yaw_dir(yaw);
                // Lobbed ahead and up a little.
                let dir = [f[0] * 0.8, f[1] * 0.8, 0.6];
                let vel = p.ps.velocity;
                let ahead = [eye[0] + f[0] * 200.0, eye[1] + f[1] * 200.0, p.ps.origin[2]];
                self.throw_marker(i, streak, eye, dir, vel, ahead, now);
            }
            Streak::Sentry => self.give_sentry(i, now),
            Streak::Predator => self.launch_predator(i),
            Streak::Precision | Streak::Harrier | Streak::Stealth => {
                let me = self.fighters[i].player.ps.origin;
                let target = nearest_enemy(self, i).unwrap_or(me);
                let yaw = libm::atan2f(target[1] - me[1], target[0] - me[0]).to_degrees();
                self.call_strike(i, streak, target, yaw, now);
            }
            Streak::Helicopter | Streak::PaveLow | Streak::ChopperGunner | Streak::Ac130 => {
                self.call_air(i, streak, now)
            }
        }
        if matches!(
            streak,
            Streak::Uav | Streak::CounterUav | Streak::Emp | Streak::Nuke
        ) {
            self.streaks
                .notes
                .push(super::Note::Called { fighter: i, streak });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips;
    use crate::player::Player;

    #[test]
    fn a_bot_calls_what_it_earns_and_flies_its_predator_onto_you() {
        let floor = test_clips::boxes(&[([-30000.0, -30000.0, -64.0], [30000.0, 30000.0, 0.0])]);
        let clip = MapClip::new(&floor);
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([0.0, 0.0, 0.0], 0.0)];
        let human = Player::spawn(&clip, [1200.0, 900.0, 0.0], 0.0);
        let mut a = Arena::new(
            &clip,
            &spawns,
            1,
            human,
            wpk::rules::Rules::default(),
            |_| None,
        );
        a.streaks = super::super::Streaks::new(
            wpk::streaks::StreakRules::default(),
            super::super::Weapons::default(),
            super::super::MapInfo::default(),
        );
        a.streaks.fit(2);
        a.streaks.sets[1] = [Some((Streak::Uav, 3)), Some((Streak::Predator, 5)), None];
        a.streaks.on_kill(1, 3);
        a.streaks.on_kill(1, 4);
        a.streaks.on_kill(1, 5);
        assert_eq!(a.streaks.rewards[1], [Streak::Uav, Streak::Predator]);
        let mut ev = alloc::vec::Vec::new();
        let mut now = 0;
        // The UAV after 1.5 s, the Predator 1.5 s later; the missile comes down on the player.
        while now < 30_000 && a.fighters[0].life.alive() {
            now += 50;
            a.update_killstreaks(&clip, 0.05, now, &mut ev);
        }
        assert!(a.fighters[1].uav_until.is_some());
        assert!(
            !a.fighters[0].life.alive(),
            "the bot's Predator found the player"
        );
        assert!(ev.contains(&crate::arena::Event::Kill {
            attacker: 1,
            victim: 0
        }));
    }
}

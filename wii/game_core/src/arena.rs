//! A free-for-all match on one Wii: the player (slot 0, driven by the pad elsewhere) and bots, each with IW4L
//! movement, an IW4L gun and a life; bullets land on players' hit volumes ([`crate::damage`]) before the wall behind
//! them; kills, deaths and respawns at the spawn point furthest from everyone alive.
//!
//! MW2 FFA: first to 30 kills or the 10-minute clock (both are GSC-scripted in MW2; these are its defaults).

use alloc::vec::Vec;

use weapon_iw4::WeaponTickEvent;
use wpk::nav::NavGraph;

use crate::bot::{self, Brain, Contact, Senses, Skill};
use crate::collision::MapClip;
use crate::damage;
use crate::gun::{Gun, GunInput, MASK_SHOT, Shot, SpreadInput};
use crate::life::Life;
use crate::player::Player;

/// MW2 FFA score limit in kills.
pub const KILL_LIMIT: u32 = 30;
/// How far a bot looks to either side of where it faces.
const VIEW_HALF_DEG: f32 = 75.0;
/// How far gunfire is heard.
const HEARING: f32 = 2000.0;

pub struct Fighter {
    pub player: Player,
    pub life: Life,
    pub gun: Option<Gun>,
    /// None for the human.
    pub brain: Option<Brain>,
    pub kills: u32,
    pub last_fire_ms: Option<u32>,
    /// Who last hurt this fighter (credited with the kill).
    pub last_attacker: Option<usize>,
}

/// Something that happened this frame (for the HUD and sounds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// `attacker` hit `victim` for `damage` (`location`: IW4 hit location).
    Hit {
        attacker: usize,
        victim: usize,
        damage: i32,
        location: u8,
    },
    Kill {
        attacker: usize,
        victim: usize,
    },
    /// A fighter (not the human) fired.
    BotFired {
        bot: usize,
    },
}

/// The player's position for hit tests and for the bots to see.
pub fn feet_and_aim(p: &Player) -> ([f32; 3], [f32; 3]) {
    let o = p.ps.origin;
    let chest = if p.crouched() { 32.0 } else { 45.0 };
    (o, [o[0], o[1], o[2] + chest])
}

pub struct Arena {
    pub fighters: Vec<Fighter>,
    /// Whether the human (slot 0) is in the match (not free-flying): bots ignore and can't hit it otherwise.
    pub human_present: bool,
    spawn_count: usize,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum()
}

/// The spawn point furthest from every other living fighter (MW2 FFA picks spawns away from enemies).
pub fn pick_spawn(
    spawns: &[([f32; 3], f32)],
    others: &[[f32; 3]],
    salt: usize,
) -> Option<([f32; 3], f32)> {
    if spawns.is_empty() {
        return None;
    }
    if others.is_empty() {
        return Some(spawns[salt % spawns.len()]);
    }
    spawns.iter().copied().max_by(|a, b| {
        let near = |p: [f32; 3]| others.iter().map(|o| dist2(p, *o)).fold(f32::MAX, f32::min);
        near(a.0).total_cmp(&near(b.0))
    })
}

impl Arena {
    /// The human (slot 0) and `bots` bots, all at spawns. `gun(i)` arms fighter `i` (called again on respawn).
    pub fn new(
        clip: &MapClip<'_>,
        spawns: &[([f32; 3], f32)],
        bots: usize,
        human: Player,
        mut gun: impl FnMut(usize) -> Option<Gun>,
    ) -> Arena {
        let mut a = Arena {
            fighters: Vec::with_capacity(bots + 1),
            human_present: true,
            spawn_count: 0,
        };
        a.fighters.push(Fighter {
            player: human,
            life: Life::default(),
            gun: gun(0),
            brain: None,
            kills: 0,
            last_fire_ms: None,
            last_attacker: None,
        });
        for i in 1..=bots {
            let p = a.spawn_point(spawns, i);
            a.fighters.push(Fighter {
                player: Player::spawn(clip, p.0, p.1),
                life: Life::default(),
                gun: gun(i),
                brain: Some(Brain::new(Skill::REGULAR, 0x51ed_270b ^ (i as u32 * 7919))),
                kills: 0,
                last_fire_ms: None,
                last_attacker: None,
            });
        }
        a
    }

    fn spawn_point(&mut self, spawns: &[([f32; 3], f32)], who: usize) -> ([f32; 3], f32) {
        let others: Vec<[f32; 3]> = self
            .fighters
            .iter()
            .enumerate()
            .filter(|(i, f)| *i != who && f.life.alive())
            .map(|(_, f)| f.player.ps.origin)
            .collect();
        self.spawn_count += 1;
        pick_spawn(spawns, &others, self.spawn_count).unwrap_or(([0.0, 0.0, 200.0], 0.0))
    }

    /// Respawn fighter `i` (the human too: the caller re-arms its own gun).
    pub fn respawn(
        &mut self,
        clip: &MapClip<'_>,
        spawns: &[([f32; 3], f32)],
        i: usize,
        gun: Option<Gun>,
    ) {
        let (o, yaw) = self.spawn_point(spawns, i);
        let f = &mut self.fighters[i];
        f.player = Player::spawn(clip, o, yaw);
        f.life.respawn();
        f.gun = gun;
        f.last_attacker = None;
        if let Some(b) = f.brain.as_mut() {
            b.respawned();
        }
    }

    /// The leading kill count and whether the match is won.
    pub fn leader(&self) -> (usize, u32) {
        self.fighters
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f.kills))
            .max_by_key(|&(i, k)| (k, usize::MAX - i))
            .unwrap_or((0, 0))
    }

    pub fn won(&self) -> bool {
        self.leader().1 >= KILL_LIMIT
    }

    /// Resolve a bullet from fighter `from` (traced against the world as `shot`): the first player hit volume
    /// before the wall takes the weapon's damage. Returns the events.
    pub fn bullet(
        &mut self,
        from: usize,
        shot: &Shot,
        facts: &weapon_iw4::WeaponCombatFacts,
        now: u32,
        out: &mut Vec<Event>,
    ) {
        let mut best: Option<(f32, usize, u8)> = None;
        for (i, f) in self.fighters.iter().enumerate() {
            if i == from || !f.life.alive() || (i == 0 && !self.human_present) {
                continue;
            }
            let boxes = damage::boxes(f.player.ps.origin, f.player.crouched());
            if let Some((t, loc)) = damage::first_hit(shot.start, shot.end, &boxes)
                && best.is_none_or(|b| t < b.0)
            {
                best = Some((t, i, loc));
            }
        }
        let Some((t, victim, loc)) = best else {
            return;
        };
        let length = libm::sqrtf(dist2(shot.start, shot.end));
        let d = damage::bullet(facts, &facts.location_damage, t * length, loc);
        let v = &mut self.fighters[victim];
        v.last_attacker = Some(from);
        out.push(Event::Hit {
            attacker: from,
            victim,
            damage: d,
            location: loc,
        });
        if v.life.damage(d, now) {
            out.push(Event::Kill {
                attacker: from,
                victim,
            });
            if from != victim {
                self.fighters[from].kills += 1;
            }
        }
    }

    /// What fighter `me` perceives of the others.
    fn contacts(&self, clip: &MapClip<'_>, me: usize, now: u32) -> Vec<Contact> {
        let f = &self.fighters[me];
        let eye = f.player.eye();
        let (_, yaw) = f.player.view();
        self.fighters
            .iter()
            .enumerate()
            .filter(|(i, o)| *i != me && o.life.alive() && (*i != 0 || self.human_present))
            .map(|(i, o)| {
                let (feet, aim) = feet_and_aim(&o.player);
                let (_, to_yaw) = bot::look_angles(eye, aim);
                let mut off = to_yaw - yaw;
                off -= libm::floorf((off + 180.0) / 360.0) * 360.0;
                let in_view = libm::fabsf(off) <= VIEW_HALF_DEG;
                let visible = in_view && {
                    let t = clip.trace(eye, aim, [0.0; 3], [0.0; 3], MASK_SHOT);
                    t.fraction >= 1.0
                };
                let heard = o.last_fire_ms.is_some_and(|t| {
                    now.wrapping_sub(t) < 1500
                        && dist2(feet, f.player.ps.origin) < HEARING * HEARING
                });
                Contact {
                    id: i,
                    aim,
                    feet,
                    visible,
                    heard,
                }
            })
            .collect()
    }

    /// Run every bot for `msec`: think, move, shoot. Dead bots wait out the respawn delay.
    #[allow(clippy::too_many_arguments)]
    pub fn update_bots(
        &mut self,
        clip: &MapClip<'_>,
        nav: Option<&NavGraph>,
        spawns: &[([f32; 3], f32)],
        now: u32,
        msec: i32,
        mut regun: impl FnMut(usize) -> Option<Gun>,
        out: &mut Vec<Event>,
    ) {
        for i in 1..self.fighters.len() {
            if !self.fighters[i].life.alive() {
                if self.fighters[i].life.ready_to_respawn(now) {
                    let g = regun(i);
                    self.respawn(clip, spawns, i, g);
                }
                continue;
            }
            let contacts = self.contacts(clip, i, now);
            let f = &mut self.fighters[i];
            let clip_ammo = f.gun.as_ref().map_or(0, |g| g.ammo().0);
            let senses = Senses {
                now_ms: now,
                msec,
                feet: f.player.ps.origin,
                eye: f.player.eye(),
                view: f.player.view(),
                enemies: &contacts,
                clip: clip_ammo,
            };
            let Some(brain) = f.brain.as_mut() else {
                continue;
            };
            let act = brain.think(nav, &senses);
            let fall = f.player.steer(clip, &act.steer, msec);
            if fall > 0 && f.life.damage(fall, now) {
                out.push(Event::Kill {
                    attacker: i,
                    victim: i,
                });
            }
            let Some(g) = f.gun.as_mut() else {
                continue;
            };
            let p = &f.player;
            g.update_spread(
                msec,
                &SpreadInput {
                    ps: &p.ps,
                    cmd_angles: p.cmd_angles,
                    old_cmd_angles: p.old_cmd_angles,
                    forwardmove: p.cmd_move.0,
                    rightmove: p.cmd_move.1,
                },
            );
            let cam = p.camera(4.0 / 3.0);
            let (ev, shot) = g.update(
                clip,
                msec,
                &GunInput {
                    fire: act.fire,
                    reload: act.reload,
                    pm_flags: p.ps.pm_flags,
                },
                cam.eye,
                cam.forward(),
                cam.right(),
                cam.up(),
            );
            if matches!(ev, Some(WeaponTickEvent::ShotAccepted { .. })) {
                f.last_fire_ms = Some(now);
                out.push(Event::BotFired { bot: i });
            }
            if let Some(s) = shot {
                let facts = *g.facts();
                self.bullet(i, &s, &facts, now, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips::boxes;
    use weapon_iw4::CapturedCombatInput;
    use wpk::weapon::WeaponRecord;

    fn rifle() -> Gun {
        let c = CapturedCombatInput {
            fire_time_ms: 100,
            clip_size: 30,
            start_ammo: 300,
            max_ammo: 300,
            reload_time_ms: 2000,
            reload_empty_time_ms: 2500,
            raise_time_ms: 300,
            first_raise_time_ms: 300,
            drop_time_ms: 300,
            damage: 40,
            min_damage: 30,
            max_damage_range: 1500.0,
            min_damage_range: 3000.0,
            hip_spread_stand_min: 1.0,
            hip_spread_stand_max: 2.0,
            ads_spread: 0.5,
            location_damage: weapon_iw4::LOCATION_DAMAGE_IDENTITY,
            ..CapturedCombatInput::default()
        };
        Gun::new(&WeaponRecord::new("rifle_mp", c)).expect("usable")
    }

    #[test]
    fn spawns_keep_away_from_the_living() {
        let s = [
            ([0.0, 0.0, 0.0], 0.0),
            ([1000.0, 0.0, 0.0], 0.0),
            ([500.0, 0.0, 0.0], 0.0),
        ];
        assert_eq!(
            pick_spawn(&s, &[[0.0, 0.0, 0.0]], 0).map(|p| p.0[0]),
            Some(1000.0)
        );
    }

    #[test]
    fn bullets_hit_players_before_walls() {
        let clip = boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([300.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 1, human, |_| Some(rifle()));
        let bot_at = a.fighters[1].player.ps.origin;
        let shot = Shot {
            start: [bot_at[0] - 300.0, bot_at[1], 64.0],
            end: [bot_at[0] + 500.0, bot_at[1], 64.0],
            hit: true,
            normal: [0.0; 3],
        };
        let facts = *rifle().facts();
        let mut ev = Vec::new();
        a.bullet(0, &shot, &facts, 1000, &mut ev);
        assert!(matches!(
            ev[0],
            Event::Hit {
                victim: 1,
                damage: 40,
                location: damage::hitloc::HEAD,
                ..
            }
        ));
        assert_eq!(a.fighters[1].life.health, 60);
        // Three more head shots: dead, and the kill goes to the human.
        for t in 0..3 {
            a.bullet(0, &shot, &facts, 1100 + t, &mut ev);
        }
        assert!(!a.fighters[1].life.alive());
        assert_eq!(a.fighters[0].kills, 1);
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
    }

    #[test]
    fn a_bot_shoots_a_player_in_the_open() {
        let clip = boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        // The bot spawns furthest from the human, facing it.
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([400.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 1, human, |_| Some(rifle()));
        let mut ev = Vec::new();
        for f in 0..300u32 {
            a.update_bots(
                &mc,
                None,
                &spawns,
                1000 + f * 16,
                16,
                |_| Some(rifle()),
                &mut ev,
            );
        }
        let hits = ev
            .iter()
            .filter(|e| matches!(e, Event::Hit { victim: 0, .. }))
            .count();
        assert!(hits > 0, "the bot hit the player");
        assert!(!a.fighters[0].life.alive(), "and killed it within 5 s");
        assert_eq!(a.fighters[1].kills, 1);
    }

    /// Five bots on a locally converted `out/packs/mp_rust.wpk` (never in git; skipped when absent) for two minutes
    /// of match time: they move through the map and fight.
    #[test]
    fn bots_roam_and_fight_on_a_real_map() {
        extern crate std;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/mp_rust.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local mp_rust pack, skipped");
            return;
        };
        let Ok(pack) = wpk::Pack::parse(&bytes, true) else {
            std::println!("stale mp_rust pack, skipped");
            return;
        };
        let (Some(c), Some(n)) = (pack.section(wpk::tag::CLIP), pack.section(wpk::tag::NAV)) else {
            std::println!("mp_rust pack without a walk graph, skipped");
            return;
        };
        let clip = wpk::clip::Clip::decode(c.data).expect("clip");
        let nav = NavGraph::decode(n.data).expect("nav");
        let mc = MapClip::new(&clip);
        let spawns: Vec<([f32; 3], f32)> = wpk::records::iter::<wpk::records::SpawnRecord>(
            pack.section(wpk::tag::SPAWNS).expect("spawns").data,
        )
        .filter(|s| s.kind == wpk::records::spawn_kind::DM)
        .map(|s| (s.origin, s.yaw))
        .collect();
        let human = Player::spawn(&mc, spawns[0].0, spawns[0].1);
        let mut a = Arena::new(&mc, &spawns, 5, human, |i| (i > 0).then(rifle));
        a.human_present = false;
        let start: Vec<[f32; 3]> = a.fighters.iter().map(|f| f.player.ps.origin).collect();
        let mut furthest = [0.0f32; 6];
        let mut ev = Vec::new();
        let (mut hits, mut kills) = (0, 0);
        for f in 0..(120 * 60u32) {
            a.update_bots(
                &mc,
                Some(&nav),
                &spawns,
                1000 + f * 16,
                16,
                |_| Some(rifle()),
                &mut ev,
            );
            for (i, x) in a.fighters.iter().enumerate() {
                furthest[i] = furthest[i].max(dist2(x.player.ps.origin, start[i]).sqrt());
            }
            for e in ev.drain(..) {
                match e {
                    Event::Hit { .. } => hits += 1,
                    Event::Kill { .. } => kills += 1,
                    _ => {}
                }
            }
        }
        std::println!("mp_rust, 5 bots, 120 s: furthest {furthest:?}, {hits} hits, {kills} kills");
        assert!(
            furthest[1..].iter().filter(|&&d| d > 400.0).count() >= 3,
            "bots roam"
        );
        assert!(kills > 0, "bots fight");
    }
}

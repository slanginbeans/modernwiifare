//! A free-for-all match on one Wii: the player (slot 0, driven by the pad elsewhere) and bots, each with IW4L
//! movement, an IW4L gun and a life; bullets land on players' hit volumes ([`crate::damage`]) before the wall behind
//! them; kills, deaths and respawns at the spawn point furthest from everyone alive.
//!
//! MW2 FFA rules (`wpk::rules::Rules`, read from the user's `dm.gsc` / `_healthoverlay.gsc` at convert time): points
//! a kill and a headshot bonus, the score limit, health and its regeneration.

use alloc::vec::Vec;

use crate::nav::Navigator;
use weapon_iw4::WeaponTickEvent;

use crate::bot::{self, Act, Brain, Contact, Senses, Skill};
use crate::collision::{Body, MapClip};
use crate::damage;
use crate::gun::{Gun, GunInput, MASK_SHOT, Shot, SpreadInput};
use crate::life::Life;
use crate::player::Player;
use wpk::rules::Rules;
/// How far a bot looks to either side of where it faces, and how far it sees (IW4L `bots` sensor: a 90° field of
/// view, 5906 units).
const VIEW_HALF_DEG: f32 = 45.0;
const SIGHT_RANGE: f32 = 5906.0;
/// How far gunfire is heard.
const HEARING: f32 = 2000.0;
/// Bots run like IW4's server runs them: one usercmd (movement and weapon step) every 50 ms, thinking every second
/// one (IW4L `bots`: commands per server frame, a decision every 2 ticks). Steps are staggered across frames, and
/// bodies are drawn between their last two positions.
pub const BOT_TICK_MS: u32 = 50;
const THINK_EVERY: u32 = 2;

pub struct Fighter {
    pub player: Player,
    pub life: Life,
    pub gun: Option<Gun>,
    /// None for the human.
    pub brain: Option<Brain>,
    pub kills: u32,
    pub headshots: u32,
    /// Match points (`Rules::kill_score` a kill, `headshot_score` more for a headshot).
    pub score: u32,
    pub last_fire_ms: Option<u32>,
    /// Who last hurt this fighter (credited with the kill).
    pub last_attacker: Option<usize>,
    /// The spawn point used last (MW2 avoids it next time).
    pub last_spawn: Option<usize>,
    /// This fighter's bullet damage scale (Stopping Power: 1.4).
    pub bullet_scale: f32,
    /// Team in team modes (`TEAM_ALLIES` / `TEAM_AXIS`), `TEAM_NONE` in free-for-all.
    pub team: u8,
    /// Kills since this fighter's last death.
    pub streak: u32,
    /// A bot's UAV: until when it knows where everyone is.
    pub uav_until: Option<u32>,
    /// Bot stepping: when the next step is due, steps taken, the decision being carried out, and the position
    /// before the last step (with its time) for drawing in between.
    next_step_ms: Option<u32>,
    steps: u32,
    act: Act,
    prev_origin: [f32; 3],
    last_step_ms: u32,
}

impl Fighter {
    fn new(player: Player, gun: Option<Gun>, brain: Option<Brain>, rules: &Rules) -> Fighter {
        Fighter {
            prev_origin: player.ps.origin,
            player,
            life: Life::with_rules(rules),
            gun,
            brain,
            kills: 0,
            headshots: 0,
            score: 0,
            last_fire_ms: None,
            last_attacker: None,
            last_spawn: None,
            bullet_scale: 1.0,
            team: TEAM_NONE,
            streak: 0,
            uav_until: None,
            next_step_ms: None,
            steps: 0,
            act: Act::default(),
            last_step_ms: 0,
        }
    }

    /// Where to draw this fighter at `now`: between its last two steps (bots step every 50 ms).
    pub fn draw_origin(&self, now: u32) -> [f32; 3] {
        if self.brain.is_none() {
            return self.player.ps.origin;
        }
        let t = (now.wrapping_sub(self.last_step_ms) as f32 / BOT_TICK_MS as f32).clamp(0.0, 1.0);
        let o = self.player.ps.origin;
        core::array::from_fn(|k| self.prev_origin[k] + (o[k] - self.prev_origin[k]) * t)
    }
}

/// Something that happened this frame (for the HUD and sounds).
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// A grenade went off at `origin`.
    Explosion {
        origin: [f32; 3],
        weapon: u32,
    },
}

/// The player's position for hit tests and for the bots to see.
pub fn feet_and_aim(p: &Player) -> ([f32; 3], [f32; 3]) {
    let o = p.ps.origin;
    let chest = if p.crouched() { 32.0 } else { 45.0 };
    (o, [o[0], o[1], o[2] + chest])
}

/// The match's game type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Free-for-all (`dm.gsc`).
    #[default]
    Ffa,
    /// Team deathmatch (`war.gsc`).
    Tdm,
}

pub const TEAM_NONE: u8 = 0;
pub const TEAM_ALLIES: u8 = 1;
pub const TEAM_AXIS: u8 = 2;

/// The match's limits and points, from the rules for its mode or as set before the match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub score_limit: u32,
    pub time_limit_ms: u32,
    pub kill_score: u32,
    pub headshot_score: u32,
}

impl Limits {
    pub fn of(rules: &Rules, mode: Mode) -> Limits {
        let (time_limit_ms, score_limit, kill_score, headshot_score) =
            rules.of_mode(mode == Mode::Tdm);
        Limits {
            score_limit,
            time_limit_ms,
            kill_score,
            headshot_score,
        }
    }
}

pub struct Arena {
    pub fighters: Vec<Fighter>,
    pub rules: Rules,
    pub mode: Mode,
    pub limits: Limits,
    /// Team scores (index by team).
    pub team_score: [u32; 3],
    /// Kills in a row a bot needs for its UAV (the UAV's killstreakTable cost; 0 = bots get none).
    pub bot_uav_kills: u32,
    /// Grenades in flight or lying on the ground.
    pub grenades: Vec<crate::grenade::Grenade>,
    /// The thrown weapons' numbers by weapon index (for grenades the bots throw).
    pub equips: &'static [u8],
    rng: u32,
    /// Whether the human (slot 0) is in the match (not free-flying): bots ignore and can't hit it otherwise.
    pub human_present: bool,
    spawn_count: usize,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum()
}

/// MW2's free-for-all spawn weighting (`_spawnlogic.gsc` `getSpawnpoint_DM` and the penalties it applies): most
/// players near [`SPAWN_IDEAL_DIST`] away, a penalty for anyone within [`SPAWN_BAD_DIST`], a large one for each
/// enemy that can see the spawn and for the nearest enemy within [`NEARBY_ENEMY_RANGE`], and the spawn used last time
/// avoided. `jitter(i)` is MW2's `randomfloat(.2)` for spawn `i` (0..0.2). Returns the index of the best spawn.
pub fn pick_spawn(
    spawns: &[([f32; 3], f32)],
    others: &[[f32; 3]],
    last: Option<usize>,
    sees: impl Fn(usize, [f32; 3]) -> bool,
    jitter: impl Fn(usize) -> f32,
) -> Option<usize> {
    if spawns.is_empty() {
        return None;
    }
    let weight = |i: usize| {
        let p = spawns[i].0;
        let mut w = jitter(i);
        if !others.is_empty() {
            let (mut from_ideal, mut bad) = (0.0, 0.0);
            let mut nearest = f32::MAX;
            let mut sights = 0;
            for &o in others {
                let d = libm::sqrtf(dist2(p, o));
                if d < SPAWN_BAD_DIST {
                    bad += (SPAWN_BAD_DIST - d) / SPAWN_BAD_DIST;
                }
                from_ideal += libm::fabsf(d - SPAWN_IDEAL_DIST);
                nearest = nearest.min(d);
                if sees(i, o) {
                    sights += 1;
                }
            }
            let well = (SPAWN_IDEAL_DIST - from_ideal / others.len() as f32) / SPAWN_IDEAL_DIST;
            w += well - bad * 2.0;
            w -= SPAWN_LOS_PENALTY * sights as f32;
            if nearest < NEARBY_ENEMY_RANGE {
                w -= NEARBY_ENEMY_PENALTY * (1.0 - nearest / NEARBY_ENEMY_RANGE);
            }
        }
        if last == Some(i) {
            w -= SAME_SPAWN_PENALTY;
        }
        w
    };
    (0..spawns.len()).max_by(|&a, &b| weight(a).total_cmp(&weight(b)))
}

/// MW2's team spawn weighting (`_spawnlogic.gsc` `getSpawnpoint_NearTeam` with the penalties it applies): far from
/// enemies and near allies (`(enemy distances - 2 × ally distances) / players`), then as free-for-all a large penalty
/// for each enemy that sees the spawn and for the nearest enemy within [`NEARBY_ENEMY_RANGE`], and the last spawn
/// avoided. Returns the index of the best spawn.
pub fn pick_spawn_team(
    spawns: &[([f32; 3], f32)],
    allies: &[[f32; 3]],
    enemies: &[[f32; 3]],
    last: Option<usize>,
    sees: impl Fn(usize, [f32; 3]) -> bool,
    jitter: impl Fn(usize) -> f32,
) -> Option<usize> {
    if spawns.is_empty() {
        return None;
    }
    let players = allies.len() + enemies.len();
    let weight = |i: usize| {
        let p = spawns[i].0;
        let sum = |set: &[[f32; 3]]| set.iter().map(|&o| libm::sqrtf(dist2(p, o))).sum::<f32>();
        let mut w = jitter(i);
        if players > 0 {
            w += (sum(enemies) - 2.0 * sum(allies)) / players as f32;
        }
        let nearest = enemies
            .iter()
            .map(|&o| libm::sqrtf(dist2(p, o)))
            .fold(f32::MAX, f32::min);
        w -= SPAWN_LOS_PENALTY * enemies.iter().filter(|&&o| sees(i, o)).count() as f32;
        if nearest < NEARBY_ENEMY_RANGE {
            w -= NEARBY_ENEMY_PENALTY * (1.0 - nearest / NEARBY_ENEMY_RANGE);
        }
        if last == Some(i) {
            w -= SAME_SPAWN_PENALTY;
        }
        w
    };
    (0..spawns.len()).max_by(|&a, &b| weight(a).total_cmp(&weight(b)))
}

/// MW2's spawn weighting numbers (`_spawnlogic.gsc`; the distances are IW4L's `sim::SPAWN_IDEAL_DIST` /
/// `SPAWN_BAD_DIST` too).
pub const SPAWN_IDEAL_DIST: f32 = 1600.0;
pub const SPAWN_BAD_DIST: f32 = 1200.0;
const SPAWN_LOS_PENALTY: f32 = 100_000.0;
const NEARBY_ENEMY_RANGE: f32 = 1000.0;
const NEARBY_ENEMY_PENALTY: f32 = 2000.0;
const SAME_SPAWN_PENALTY: f32 = 1000.0;
/// Where MW2's spawn sight traces look from (above the spawn point) and to (a player's chest).
const SPAWN_SIGHT_HEIGHT: f32 = 50.0;

impl Arena {
    /// The human (slot 0) and `bots` bots, all at spawns. `gun(i)` arms fighter `i` (called again on respawn).
    pub fn new(
        clip: &MapClip<'_>,
        spawns: &[([f32; 3], f32)],
        bots: usize,
        human: Player,
        rules: Rules,
        mut gun: impl FnMut(usize) -> Option<Gun>,
    ) -> Arena {
        let mut a = Arena {
            fighters: Vec::with_capacity(bots + 1),
            rules,
            mode: Mode::Ffa,
            limits: Limits::of(&rules, Mode::Ffa),
            team_score: [0; 3],
            bot_uav_kills: 0,
            grenades: Vec::new(),
            equips: &[],
            rng: 0x2545_f491,
            human_present: true,
            spawn_count: 0,
        };
        a.fighters.push(Fighter::new(human, gun(0), None, &rules));
        for i in 1..=bots {
            let p = a.spawn_point(clip, spawns, i);
            a.fighters.push(Fighter::new(
                Player::spawn(clip, p.0, p.1),
                gun(i),
                Some(Brain::new(Skill::REGULAR, 0x51ed_270b ^ (i as u32 * 7919))),
                &rules,
            ));
        }
        a
    }

    /// Choose fighter `who`'s spawn (MW2's weighting, [`pick_spawn`]) and remember it.
    pub fn spawn_point(
        &mut self,
        clip: &MapClip<'_>,
        spawns: &[([f32; 3], f32)],
        who: usize,
    ) -> ([f32; 3], f32) {
        let live =
            |i: usize, f: &Fighter| i != who && f.life.alive() && (i != 0 || self.human_present);
        let others: Vec<[f32; 3]> = self
            .fighters
            .iter()
            .enumerate()
            .filter(|(i, f)| live(*i, f) && self.enemies(who, *i))
            .map(|(_, f)| f.player.ps.origin)
            .collect();
        let allies: Vec<[f32; 3]> = self
            .fighters
            .iter()
            .enumerate()
            .filter(|(i, f)| live(*i, f) && self.mode == Mode::Tdm && !self.enemies(who, *i))
            .map(|(_, f)| f.player.ps.origin)
            .collect();
        self.spawn_count += 1;
        let last = self.fighters.get(who).and_then(|f| f.last_spawn);
        let salt = self.spawn_count as u32 ^ self.rng;
        let sees = |i: usize, o: [f32; 3]| {
            let from = spawns[i].0;
            let a = [from[0], from[1], from[2] + SPAWN_SIGHT_HEIGHT];
            let b = [o[0], o[1], o[2] + SPAWN_SIGHT_HEIGHT];
            clip.trace(a, b, [0.0; 3], [0.0; 3], MASK_SHOT).fraction >= 1.0
        };
        let jitter = |i: usize| {
            let mut x = salt ^ (i as u32).wrapping_mul(0x9e37_79b9);
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as f32 / (1u32 << 24) as f32 * 0.2
        };
        let picked = match self.mode {
            Mode::Ffa => pick_spawn(spawns, &others, last, sees, jitter),
            Mode::Tdm => pick_spawn_team(spawns, &allies, &others, last, sees, jitter),
        };
        match picked {
            Some(i) => {
                if let Some(f) = self.fighters.get_mut(who) {
                    f.last_spawn = Some(i);
                }
                spawns[i]
            }
            None => ([0.0, 0.0, 200.0], 0.0),
        }
    }

    /// Respawn fighter `i` (the human too: the caller re-arms its own gun).
    pub fn respawn(
        &mut self,
        clip: &MapClip<'_>,
        spawns: &[([f32; 3], f32)],
        i: usize,
        gun: Option<Gun>,
    ) {
        let (o, yaw) = self.spawn_point(clip, spawns, i);
        let f = &mut self.fighters[i];
        f.player = Player::spawn(clip, o, yaw);
        f.prev_origin = o;
        f.act = Act::default();
        f.life.respawn();
        f.gun = gun;
        f.last_attacker = None;
        if let Some(b) = f.brain.as_mut() {
            b.respawned();
        }
    }

    /// Set the game type and limits before play: in team deathmatch the human and every second bot are allies, the
    /// other bots axis (3 against 3 with five bots).
    pub fn set_mode(&mut self, mode: Mode, limits: Limits) {
        self.mode = mode;
        self.limits = limits;
        for (i, f) in self.fighters.iter_mut().enumerate() {
            f.team = match mode {
                Mode::Ffa => TEAM_NONE,
                Mode::Tdm if i == 0 || i % 2 == 0 => TEAM_ALLIES,
                Mode::Tdm => TEAM_AXIS,
            };
        }
    }

    /// Whether fighters `a` and `b` are enemies (everyone else in free-for-all; the other team in team modes).
    pub fn enemies(&self, a: usize, b: usize) -> bool {
        if a == b {
            return false;
        }
        match self.mode {
            Mode::Ffa => true,
            Mode::Tdm => self.fighters[a].team != self.fighters[b].team,
        }
    }

    /// The winning team in team modes (None in free-for-all or on a tie).
    pub fn leading_team(&self) -> Option<u8> {
        let (a, x) = (self.team_score[1], self.team_score[2]);
        match self.mode {
            Mode::Tdm if a > x => Some(TEAM_ALLIES),
            Mode::Tdm if x > a => Some(TEAM_AXIS),
            _ => None,
        }
    }

    /// The leader and their score: highest score, then fewer deaths (IW4L `gamemode_iw4::ffa`), then lower slot.
    pub fn leader(&self) -> (usize, u32) {
        self.fighters
            .iter()
            .enumerate()
            .max_by_key(|&(i, f)| (f.score, u32::MAX - f.life.deaths, usize::MAX - i))
            .map_or((0, 0), |(i, f)| (i, f.score))
    }

    /// Someone (a player in free-for-all, a team in team modes) reached the score limit (0: none).
    pub fn won(&self) -> bool {
        let limit = self.limits.score_limit;
        if limit == 0 {
            return false;
        }
        match self.mode {
            Mode::Ffa => self.leader().1 >= limit,
            Mode::Tdm => self.team_score.iter().any(|&s| s >= limit),
        }
    }

    /// Fighters in scoreboard order (as [`Arena::leader`]).
    pub fn standings(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.fighters.len()).collect();
        order.sort_by_key(|&i| {
            let f = &self.fighters[i];
            (core::cmp::Reverse(f.score), f.life.deaths, i)
        });
        order
    }

    /// The bodies fighter `me` collides with: every other living fighter in the match.
    pub fn bodies_except(&self, me: usize) -> impl Iterator<Item = Body> + '_ {
        self.fighters
            .iter()
            .enumerate()
            .filter(move |(i, f)| *i != me && f.life.alive() && (*i != 0 || self.human_present))
            .map(|(_, f)| Body {
                origin: f.player.ps.origin,
                height: if f.player.crouched() { 50.0 } else { 70.0 },
            })
    }

    /// Run every living fighter's health regeneration up to `now`.
    pub fn regen(&mut self, now: u32) {
        let rules = self.rules;
        for f in &mut self.fighters {
            f.life.regen(now, &rules);
        }
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
        let d = (d as f32 * self.fighters[from].bullet_scale) as i32;
        let dir = [
            shot.end[0] - shot.start[0],
            shot.end[1] - shot.start[1],
            shot.end[2] - shot.start[2],
        ];
        self.hurt(from, victim, d, loc, dir, now, out);
    }

    /// A melee swing from fighter `from` at `eye` along the view axes (IW4L `fire_weapon_melee`): nine short traces
    /// ahead (64 units, a 10-unit cross); the nearest player hit before a wall takes `melee_damage` plus 0..4.
    /// Returns whether it hit someone.
    #[allow(clippy::too_many_arguments)]
    pub fn melee(
        &mut self,
        clip: &MapClip<'_>,
        from: usize,
        eye: [f32; 3],
        axes: [[f32; 3]; 3],
        melee_damage: i32,
        now: u32,
        out: &mut Vec<Event>,
    ) -> bool {
        if melee_damage <= 0 {
            return false;
        }
        let [forward, right, up] = axes;
        let (range, width, height) = (
            weapon_iw4::PLAYER_MELEE_RANGE_DEFAULT,
            weapon_iw4::PLAYER_MELEE_WIDTH_DEFAULT,
            weapon_iw4::PLAYER_MELEE_HEIGHT_DEFAULT,
        );
        let mut best: Option<(f32, usize)> = None;
        let n = weapon_iw4::melee_trace_count(width, height);
        for offset in weapon_iw4::MELEE_TRACE_OFFSETS.iter().take(n) {
            let end =
                weapon_iw4::melee_trace_end(eye, forward, right, up, range, width, height, *offset);
            let wall = clip.trace(eye, end, [0.0; 3], [0.0; 3], MASK_SHOT).fraction;
            for (i, f) in self.fighters.iter().enumerate() {
                if i == from || !f.life.alive() || (i == 0 && !self.human_present) {
                    continue;
                }
                let boxes = damage::boxes(f.player.ps.origin, f.player.crouched());
                if let Some((t, _)) = damage::first_hit(eye, end, &boxes)
                    && t < wall
                    && best.is_none_or(|b| t < b.0)
                {
                    best = Some((t, i));
                }
            }
        }
        let Some((_, victim)) = best else {
            return false;
        };
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        let d = melee_damage + (self.rng % 5) as i32;
        self.hurt(from, victim, d, damage::hitloc::NONE, forward, now, out);
        true
    }

    /// `victim` takes `d` from `from` at hit location `loc` along `dir`: the damage timer, the events, and the kill's
    /// points.
    #[allow(clippy::too_many_arguments)]
    fn hurt(
        &mut self,
        from: usize,
        victim: usize,
        d: i32,
        loc: u8,
        dir: [f32; 3],
        now: u32,
        out: &mut Vec<Event>,
    ) {
        // No friendly fire in team modes (MW2's default `scr_team_fftype` 0); your own grenade still hurts you.
        if from != victim && self.mode == Mode::Tdm && !self.enemies(from, victim) {
            return;
        }
        let v = &mut self.fighters[victim];
        v.last_attacker = Some(from);
        // The hit slows the victim for a moment (IW4L `update_damage_timer`: 100 ms a point, at most 750 ms).
        movement_iw4::update_damage_timer(&mut v.player.ps, d, Some(dir));
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
            self.fighters[victim].streak = 0;
            if from != victim {
                let head = loc == damage::hitloc::HEAD || loc == damage::hitloc::HELMET;
                let (rules, uav_kills, limits) = (self.rules, self.bot_uav_kills, self.limits);
                let k = &mut self.fighters[from];
                k.kills += 1;
                k.streak += 1;
                let mut points = limits.kill_score;
                if head {
                    k.headshots += 1;
                    points += limits.headshot_score;
                }
                k.score += points;
                // Team modes: the kill's points go to the team's score too (`war.gsc` `onNormalDeath`).
                let team = usize::from(k.team);
                if team != 0 {
                    self.team_score[team] += limits.kill_score;
                }
                let k = &mut self.fighters[from];
                if k.brain.is_some() && uav_kills > 0 && k.streak == uav_kills {
                    k.uav_until = Some(now.wrapping_add(rules.uav_time_ms));
                }
            }
        }
    }

    /// Fly every grenade for `dt` seconds; those whose fuse is up explode, hurting everyone in reach (the thrower
    /// too) and crediting the thrower with the kills.
    pub fn update_grenades(&mut self, clip: &MapClip<'_>, dt: f32, now: u32, out: &mut Vec<Event>) {
        let mut i = 0;
        while i < self.grenades.len() {
            if !self.grenades[i].step(clip, dt, now) {
                i += 1;
                continue;
            }
            let g = self.grenades.swap_remove(i);
            out.push(Event::Explosion {
                origin: g.origin,
                weapon: g.weapon,
            });
            for v in 0..self.fighters.len() {
                let f = &self.fighters[v];
                if !f.life.alive() || (v == 0 && !self.human_present) {
                    continue;
                }
                let (_, yaw) = f.player.view();
                let d = crate::grenade::blast_damage(
                    &g.facts,
                    g.origin,
                    f.player.ps.origin,
                    f.player.crouched(),
                    yaw,
                    |a, b| {
                        let t = clip.trace(
                            a,
                            b,
                            [0.0; 3],
                            [0.0; 3],
                            gamemode_iw4::G_CAN_DAMAGE_CONTENTS_MASK,
                        );
                        t.fraction >= 1.0 && t.startsolid == 0
                    },
                );
                if d > 0 {
                    let dir = core::array::from_fn(|k| f.player.ps.origin[k] - g.origin[k]);
                    self.hurt(g.owner, v, d, damage::hitloc::NONE, dir, now, out);
                }
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
            .filter(|(i, o)| {
                self.enemies(me, *i) && o.life.alive() && (*i != 0 || self.human_present)
            })
            .map(|(i, o)| {
                let (feet, aim) = feet_and_aim(&o.player);
                let (_, to_yaw) = bot::look_angles(eye, aim);
                let mut off = to_yaw - yaw;
                off -= libm::floorf((off + 180.0) / 360.0) * 360.0;
                let in_view = libm::fabsf(off) <= VIEW_HALF_DEG
                    && dist2(aim, eye) <= SIGHT_RANGE * SIGHT_RANGE;
                let visible = in_view && {
                    let t = clip.trace(eye, aim, [0.0; 3], [0.0; 3], MASK_SHOT);
                    t.fraction >= 1.0
                };
                // Under its UAV a bot knows where everyone is (as the radar shows them).
                let uav = f
                    .uav_until
                    .is_some_and(|u| (u.wrapping_sub(now) as i32) > 0);
                let heard = uav
                    || o.last_fire_ms.is_some_and(|t| {
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

    /// Run the bots that are due at `now`: each steps every [`BOT_TICK_MS`] (thinking every second step), so a
    /// frame steps about one or two of them. Dead bots wait out the respawn delay. (`_frame_msec`: the frame's time,
    /// unused: bots keep their own tick.)
    #[allow(clippy::too_many_arguments)]
    pub fn update_bots(
        &mut self,
        clip: &MapClip<'_>,
        mut nav: Option<&mut Navigator>,
        spawns: &[([f32; 3], f32)],
        now: u32,
        _frame_msec: i32,
        mut regun: impl FnMut(usize) -> Option<Gun>,
        out: &mut Vec<Event>,
    ) {
        // At most one new route a frame across all bots (A* is the costly part of a bot's turn).
        if let Some(n) = nav.as_deref_mut() {
            n.routes_left = 1;
        }
        for i in 1..self.fighters.len() {
            if !self.fighters[i].life.alive() {
                if self.fighters[i].life.ready_to_respawn(now) {
                    let g = regun(i);
                    self.respawn(clip, spawns, i, g);
                }
                continue;
            }
            // Due for a step? (Staggered: bot i starts i × 17 ms in.)
            let due = *self.fighters[i]
                .next_step_ms
                .get_or_insert(now.wrapping_add((i as u32 * 17) % BOT_TICK_MS));
            if (now.wrapping_sub(due) as i32) < 0 {
                continue;
            }
            // Catch up at most one step behind; never step faster than the tick.
            let next = if now.wrapping_sub(due) > BOT_TICK_MS {
                now.wrapping_add(BOT_TICK_MS)
            } else {
                due.wrapping_add(BOT_TICK_MS)
            };
            self.fighters[i].next_step_ms = Some(next);
            let msec = BOT_TICK_MS as i32;
            let think = self.fighters[i].steps.is_multiple_of(THINK_EVERY);
            self.fighters[i].steps += 1;
            if think {
                let contacts = self.contacts(clip, i, now);
                let f = &mut self.fighters[i];
                let senses = Senses {
                    now_ms: now,
                    msec: msec * THINK_EVERY as i32,
                    feet: f.player.ps.origin,
                    eye: f.player.eye(),
                    view: f.player.view(),
                    enemies: &contacts,
                    clip: f.gun.as_ref().map_or(0, |g| g.ammo().0),
                };
                if let Some(brain) = f.brain.as_mut() {
                    f.act = brain.think(nav.as_deref_mut(), &senses);
                }
            }
            clip.set_bodies(self.bodies_except(i));
            let f = &mut self.fighters[i];
            if f.brain.is_none() {
                continue;
            }
            let act = f.act;
            f.prev_origin = f.player.ps.origin;
            f.last_step_ms = now;
            let w = f.gun.as_ref().map(Gun::move_context).unwrap_or_default();
            // MW2's move speed for the primary's class (`_weapons.gsc`; bots carry no perks).
            f.player.ps.move_speed_scale_multiplier = self.rules.move_speed(
                f.gun.as_ref().and_then(|g| {
                    crate::perks::speed_class(
                        g.facts().weap_class,
                        g.name().starts_with("riotshield"),
                    )
                }),
                false,
            );
            let mut steer = act.steer;
            steer.fire = act.fire;
            steer.reload = act.reload;
            let fall = f.player.steer(clip, &steer, &w, msec);
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
                    melee: act.melee,
                    frag: act.frag,
                    pm_flags: p.ps.pm_flags,
                    perks: p.ps.perks,
                    ..GunInput::default()
                },
                cam.eye,
                cam.forward(),
                cam.right(),
                cam.up(),
            );
            g.sync_player(&mut f.player.ps);
            if matches!(ev, Some(WeaponTickEvent::ShotAccepted { .. })) {
                f.last_fire_ms = Some(now);
                out.push(Event::BotFired { bot: i });
            }
            if let Some(WeaponTickEvent::OffhandUsed {
                weapon,
                remaining_fuse_ms,
            }) = ev
                && let Some(e) =
                    wpk::records::get::<wpk::weapon::EquipRecord>(self.equips, weapon as usize)
            {
                let g = crate::grenade::Grenade::throw(
                    i,
                    weapon,
                    e,
                    cam.eye,
                    cam.forward(),
                    f.player.ps.velocity,
                    remaining_fuse_ms,
                    now,
                );
                self.grenades.push(g);
            }
            let swing = (ev == Some(WeaponTickEvent::MeleeFired)).then(|| {
                (
                    cam.eye,
                    [cam.forward(), cam.right(), cam.up()],
                    g.facts().melee_damage,
                )
            });
            if let Some(s) = shot {
                let facts = *g.facts();
                let extra: Vec<Shot> = g.extra_pellets().to_vec();
                self.bullet(i, &s, &facts, now, out);
                for p in &extra {
                    self.bullet(i, p, &facts, now, out);
                }
            }
            if let Some((eye, axes, melee_damage)) = swing {
                self.melee(clip, i, eye, axes, melee_damage, now, out);
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
    fn spawns_follow_mw2s_weights() {
        let s = [
            ([0.0, 0.0, 0.0], 0.0),
            ([1600.0, 0.0, 0.0], 0.0),
            ([500.0, 0.0, 0.0], 0.0),
            ([4000.0, 0.0, 0.0], 0.0),
        ];
        let blind = |_: usize, _: [f32; 3]| false;
        let none = |_: usize| 0.0;
        let enemy = [[0.0, 0.0, 0.0]];
        // The ideal distance (1600) beats both nearer and much farther points.
        assert_eq!(pick_spawn(&s, &enemy, None, blind, none), Some(1));
        // A spawn the enemy can see is out, whatever its distance.
        let sees_1 = |i: usize, _: [f32; 3]| i == 1;
        assert_eq!(pick_spawn(&s, &enemy, None, sees_1, none), Some(3));
        // The last spawn used is avoided.
        assert_eq!(pick_spawn(&s, &enemy, Some(1), blind, none), Some(3));
        // Nobody else alive: the random part decides.
        let favour_2 = |i: usize| if i == 2 { 0.1 } else { 0.0 };
        assert_eq!(pick_spawn(&s, &[], None, blind, favour_2), Some(2));
        assert_eq!(pick_spawn(&[], &enemy, None, blind, none), None);
    }

    #[test]
    fn bullets_hit_players_before_walls() {
        let clip = boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([300.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 1, human, Rules::default(), |_| Some(rifle()));
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
        // MW2 FFA: 50 a kill and 50 more for a headshot.
        assert_eq!((a.fighters[0].score, a.fighters[0].headshots), (100, 1));
        assert_eq!(a.standings()[0], 0);
        assert!(ev.contains(&Event::Kill {
            attacker: 0,
            victim: 1
        }));
    }

    #[test]
    fn a_frag_at_a_bots_feet_kills_it_for_the_thrower() {
        let clip = boxes(&[([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([1500.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 1, human, Rules::default(), |_| Some(rifle()));
        let bot = a.fighters[1].player.ps.origin;
        let frag = wpk::weapon::EquipRecord {
            fuse_time_ms: 100,
            timed_detonation: true,
            explosion_radius: 256,
            explosion_inner_damage: 130,
            explosion_outer_damage: 55,
            ..wpk::weapon::EquipRecord::default()
        };
        a.grenades.push(crate::grenade::Grenade::throw(
            0,
            9,
            frag,
            [bot[0], bot[1], bot[2] + 2.0],
            [0.0, 0.0, -1.0],
            [0.0; 3],
            None,
            1000,
        ));
        let mut ev = Vec::new();
        for t in 0..20 {
            a.update_grenades(&mc, 0.016, 1000 + t * 16, &mut ev);
        }
        assert!(ev.iter().any(|e| matches!(e, Event::Explosion { .. })));
        assert!(!a.fighters[1].life.alive(), "the bot died");
        assert!(
            a.fighters[0].life.alive(),
            "the thrower, 1500 units away, lived"
        );
        assert_eq!(a.fighters[0].kills, 1);
        assert!(a.grenades.is_empty());
    }

    #[test]
    fn team_deathmatch_teams_scores_and_friendly_fire() {
        let clip = boxes(&[([-4000.0, -4000.0, -64.0], [4000.0, 4000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let spawns = [
            ([0.0, 0.0, 0.0], 0.0),
            ([300.0, 0.0, 0.0], 180.0),
            ([600.0, 0.0, 0.0], 180.0),
        ];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 2, human, Rules::default(), |_| Some(rifle()));
        let rules = Rules::default();
        a.set_mode(Mode::Tdm, Limits::of(&rules, Mode::Tdm));
        // The human and bot 2 are allies, bot 1 axis.
        assert_eq!(
            [a.fighters[0].team, a.fighters[1].team, a.fighters[2].team],
            [TEAM_ALLIES, TEAM_AXIS, TEAM_ALLIES]
        );
        assert!(a.enemies(0, 1) && !a.enemies(0, 2));
        // Shooting a teammate does nothing.
        let mut ev = Vec::new();
        a.hurt(
            0,
            2,
            50,
            damage::hitloc::NONE,
            [1.0, 0.0, 0.0],
            1000,
            &mut ev,
        );
        assert_eq!(a.fighters[2].life.health, a.fighters[2].life.max_health);
        // Killing an enemy scores for the team.
        a.hurt(
            0,
            1,
            500,
            damage::hitloc::NONE,
            [1.0, 0.0, 0.0],
            1000,
            &mut ev,
        );
        assert!(!a.fighters[1].life.alive());
        assert_eq!(
            a.team_score[usize::from(TEAM_ALLIES)],
            rules.team_kill_score
        );
        assert_eq!(a.fighters[0].score, rules.team_kill_score);
        assert_eq!(a.leading_team(), Some(TEAM_ALLIES));
        // Bots only perceive enemies.
        assert!(a.contacts(&mc, 2, 1000).iter().all(|c| a.enemies(2, c.id)));
        // Team spawns: near allies, away from enemies.
        let s = [([0.0, 0.0, 0.0], 0.0), ([2000.0, 0.0, 0.0], 0.0)];
        let blind = |_: usize, _: [f32; 3]| false;
        let none = |_: usize| 0.0;
        assert_eq!(
            pick_spawn_team(
                &s,
                &[[2100.0, 0.0, 0.0]],
                &[[-300.0, 0.0, 0.0]],
                None,
                blind,
                none
            ),
            Some(1)
        );
    }

    #[test]
    fn players_block_each_other() {
        let clip = boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([150.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let a = Arena::new(&mc, &spawns, 1, human, Rules::default(), |_| Some(rifle()));
        let bot_x = a.fighters[1].player.ps.origin[0];
        let mut p = a.fighters[0].player.clone();
        let walk = crate::player::MovePad {
            forward: 127,
            ..Default::default()
        };
        mc.set_bodies(a.bodies_except(0));
        for _ in 0..90 {
            p.update(&mc, &walk, &crate::player::WeaponMove::default(), 16);
        }
        let x = p.ps.origin[0];
        assert!(
            x < bot_x - 29.0,
            "stopped at the bot's hull: {x} vs bot at {bot_x}"
        );
        assert!(x > bot_x - 40.0, "but walked up to it: {x}");
        // Without bodies the same walk goes through.
        mc.set_bodies(core::iter::empty());
        for _ in 0..30 {
            p.update(&mc, &walk, &crate::player::WeaponMove::default(), 16);
        }
        assert!(p.ps.origin[0] > bot_x, "{}", p.ps.origin[0]);
    }

    #[test]
    fn a_knife_kills_up_close_and_misses_beyond_reach() {
        let clip = boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([48.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 1, human, Rules::default(), |_| Some(rifle()));
        let bot = a.fighters[1].player.ps.origin;
        let eye = [bot[0] - 40.0, bot[1], 50.0];
        let axes = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut ev = Vec::new();
        // Out of reach (64 units): nothing.
        let far = [bot[0] - 120.0, bot[1], 50.0];
        assert!(!a.melee(&mc, 0, far, axes, 135, 1000, &mut ev));
        assert!(a.melee(&mc, 0, eye, axes, 135, 1000, &mut ev));
        assert!(
            !a.fighters[1].life.alive(),
            "MW2's knife (135) kills from full health"
        );
        assert_eq!(
            a.fighters[0].score,
            Rules::default().kill_score,
            "no headshot bonus"
        );
    }

    #[test]
    fn a_bot_shoots_a_player_in_the_open() {
        let clip = boxes(&[([-2000.0, -2000.0, -64.0], [2000.0, 2000.0, 0.0])]);
        let mc = MapClip::new(&clip);
        // The bot spawns furthest from the human, facing it.
        let spawns = [([0.0, 0.0, 0.0], 0.0), ([400.0, 0.0, 0.0], 180.0)];
        let human = Player::spawn(&mc, [0.0, 0.0, 0.0], 0.0);
        let mut a = Arena::new(&mc, &spawns, 1, human, Rules::default(), |_| Some(rifle()));
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
        let mut nav = Navigator::new(wpk::nav::NavGraph::decode(n.data).expect("nav"));
        let mc = MapClip::new(&clip);
        let spawns: Vec<([f32; 3], f32)> = wpk::records::iter::<wpk::records::SpawnRecord>(
            pack.section(wpk::tag::SPAWNS).expect("spawns").data,
        )
        .filter(|s| s.kind == wpk::records::spawn_kind::DM)
        .map(|s| (s.origin, s.yaw))
        .collect();
        let human = Player::spawn(&mc, spawns[0].0, spawns[0].1);
        let mut a = Arena::new(&mc, &spawns, 5, human, Rules::default(), |i| {
            (i > 0).then(rifle)
        });
        a.human_present = false;
        let start: Vec<[f32; 3]> = a.fighters.iter().map(|f| f.player.ps.origin).collect();
        let mut furthest = [0.0f32; 6];
        let mut ev = Vec::new();
        let (mut hits, mut kills) = (0, 0);
        for f in 0..(120 * 60u32) {
            a.update_bots(
                &mc,
                Some(&mut nav),
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

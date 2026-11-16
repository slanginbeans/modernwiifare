//! Bot brains: what an enemy sees and remembers turned into the same movement and weapon input a player gives.
//!
//! The motor (view turning limited by rate and acceleration, an 8° fire cone, a move goal turned into forward / right
//! amounts) follows IW4L's bot motor (`crates/bots/src/motor.rs`, Apache-2.0, rev `acc57a5`). Above it, written fresh
//! for the Wii in IW4L's shape (sense → intent → motor): pick the nearest enemy in view, react after a delay, track
//! with an aim error that settles, fire inside the cone, strafe while fighting; otherwise chase where an enemy was
//! last seen or heard, or roam the walk graph; jump and pick a new goal when stuck.
//!
//! Ported from IW4L's `bots` crate (logic only; it is bevy-bound): the contact memory ([`Memory`], `memory.rs`: a
//! contact seen this think, last seen, forgotten after 80 ticks, "fresh" for 40), the personality drawn from the seed
//! (`HostController::new`: aggression and self-preservation), the wounded back-off (`pick_task`'s Recover: at or below
//! `20 + 20 x self_preserve` health, or with an empty magazine in a fight, back away 192 units from the threat,
//! crouched), crouching to fight under 60 health, and the reload-or-switch choice ([`choose_weapon`], `weapon.rs`
//! `choose`: with the magazine empty, switch to the loaded other gun when it is quicker than a reload and an enemy is
//! in sight, or when nothing is left to load). The contact's confidence fading with its age, and heard contacts at
//! half confidence, are ours (IW4L marks a seen contact 1.0 and has no hearing).

use alloc::vec::Vec;

use wpk::nav::NavGraph;

use crate::nav::Navigator;

use crate::player::Steer;

const MAX_YAW_DEG_PER_MS: f32 = 0.36;
const MAX_PITCH_DEG_PER_MS: f32 = 0.24;
const MAX_YAW_ACCEL_DEG_PER_MS2: f32 = 0.015;
const MAX_PITCH_ACCEL_DEG_PER_MS2: f32 = 0.01;
const FIRE_CONE_DEG: f32 = 8.0;
const ARRIVE: f32 = 24.0;
const PITCH_LIMIT: f32 = 70.0;
/// How far a bot looks for a graph point under itself or a goal.
const SNAP: f32 = 256.0;
/// A* expansions per route.
const ROUTE_BUDGET: usize = 4000;
/// IW4L `bots::memory` / `controller`, in its 50 ms ticks: how long a contact is remembered, and how long a lost one
/// is fresh enough to go and look for.
pub const MEMORY_TTL_MS: u32 = 80 * 50;
pub const LAST_SEEN_FRESH_MS: u32 = 40 * 50;
/// Heard, not seen: half as sure.
const HEARD_CONFIDENCE: f32 = 0.5;
/// IW4L `SeekKind::BackOff`: how far a wounded bot backs away from the threat.
pub const BACK_OFF: f32 = 192.0;
/// IW4L: crouch to fight under this much health (of 100).
const CROUCH_FIGHT_HEALTH: i32 = 60;

/// Where a remembered contact came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Seen,
    LastSeen,
    Heard,
}

/// One enemy as a bot remembers it (IW4L `observation::Contact`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Remembered {
    pub id: usize,
    pub origin: [f32; 3],
    pub source: Source,
    pub seen_ms: u32,
    pub confidence: f32,
}

/// IW4L `bots::memory::Memory`: what a bot knows about its enemies across thinks.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub contacts: Vec<Remembered>,
}

impl Memory {
    /// Fold one think's senses in: seen contacts are current (confidence 1), current ones no longer seen become last
    /// seen, heard ones are remembered at half confidence, and anything older than [`MEMORY_TTL_MS`] is forgotten.
    pub fn ingest(&mut self, now: u32, enemies: &[Contact]) {
        self.contacts
            .retain(|c| now.wrapping_sub(c.seen_ms) <= MEMORY_TTL_MS);
        for row in &mut self.contacts {
            if row.source == Source::Seen && !enemies.iter().any(|e| e.id == row.id && e.visible) {
                row.source = Source::LastSeen;
            }
        }
        for e in enemies.iter().filter(|e| e.visible || e.heard) {
            let fresh = Remembered {
                id: e.id,
                origin: e.feet,
                source: if e.visible {
                    Source::Seen
                } else {
                    Source::Heard
                },
                seen_ms: now,
                confidence: if e.visible { 1.0 } else { HEARD_CONFIDENCE },
            };
            match self.contacts.iter_mut().find(|c| c.id == e.id) {
                // A sound doesn't overwrite a sighting this think.
                Some(row) if !e.visible && row.source == Source::Seen => {}
                Some(row) => *row = fresh,
                None => self.contacts.push(fresh),
            }
        }
    }

    /// How sure the bot is of `c` at `now`: its confidence, fading out over the memory's span once out of sight.
    pub fn confidence(c: &Remembered, now: u32) -> f32 {
        if c.source == Source::Seen {
            return c.confidence;
        }
        let age = now.wrapping_sub(c.seen_ms) as f32 / MEMORY_TTL_MS as f32;
        c.confidence * (1.0 - age).max(0.0)
    }

    pub fn currently_seen(&self) -> impl Iterator<Item = &Remembered> {
        self.contacts.iter().filter(|c| c.source == Source::Seen)
    }

    /// The best lead to go and look at: the surest fresh contact out of sight (IW4L's Investigate).
    pub fn lead(&self, now: u32) -> Option<&Remembered> {
        self.contacts
            .iter()
            .filter(|c| {
                c.source != Source::Seen && now.wrapping_sub(c.seen_ms) <= LAST_SEEN_FRESH_MS
            })
            .max_by(|a, b| Self::confidence(a, now).total_cmp(&Self::confidence(b, now)))
    }

    /// Forget every lead out of sight (a bot stuck on its way to one).
    pub fn drop_leads(&mut self) {
        self.contacts.retain(|c| c.source == Source::Seen);
    }
}

/// A weapon decision (IW4L `weapon::WeaponAct`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponAct {
    Switch,
    Reload,
}

/// IW4L `weapon::choose`: with the magazine empty, reload or switch to the other gun. `reload_ms` is the reload's
/// time (None: nothing left to load), `switch_ms` the switch's (None: the other gun is empty or missing); with an enemy
/// in sight the quicker wins, otherwise the bot keeps its gun and reloads.
pub fn choose_weapon(
    clip: i32,
    reload_ms: Option<i32>,
    switch_ms: Option<i32>,
    threatened: bool,
) -> Option<WeaponAct> {
    if clip > 0 {
        return None;
    }
    match (switch_ms, reload_ms) {
        (Some(s), Some(r)) => Some(if threatened && s < r {
            WeaponAct::Switch
        } else {
            WeaponAct::Reload
        }),
        (Some(_), None) => Some(WeaponAct::Switch),
        (None, Some(_)) => Some(WeaponAct::Reload),
        (None, None) => None,
    }
}

/// What a bot knows of itself beyond its eyes: health (out of 100), and its guns' state for [`choose_weapon`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelfState {
    pub health: i32,
    pub stock: i32,
    pub reload_ms: i32,
    /// The time to switch to the other gun, when it is there and loaded.
    pub switch_ms: Option<i32>,
}

impl Default for SelfState {
    fn default() -> Self {
        SelfState {
            health: 100,
            stock: 90,
            reload_ms: 2000,
            switch_ms: None,
        }
    }
}

/// How good a bot is: reaction time and aim error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skill {
    pub reaction_ms: u32,
    pub aim_error_deg: f32,
}

impl Skill {
    /// A middling opponent (MW2's "regular" bots react in a few tenths of a second).
    pub const REGULAR: Skill = Skill {
        reaction_ms: 350,
        aim_error_deg: 3.0,
    };
}

/// One other player as a bot perceives it this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub id: usize,
    /// Where to aim (centre of the chest).
    pub aim: [f32; 3],
    pub feet: [f32; 3],
    /// In view with a clear line.
    pub visible: bool,
    /// Fired recently within hearing.
    pub heard: bool,
}

pub struct Senses<'a> {
    pub now_ms: u32,
    pub msec: i32,
    pub feet: [f32; 3],
    pub eye: [f32; 3],
    /// Current view (pitch, yaw), degrees.
    pub view: (f32, f32),
    pub enemies: &'a [Contact],
    /// Rounds in the magazine.
    pub clip: i32,
    /// Where the game mode wants this bot (a Domination flag to take or hold); it goes there when it has no
    /// enemy to chase and waits once inside.
    pub objective: Option<([f32; 3], f32)>,
    /// Health and guns.
    pub me: SelfState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Act {
    pub steer: Steer,
    pub fire: bool,
    pub reload: bool,
    /// Swing (an enemy within melee reach ahead).
    pub melee: bool,
    /// Throw the lethal grenade (an enemy seen at grenade range).
    pub frag: bool,
    /// Change to the other gun ([`choose_weapon`]).
    pub switch: bool,
}

/// A bot throws a grenade at an enemy it sees between these distances, at most once per [`FRAG_COOLDOWN_MS`], and
/// then only with this chance per decision (bots don't throw at everything they see).
const FRAG_RANGE: (f32, f32) = (400.0, 1100.0);
const FRAG_COOLDOWN_MS: u32 = 8000;
const FRAG_CHANCE: f32 = 0.08;

/// A bot swings instead of shooting inside this distance (feet to feet): IW4's 64-unit melee reach from the eye,
/// less a little so the swing lands.
const MELEE_DIST: f32 = 56.0;

pub struct Brain {
    skill: Skill,
    yaw: f32,
    pitch: f32,
    yaw_vel: f32,
    pitch_vel: f32,
    err: (f32, f32),
    next_err_ms: u32,
    armed: bool,
    target: Option<usize>,
    react_at_ms: u32,
    /// No grenade before this time.
    next_frag_ms: u32,
    /// What it knows of its enemies (IW4L memory).
    pub memory: Memory,
    /// IW4L personality: 0..1 each, from the seed.
    pub aggression: f32,
    pub self_preserve: f32,
    route: Vec<u16>,
    route_goal: Option<[f32; 3]>,
    next_repath_ms: u32,
    stuck_from: ([f32; 3], u32),
    jump_until_ms: u32,
    strafe: i8,
    next_strafe_ms: u32,
    rng: u32,
}

fn angle_sub(a: f32, b: f32) -> f32 {
    let d = a - b;
    d - libm::floorf((d + 180.0) / 360.0) * 360.0
}

/// IW4 view angles (pitch down positive, yaw) looking from `from` to `to`.
pub fn look_angles(from: [f32; 3], to: [f32; 3]) -> (f32, f32) {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let flat = libm::sqrtf(d[0] * d[0] + d[1] * d[1]);
    (
        -libm::atan2f(d[2], flat).to_degrees(),
        libm::atan2f(d[1], d[0]).to_degrees(),
    )
}

fn slew_vel(err: f32, vel: f32, max_rate: f32, max_accel: f32, dt: f32) -> (f32, f32) {
    let want = (err / dt).clamp(-max_rate, max_rate);
    let vel =
        (vel + (want - vel).clamp(-max_accel * dt, max_accel * dt)).clamp(-max_rate, max_rate);
    (vel * dt, vel)
}

/// Forward / right amounts that move along `wish` (xy) when facing `yaw` (IW4L `wish_to_move`).
fn wish_to_move(yaw: f32, wish: [f32; 2]) -> (i8, i8) {
    let len = libm::sqrtf(wish[0] * wish[0] + wish[1] * wish[1]);
    if len < 1.0 {
        return (0, 0);
    }
    let r = yaw.to_radians();
    let (fwd, right) = (
        [libm::cosf(r), libm::sinf(r)],
        [libm::sinf(r), -libm::cosf(r)],
    );
    let f = (wish[0] * fwd[0] + wish[1] * fwd[1]) / len;
    let s = (wish[0] * right[0] + wish[1] * right[1]) / len;
    (
        libm::roundf(f * 127.0).clamp(-127.0, 127.0) as i8,
        libm::roundf(s * 127.0).clamp(-127.0, 127.0) as i8,
    )
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum()
}

impl Brain {
    /// Change how good this bot is (the debug menu's bot skill).
    pub fn set_skill(&mut self, skill: Skill) {
        self.skill = skill;
    }

    /// Where the bot is walking to (its route's goal), if anywhere.
    pub fn goal(&self) -> Option<[f32; 3]> {
        self.route_goal
    }

    pub fn new(skill: Skill, seed: u32) -> Brain {
        Brain {
            skill,
            yaw: 0.0,
            pitch: 0.0,
            yaw_vel: 0.0,
            pitch_vel: 0.0,
            err: (0.0, 0.0),
            next_err_ms: 0,
            armed: false,
            target: None,
            react_at_ms: 0,
            next_frag_ms: 0,
            memory: Memory::default(),
            aggression: (seed & 0xff) as f32 / 255.0,
            self_preserve: ((seed >> 8) & 0xff) as f32 / 255.0,
            route: Vec::new(),
            route_goal: None,
            next_repath_ms: 0,
            stuck_from: ([0.0; 3], 0),
            jump_until_ms: 0,
            strafe: 0,
            next_strafe_ms: 0,
            rng: seed | 1,
        }
    }

    /// Forget everything (a new life).
    pub fn respawned(&mut self) {
        let (skill, rng) = (self.skill, self.rng);
        *self = Brain::new(skill, rng);
    }

    fn random(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A graph point 300-1500 units away to wander to (IW4L `roam_node`'s band, picked at random).
    fn roam_goal(&mut self, g: &NavGraph, from: [f32; 3]) -> Option<[f32; 3]> {
        if g.nodes.is_empty() {
            return None;
        }
        for _ in 0..24 {
            let i = (self.random() * g.nodes.len() as f32) as usize % g.nodes.len();
            let p = g.nodes[i];
            let d = dist2(p, from);
            if (300.0 * 300.0..=1500.0 * 1500.0).contains(&d) {
                return Some(p);
            }
        }
        None
    }

    /// The point to walk to now along a route to `goal` (computing the route when the goal moved).
    fn next_waypoint(
        &mut self,
        nv: &mut Navigator,
        feet: [f32; 3],
        goal: [f32; 3],
        now: u32,
    ) -> Option<[f32; 3]> {
        let moved = self
            .route_goal
            .is_none_or(|old| dist2(old, goal) > 128.0 * 128.0);
        // New routes are rationed (one per frame across all bots): a bot without its turn keeps its old route.
        if (moved || self.route.is_empty()) && now >= self.next_repath_ms && nv.routes_left > 0 {
            nv.routes_left -= 1;
            self.next_repath_ms = now + 500;
            self.route_goal = Some(goal);
            self.route = match (nv.nearest(feet, SNAP), nv.nearest(goal, SNAP)) {
                (Some(a), Some(b)) if a != b => nv.route(a, b, ROUTE_BUDGET).unwrap_or_default(),
                _ => Vec::new(),
            };
        }
        while let Some(&n) = self.route.first() {
            let p = nv.graph.nodes[usize::from(n)];
            let flat = (p[0] - feet[0]) * (p[0] - feet[0]) + (p[1] - feet[1]) * (p[1] - feet[1]);
            if flat < ARRIVE * ARRIVE && libm::fabsf(p[2] - feet[2]) < 40.0 {
                self.route.remove(0);
            } else {
                return Some(p);
            }
        }
        None
    }

    pub fn think(&mut self, nav: Option<&mut Navigator>, s: &Senses<'_>) -> Act {
        let now = s.now_ms;
        let dt = s.msec.max(1) as f32;
        if !self.armed {
            (self.pitch, self.yaw) = s.view;
            self.stuck_from = (s.feet, now);
            self.armed = true;
        }
        // Target: the nearest enemy in view (keep the current one while it stays in view).
        let visible = |id: usize| s.enemies.iter().find(|c| c.id == id && c.visible);
        let target = self.target.and_then(visible).or_else(|| {
            s.enemies
                .iter()
                .filter(|c| c.visible)
                .min_by(|a, b| dist2(a.feet, s.feet).total_cmp(&dist2(b.feet, s.feet)))
        });
        if target.map(|t| t.id) != self.target {
            self.target = target.map(|t| t.id);
            let jitter = self.random() * 0.5 + 0.75;
            self.react_at_ms = now + (self.skill.reaction_ms as f32 * jitter) as u32;
        }
        self.memory.ingest(now, s.enemies);
        // IW4L `pick_task`: wounded (or out of rounds in a fight) the bot backs off from the threat.
        let wounded = s.me.health > 0 && (s.me.health as f32) <= 20.0 + 20.0 * self.self_preserve;
        let seen = target.is_some();
        let recover = (wounded && !seen) || (seen && s.clip == 0 && s.me.switch_ms.is_none());
        let threat = self
            .memory
            .contacts
            .iter()
            .filter(|c| c.source != Source::Seen)
            .max_by_key(|c| c.seen_ms)
            .or_else(|| self.memory.currently_seen().next())
            .map(|c| c.origin);
        // Aim error: re-drawn every 400 ms.
        if now >= self.next_err_ms {
            self.next_err_ms = now + 400;
            let e = self.skill.aim_error_deg;
            self.err = ((self.random() - 0.5) * e, (self.random() - 0.5) * 2.0 * e);
        }

        let mut act = Act::default();
        // Where to look and where to go.
        let mut look_at = None;
        let mut move_goal = None;
        if let Some(t) = target {
            look_at = Some(t.aim);
            let d2 = dist2(t.feet, s.feet);
            // Close in from far away; strafe once in range.
            if d2 > 900.0 * 900.0 {
                move_goal = Some(t.feet);
            }
            act.steer.ads = d2 > 500.0 * 500.0;
        } else if let Some(c) = self.memory.lead(now) {
            move_goal = Some(c.origin);
        }
        if recover && let Some(t) = threat {
            // Back off 192 units from it, crouched, still facing it (`SeekKind::BackOff`).
            let (dx, dy) = (s.feet[0] - t[0], s.feet[1] - t[1]);
            let len = libm::sqrtf(dx * dx + dy * dy).max(1.0);
            move_goal = Some([
                s.feet[0] + dx / len * BACK_OFF,
                s.feet[1] + dy / len * BACK_OFF,
                s.feet[2],
            ]);
            if look_at.is_none() {
                look_at = Some([t[0], t[1], t[2] + 45.0]);
            }
            act.steer.crouch = true;
        }
        let mut wander = false;
        let mut holding = false;
        if move_goal.is_none()
            && target.is_none()
            && let Some((o, r)) = s.objective
        {
            if dist2(o, s.feet) > (r * 0.5) * (r * 0.5) {
                move_goal = Some(o);
            } else {
                holding = true;
            }
        }
        if move_goal.is_none() && target.is_none() && !holding {
            if self.route_goal.is_none() || self.route.is_empty() {
                let g = nav
                    .as_deref()
                    .and_then(|n| self.roam_goal(&n.graph, s.feet));
                self.route_goal = None;
                if let Some(g) = g {
                    self.route_goal = Some(g);
                }
            }
            move_goal = self.route_goal;
            wander = true;
        }
        let waypoint = match (nav, move_goal) {
            (Some(g), Some(goal)) => {
                let w = self.next_waypoint(g, s.feet, goal, now);
                if w.is_none() && wander {
                    // Arrived or no route: wander elsewhere next frame.
                    self.route_goal = None;
                }
                w
            }
            _ => None,
        };
        if look_at.is_none() {
            look_at = waypoint.map(|w| [w[0], w[1], w[2] + 60.0]);
        }

        // Motor: turn towards the look point (IW4L slew), then fire inside the cone.
        if let Some(at) = look_at {
            let (dp, dy) = look_angles(s.eye, at);
            let (err_p, err_y) = if target.is_some() {
                self.err
            } else {
                (0.0, 0.0)
            };
            let (d, v) = slew_vel(
                angle_sub(dy + err_y, self.yaw),
                self.yaw_vel,
                MAX_YAW_DEG_PER_MS,
                MAX_YAW_ACCEL_DEG_PER_MS2,
                dt,
            );
            self.yaw = angle_sub(self.yaw + d, 0.0);
            self.yaw_vel = v;
            let want = (dp + err_p).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            let (d, v) = slew_vel(
                want - self.pitch,
                self.pitch_vel,
                MAX_PITCH_DEG_PER_MS,
                MAX_PITCH_ACCEL_DEG_PER_MS2,
                dt,
            );
            self.pitch = (self.pitch + d).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            self.pitch_vel = v;
            if let Some(t) = target.filter(|_| now >= self.react_at_ms) {
                let on_target = libm::fabsf(angle_sub(dy, self.yaw)) <= FIRE_CONE_DEG
                    && libm::fabsf(dp - self.pitch) <= FIRE_CONE_DEG;
                let d2 = dist2(t.feet, s.feet);
                if d2 < MELEE_DIST * MELEE_DIST {
                    act.melee = on_target;
                } else {
                    act.fire = on_target;
                }
                if on_target
                    && t.visible
                    && now >= self.next_frag_ms
                    && d2 > FRAG_RANGE.0 * FRAG_RANGE.0
                    && d2 < FRAG_RANGE.1 * FRAG_RANGE.1
                    && self.random() < FRAG_CHANCE
                {
                    act.frag = true;
                    act.fire = false;
                    self.next_frag_ms = now + FRAG_COOLDOWN_MS;
                }
            }
        } else {
            self.yaw_vel = 0.0;
            self.pitch_vel = 0.0;
        }
        act.steer.yaw = self.yaw;
        act.steer.pitch = self.pitch;

        if let Some(w) = waypoint {
            (act.steer.forward, act.steer.right) =
                wish_to_move(self.yaw, [w[0] - s.feet[0], w[1] - s.feet[1]]);
            // Sprint along open routes (not while fighting).
            act.steer.sprint =
                target.is_none() && act.steer.forward > 110 && act.steer.right.abs() < 45;
        }
        if target.is_some() {
            // Strafe while fighting, changing side every 0.6-1.4 s.
            if now >= self.next_strafe_ms {
                self.next_strafe_ms = now + 600 + (self.random() * 800.0) as u32;
                self.strafe = match (self.random() * 3.0) as u32 {
                    0 => -127,
                    1 => 0,
                    _ => 127,
                };
            }
            if waypoint.is_none() {
                act.steer.right = self.strafe;
            }
        }
        // Stuck: little progress for 1.5 s while trying to move → jump, and wander elsewhere.
        let moving = act.steer.forward != 0 || act.steer.right != 0;
        if !moving || dist2(s.feet, self.stuck_from.0) > 32.0 * 32.0 {
            self.stuck_from = (s.feet, now);
        } else if now.wrapping_sub(self.stuck_from.1) > 1500 {
            self.jump_until_ms = now + 200;
            self.stuck_from = (s.feet, now);
            self.route.clear();
            self.route_goal = None;
            self.memory.drop_leads();
        }
        act.steer.jump = now < self.jump_until_ms;
        act.reload = s.clip == 0 || (target.is_none() && s.clip < 5);
        // IW4L `weapon::choose` with the magazine empty.
        let reload_ms = (s.me.stock > 0).then_some(s.me.reload_ms);
        if choose_weapon(s.clip, reload_ms, s.me.switch_ms, seen) == Some(WeaponAct::Switch) {
            act.switch = true;
            act.reload = false;
        }
        // Hurt in a fight: crouch (IW4L, under 60 health, firing).
        if act.fire && s.me.health < CROUCH_FIGHT_HEALTH {
            act.steer.crouch = true;
        }
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::MapClip;
    use crate::collision::test_clips::floor_with_wall;
    use crate::nav;
    use crate::player::Player;

    fn enemy(at: [f32; 3], visible: bool) -> Contact {
        Contact {
            id: 7,
            aim: [at[0], at[1], at[2] + 45.0],
            feet: at,
            visible,
            heard: false,
        }
    }

    #[test]
    fn turns_to_an_enemy_and_fires_after_reacting() {
        let mut b = Brain::new(Skill::REGULAR, 1);
        let e = [enemy([0.0, 400.0, 0.0], true)];
        let mut first_fire = None;
        let mut yaw = 0.0;
        for f in 0..120u32 {
            let s = Senses {
                objective: None,
                me: SelfState::default(),
                now_ms: f * 16,
                msec: 16,
                feet: [0.0; 3],
                eye: [0.0, 0.0, 60.0],
                view: (0.0, yaw),
                enemies: &e,
                clip: 30,
            };
            let a = b.think(None, &s);
            yaw = a.steer.yaw;
            if a.fire && first_fire.is_none() {
                first_fire = Some(f * 16);
            }
        }
        let t = first_fire.expect("fires");
        assert!(t >= 250, "reacts before firing ({t} ms)");
        assert!(
            angle_sub(yaw, 90.0).abs() < 10.0,
            "faces the enemy (yaw {yaw})"
        );
        // Out of view: no fire.
        let mut b = Brain::new(Skill::REGULAR, 1);
        let hidden = [enemy([0.0, 400.0, 0.0], false)];
        for f in 0..120u32 {
            let s = Senses {
                objective: None,
                me: SelfState::default(),
                now_ms: f * 16,
                msec: 16,
                feet: [0.0; 3],
                eye: [0.0, 0.0, 60.0],
                view: (0.0, 0.0),
                enemies: &hidden,
                clip: 30,
            };
            assert!(!b.think(None, &s).fire);
        }
    }

    #[test]
    fn walks_around_a_wall_to_a_heard_enemy() {
        let clip = floor_with_wall();
        let mc = MapClip::new(&clip);
        let mut nv = Navigator::new(nav::bake(&mc, &[[-200.0, 0.0, 0.0]]));
        let mut p = Player::spawn(&mc, [-200.0, 0.0, 0.0], 0.0);
        let mut b = Brain::new(Skill::REGULAR, 3);
        let goal = [250.0, 0.0, 0.0];
        let mut best = f32::MAX;
        for f in 0..900u32 {
            let heard = Contact {
                heard: true,
                ..enemy(goal, false)
            };
            let e = [heard];
            let s = Senses {
                objective: None,
                me: SelfState::default(),
                now_ms: f * 16,
                msec: 16,
                feet: p.ps.origin,
                eye: p.eye(),
                view: p.view(),
                enemies: &e,
                clip: 30,
            };
            let a = b.think(Some(&mut nv), &s);
            p.steer(&mc, &a.steer, &crate::player::WeaponMove::default(), 16);
            best = best.min(dist2(p.ps.origin, goal).sqrt());
        }
        assert!(best < 80.0, "got within {best} of the enemy");
    }

    #[test]
    fn memory_follows_iw4l_seen_lost_fresh_and_forgotten() {
        let mut m = Memory::default();
        m.ingest(0, &[enemy([100.0, 0.0, 0.0], true)]);
        assert_eq!(m.contacts[0].source, Source::Seen);
        assert!(m.lead(0).is_none(), "in sight: not a lead");
        // Out of sight: last seen, a lead while fresh (2 s), its confidence fading, gone after 4 s.
        m.ingest(1000, &[enemy([100.0, 0.0, 0.0], false)]);
        assert_eq!(m.contacts[0].source, Source::LastSeen);
        let c = m.lead(1000).copied().unwrap();
        assert!((Memory::confidence(&c, 1000) - 0.75).abs() < 1e-3);
        assert!(m.lead(2000).is_some() && m.lead(2100).is_none());
        m.ingest(4100, &[]);
        assert!(m.contacts.is_empty());
        // Heard: half as sure, and never over a sighting.
        let heard = Contact {
            heard: true,
            ..enemy([0.0, 50.0, 0.0], false)
        };
        m.ingest(5000, &[heard]);
        assert_eq!(m.contacts[0].source, Source::Heard);
        assert_eq!(m.contacts[0].confidence, 0.5);
    }

    #[test]
    fn reload_or_switch_follows_iw4l_choose() {
        // Loaded: nothing to decide.
        assert_eq!(choose_weapon(5, Some(2000), Some(900), true), None);
        // In a fight the quicker wins; out of one the bot keeps its gun.
        assert_eq!(
            choose_weapon(0, Some(2000), Some(900), true),
            Some(WeaponAct::Switch)
        );
        assert_eq!(
            choose_weapon(0, Some(2000), Some(900), false),
            Some(WeaponAct::Reload)
        );
        assert_eq!(
            choose_weapon(0, Some(800), Some(900), true),
            Some(WeaponAct::Reload)
        );
        // Nothing left to load: switch; nothing at all: nothing.
        assert_eq!(
            choose_weapon(0, None, Some(900), false),
            Some(WeaponAct::Switch)
        );
        assert_eq!(choose_weapon(0, None, None, true), None);
    }

    #[test]
    fn a_wounded_bot_backs_off_crouched_from_where_it_lost_the_enemy() {
        let clip = floor_with_wall();
        let mc = MapClip::new(&clip);
        let p = Player::spawn(&mc, [-200.0, 0.0, 0.0], 0.0);
        // Self-preservation 0 (seed byte 1 = 0): wounded at 20 health or less.
        let mut b = Brain::new(Skill::REGULAR, 0x0000_0001);
        assert_eq!(b.self_preserve, 0.0);
        let seen = [enemy([-200.0, 300.0, 0.0], true)];
        let hidden = [enemy([-200.0, 300.0, 0.0], false)];
        fn senses<'a>(p: &Player, enemies: &'a [Contact], health: i32, now: u32) -> Senses<'a> {
            Senses {
                objective: None,
                me: SelfState {
                    health,
                    ..SelfState::default()
                },
                now_ms: now,
                msec: 16,
                feet: p.ps.origin,
                eye: p.eye(),
                view: p.view(),
                enemies,
                clip: 30,
            }
        }
        b.think(None, &senses(&p, &seen, 100, 0));
        // Healthy and out of sight: no backing off.
        assert!(!b.think(None, &senses(&p, &hidden, 100, 100)).steer.crouch);
        // Wounded, out of sight: crouched (backing away from where it was).
        assert!(b.think(None, &senses(&p, &hidden, 15, 200)).steer.crouch);
        // Wounded but in sight: it fights (IW4L scores the fight over the back-off).
        let a = b.think(None, &senses(&p, &seen, 15, 300));
        assert!(!a.steer.crouch || a.fire);
    }
}

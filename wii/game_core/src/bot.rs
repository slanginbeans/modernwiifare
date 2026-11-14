//! Bot brains: what an enemy sees and remembers turned into the same movement and weapon input a player gives.
//!
//! The motor (view turning limited by rate and acceleration, an 8° fire cone, a move goal turned into forward / right
//! amounts) follows IW4L's bot motor (`crates/bots/src/motor.rs`, Apache-2.0, rev `acc57a5`). Above it, written fresh
//! for the Wii in IW4L's shape (sense → intent → motor): pick the nearest enemy in view, react after a delay, track
//! with an aim error that settles, fire inside the cone, strafe while fighting; otherwise chase where an enemy was
//! last seen or heard, or roam the walk graph; jump and pick a new goal when stuck.

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
/// How long an enemy stays worth chasing after it was last seen or heard.
const MEMORY_MS: u32 = 6000;

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
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Act {
    pub steer: Steer,
    pub fire: bool,
    pub reload: bool,
    /// Swing (an enemy within melee reach ahead).
    pub melee: bool,
}

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
    last_known: Option<([f32; 3], u32)>,
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
            last_known: None,
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
        for c in s.enemies.iter().filter(|c| c.visible || c.heard) {
            if self
                .last_known
                .is_none_or(|(p, _)| c.visible || dist2(c.feet, s.feet) < dist2(p, s.feet))
            {
                self.last_known = Some((c.feet, now));
            }
        }
        if self
            .last_known
            .is_some_and(|(_, t)| now.wrapping_sub(t) > MEMORY_MS)
        {
            self.last_known = None;
        }
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
        } else if let Some((p, _)) = self.last_known {
            move_goal = Some(p);
        }
        let mut wander = false;
        if move_goal.is_none() && target.is_none() {
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
                if dist2(t.feet, s.feet) < MELEE_DIST * MELEE_DIST {
                    act.melee = on_target;
                } else {
                    act.fire = on_target;
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
            self.last_known = None;
        }
        act.steer.jump = now < self.jump_until_ms;
        act.reload = s.clip == 0 || (target.is_none() && s.clip < 5);
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
}

//! Domination, as MW2's scripts run it: each flag (`OBJS`) is a proximity use object (`_gameobjects.gsc`
//! `carryObjectProxThinkDelayed` / `updateUseRate`). The first team inside a flag it may take (any team while it is
//! neutral, the enemy once it is owned) claims it; the claim fills at `min(players, 4)` times real time while no
//! other team is inside (paused while contested) and is lost when the claimants leave. A full claim (`dom_use_ms`)
//! gives the flag to that team and the capture's points to the first claimant (`dom.gsc` `onUse`). Every
//! `dom_tick_ms` each owned flag scores a point for its team (`updateDomScores`).

use alloc::vec::Vec;

use wpk::records::ObjectiveRecord;
use wpk::rules::Rules;

use crate::arena::TEAM_NONE;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flag {
    /// 0 for A, 1 for B ...
    pub label: u8,
    pub origin: [f32; 3],
    pub radius: f32,
    pub height: f32,
    /// The owning team (`TEAM_NONE` while neutral).
    pub owner: u8,
    /// The team taking it (`TEAM_NONE`: nobody), the first of them to step in, and how far along (ms).
    pub claim: u8,
    pub claimant: Option<usize>,
    pub progress_ms: f32,
}

impl Flag {
    pub fn letter(&self) -> char {
        char::from(b'A' + self.label.min(25))
    }

    /// Whether `feet` is inside the flag's trigger (a cylinder standing on its origin).
    pub fn contains(&self, feet: [f32; 3]) -> bool {
        let (dx, dy) = (feet[0] - self.origin[0], feet[1] - self.origin[1]);
        dx * dx + dy * dy <= self.radius * self.radius
            && feet[2] >= self.origin[2] - 1.0
            && feet[2] <= self.origin[2] + self.height
    }
}

/// A player as the flags see them.
#[derive(Clone, Copy, Debug)]
pub struct Visitor {
    pub id: usize,
    pub team: u8,
    pub feet: [f32; 3],
    /// Alive and on the ground (`useObjectUseThink` refuses players in the air).
    pub able: bool,
}

/// Something the flags did this update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomEvent {
    Captured {
        label: u8,
        team: u8,
        by: Option<usize>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Domination {
    pub flags: Vec<Flag>,
    next_tick_ms: Option<u32>,
}

impl Domination {
    /// The map's Domination flags (None when it has none).
    pub fn from_objectives(objs: &[ObjectiveRecord]) -> Option<Domination> {
        let flags: Vec<Flag> = objs
            .iter()
            .filter(|o| o.kind == wpk::records::objective_kind::DOM_FLAG)
            .map(|o| Flag {
                label: o.label,
                origin: o.origin,
                radius: o.radius,
                height: o.height,
                owner: TEAM_NONE,
                claim: TEAM_NONE,
                claimant: None,
                progress_ms: 0.0,
            })
            .collect();
        (!flags.is_empty()).then_some(Domination {
            flags,
            next_tick_ms: None,
        })
    }

    /// Step the flags by `dt_ms` at `now`. Returns the team points scored this step (by team index) and pushes
    /// captures to `out`.
    pub fn update(
        &mut self,
        rules: &Rules,
        visitors: &[Visitor],
        dt_ms: f32,
        now: u32,
        out: &mut Vec<DomEvent>,
    ) -> [u32; 3] {
        for f in &mut self.flags {
            let mut touching = [0u32; 3];
            let mut first: [Option<usize>; 3] = [None; 3];
            for v in visitors.iter().filter(|v| v.able && f.contains(v.feet)) {
                let t = usize::from(v.team).min(2);
                touching[t] += 1;
                first[t].get_or_insert(v.id);
            }
            if f.claim == TEAM_NONE {
                // A team that may take it claims it (`allowUse( "enemy" )`: not the owner).
                if let Some(t) = (1..=2u8).find(|&t| touching[usize::from(t)] > 0 && t != f.owner) {
                    f.claim = t;
                    f.claimant = first[usize::from(t)];
                    f.progress_ms = 0.0;
                } else {
                    continue;
                }
            }
            let c = usize::from(f.claim);
            if touching[c] == 0 {
                f.claim = TEAM_NONE;
                f.claimant = None;
                f.progress_ms = 0.0;
                continue;
            }
            let others = touching[1] + touching[2] - touching[c];
            let rate = if others == 0 {
                touching[c].min(4) as f32
            } else {
                0.0
            };
            f.progress_ms += dt_ms * rate;
            if f.progress_ms >= rules.dom_use_ms as f32 {
                f.owner = f.claim;
                out.push(DomEvent::Captured {
                    label: f.label,
                    team: f.claim,
                    by: f.claimant,
                });
                f.claim = TEAM_NONE;
                f.claimant = None;
                f.progress_ms = 0.0;
            }
        }
        let mut points = [0u32; 3];
        let due = *self
            .next_tick_ms
            .get_or_insert(now.wrapping_add(rules.dom_tick_ms));
        if (now.wrapping_sub(due) as i32) >= 0 {
            self.next_tick_ms = Some(due.wrapping_add(rules.dom_tick_ms.max(1)));
            for f in &self.flags {
                if f.owner != TEAM_NONE {
                    points[usize::from(f.owner).min(2)] += 1;
                }
            }
        }
        points
    }

    /// The flag a bot of `team` at `feet` should make for: the nearest it doesn't own, else (all owned) the
    /// nearest. With its trigger radius.
    pub fn goal_for(&self, team: u8, feet: [f32; 3]) -> Option<([f32; 3], f32)> {
        let d = |f: &Flag| {
            let (x, y, z) = (
                f.origin[0] - feet[0],
                f.origin[1] - feet[1],
                f.origin[2] - feet[2],
            );
            x * x + y * y + z * z
        };
        let nearest = |owned: bool| {
            self.flags
                .iter()
                .filter(|f| (f.owner == team) == owned)
                .min_by(|a, b| d(a).total_cmp(&d(b)))
        };
        nearest(false)
            .or_else(|| nearest(true))
            .map(|f| (f.origin, f.radius))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{TEAM_ALLIES, TEAM_AXIS};

    fn dom() -> Domination {
        Domination::from_objectives(&[ObjectiveRecord {
            kind: wpk::records::objective_kind::DOM_FLAG,
            label: 0,
            origin: [0.0; 3],
            radius: 160.0,
            height: 128.0,
        }])
        .unwrap()
    }

    fn at(id: usize, team: u8, x: f32) -> Visitor {
        Visitor {
            id,
            team,
            feet: [x, 0.0, 0.0],
            able: true,
        }
    }

    #[test]
    fn taking_holding_and_contesting_a_flag() {
        let r = Rules::default();
        let mut d = dom();
        let mut ev = Vec::new();
        let mut now = 0;
        let mut step = |d: &mut Domination, v: &[Visitor], ms: u32, ev: &mut Vec<DomEvent>| {
            let mut pts = [0; 3];
            for _ in 0..(ms / 50) {
                now += 50;
                let p = d.update(&r, v, 50.0, now, ev);
                for k in 0..3 {
                    pts[k] += p[k];
                }
            }
            pts
        };
        // One ally: 10 s to take it.
        step(&mut d, &[at(1, TEAM_ALLIES, 0.0)], 9900, &mut ev);
        assert_eq!(d.flags[0].owner, TEAM_NONE);
        step(&mut d, &[at(1, TEAM_ALLIES, 0.0)], 100, &mut ev);
        assert_eq!(d.flags[0].owner, TEAM_ALLIES);
        assert_eq!(
            ev[0],
            DomEvent::Captured {
                label: 0,
                team: TEAM_ALLIES,
                by: Some(1)
            }
        );
        // Held: a point every 5 s.
        let pts = step(&mut d, &[], 10_000, &mut ev);
        assert_eq!(pts[usize::from(TEAM_ALLIES)], 2);
        // Two axis players take it in half the time; an ally inside pauses them.
        let two = [at(2, TEAM_AXIS, 10.0), at(3, TEAM_AXIS, -10.0)];
        step(&mut d, &two, 2500, &mut ev);
        let mut contested = two.to_vec();
        contested.push(at(1, TEAM_ALLIES, 0.0));
        step(&mut d, &contested, 5000, &mut ev);
        assert_eq!(d.flags[0].owner, TEAM_ALLIES, "paused while contested");
        step(&mut d, &two, 2500, &mut ev);
        assert_eq!(d.flags[0].owner, TEAM_AXIS);
        // Bots go for flags they don't own.
        assert!(d.goal_for(TEAM_ALLIES, [500.0, 0.0, 0.0]).is_some());
    }
}

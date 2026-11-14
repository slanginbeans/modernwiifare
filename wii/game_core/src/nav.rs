//! The bots' walk graph: baked by the converter from the map's collision, searched on the Wii.
//!
//! The approach follows IW4L's bot navigation (`crates/bots/src/nav.rs`, Apache-2.0, rev `acc57a5`): floor points on
//! a 48-unit grid flooded out from the spawn points, a move kept when the standing player hull fits through it at
//! step height (18 units) and lands on walkable floor (normal z ≥ 0.7); higher-to-lower moves of more than a step are
//! one-way drops. Written fresh for the Wii (no jumps, mantles or doors yet). Routes are A* over the graph.

use alloc::collections::{BTreeMap, BinaryHeap, VecDeque};
use alloc::vec::Vec;
use core::cmp::Ordering;

use wpk::nav::{Edge, NavGraph, kind};

use crate::collision::{MASK_PLAYER_SOLID, MapClip, PLAYER_MAXS, PLAYER_MINS};

pub const GRID: f32 = 48.0;
pub const STEP: f32 = 18.0;
/// The furthest a bot drops off a ledge (beyond, fall damage starts to matter).
pub const MAX_DROP: f32 = 160.0;
/// Walkable floor (IW4 `MIN_WALK_NORMAL`).
const MIN_WALK_NORMAL: f32 = 0.7;
/// Graph indices are u16.
pub const MAX_NODES: usize = 16000;

/// The floor under `p` within `depth` units for the standing hull, if walkable.
fn floor(clip: &MapClip<'_>, p: [f32; 3], depth: f32) -> Option<f32> {
    let end = [p[0], p[1], p[2] - depth];
    let t = clip.trace(p, end, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYER_SOLID);
    if t.startsolid != 0 || t.fraction >= 1.0 || t.normal[2] < MIN_WALK_NORMAL {
        return None;
    }
    Some(p[2] + (end[2] - p[2]) * t.fraction)
}

fn cell(x: f32) -> i32 {
    libm::floorf(x / GRID) as i32
}

/// Bake the graph from `seeds` (spawn points; each dropped onto its floor). At most `MAX_NODES` nodes.
pub fn bake(clip: &MapClip<'_>, seeds: &[[f32; 3]]) -> NavGraph {
    let mut nodes: Vec<[f32; 3]> = Vec::new();
    let mut adj: Vec<Vec<Edge>> = Vec::new();
    // (cell x, cell y) → nodes in that column (several floors).
    let mut columns: BTreeMap<(i32, i32), Vec<u16>> = BTreeMap::new();
    let mut queue = VecDeque::new();
    let mut find_or_add = |p: [f32; 3],
                           nodes: &mut Vec<[f32; 3]>,
                           adj: &mut Vec<Vec<Edge>>,
                           queue: &mut VecDeque<u16>| {
        let key = (cell(p[0]), cell(p[1]));
        let col = columns.entry(key).or_default();
        if let Some(&n) = col
            .iter()
            .find(|&&n| libm::fabsf(nodes[usize::from(n)][2] - p[2]) < STEP)
        {
            return Some(n);
        }
        if nodes.len() >= MAX_NODES {
            return None;
        }
        let n = nodes.len() as u16;
        nodes.push(p);
        adj.push(Vec::new());
        col.push(n);
        queue.push_back(n);
        Some(n)
    };
    for s in seeds {
        if let Some(z) = floor(clip, [s[0], s[1], s[2] + STEP], 256.0) {
            let c = [
                (cell(s[0]) as f32 + 0.5) * GRID,
                (cell(s[1]) as f32 + 0.5) * GRID,
            ];
            // The cell centre when the player fits there, else the seed itself.
            let p = floor(clip, [c[0], c[1], z + STEP], STEP * 2.0)
                .map_or([s[0], s[1], z], |cz| [c[0], c[1], cz]);
            find_or_add(p, &mut nodes, &mut adj, &mut queue);
        }
    }
    const DIRS: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    while let Some(n) = queue.pop_front() {
        let p = nodes[usize::from(n)];
        let raised = [p[0], p[1], p[2] + STEP];
        for (dx, dy) in DIRS {
            let to = [
                (cell(p[0]) + dx) as f32 * GRID + GRID * 0.5,
                (cell(p[1]) + dy) as f32 * GRID + GRID * 0.5,
                raised[2],
            ];
            let t = clip.trace(raised, to, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYER_SOLID);
            if t.startsolid != 0 || t.fraction < 1.0 {
                continue;
            }
            let Some(z) = floor(clip, to, STEP + MAX_DROP) else {
                continue;
            };
            let q = [to[0], to[1], z];
            let Some(m) = find_or_add(q, &mut nodes, &mut adj, &mut queue) else {
                continue;
            };
            let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
            let cost = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            let k = if p[2] - z > STEP {
                kind::DROP
            } else {
                kind::WALK
            };
            if !adj[usize::from(n)].iter().any(|e| e.to == m) {
                adj[usize::from(n)].push(Edge {
                    to: m,
                    cost: cost.min(65535.0) as u16,
                    kind: k,
                });
            }
        }
    }
    let mut first = Vec::with_capacity(nodes.len() + 1);
    let mut edges = Vec::new();
    for a in &adj {
        first.push(edges.len() as u32);
        edges.extend_from_slice(a);
    }
    first.push(edges.len() as u32);
    NavGraph {
        nodes,
        first,
        edges,
    }
}

/// The node nearest `feet` within `max` units (preferring the same floor, as IW4L's `nearest_within`).
pub fn nearest(g: &NavGraph, feet: [f32; 3], max: f32) -> Option<u16> {
    let mut best: Option<(u16, f32)> = None;
    for (i, p) in g.nodes.iter().enumerate() {
        let d = [p[0] - feet[0], p[1] - feet[1], p[2] - feet[2]];
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if d2 > max * max {
            continue;
        }
        let score = d2 + 3.0 * d[2] * d[2];
        if best.is_none_or(|(_, b)| score < b) {
            best = Some((i as u16, score));
        }
    }
    best.map(|b| b.0)
}

#[derive(PartialEq)]
struct Open(f32, u16);
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

/// A* from `start` to `goal`: the nodes after `start` up to `goal`, or None. Expands at most `budget` nodes.
pub fn route(g: &NavGraph, start: u16, goal: u16, budget: usize) -> Option<Vec<u16>> {
    let n = g.nodes.len();
    if usize::from(start) >= n || usize::from(goal) >= n {
        return None;
    }
    let h = |a: u16| {
        let (p, q) = (g.nodes[usize::from(a)], g.nodes[usize::from(goal)]);
        let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
    };
    let mut cost = alloc::vec![f32::INFINITY; n];
    let mut from = alloc::vec![u16::MAX; n];
    let mut open = BinaryHeap::new();
    cost[usize::from(start)] = 0.0;
    open.push(Open(h(start), start));
    let mut expanded = 0;
    while let Some(Open(_, a)) = open.pop() {
        if a == goal {
            let mut path = Vec::new();
            let mut c = goal;
            while c != start {
                path.push(c);
                c = from[usize::from(c)];
            }
            path.reverse();
            return Some(path);
        }
        expanded += 1;
        if expanded > budget {
            return None;
        }
        for e in g.edges_of(a) {
            let c = cost[usize::from(a)] + f32::from(e.cost);
            if c < cost[usize::from(e.to)] {
                cost[usize::from(e.to)] = c;
                from[usize::from(e.to)] = a;
                open.push(Open(c + h(e.to), e.to));
            }
        }
    }
    None
}

/// The walk graph as the bots use it at run time: a grid index for finding the nearest point without scanning
/// them all, and one A* scratch reused by every search (no allocation or clearing per route: entries are stamped
/// with the search they belong to). `routes_left` caps new routes per frame (the arena sets it each frame).
pub struct Navigator {
    pub graph: NavGraph,
    cells: BTreeMap<(i32, i32), Vec<u16>>,
    cost: Vec<f32>,
    from: Vec<u16>,
    stamp: Vec<u32>,
    search: u32,
    heap: BinaryHeap<Open>,
    pub routes_left: u32,
}

impl Navigator {
    pub fn new(graph: NavGraph) -> Navigator {
        let mut cells: BTreeMap<(i32, i32), Vec<u16>> = BTreeMap::new();
        for (i, p) in graph.nodes.iter().enumerate() {
            cells
                .entry((cell(p[0]), cell(p[1])))
                .or_default()
                .push(i as u16);
        }
        let n = graph.nodes.len();
        Navigator {
            graph,
            cells,
            cost: alloc::vec![0.0; n],
            from: alloc::vec![u16::MAX; n],
            stamp: alloc::vec![0; n],
            search: 0,
            heap: BinaryHeap::new(),
            routes_left: u32::MAX,
        }
    }

    /// The point nearest `feet` within `max` units, preferring the same floor (as [`nearest`]), from the grid cells
    /// around it.
    pub fn nearest(&self, feet: [f32; 3], max: f32) -> Option<u16> {
        let r = libm::ceilf(max / GRID) as i32;
        let (cx, cy) = (cell(feet[0]), cell(feet[1]));
        let mut best: Option<(u16, f32)> = None;
        for x in cx - r..=cx + r {
            for y in cy - r..=cy + r {
                for &i in self.cells.get(&(x, y)).into_iter().flatten() {
                    let p = self.graph.nodes[usize::from(i)];
                    let d = [p[0] - feet[0], p[1] - feet[1], p[2] - feet[2]];
                    let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    if d2 > max * max {
                        continue;
                    }
                    let score = d2 + 3.0 * d[2] * d[2];
                    // Ties go to the lower index, as in the plain scan.
                    if best.is_none_or(|(bi, b)| score < b || (score == b && i < bi)) {
                        best = Some((i, score));
                    }
                }
            }
        }
        best.map(|b| b.0)
    }

    /// A* from `start` to `goal` (as [`route`]), with the shared scratch.
    pub fn route(&mut self, start: u16, goal: u16, budget: usize) -> Option<Vec<u16>> {
        let n = self.graph.nodes.len();
        if usize::from(start) >= n || usize::from(goal) >= n {
            return None;
        }
        self.search = self.search.wrapping_add(1).max(1);
        let search = self.search;
        let g = &self.graph;
        let q = g.nodes[usize::from(goal)];
        // Squared-free heuristic: straight-line distance (one square root per push).
        let h = |a: u16| {
            let p = g.nodes[usize::from(a)];
            let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
            libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
        };
        self.heap.clear();
        self.stamp[usize::from(start)] = search;
        self.cost[usize::from(start)] = 0.0;
        self.from[usize::from(start)] = u16::MAX;
        self.heap.push(Open(h(start), start));
        let mut expanded = 0;
        while let Some(Open(_, a)) = self.heap.pop() {
            if a == goal {
                let mut path = Vec::new();
                let mut c = goal;
                while c != start {
                    path.push(c);
                    c = self.from[usize::from(c)];
                }
                path.reverse();
                return Some(path);
            }
            expanded += 1;
            if expanded > budget {
                return None;
            }
            let ca = self.cost[usize::from(a)];
            for e in g.edges_of(a) {
                let t = usize::from(e.to);
                let c = ca + f32::from(e.cost);
                if self.stamp[t] != search || c < self.cost[t] {
                    self.stamp[t] = search;
                    self.cost[t] = c;
                    self.from[t] = a;
                    self.heap.push(Open(c + h(e.to), e.to));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips::{floor_with_wall, ledge};

    #[test]
    fn floods_a_floor_and_routes_around_a_wall() {
        let clip = floor_with_wall();
        let mc = MapClip::new(&clip);
        let g = bake(&mc, &[[-200.0, 0.0, 0.0]]);
        assert!(g.nodes.len() > 50, "{} nodes", g.nodes.len());
        let a = nearest(&g, [-200.0, 0.0, 0.0], 64.0).expect("start");
        let b = nearest(&g, [200.0, 0.0, 0.0], 64.0).expect("goal");
        let path = route(&g, a, b, 20000).expect("route");
        // The wall spans y -100..100 at x 0: the route must go around it.
        assert!(
            path.iter()
                .any(|&n| g.nodes[usize::from(n)][1].abs() > 100.0)
        );
    }

    #[test]
    fn the_navigator_matches_the_plain_search() {
        let clip = floor_with_wall();
        let mc = MapClip::new(&clip);
        let g = bake(&mc, &[[-200.0, 0.0, 0.0]]);
        let mut nav = Navigator::new(g.clone());
        for feet in [[-200.0, 0.0, 0.0], [210.0, -150.0, 0.0], [5.0, 300.0, 0.0]] {
            assert_eq!(nav.nearest(feet, 256.0), nearest(&g, feet, 256.0));
        }
        let a = nav.nearest([-200.0, 0.0, 0.0], 64.0).unwrap();
        let b = nav.nearest([200.0, 0.0, 0.0], 64.0).unwrap();
        // Twice: the stamped scratch must not leak from one search into the next.
        for _ in 0..2 {
            assert_eq!(nav.route(a, b, 20000), route(&g, a, b, 20000));
        }
    }

    #[test]
    fn ledges_are_one_way() {
        let clip = ledge();
        let mc = MapClip::new(&clip);
        let g = bake(&mc, &[[-100.0, 0.0, 64.0], [100.0, 0.0, 0.0]]);
        let top = nearest(&g, [-100.0, 0.0, 64.0], 64.0).expect("top");
        let low = nearest(&g, [100.0, 0.0, 0.0], 64.0).expect("low");
        assert!(route(&g, top, low, 20000).is_some());
        assert!(route(&g, low, top, 20000).is_none());
    }
}

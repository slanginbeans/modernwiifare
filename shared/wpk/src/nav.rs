//! Bot walk graph (`NAVG`, decoded at load): floor points a standing player can stand on and the moves between them.
//!
//! Layout: u32 node count, u32 edge count, nodes (f32 × 3 each: feet position), per node its first edge (u32, plus
//! one at the end), edges (u16 target node, u16 cost in units, u8 kind, u8 pad).

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Edge kinds.
pub mod kind {
    /// Walk (up a step or across).
    pub const WALK: u8 = 0;
    /// Drop off a ledge (one way).
    pub const DROP: u8 = 1;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edge {
    pub to: u16,
    pub cost: u16,
    pub kind: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavGraph {
    pub nodes: Vec<[f32; 3]>,
    /// `edges[first[n]..first[n + 1]]` leave node `n`.
    pub first: Vec<u32>,
    pub edges: Vec<Edge>,
}

impl NavGraph {
    pub fn edges_of(&self, n: u16) -> &[Edge] {
        let n = usize::from(n);
        match (self.first.get(n), self.first.get(n + 1)) {
            (Some(&a), Some(&b)) => self.edges.get(a as usize..b as usize).unwrap_or(&[]),
            _ => &[],
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(8 + self.nodes.len() * 16 + self.edges.len() * 6);
        o.u32(self.nodes.len() as u32);
        o.u32(self.edges.len() as u32);
        for p in &self.nodes {
            for v in p {
                o.f32(*v);
            }
        }
        for f in &self.first {
            o.u32(*f);
        }
        for e in &self.edges {
            o.u16(e.to);
            o.u16(e.cost);
            o.push(e.kind);
            o.push(0);
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<NavGraph> {
        let n = be::u32(b.get(..4)?, 0) as usize;
        let e = be::u32(b.get(..8)?, 4) as usize;
        let need = 8usize
            .checked_add(n.checked_mul(12)?)?
            .checked_add((n + 1).checked_mul(4)?)?
            .checked_add(e.checked_mul(6)?)?;
        if b.len() < need || n > usize::from(u16::MAX) {
            return None;
        }
        let mut at = 8;
        let nodes = (0..n)
            .map(|_| {
                let p = [be::f32(b, at), be::f32(b, at + 4), be::f32(b, at + 8)];
                at += 12;
                p
            })
            .collect();
        let first: Vec<u32> = (0..=n)
            .map(|_| {
                let v = be::u32(b, at);
                at += 4;
                v
            })
            .collect();
        let edges: Vec<Edge> = (0..e)
            .map(|_| {
                let x = Edge {
                    to: be::u16(b, at),
                    cost: be::u16(b, at + 2),
                    kind: b[at + 4],
                };
                at += 6;
                x
            })
            .collect();
        let ok = first.windows(2).all(|w| w[0] <= w[1])
            && first.last().is_some_and(|&l| l as usize == e)
            && edges.iter().all(|x| usize::from(x.to) < n);
        ok.then_some(NavGraph {
            nodes,
            first,
            edges,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trips_and_checks() {
        let g = NavGraph {
            nodes: vec![[0.0, 0.0, 0.0], [48.0, 0.0, 8.0]],
            first: vec![0, 1, 2],
            edges: vec![
                Edge {
                    to: 1,
                    cost: 49,
                    kind: kind::WALK,
                },
                Edge {
                    to: 0,
                    cost: 49,
                    kind: kind::DROP,
                },
            ],
        };
        let b = g.encode();
        assert_eq!(NavGraph::decode(&b), Some(g.clone()));
        assert_eq!(g.edges_of(1), &g.edges[1..]);
        let mut bad = b.clone();
        let last = bad.len() - 6;
        bad[last + 1] = 9; // edge to node 9
        assert_eq!(NavGraph::decode(&bad), None);
        assert_eq!(NavGraph::decode(&b[..b.len() - 1]), None);
    }
}

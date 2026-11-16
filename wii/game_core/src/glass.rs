//! Breakable glass: IW4L's `entity_iw4::glass` server rules (Apache-2.0, rev `acc57a5`), ported because its crate
//! depends on the std-only `asset_core`: damage thresholds (weakened at 25, shattered at 100), damage that saturates
//! below the "deleted" marker, a weakened pane's random collapse time, the blast falloff, and IW4L `sim`
//! `world_objects`' `apply_glass_hit` / `apply_glass_blast` / `glass_update` around them.
//!
//! The panes are the map's clip brushes that carry a glass piece (`glass_encoded`, piece + 1, already in the pack's
//! `CLIP`): a pane's bounds are its brushes' (each brush's first six planes are its box). A shattered pane drops out
//! of every trace ([`crate::collision::MapClip::set_glass`]), so it stops blocking bullets, sight and movement.
//! Bullets don't stop at intact glass either (IW4L `fire_extended` / `fire_penetrate` step past a pane and damage
//! it): [`crate::collision::MapClip::bullet_trace`].
//!
//! Not ported: the pane's own plane and the shatter seed (`glass_shatter_seed_from_hit`, the FX glass pieces): the
//! Wii doesn't draw MW2's glass pieces or their shatter, only a stand-in pane and a burst of glass impacts.

use alloc::vec::Vec;

pub const GLASS_DAMAGE_TO_WEAKEN: u16 = 25;
pub const GLASS_DAMAGE_TO_DESTROY: u16 = 100;
pub const GLASS_MELEE_DAMAGE: u16 = 1000;
pub const GLASS_DAMAGE_INVALID: u16 = 0xffff;
pub const GLASS_DAMAGE_ADD_CAP: u16 = 0xfffe;
pub const GLASS_BLAST_RADIUS_CAP: f32 = 256.0;
pub const GLASS_BLAST_DAMAGE_SCALE: f32 = 4.0;
pub const GLASS_COLLAPSE_SHORT_THRESHOLD: f32 = 0.3;
pub const GLASS_COLLAPSE_LONG_THRESHOLD: f32 = 0.7;
pub const GLASS_COLLAPSE_SHORT_RANGE_MS: f32 = -57_000.0;
pub const GLASS_COLLAPSE_LONG_RANGE_MS: f32 = -540_000.0;
pub const GLASS_COLLAPSE_SHORT_BASE_MS: i32 = 3_000;
pub const GLASS_COLLAPSE_LONG_BASE_MS: i32 = 60_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum GlassPieceState {
    #[default]
    Intact,
    Weakened,
    Shattered,
    Deleted,
}

/// IW4 `ServerGlassPiece` without the shatter seed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerGlassPiece {
    pub damage: u16,
    /// Centiseconds after the last change when a weakened pane falls apart (0: never).
    pub collapse_time: u16,
    pub last_state_change_time: i32,
}

impl ServerGlassPiece {
    pub fn state(self) -> GlassPieceState {
        glass_state_from_damage(self.damage)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlassStateChange {
    pub previous: GlassPieceState,
    pub current: GlassPieceState,
}

pub fn glass_state_from_damage(damage: u16) -> GlassPieceState {
    if damage == GLASS_DAMAGE_INVALID {
        GlassPieceState::Deleted
    } else if damage >= GLASS_DAMAGE_TO_DESTROY {
        GlassPieceState::Shattered
    } else if damage >= GLASS_DAMAGE_TO_WEAKEN {
        GlassPieceState::Weakened
    } else {
        GlassPieceState::Intact
    }
}

pub fn glass_add_damage(current: u16, added: u32) -> u16 {
    let sum = u32::from(current).saturating_add(added);
    if sum < u32::from(GLASS_DAMAGE_INVALID) {
        sum as u16
    } else {
        GLASS_DAMAGE_ADD_CAP
    }
}

pub fn glass_is_solid(damage: u16) -> bool {
    damage != GLASS_DAMAGE_INVALID && damage < GLASS_DAMAGE_TO_DESTROY
}

/// IW4L `glass_apply_damage` (no shatter seed).
pub fn glass_apply_damage(
    piece: &mut ServerGlassPiece,
    added: u32,
    at_time_ms: i32,
    weakened_collapse_time_cs: Option<u16>,
) -> Option<GlassStateChange> {
    let previous = piece.state();
    if previous == GlassPieceState::Deleted || added == 0 {
        return None;
    }
    piece.damage = glass_add_damage(piece.damage, added);
    let current = piece.state();
    if current == previous {
        return None;
    }
    piece.last_state_change_time = at_time_ms;
    if current == GlassPieceState::Weakened
        && let Some(c) = weakened_collapse_time_cs
    {
        piece.collapse_time = c;
    }
    Some(GlassStateChange { previous, current })
}

pub fn glass_blast_integer_damage(inner: i32, outer: i32, radius: f32, distance: f32) -> u32 {
    let r = radius.min(GLASS_BLAST_RADIUS_CAP);
    if r <= 0.0 || r.is_nan() || distance > r {
        return 0;
    }
    let f = (1.0 - distance / r).clamp(0.0, 1.0);
    let raw = (outer as f32 + f * (inner - outer) as f32).max(0.0) * GLASS_BLAST_DAMAGE_SCALE;
    libm::floorf(raw + 0.5) as u32
}

fn ftol_round(x: f32) -> i32 {
    if x >= 0.0 {
        (x + 0.5) as i32
    } else {
        -((-x + 0.5) as i32)
    }
}

pub fn glass_weakened_collapse_time_cs(next_random: &mut impl FnMut() -> f32) -> u16 {
    let first = next_random();
    let ms = if first < GLASS_COLLAPSE_SHORT_THRESHOLD {
        GLASS_COLLAPSE_SHORT_BASE_MS - ftol_round(next_random() * GLASS_COLLAPSE_SHORT_RANGE_MS)
    } else if first < GLASS_COLLAPSE_LONG_THRESHOLD {
        GLASS_COLLAPSE_LONG_BASE_MS - ftol_round(next_random() * GLASS_COLLAPSE_LONG_RANGE_MS)
    } else {
        return 0;
    };
    (ms / 100) as u16
}

pub fn glass_collapse_due(piece: ServerGlassPiece, level_time_ms: i32) -> bool {
    if piece.collapse_time == 0
        || piece.damage == GLASS_DAMAGE_INVALID
        || piece.damage >= GLASS_DAMAGE_TO_DESTROY
    {
        return false;
    }
    let deadline = i32::from(piece.collapse_time)
        .saturating_mul(100)
        .saturating_add(piece.last_state_change_time);
    deadline < level_time_ms
}

pub fn glass_collapse_piece(
    piece: &mut ServerGlassPiece,
    at_time_ms: i32,
) -> Option<GlassStateChange> {
    if !glass_collapse_due(*piece, at_time_ms) {
        return None;
    }
    let previous = piece.state();
    piece.damage = GLASS_DAMAGE_TO_DESTROY;
    let current = piece.state();
    if current == previous {
        return None;
    }
    piece.last_state_change_time = at_time_ms;
    Some(GlassStateChange { previous, current })
}

/// A pane: the box around its brushes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pane {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

impl Pane {
    pub fn center(&self) -> [f32; 3] {
        core::array::from_fn(|k| (self.mins[k] + self.maxs[k]) * 0.5)
    }
}

/// The map's glass: each piece's state and pane, and the panes that shattered since [`Glass::take_shattered`].
#[derive(Clone, Debug, Default)]
pub struct Glass {
    pub pieces: Vec<ServerGlassPiece>,
    pub panes: Vec<Option<Pane>>,
    shattered: Vec<usize>,
    /// IW4's `holdrand` for the collapse times.
    seed: u32,
}

impl Glass {
    /// The glass of a map's collision: one piece per `glass_encoded` value its brushes carry.
    pub fn from_clip(clip: &wpk::clip::Clip) -> Glass {
        let n = clip
            .brushes
            .iter()
            .map(|b| usize::from(b.glass_encoded))
            .max()
            .unwrap_or(0);
        let mut panes: Vec<Option<Pane>> = alloc::vec![None; n];
        for b in clip.brushes.iter().filter(|b| b.glass_encoded != 0) {
            let at = b.first_plane as usize;
            let Some(p) = clip.planes.get(at..at + 6) else {
                continue;
            };
            let (lo, hi) = ([-p[1][3], -p[3][3], -p[5][3]], [p[0][3], p[2][3], p[4][3]]);
            let slot = &mut panes[usize::from(b.glass_encoded) - 1];
            *slot = Some(match *slot {
                None => Pane { mins: lo, maxs: hi },
                Some(q) => Pane {
                    mins: core::array::from_fn(|k| q.mins[k].min(lo[k])),
                    maxs: core::array::from_fn(|k| q.maxs[k].max(hi[k])),
                },
            });
        }
        Glass {
            pieces: alloc::vec![ServerGlassPiece::default(); n],
            panes,
            shattered: Vec::new(),
            seed: 0x2545_f491,
        }
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub fn is_solid(&self, piece: usize) -> bool {
        self.pieces
            .get(piece)
            .is_none_or(|p| glass_is_solid(p.damage))
    }

    pub fn state(&self, piece: usize) -> GlassPieceState {
        self.pieces
            .get(piece)
            .map_or(GlassPieceState::Deleted, |p| p.state())
    }

    fn random_unit(&mut self) -> f32 {
        anim_iw4::random(&mut self.seed) as f32 * (1.0 / 32768.0)
    }

    /// `damage` to `piece` at `now` (IW4L `apply_glass_hit`): weakening draws its collapse time.
    pub fn hit(&mut self, piece: usize, damage: u32, now: u32) -> Option<GlassStateChange> {
        if damage == 0 || piece >= self.pieces.len() {
            return None;
        }
        let previous = self.pieces[piece].state();
        if previous == GlassPieceState::Deleted {
            return None;
        }
        let next = glass_state_from_damage(glass_add_damage(self.pieces[piece].damage, damage));
        let collapse = (next != previous && next == GlassPieceState::Weakened).then(|| {
            let mut r = || self.random_unit();
            glass_weakened_collapse_time_cs(&mut r)
        });
        let change = glass_apply_damage(&mut self.pieces[piece], damage, now as i32, collapse);
        self.note(piece, change);
        change
    }

    /// An explosion at `origin` (IW4L `apply_glass_blast`): every solid pane within `radius` (at most 256) by its
    /// centre's distance, `inner` down to `outer`, four times over.
    pub fn blast(&mut self, origin: [f32; 3], radius: f32, inner: i32, outer: i32, now: u32) {
        if inner <= 0 && outer <= 0 {
            return;
        }
        for piece in 0..self.pieces.len() {
            let Some(pane) = self.panes[piece] else {
                continue;
            };
            if !self.is_solid(piece) {
                continue;
            }
            let c = pane.center();
            let d = libm::sqrtf(
                (0..3)
                    .map(|k| (c[k] - origin[k]) * (c[k] - origin[k]))
                    .sum(),
            );
            let amount = glass_blast_integer_damage(inner, outer, radius, d);
            if amount > 0 {
                self.hit(piece, amount, now);
            }
        }
    }

    /// Weakened panes whose time came fall apart (IW4L `glass_update`).
    pub fn update(&mut self, now: u32) {
        for piece in 0..self.pieces.len() {
            let change = glass_collapse_piece(&mut self.pieces[piece], now as i32);
            self.note(piece, change);
        }
    }

    fn note(&mut self, piece: usize, change: Option<GlassStateChange>) {
        if let Some(c) = change
            && c.previous < GlassPieceState::Shattered
            && c.current >= GlassPieceState::Shattered
        {
            self.shattered.push(piece);
        }
    }

    /// The panes that shattered since the last call (for their break effect).
    pub fn take_shattered(&mut self) -> Vec<usize> {
        core::mem::take(&mut self.shattered)
    }

    /// Which pieces no longer block (a bit per piece), for [`crate::collision::MapClip::set_glass`].
    pub fn open_bits(&self) -> impl Iterator<Item = u32> + '_ {
        self.pieces.chunks(32).map(|c| {
            c.iter()
                .enumerate()
                .filter(|(_, p)| !glass_is_solid(p.damage))
                .fold(0u32, |w, (i, _)| w | (1 << i))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_and_damage_as_iw4l() {
        assert_eq!(glass_state_from_damage(0), GlassPieceState::Intact);
        assert_eq!(glass_state_from_damage(24), GlassPieceState::Intact);
        assert_eq!(glass_state_from_damage(25), GlassPieceState::Weakened);
        assert_eq!(glass_state_from_damage(100), GlassPieceState::Shattered);
        assert_eq!(glass_state_from_damage(0xffff), GlassPieceState::Deleted);
        assert_eq!(glass_add_damage(0xfff0, 100), GLASS_DAMAGE_ADD_CAP);
        assert!(glass_is_solid(99) && !glass_is_solid(100) && !glass_is_solid(0xffff));
        // Blast: 256 at most, four times the interpolated damage, rounded.
        assert_eq!(glass_blast_integer_damage(100, 0, 512.0, 128.0), 200);
        assert_eq!(glass_blast_integer_damage(100, 0, 512.0, 300.0), 0);
        assert_eq!(glass_blast_integer_damage(10, 10, 100.0, 50.0), 40);
    }

    #[test]
    fn collapse_times_follow_the_draws() {
        // A first draw under 0.3: 3 s plus up to 57 s more; under 0.7: 60 s plus up to 540 s; else never.
        let mut draws = [0.1, 0.5].into_iter();
        assert_eq!(
            glass_weakened_collapse_time_cs(&mut || draws.next().unwrap()),
            315
        );
        let mut draws = [0.5, 0.5].into_iter();
        assert_eq!(
            glass_weakened_collapse_time_cs(&mut || draws.next().unwrap()),
            3300
        );
        assert_eq!(glass_weakened_collapse_time_cs(&mut || 0.9), 0);
        let mut p = ServerGlassPiece {
            damage: 30,
            collapse_time: 315,
            last_state_change_time: 1000,
        };
        assert!(glass_collapse_piece(&mut p, 32_500).is_none());
        assert_eq!(
            glass_collapse_piece(&mut p, 32_501).map(|c| c.current),
            Some(GlassPieceState::Shattered)
        );
    }

    #[test]
    fn panes_come_from_the_clip_and_break() {
        let mut clip = wpk::clip::Clip::default();
        let bx = |lo: [f32; 3], hi: [f32; 3]| {
            [
                [1.0, 0.0, 0.0, hi[0]],
                [-1.0, 0.0, 0.0, -lo[0]],
                [0.0, 1.0, 0.0, hi[1]],
                [0.0, -1.0, 0.0, -lo[1]],
                [0.0, 0.0, 1.0, hi[2]],
                [0.0, 0.0, -1.0, -lo[2]],
            ]
        };
        clip.push_brush(&bx([0.0; 3], [100.0; 3]), &[0; 6], 1, 0);
        clip.push_brush(&bx([0.0, 0.0, 0.0], [1.0, 32.0, 64.0]), &[0; 6], 0x10, 2);
        clip.push_brush(&bx([0.0, 32.0, 0.0], [1.0, 64.0, 64.0]), &[0; 6], 0x10, 2);
        let mut g = Glass::from_clip(&clip);
        // Pieces 0 and 1 (piece 0 has no brush here); piece 1 spans both brushes.
        assert_eq!(g.len(), 2);
        assert!(g.panes[0].is_none());
        assert_eq!(
            g.panes[1],
            Some(Pane {
                mins: [0.0; 3],
                maxs: [1.0, 64.0, 64.0]
            })
        );
        // A 40-damage bullet weakens it; three more shatter it, once.
        assert_eq!(
            g.hit(1, 40, 100).map(|c| c.current),
            Some(GlassPieceState::Weakened)
        );
        assert!(g.is_solid(1));
        g.hit(1, 40, 200);
        assert!(g.take_shattered().is_empty());
        g.hit(1, 40, 300);
        assert!(!g.is_solid(1));
        assert_eq!(g.take_shattered(), [1]);
        g.hit(1, 40, 400);
        assert!(g.take_shattered().is_empty());
        assert_eq!(g.open_bits().collect::<Vec<_>>(), [0b10]);
        // A grenade beside a fresh pane: 256 cap, so one at 100 units with 130..10 damage shatters it.
        let mut g = Glass::from_clip(&clip);
        g.blast([100.0, 32.0, 32.0], 256.0, 130, 10, 0);
        assert!(!g.is_solid(1));
    }
}

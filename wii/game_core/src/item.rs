//! Weapons dropped on death and Scavenger bags: IW4L `sim/src/item.rs` (Apache-2.0, rev `acc57a5`): the drop's
//! throw (`drop_item_velocity`), the fall and its landing (`think_item_move`, `blocked_fall`; IW4L
//! `entity_iw4::trajectory`'s `TR_GRAVITY`), the cap of 16 with the oldest evicted (`push_dropped_item`), touching
//! for ammunition (`try_touch_one`), the item a player looks at for a swap (`selected_item`) and the ammo split
//! (`split_item_ammo`).
//!
//! MW2's script rules (the user's `_weapons.gsc`, read for behaviour, ADR 0007): on death, the weapon in hand
//! drops when it has rounds in the clip (`dropWeaponForDeath`, the stock capped at the weapon's maximum) and goes
//! after 60 s (`deletePickupAfterAWhile`); a player killed by someone else drops a Scavenger bag
//! (`dropScavengerForDeath`); the bag gives a clip's worth of reserve to each weapon and a grenade of each kind
//! (`handleScavengerBagPickup`) to whoever has Scavenger.

use alloc::vec::Vec;

use crate::collision::{MASK_PLAYER_SOLID, MapClip, PLAYER_MAXS, PLAYER_MINS};

pub const G_MAX_DROPPED_WEAPONS: usize = 16;
pub const G_DROP_FORWARD_SPEED: f32 = 10.0;
pub const G_DROP_UP_SPEED_BASE: f32 = 10.0;
pub const G_DROP_UP_SPEED_RAND: f32 = 5.0;
pub const G_DROP_HORZ_SPEED_RAND: f32 = 100.0;
pub const ITEM_MINS: [f32; 3] = [0.0, 0.0, 0.0];
pub const ITEM_MAXS: [f32; 3] = [1.0, 1.0, 1.0];
/// How long the use button is held over a weapon to take it (IW4L `ITEM_USE_HOLD_MS`).
pub const ITEM_USE_HOLD_MS: u32 = 250;
/// Drops leave from the middle of the hull.
pub const PLAYER_DROP_Z: f32 = (PLAYER_MAXS[2] - PLAYER_MINS[2]) * 0.5;
/// `deletePickupAfterAWhile`: a dropped weapon goes after a minute.
pub const WEAPON_LIFETIME_MS: u32 = 60_000;
/// `selected_item`: weapons within this of the eye can be taken.
pub const USE_RANGE: f32 = 128.0;
/// IW4 `TR_GRAVITY` (`entity_iw4::trajectory`): half of 800 for the position.
const TRAJECTORY_GRAVITY: f32 = 400.0;
const TRAJECTORY_GRAVITY_DELTA: f32 = 800.0;

/// IW4's `random` (`anim_iw4`) as 0..1.
pub fn random_unit(seed: &mut u32) -> f32 {
    anim_iw4::random(seed) as f32 * (1.0 / 32768.0)
}

pub fn random_signed(seed: &mut u32) -> f32 {
    let u = random_unit(seed);
    u + u - 1.0
}

/// IW4L `drop_item_velocity`: a little forward along `yaw`, up 10 ± 5, and up to 100 sideways each way.
pub fn drop_item_velocity(yaw_deg: f32, seed: &mut u32) -> [f32; 3] {
    let (s, c) = (
        libm::sinf(yaw_deg.to_radians()),
        libm::cosf(yaw_deg.to_radians()),
    );
    let mut v = [c * G_DROP_FORWARD_SPEED, s * G_DROP_FORWARD_SPEED, 0.0];
    v[2] += random_signed(seed) * G_DROP_UP_SPEED_RAND + G_DROP_UP_SPEED_BASE;
    v[0] += random_signed(seed) * G_DROP_HORZ_SPEED_RAND;
    v[1] += random_signed(seed) * G_DROP_HORZ_SPEED_RAND;
    v
}

/// IW4L `split_item_ammo`: up to `room` rounds out of the item, reserve first, then the clips; returns what was
/// taken and what is left (right clip, left clip, reserve).
pub fn split_item_ammo(room: i32, clip_r: i32, clip_l: i32, stock: i32) -> (i32, [i32; 3]) {
    let mut room = room.max(0);
    let mut take = |have: i32| {
        let from = have.max(0).min(room);
        room -= from;
        (from, have - from)
    };
    let (from_stock, stock) = take(stock);
    let (from_r, clip_r) = take(clip_r);
    let (from_l, clip_l) = take(clip_l);
    (from_stock + from_r + from_l, [clip_r, clip_l, stock])
}

/// One item on the ground (or falling).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dropped {
    /// The weapon (index in `WEAP`); for a Scavenger bag, the weapon of whoever dropped it.
    pub weapon: u32,
    pub origin: [f32; 3],
    pub yaw: f32,
    /// The fall (IW4 `TR_GRAVITY` from `base` with `delta` at `time`), until it lands.
    pub falling: bool,
    base: [f32; 3],
    delta: [f32; 3],
    time: u32,
    pub clip: i32,
    pub stock: i32,
    pub scavenger: bool,
    /// Who dropped it, and when.
    pub owner: usize,
    pub dropped_ms: u32,
    seq: u32,
}

impl Dropped {
    fn at(&self, now: u32) -> [f32; 3] {
        let dt = now.wrapping_sub(self.time) as f32 * 0.001;
        [
            self.base[0] + self.delta[0] * dt,
            self.base[1] + self.delta[1] * dt,
            self.base[2] + self.delta[2] * dt - TRAJECTORY_GRAVITY * dt * dt,
        ]
    }

    fn velocity(&self, now: u32) -> [f32; 3] {
        let dt = now.wrapping_sub(self.time) as f32 * 0.001;
        [
            self.delta[0],
            self.delta[1],
            self.delta[2] - dt * TRAJECTORY_GRAVITY_DELTA,
        ]
    }

    /// Whether a player standing at `origin` touches it (IW4L `aabb_overlap` of the hull and the item's box).
    pub fn touches(&self, origin: [f32; 3]) -> bool {
        (0..3).all(|k| {
            origin[k] + PLAYER_MINS[k] <= self.origin[k] + ITEM_MAXS[k]
                && origin[k] + PLAYER_MAXS[k] >= self.origin[k] + ITEM_MINS[k]
        })
    }

    /// The rounds it holds.
    pub fn rounds(&self) -> i32 {
        self.clip + self.stock
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn slide_along(v: &mut [f32; 3], n: [f32; 3]) {
    let into = dot(*v, n);
    if into < 0.0 {
        for k in 0..3 {
            v[k] -= n[k] * into;
        }
    }
}

/// The items in the match.
#[derive(Clone, Debug, Default)]
pub struct Items {
    pub list: Vec<Dropped>,
    seed: u32,
    next_seq: u32,
}

impl Items {
    pub fn new() -> Items {
        Items {
            list: Vec::with_capacity(G_MAX_DROPPED_WEAPONS),
            seed: 0x1d87_2b41,
            next_seq: 0,
        }
    }

    fn push(&mut self, mut d: Dropped) {
        if self.list.len() >= G_MAX_DROPPED_WEAPONS
            && let Some(oldest) = (0..self.list.len()).min_by_key(|&i| self.list[i].seq)
        {
            self.list.swap_remove(oldest);
        }
        d.seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.list.push(d);
    }

    /// Throw an item from a player at `feet` facing `yaw` (IW4L `launch_dropped_from_ps`).
    #[allow(clippy::too_many_arguments)]
    pub fn launch(
        &mut self,
        feet: [f32; 3],
        yaw: f32,
        weapon: u32,
        clip: i32,
        stock: i32,
        scavenger: bool,
        owner: usize,
        now: u32,
    ) {
        let delta = drop_item_velocity(yaw, &mut self.seed);
        let origin = [feet[0], feet[1], feet[2] + PLAYER_DROP_Z];
        self.push(Dropped {
            weapon,
            origin,
            yaw,
            falling: true,
            base: origin,
            delta,
            time: now,
            clip,
            stock,
            scavenger,
            owner,
            dropped_ms: now,
            seq: 0,
        });
    }

    /// Lay a weapon down where another was taken (IW4L `drop_current_primary_at`: no fall).
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        origin: [f32; 3],
        yaw: f32,
        weapon: u32,
        clip: i32,
        stock: i32,
        owner: usize,
        now: u32,
    ) {
        self.push(Dropped {
            weapon,
            origin,
            yaw,
            falling: false,
            base: origin,
            delta: [0.0; 3],
            time: now,
            clip,
            stock,
            scavenger: false,
            owner,
            dropped_ms: now,
            seq: 0,
        });
    }

    /// Move falling items to `now` (IW4L `think_item_move`: a landing on anything facing up rests; a wall turns
    /// the fall along it) and remove weapons older than [`WEAPON_LIFETIME_MS`].
    pub fn think(&mut self, clip: &MapClip<'_>, now: u32) {
        self.list
            .retain(|d| d.scavenger || now.wrapping_sub(d.dropped_ms) < WEAPON_LIFETIME_MS);
        for d in self.list.iter_mut().filter(|d| d.falling) {
            let start = d.origin;
            let desired = d.at(now);
            let hit = clip.trace(start, desired, ITEM_MINS, ITEM_MAXS, MASK_PLAYER_SOLID);
            if hit.startsolid == 0 && hit.fraction >= 1.0 {
                d.origin = desired;
                continue;
            }
            let rest = |d: &mut Dropped, at: [f32; 3]| {
                d.origin = at;
                d.falling = false;
                d.base = at;
                d.delta = [0.0; 3];
            };
            if hit.startsolid != 0 || hit.allsolid != 0 {
                rest(d, start);
                continue;
            }
            let n = hit.normal;
            if n[2] > 0.0 || dot(n, n) < 0.25 {
                rest(d, hit.endpos);
                continue;
            }
            // Off a wall: on along it from just off the wall (IW4L `blocked_fall`'s deflection).
            let mut vel = d.velocity(now);
            slide_along(&mut vel, n);
            let base: [f32; 3] = core::array::from_fn(|k| hit.endpos[k] + n[k]);
            let off = clip.trace(hit.endpos, base, ITEM_MINS, ITEM_MAXS, MASK_PLAYER_SOLID);
            if off.startsolid != 0 || off.fraction <= 0.0 {
                rest(d, hit.endpos);
                continue;
            }
            if off.fraction < 1.0 {
                slide_along(&mut vel, off.normal);
                if dot(vel, n) < 0.0 {
                    vel[0] = 0.0;
                    vel[1] = 0.0;
                }
            }
            d.origin = off.endpos;
            d.base = off.endpos;
            d.delta = vel;
            d.time = now;
        }
    }

    /// The weapon a player with eyes at `eye` looking along `forward` would take (IW4L `selected_item`): not a bag,
    /// not one of `held`, within [`USE_RANGE`], in sight (`MASK 0x11`), best by distance and how straight ahead.
    pub fn selected(
        &self,
        clip: &MapClip<'_>,
        eye: [f32; 3],
        forward: [f32; 3],
        held: &[u32],
    ) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, d) in self.list.iter().enumerate() {
            if d.scavenger || held.contains(&d.weapon) {
                continue;
            }
            let center: [f32; 3] =
                core::array::from_fn(|k| d.origin[k] + (ITEM_MINS[k] + ITEM_MAXS[k]) * 0.5);
            let delta: [f32; 3] = core::array::from_fn(|k| center[k] - eye[k]);
            let distance = libm::sqrtf(dot(delta, delta));
            if distance > USE_RANGE {
                continue;
            }
            let facing = if distance > 0.0 {
                dot(forward, delta) / distance
            } else {
                0.0
            };
            if clip.trace(eye, center, [0.0; 3], [0.0; 3], 0x11).fraction < 1.0 {
                continue;
            }
            let score = distance + (1.0 - (facing + 1.0) * 0.5) * 256.0;
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// Take item `i` off the ground.
    pub fn take(&mut self, i: usize) -> Option<Dropped> {
        (i < self.list.len()).then(|| self.list.swap_remove(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_clips::boxes;

    #[test]
    fn drops_fall_and_land() {
        let clip = boxes(&[
            ([-400.0, -400.0, -64.0], [400.0, 400.0, 0.0]),
            ([100.0, -400.0, 0.0], [116.0, 400.0, 200.0]),
        ]);
        let mc = MapClip::new(&clip);
        let mut items = Items::new();
        items.launch([0.0, 0.0, 0.0], 0.0, 7, 30, 90, false, 1, 1000);
        let d = items.list[0];
        assert_eq!(d.origin, [0.0, 0.0, 35.0]);
        // The throw: 10 forward, 10 ± 5 up, ± 100 sideways.
        assert!(d.delta[2] >= 5.0 && d.delta[2] <= 15.0);
        assert!(d.delta[0].abs() <= 110.0 && d.delta[1].abs() <= 100.0);
        for t in (1050..3000).step_by(50) {
            items.think(&mc, t);
        }
        let d = items.list[0];
        assert!(!d.falling);
        assert!(d.origin[2].abs() < 0.5, "{:?}", d.origin);
        // Not past the wall at x 100.
        assert!(d.origin[0] < 100.0);
        // A minute later it's gone.
        items.think(&mc, 1000 + WEAPON_LIFETIME_MS);
        assert!(items.list.is_empty());
    }

    #[test]
    fn sixteen_at_most_the_oldest_goes() {
        let mut items = Items::new();
        for w in 0..20 {
            items.place([w as f32 * 10.0, 0.0, 0.0], 0.0, w, 1, 0, 1, 0);
        }
        assert_eq!(items.list.len(), G_MAX_DROPPED_WEAPONS);
        let mut weapons: Vec<u32> = items.list.iter().map(|d| d.weapon).collect();
        weapons.sort();
        assert_eq!(weapons, (4..20).collect::<Vec<_>>());
    }

    #[test]
    fn the_split_takes_the_reserve_first() {
        assert_eq!(split_item_ammo(40, 30, 0, 90), (40, [30, 0, 50]));
        assert_eq!(split_item_ammo(100, 30, 0, 90), (100, [20, 0, 0]));
        assert_eq!(split_item_ammo(0, 30, 0, 90), (0, [30, 0, 90]));
    }

    #[test]
    fn the_one_ahead_is_selected() {
        let clip = boxes(&[([-400.0, -400.0, -64.0], [400.0, 400.0, 0.0])]);
        let mc = MapClip::new(&clip);
        let mut items = Items::new();
        items.place([60.0, 0.0, 0.0], 0.0, 3, 30, 0, 1, 0);
        items.place([0.0, 60.0, 0.0], 0.0, 4, 30, 0, 1, 0);
        items.place([300.0, 0.0, 0.0], 0.0, 5, 30, 0, 1, 0);
        let eye = [0.0, 0.0, 60.0];
        assert_eq!(items.selected(&mc, eye, [1.0, 0.0, 0.0], &[]), Some(0));
        assert_eq!(items.selected(&mc, eye, [0.0, 1.0, 0.0], &[]), Some(1));
        // Already carried: not offered.
        assert_eq!(items.selected(&mc, eye, [1.0, 0.0, 0.0], &[3]), Some(1));
        assert!(items.list[0].touches([50.0, 0.0, 0.0]));
        assert!(!items.list[0].touches([100.0, 0.0, 0.0]));
    }
}

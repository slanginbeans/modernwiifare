//! The killcam on the Wii: every player's pose recorded each server frame into a ring in MEM2
//! (`game_core::killcam::Ring`), and the replay drawn from the killer's eye: the world, the other players' bodies at
//! their recorded poses (their own animation state: slots past [`SLOT_OFFSET`]), the killer's weapon in front of the
//! eye (its world model: the first-person files of the killer's weapon aren't loaded), the muzzle flashes and the
//! bullet impacts recorded with them.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use game_core::arena::{self, Arena, Event};
use game_core::camera::Camera;
use game_core::gfx::{Gfx, Mtx34};
use game_core::killcam::{Eye, FRAMES, Killcam, Ring, angle_delta};

use crate::character::{Characters, Pose};
use crate::effects::{self, Effects};
use crate::ffi;

/// Replayed bodies animate in slots this far past the live ones (their own animation state; a slot's state is small,
/// the skinned buffers go by draw order, not slot).
pub const SLOT_OFFSET: usize = 32;
/// Recorded impacts kept (a few seconds of shooting).
const EVENTS: usize = 192;
/// The killer's weapon in eye space (IW4 model axes x forward / y left / z up to the eye's x right / y up / -z
/// forward), a little right of and below the eye, 14 units ahead.
const IN_VIEW: Mtx34 = [
    [0.0, -1.0, 0.0, 6.0],
    [0.0, 0.0, 1.0, -6.5],
    [-1.0, 0.0, 0.0, -14.0],
];

/// One player in one frame.
#[derive(Clone, Copy)]
pub struct Sample {
    pub eye: Eye,
    pub pose: Pose,
}

pub struct Replay {
    pub ring: Ring<'static, Sample>,
    /// Bullet impacts (archive time, origin, normal, effect) for the replay.
    impacts: VecDeque<(u32, [f32; 3], [f32; 3], u16)>,
    /// The replay time last drawn, and each slot's last fire time seen (a change is a muzzle flash).
    shown_ms: Option<u32>,
    fired: Vec<Option<u32>>,
    /// MEM2 taken.
    pub bytes: usize,
}

fn team_model(f: &arena::Fighter, i: usize) -> usize {
    match f.team {
        arena::TEAM_AXIS => 1,
        arena::TEAM_ALLIES => 0,
        _ => i % 2,
    }
}

/// The sample of fighter `i` at `now` (`weapon`: the human's gun, which lives outside the arena).
fn sample(a: &Arena, i: usize, now: u32, weapon: Option<u32>) -> Sample {
    let f = &a.fighters[i];
    let mut pose = Pose::of(f, i, team_model(f, i), now);
    if i == 0 {
        pose.weapon = weapon;
    }
    let (pitch, yaw) = f.player.view();
    Sample {
        eye: Eye {
            origin: f.draw_origin(now),
            view_height: f.player.ps.view_height_current,
            pitch,
            yaw,
            alive: f.life.alive(),
            weapon: pose.weapon.unwrap_or(u32::MAX),
        },
        pose,
    }
}

/// `count` items of `T` in MEM2 (until the map's MEM2 is released), each `fill`.
fn mem2_slice<T: Copy>(count: usize, fill: T) -> Option<&'static mut [T]> {
    let align = core::mem::align_of::<T>().max(32) as u32;
    let p = unsafe { ffi::mwf_mem2_alloc((count * core::mem::size_of::<T>()) as u32, align) };
    if p.is_null() {
        return None;
    }
    let p = p.cast::<T>();
    for k in 0..count {
        unsafe { p.add(k).write(fill) };
    }
    Some(unsafe { core::slice::from_raw_parts_mut(p, count) })
}

/// The eye's frame to the world (the inverse of `cam.view()`).
fn eye_to_world(cam: &Camera) -> Mtx34 {
    let (r, u, f, e) = (cam.right(), cam.up(), cam.forward(), cam.eye);
    [
        [r[0], u[0], -f[0], e[0]],
        [r[1], u[1], -f[1], e[1]],
        [r[2], u[2], -f[2], e[2]],
    ]
}

impl Replay {
    /// A ring for `a`'s fighters over [`FRAMES`] frames in MEM2 (None when MEM2 is short).
    pub fn new(a: &Arena, now: u32) -> Option<Replay> {
        let slots = a.fighters.len();
        let first = sample(a, 0, now, None);
        let times = mem2_slice::<u32>(FRAMES, 0)?;
        let items = mem2_slice::<Sample>(FRAMES * slots, first)?;
        Some(Replay {
            ring: Ring::new(times, items, slots),
            impacts: VecDeque::with_capacity(EVENTS),
            shown_ms: None,
            fired: alloc::vec![None; slots],
            bytes: Ring::<Sample>::bytes(FRAMES, slots),
        })
    }

    /// Record a frame when one is due.
    pub fn record(&mut self, a: &Arena, now: u32, human_weapon: Option<u32>) {
        if self.ring.due(now) && self.ring.slots() == a.fighters.len() {
            self.ring.push(now, |s| sample(a, s, now, human_weapon));
        }
    }

    /// Keep what the replay shows of an event (bullet impacts, with their effect).
    pub fn note(&mut self, now: u32, e: &Event, fx: Option<&Effects>) {
        if let (
            Event::Impact {
                origin,
                normal,
                surface,
                flesh,
                impact_type,
            },
            Some(fx),
        ) = (e, fx)
        {
            if self.impacts.len() == EVENTS {
                self.impacts.pop_front();
            }
            let effect = fx.impact(*impact_type, *surface, *flesh);
            self.impacts.push_back((now, *origin, *normal, effect));
        }
    }

    /// The killer's eye at `now` of killcam `k` (None when nothing is recorded).
    pub fn camera(&self, k: &Killcam, now: u32, aspect: f32) -> Option<Camera> {
        let (a, b, f) = self.ring.at(k.replay_ms(now))?;
        let e = a.get(k.killer)?.eye.lerp(&b.get(k.killer)?.eye, f);
        let mut cam = Camera::new(e.eye(), e.yaw, e.pitch);
        cam.aspect = aspect;
        Some(cam)
    }

    /// Forget what was shown (a new killcam starts).
    pub fn restart(&mut self) {
        self.shown_ms = None;
        self.fired.iter_mut().for_each(|f| *f = None);
    }

    /// Draw killcam `k` at `now` through `cam` (after the world): the bodies, the killer's weapon, flashes and impacts.
    pub fn draw(
        &mut self,
        gfx: &mut impl Gfx,
        ch: Option<&mut Characters>,
        mut fx: Option<&mut Effects>,
        k: &Killcam,
        cam: &Camera,
        now: u32,
    ) {
        let t = k.replay_ms(now);
        let anim = k.anim_ms(now);
        let shift = anim.wrapping_sub(t);
        let Some((a, b, f)) = self.ring.at(t) else {
            return;
        };
        let mut poses: Vec<Pose> = Vec::with_capacity(a.len());
        for s in 0..a.len().min(b.len()) {
            let (pa, pb) = (&a[s].pose, &b[s].pose);
            let mut p = if f < 0.5 { *pa } else { *pb };
            p.origin = core::array::from_fn(|i| pa.origin[i] + (pb.origin[i] - pa.origin[i]) * f);
            p.pitch = pa.pitch + angle_delta(pa.pitch, pb.pitch) * f;
            p.yaw = pa.yaw + angle_delta(pa.yaw, pb.yaw) * f;
            p.slot = s + SLOT_OFFSET;
            p.last_fire_ms = p.last_fire_ms.map(|x| x.wrapping_add(shift));
            // A muzzle flash when the recorded fire time moves on.
            let fire = a[s].pose.last_fire_ms;
            let flash = fire.is_some() && self.fired.get(s).copied().flatten() != fire;
            if let Some(slot) = self.fired.get_mut(s) {
                *slot = fire;
            }
            if s != k.killer {
                poses.push(p);
            }
            if flash
                && self.shown_ms.is_some()
                && s != k.killer
                && let (Some(fx), Some(ch)) = (fx.as_deref_mut(), ch.as_deref())
                && let (Some((o, d)), Some(w)) = (ch.muzzle(s + SLOT_OFFSET), p.weapon)
            {
                fx.play(fx.weapon(w, effects::WORLD_FLASH), o, d, now);
            }
        }
        let killer = a.get(k.killer).map(|s| s.eye);
        let shown = self.shown_ms.is_some();
        // The killer's shots (its weapon is drawn after the bodies; tracked in its own slot).
        let killer_fired = {
            let fire = a.get(k.killer).and_then(|x| x.pose.last_fire_ms);
            let key = SLOT_OFFSET + k.killer;
            while self.fired.len() <= key {
                self.fired.push(None);
            }
            let changed = fire.is_some() && self.fired[key] != fire;
            self.fired[key] = fire;
            changed
        };
        if let Some(ch) = ch {
            ch.draw(gfx, &cam.view(), &poses, anim);
            if let Some(eye) = killer.filter(|e| e.alive && e.weapon != u32::MAX)
                && let Some((o, d)) =
                    ch.draw_held_in_view(gfx, eye.weapon, &IN_VIEW, &eye_to_world(cam))
                && killer_fired
                && shown
                && let Some(fx) = fx.as_deref_mut()
            {
                fx.play(fx.weapon(eye.weapon, effects::WORLD_FLASH), o, d, now);
            }
        }
        // The impacts recorded since the last frame drawn.
        if let (Some(fx), Some(from)) = (fx, self.shown_ms) {
            let after = |x: u32, y: u32| (x.wrapping_sub(y) as i32) > 0;
            for &(at, o, n, e) in &self.impacts {
                if after(at, from) && !after(at, t) {
                    fx.play(e, o, n, now);
                }
            }
        }
        self.shown_ms = Some(t);
    }
}

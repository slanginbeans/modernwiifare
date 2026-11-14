//! The first-person weapon's animation tree, as IW4L's `render_anim::anim::viewmodel_controller` (MW2's viewmodel
//! anim tree): one node per weapon animation slot, each with a weight that moves towards a goal weight, a time and a
//! rate. The gun's `weap_anim` word starts clips (IW4L `observe_weap_anim_edge`: idle, forced idle, or a slot to
//! dispatch); an action clip's rate is fitted to its weapon timer (`playback_rate`: a reload plays in the weapon's
//! reload time, halved with Sleight of Hand); idle only takes over once the action clip has finished. The ADS up and
//! down clips are scrubbed by the ADS fraction as overlays (`ads_overlay_scrub`). Every weighted clip is blended per
//! bone, rotations and translations weighted and normalised (IW4L `DObj::calc_anim`).

use alloc::vec::Vec;

use weapon_iw4::{
    ACTION_GOAL_TIME_SECS, ACTIVE_GOAL_WEIGHT, IDLE_INTERRUPT_GOAL_TIME_SECS, INACTIVE_GOAL_WEIGHT,
    WEAP_ANIM_EVENT_MASK, WeaponCombatFacts, ads_overlay_scrub, playback_rate,
    slot_for_weap_anim_event, slot_uses_native_rate,
};
use wpk::view::{Clip, ViewModel, quat_from_i16, slot};

use crate::gfx::Mtx34;

const SLOTS: usize = slot::COUNT;
/// The slots that idle and actions compete over (everything but the ADS overlays).
const DISPATCH_FIRST: usize = 1;
const DISPATCH_LAST: usize = slot::ADS_RECHAMBER as usize;

/// The weapon timers clips are fitted to (from the weapon's combat facts).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timers {
    pub fire: i32,
    pub raise: i32,
    pub quick_raise: i32,
    pub alt_raise: i32,
    pub drop: i32,
    pub quick_drop: i32,
    pub alt_drop: i32,
    pub reload: i32,
    pub reload_empty: i32,
    pub reload_start: i32,
    pub reload_end: i32,
    pub sprint_raise: i32,
    pub sprint_drop: i32,
    pub melee: i32,
    pub melee_charge: i32,
    pub inherits_perks: bool,
}

impl Timers {
    pub fn of(f: &WeaponCombatFacts) -> Timers {
        Timers {
            fire: f.fire_time_ms,
            raise: f.raise_time_ms,
            quick_raise: f.quick_raise_time_ms,
            alt_raise: f.alternate_raise_time_ms,
            drop: f.drop_time_ms,
            quick_drop: f.quick_drop_time_ms,
            alt_drop: f.alternate_drop_time_ms,
            reload: f.reload_time_ms,
            reload_empty: f.reload_empty_time_ms,
            reload_start: f.reload_start_time_ms,
            reload_end: f.reload_end_time_ms,
            sprint_raise: f.sprint_raise_time_ms,
            sprint_drop: f.sprint_drop_time_ms,
            melee: f.melee_time_ms,
            melee_charge: f.melee_charge_time_ms,
            inherits_perks: f.inherits_perks,
        }
    }

    /// The timer a slot's clip is fitted to (IW4L `state_and_timer_for_slot`), None for the clip's own rate.
    fn for_slot(&self, s: u8) -> Option<i32> {
        let pos = |ms: i32| (ms > 0).then_some(ms);
        match s {
            slot::FIRE | slot::ADS_FIRE | slot::LASTSHOT | slot::ADS_LASTSHOT | slot::HOLD_FIRE => {
                pos(self.fire)
            }
            slot::QUICK_RAISE => pos(self.quick_raise),
            slot::ALT_RAISE => pos(self.alt_raise),
            slot::RAISE => Some(self.raise),
            slot::RELOAD => pos(self.reload),
            slot::RELOAD_EMPTY => pos(self.reload_empty),
            slot::RELOAD_START => pos(self.reload_start),
            slot::RELOAD_END => pos(self.reload_end),
            slot::DROP => pos(self.drop),
            slot::QUICK_DROP => pos(self.quick_drop),
            slot::ALT_DROP => pos(self.alt_drop),
            slot::SPRINT_IN => pos(self.sprint_raise),
            slot::SPRINT_OUT => pos(self.sprint_drop),
            slot::MELEE => pos(self.melee),
            slot::MELEE_CHARGE => pos(self.melee_charge),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Node {
    weight: f32,
    goal: f32,
    goal_left: f32,
    time: f32,
    rate: f32,
}

/// One clip being played this frame: its index in the view model's clips, time and weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub clip: usize,
    pub time: f32,
    pub weight: f32,
}

pub struct Animator {
    nodes: [Node; SLOTS],
    /// The clip of each slot (index into `ViewModel::clips`).
    clip_of: [Option<usize>; SLOTS],
    /// Each slot clip's duration and whether it loops.
    duration: [f32; SLOTS],
    looping: [bool; SLOTS],
    timers: Timers,
    action: Option<u8>,
    action_left: Option<f32>,
    last_ads_frac: f32,
    last_weap_anim: Option<i32>,
    perks: [u32; 2],
}

fn clip_duration(c: &Clip) -> f32 {
    f32::from(c.frames.saturating_sub(1)) / c.framerate.max(1e-3)
}

impl Animator {
    pub fn new(vm: &ViewModel, timers: Timers) -> Animator {
        let mut a = Animator {
            nodes: [Node {
                rate: 1.0,
                ..Node::default()
            }; SLOTS],
            clip_of: [None; SLOTS],
            duration: [0.0; SLOTS],
            looping: [false; SLOTS],
            timers,
            action: None,
            action_left: None,
            last_ads_frac: 0.0,
            last_weap_anim: None,
            perks: [0; 2],
        };
        for (i, c) in vm.clips.iter().enumerate() {
            let s = usize::from(c.slot);
            if s < SLOTS && a.clip_of[s].is_none() {
                a.clip_of[s] = Some(i);
                a.duration[s] = clip_duration(c);
                a.looping[s] = c.looping;
            }
        }
        a.set_weight(slot::IDLE, ACTIVE_GOAL_WEIGHT, ACTION_GOAL_TIME_SECS);
        // Bones only the ADS clips animate still need their lowered pose at the hip.
        if a.has(slot::ADS_DOWN) {
            let s = usize::from(slot::ADS_DOWN);
            a.set_weight(slot::ADS_DOWN, ACTIVE_GOAL_WEIGHT, 0.0);
            a.nodes[s].time = a.duration[s];
            a.nodes[s].rate = 0.0;
        }
        a
    }

    fn has(&self, s: u8) -> bool {
        self.clip_of[usize::from(s)].is_some()
    }

    fn set_weight(&mut self, s: u8, weight: f32, goal_time: f32) {
        let n = &mut self.nodes[usize::from(s)];
        if self.clip_of[usize::from(s)].is_none() {
            return;
        }
        n.goal = weight.max(0.0);
        n.goal_left = goal_time.max(0.0);
        if n.goal_left <= 0.0 {
            n.weight = n.goal;
        }
    }

    fn zero_dispatch(&mut self, goal_time: f32) {
        for s in DISPATCH_FIRST..=DISPATCH_LAST {
            self.set_weight(s as u8, INACTIVE_GOAL_WEIGHT, goal_time);
            self.nodes[s].rate = 1.0;
        }
    }

    /// The perk bits (Sleight of Hand shortens reload clips, fast sprint recovery the sprint out).
    pub fn set_perks(&mut self, perks: [u32; 2]) {
        self.perks = perks;
    }

    fn perk_scaled(&self, s: u8, timer: Option<i32>) -> Option<i32> {
        if s == slot::SPRINT_OUT && self.perks[1] & playerstate_iw4::PERK1_FASTSPRINTRECOVERY != 0 {
            return timer.map(|t| (t as f32 * playerstate_iw4::SPRINT_RECOVERY_MULTIPLIER) as i32);
        }
        let reload = matches!(
            s,
            slot::RELOAD | slot::RELOAD_EMPTY | slot::RELOAD_START | slot::RELOAD_END
        );
        if reload && weapon_iw4::perk_fastreload_eligible(self.perks[0], self.timers.inherits_perks)
        {
            let m = weapon_iw4::PERK_WEAP_RELOAD_MULTIPLIER_DEFAULT;
            if m > 0.0 {
                return timer.map(|t| libm::roundf(t as f32 * m) as i32);
            }
        }
        timer
    }

    fn start_action(&mut self, s: u8) {
        let i = usize::from(s);
        self.zero_dispatch(ACTION_GOAL_TIME_SECS);
        if !self.has(s) {
            self.action = None;
            self.action_left = None;
            return;
        }
        let timer = self.perk_scaled(s, self.timers.for_slot(s));
        let rate = if slot_uses_native_rate(i) {
            1.0
        } else {
            timer
                .filter(|t| *t > 0)
                .map(|t| playback_rate(libm::roundf(self.duration[i] * 1000.0) as i32, t))
                .unwrap_or(1.0)
        };
        self.nodes[i].time = 0.0;
        self.nodes[i].rate = rate;
        self.set_weight(s, ACTIVE_GOAL_WEIGHT, ACTION_GOAL_TIME_SECS);
        self.action = Some(s);
        self.action_left = if self.looping[i] {
            None
        } else {
            timer.filter(|t| *t > 0).map(|t| t as f32 / 1000.0)
        };
    }

    fn start_idle(&mut self, empty: bool, interrupt: bool) {
        let s = if empty && self.has(slot::EMPTY_IDLE) {
            slot::EMPTY_IDLE
        } else {
            slot::IDLE
        };
        let goal = if interrupt {
            IDLE_INTERRUPT_GOAL_TIME_SECS
        } else {
            ACTION_GOAL_TIME_SECS
        };
        self.action = None;
        self.action_left = None;
        self.zero_dispatch(goal);
        // A fresh idle starts from its beginning only when it wasn't playing.
        if self.nodes[usize::from(s)].weight <= 0.0 {
            self.nodes[usize::from(s)].time = 0.0;
        }
        self.set_weight(s, ACTIVE_GOAL_WEIGHT, goal);
    }

    /// Whether a weighted action clip is still playing (a looping one always is).
    fn dispatch_unfinished(&self) -> bool {
        (DISPATCH_FIRST..=DISPATCH_LAST).any(|s| {
            let n = &self.nodes[s];
            n.weight > 0.0
                && self.clip_of[s].is_some()
                && s != usize::from(slot::IDLE)
                && s != usize::from(slot::EMPTY_IDLE)
                && (self.looping[s] || n.time < self.duration[s])
        })
    }

    /// The gun's `weap_anim` this frame (`empty`: the magazine is empty, for the empty idle).
    pub fn weap_anim(&mut self, raw: i32, empty: bool) {
        let last = self.last_weap_anim.replace(raw);
        if last == Some(raw) {
            return;
        }
        let masked = raw as u32 & WEAP_ANIM_EVENT_MASK;
        match masked {
            0 => {
                if self.dispatch_unfinished() {
                    // Try again next frame, once the action clip has played out.
                    self.last_weap_anim = None;
                } else {
                    self.start_idle(empty, false);
                }
            }
            1 => {
                let interrupt = self.dispatch_unfinished();
                self.start_idle(empty, interrupt);
            }
            _ => match slot_for_weap_anim_event(masked) {
                Some(s) if s == usize::from(slot::IDLE) || s == usize::from(slot::EMPTY_IDLE) => {
                    self.start_idle(empty, false)
                }
                Some(s)
                    if s < SLOTS
                        && s != usize::from(slot::ADS_UP)
                        && s != usize::from(slot::ADS_DOWN) =>
                {
                    let s = s as u8;
                    // MW2 picks the ADS fire clip while aiming.
                    let s = match s {
                        slot::FIRE if self.last_ads_frac > 0.0 && self.has(slot::ADS_FIRE) => {
                            slot::ADS_FIRE
                        }
                        slot::LASTSHOT
                            if self.last_ads_frac > 0.0 && self.has(slot::ADS_LASTSHOT) =>
                        {
                            slot::ADS_LASTSHOT
                        }
                        other => other,
                    };
                    self.start_action(s);
                }
                _ => {
                    if self.dispatch_unfinished() {
                        self.last_weap_anim = None;
                    } else {
                        self.start_idle(empty, false);
                    }
                }
            },
        }
    }

    /// Advance every clip by `dt` seconds (weights move towards their goals).
    pub fn advance(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        for n in &mut self.nodes {
            if n.goal_left > 0.0 {
                let step = dt.min(n.goal_left);
                n.weight += (n.goal - n.weight) * step / n.goal_left;
                n.goal_left -= step;
                if n.goal_left <= 0.0 {
                    n.weight = n.goal;
                }
            }
            if n.weight > 0.0 || n.goal > 0.0 {
                n.time += dt * n.rate;
            }
        }
        for s in 0..SLOTS {
            if self.looping[s] && self.duration[s] > 0.0 {
                let d = self.duration[s];
                let t = &mut self.nodes[s].time;
                if *t >= d {
                    *t %= d;
                }
            }
        }
        if let Some(left) = self.action_left.as_mut() {
            *left = (*left - dt).max(0.0);
        }
    }

    /// The ADS overlays for ADS fraction `frac` (IW4L `apply_ads_overlay_frame`, MW2's convention: the ADS up clip
    /// at `frac` of its length weighs `frac`; the ADS down clip at `1 - frac` stays on).
    pub fn ads(&mut self, frac: f32) {
        let scrub = ads_overlay_scrub(frac);
        self.last_ads_frac = scrub.ads_up_weight;
        if matches!(self.action, Some(slot::MELEE | slot::MELEE_CHARGE)) {
            self.set_weight(slot::ADS_UP, INACTIVE_GOAL_WEIGHT, 0.0);
            self.set_weight(slot::ADS_DOWN, INACTIVE_GOAL_WEIGHT, 0.0);
            return;
        }
        for (s, norm, weight) in [
            (slot::ADS_UP, scrub.ads_up_time_norm, scrub.ads_up_weight),
            (slot::ADS_DOWN, scrub.ads_down_time_norm, ACTIVE_GOAL_WEIGHT),
        ] {
            if self.has(s) {
                let i = usize::from(s);
                self.nodes[i].rate = 0.0;
                self.nodes[i].time = self.duration[i] * norm;
                self.set_weight(s, weight, ACTION_GOAL_TIME_SECS);
            }
        }
    }

    /// The clips with weight this frame.
    pub fn playing(&self, out: &mut Vec<Playing>) {
        out.clear();
        for s in 0..SLOTS {
            let n = &self.nodes[s];
            if n.weight > 0.0
                && let Some(c) = self.clip_of[s]
            {
                let t = if self.looping[s] {
                    n.time
                } else {
                    n.time.min(self.duration[s])
                };
                out.push(Playing {
                    clip: c,
                    time: t,
                    weight: n.weight,
                });
            }
        }
    }

    /// The action slot playing (the one notetrack sounds follow), with its time.
    pub fn action(&self) -> Option<(usize, f32)> {
        let s = usize::from(self.action?);
        Some((self.clip_of[s]?, self.nodes[s].time.min(self.duration[s])))
    }

    /// The idle clip playing (looping), with its time.
    pub fn idle(&self) -> Option<(usize, f32)> {
        [slot::IDLE, slot::EMPTY_IDLE].iter().find_map(|&s| {
            let i = usize::from(s);
            (self.nodes[i].weight > 0.0).then_some(())?;
            Some((self.clip_of[i]?, self.nodes[i].time))
        })
    }
}

type Quat = [f32; 4];

/// Sample clip `c` at `time`: each track's bone, rotation and translation delta.
fn sample(c: &Clip, time: f32, bone: &mut dyn FnMut(usize, Quat, Option<[f32; 3]>)) {
    let n = usize::from(c.frames.max(1));
    let (a, b, w) = if n == 1 {
        (0, 0, 0.0)
    } else {
        let last = (n - 1) as f32;
        let mut f = time * c.framerate;
        if c.looping {
            f %= last;
            if f < 0.0 {
                f += last;
            }
        } else {
            f = f.clamp(0.0, last);
        }
        let a = libm::floorf(f) as usize;
        (a, (a + 1).min(n - 1), f - a as f32)
    };
    for tr in &c.tracks {
        let at = |i: usize, len: usize| if len == 1 { 0 } else { i.min(len - 1) };
        let qa = quat_from_i16(tr.rot[at(a, tr.rot.len())]);
        let mut qb = quat_from_i16(tr.rot[at(b, tr.rot.len())]);
        if qa.iter().zip(&qb).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
            qb = qb.map(|v| -v);
        }
        let q: Quat = core::array::from_fn(|k| qa[k] + (qb[k] - qa[k]) * w);
        let t = (!tr.trans.is_empty()).then(|| {
            let (ta, tb) = (
                tr.trans[at(a, tr.trans.len())],
                tr.trans[at(b, tr.trans.len())],
            );
            core::array::from_fn(|k| ta[k] + (tb[k] - ta[k]) * w)
        });
        bone(usize::from(tr.bone), q, t);
    }
}

/// Scratch for [`pose_blend`] (kept between frames: no allocation per frame).
#[derive(Default)]
pub struct BlendScratch {
    rot: Vec<Quat>,
    trans: Vec<[f32; 3]>,
    weight: Vec<f32>,
}

/// Model-space transforms of every bone with the weighted `clips` blended per bone (IW4 `calc_anim`): a bone some
/// clips animate takes their weighted, normalised rotation and translation delta; the others keep the bind pose.
pub fn pose_blend(vm: &ViewModel, clips: &[Playing], s: &mut BlendScratch, out: &mut Vec<Mtx34>) {
    let n = vm.bones.len();
    s.rot.clear();
    s.rot.resize(n, [0.0; 4]);
    s.trans.clear();
    s.trans.resize(n, [0.0; 3]);
    s.weight.clear();
    s.weight.resize(n, 0.0);
    let (rot, trans, weight) = (&mut s.rot, &mut s.trans, &mut s.weight);
    for p in clips {
        let Some(c) = vm.clips.get(p.clip) else {
            continue;
        };
        let w = p.weight;
        sample(c, p.time, &mut |bone, q, t| {
            if bone >= n {
                return;
            }
            let acc = &mut rot[bone];
            let sign = if acc.iter().zip(&q).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
                -1.0
            } else {
                1.0
            };
            for k in 0..4 {
                acc[k] += q[k] * w * sign;
            }
            weight[bone] += w;
            if let Some(t) = t {
                for k in 0..3 {
                    trans[bone][k] += t[k] * w;
                }
            }
        });
    }
    out.clear();
    for (i, bone) in vm.bones.iter().enumerate() {
        let (q, delta) = if weight[i] > 0.0 {
            let r = 1.0 / weight[i];
            (rot[i], trans[i].map(|v| v * r))
        } else {
            (bone.local_rot, [0.0; 3])
        };
        let t = core::array::from_fn(|k| bone.local_trans[k] + delta[k]);
        let local = crate::viewmodel::mtx(q, t);
        let world = if bone.parent >= 0 {
            crate::viewmodel::mul(&out[bone.parent as usize], &local)
        } else {
            local
        };
        out.push(world);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::view::{Bone, Track};

    fn clip(s: u8, frames: u16, looping: bool, angle: f32) -> Clip {
        // Bone 1 turns about z from 0 to `angle` over the clip (1 s at 10 fps when frames = 11).
        let q = |a: f32| {
            let h = a.to_radians() * 0.5;
            [
                0,
                0,
                (libm::sinf(h) * 32767.0) as i16,
                (libm::cosf(h) * 32767.0) as i16,
            ]
        };
        Clip {
            slot: s,
            looping,
            framerate: 10.0,
            frames,
            tracks: vec![Track {
                bone: 1,
                rot: (0..frames)
                    .map(|f| q(angle * f as f32 / f32::from(frames - 1).max(1.0)))
                    .collect(),
                trans: vec![],
            }],
        }
    }

    fn model() -> ViewModel {
        let bone = |parent: i16| Bone {
            parent,
            local_rot: [0.0, 0.0, 0.0, 1.0],
            local_trans: [10.0, 0.0, 0.0],
            inv_bind: crate::viewmodel::IDENTITY,
        };
        ViewModel {
            bones: vec![bone(-1), bone(0)],
            clips: vec![
                clip(slot::IDLE, 11, true, 0.0),
                clip(slot::RELOAD, 11, false, 90.0),
                clip(slot::ADS_UP, 11, false, 40.0),
            ],
            ..ViewModel::default()
        }
    }

    fn timers() -> Timers {
        Timers {
            reload: 2000,
            fire: 100,
            ..Timers::default()
        }
    }

    #[test]
    fn a_reload_plays_in_the_weapons_reload_time_and_idle_waits_for_it() {
        let vm = model();
        let mut a = Animator::new(&vm, timers());
        // weap_anim: reload (0xd), then idle (0) half way: the reload keeps playing.
        a.weap_anim(0xd, false);
        a.advance(1.0);
        let (clip, t) = a.action().unwrap();
        assert_eq!(vm.clips[clip].slot, slot::RELOAD);
        assert!(
            (t - 0.5).abs() < 1e-3,
            "a 1 s clip over a 2 s reload: half speed, {t}"
        );
        a.weap_anim(0, false);
        a.advance(0.5);
        assert!(a.action().is_some(), "idle waits");
        a.advance(0.6);
        a.weap_anim(0, false);
        assert!(a.action().is_none(), "then takes over");
        // Sleight of Hand halves the reload: full speed.
        a.set_perks([weapon_iw4::PERK_FASTRELOAD, 0]);
        let mut fast = Animator::new(
            &vm,
            Timers {
                inherits_perks: true,
                ..timers()
            },
        );
        fast.set_perks([weapon_iw4::PERK_FASTRELOAD, 0]);
        fast.weap_anim(0xd, false);
        fast.advance(0.5);
        assert!((fast.action().unwrap().1 - 0.5).abs() < 1e-3);
    }

    /// The M4's first-person file from a locally converted `out/packs/views/m4.wpk` (never in git; skipped when
    /// absent): its clips fill MW2's slots, and a reload plays through in the weapon's reload time.
    #[test]
    fn real_m4_view_animates() {
        extern crate std;
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/");
        let (Ok(view), Ok(common)) = (
            std::fs::read(std::format!("{base}views/m4.wpk")),
            std::fs::read(std::format!("{base}common.wpk")),
        ) else {
            std::println!("no local view pack, skipped");
            return;
        };
        let pack = wpk::Pack::parse(&view, true).unwrap();
        let vm = ViewModel::decode(pack.section(wpk::tag::VIEWMODEL).unwrap().data).unwrap();
        let slots: Vec<u8> = vm.clips.iter().map(|c| c.slot).collect();
        std::println!("m4 clips: {slots:?}");
        for s in [
            slot::IDLE,
            slot::FIRE,
            slot::RELOAD,
            slot::RELOAD_EMPTY,
            slot::RAISE,
            slot::DROP,
            slot::ADS_UP,
        ] {
            assert!(slots.contains(&s), "slot {s:#x} missing");
        }
        let cp = wpk::Pack::parse(&common, true).unwrap();
        let rec = wpk::weapon::find(cp.section(wpk::tag::WEAPONS).unwrap().data, "m4_mp").unwrap();
        let facts = WeaponCombatFacts::try_from_captured(rec.combat).unwrap();
        let mut a = Animator::new(&vm, Timers::of(&facts));
        a.weap_anim(0xd, false);
        let mut t = 0.0;
        while a.action().is_some() && t < 10.0 {
            a.advance(1.0 / 60.0);
            a.weap_anim(0, false);
            t += 1.0 / 60.0;
        }
        let want = facts.reload_time_ms as f32 / 1000.0;
        assert!(
            (t - want).abs() < 0.1,
            "reload clip {t} s, weapon reload {want} s"
        );
        let mut playing = Vec::new();
        let mut scratch = BlendScratch::default();
        let mut world = Vec::new();
        a.ads(1.0);
        a.playing(&mut playing);
        pose_blend(&vm, &playing, &mut scratch, &mut world);
        assert_eq!(world.len(), vm.bones.len());
        assert!(world.iter().flatten().flatten().all(|v| v.is_finite()));
    }
    #[test]
    fn ads_overlay_blends_the_ads_up_pose_by_the_fraction() {
        let vm = model();
        let mut a = Animator::new(&vm, timers());
        let mut playing = Vec::new();
        let mut scratch = BlendScratch::default();
        let mut world = Vec::new();
        let angle = |a: &Animator,
                     playing: &mut Vec<Playing>,
                     scratch: &mut BlendScratch,
                     world: &mut Vec<Mtx34>| {
            a.playing(playing);
            pose_blend(&vm, playing, scratch, world);
            // Bone 1's x axis direction in the plane.
            libm::atan2f(world[1][1][0], world[1][0][0]).to_degrees()
        };
        a.ads(0.0);
        assert!(angle(&a, &mut playing, &mut scratch, &mut world).abs() < 0.5);
        a.ads(1.0);
        // Idle (0°) and ADS up at its end (40°), weights 1 and 1: 20°.
        let full = angle(&a, &mut playing, &mut scratch, &mut world);
        assert!((full - 20.0).abs() < 1.0, "{full}");
        a.ads(0.5);
        // ADS up half way (20°) at weight 0.5 against idle at 1: 20 * 0.5 / 1.5.
        let half = angle(&a, &mut playing, &mut scratch, &mut world);
        assert!((half - 20.0 / 3.0).abs() < 1.0, "{half}");
    }
}

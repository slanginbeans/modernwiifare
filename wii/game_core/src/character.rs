//! Player characters: MW2's player animation script run per player, its legs and torso animations blended on the
//! character's skeleton, and IW4's spine controllers turning the torso to where the player aims.
//!
//! - The script (`wpk::anim_script`, the user's `mp/playeranim.script`): each frame the player's movement type picks
//!   the first item whose conditions hold, and its command plays on the legs (`legs`), the torso (`torso`) or both
//!   (`both`: legs, with the torso animation cleared). Events (firing, reloading, dying) play their item's command
//!   and hold that body part for the command's duration (IW4 `BG_AnimScriptAnimation` / `BG_AnimScriptEvent`, as
//!   IW4L's `sim::player_anim_script`).
//! - Pose: the legs animation over the whole skeleton, then the torso animation's bones over it (IW4L
//!   `overlay_legs_clip`); a change of animation cross-fades over the blend time IW4 picks (`anim_iw4`).
//! - Controllers: IW4's player controller tags (`back_low`, `back_mid`, `back_up`) take the aim pitch and the
//!   torso's yaw away from the legs, in the root's frame (IW4L `xmodel_runtime::apply_player_controller`). The legs
//!   turn after the view once it is more than 20 degrees away (IW4 `BG_SwingAngles`, ported from IW4L
//!   `entity_iw4::player_angles`).

use alloc::string::String;
use alloc::vec::Vec;

use anim_iw4::{
    ANIM_COND_FIRING, ANIM_COND_MOVETYPE, ANIM_COND_PLAYERANIMTYPE,
    ANIM_COND_PLAYERANIMTYPEPRIMARY, ANIM_COND_STRAFING, ANIM_COND_WEAPON_POSITION,
    ANIM_COND_WEAPONCLASS, ANIM_MT_IDLE,
};
use wpk::anim_script::{Conditions, Script, part};
use wpk::player_anim::{Anims, ClipRef};
use wpk::view::ViewModel;

use crate::gfx::Mtx34;
use crate::viewmodel::{mtx, mul, nlerp};

/// No model bone.
pub const NO_BONE: u16 = u16::MAX;

/// MW2's player animations and the script that plays them (from `common.wpk`).
pub struct PlayerAnims<'a> {
    pub clips: Anims<'a>,
    pub script: Script,
    /// Each clip's move speed (`PANS`: big-endian f32 units/s; 0 or absent: played at rate 1).
    pub speeds: &'a [u8],
}

impl PlayerAnims<'_> {
    /// Clip `clip`'s move speed (IW4L `AnimClip::move_speed`, 0 for IW4's stationary clips).
    pub fn move_speed(&self, clip: u16) -> f32 {
        let at = usize::from(clip) * 4;
        self.speeds
            .get(at..at + 4)
            .map_or(0.0, |b| f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// A character model's links to the animations: the model bone of each animation bone (by name), and its
/// controller and weapon tags.
#[derive(Clone, Debug, Default)]
pub struct Rig {
    pub map: Vec<u16>,
    /// `back_low`, `back_mid`, `back_up` (IW4L `PLAYER_CONTROLLER_TAGS` without `pelvis`, which IW4 leaves still).
    pub controllers: [u16; 3],
    /// `tag_weapon_right` (where the third-person weapon goes), or [`NO_BONE`].
    pub weapon: u16,
    /// `tag_weapon_left` (a riot shield in the hands) and `tag_shield_back` (a riot shield carried as the other
    /// weapon), or [`NO_BONE`].
    pub weapon_left: u16,
    pub shield_back: u16,
}

impl Rig {
    pub fn new(model_names: &[String], anim_names: &[String]) -> Rig {
        let find = |n: &str| {
            model_names
                .iter()
                .position(|m| m == n)
                .map_or(NO_BONE, |i| i as u16)
        };
        Rig {
            map: anim_names
                .iter()
                .map(|n| if n.is_empty() { NO_BONE } else { find(n) })
                .collect(),
            controllers: [find("back_low"), find("back_mid"), find("back_up")],
            weapon: find("tag_weapon_right"),
            weapon_left: find("tag_weapon_left"),
            shield_back: find("tag_shield_back"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Playing {
    clip: u16,
    moving: bool,
    /// The clip's time (ms) at `anchor_ms`, from where it runs on at `rate` (IW4L `XAnimTreeRuntime::set_rate`).
    anchor_ms: u32,
    anchor_time: f32,
    rate: f32,
}

impl Playing {
    /// The clip's time (seconds) at `now`.
    fn time(&self, now: u32) -> f32 {
        (self.anchor_time + now.wrapping_sub(self.anchor_ms) as f32 * self.rate) / 1000.0
    }

    /// Run at `rate` from `from_ms` on (IW4L sets the rate measured over a frame, then advances the tree by that
    /// frame: the new rate covers the frame just measured). Never before the clip's own anchor.
    fn set_rate(&mut self, rate: f32, from_ms: u32) {
        let from = if (from_ms.wrapping_sub(self.anchor_ms) as i32) < 0 {
            self.anchor_ms
        } else {
            from_ms
        };
        self.anchor_time += from.wrapping_sub(self.anchor_ms) as f32 * self.rate;
        self.anchor_ms = from;
        self.rate = rate;
    }
}

/// Where and when a body part's rate was last measured (IW4L `ClientAnimSample`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct RateSample {
    origin: [f32; 3],
    time_ms: i32,
}

/// One body part's animation: what plays, what it is fading from, and how long an event holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Part {
    cur: Option<Playing>,
    prev: Option<Playing>,
    blend_start_ms: u32,
    blend_ms: u32,
    /// An event's animation holds the part until then.
    hold_until_ms: u32,
    rate: RateSample,
}

impl Part {
    fn held(&self, now: u32) -> bool {
        // IW4: a timer of 50 ms or more blocks a new animation.
        self.hold_until_ms.wrapping_sub(now) as i32 >= 50
    }

    /// How far the fade from `prev` to `cur` is (1 when done).
    fn weight(&self, now: u32) -> f32 {
        if self.blend_ms == 0 {
            return 1.0;
        }
        (now.wrapping_sub(self.blend_start_ms) as f32 / self.blend_ms as f32).clamp(0.0, 1.0)
    }

    /// Play `clip` (`None`: no animation on this part). `restart`: start it again when it is already playing.
    /// `phase`: where in the clip to start (0..1), else its start.
    #[allow(clippy::too_many_arguments)]
    fn play(
        &mut self,
        clip: Option<u16>,
        moving: bool,
        authored_blend_ms: i32,
        torso: bool,
        now: u32,
        restart: bool,
        phase: Option<f32>,
    ) {
        let same = self.cur.map(|p| p.clip) == clip;
        if same && !restart {
            return;
        }
        let old = self.cur;
        let blend = anim_iw4::client_anim_blend_ms(
            clip.map_or(0, |c| c + 1),
            authored_blend_ms.max(0),
            old.is_some(),
            torso,
            old.is_some_and(|p| p.moving),
            moving,
        );
        // A fade still under way restarts from what shows now (close enough: the newer of the two).
        self.prev = old;
        self.cur = clip.map(|clip| Playing {
            clip,
            moving,
            anchor_ms: now,
            anchor_time: phase.unwrap_or(0.0),
            rate: 1.0,
        });
        self.blend_start_ms = now;
        self.blend_ms = blend.max(0) as u32;
    }
}

/// IW4 `BG_SwingAngles` (IW4L `entity_iw4::player_angles::swing_angles`): turn `angle` toward `dest`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Swing {
    angle: f32,
    swinging: bool,
}

fn angle_sub(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

fn norm360(a: f32) -> f32 {
    let a = a % 360.0;
    if a < 0.0 { a + 360.0 } else { a }
}

impl Swing {
    fn swing(&mut self, dest: f32, tolerance: f32, clamp: f32, frametime_ms: f32) {
        const SPEED: f32 = 0.2;
        if !self.swinging {
            let d = angle_sub(self.angle, dest);
            if d <= tolerance && -tolerance <= d {
                return;
            }
            self.swinging = true;
        }
        let swing = angle_sub(dest, self.angle);
        let scale = (libm::fabsf(swing) * 0.05).max(0.5);
        let step = if swing < 0.0 {
            let s = -frametime_ms * scale * SPEED;
            if swing < s {
                s
            } else {
                self.swinging = false;
                swing
            }
        } else {
            let s = frametime_ms * scale * SPEED;
            if s < swing {
                s
            } else {
                self.swinging = false;
                swing
            }
        };
        self.angle = norm360(self.angle + step);
        let after = angle_sub(dest, self.angle);
        if after > clamp {
            self.angle = norm360(dest - clamp);
        } else if after < -clamp {
            self.angle = norm360(dest + clamp);
        }
    }
}

/// One player's body animation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Body {
    legs: Part,
    torso: Part,
    legs_yaw: Swing,
    torso_yaw: Swing,
    /// Which of an item's commands this player takes (IW4: the client number).
    client: u32,
    seed: u32,
    last_ms: Option<u32>,
}

/// Movement types whose legs animation moves the player (IW4: an animation with a move speed).
fn moving(movetype: u8) -> bool {
    matches!(movetype, 4..=13 | 20)
}

impl Body {
    pub fn new(client: u32) -> Body {
        Body {
            client,
            seed: client.wrapping_mul(0x9E37_79B9) | 1,
            ..Body::default()
        }
    }

    /// Start over (a respawn): nothing playing, facing `yaw`.
    pub fn reset(&mut self, yaw: f32) {
        *self = Body {
            legs_yaw: Swing {
                angle: norm360(yaw),
                swinging: false,
            },
            torso_yaw: Swing {
                angle: norm360(yaw),
                swinging: false,
            },
            ..Body::new(self.client)
        };
    }

    fn duration_ms(anims: &PlayerAnims, clip: u16) -> u32 {
        anims
            .clips
            .clip(usize::from(clip))
            .map_or(0, |c| (c.duration() * 1000.0) as u32)
    }

    /// Run the command: legs for `legs` / `both`, torso for `torso`, and `both` clears the torso. `hold`: an event
    /// (holds the part for its duration, restarts its animation).
    fn execute(
        &mut self,
        anims: &PlayerAnims,
        cmd: wpk::anim_script::Command,
        movetype: u8,
        now: u32,
        hold: bool,
        force: bool,
    ) {
        let duration = if cmd.duration_ms > 0 {
            cmd.duration_ms as u32
        } else {
            Self::duration_ms(anims, cmd.clip)
        } + 50;
        let go = |p: &mut Part, clip: Option<u16>, torso: bool| {
            if p.held(now) && !force {
                return;
            }
            let moves = !torso && moving(movetype);
            // IW4L: a moving looping legs clip after another one carries on at the same point of its cycle.
            let phase = match (p.cur, clip) {
                (Some(old), Some(new)) if moves && old.moving && !hold && old.clip != new => {
                    let looped = |c: u16| anims.clips.clip(usize::from(c));
                    match (looped(old.clip), looped(new)) {
                        (Some(a), Some(b)) if a.looping() && b.looping() && a.duration() > 0.0 => {
                            let t = old.time(now) / a.duration();
                            let frac = t - libm::floorf(t);
                            Some(frac * b.duration() * 1000.0)
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            p.play(clip, moves, cmd.blend_ms, torso, now, hold, phase);
            if hold {
                p.hold_until_ms = now.wrapping_add(duration);
            }
        };
        match cmd.part {
            part::LEGS => go(&mut self.legs, Some(cmd.clip), false),
            part::TORSO => go(&mut self.torso, Some(cmd.clip), true),
            part::BOTH => {
                go(&mut self.legs, Some(cmd.clip), false);
                go(&mut self.torso, None, true);
            }
            _ => {}
        }
    }

    /// This frame's movement (IW4 `BG_AnimScriptAnimation`): the item for `movetype` (or idle's) under `c`.
    pub fn moves(&mut self, anims: &PlayerAnims, movetype: u8, c: &Conditions, now: u32) {
        let pick = c
            .first(anims.script.move_items(movetype))
            .map(|i| (movetype, i))
            .or_else(|| {
                (movetype != ANIM_MT_IDLE)
                    .then(|| c.first(anims.script.move_items(ANIM_MT_IDLE)))
                    .flatten()
                    .map(|i| (ANIM_MT_IDLE, i))
            });
        let Some((mt, item)) = pick else {
            return;
        };
        if item.commands.is_empty() {
            return;
        }
        let cmd = item.commands[self.client as usize % item.commands.len()];
        self.execute(anims, cmd, mt, now, false, false);
    }

    /// An event (IW4 `BG_AnimScriptEvent`, `ANIM_ET_*`) under `c`; `force` overrides a held part (death).
    /// Returns whether an animation played.
    pub fn event(
        &mut self,
        anims: &PlayerAnims,
        event: u8,
        movetype: u8,
        c: &Conditions,
        now: u32,
        force: bool,
    ) -> bool {
        let Some(item) = c.first(anims.script.event_items(event)) else {
            return false;
        };
        if item.commands.is_empty() {
            return false;
        }
        let pick = (anim_iw4::random(&mut self.seed) as usize) % item.commands.len();
        self.execute(anims, item.commands[pick], movetype, now, true, force);
        true
    }

    /// Each body part's playback rate from how fast the player at `origin` moves against its clip's move speed
    /// (IW4L `apply_player_anim_rates` / `anim_iw4::client_anim_playback_rate`, once per frame before the pose).
    pub fn rates(&mut self, anims: &PlayerAnims, origin: [f32; 3], now: u32) {
        for p in [&mut self.legs, &mut self.torso] {
            let Some(cur) = p.cur.as_mut() else {
                continue;
            };
            let now_i = now as i32;
            if now_i < p.rate.time_ms {
                p.rate.time_ms = 0;
            }
            let Some(rate) = anim_iw4::client_anim_playback_rate(
                origin,
                p.rate.origin,
                now_i,
                p.rate.time_ms,
                anims.move_speed(cur.clip),
                false,
            ) else {
                continue;
            };
            if rate != cur.rate {
                cur.set_rate(rate, p.rate.time_ms as u32);
            }
            p.rate = RateSample {
                origin,
                time_ms: now_i,
            };
        }
    }

    /// Whether the legs play a clip whose time runs at other than its own pace (for tests and the log).
    pub fn legs_rate(&self) -> f32 {
        self.legs.cur.map_or(1.0, |c| c.rate)
    }

    /// Turn the legs and torso toward `view_yaw` (degrees): the torso follows within 90 degrees, the legs once the
    /// view is 20 degrees away (at once while moving). Returns (legs yaw, torso yaw).
    pub fn turn(&mut self, view_yaw: f32, moving: bool, now: u32) -> (f32, f32) {
        let dt = self
            .last_ms
            .map_or(0.0, |l| now.wrapping_sub(l).min(200) as f32);
        if self.last_ms.is_none() {
            self.legs_yaw.angle = norm360(view_yaw);
            self.torso_yaw.angle = norm360(view_yaw);
        }
        self.last_ms = Some(now);
        let dest = norm360(view_yaw);
        self.torso_yaw.swing(dest, 0.0, 90.0, dt);
        let tolerance = if self.legs_yaw.swinging || moving {
            0.0
        } else {
            20.0
        };
        self.legs_yaw.swing(dest, tolerance, 150.0, dt);
        (self.legs_yaw.angle, self.torso_yaw.angle)
    }

    /// The pose at `now`: bone world transforms in model space (x forward, z up, at the feet) into `out`.
    /// `aim_pitch`: the view pitch (degrees, positive down); `torso_yaw`: the torso's yaw relative to the legs.
    #[allow(clippy::too_many_arguments)]
    pub fn pose(
        &self,
        anims: &PlayerAnims,
        rig: &Rig,
        model: &ViewModel,
        now: u32,
        aim_pitch: f32,
        torso_yaw: f32,
        prone: bool,
        scratch: &mut Scratch,
        out: &mut Vec<Mtx34>,
    ) {
        self.pose_bones(
            anims, rig, model, now, aim_pitch, torso_yaw, prone, None, scratch, out,
        );
    }

    /// [`Body::pose`] for the bones `need` marks only (none: all). A bone's transform depends on its ancestors
    /// alone, so with `need` closed under parents ([`needed_bones`]) the marked bones come out exactly as a full
    /// pose gives them; the others are left as the identity. A body nobody sees is posed this way for its hit boxes
    /// and weapon tags.
    #[allow(clippy::too_many_arguments)]
    pub fn pose_bones(
        &self,
        anims: &PlayerAnims,
        rig: &Rig,
        model: &ViewModel,
        now: u32,
        aim_pitch: f32,
        torso_yaw: f32,
        prone: bool,
        need: Option<&[bool]>,
        scratch: &mut Scratch,
        out: &mut Vec<Mtx34>,
    ) {
        let n = model.bones.len();
        let s = scratch;
        s.reset(model);
        let sample = |p: &Playing, rot: &mut [[f32; 4]], tr: &mut [[f32; 3]]| {
            if let Some(c) = anims.clips.clip(usize::from(p.clip)) {
                let f = c.frame_at(p.time(now));
                apply(&c, f, rig, need, rot, tr);
            }
        };
        // Legs over the whole body, fading from the previous legs animation.
        if let Some(cur) = &self.legs.cur {
            let w = self.legs.weight(now);
            if let (Some(prev), true) = (&self.legs.prev, w < 1.0) {
                s.copy_base_to_b();
                sample(prev, &mut s.rot, &mut s.tr);
                sample(cur, &mut s.rot_b, &mut s.tr_b);
                s.blend_b_into_a(w, need);
            } else {
                sample(cur, &mut s.rot, &mut s.tr);
            }
        }
        // The torso animation's bones over the legs, fading between torso animations (none: the legs' pose).
        let w = self.torso.weight(now);
        let fading = w < 1.0 && self.torso.prev.is_some();
        if self.torso.cur.is_some() || fading {
            s.copy_a_to_b();
            if let Some(cur) = &self.torso.cur {
                sample(cur, &mut s.rot_b, &mut s.tr_b);
            }
            if fading {
                // a = legs + previous torso.
                if let Some(prev) = &self.torso.prev {
                    sample(prev, &mut s.rot, &mut s.tr);
                }
                s.blend_b_into_a(w, need);
            } else {
                core::mem::swap(&mut s.rot, &mut s.rot_b);
                core::mem::swap(&mut s.tr, &mut s.tr_b);
            }
        }
        // The controllers (IW4L `plan_player_controller`, standing and prone; no lean): pitch doubled then shared
        // out down the spine, the torso's yaw from the legs likewise.
        let pitch = 2.0 * angle_sub(aim_pitch, 0.0);
        let angles: [[f32; 3]; 3] = if prone {
            let p = if pitch <= 0.0 {
                pitch * 0.25
            } else {
                pitch * 0.5
            };
            [
                [0.0; 3],
                [0.0, torso_yaw * 0.1, 0.0],
                [p, torso_yaw * 0.8, 0.0],
            ]
        } else {
            [
                [pitch * 0.2, torso_yaw * 0.4, 0.0],
                [pitch * 0.3, torso_yaw * 0.4, 0.0],
                [pitch * 0.5, torso_yaw * 0.2, 0.0],
            ]
        };
        out.clear();
        for i in 0..n {
            if need.is_some_and(|m| !m.get(i).copied().unwrap_or(false)) {
                out.push(crate::viewmodel::IDENTITY);
                continue;
            }
            let bone = &model.bones[i];
            let t: [f32; 3] = core::array::from_fn(|k| bone.local_trans[k] + s.tr[i][k]);
            let control = rig
                .controllers
                .iter()
                .position(|&c| usize::from(c) == i)
                .map(|k| angles[k]);
            let world = match (bone.parent, control) {
                (p, Some(a)) if p >= 0 => {
                    // world rotation = root Ã— control Ã— rootâ»Â¹ Ã— parent rotation, at the parent's point.
                    let parent = out[p as usize];
                    let root = out[0];
                    let c = mtx(set_angles(a), [0.0; 3]);
                    let r = rot_only(&root);
                    let r_inv = transpose(&r);
                    let rot = mul(&mul(&mul(&r, &c), &r_inv), &rot_only(&parent));
                    let at = crate::viewmodel::apply(&parent, t);
                    [
                        [rot[0][0], rot[0][1], rot[0][2], at[0]],
                        [rot[1][0], rot[1][1], rot[1][2], at[1]],
                        [rot[2][0], rot[2][1], rot[2][2], at[2]],
                    ]
                }
                (p, _) if p >= 0 => mul(&out[p as usize], &mtx(s.rot[i], t)),
                _ => mtx(s.rot[i], t),
            };
            out.push(world);
        }
    }
}

fn rot_only(m: &Mtx34) -> Mtx34 {
    [
        [m[0][0], m[0][1], m[0][2], 0.0],
        [m[1][0], m[1][1], m[1][2], 0.0],
        [m[2][0], m[2][1], m[2][2], 0.0],
    ]
}

fn transpose(m: &Mtx34) -> Mtx34 {
    core::array::from_fn(|r| [m[0][r], m[1][r], m[2][r], 0.0])
}

/// IW4 angles (pitch, yaw, roll in degrees) to a quaternion (IW4L `xmodel_runtime::set_angles`).
fn set_angles(a: [f32; 3]) -> [f32; 4] {
    const HALF: f32 = 0.008_726_646;
    let (ys, yc) = (libm::sinf(a[1] * HALF), libm::cosf(a[1] * HALF));
    let (ps, pc) = (libm::sinf(a[0] * HALF), libm::cosf(a[0] * HALF));
    let (rs, rc) = (libm::sinf(a[2] * HALF), libm::cosf(a[2] * HALF));
    let (t0, t1, t2, t3) = (-ps * ys, ps * yc, pc * ys, pc * yc);
    [
        rs * t3 + rc * t0,
        rc * t1 + rs * t2,
        -rs * t1 + rc * t2,
        rc * t3 - rs * t0,
    ]
}

/// Apply clip `c` at frame `f` to the model bones its tracks map to (those `need` marks, when given).
fn apply(
    c: &ClipRef,
    f: f32,
    rig: &Rig,
    need: Option<&[bool]>,
    rot: &mut [[f32; 4]],
    tr: &mut [[f32; 3]],
) {
    for k in 0..c.tracks() {
        let Some(t) = c.track(k) else { continue };
        let Some(&b) = rig.map.get(usize::from(t.bone)) else {
            continue;
        };
        let b = usize::from(b);
        if b >= rot.len() || need.is_some_and(|m| !m.get(b).copied().unwrap_or(false)) {
            continue;
        }
        if let Some(q) = t.rotation(f) {
            rot[b] = q;
        }
        if let Some(d) = t.translation(f) {
            tr[b] = d;
        }
    }
}

/// The bones `bones` and every ancestor of theirs (a mask for [`Body::pose_bones`]); indices past `model`'s bones
/// are ignored.
pub fn needed_bones(model: &ViewModel, bones: impl IntoIterator<Item = usize>) -> Vec<bool> {
    let mut need = alloc::vec![false; model.bones.len()];
    for b in bones {
        let mut i = b;
        while i < need.len() && !need[i] {
            need[i] = true;
            match usize::try_from(model.bones[i].parent) {
                Ok(p) => i = p,
                Err(_) => break,
            }
        }
    }
    need
}

/// Working buffers for [`Body::pose`] (kept between frames: no allocation per frame).
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    rot: Vec<[f32; 4]>,
    tr: Vec<[f32; 3]>,
    rot_b: Vec<[f32; 4]>,
    tr_b: Vec<[f32; 3]>,
}

impl Scratch {
    fn reset(&mut self, model: &ViewModel) {
        self.rot.clear();
        self.rot.extend(model.bones.iter().map(|b| b.local_rot));
        self.tr.clear();
        self.tr.resize(model.bones.len(), [0.0; 3]);
        self.rot_b.clone_from(&self.rot);
        self.tr_b.clone_from(&self.tr);
    }

    fn copy_base_to_b(&mut self) {
        self.rot_b.clone_from(&self.rot);
        self.tr_b.clone_from(&self.tr);
    }

    fn copy_a_to_b(&mut self) {
        self.copy_base_to_b();
    }

    /// a = a â†’ b by `w`.
    /// (Only the bones `need` marks, when given.)
    fn blend_b_into_a(&mut self, w: f32, need: Option<&[bool]>) {
        for i in 0..self.rot.len() {
            if need.is_some_and(|m| !m.get(i).copied().unwrap_or(false)) {
                continue;
            }
            self.rot[i] = nlerp(self.rot[i], self.rot_b[i], w);
            self.tr[i] =
                core::array::from_fn(|k| self.tr[i][k] + (self.tr_b[i][k] - self.tr[i][k]) * w);
        }
    }
}

/// The movement type for the script (IW4L `movement_iw4::footsteps_anim_move_type`), `None` in the air (the legs
/// keep what they play).
pub fn movetype(ps: &playerstate_iw4::PlayerState, forward: i8, right: i8) -> Option<u8> {
    // A mantle plays its clip's movement type (`MANTLE_UP_*`, then `MANTLE_OVER_*`).
    crate::mantle_path::anim_move_type(ps)
        .or_else(|| movement_iw4::footsteps_anim_move_type(ps, forward, right, false))
}

/// What the movement events need of a player from frame to frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    /// pmove's `jump_time` (changes when a jump starts).
    pub jump_time: i32,
    pub on_ground: bool,
    /// Vertical speed (units/s).
    pub vz: f32,
    pub prone: bool,
    /// Moving backwards (the move command's forward part below zero).
    pub backward: bool,
}

impl Motion {
    pub fn of(ps: &playerstate_iw4::PlayerState, forward: i8) -> Motion {
        Motion {
            jump_time: ps.jump_time,
            on_ground: ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE,
            vz: ps.velocity[2],
            prone: ps.pm_flags & playerstate_iw4::pm_flags::PRONE != 0,
            backward: forward < 0,
        }
    }
}

/// The landing speed (units/s, downwards) that plays the landing animation, as the Quake 3 lineage IW4 comes from.
pub const LAND_SPEED: f32 = 200.0;

/// The animation events (`ANIM_ET_*`) a change of movement from `was` to `now` plays: a jump (backwards when moving
/// back), a landing (when falling faster than [`LAND_SPEED`]), going prone or getting up.
pub fn motion_events(was: &Motion, now: &Motion) -> impl Iterator<Item = u8> {
    let jumped = now.jump_time != was.jump_time && !now.on_ground;
    let landed = now.on_ground && !was.on_ground && was.vz < -LAND_SPEED;
    [
        jumped.then_some(if now.backward { EV_JUMPBK } else { EV_JUMP }),
        landed.then_some(EV_LAND),
        (now.prone && !was.prone).then_some(EV_CROUCH_TO_PRONE),
        (!now.prone && was.prone).then_some(EV_PRONE_TO_CROUCH),
    ]
    .into_iter()
    .flatten()
}

/// IW4 `ANIM_ET_*` numbers IW4L names but doesn't export (`anim_iw4::ANIM_ET_NAMES` order; a test checks them).
pub const EV_JUMP: u8 = 3;
pub const EV_JUMPBK: u8 = 4;
pub const EV_LAND: u8 = 5;
pub const EV_CROUCH_TO_PRONE: u8 = 11;
pub const EV_PRONE_TO_CROUCH: u8 = 12;

/// IW4's strafing condition (IW4L `sim::player_anim_script::anim_strafing`): 0 not, 1 left, 2 right.
pub fn strafing(ps: &playerstate_iw4::PlayerState, forward: i8, right: i8) -> u8 {
    let (f, r) = (f32::from(forward), f32::from(right));
    let len = libm::sqrtf(f * f + r * r);
    let speed_sq = ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1];
    if len == 0.0 || speed_sq <= 100.0 || libm::fabsf(f) > len * 0.5 {
        0
    } else if r > 0.0 {
        2
    } else {
        1
    }
}

/// The weapon a player holds, for the script's conditions: its player-anim type and weapon class (IW4 numbers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub anim_type: u8,
    pub class: u8,
}

/// The script's conditions for a player (IW4L `anim_conditions_from_pmove`): the weapon, the movement type, firing,
/// strafing and aiming down the sights; mounted, akimbo, perks and riot-shield-next are always off here.
pub fn conditions(
    ps: &playerstate_iw4::PlayerState,
    held: Option<Held>,
    movetype: u8,
    strafing: u8,
    firing: bool,
) -> Conditions {
    let mut c = Conditions::default();
    c.set_value(anim_iw4::ANIM_COND_MOUNTED, 0);
    c.set_value(ANIM_COND_FIRING, u32::from(firing));
    // IW4: the fast mantle tree (`mantle_flags` bit 6, Marathon Pro's quick climb).
    c.set_value(anim_iw4::ANIM_COND_FASTMANTLE, (ps.mantle_flags >> 6) & 1);
    if let Some(h) = held {
        c.set_bit(ANIM_COND_PLAYERANIMTYPE, h.anim_type);
        c.set_bit(ANIM_COND_WEAPONCLASS, h.class);
        c.set_bit(ANIM_COND_PLAYERANIMTYPEPRIMARY, h.anim_type);
    }
    c.set_value(anim_iw4::ANIM_COND_AKIMBO, 0);
    c.set_bit(ANIM_COND_MOVETYPE, movetype);
    c.set_value(ANIM_COND_STRAFING, u32::from(strafing));
    c.set_value(anim_iw4::ANIM_COND_PERK, 0);
    c.set_value(anim_iw4::ANIM_COND_RIOTSHIELDNEXT, 0);
    c.set_value(
        ANIM_COND_WEAPON_POSITION,
        anim_iw4::anim_weapon_position_from_pm_flags(ps.pm_flags),
    );
    c
}

/// How a player died, for the death animation's conditions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Death {
    /// `crate::damage::hitloc`.
    pub location: u8,
    /// Direction the damage travelled (world).
    pub dir: [f32; 3],
    /// 0 a bullet (or melee), 1 a light explosion (far), 2 an explosion (near): IW4 `ANIM_DAMAGETYPE_NAMES`.
    pub kind: u8,
}

/// `c` with the death's damage type, hit location and hit direction (IW4 `BG_AnimScriptEvent` for `ANIM_ET_DEATH`)
/// for a player facing `yaw` (degrees).
pub fn death_conditions(mut c: Conditions, d: &Death, yaw: f32) -> Conditions {
    c.set_value(anim_iw4::ANIM_COND_DAMAGETYPE, u32::from(d.kind));
    c.set_value(
        anim_iw4::ANIM_COND_HITLOCATION,
        u32::from(anim_iw4::anim_script_hit_location(d.location)),
    );
    let y = yaw.to_radians();
    let fwd = [libm::cosf(y), libm::sinf(y)];
    let len = libm::sqrtf(d.dir[0] * d.dir[0] + d.dir[1] * d.dir[1]).max(1e-6);
    c.set_value(
        anim_iw4::ANIM_COND_HITDIRECTION,
        u32::from(anim_iw4::anim_script_hit_direction(
            fwd,
            [d.dir[0] / len, d.dir[1] / len],
        )),
    );
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::anim_script::{Command, Condition, Item, NO_BLEND};
    use wpk::view::{Bone, Clip, Track, quat_to_i16};

    /// Clips: 0 idle (bone "j_a" turned 90 degrees about z), 1 run, 2 torso fire (bone "j_b" only), 3 death.
    fn anims_bytes() -> Vec<u8> {
        let h = core::f32::consts::FRAC_1_SQRT_2;
        let clip = |bone: u16, q: [f32; 4], frames: u16, looping: bool| Clip {
            looping,
            framerate: 30.0,
            frames,
            tracks: vec![Track {
                bone,
                rot: vec![quat_to_i16(q)],
                trans: vec![],
                keys: vec![],
            }],
            ..Clip::default()
        };
        wpk::player_anim::encode(&[
            clip(0, [0.0, 0.0, h, h], 30, true),
            clip(0, [0.0, 0.0, 1.0, 0.0], 20, true),
            clip(1, [h, 0.0, 0.0, h], 6, false),
            clip(0, [0.0, h, 0.0, h], 45, false),
        ])
    }

    fn script() -> Script {
        let cmd = |part, clip| Command {
            part,
            clip,
            duration_ms: 0,
            blend_ms: NO_BLEND,
        };
        let any = |cmds| Item {
            skip: false,
            conditions: vec![],
            commands: cmds,
        };
        Script {
            moves: vec![
                (1, vec![any(vec![cmd(part::BOTH, 0)])]),
                (10, vec![any(vec![cmd(part::BOTH, 1)])]),
            ],
            events: vec![
                (
                    2,
                    vec![
                        // Pistols fire with nothing (a made-up rule), others with the torso clip.
                        Item {
                            skip: false,
                            conditions: vec![Condition {
                                index: ANIM_COND_PLAYERANIMTYPE,
                                bitflags: true,
                                bits: 1 << 2,
                                value: 0,
                            }],
                            commands: vec![],
                        },
                        any(vec![cmd(part::TORSO, 2)]),
                    ],
                ),
                (1, vec![any(vec![cmd(part::BOTH, 3)])]),
            ],
        }
    }

    fn model() -> (ViewModel, Vec<String>) {
        let bone = |p: i16| Bone {
            parent: p,
            local_rot: [0.0, 0.0, 0.0, 1.0],
            local_trans: [0.0, 0.0, 10.0],
            ..Bone::default()
        };
        (
            ViewModel {
                bones: vec![bone(-1), bone(0), bone(1)],
                ..ViewModel::default()
            },
            vec!["tag_origin".into(), "j_a".into(), "j_b".into()],
        )
    }

    #[test]
    fn rig_maps_bones_by_name() {
        let (_, names) = model();
        let r = Rig::new(&names, &["j_b".into(), "j_missing".into(), "j_a".into()]);
        assert_eq!(r.map, [2, NO_BONE, 1]);
        assert_eq!(r.controllers, [NO_BONE; 3]);
        assert_eq!(r.weapon, NO_BONE);
    }

    #[test]
    fn script_drives_legs_torso_and_death() {
        let bytes = anims_bytes();
        let anims = PlayerAnims {
            speeds: &[],
            clips: Anims::parse(&bytes).unwrap(),
            script: script(),
        };
        let (vm, names) = model();
        let rig = Rig::new(&names, &["j_a".into(), "j_b".into()]);
        let ps = playerstate_iw4::PlayerState::ZERO;
        let rifle = Some(Held {
            anim_type: 4,
            class: 0,
        });
        let c = conditions(&ps, rifle, 1, 0, false);
        let mut b = Body::new(0);
        let (mut s, mut w) = (Scratch::default(), Vec::new());
        b.moves(&anims, 1, &c, 0);
        assert_eq!(b.legs.cur.map(|p| p.clip), Some(0));
        b.pose(&anims, &rig, &vm, 1000, 0.0, 0.0, false, &mut s, &mut w);
        // j_a turned 90 degrees about z: j_b (10 up its z) stays above it, and j_a's x axis is the model's y.
        assert!((w[1][1][0] - 1.0).abs() < 1e-3, "{:?}", w[1]);
        // Running: the legs fade to the run clip.
        let c = conditions(&ps, rifle, 10, 0, false);
        b.moves(&anims, 10, &c, 1000);
        assert_eq!(b.legs.cur.map(|p| p.clip), Some(1));
        assert!(b.legs.weight(1000) < 1.0 && b.legs.weight(5000) == 1.0);
        // Firing a rifle: the torso clip plays and holds; the run's `both` doesn't clear it while held.
        assert!(b.event(&anims, 2, 10, &c, 2000, false));
        assert_eq!(b.torso.cur.map(|p| p.clip), Some(2));
        b.moves(&anims, 10, &c, 2050);
        assert_eq!(b.torso.cur.map(|p| p.clip), Some(2));
        // After the fire clip (200 ms + 50) the run clears the torso.
        b.moves(&anims, 10, &c, 2400);
        assert_eq!(b.torso.cur, None);
        // A pistol's fire item is empty: nothing plays.
        let pc = conditions(
            &ps,
            Some(Held {
                anim_type: 2,
                class: 5,
            }),
            10,
            0,
            false,
        );
        assert!(!b.event(&anims, 2, 10, &pc, 3000, false));
        // Death: forced, holds the legs; movement can't replace it.
        let d = death_conditions(c, &Death::default(), 0.0);
        assert!(b.event(&anims, 1, 10, &d, 4000, true));
        b.moves(&anims, 1, &c, 4100);
        assert_eq!(b.legs.cur.map(|p| p.clip), Some(3));
    }

    /// A pose of some bones only gives those bones exactly as the full pose does (mid fade, aimed, torso turned);
    /// the bones left out are the identity.
    #[test]
    fn partial_pose_matches_the_full_pose() {
        let bytes = anims_bytes();
        let anims = PlayerAnims {
            speeds: &[],
            clips: Anims::parse(&bytes).unwrap(),
            script: script(),
        };
        let (mut vm, mut names) = model();
        names[1] = "back_low".into();
        // A fourth bone beside the spine, on the root.
        vm.bones.push(vm.bones[1]);
        names.push("j_b".into());
        names[2] = "j_a".into();
        let rig = Rig::new(&names, &["j_a".into(), "j_b".into(), "back_low".into()]);
        let ps = playerstate_iw4::PlayerState::ZERO;
        let rifle = Some(Held {
            anim_type: 4,
            class: 0,
        });
        let mut b = Body::new(0);
        b.moves(&anims, 1, &conditions(&ps, rifle, 1, 0, false), 0);
        let c = conditions(&ps, rifle, 10, 0, false);
        b.moves(&anims, 10, &c, 1000);
        assert!(b.event(&anims, 2, 10, &c, 1050, false));
        let need = needed_bones(&vm, [2]);
        assert_eq!(need, [true, true, true, false]);
        let (mut s, mut full, mut part) = (Scratch::default(), Vec::new(), Vec::new());
        for now in [1060, 1100, 1180] {
            b.pose(&anims, &rig, &vm, now, 30.0, 25.0, false, &mut s, &mut full);
            b.pose_bones(
                &anims,
                &rig,
                &vm,
                now,
                30.0,
                25.0,
                false,
                Some(&need),
                &mut s,
                &mut part,
            );
            assert_eq!(full[..3], part[..3]);
            assert_eq!(part[3], crate::viewmodel::IDENTITY);
            assert_ne!(full[3], crate::viewmodel::IDENTITY);
        }
    }

    #[test]
    fn legs_follow_the_view_after_twenty_degrees() {
        let mut b = Body::new(0);
        b.turn(0.0, false, 0);
        // 15 degrees: the torso turns, the legs stay.
        let (legs, torso) = b.turn(15.0, false, 500);
        assert_eq!(legs, 0.0);
        assert!((torso - 15.0).abs() < 1e-3);
        // 40 degrees: the legs swing over a few frames.
        let mut t = 500;
        for _ in 0..60 {
            t += 16;
            b.turn(40.0, false, t);
        }
        let (legs, _) = b.turn(40.0, false, t + 16);
        assert!((legs - 40.0).abs() < 1.0, "{legs}");
    }

    #[test]
    fn controllers_bend_the_spine_with_the_aim() {
        let (mut vm, mut names) = model();
        names[1] = "back_low".into();
        vm.bones[2].local_trans = [0.0, 0.0, 10.0];
        let rig = Rig::new(&names, &[]);
        assert_eq!(rig.controllers[0], 1);
        let bytes = wpk::player_anim::encode(&[]);
        let anims = PlayerAnims {
            speeds: &[],
            clips: Anims::parse(&bytes).unwrap(),
            script: Script::default(),
        };
        let b = Body::new(0);
        let (mut s, mut w) = (Scratch::default(), Vec::new());
        b.pose(&anims, &rig, &vm, 0, 0.0, 0.0, false, &mut s, &mut w);
        let straight = w[2];
        // Looking 45 degrees down: back_low takes 0.2 Ã— 90 = 18 degrees of pitch; the bone above leans forward
        // (+x).
        b.pose(&anims, &rig, &vm, 0, 45.0, 0.0, false, &mut s, &mut w);
        assert!(
            w[2][0][3] > straight[0][3] + 2.0,
            "{:?} vs {:?}",
            w[2],
            straight
        );
    }

    /// The run clip plays at the rate the player moves against its move speed (IW4L `client_anim_playback_rate`),
    /// and a clip with no move speed at 1.
    #[test]
    fn playback_rate_follows_the_move_speed() {
        let bytes = anims_bytes();
        // Clip 1 (run) moves at 100 units/s; the others are stationary.
        let speeds: Vec<u8> = [0.0f32, 100.0, 0.0, 0.0]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let anims = PlayerAnims {
            speeds: &speeds,
            clips: Anims::parse(&bytes).unwrap(),
            script: script(),
        };
        let c = Conditions::default();
        let mut b = Body::new(0);
        b.moves(&anims, 10, &c, 1000);
        // The first sample only anchors (IW4L: no last time, rate 1).
        b.rates(&anims, [0.0; 3], 1000);
        assert_eq!(b.legs_rate(), 1.0);
        // 200 units/s against 100: twice as fast (IW4L's cap is 2).
        b.rates(&anims, [20.0, 0.0, 0.0], 1100);
        let want = anim_iw4::client_anim_playback_rate(
            [20.0, 0.0, 0.0],
            [0.0; 3],
            1100,
            1000,
            100.0,
            false,
        );
        assert_eq!(Some(b.legs_rate()), want);
        assert!((b.legs_rate() - 2.0).abs() < 1e-4, "{}", b.legs_rate());
        // 50 units/s: half.
        b.rates(&anims, [25.0, 0.0, 0.0], 1200);
        assert!((b.legs_rate() - 0.5).abs() < 1e-4, "{}", b.legs_rate());
        // The clip's time ran 100 ms Ã— 2 then 100 ms Ã— 0.5 since it started.
        let legs = b.legs.cur.unwrap();
        assert!((legs.time(1200) - 0.25).abs() < 1e-4, "{}", legs.time(1200));
        // Standing still: IW4L's floor of 0.1.
        b.rates(&anims, [25.0, 0.0, 0.0], 1300);
        assert!((b.legs_rate() - 0.1).abs() < 1e-4, "{}", b.legs_rate());
        // Idle (no move speed) plays at 1 however fast the player moves.
        let mut b = Body::new(0);
        b.moves(&anims, 1, &c, 1000);
        b.rates(&anims, [0.0; 3], 1000);
        b.rates(&anims, [50.0, 0.0, 0.0], 1100);
        assert_eq!(b.legs_rate(), 1.0);
    }

    /// A mantle plays its clip's movement type: IW4's `MANTLE_ROOT` + the up clip, then the over clip.
    #[test]
    fn a_mantle_picks_the_mantle_movement_type() {
        let mut ps = playerstate_iw4::PlayerState::ZERO;
        assert_eq!(crate::mantle_path::anim_move_type(&ps), None);
        ps.pm_flags |= playerstate_iw4::pm_flags::MANTLE;
        ps.mantle_trans_index = 0;
        ps.mantle_flags = playerstate_iw4::mantle_flags::OVER;
        ps.mantle_timer = 10;
        let up = movement_iw4::mantle::trans_up_anim(0) as u8;
        let over = movement_iw4::mantle::trans_over_anim(0) as u8;
        assert_eq!(movetype(&ps, 0, 0), Some(21 + up));
        assert_eq!(
            anim_iw4::ANIM_MT_NAMES[usize::from(21 + up)],
            "MANTLE_UP_57"
        );
        ps.mantle_timer = 401;
        assert_eq!(movetype(&ps, 0, 0), Some(21 + over));
        assert!(anim_iw4::ANIM_MT_NAMES[usize::from(21 + over)].starts_with("MANTLE_OVER"));
    }

    #[test]
    fn event_numbers_match_iw4() {
        let n = anim_iw4::ANIM_ET_NAMES;
        assert_eq!(n[usize::from(EV_JUMP)], "JUMP");
        assert_eq!(n[usize::from(EV_JUMPBK)], "JUMPBK");
        assert_eq!(n[usize::from(EV_LAND)], "LAND");
        assert_eq!(n[usize::from(EV_CROUCH_TO_PRONE)], "CROUCH_TO_PRONE");
        assert_eq!(n[usize::from(EV_PRONE_TO_CROUCH)], "PRONE_TO_CROUCH");
    }

    #[test]
    fn jumps_landings_and_prone_play_events() {
        let ground = Motion {
            on_ground: true,
            ..Motion::default()
        };
        let events = |a: &Motion, b: &Motion| motion_events(a, b).collect::<Vec<_>>();
        assert!(events(&ground, &ground).is_empty());
        // A jump: the jump time changes and the player leaves the ground.
        let up = Motion {
            jump_time: 100,
            on_ground: false,
            vz: 260.0,
            ..ground
        };
        assert_eq!(events(&ground, &up), [EV_JUMP]);
        assert_eq!(
            events(
                &ground,
                &Motion {
                    backward: true,
                    ..up
                }
            ),
            [EV_JUMPBK]
        );
        // Walking off a ledge isn't a jump.
        let off = Motion {
            on_ground: false,
            ..ground
        };
        assert!(events(&ground, &off).is_empty());
        // Landing hard plays the landing; stepping down a little doesn't.
        let falling = Motion { vz: -300.0, ..up };
        let landed = Motion {
            jump_time: 100,
            ..ground
        };
        assert_eq!(events(&falling, &landed), [EV_LAND]);
        assert!(events(&Motion { vz: -100.0, ..up }, &landed).is_empty());
        let prone = Motion {
            prone: true,
            ..ground
        };
        assert_eq!(events(&ground, &prone), [EV_CROUCH_TO_PRONE]);
        assert_eq!(events(&prone, &ground), [EV_PRONE_TO_CROUCH]);
    }
}

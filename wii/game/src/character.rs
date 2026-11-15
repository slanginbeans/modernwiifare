//! Player characters on the Wii: each team's MW2 body and head from the map pack (`CHAR`), animated by MW2's player
//! animation script from the common pack (`PSCR`, clips `PANM` read in place, bones `PANB`): legs and torso
//! animations, firing, reloading, melee and death animations, the spine turned to the aim
//! (`game_core::character`). Skinned on the CPU like the first-person model and drawn at each player's position,
//! turned with the legs.
//!
//! Lighting is a flat ×1 for now (MW2 lights characters from the light grid).

use alloc::vec::Vec;

use game_core::arena::Fighter;
use game_core::character::{
    Body, Death, Held as HeldType, NO_BONE, PlayerAnims, Rig, Scratch, conditions,
    death_conditions, movetype, strafing,
};
use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::viewmodel as vmod;
use wpk::anim_script::{Conditions, Script};
use wpk::held::Guns;
use wpk::player_anim::Anims;
use wpk::records::{self, NONE, TextureRecord, texture_flags};
use wpk::view::ViewModel;

use crate::ffi;

/// GX vertex format of the display lists (POS, CLR0, TEX0 indexed), as the first-person model's.
const VTXFMT: u8 = 6;

/// IW4 animation events (`anim_iw4::ANIM_ET_*`).
const EV_DEATH: u8 = anim_iw4::ANIM_ET_DEATH;
const EV_FIRE: u8 = anim_iw4::ANIM_ET_FIREWEAPON;
const EV_RELOAD: u8 = anim_iw4::ANIM_ET_RELOAD;
const EV_MELEE: u8 = anim_iw4::ANIM_ET_MELEEATTACK;
const EV_KNIFE: u8 = anim_iw4::ANIM_ET_KNIFE_MELEE;

/// IW4 weapon states (`weapon_iw4::WeaponState`) that start an animation event.
const WS_RELOADING: i32 = 0x8;
const WS_RELOAD_START: i32 = 0xA;
const WS_MELEE_INIT: i32 = 0xD;
const WS_OFFHAND_START: i32 = 0x13;

/// A thrown grenade's player-anim type and class (IW4 `grenade`): the throw plays as its fire event.
const THROWN: HeldType = HeldType {
    anim_type: 9,
    class: 6,
};

struct Draw {
    dl: &'static [u8],
    state: DrawState,
}

struct Team {
    vm: ViewModel,
    rig: Rig,
    attributes: *const u8,
    draws: Vec<Draw>,
}

/// What the animation saw of one player last frame.
#[derive(Clone, Copy, Default)]
struct Seen {
    body: Body,
    alive: bool,
    last_fire_ms: Option<u32>,
    gun_state: i32,
    movetype: u8,
}

/// The weapons in the players' hands (`common.wpk` `GUNM`): the models, each weapon record's model and the texture
/// handles.
struct Held {
    guns: Guns<'static>,
    index: &'static [u8],
    textures: Vec<i32>,
}

impl Held {
    fn load(common: &wpk::Pack<'static>, gfx: &mut impl Gfx) -> Option<Held> {
        let guns = Guns::parse(common.section(wpk::tag::HELD_MODELS)?.data)?;
        let index = common.section(wpk::tag::HELD_INDEX)?.data;
        let txda = common.section(wpk::tag::HELD_TEXTURE_DATA)?.data;
        let mut textures = Vec::new();
        for r in records::iter::<TextureRecord>(common.section(wpk::tag::HELD_TEXTURES)?.data) {
            let Some(data) = txda.get(r.data_off as usize..(r.data_off + r.data_len) as usize)
            else {
                textures.push(-1);
                continue;
            };
            let alpha = if r.flags & texture_flags::ALPHA_PLANE != 0 {
                txda.get(r.alpha_off as usize..(r.alpha_off + r.alpha_len) as usize)
                    .map_or(core::ptr::null(), |a| a.as_ptr())
            } else {
                core::ptr::null()
            };
            textures.push(gfx.tex_create(&TexDesc {
                data: data.as_ptr(),
                alpha,
                width: r.width,
                height: r.height,
                format: r.format,
                levels: r.levels,
                wrap: r.wrap,
            }));
        }
        Some(Held {
            guns,
            index,
            textures,
        })
    }

    /// The model weapon record `weapon` is held as.
    fn model(&self, weapon: u32) -> Option<wpk::held::GunRef<'static>> {
        let at = weapon as usize * 2;
        let m = u16::from_be_bytes([*self.index.get(at)?, *self.index.get(at + 1)?]);
        (m != NONE)
            .then(|| self.guns.model(usize::from(m)))
            .flatten()
    }
}

/// The drawn players.
pub struct Characters {
    teams: Vec<Team>,
    held: Option<Held>,
    anims: PlayerAnims<'static>,
    seen: Vec<Seen>,
    /// Skinned positions per drawn player (MEM2; the GPU reads them after the CPU moved on, so each player has its
    /// own, and two of them: frames are pipelined, so the GPU may still be drawing last frame's).
    buffers: Vec<(usize, &'static mut [[f32; 3]])>,
    /// Which of each player's two buffers this frame writes.
    parity: usize,
    scratch: Scratch,
    world: Vec<Mtx34>,
    skin: Vec<Mtx34>,
}

fn mem2(count: usize) -> Option<&'static mut [u8]> {
    let p = unsafe { ffi::mwf_mem2_alloc(count as u32, 32) };
    (!p.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(p, count) })
}

/// What one player looks like and does this frame.
pub struct Pose {
    /// The fighter's slot (its animation state).
    pub slot: usize,
    /// 0 allies (or free-for-all), 1 axis.
    pub team: usize,
    pub origin: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub prone: bool,
    pub alive: bool,
    pub death: Death,
    /// The script's movement type (`None` in the air) and conditions.
    pub movetype: Option<u8>,
    pub conditions: Conditions,
    pub moving: bool,
    pub last_fire_ms: Option<u32>,
    pub gun_state: i32,
    /// The weapon in the hand (`WEAP` record).
    pub weapon: Option<u32>,
}

impl Pose {
    /// Fighter `slot` of the arena at `now`, on team model `team`.
    pub fn of(f: &Fighter, slot: usize, team: usize, now: u32) -> Pose {
        let ps = &f.player.ps;
        let (forward, right) = f.player.cmd_move;
        let mt = movetype(ps, forward, right);
        let held = f.gun.as_ref().map(|g| {
            let facts = g.facts();
            HeldType {
                anim_type: facts.player_anim_type.clamp(0, 63) as u8,
                class: facts.weap_class.clamp(0, 63) as u8,
            }
        });
        let firing = f.last_fire_ms.is_some_and(|t| now.wrapping_sub(t) < 150);
        let (pitch, yaw) = f.player.view();
        let speed = libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]);
        Pose {
            slot,
            team,
            origin: f.draw_origin(now),
            pitch,
            yaw,
            prone: f.player.prone(),
            alive: f.life.alive(),
            death: f.death,
            movetype: mt,
            conditions: conditions(
                ps,
                held,
                mt.unwrap_or(anim_iw4::ANIM_MT_IDLE),
                strafing(ps, forward, right),
                firing,
            ),
            moving: speed > 10.0,
            last_fire_ms: f.last_fire_ms,
            gun_state: f.gun.as_ref().map_or(0, |g| g.state()),
            weapon: f.gun.as_ref().map(|g| g.weapon_index()),
        }
    }
}

impl Characters {
    /// The map's characters (`map`'s `CHAR`) with the common pack's player animations and held weapons (their
    /// textures created through `gfx`); `texture` turns a map texture index into its handle. `None` without
    /// characters or animations.
    pub fn load(
        map: &'static [u8],
        common: &'static [u8],
        texture: impl Fn(u16) -> i32,
        gfx: &mut impl Gfx,
    ) -> Option<Characters> {
        let pack = wpk::Pack::parse(map, false).ok()?;
        let teams = wpk::view::decode_characters(pack.section(wpk::tag::CHARACTERS)?.data);
        let common = wpk::Pack::parse(common, false).ok()?;
        let held = Held::load(&common, gfx);
        let clips = Anims::parse(common.section(wpk::tag::PLAYER_ANIMS)?.data)?;
        let script = common
            .section(wpk::tag::PLAYER_ANIM_SCRIPT)
            .and_then(|s| Script::decode(s.data))?;
        let anim_names = common
            .section(wpk::tag::PLAYER_ANIM_BONES)
            .and_then(|s| wpk::view::decode_names(s.data))
            .unwrap_or_default();
        let mut out = Vec::new();
        for (vm, names) in teams {
            let rig = Rig::new(&names, &anim_names);
            let attr = mem2(vm.verts.len() * 12)?;
            for (i, v) in vm.verts.iter().enumerate() {
                let at = i * 12;
                attr[at..at + 4].copy_from_slice(&v.rgba.to_be_bytes());
                attr[at + 4..at + 8].copy_from_slice(&v.uv[0].to_be_bytes());
                attr[at + 8..at + 12].copy_from_slice(&v.uv[1].to_be_bytes());
            }
            unsafe { ffi::mwf_flush(attr.as_ptr(), attr.len() as u32) };
            let mut draws = Vec::with_capacity(vm.surfaces.len());
            for s in &vm.surfaces {
                let idx =
                    &vm.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
                if idx.is_empty() {
                    continue;
                }
                let len = (3 + idx.len() * 6).div_ceil(32) * 32;
                let dl = mem2(len)?;
                dl.fill(0);
                dl[0] = records::gx_triangles(VTXFMT);
                dl[1..3].copy_from_slice(&(idx.len() as u16).to_be_bytes());
                for (k, &i) in idx.iter().enumerate() {
                    for a in 0..3 {
                        let at = 3 + k * 6 + a * 2;
                        dl[at..at + 2].copy_from_slice(&i.to_be_bytes());
                    }
                }
                unsafe { ffi::mwf_flush(dl.as_ptr(), len as u32) };
                draws.push(Draw {
                    dl,
                    state: DrawState {
                        tex: if s.texture == NONE {
                            -1
                        } else {
                            texture(s.texture)
                        },
                        lightmap: -1,
                        blend: s.blend,
                        alpha_ref: s.alpha_ref,
                        cull: 0,
                        tint: 0,
                    },
                });
            }
            out.push(Team {
                attributes: attr.as_ptr(),
                draws,
                rig,
                vm,
            });
        }
        (!out.is_empty()).then(|| Characters {
            teams: out,
            held,
            anims: PlayerAnims { clips, script },
            seen: Vec::new(),
            buffers: Vec::new(),
            parity: 0,
            scratch: Scratch::default(),
            world: Vec::new(),
            skin: Vec::new(),
        })
    }

    /// Run each player's animation script for this frame: its movement, and the events since the last frame.
    fn animate(&mut self, p: &Pose, now: u32) -> (f32, f32) {
        while self.seen.len() <= p.slot {
            let n = self.seen.len() as u32;
            self.seen.push(Seen {
                body: Body::new(n),
                ..Seen::default()
            });
        }
        let anims = &self.anims;
        let s = &mut self.seen[p.slot];
        let mt = p.movetype.unwrap_or(s.movetype);
        if p.alive && !s.alive {
            s.body.reset(p.yaw);
            s.last_fire_ms = p.last_fire_ms;
            s.gun_state = p.gun_state;
        }
        if !p.alive && s.alive {
            let c = death_conditions(p.conditions, &p.death, p.yaw);
            s.body.event(anims, EV_DEATH, mt, &c, now, true);
        }
        s.alive = p.alive;
        if p.alive {
            if p.last_fire_ms != s.last_fire_ms && p.last_fire_ms.is_some() {
                s.body.event(anims, EV_FIRE, mt, &p.conditions, now, false);
            }
            let entered = |state: i32| p.gun_state == state && s.gun_state != state;
            if (entered(WS_RELOADING) && s.gun_state != WS_RELOAD_START) || entered(WS_RELOAD_START)
            {
                s.body
                    .event(anims, EV_RELOAD, mt, &p.conditions, now, false);
            }
            if entered(WS_MELEE_INIT)
                && !s.body.event(anims, EV_KNIFE, mt, &p.conditions, now, false)
            {
                s.body.event(anims, EV_MELEE, mt, &p.conditions, now, false);
            }
            if entered(WS_OFFHAND_START) {
                let mut c = p.conditions;
                c.value[usize::from(anim_iw4::ANIM_COND_PLAYERANIMTYPE)] = 1 << THROWN.anim_type;
                c.value[usize::from(anim_iw4::ANIM_COND_WEAPONCLASS)] = 1 << THROWN.class;
                s.body.event(anims, EV_FIRE, mt, &c, now, false);
            }
            if p.movetype.is_some() {
                s.body.moves(anims, mt, &p.conditions, now);
            }
            s.last_fire_ms = p.last_fire_ms;
            s.gun_state = p.gun_state;
            s.movetype = mt;
            s.body.turn(p.yaw, p.moving, now)
        } else {
            let (legs, _) = s.body.turn(p.yaw, false, now);
            (legs, legs)
        }
    }

    /// Animate and draw `players` (after the world, before the first-person model) seen through `view` (world to eye)
    /// at `now_ms`.
    pub fn draw(&mut self, gfx: &mut impl Gfx, view: &Mtx34, players: &[Pose], now_ms: u32) {
        self.parity ^= 1;
        for (k, p) in players.iter().enumerate() {
            let (legs_yaw, torso_yaw) = self.animate(p, now_ms);
            // This player's buffer for this frame (two per drawn player, alternating).
            let k = k * 2 + self.parity;
            // Behind the camera (the eye looks down -z): not posed or drawn.
            let o = p.origin;
            if vmod::apply(view, [o[0], o[1], o[2] + 36.0])[2] > 48.0 {
                continue;
            }
            let t = p.team.min(self.teams.len() - 1);
            let n = self.teams[t].vm.verts.len();
            while self.buffers.len() <= k {
                let Some(b) = mem2(n * 12) else {
                    return;
                };
                let s = unsafe {
                    core::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<[f32; 3]>(), n)
                };
                self.buffers.push((t, s));
            }
            if self.buffers[k].0 != t || self.buffers[k].1.len() != n {
                let Some(b) = mem2(n * 12) else {
                    return;
                };
                self.buffers[k] = (t, unsafe {
                    core::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<[f32; 3]>(), n)
                });
            }
            let team = &self.teams[t];
            let body = &self.seen[p.slot].body;
            let mut rel = torso_yaw - legs_yaw;
            if rel > 180.0 {
                rel -= 360.0;
            } else if rel < -180.0 {
                rel += 360.0;
            }
            let pitch = if p.alive { p.pitch } else { 0.0 };
            body.pose(
                &self.anims,
                &team.rig,
                &team.vm,
                now_ms,
                pitch,
                rel,
                p.prone,
                &mut self.scratch,
                &mut self.world,
            );
            vmod::skin_matrices(&team.vm, &self.world, &mut self.skin);
            let buf = &mut self.buffers[k].1;
            vmod::skin_into(&team.vm, &self.skin, buf);
            // Model (IW4 axes: x forward, z up) to world: turned with the legs, at the feet.
            let (s, c) = (
                libm::sinf(legs_yaw.to_radians()),
                libm::cosf(legs_yaw.to_radians()),
            );
            let model: Mtx34 = [[c, -s, 0.0, o[0]], [s, c, 0.0, o[1]], [0.0, 0.0, 1.0, o[2]]];
            unsafe {
                ffi::mwf_flush(buf.as_ptr().cast(), (n * 12) as u32);
                ffi::mwf_world_view();
                ffi::mwf_view_arrays(buf.as_ptr().cast(), team.attributes);
            }
            gfx.pos_matrix(&vmod::mul(view, &model));
            let mut last: Option<DrawState> = None;
            for d in &team.draws {
                if last != Some(d.state) {
                    gfx.world_state(&d.state);
                    last = Some(d.state);
                }
                gfx.call_dl(d.dl);
            }
            // The weapon in the hand: its model's origin on `tag_weapon_right` (living players only: MW2 drops it).
            let hand = team.rig.weapon;
            if let (Some(held), Some(w), true) = (&self.held, p.weapon, p.alive && hand != NO_BONE)
                && let Some(gun) = held.model(w)
                && let Some(bone) = self.world.get(usize::from(hand))
            {
                unsafe {
                    ffi::mwf_world_view();
                    ffi::mwf_view_arrays(gun.positions.as_ptr(), gun.attributes.as_ptr());
                }
                gfx.pos_matrix(&vmod::mul(&vmod::mul(view, &model), bone));
                let mut last: Option<DrawState> = None;
                for i in 0..gun.draws() {
                    let Some(d) = gun.draw(i) else { continue };
                    let state = DrawState {
                        tex: held
                            .textures
                            .get(usize::from(d.texture))
                            .copied()
                            .unwrap_or(-1),
                        lightmap: -1,
                        blend: d.blend,
                        alpha_ref: d.alpha_ref,
                        cull: 0,
                        tint: 0,
                    };
                    if last != Some(state) {
                        gfx.world_state(&state);
                        last = Some(state);
                    }
                    gfx.call_dl(d.dl);
                }
            }
        }
    }
}

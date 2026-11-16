//! Player characters on the Wii: each team's MW2 body and head from the map pack (`CHAR`), animated by MW2's player
//! animation script from the common pack (`PSCR`, clips `PANM` read in place, bones `PANB`): legs and torso
//! animations, firing, reloading, melee and death animations, the spine turned to the aim
//! (`game_core::character`). Skinned on the CPU like the first-person model and drawn at each player's position,
//! turned with the legs.
//!
//! Lit by the map's light lattice at each character's middle (`game_core::light`, [`Characters::lattice`]), the
//! weapon in the hand too.

use alloc::vec::Vec;

use game_core::arena::Fighter;
use game_core::character::{
    Body, Death, Held as HeldType, Motion, NO_BONE, PlayerAnims, Rig, Scratch, conditions,
    death_conditions, motion_events, movetype, strafing,
};
use game_core::damage::HitBox;
use game_core::gfx::{DrawState, Gfx, Mtx34};
use game_core::skin_kernels::SkinVert;
use game_core::viewmodel as vmod;
use wpk::anim_script::{Conditions, Script};
use wpk::held::Guns;
use wpk::player_anim::Anims;
use wpk::records::{self, NONE};
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
/// `SHELLSHOCK` and `STUNNED` (`anim_iw4::ANIM_ET_NAMES` 20 and 21): a flashbang's and a stun grenade's torso
/// animation.
const EV_SHELLSHOCK: u8 = 20;
const EV_STUNNED: u8 = 21;

/// Bodies kept after their players respawn (the death pose held, the oldest reused first).
pub const CORPSES: usize = 4;

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
    /// The matrix of each GX position-matrix slot this draw's vertices use (`vmod::matrix_key`: a bone, or
    /// `vmod::BLENDED` for the vertices the CPU skins).
    slots: Vec<u16>,
}

struct Team {
    /// Its vertices reordered at load: those on more than one bone first (`blended`), then the rigid ones.
    vm: ViewModel,
    rig: Rig,
    attributes: *const u8,
    draws: Vec<Draw>,
    /// The vertices the CPU skins (the first `blended.len()`); GX places the rest with their bone's matrix.
    blended: Vec<SkinVert>,
    /// Each bone's inverse bind matrix, contiguous for `ps::concat_each`.
    inv_bind: Vec<Mtx34>,
    /// Its bones' hit boxes (`CHHB`; none with an older pack).
    hitboxes: Vec<records::BoneHitRecord>,
    /// The bones a body nobody sees is still posed for: its hit boxes', the weapon and shield tags, and their
    /// ancestors (`character::needed_bones`).
    hit_need: Vec<bool>,
}

/// What the animation saw of one player last frame.
#[derive(Clone, Copy, Default)]
struct Seen {
    body: Body,
    alive: bool,
    last_fire_ms: Option<u32>,
    gun_state: i32,
    movetype: u8,
    motion: Motion,
    /// Where its muzzle was last drawn (world origin and forward).
    flash: Option<([f32; 3], [f32; 3])>,
    /// The start of the shock whose animation played.
    shock_ms: Option<u32>,
    /// While dead: where the body lies (feet, legs yaw) and its team model, for the corpse at the respawn.
    dead_at: Option<([f32; 3], f32, usize)>,
}

/// A body left after its player respawned: skinned once, in its death pose, into its own buffer.
struct Corpse {
    team: usize,
    model: Mtx34,
    buf: &'static mut [[f32; 3]],
    tint: u32,
    born_ms: u32,
    used: bool,
}

/// The weapons in the players' hands (`common.wpk` `GUNM`): the models, each weapon record's model and the texture
/// handles.
struct Held {
    guns: Guns<'static>,
    index: &'static [u8],
    /// Per weapon record, the draws its model leaves out (`GUNH`: attachment parts that variant hides).
    parts: Option<wpk::parts::Parts<'static>>,
    textures: Vec<i32>,
}

impl Held {
    fn load(common: &wpk::Pack<'static>, gfx: &mut impl Gfx) -> Option<Held> {
        Some(Held {
            guns: Guns::parse(common.section(wpk::tag::HELD_MODELS)?.data)?,
            index: common.section(wpk::tag::HELD_INDEX)?.data,
            parts: common
                .section(wpk::tag::HELD_PARTS)
                .and_then(|s| wpk::parts::Parts::parse(s.data)),
            textures: crate::effects::create_textures(
                common,
                wpk::tag::HELD_TEXTURES,
                wpk::tag::HELD_TEXTURE_DATA,
                gfx,
            ),
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

    /// The draws of its model weapon record `weapon` leaves out (attachments it doesn't carry).
    fn hidden(&self, weapon: u32) -> wpk::parts::Hidden<'static> {
        self.parts
            .map_or(wpk::parts::Hidden::NONE, |p| p.hidden(weapon))
    }
}

/// The drawn players.
pub struct Characters {
    /// The map's light lattice (none: unlit, ×1).
    pub lattice: Option<wpk::light::LightLattice<'static>>,
    teams: Vec<Team>,
    held: Option<Held>,
    anims: PlayerAnims<'static>,
    seen: Vec<Seen>,
    /// Positions per drawn player, by its place in the list drawn (not its slot: the killcam's replayed bodies
    /// animate in slots of their own), MEM2 taken at load, two each: frames are pipelined, so the GPU may still be
    /// drawing last frame's. Each holds a team model's rigid vertices in model space (written when the team
    /// changes) and the blended ones skinned each frame it is drawn. The team it holds, or `usize::MAX`.
    buffers: Vec<[Positions; 2]>,
    /// The view this frame (none: everything in front of the camera is drawn). Bodies outside it are posed for
    /// their hit boxes but neither skinned nor drawn.
    pub frustum: Option<game_core::camera::Frustum>,
    /// Which of each player's two buffers this frame writes.
    parity: usize,
    scratch: Scratch,
    world: Vec<Mtx34>,
    skin: Vec<Mtx34>,
    /// Per slot: when its body was last posed and its bones' hit boxes then (world space).
    hits: Vec<(u32, Vec<HitBox>)>,
    /// The riot shield's weapon record (its `GUNM` model goes on the back when carried as the other weapon).
    shield_weapon: Option<u32>,
    corpses: Vec<Corpse>,
    /// Shell shock / stun torso animations played (for the log).
    pub shock_anims: u32,
    /// Corpse draws so far (for the log).
    pub corpse_draws: u32,
    /// Where the body time went since the log last took it ([`Characters::take_cost`]).
    cost: BodyCost,
}

/// What drawing the bodies cost (the match log's `bodcost`, every 5 s): microseconds animating and posing (with the
/// hit boxes), skinning, and submitting the draws (guns and corpses too); bodies posed, drawn, and posed but not drawn
/// (outside the view or past the far plane); frames.
#[derive(Clone, Copy, Default, Debug)]
pub struct BodyCost {
    pub pose_us: u32,
    pub skin_us: u32,
    pub submit_us: u32,
    pub posed: u32,
    pub drawn: u32,
    pub culled: u32,
    pub frames: u32,
}

fn mem2(count: usize) -> Option<&'static mut [u8]> {
    let p = unsafe { ffi::mwf_mem2_alloc(count as u32, 32) };
    (!p.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(p, count) })
}

/// A player's position buffer and the team model whose rigid vertices it holds (`usize::MAX`: none yet).
type Positions = (usize, &'static mut [[f32; 3]]);

/// `n` positions in MEM2.
fn positions(n: usize) -> Option<&'static mut [[f32; 3]]> {
    let b = mem2(n * 12)?;
    Some(unsafe { core::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<[f32; 3]>(), n) })
}

/// What one player looks like and does this frame.
#[derive(Clone, Copy)]
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
    /// For jumps, landings and going prone.
    pub motion: Motion,
    /// The shell shock under way: its start and whether it is a flashbang's (else a stun grenade's).
    pub shock: Option<(u32, bool)>,
    /// Where the riot shield is carried.
    pub shield: game_core::shield::Carry,
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
            motion: Motion::of(ps, forward),
            shock: f
                .shock
                .filter(|s| s.remaining(now) > 0)
                .map(|s| (s.start_ms, s.flash)),
            shield: f.shield,
        }
    }
}

impl Characters {
    /// The map's characters (`map`'s `CHAR`) with the common pack's player animations and held weapons (their
    /// textures created through `gfx`); `texture` turns a map texture index into its handle. `None` without
    /// characters or animations. `slots`: the match's players (you and the bots): position buffers for that many
    /// bodies drawn at once are taken now; a body past them takes its own when first drawn.
    pub fn load(
        map: &'static [u8],
        common: &'static [u8],
        slots: usize,
        texture: impl Fn(u16) -> i32,
        gfx: &mut impl Gfx,
    ) -> Option<Characters> {
        let pack = wpk::Pack::parse_prefix(map).ok()?;
        let teams = wpk::view::decode_characters(pack.section(wpk::tag::CHARACTERS)?.data);
        let hitboxes: Vec<records::BoneHitRecord> = pack
            .section(wpk::tag::CHARACTER_HITBOXES)
            .map(|s| records::iter(s.data).collect())
            .unwrap_or_default();
        let common = wpk::Pack::parse(common, false).ok()?;
        let held = Held::load(&common, gfx);
        let clips = Anims::parse(common.section(wpk::tag::PLAYER_ANIMS)?.data)?;
        let script = common
            .section(wpk::tag::PLAYER_ANIM_SCRIPT)
            .and_then(|s| Script::decode(s.data))?;
        let speeds = common
            .section(wpk::tag::PLAYER_ANIM_SPEEDS)
            .map_or(&[][..], |s| s.data);
        let shield_weapon = common.section(wpk::tag::WEAPONS).and_then(|s| {
            records::iter::<wpk::weapon::WeaponRecord>(s.data)
                .position(|w| game_core::shield::is_shield(w.name(), w.combat.weap_type))
                .map(|i| i as u32)
        });
        let anim_names = common
            .section(wpk::tag::PLAYER_ANIM_BONES)
            .and_then(|s| wpk::view::decode_names(s.data))
            .unwrap_or_default();
        let mut out = Vec::new();
        for (mut vm, names) in teams {
            let rig = Rig::new(&names, &anim_names);
            // The vertices on more than one bone first: the CPU skins and flushes that front part only.
            let n = vm.verts.len().min(usize::from(u16::MAX));
            let rigid = |v: &wpk::view::SkinVertex| vmod::matrix_key(v) != vmod::BLENDED;
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&i| rigid(&vm.verts[i]));
            let mut remap = alloc::vec![0u16; n];
            for (new, &old) in order.iter().enumerate() {
                remap[old] = new as u16;
            }
            vm.verts = order.iter().map(|&i| vm.verts[i]).collect();
            for i in vm.indices.iter_mut() {
                *i = remap.get(usize::from(*i)).copied().unwrap_or(0);
            }
            let blended: Vec<SkinVert> = vm
                .verts
                .iter()
                .take_while(|v| !rigid(v))
                .enumerate()
                .map(|(i, v)| SkinVert::of(v, i as u16))
                .collect();
            let inv_bind: Vec<Mtx34> = vm.bones.iter().map(|b| b.inv_bind).collect();
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
                // Cut into groups of at most ten matrices (as the first-person model): GX_TRIANGLES, each vertex a
                // position-matrix index (slot x 3) then three u16 indices.
                for (slots, tris) in vmod::matrix_groups(&vm, idx) {
                    const VERTEX: usize = 7;
                    let len = (3 + tris.len() * VERTEX).div_ceil(32) * 32;
                    let dl = mem2(len)?;
                    dl.fill(0);
                    dl[0] = records::gx_triangles(VTXFMT);
                    dl[1..3].copy_from_slice(&(tris.len() as u16).to_be_bytes());
                    for (k, &i) in tris.iter().enumerate() {
                        let at = 3 + k * VERTEX;
                        let key = vmod::matrix_key(&vm.verts[usize::from(i)]);
                        let slot = slots.iter().position(|&s| s == key).unwrap_or(0);
                        dl[at] = (slot * 3) as u8;
                        for a in 0..3 {
                            dl[at + 1 + a * 2..at + 3 + a * 2].copy_from_slice(&i.to_be_bytes());
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
                        slots,
                    });
                }
            }
            let team = out.len() as u8;
            let hitboxes: Vec<records::BoneHitRecord> = hitboxes
                .iter()
                // No location ("none", the head model's own bones in packs from before the converter left
                // them out): not a hit volume.
                .filter(|h| h.team == team && h.loc != 0 && usize::from(h.bone) < vm.bones.len())
                .copied()
                .collect();
            let hit_need = game_core::character::needed_bones(
                &vm,
                hitboxes
                    .iter()
                    .map(|h| usize::from(h.bone))
                    .chain([rig.weapon, rig.weapon_left, rig.shield_back].map(usize::from)),
            );
            out.push(Team {
                attributes: attr.as_ptr(),
                draws,
                blended,
                inv_bind,
                rig,
                hitboxes,
                hit_need,
                vm,
            });
        }
        // The corpse pool's buffers now, at load: MEM2 taken later in the match may sit above the weapon files'
        // mark (`Arsenal::new`) and be handed out again when they stream.
        let most = out.iter().map(|t| t.vm.verts.len()).max().unwrap_or(0);
        let mut corpses = Vec::with_capacity(CORPSES);
        for _ in 0..CORPSES {
            let Some(b) = mem2(most * 12) else {
                break;
            };
            corpses.push(Corpse {
                team: 0,
                model: [[0.0; 4]; 3],
                buf: unsafe {
                    core::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<[f32; 3]>(), most)
                },
                tint: 0,
                born_ms: 0,
                used: false,
            });
        }
        // The players' position buffers now too (same reason).
        let mut buffers = Vec::with_capacity(slots);
        for _ in 0..slots {
            let (Some(a), Some(b)) = (positions(most), positions(most)) else {
                break;
            };
            buffers.push([(usize::MAX, a), (usize::MAX, b)]);
        }
        (!out.is_empty()).then(|| Characters {
            lattice: None,
            teams: out,
            held,
            anims: PlayerAnims {
                clips,
                script,
                speeds,
            },
            seen: Vec::new(),
            buffers,
            frustum: None,
            parity: 0,
            scratch: Scratch::default(),
            world: Vec::new(),
            skin: Vec::new(),
            hits: Vec::new(),
            shield_weapon,
            corpses,
            shock_anims: 0,
            corpse_draws: 0,
            cost: BodyCost::default(),
        })
    }

    /// For the load log: the largest team model's vertex count, whether every team model has the riot shield's
    /// tags (left hand, back), and whether the shield has a model.
    pub fn summary(&self) -> (usize, bool, bool, bool) {
        let verts = self
            .teams
            .iter()
            .map(|t| t.vm.verts.len())
            .max()
            .unwrap_or(0);
        let left = self.teams.iter().all(|t| t.rig.weapon_left != NO_BONE);
        let back = self.teams.iter().all(|t| t.rig.shield_back != NO_BONE);
        let shield = self
            .shield_weapon
            .zip(self.held.as_ref())
            .is_some_and(|(w, h)| h.model(w).is_some());
        (verts, left, back, shield)
    }

    /// The position buffer of the `k`th body drawn, for this frame, holding team model `t`'s rigid vertices
    /// (written and flushed when it held another team's). None when MEM2 is short.
    fn buffer(&mut self, k: usize, t: usize) -> Option<&'static mut [[f32; 3]]> {
        while self.buffers.len() <= k {
            let most = self.teams.iter().map(|t| t.vm.verts.len()).max()?;
            let (a, b) = (positions(most)?, positions(most)?);
            self.buffers.push([(usize::MAX, a), (usize::MAX, b)]);
        }
        let verts = &self.teams[t].vm.verts;
        let (held, buf) = &mut self.buffers[k][self.parity];
        if *held != t {
            for (o, v) in buf.iter_mut().zip(verts) {
                *o = v.pos;
            }
            unsafe { ffi::mwf_flush(buf.as_ptr().cast(), (buf.len() * 12) as u32) };
            *held = t;
        }
        let n = verts.len().min(buf.len());
        // SAFETY: the buffer lives for the program (MEM2, never freed); this frame's drawing is its only user.
        Some(unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr(), n) })
    }

    /// The paired-single kernels against the scalar ones on team 0's model, turned away from its bind pose
    /// (`ps::self_check`): the largest difference in a skin matrix entry and in a skinned position.
    pub fn kernel_check(&self) -> (f32, f32) {
        let Some(t) = self.teams.first() else {
            return (0.0, 0.0);
        };
        let turn = vmod::mtx([0.0, 0.2, 0.3, 0.93], [4.0, -2.0, 1.0]);
        let world: Vec<Mtx34> = t
            .inv_bind
            .iter()
            .map(|b| vmod::mul(&turn, &vmod::inverse_rigid(b)))
            .collect();
        crate::ps::self_check(&world, &t.inv_bind, &t.blended)
    }

    /// The body cost since the last call (for the log).
    pub fn take_cost(&mut self) -> BodyCost {
        core::mem::take(&mut self.cost)
    }

    /// For the load log, per team model: the vertices the CPU skins, all vertices, display lists, and bones posed
    /// for a body nobody sees of all bones; and the position buffers' MEM2 (KiB).
    #[allow(clippy::type_complexity)]
    pub fn skin_summary(
        &self,
    ) -> (
        Vec<usize>,
        Vec<usize>,
        Vec<usize>,
        Vec<(usize, usize)>,
        usize,
    ) {
        (
            self.teams.iter().map(|t| t.blended.len()).collect(),
            self.teams.iter().map(|t| t.vm.verts.len()).collect(),
            self.teams.iter().map(|t| t.draws.len()).collect(),
            self.teams
                .iter()
                .map(|t| (t.hit_need.iter().filter(|&&b| b).count(), t.hit_need.len()))
                .collect(),
            self.buffers
                .iter()
                .map(|b| (b[0].1.len() + b[1].1.len()) * 12)
                .sum::<usize>()
                / 1024,
        )
    }

    /// Bodies lying after a respawn now.
    pub fn corpse_count(&self) -> usize {
        self.corpses.iter().filter(|c| c.used).count()
    }

    /// Leave player `slot`'s body where it died, in its death pose at `now` (the oldest body makes room).
    fn leave_corpse(&mut self, slot: usize, now: u32) {
        let Some((o, yaw, t)) = self.seen.get(slot).and_then(|s| s.dead_at) else {
            return;
        };
        let t = t.min(self.teams.len() - 1);
        let n = self.teams[t].vm.verts.len();
        if self.corpses.is_empty() {
            return;
        }
        let i = match self.corpses.iter().position(|c| !c.used) {
            Some(i) => i,
            None => (0..self.corpses.len())
                .max_by_key(|&i| now.wrapping_sub(self.corpses[i].born_ms))
                .unwrap_or(0),
        };
        let team = &self.teams[t];
        self.seen[slot].body.pose(
            &self.anims,
            &team.rig,
            &team.vm,
            now,
            0.0,
            0.0,
            false,
            &mut self.scratch,
            &mut self.world,
        );
        vmod::skin_matrices(&team.vm, &self.world, &mut self.skin);
        let c = &mut self.corpses[i];
        vmod::skin_into(&team.vm, &self.skin, &mut c.buf[..n]);
        unsafe { ffi::mwf_flush(c.buf.as_ptr().cast(), (n * 12) as u32) };
        let (s, co) = (libm::sinf(yaw.to_radians()), libm::cosf(yaw.to_radians()));
        c.model = [
            [co, -s, 0.0, o[0]],
            [s, co, 0.0, o[1]],
            [0.0, 0.0, 1.0, o[2]],
        ];
        c.team = t;
        c.tint = game_core::light::tint_at(
            self.lattice.as_ref(),
            [o[0], o[1], o[2] + game_core::light::CHARACTER_LIGHT_HEIGHT],
        );
        c.born_ms = now;
        c.used = true;
        crate::devlog::event(
            now,
            "corpse",
            format_args!(
                "slot={slot} at={:.0},{:.0},{:.0} pool={i}",
                o[0], o[1], o[2]
            ),
        );
    }

    /// The bodies left after respawns, seen through `view`.
    fn draw_corpses(&mut self, gfx: &mut impl Gfx, view: &Mtx34) {
        for c in self.corpses.iter().filter(|c| c.used) {
            let o = [c.model[0][3], c.model[1][3], c.model[2][3]];
            if vmod::apply(view, [o[0], o[1], o[2] + 16.0])[2] > 64.0 {
                continue;
            }
            let team = &self.teams[c.team.min(self.teams.len() - 1)];
            self.corpse_draws = self.corpse_draws.wrapping_add(1);
            // Skinned whole when it fell: every slot of the indexed lists takes the model-to-eye matrix.
            unsafe {
                ffi::mwf_world_view();
                ffi::mwf_view_arrays_indexed(c.buf.as_ptr().cast(), team.attributes);
            }
            let eye = vmod::mul(view, &c.model);
            let mut loaded = crate::ps::UNLOADED;
            let mut last: Option<DrawState> = None;
            for d in &team.draws {
                crate::ps::load_slots(&eye, &[], &d.slots, &mut loaded);
                let state = DrawState {
                    tint: c.tint,
                    ..d.state
                };
                if last != Some(state) {
                    gfx.world_state(&state);
                    last = Some(state);
                }
                gfx.call_dl(d.dl);
            }
        }
    }

    /// Draw `gun` (a `GUNM` model) at `at` (model to world) seen through `view`.
    fn draw_gun(
        &self,
        gfx: &mut impl Gfx,
        gun: &wpk::held::GunRef<'static>,
        weapon: u32,
        view: &Mtx34,
        at: &Mtx34,
        tint: u32,
    ) {
        let Some(held) = self.held.as_ref() else {
            return;
        };
        unsafe {
            ffi::mwf_world_view();
            ffi::mwf_view_arrays(gun.positions.as_ptr(), gun.attributes.as_ptr());
        }
        gfx.pos_matrix(&vmod::mul(view, at));
        let mut last: Option<DrawState> = None;
        let hidden = held.hidden(weapon);
        for i in (0..gun.draws()).filter(|&i| !hidden.contains(i)) {
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
                tint,
            };
            if last != Some(state) {
                gfx.world_state(&state);
                last = Some(state);
            }
            gfx.call_dl(d.dl);
        }
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
            s.motion = p.motion;
        }
        if !p.alive && s.alive {
            let c = death_conditions(p.conditions, &p.death, p.yaw);
            s.body.event(anims, EV_DEATH, mt, &c, now, true);
        }
        s.alive = p.alive;
        // The playback rate from how fast the player moves against its clips (IW4's client anim rate).
        s.body.rates(anims, p.origin, now);
        // A new flashbang or stun grenade shock plays its torso animation (a stun falls back to the shell shock's
        // when the script has no stunned one).
        let shock = p.shock.filter(|_| p.alive);
        if let Some((start, flash)) = shock
            && s.shock_ms != Some(start)
        {
            let played = (!flash
                && s.body
                    .event(anims, EV_STUNNED, mt, &p.conditions, now, false))
                || s.body
                    .event(anims, EV_SHELLSHOCK, mt, &p.conditions, now, false);
            self.shock_anims += u32::from(played);
        }
        s.shock_ms = shock.map(|(start, _)| start);
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
            // Jumps, landings and prone, under the movement type before the jump (IW4 plays them from pmove, where
            // the movement type is still the one on the ground).
            for ev in motion_events(&s.motion, &p.motion) {
                let mut c = p.conditions;
                c.value[usize::from(anim_iw4::ANIM_COND_MOVETYPE)] = 1 << s.movetype;
                s.body.event(anims, ev, s.movetype, &c, now, false);
            }
            s.motion = p.motion;
            if p.movetype.is_some() {
                s.body.moves(anims, mt, &p.conditions, now);
            }
            s.last_fire_ms = p.last_fire_ms;
            s.gun_state = p.gun_state;
            s.movetype = mt;
            s.body.turn(p.yaw, p.moving, now)
        } else {
            let (legs, _) = s.body.turn(p.yaw, false, now);
            // Where the body lies: it may settle a little, but a jump is the arena placing the player for its
            // respawn before it lives again.
            let o = p.origin;
            let near = |d: &([f32; 3], f32, usize)| {
                let (dx, dy) = (o[0] - d.0[0], o[1] - d.0[1]);
                dx * dx + dy * dy < 48.0 * 48.0
            };
            if s.dead_at.is_none_or(|d| near(&d)) {
                s.dead_at = Some((o, legs, p.team));
            }
            (legs, legs)
        }
    }

    /// Weapons lying on the ground: each weapon's third-person model (`GUNM`) at the item, turned to its yaw and
    /// laid on its side (MW2 draws its item model; ours lies flat so its grip doesn't sink into the floor).
    /// Scavenger bags aren't drawn here (no bag model is packed: the caller draws a stand-in).
    pub fn draw_items(&self, gfx: &mut impl Gfx, view: &Mtx34, items: &[game_core::item::Dropped]) {
        let Some(held) = &self.held else {
            return;
        };
        for d in items.iter().filter(|d| !d.scavenger) {
            let Some(gun) = held.model(d.weapon) else {
                continue;
            };
            let o = d.origin;
            if vmod::apply(view, o)[2] > 32.0 {
                continue;
            }
            let (s, c) = (
                libm::sinf(d.yaw.to_radians()),
                libm::cosf(d.yaw.to_radians()),
            );
            // Yaw about z, then a quarter roll about the barrel (x): the model's up (z) lies along -y.
            let model: Mtx34 = [
                [c, 0.0, s, o[0]],
                [s, 0.0, -c, o[1]],
                [0.0, 1.0, 0.0, o[2] + 2.0],
            ];
            unsafe {
                ffi::mwf_world_view();
                ffi::mwf_view_arrays(gun.positions.as_ptr(), gun.attributes.as_ptr());
            }
            gfx.pos_matrix(&vmod::mul(view, &model));
            let mut last: Option<DrawState> = None;
            let hidden = held.hidden(d.weapon);
            for i in (0..gun.draws()).filter(|&i| !hidden.contains(i)) {
                let Some(dr) = gun.draw(i) else { continue };
                let state = DrawState {
                    tex: held
                        .textures
                        .get(usize::from(dr.texture))
                        .copied()
                        .unwrap_or(-1),
                    lightmap: -1,
                    blend: dr.blend,
                    alpha_ref: dr.alpha_ref,
                    cull: 0,
                    tint: 0,
                };
                if last != Some(state) {
                    gfx.world_state(&state);
                    last = Some(state);
                }
                gfx.call_dl(dr.dl);
            }
        }
    }

    /// The bone hit boxes of every team's model (`CHHB`; 0 with an older pack).
    pub fn hitbox_count(&self) -> usize {
        self.teams.iter().map(|t| t.hitboxes.len()).sum()
    }

    /// Player `slot`'s bone hit boxes, when its body was posed at `now` (drawn this frame).
    pub fn hit_boxes(&self, slot: usize, now: u32) -> Option<&[HitBox]> {
        self.hits
            .get(slot)
            .filter(|(t, l)| *t == now && !l.is_empty())
            .map(|(_, l)| l.as_slice())
    }

    /// The killcam's stand-in for the killer's first-person weapon: weapon record `weapon`'s world model (`GUNM`)
    /// held in front of the eye at `in_view` (model to eye space; IW4 axes, x forward). Returns where its muzzle is
    /// in the world (through `eye_to_world`) for the replay's flashes.
    pub fn draw_held_in_view(
        &self,
        gfx: &mut impl Gfx,
        weapon: u32,
        in_view: &Mtx34,
        eye_to_world: &Mtx34,
    ) -> Option<([f32; 3], [f32; 3])> {
        let held = self.held.as_ref()?;
        let gun = held.model(weapon)?;
        unsafe {
            ffi::mwf_world_view();
            ffi::mwf_view_arrays(gun.positions.as_ptr(), gun.attributes.as_ptr());
        }
        gfx.pos_matrix(in_view);
        let mut last: Option<DrawState> = None;
        let hidden = held.hidden(weapon);
        for i in (0..gun.draws()).filter(|&i| !hidden.contains(i)) {
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
        let world = vmod::mul(eye_to_world, in_view);
        let [o, f] = gun.flash;
        let dir =
            core::array::from_fn(|r| world[r][0] * f[0] + world[r][1] * f[1] + world[r][2] * f[2]);
        Some((vmod::apply(&world, o), dir))
    }

    /// Where player `slot`'s muzzle was last drawn (world origin and forward), if it was.
    pub fn muzzle(&self, slot: usize) -> Option<([f32; 3], [f32; 3])> {
        self.seen.get(slot).and_then(|s| s.flash)
    }

    /// Animate and draw `players` (after the world, before the first-person model) seen through `view` (world to eye)
    /// at `now_ms`.
    pub fn draw(&mut self, gfx: &mut impl Gfx, view: &Mtx34, players: &[Pose], now_ms: u32) {
        self.parity ^= 1;
        self.cost.frames += 1;
        let mut mark = ffi::us();
        let lap = |mark: &mut u32| {
            let t = ffi::us();
            let d = t.wrapping_sub(*mark);
            *mark = t;
            d
        };
        for p in players {
            // A respawn leaves the old body where it lay.
            if p.alive
                && self
                    .seen
                    .get(p.slot)
                    .is_some_and(|s| !s.alive && s.dead_at.is_some())
            {
                self.leave_corpse(p.slot, now_ms);
                self.seen[p.slot].dead_at = None;
            }
        }
        for (k, p) in players.iter().enumerate() {
            let (legs_yaw, torso_yaw) = self.animate(p, now_ms);
            // Behind the camera (the eye looks down -z): not posed or drawn (no bone hit boxes: shots use the
            // stand-in volume, as before culling existed).
            let o = p.origin;
            if vmod::apply(view, [o[0], o[1], o[2] + 36.0])[2] > 48.0 {
                continue;
            }
            // Outside the view or past the far plane: posed for the hit boxes and weapon tags only (those bones
            // come out exactly as a full pose gives them), not skinned or drawn. The sphere holds a standing or
            // prone body with what it carries, and a dying one wherever its animation throws it.
            let centre = [o[0], o[1], o[2] + 32.0];
            let radius = if p.alive { 80.0 } else { 112.0 };
            let seen = self
                .frustum
                .as_ref()
                .is_none_or(|f| f.sees_sphere(centre, radius));
            let t = p.team.min(self.teams.len() - 1);
            let team = &self.teams[t];
            let body = &self.seen[p.slot].body;
            let mut rel = torso_yaw - legs_yaw;
            if rel > 180.0 {
                rel -= 360.0;
            } else if rel < -180.0 {
                rel += 360.0;
            }
            let pitch = if p.alive { p.pitch } else { 0.0 };
            body.pose_bones(
                &self.anims,
                &team.rig,
                &team.vm,
                now_ms,
                pitch,
                rel,
                p.prone,
                (!seen).then_some(team.hit_need.as_slice()),
                &mut self.scratch,
                &mut self.world,
            );
            self.cost.posed += 1;
            // Model (IW4 axes: x forward, z up) to world: turned with the legs, at the feet.
            let (s, c) = (
                libm::sinf(legs_yaw.to_radians()),
                libm::cosf(legs_yaw.to_radians()),
            );
            let model: Mtx34 = [[c, -s, 0.0, o[0]], [s, c, 0.0, o[1]], [0.0, 0.0, 1.0, o[2]]];
            // The bones' hit boxes as drawn (IW4L poses its collision bones the same way). Every body in front of
            // the camera is posed for them, whether it is drawn or not.
            if p.alive && !team.hitboxes.is_empty() {
                while self.hits.len() <= p.slot {
                    self.hits.push((0, Vec::new()));
                }
                let (when, list) = &mut self.hits[p.slot];
                *when = now_ms;
                list.clear();
                list.extend(team.hitboxes.iter().filter_map(|h| {
                    let bone = self.world.get(usize::from(h.bone))?;
                    game_core::damage::bone_box(&model, bone, h)
                }));
            }
            // The weapon in the hand: its model's origin on `tag_weapon_right` (living players only: MW2 drops it).
            // A riot shield in the hands goes on `tag_weapon_left` instead, and one carried as the other weapon on
            // `tag_shield_back` (MW2's attach tags; `game_core::shield` blocks with the same carry). The muzzle is
            // kept for the flashes and tracers whether the body is drawn or not.
            let rig = &team.rig;
            let front = p.shield == game_core::shield::Carry::Front && rig.weapon_left != NO_BONE;
            let hand = if front { rig.weapon_left } else { rig.weapon };
            let gun = self
                .held
                .as_ref()
                .zip(p.weapon)
                .and_then(|(h, w)| h.model(w))
                .filter(|_| p.alive && hand != NO_BONE)
                .and_then(|g| Some((g, vmod::mul(&model, self.world.get(usize::from(hand))?))));
            if let Some((gun, gun_world)) = &gun
                && !front
            {
                let [o, f] = gun.flash;
                let dir = core::array::from_fn(|r| {
                    gun_world[r][0] * f[0] + gun_world[r][1] * f[1] + gun_world[r][2] * f[2]
                });
                self.seen[p.slot].flash = Some((vmod::apply(gun_world, o), dir));
            }
            let back_bone = (p.alive
                && p.shield == game_core::shield::Carry::Back
                && rig.shield_back != NO_BONE)
                .then(|| self.world.get(usize::from(rig.shield_back)).copied())
                .flatten();
            self.cost.pose_us += lap(&mut mark);
            if !seen {
                self.cost.culled += 1;
                continue;
            }
            let Some(buf) = self.buffer(k, t) else {
                continue;
            };
            let team = &self.teams[t];
            // Skin matrices, then only the vertices on more than one bone (the load put them first); GX places the
            // rest with their bone's matrix.
            crate::ps::concat_each(&self.world, &team.inv_bind, &mut self.skin);
            crate::ps::skin(&team.blended, &self.skin, buf);
            unsafe { ffi::mwf_flush(buf.as_ptr().cast(), (team.blended.len() * 12) as u32) };
            self.cost.skin_us += lap(&mut mark);
            self.cost.drawn += 1;
            let eye = vmod::mul(view, &model);
            unsafe {
                ffi::mwf_world_view();
                ffi::mwf_view_arrays_indexed(buf.as_ptr().cast(), team.attributes);
            }
            let tint = game_core::light::tint_at(
                self.lattice.as_ref(),
                [o[0], o[1], o[2] + game_core::light::CHARACTER_LIGHT_HEIGHT],
            );
            let mut last: Option<DrawState> = None;
            let mut loaded = crate::ps::UNLOADED;
            for d in &team.draws {
                crate::ps::load_slots(&eye, &self.skin, &d.slots, &mut loaded);
                let state = DrawState { tint, ..d.state };
                if last != Some(state) {
                    gfx.world_state(&state);
                    last = Some(state);
                }
                gfx.call_dl(d.dl);
            }
            if let (Some((gun, gun_world)), Some(w)) = (gun, p.weapon) {
                self.draw_gun(gfx, &gun, w, view, &gun_world, tint);
            }
            let back = self
                .shield_weapon
                .zip(self.held.as_ref())
                .and_then(|(w, h)| Some((w, h.model(w)?)));
            if let (Some((shield_w, shield)), Some(bone)) = (back, back_bone) {
                let at = vmod::mul(&model, &bone);
                self.draw_gun(gfx, &shield, shield_w, view, &at, tint);
            }
            self.cost.submit_us += lap(&mut mark);
        }
        self.cost.pose_us += lap(&mut mark);
        self.draw_corpses(gfx, view);
        self.cost.submit_us += lap(&mut mark);
    }
}

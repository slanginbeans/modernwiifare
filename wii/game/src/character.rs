//! Player characters on the Wii: each team's MW2 body and head from the map pack (`CHAR`), animated with MW2's
//! player animations from the common pack (`PANM` / `PANB`, bound by bone name), skinned on the CPU like the
//! first-person model, and drawn in the world at each player's position and facing.
//!
//! Lighting is a flat ×1 for now (MW2 lights characters from the light grid).

use alloc::vec::Vec;

use game_core::character::{bind_clips, slot_for};
use game_core::gfx::{DrawState, Gfx, Mtx34};
use game_core::viewmodel as vmod;
use wpk::records::{self, NONE};
use wpk::view::{Clip, ViewModel};

use crate::ffi;

/// GX vertex format of the display lists (POS, CLR0, TEX0 indexed), as the first-person model's.
const VTXFMT: u8 = 6;

struct Draw {
    dl: &'static [u8],
    state: DrawState,
}

struct Team {
    vm: ViewModel,
    attributes: *const u8,
    draws: Vec<Draw>,
    /// The bound clip of each player animation slot.
    by_slot: Vec<Option<usize>>,
}

/// One drawn player: its team model and the positions buffer it was skinned into this frame.
pub struct Characters {
    teams: Vec<Team>,
    /// Skinned positions per drawn player (MEM2; the GPU reads them after the CPU moved on, so each player has its
    /// own, and two of them: frames are pipelined, so the GPU may still be drawing last frame's).
    buffers: Vec<(usize, &'static mut [[f32; 3]])>,
    /// Which of each player's two buffers this frame writes.
    parity: usize,
    world: Vec<Mtx34>,
    skin: Vec<Mtx34>,
}

fn mem2(count: usize) -> Option<&'static mut [u8]> {
    let p = unsafe { ffi::mwf_mem2_alloc(count as u32, 32) };
    (!p.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(p, count) })
}

/// What one player looks like this frame.
pub struct Pose {
    /// 0 allies (or free-for-all), 1 axis.
    pub team: usize,
    pub origin: [f32; 3],
    pub yaw: f32,
    pub prone: bool,
    pub crouched: bool,
    pub sprinting: bool,
    pub velocity: [f32; 3],
}

impl Characters {
    /// The map's characters (`map`'s `CHAR`) with the common pack's player animations; `texture` turns a map texture
    /// index into its handle.
    pub fn load(
        map: &'static [u8],
        common: &'static [u8],
        texture: impl Fn(u16) -> i32,
    ) -> Option<Characters> {
        let pack = wpk::Pack::parse(map, false).ok()?;
        let teams = wpk::view::decode_characters(pack.section(wpk::tag::CHARACTERS)?.data);
        let common = wpk::Pack::parse(common, false).ok()?;
        let anims = common
            .section(wpk::tag::PLAYER_ANIMS)
            .and_then(|s| ViewModel::decode(s.data).ok())
            .unwrap_or_default();
        let anim_names = common
            .section(wpk::tag::PLAYER_ANIM_BONES)
            .and_then(|s| wpk::view::decode_names(s.data))
            .unwrap_or_default();
        let mut out = Vec::new();
        for (mut vm, names) in teams {
            vm.clips = bind_clips(&vm, &names, &anims, &anim_names);
            let mut by_slot = alloc::vec![None; 16];
            for (i, c) in vm.clips.iter().enumerate() {
                if let Some(s) = by_slot.get_mut(usize::from(c.slot)) {
                    *s = Some(i);
                }
            }
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
                by_slot,
                vm,
            });
        }
        (!out.is_empty()).then(|| Characters {
            teams: out,
            buffers: Vec::new(),
            parity: 0,
            world: Vec::new(),
            skin: Vec::new(),
        })
    }

    /// Draw `players` (after the world, before the first-person model) seen through `view` (world to eye) at
    /// `now_ms`.
    pub fn draw(&mut self, gfx: &mut impl Gfx, view: &Mtx34, players: &[Pose], now_ms: u32) {
        self.parity ^= 1;
        for (player, p) in players.iter().enumerate() {
            // This player's buffer for this frame (two per player, alternating).
            let k = player * 2 + self.parity;
            // Behind the camera (the eye looks down -z): not skinned or drawn.
            let o = p.origin;
            if vmod::apply(view, [o[0], o[1], o[2] + 36.0])[2] > 48.0 {
                continue;
            }
            let t = p.team.min(self.teams.len() - 1);
            let team = &self.teams[t];
            let n = team.vm.verts.len();
            // This player's positions buffer (per player slot, frame parity and team model).
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
            let slot = slot_for(p.prone, p.crouched, p.sprinting, p.velocity, p.yaw);
            let clip: Option<&Clip> = team
                .by_slot
                .get(usize::from(slot))
                .copied()
                .flatten()
                .or_else(|| team.by_slot.first().copied().flatten())
                .and_then(|i| team.vm.clips.get(i));
            // Players loop their clip from their own start (spread by slot so they don't move in step).
            let time = clip.map_or(0.0, |c| {
                let d = vmod::duration(c).max(0.001);
                let t = now_ms.wrapping_add(player as u32 * 377) as f32 / 1000.0;
                t - libm::floorf(t / d) * d
            });
            vmod::pose(&team.vm, clip.map(|c| (c, time)), &mut self.world);
            vmod::skin_matrices(&team.vm, &self.world, &mut self.skin);
            let buf = &mut self.buffers[k].1;
            vmod::skin_into(&team.vm, &self.skin, buf);
            // Model (IW4 axes: x forward, z up) to world: turned by the yaw, at the feet.
            let (s, c) = (
                libm::sinf(p.yaw.to_radians()),
                libm::cosf(p.yaw.to_radians()),
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
        }
    }
}

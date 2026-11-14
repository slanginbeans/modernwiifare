//! The first-person model on the Wii: decoded from `common.wpk` once per map, posed and skinned on the CPU each frame
//! (game_core::viewmodel), drawn from prebuilt display lists over positions written to MEM2.
//!
//! Lighting is a flat ×1 for now (MW2 lights the viewmodel from the light grid at the player).

use alloc::vec::Vec;

use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::viewmodel as vmod;
use wpk::records::{self, NONE, TextureRecord, texture_flags};
use wpk::view::{Clip, ViewModel};

use crate::ffi;

/// MW2's first-person FOV is the player's (65° horizontal at 4:3 ≈ 50.5° vertical); near 1 unit.
const FOVY: f32 = 50.5;
const ZNEAR: f32 = 1.0;
const ZFAR: f32 = 512.0;
/// GX vertex format of the model's display lists (POS, CLR0, TEX0 indexed).
const VTXFMT: u8 = 6;

struct Draw {
    dl: &'static [u8],
    state: DrawState,
}

pub struct View {
    vm: ViewModel,
    /// Skinned positions (MEM2, written each frame).
    positions: &'static mut [[f32; 3]],
    /// RGBA + UV per vertex (MEM2, fixed).
    attributes: *const u8,
    draws: Vec<Draw>,
    world: Vec<Mtx34>,
    skin: Vec<Mtx34>,
    skinned: Vec<[f32; 3]>,
    /// The clip playing: (slot, restart toggle, start time in ms).
    playing: (u8, bool, u32),
}

/// `count` bytes of 32-byte aligned MEM2, released with the map.
fn mem2(count: usize) -> Option<&'static mut [u8]> {
    let p = unsafe { ffi::mwf_mem2_alloc(count as u32, 32) };
    (!p.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(p, count) })
}

impl View {
    /// The model of weapon `index` (its `WEAP` record index) from the common pack, with its textures created.
    pub fn load(common: &[u8], index: u32, gfx: &mut impl Gfx) -> Option<View> {
        let pack = wpk::Pack::parse(common, false).ok()?;
        let section = pack
            .section(wpk::tag::VIEWMODEL)
            .filter(|s| s.count == index)?;
        let vm = ViewModel::decode(section.data).ok()?;
        // Textures (the common pack is in MEM2, 32-byte aligned, flushed).
        let txda = pack.section(wpk::tag::TEXTURE_DATA)?.data;
        let mut handles = Vec::new();
        for r in records::iter::<TextureRecord>(pack.section(wpk::tag::TEXTURES)?.data) {
            let data = txda.get(r.data_off as usize..(r.data_off + r.data_len) as usize)?;
            let alpha = if r.flags & texture_flags::ALPHA_PLANE != 0 {
                txda.get(r.alpha_off as usize..(r.alpha_off + r.alpha_len) as usize)?
                    .as_ptr()
            } else {
                core::ptr::null()
            };
            handles.push(gfx.tex_create(&TexDesc {
                data: data.as_ptr(),
                alpha,
                width: r.width,
                height: r.height,
                format: r.format,
                levels: r.levels,
                wrap: r.wrap,
            }));
        }
        let n = vm.verts.len();
        // Positions and attributes in MEM2.
        let pos_bytes = mem2(n * 12)?;
        let positions = unsafe {
            core::slice::from_raw_parts_mut(pos_bytes.as_mut_ptr().cast::<[f32; 3]>(), n)
        };
        let attr = mem2(n * 12)?;
        for (i, v) in vm.verts.iter().enumerate() {
            let at = i * 12;
            attr[at..at + 4].copy_from_slice(&v.rgba.to_be_bytes());
            attr[at + 4..at + 8].copy_from_slice(&v.uv[0].to_be_bytes());
            attr[at + 8..at + 12].copy_from_slice(&v.uv[1].to_be_bytes());
        }
        unsafe { ffi::mwf_flush(attr.as_ptr(), attr.len() as u32) };
        // One display list per surface: GX_TRIANGLES, three u16 indices per vertex.
        let mut draws = Vec::with_capacity(vm.surfaces.len());
        for s in &vm.surfaces {
            let idx = &vm.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
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
            let tex = if s.texture == NONE {
                -1
            } else {
                handles.get(usize::from(s.texture)).copied().unwrap_or(-1)
            };
            draws.push(Draw {
                dl,
                state: DrawState {
                    tex,
                    lightmap: -1,
                    blend: s.blend,
                    alpha_ref: s.alpha_ref,
                    cull: 0,
                    tint: 0,
                },
            });
        }
        Some(View {
            positions,
            attributes: attr.as_ptr(),
            draws,
            world: Vec::with_capacity(vm.bones.len()),
            skin: Vec::with_capacity(vm.bones.len()),
            skinned: Vec::with_capacity(n),
            playing: (wpk::view::slot::IDLE, false, 0),
            vm,
        })
    }

    /// Pose for the gun's animation (`slot`, restart toggle) at `now_ms`, skin, and draw. Call after the world.
    pub fn draw(&mut self, gfx: &mut impl Gfx, slot: u8, restart: bool, now_ms: u32) {
        if (slot, restart) != (self.playing.0, self.playing.1) {
            self.playing = (slot, restart, now_ms);
        }
        let time = now_ms.wrapping_sub(self.playing.2) as f32 / 1000.0;
        let clip: Option<&Clip> = self
            .vm
            .clip(slot)
            .or_else(|| self.vm.clip(wpk::view::slot::IDLE));
        vmod::pose(&self.vm, clip.map(|c| (c, time)), &mut self.world);
        vmod::skin_matrices(&self.vm, &self.world, &mut self.skin);
        vmod::skin(&self.vm, &self.skin, &mut self.skinned);
        self.positions.copy_from_slice(&self.skinned);
        unsafe {
            ffi::mwf_flush(
                self.positions.as_ptr().cast(),
                (self.positions.len() * 12) as u32,
            );
            ffi::mwf_view_begin(FOVY, ZNEAR, ZFAR);
            ffi::mwf_view_arrays(self.positions.as_ptr().cast(), self.attributes);
        }
        gfx.pos_matrix(&vmod::eye_from_model(
            &self.world[usize::from(self.vm.view_bone)],
        ));
        let mut last: Option<DrawState> = None;
        for d in &self.draws {
            if last != Some(d.state) {
                gfx.world_state(&d.state);
                last = Some(d.state);
            }
            gfx.call_dl(d.dl);
        }
    }
}

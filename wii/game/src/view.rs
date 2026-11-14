//! The first-person model on the Wii: decoded from the weapon's view file when its class is equipped, animated as
//! MW2's viewmodel anim tree (`game_core::weapon_anim`: every weapon animation slot, clips fitted to the weapon's
//! timers, the ADS overlay, per-bone blending), placed as MW2 places the gun (`game_core::weapon_view`: view sway,
//! bob, idle sway, stance and movement offsets, recoil), skinned on the CPU, and drawn from prebuilt display lists
//! over positions written to MEM2.
//!
//! Lighting is a flat ×1 for now (MW2 lights the viewmodel from the light grid at the player).

use alloc::vec::Vec;

use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::gun::Gun;
use game_core::viewmodel as vmod;
use game_core::weapon_anim::{Animator, BlendScratch, Playing, Timers, pose_blend};
use game_core::weapon_view::{WeaponView, eye_transform};
use playerstate_iw4::PlayerState;
use wpk::records::{self, NONE, TextureRecord, texture_flags};
use wpk::sound::WeaponSounds;
use wpk::view::ViewModel;
use wpk::weapon::FpvRecord;

use crate::ffi;

/// MW2's first-person FOV is the player's (65° horizontal at 4:3 = 51.08° vertical); near 1 unit.
const FOVY: f32 = 51.08;
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
    /// The anim tree (built for the gun it was first drawn with), the clips it plays, blend scratch.
    anim: Option<(u32, Animator)>,
    playing: Vec<Playing>,
    blend: BlendScratch,
    /// Where the gun sits (sway, bob ...).
    placement: WeaponView,
    /// Model-to-eye matrix of the last draw (for the muzzle flash).
    last_eye: Option<Mtx34>,
    /// The weapon's sounds (`WSND`; empty when the pack has none) and the clips they play.
    pub sounds: WeaponSounds,
    bank: Option<crate::sound::Sounds<'static>>,
    /// The clip notetracks follow (action, else idle) and its fraction at the last draw.
    last_frac: Option<(usize, f32)>,
    /// Notetrack sounds the last draw passed.
    notes_due: Vec<u16>,
}

/// `count` bytes of 32-byte aligned MEM2, released with the map.
fn mem2(count: usize) -> Option<&'static mut [u8]> {
    let p = unsafe { ffi::mwf_mem2_alloc(count as u32, 32) };
    (!p.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(p, count) })
}

impl View {
    /// A weapon's first-person file (`views/<weapon>.wpk`, in MEM2 and flushed): its model with textures created,
    /// and its sounds.
    pub fn load(file: &'static [u8], gfx: &mut impl Gfx) -> Option<View> {
        let pack = wpk::Pack::parse(file, false).ok()?;
        let vm = ViewModel::decode(pack.section(wpk::tag::VIEWMODEL)?.data).ok()?;
        let sounds = pack
            .section(wpk::tag::WEAPON_SOUNDS)
            .and_then(|s| WeaponSounds::decode(s.data))
            .unwrap_or_default();
        let bank = crate::sound::Sounds::load(file);
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
            anim: None,
            playing: Vec::with_capacity(8),
            blend: BlendScratch::default(),
            placement: WeaponView::default(),
            last_eye: None,
            sounds,
            bank,
            last_frac: None,
            notes_due: Vec::new(),
            vm,
        })
    }

    /// Play the sound of weapon event `e` (`wpk::sound::event`).
    pub fn play_event_alias(&mut self, e: usize) {
        if let Some(&a) = self.sounds.events.get(e) {
            self.play(a);
        }
    }

    /// Play alias `alias` of this weapon.
    pub fn play(&mut self, alias: u16) {
        if let Some(b) = self.bank.as_mut() {
            b.play(alias);
        }
    }

    /// Play the notetrack sounds the last draw's animation passed.
    pub fn play_notes(&mut self) {
        if let Some(b) = self.bank.as_mut() {
            for &a in &self.notes_due {
                b.play(a);
            }
        }
    }

    /// Where the gun's `tag_flash` is on the 640 × 480 HUD, from the last draw.
    pub fn muzzle_screen(&self, aspect: f32) -> Option<[f32; 2]> {
        let eye = self.last_eye.as_ref()?;
        let m = self.world.get(usize::from(self.vm.flash_bone))?;
        let p = vmod::apply(eye, [m[0][3], m[1][3], m[2][3]]);
        if p[2] > -ZNEAR {
            return None;
        }
        let ty = libm::tanf(FOVY.to_radians() * 0.5);
        let tx = ty * aspect;
        Some([
            320.0 + p[0] / (-p[2] * tx) * 320.0,
            240.0 - p[1] / (-p[2] * ty) * 240.0,
        ])
    }

    /// Animate for `gun`'s state over `dt` seconds, place by the player's state `ps` and the weapon's first-person
    /// numbers, skin, and draw. Call after the world.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gfx: &mut impl Gfx,
        gun: &Gun,
        ps: &PlayerState,
        fpv: &FpvRecord,
        dt: f32,
        now_ms: u32,
    ) {
        // The anim tree, built for this gun's timers (again when the gun changes).
        let index = gun.weapon_index();
        if self.anim.as_ref().is_none_or(|(w, _)| *w != index) {
            self.anim = Some((index, Animator::new(&self.vm, Timers::of(gun.facts()))));
            self.placement.reset();
            self.last_frac = None;
        }
        let Some((_, anim)) = self.anim.as_mut() else {
            return;
        };
        anim.set_perks(ps.perks);
        anim.weap_anim(gun.weap_anim_raw(), gun.ammo().0 == 0);
        anim.advance(dt);
        anim.ads(ps.f_weapon_pos_frac);
        anim.playing(&mut self.playing);
        // Notetrack sounds of the clip in front (the action, else the idle) passed since the last frame.
        self.notes_due.clear();
        if let Some((clip, time)) = anim.action().or_else(|| anim.idle())
            && let Some(c) = self.vm.clips.get(clip)
        {
            let frac = vmod::fraction(c, time);
            let from = self
                .last_frac
                .filter(|(k, f)| *k == clip && *f <= frac)
                .map(|(_, f)| f);
            self.notes_due.extend(game_core::weapon_sound::notes_passed(
                &self.sounds.notes,
                c.slot,
                from,
                frac,
            ));
            self.last_frac = Some((clip, frac));
        }
        pose_blend(&self.vm, &self.playing, &mut self.blend, &mut self.world);
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
        // MW2's weapon placement (sway, bob, idle sway, movement offsets, recoil) about the eye.
        let placed = self.placement.update(
            ps,
            gun.facts(),
            fpv,
            gun.gun_max(),
            gun.gun_recoil_state(),
            dt,
            now_ms,
        );
        let eye = vmod::eye_from_model(&self.world[usize::from(self.vm.view_bone)]);
        let eye = vmod::mul(&eye_transform(&placed), &eye);
        gfx.pos_matrix(&eye);
        self.last_eye = Some(eye);
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

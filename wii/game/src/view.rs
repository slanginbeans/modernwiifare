//! The first-person model on the Wii: decoded from the weapon's view file when its class is equipped, animated as
//! MW2's viewmodel anim tree (`game_core::weapon_anim`: every weapon animation slot, clips fitted to the weapon's
//! timers, the ADS overlay, per-bone blending), placed as MW2 places the gun (`game_core::weapon_view`: view sway,
//! bob, idle sway, stance and movement offsets, recoil), and drawn from prebuilt display lists over positions in
//! MEM2. Rigid vertices (one bone at full weight: most of a gun) keep their model-space positions and GX places them
//! with their bone's matrix (a matrix index per vertex, at most ten matrices per draw); only the blended ones (arms,
//! hands) are skinned on the CPU each frame.
//!
//! Lit by the map's light lattice at IW4's viewmodel lighting origin (`game_core::light`, [`View::tint`], set by
//! the caller each frame; 0 is ×1).
//!
//! Other weapons on the same model (`VSET`): an akimbo pair and an alternate (the underbarrel launcher or shotgun)
//! play their own clip sets, loaded from the weapon's companion file ([`View::attach`]) and bound to the guns that
//! use them ([`View::bind`]). An akimbo pair's left gun is the model drawn a second time, posed by the left hand's
//! clips and its own anim tree (IW4's `szXAnimsLeftHanded`, driven by the left hand's `weap_anim`).

use alloc::vec::Vec;

use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::gun::Gun;
use game_core::viewmodel as vmod;
use game_core::weapon_anim::{Animator, BlendScratch, Playing, Timers, pose_blend};
use game_core::weapon_view::{WeaponView, eye_transform};
use playerstate_iw4::PlayerState;
use wpk::records::{self, NONE, TextureRecord, texture_flags};
use wpk::sound::WeaponSounds;
use wpk::view::{SetRecord, ViewModel};
use wpk::weapon::FpvRecord;

use crate::ffi;

/// MW2's first-person FOV is the player's (65° horizontal at 4:3 = 51.08° vertical); near 1 unit.
const FOVY: f32 = 51.08;
const ZNEAR: f32 = 1.0;
const ZFAR: f32 = 512.0;
/// GX vertex format of the model's display lists (matrix index direct; POS, CLR0, TEX0 indexed).
const VTXFMT: u8 = 6;

struct Draw {
    dl: &'static [u8],
    state: DrawState,
    /// The matrix of each slot this draw's vertices use (`vmod::matrix_key`: a bone, or `BLENDED`).
    slots: Vec<u16>,
}

pub struct View {
    /// The light colour this frame (`DrawState::tint`; 0: ×1).
    pub tint: u32,
    vm: ViewModel,
    /// Skinned positions (MEM2, written each frame), two sets: frames are pipelined, so the GPU may still be drawing
    /// last frame's while this one is written.
    positions: [&'static mut [[f32; 3]]; 2],
    /// Which set this frame writes.
    parity: usize,
    /// RGBA + UV per vertex (MEM2, fixed).
    attributes: *const u8,
    draws: Vec<Draw>,
    world: Vec<Mtx34>,
    skin: Vec<Mtx34>,
    /// The vertices the CPU skins (blended); the rest are placed by GX.
    blended: Vec<u16>,
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
    /// Notetrack sounds the last draw passed (alias, from the companion's bank).
    notes_due: Vec<(u16, bool)>,
    /// The weapons whose clips are sets of this model (`VSET`), the sets bound to each gun (weapon index, set,
    /// left hand's set or 0), and the companion file's sounds.
    sets: Vec<SetRecord>,
    binds: Vec<(u32, u8, u8)>,
    companion: Option<Companion>,
    /// The left gun of an akimbo pair: a second pose of the model.
    left: Option<LeftGun>,
}

/// The sounds of a weapon's companion file (an alternate's own fire, reload ... and its clips' notetracks).
struct Companion {
    sounds: WeaponSounds,
    bank: Option<crate::sound::Sounds<'static>>,
    /// The clip sets it brought.
    sets: Vec<u8>,
}

/// The left gun's pose: its anim tree, skinned positions (two sets, as the right's) and matrices.
struct LeftGun {
    positions: [&'static mut [[f32; 3]]; 2],
    anim: Option<(u32, Animator)>,
    playing: Vec<Playing>,
    world: Vec<Mtx34>,
    skin: Vec<Mtx34>,
    last_frac: Option<(usize, f32)>,
}

/// `count` bytes of 32-byte aligned MEM2, released with the map.
fn mem2(count: usize) -> Option<&'static mut [u8]> {
    let p = unsafe { ffi::mwf_mem2_alloc(count as u32, 32) };
    (!p.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(p, count) })
}

impl View {
    /// A weapon's first-person file (`views/<weapon>.wpk`, in MEM2 and flushed): its model with textures created,
    /// and its sounds. `names`: the weapon names the slot's gun may be, in the order tried: the first the file has
    /// a `VHID` row for decides which attachment parts are kept (the rest are left out of memory).
    pub fn load(
        file: &'static [u8],
        gfx: &mut impl Gfx,
        names: &[alloc::string::String],
    ) -> Option<View> {
        let pack = wpk::Pack::parse(file, false).ok()?;
        let mut vm = ViewModel::decode(pack.section(wpk::tag::VIEWMODEL)?.data).ok()?;
        if let Some(parts) = pack
            .section(wpk::tag::VIEW_PARTS)
            .and_then(|s| wpk::parts::Parts::parse(s.data))
            && let Some(hidden) = names
                .iter()
                .find_map(|n| parts.row(wpk::parts::name_key(n)))
        {
            vm.drop_surfaces(|i| hidden.contains(i));
        }
        let sounds = pack
            .section(wpk::tag::WEAPON_SOUNDS)
            .and_then(|s| WeaponSounds::decode(s.data))
            .unwrap_or_default();
        let sets = pack
            .section(wpk::tag::VIEW_SETS)
            .map(|s| records::iter::<SetRecord>(s.data).collect())
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
        let set = || -> Option<&'static mut [[f32; 3]]> {
            let b = mem2(n * 12)?;
            Some(unsafe { core::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<[f32; 3]>(), n) })
        };
        let mut positions = [set()?, set()?];
        // Rigid vertices stay in model space in both sets (GX moves them); the blended ones are written each frame.
        for p in positions.iter_mut() {
            for (o, v) in p.iter_mut().zip(&vm.verts) {
                *o = v.pos;
            }
            unsafe { ffi::mwf_flush(p.as_ptr().cast(), (n * 12) as u32) };
        }
        let blended: Vec<u16> = (0..n as u16)
            .filter(|&i| vmod::matrix_key(&vm.verts[usize::from(i)]) == vmod::BLENDED)
            .collect();
        let attr = mem2(n * 12)?;
        for (i, v) in vm.verts.iter().enumerate() {
            let at = i * 12;
            attr[at..at + 4].copy_from_slice(&v.rgba.to_be_bytes());
            attr[at + 4..at + 8].copy_from_slice(&v.uv[0].to_be_bytes());
            attr[at + 8..at + 12].copy_from_slice(&v.uv[1].to_be_bytes());
        }
        unsafe { ffi::mwf_flush(attr.as_ptr(), attr.len() as u32) };
        // Display lists per surface, cut into groups of at most ten matrices: GX_TRIANGLES, each vertex a matrix
        // index (slot x 3) then three u16 indices.
        let mut draws = Vec::with_capacity(vm.surfaces.len());
        for s in &vm.surfaces {
            let idx = &vm.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
            if idx.is_empty() {
                continue;
            }
            let tex = if s.texture == NONE {
                -1
            } else {
                handles.get(usize::from(s.texture)).copied().unwrap_or(-1)
            };
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
                        tex,
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
        Some(View {
            positions,
            parity: 0,
            attributes: attr.as_ptr(),
            draws,
            world: Vec::with_capacity(vm.bones.len()),
            skin: Vec::with_capacity(vm.bones.len()),
            blended,
            anim: None,
            playing: Vec::with_capacity(8),
            tint: 0,
            blend: BlendScratch::default(),
            placement: WeaponView::default(),
            last_eye: None,
            sounds,
            bank,
            last_frac: None,
            notes_due: Vec::new(),
            sets,
            binds: Vec::new(),
            companion: None,
            left: None,
            vm,
        })
    }

    /// The companion file to load for a slot carrying the first of `names` (weapon names, most specific first) that
    /// this model has sets for: `views/<that>.wpk`'s name (`wpk::view::companion_name`).
    pub fn companion_for<'n>(
        &self,
        base: &str,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Option<alloc::string::String> {
        names.into_iter().find_map(|n| {
            let r = self.sets.iter().find(|r| r.name() == n)?;
            Some(wpk::view::companion_name(base, r.set))
        })
    }

    /// Add a companion file (`views/<base>@<set>.wpk`, in MEM2 and flushed): its clip sets join the model, its
    /// sounds play for the guns bound to them. False when it doesn't fit this model.
    pub fn attach(&mut self, file: &'static [u8]) -> bool {
        let Ok(pack) = wpk::Pack::parse(file, false) else {
            return false;
        };
        let Some(clips) = pack
            .section(wpk::tag::VIEW_CLIPS)
            .and_then(|s| wpk::view::decode_clip_set(s.data).ok())
        else {
            return false;
        };
        let mut sets: Vec<u8> = clips.iter().map(|c| c.set).collect();
        sets.sort_unstable();
        sets.dedup();
        if !self.vm.add_clips(clips) {
            return false;
        }
        let sounds = pack
            .section(wpk::tag::WEAPON_SOUNDS)
            .and_then(|s| WeaponSounds::decode(s.data))
            .unwrap_or_default();
        self.companion = Some(Companion {
            sounds,
            bank: crate::sound::Sounds::load(file),
            sets,
        });
        // The anim trees rebuild with the new clips.
        self.anim = None;
        true
    }

    /// Bind the guns of a slot to their clip sets: `gun` (weapon index, name) and its alternate's weapon index.
    /// An akimbo pair gets its hands' sets (and the left gun's pose), an alternate its set; others play set 0.
    pub fn bind(&mut self, gun: Option<(u32, &str)>, alternate: Option<u32>) {
        self.binds.clear();
        let Some((index, name)) = gun else {
            return;
        };
        let Some(r) = self.sets.iter().find(|r| r.name() == name).copied() else {
            return;
        };
        let have = |s: u8| self.companion.as_ref().is_some_and(|c| c.sets.contains(&s));
        if r.alternate {
            if let Some(a) = alternate.filter(|_| have(r.set)) {
                self.binds.push((a, r.set, 0));
            }
        } else if have(r.set) && have(r.left) {
            self.binds.push((index, r.set, r.left));
            if self.left.is_none() {
                self.left = LeftGun::new(self.vm.verts.len(), &self.vm);
            }
        }
        self.anim = None;
    }

    /// The clip sets weapon `index` plays (its set, its left hand's set or 0).
    fn sets_of(&self, index: u32) -> (u8, u8) {
        self.binds
            .iter()
            .find(|b| b.0 == index)
            .map_or((0, 0), |b| (b.1, b.2))
    }

    /// The sounds weapon `index` plays: the companion's when it is bound to an alternate's set there.
    fn sounds_of(
        &mut self,
        index: u32,
    ) -> (&WeaponSounds, Option<&mut crate::sound::Sounds<'static>>) {
        let (set, left) = self.sets_of(index);
        match self.companion.as_mut() {
            Some(c) if set != 0 && left == 0 && c.sets.contains(&set) => {
                (&c.sounds, c.bank.as_mut())
            }
            _ => (&self.sounds, self.bank.as_mut()),
        }
    }

    /// Play the sound of weapon event `ev` for weapon `index` (`game_core::weapon_sound::for_event`; an alternate's
    /// from its companion file), `clip_after` rounds left in the magazine. False when it has none.
    pub fn play_tick_event(
        &mut self,
        index: u32,
        ev: &weapon_iw4::WeaponTickEvent,
        clip_after: i32,
    ) -> bool {
        let (sounds, bank) = self.sounds_of(index);
        match (
            game_core::weapon_sound::for_event(sounds, ev, clip_after),
            bank,
        ) {
            (Some(a), Some(b)) => {
                b.play(a);
                true
            }
            _ => false,
        }
    }

    /// Play the sound of weapon event `e` (`wpk::sound::event`) for weapon `index` (an alternate's own).
    pub fn play_event_alias_for(&mut self, index: u32, e: usize) {
        let (sounds, bank) = self.sounds_of(index);
        if let (Some(&a), Some(b)) = (sounds.events.get(e), bank)
            && a != wpk::sound::NONE
        {
            b.play(a);
        }
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
        for &(a, companion) in &self.notes_due {
            let bank = if companion {
                self.companion.as_mut().and_then(|c| c.bank.as_mut())
            } else {
                self.bank.as_mut()
            };
            if let Some(b) = bank {
                b.play(a);
            }
        }
    }

    /// The gun's `tag_flash` in eye space (origin, forward), from the last draw.
    pub fn muzzle_eye(&self) -> Option<([f32; 3], [f32; 3])> {
        let eye = self.last_eye.as_ref()?;
        let m = vmod::mul(eye, self.world.get(usize::from(self.vm.flash_bone))?);
        Some(([m[0][3], m[1][3], m[2][3]], [m[0][0], m[1][0], m[2][0]]))
    }

    /// The left gun's `tag_flash` in eye space (an akimbo pair), from the last draw.
    pub fn muzzle_eye_left(&self) -> Option<([f32; 3], [f32; 3])> {
        let eye = self.last_eye.as_ref()?;
        let left = self.left.as_ref().filter(|l| l.anim.is_some())?;
        let m = vmod::mul(eye, left.world.get(usize::from(self.vm.flash_bone))?);
        Some(([m[0][3], m[1][3], m[2][3]], [m[0][0], m[1][0], m[2][0]]))
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
        // The anim tree, built for this gun's timers and clip set (again when the gun changes).
        let index = gun.weapon_index();
        let (set, left_set) = self.sets_of(index);
        if self.anim.as_ref().is_none_or(|(w, _)| *w != index) {
            let timers = Timers::of(gun.facts());
            // An akimbo hand's set is whole (no fallback to the right-handed clips of set 0).
            self.anim = Some((
                index,
                Animator::for_set(&self.vm, timers, set, left_set == 0),
            ));
            self.placement.reset();
            self.last_frac = None;
            if let Some(l) = self.left.as_mut() {
                l.anim = (left_set != 0)
                    .then(|| (index, Animator::for_set(&self.vm, timers, left_set, false)));
                l.last_frac = None;
            }
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
        let companion_sets = self
            .companion
            .as_ref()
            .map_or(&[][..], |c| c.sets.as_slice());
        let notes_of = |c: &wpk::view::Clip| -> &[wpk::sound::Note] {
            match self.companion.as_ref() {
                Some(k) if companion_sets.contains(&c.set) => &k.sounds.notes,
                _ => &self.sounds.notes,
            }
        };
        if let Some((clip, time)) = anim.action().or_else(|| anim.idle())
            && let Some(c) = self.vm.clips.get(clip)
        {
            let frac = vmod::fraction(c, time);
            let from = self
                .last_frac
                .filter(|(k, f)| *k == clip && *f <= frac)
                .map(|(_, f)| f);
            let from_companion = companion_sets.contains(&c.set);
            self.notes_due.extend(
                game_core::weapon_sound::notes_passed(notes_of(c), c.set, c.slot, from, frac)
                    .map(|a| (a, from_companion)),
            );
            self.last_frac = Some((clip, frac));
        }
        pose_blend(&self.vm, &self.playing, &mut self.blend, &mut self.world);
        vmod::skin_matrices(&self.vm, &self.world, &mut self.skin);
        // The left gun of an akimbo pair: its own hand's state on its own tree.
        let left_raw = gun.left_weap_anim_raw();
        let left_drawn = match (self.left.as_mut(), left_raw) {
            (Some(l), Some(raw)) if l.anim.is_some() => {
                let Some((_, la)) = l.anim.as_mut() else {
                    return;
                };
                la.set_perks(ps.perks);
                la.weap_anim(raw, gun.left_clip() == Some(0));
                la.advance(dt);
                la.ads(ps.f_weapon_pos_frac);
                la.playing(&mut l.playing);
                if let Some((clip, time)) = la.action().or_else(|| la.idle())
                    && let Some(c) = self.vm.clips.get(clip)
                {
                    let frac = vmod::fraction(c, time);
                    let from = l
                        .last_frac
                        .filter(|(k, f)| *k == clip && *f <= frac)
                        .map(|(_, f)| f);
                    self.notes_due.extend(
                        game_core::weapon_sound::notes_passed(
                            &self.sounds.notes,
                            c.set,
                            c.slot,
                            from,
                            frac,
                        )
                        .map(|a| (a, false)),
                    );
                    l.last_frac = Some((clip, frac));
                }
                pose_blend(&self.vm, &l.playing, &mut self.blend, &mut l.world);
                vmod::skin_matrices(&self.vm, &l.world, &mut l.skin);
                true
            }
            _ => false,
        };
        self.parity ^= 1;
        let positions = &mut self.positions[self.parity];
        // The blended vertices, skinned straight into this frame's MEM2 positions (the GPU may still read the other
        // set); the rigid ones stay as loaded.
        vmod::skin_listed_into(&self.vm, &self.skin, &self.blended, positions);
        unsafe {
            ffi::mwf_flush(positions.as_ptr().cast(), (positions.len() * 12) as u32);
        }
        if left_drawn && let Some(l) = self.left.as_mut() {
            let lp = &mut l.positions[self.parity];
            vmod::skin_listed_into(&self.vm, &l.skin, &self.blended, lp);
            unsafe { ffi::mwf_flush(lp.as_ptr().cast(), (lp.len() * 12) as u32) };
        }
        unsafe { ffi::mwf_view_begin(FOVY, ZNEAR, ZFAR) };
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
        self.last_eye = Some(eye);
        let positions = self.positions[self.parity].as_ptr();
        draw_copy(
            gfx,
            &self.draws,
            &self.skin,
            &eye,
            self.tint,
            positions,
            self.attributes,
        );
        // The left gun: the same model, placed the same way (its clips put it in the left hand).
        if left_drawn && let Some(l) = self.left.as_ref() {
            let lp = l.positions[self.parity].as_ptr();
            draw_copy(
                gfx,
                &self.draws,
                &l.skin,
                &eye,
                self.tint,
                lp,
                self.attributes,
            );
        }
    }
}

/// Draw the model's display lists once: `positions` (this copy's skinned positions) with `attributes`, each rigid
/// part placed by its bone's `skin` matrix about `eye`.
fn draw_copy(
    gfx: &mut impl Gfx,
    draws: &[Draw],
    skin: &[Mtx34],
    eye: &Mtx34,
    tint: u32,
    positions: *const [f32; 3],
    attributes: *const u8,
) {
    unsafe { ffi::mwf_view_arrays_indexed(positions.cast(), attributes) };
    let mut last: Option<DrawState> = None;
    let mut loaded: [u16; vmod::MATRIX_SLOTS] = [u16::MAX - 1; vmod::MATRIX_SLOTS];
    for d in draws {
        // This draw's matrices: model to eye, times the bone's skin matrix for rigid parts.
        for (slot, &key) in d.slots.iter().enumerate() {
            if loaded[slot] == key {
                continue;
            }
            loaded[slot] = key;
            let m = match skin.get(usize::from(key)) {
                Some(s) if key != vmod::BLENDED => vmod::mul(eye, s),
                _ => *eye,
            };
            unsafe { ffi::mwf_pos_matrix_slot(&m, slot as u32) };
        }
        let state = DrawState { tint, ..d.state };
        if last != Some(state) {
            gfx.world_state(&state);
            last = Some(state);
        }
        gfx.call_dl(d.dl);
    }
}

impl LeftGun {
    /// Positions for `n` vertices (two sets in MEM2, the rigid ones in model space as loaded), or None when MEM2 is
    /// short.
    fn new(n: usize, vm: &ViewModel) -> Option<LeftGun> {
        let set = || -> Option<&'static mut [[f32; 3]]> {
            let b = mem2(n * 12)?;
            let p =
                unsafe { core::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<[f32; 3]>(), n) };
            for (o, v) in p.iter_mut().zip(&vm.verts) {
                *o = v.pos;
            }
            unsafe { ffi::mwf_flush(p.as_ptr().cast(), (n * 12) as u32) };
            Some(p)
        };
        Some(LeftGun {
            positions: [set()?, set()?],
            anim: None,
            playing: Vec::with_capacity(8),
            world: Vec::with_capacity(vm.bones.len()),
            skin: Vec::with_capacity(vm.bones.len()),
            last_frac: None,
        })
    }
}

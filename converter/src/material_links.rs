//! Who uses each image: walk a zone's material → image links and the assets that use each material.
//!
//! IW4L reports every asset pointer it follows: `loaded(ty, slot, insert_slot)` when the asset body is inline, and
//! `alias(ty, slot, target)` when the slot points at one loaded earlier. Mapping slots to assets (the same scheme as
//! IW4L's `assets/src/lane/helpers.rs`) links each material texture slot to its image.
//!
//! Models: each XModel's own material handle array names its materials, wherever the model loads (top level, inside a
//! weapon, inside the world). A model is a viewmodel when a weapon lists it as a gun or hand model, a character when
//! its skeleton has human bones, otherwise a prop.
//!
//! Other users: a material is "used by" the top-level zone entry whose load referenced it: GfxWorld (world surfaces
//! and skies), Fx/ImpactFx/FxWorld (effects). In the retail x86 zones those are top-level entries.

use std::collections::{BTreeSet, HashMap};

use asset_iw4::material::{
    CAMERA_REGION_DEPTH_HACK, CAMERA_REGION_EMISSIVE, SORT_KEY_SKY, SORT_KEY_SKYBOX,
};
use asset_iw4::state_bits::{Gfxs0AlphaTest, alpha_test_from_state_bits};
use fastfile_iw4::{AssetType, Ptr, ScriptStrings, ZonePtr, ZoneStream};
use gx_texture::policy::Role;

/// Material texture semantics that carry colour the Wii keeps (2D, function, colour map).
const COLOUR_SEMANTICS: [u8; 3] = [0, 1, 2];
/// `MaterialTextureDef.semantic` value for water (its image sits inside a water struct).
const TS_WATER_MAP: u8 = 11;

/// Technique types whose state bits decide how a material looks: the colour band from unlit (4) up, first present
/// wins, as IW4L's renderer reads it (`asset_iw4::color_pass_agreement`). The later "lit" slots (37–39) are additive
/// per-light passes, not the base look.
const COLOUR_TECHNIQUES: core::ops::Range<usize> =
    asset_iw4::material::TECHNIQUE_COLOR_BAND_FIRST..asset_iw4::size::TECHNIQUE_SLOT_COUNT;

/// Bones that only a human skeleton has (body or head).
const HUMAN_BONES: [&str; 4] = ["j_spinelower", "j_spineupper", "j_head", "j_neck"];

/// The zone entry that referenced a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    World,
    /// An XModel, by index into `Links::models`.
    Model(usize),
    Effect,
    /// A menu or menu list.
    Menu,
    /// A weapon (HUD icons such as kill and d-pad icons), by index into `Links::weapons`.
    Weapon(usize),
    /// The material's own top-level entry (used by code, script or menus, not by anything in this zone).
    Standalone,
    Other,
}

impl Owner {
    /// Owner credited for material references made while loading a top-level entry. Models are credited through
    /// their own material handles instead, so an XModel entry counts as `Other` here.
    pub fn for_entry(ty: AssetType) -> Owner {
        match ty {
            AssetType::GfxWorld => Owner::World,
            AssetType::Fx | AssetType::ImpactFx | AssetType::FxWorld => Owner::Effect,
            AssetType::Material => Owner::Standalone,
            AssetType::Menu | AssetType::MenuList => Owner::Menu,
            _ => Owner::Other,
        }
    }
}

/// How a material uses its colour textures' alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlphaUse {
    /// Opaque or additive: alpha is never read. The texture can drop it.
    Unused,
    /// Alpha test: only "alpha >= threshold" matters, so alpha can be cut to 1 bit.
    Cutout { threshold: u8 },
    /// Alpha blending reads alpha smoothly.
    Blend,
}

#[derive(Clone, Debug)]
pub struct MaterialInfo {
    pub name: Option<String>,
    pub camera_region: u8,
    pub sort_key: u8,
    pub techset: Option<String>,
    pub alpha: AlphaUse,
    /// Framebuffer blend of the material's colour passes.
    pub blend: BlendMode,
    /// Faces culled: 0 none, 1 back, 2 front (IW4 state bits).
    pub cull: u8,
    /// Polygon offset level 0..3 (IW4 state bits word 1); decals use it.
    pub polygon_offset: u8,
    /// (semantic, image slot) per texture.
    textures: Vec<(u8, Ptr)>,
    pub users: BTreeSet<Owner>,
}

/// How a material's colour passes write the framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    Opaque,
    /// Alpha test at this threshold.
    AlphaTest(u8),
    /// src·a + dst·(1−a).
    Blend,
    /// src + dst (or src·a + dst).
    Additive,
    /// src·dst.
    Multiply,
}

#[derive(Clone, Debug, Default)]
pub struct ModelInfo {
    pub name: String,
    pub human: bool,
    /// A weapon lists it as a gun or hand (first-person) model.
    pub viewmodel: bool,
    /// The weapons (indices into `Links::weapons`) that list it.
    pub weapons: BTreeSet<usize>,
    /// Indices into `Links::material_info`.
    pub materials: Vec<usize>,
}

/// A weapon's first-person models (names) and its animation per IW4 slot (`weapAnimFiles`).
#[derive(Clone, Debug, Default)]
pub struct WeaponView {
    pub gun: Option<String>,
    pub hands: Option<String>,
    pub anims: Vec<Option<String>>,
    /// Bone names whose surfaces are hidden (attachment parts the base weapon doesn't show).
    pub hide: Vec<String>,
    /// Player-view sound aliases and the notetrack sound map.
    pub sounds: crate::sounds::WeaponSoundNames,
}

/// Everything a zone says about one image's use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageUsage {
    pub role: Role,
    pub alpha: AlphaUse,
    /// How the role was decided, for the report.
    pub source: &'static str,
    /// When the texture has to be in memory.
    pub group: Group,
}

/// When a texture has to be in memory (load by need).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Throughout a match: world, props, characters, effects, HUD.
    Match,
    /// While one of these weapons (indices into `Links::weapons`) is in someone's loadout: first-person models and
    /// the weapon's HUD icons.
    Weapons(BTreeSet<usize>),
    /// Looked up by name from a string table (calling cards, emblems, icon tables) when shown.
    Catalog,
    /// Front-end menus only.
    Menu,
}

impl Group {
    pub fn label(&self) -> &'static str {
        match self {
            Group::Match => "match",
            Group::Weapons(_) => "weapon",
            Group::Catalog => "catalog",
            Group::Menu => "menu",
        }
    }
}

/// One kind of asset's slot → index links.
#[derive(Default)]
struct SlotMap {
    direct: HashMap<Ptr, usize>,
    aliases: HashMap<Ptr, Ptr>,
}

impl SlotMap {
    // Slots in IW4L's TEMP block are reused by later assets, so the newest link for a slot replaces any older one.
    fn loaded(&mut self, slot: Ptr, insert_slot: Option<Ptr>, index: usize) {
        for s in std::iter::once(slot).chain(insert_slot) {
            self.aliases.remove(&s);
            self.direct.insert(s, index);
        }
    }

    fn alias(&mut self, slot: Ptr, target: Ptr) {
        self.direct.remove(&slot);
        self.aliases.insert(slot, target);
    }

    fn resolve(&self, slot: Ptr) -> Option<usize> {
        let mut current = slot;
        for _ in 0..=self.aliases.len() {
            if let Some(&i) = self.direct.get(&current) {
                return Some(i);
            }
            current = *self.aliases.get(&current)?;
        }
        None
    }
}

/// An effect's sound aliases and the effects it starts (runners, impact / death / emitted effects).
#[derive(Default)]
struct FxSounds {
    sounds: Vec<String>,
    children: Vec<String>,
    /// The effect's looping life (IW4 `msecLoopingLife`; 0: none).
    looping_life_ms: i32,
    /// Its sight-blocking elements (smoke).
    blockers: Vec<FxBlocker>,
}

/// An effect element that blocks sight (IW4L `fx_iw4::FX_ELEM_FLAG_VIS_BLOCKER`): each particle is a sphere of its
/// size that takes its alpha off the visibility of lines through it.
#[derive(Clone, Copy, Debug, Default)]
pub struct FxBlocker {
    /// Looping (spawns every `spawn_a` ms, `spawn_b` at once) or one-shot (`spawn_a` + rand × `spawn_b` particles).
    pub looping: bool,
    pub spawn_a: i32,
    pub spawn_b: i32,
    pub delay_ms: [i32; 2],
    pub life_ms: [i32; 2],
    /// Spawn offset from the effect's origin: radius and height (base, amplitude).
    pub offset_radius: [f32; 2],
    pub offset_height: [f32; 2],
    /// Distance fade in / out from the viewer (base, amplitude).
    pub fade_in: [f32; 2],
    pub fade_out: [f32; 2],
    /// Size (radius) and alpha at 0, 1/4, 1/2, 3/4 and the end of a particle's life (mid random).
    pub size: [f32; 5],
    pub alpha: [u8; 5],
}

/// How far down child effects are searched for a sound.
const FX_DEPTH: u32 = 4;

/// The element at `p` as a sight blocker, when it is one (IW4L `fx_iw4`: the element view, its visual samples).
fn fx_blocker(s: &ZoneStream<'_>, p: Ptr, looping: bool) -> Option<FxBlocker> {
    let x64 = s.wire_format() == fastfile_iw4::Iw4WireFormat::X64;
    let raw = s
        .slice_at(p, 0, s.layout(asset_iw4::size::FX_ELEM_DEF, 288))
        .ok()?;
    let v = if x64 {
        fx_iw4::elem_def_view_x64(raw)?
    } else {
        fx_iw4::elem_def_view(raw)?
    };
    if v.flags & fx_iw4::FX_ELEM_FLAG_VIS_BLOCKER == 0 {
        return None;
    }
    let count = usize::from(v.vis_state_interval_count) + 1;
    let samples = match s.ptr_at(p, s.layout(0xb8, 192)) {
        Ok(fastfile_iw4::ZonePtr::Offset(a)) => s
            .slice_at(
                s.resolve_alias(a),
                0,
                count * fx_iw4::FX_ELEM_VIS_STATE_SAMPLE_SIZE,
            )
            .ok()?,
        _ => return None,
    };
    let at = |k: usize| k as f32 / 4.0;
    let n = v.vis_state_interval_count;
    Some(FxBlocker {
        looping,
        spawn_a: v.spawn_a,
        spawn_b: v.spawn_b,
        delay_ms: [v.spawn_delay_msec_base, v.spawn_delay_msec_amplitude],
        life_ms: [v.life_span_msec_base, v.life_span_msec_amplitude],
        offset_radius: [v.spawn_offset_radius_base, v.spawn_offset_radius_amplitude],
        offset_height: [v.spawn_offset_height_base, v.spawn_offset_height_amplitude],
        fade_in: v.fade_in_range,
        fade_out: v.fade_out_range,
        size: core::array::from_fn(|k| {
            fx_iw4::evaluate_size0(samples, n, at(k), 0.5).unwrap_or(0.0)
        }),
        alpha: core::array::from_fn(|k| {
            fx_iw4::evaluate_vis_alpha(samples, n, at(k), 0.5).unwrap_or(0)
        }),
    })
}

#[derive(Default)]
pub struct Links {
    pub owner: Option<Owner>,
    strings: Option<ScriptStrings>,
    images: SlotMap,
    materials: SlotMap,
    techsets: SlotMap,
    techset_names: Vec<Option<String>>,
    /// Technique slot mask of each technique set (bit n: technique type n exists).
    techset_slots: Vec<u64>,
    pub material_info: Vec<MaterialInfo>,
    xmodels: SlotMap,
    pub models: Vec<ModelInfo>,
    /// Each model's geometry pointers (parallel to `models`), read again after the walk for static models.
    pub model_geometry: Vec<fastfile_iw4::XModelGeometry>,
    /// Model name pointers by slot, for IW4L's weapon loader.
    xmodel_names: HashMap<Ptr, Ptr>,
    /// XModelSurfs (LOD surface sets) by insert slot: surface array and name, for models that share them.
    xmodel_surfaces: HashMap<Ptr, Ptr>,
    xmodel_surface_names: HashMap<Ptr, Ptr>,
    /// Gun and hand model names → the weapons that list them.
    viewmodel_names: HashMap<String, BTreeSet<usize>>,
    /// Third-person (world) model names → the weapons that list them.
    weapon_world_names: HashMap<String, BTreeSet<usize>>,
    /// Weapon names, in load order (`Owner::Weapon` indexes this).
    pub weapons: Vec<String>,
    /// Combat numbers of each weapon (parallel to `weapons`).
    pub weapon_combat: Vec<weapon_iw4::CapturedCombatInput>,
    /// Recoil of each weapon (parallel to `weapons`).
    pub weapon_kick: Vec<wpk::weapon::KickRecord>,
    /// Movement scales of each weapon (parallel to `weapons`).
    pub weapon_moves: Vec<wpk::weapon::MoveRecord>,
    /// ADS zoom and aim-assist ranges of each weapon (parallel to `weapons`).
    pub weapon_aims: Vec<wpk::weapon::AimRecord>,
    /// How each first-person weapon moves (parallel to `weapons`).
    pub weapon_fpv: Vec<wpk::weapon::FpvRecord>,
    /// Thrown / launched weapons' numbers (parallel to `weapons`).
    pub weapon_equip: Vec<wpk::weapon::EquipRecord>,
    /// Each weapon's world sound alias names (explosion, bounce, pull back, throw; parallel to `weapons`).
    pub weapon_world_sounds: Vec<[Option<String>; 4]>,
    /// Each effect's sounds and child effects, by name (lower case).
    fx_defs: HashMap<String, FxSounds>,
    /// Effect names by asset slot.
    fx_slots: HashMap<Ptr, String>,
    /// The effect captured last, waiting for its slot.
    last_fx_name: Option<String>,
    /// Each weapon's IW4 impact type (parallel to `weapons`).
    pub weapon_impact_types: Vec<i32>,
    /// Each weapon's explosion effect's sight blockers (parallel to `weapons`).
    pub weapon_blockers: Vec<Vec<FxBlocker>>,
    /// First-person models and animations of each weapon (parallel to `weapons`).
    pub weapon_views: Vec<WeaponView>,
    /// Every string table of the zone, by name (lower case).
    pub tables: HashMap<String, crate::ui_tables::Table>,
    /// Localized strings (`LocalizeEntry`) by reference name.
    pub localize: HashMap<String, String>,
    /// MW2's per-hit-location damage table (`info/mp_lochit_dmgtable`), when the zone has it.
    pub lochit: Option<[f32; weapon_iw4::HITLOC_COUNT]>,
    /// Fonts and menus of the zone.
    pub ui: crate::ui_capture::UiCapture,
    /// The game-type scripts the match rules come from (`rules::SCRIPTS`), by rawfile name.
    pub scripts: HashMap<String, String>,
    /// Sound aliases and loaded sounds of the zone.
    pub sounds: crate::sounds::Capture,
    /// Every XAnim loaded, by name, read again after the walk (`viewmodel.rs`).
    pub xanims: Vec<(String, fastfile_iw4::XAnimPartsGeometry)>,
    /// Every string-table cell in the zone (material names found here are looked up by name).
    table_strings: std::collections::HashSet<String>,
    /// Materials the zone's scripts pass to `setupMiniMap(...)` (lower case).
    pub minimap_materials: std::collections::HashSet<String>,
    /// The map script's `compassmaxrange` (world units the HUD minimap's height covers), when it sets one.
    pub compass_max_range: Option<f32>,
    /// Image index assigned to each loaded image, in load order (filled by the caller through `image_loaded`).
    image_count: usize,
    pub unresolved_texture_links: usize,
    /// Model material handles and weapon model slots that did not resolve.
    pub unresolved_model_links: usize,
}

impl Links {
    /// A script string (bone and track names) by id.
    pub fn script_string<'s>(&self, s: &'s ZoneStream<'_>, id: u16) -> Option<&'s str> {
        self.strings.as_ref().and_then(|t| t.get(s, id))
    }

    /// Hook for `AssetLinkSink::capture_xanim`.
    pub fn xanim_loaded(&mut self, s: &ZoneStream<'_>, g: fastfile_iw4::XAnimPartsGeometry) {
        if let Some(name) = g.name.and_then(|p| s.cstr(s.resolve_alias(p)).ok()) {
            self.xanims.push((name.to_owned(), g));
        }
    }

    pub fn set_strings(&mut self, strings: ScriptStrings) {
        self.strings = Some(strings);
    }

    /// Record an inline image load; returns its index.
    pub fn image_loaded(&mut self, slot: Ptr, insert_slot: Option<Ptr>) -> usize {
        let i = self.image_count;
        self.images.loaded(slot, insert_slot, i);
        self.image_count += 1;
        i
    }

    fn use_material(&mut self, index: usize) {
        if let (Some(owner), Some(m)) = (self.owner, self.material_info.get_mut(index)) {
            m.users.insert(owner);
        }
    }

    fn material_loaded(&mut self, s: &ZoneStream<'_>, slot: Ptr, insert_slot: Option<Ptr>) {
        let Some(g) = s.latest_material() else {
            return;
        };
        let mut textures = Vec::new();
        if let Some(arr) = g.textures {
            for i in 0..g.texture_count {
                let def = arr.at(i * g.texture_stride);
                if let Ok(semantic) = s.u8_at(def, 7)
                    && semantic != TS_WATER_MAP
                {
                    textures.push((semantic, def.at(8)));
                }
            }
        }
        let rows: Vec<[u32; 2]> = g
            .state_bits
            .map(|p| {
                (0..g.state_bits_count)
                    .filter_map(|i| Some([s.u32_at(p, i * 8).ok()?, s.u32_at(p, i * 8 + 4).ok()?]))
                    .collect()
            })
            .unwrap_or_default();
        let techset_index = g.technique_set.and_then(|slot| self.techsets.resolve(slot));
        let techset = techset_index.and_then(|i| self.techset_names.get(i).cloned().flatten());
        // A material carries state bits for every technique (depth prepass, shadow, lit, ...). Blending and alpha
        // test come from the colour pass: the first lit technique, else unlit/emissive (IW4L's colour band).
        let slots = techset_index.and_then(|i| self.techset_slots.get(i).copied());
        let colour_row = slots.and_then(|slots| {
            COLOUR_TECHNIQUES.clone().find_map(|tech| {
                asset_iw4::material::color_pass_row_for_tech_type(
                    g.state_bits_entry.as_ref(),
                    &rows,
                    slots,
                    tech,
                )
            })
        });
        // A technique set from another zone (its name starts with ',') arrives without its technique list, so the
        // above finds nothing. The material's own per-technique table still says which row each technique uses
        // (0xFF: technique absent): take the first colour-band technique it has. Without this, every row counts,
        // and the additive light passes make lit models see-through.
        let colour_row = colour_row.or_else(|| {
            let entries = g.state_bits_entry.as_ref()?;
            COLOUR_TECHNIQUES
                .clone()
                .find(|&t| entries[t] != 0xFF)
                .and_then(|t| rows.get(usize::from(entries[t])).copied())
        });
        let pass_rows: &[[u32; 2]] = match &colour_row {
            Some(r) => core::slice::from_ref(r),
            None => &rows,
        };
        let name = g
            .name
            .and_then(|p| s.cstr(s.resolve_alias(p)).ok())
            .map(str::to_owned);
        let index = self.material_info.len();
        self.material_info.push(MaterialInfo {
            name,
            camera_region: g.camera_region,
            sort_key: g.sort_key,
            techset,
            alpha: alpha_use(pass_rows),
            blend: blend_mode(pass_rows),
            cull: pass_rows.first().map_or(0, |&r| {
                match asset_iw4::state_bits::cull_face_from_state_bits(r) {
                    asset_iw4::state_bits::Gfxs0CullFace::None => 0,
                    asset_iw4::state_bits::Gfxs0CullFace::Back => 1,
                    asset_iw4::state_bits::Gfxs0CullFace::Front => 2,
                }
            }),
            polygon_offset: pass_rows.first().map_or(0, |r| {
                ((r[1] & asset_iw4::GFXS1_POLYGON_OFFSET_MASK)
                    >> asset_iw4::GFXS1_POLYGON_OFFSET_SHIFT) as u8
            }),
            textures,
            users: BTreeSet::new(),
        });
        self.materials.loaded(slot, insert_slot, index);
        self.use_material(index);
    }

    fn xmodel_loaded(&mut self, s: &ZoneStream<'_>, slot: Ptr, insert_slot: Option<Ptr>) {
        let Some(g) = s.xmodel() else {
            return;
        };
        let human = self.strings.is_some_and(|strings| {
            g.bone_names.is_some_and(|names| {
                (0..g.num_bones).any(|i| {
                    s.u16_at(names, i * 2)
                        .ok()
                        .and_then(|id| strings.get(s, id))
                        .is_some_and(|bone| HUMAN_BONES.contains(&bone))
                })
            })
        });
        let mut materials = Vec::new();
        if let Some(handles) = g.material_handles {
            for i in 0..g.material_handle_count {
                match self.materials.resolve(handles.at(i * s.pointer_bytes())) {
                    Some(m) => materials.push(m),
                    None => self.unresolved_model_links += 1,
                }
            }
        }
        let name = g
            .name
            .and_then(|p| s.cstr(s.resolve_alias(p)).ok())
            .unwrap_or_default()
            .to_owned();
        let index = self.models.len();
        self.model_geometry.push(g);
        self.models.push(ModelInfo {
            name,
            human,
            viewmodel: false,
            weapons: BTreeSet::new(),
            materials,
        });
        self.xmodels.loaded(slot, insert_slot, index);
        self.credit_model_to_weapon(index);
    }

    /// A weapon's gun and hand models are what the player sees in first person. IW4L reports them as model names
    /// (through the `remember_xmodel_name` / `xmodel_name_ptr` hooks), so they are matched to models by name.
    fn weapon_loaded(&mut self, s: &ZoneStream<'_>) {
        let Some(w) = s.weapon() else {
            return;
        };
        let text = |p: Ptr| s.cstr(s.resolve_alias(p)).ok().map(str::to_owned);
        // The driver set `Owner::Weapon(n)` for this entry, n = the weapon count before it.
        let index = self.weapons.len();
        self.weapons.push(w.name.and_then(text).unwrap_or_default());
        self.weapon_combat.push(crate::weapons::captured(&w));
        self.weapon_kick.push(crate::weapons::kick(&w));
        self.weapon_moves.push(crate::weapons::moves(&w));
        self.weapon_aims.push(crate::weapons::aims(&w));
        self.weapon_fpv.push(crate::weapons::fpv(&w));
        self.weapon_equip.push(crate::weapons::equip(&w));
        let anims = w.sz_xanims.map_or_else(Vec::new, |arr| {
            (0..asset_iw4::size::WEAPON_ANIM_COUNT)
                .map(|i| match s.ptr_at(arr, i * s.pointer_bytes()) {
                    Ok(fastfile_iw4::ZonePtr::Offset(q)) => s
                        .cstr(s.resolve_alias(q))
                        .ok()
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned),
                    _ => None,
                })
                .collect()
        });
        // Bones whose surfaces this weapon hides (IW4 hideTags: 32 script strings).
        let hide = w.hide_tags.map_or_else(Vec::new, |arr| {
            (0..32)
                .filter_map(|i| s.u16_at(arr, i * 2).ok())
                .filter(|&id| id != 0)
                .filter_map(|id| self.script_string(s, id))
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect()
        });
        // Player-view sounds (IW4L `WeaponSoundAliases`), in `wpk::sound::event` order.
        let direct = |p: Option<Ptr>| {
            p.and_then(|p| s.cstr(p).ok())
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        };
        let events = [
            w.fire_sound_player_name,
            w.fire_last_sound_player_name,
            w.empty_fire_sound_player_name,
            w.reload_sound_player_name,
            w.reload_empty_sound_player_name,
            w.raise_sound_player_name,
            w.first_raise_sound_player_name,
            w.putaway_sound_player_name,
            w.melee_swipe_sound_player_name,
        ]
        .map(direct);
        // A thrown weapon's sounds: its explosion, the first surface bounce it names, its pull back and throw.
        let bounce = w.bounce_sound_names.iter().copied().find_map(direct);
        // Most explosives name no explosion sound: theirs plays from the explosion effect's sound element.
        let effect_sound = w
            .explosion_slot
            .and_then(|slot| self.fx_slots.get(&slot))
            .and_then(|n| self.fx_sound(n, FX_DEPTH));
        self.weapon_impact_types.push(w.impact_type);
        // Its explosion effect's sight blockers (smoke).
        let mut blockers = Vec::new();
        if let Some(n) = w.explosion_slot.and_then(|slot| self.fx_slots.get(&slot)) {
            self.fx_blockers(n, FX_DEPTH, &mut blockers);
        }
        self.weapon_blockers
            .push(blockers.into_iter().map(|(b, _)| b).collect());
        self.weapon_world_sounds.push([
            direct(w.proj_explosion_sound_name).or(effect_sound),
            bounce,
            direct(w.pullback_sound_player_name),
            direct(w.fire_sound_player_name),
        ]);
        // notetrackSoundMap: 16 script-string pairs (IW4L `read_script_string_map`).
        let mut note_map = Vec::new();
        if let (Some(keys), Some(values)) = (w.notetrack_sound_keys, w.notetrack_sound_values) {
            for i in 0..16 {
                let (Ok(k), Ok(v)) = (s.u16_at(keys, i * 2), s.u16_at(values, i * 2)) else {
                    break;
                };
                if k == 0 {
                    break;
                }
                let k = self.script_string(s, k).filter(|n| !n.is_empty());
                let v = self.script_string(s, v).filter(|n| !n.is_empty());
                if let (Some(k), Some(v)) = (k, v) {
                    note_map.push((k.to_owned(), v.to_owned()));
                }
            }
        }
        self.weapon_views.push(WeaponView {
            gun: w.gun_xmodel_name.and_then(text),
            hands: w.hand_xmodel_name.and_then(text),
            anims,
            hide,
            sounds: crate::sounds::WeaponSoundNames { events, note_map },
        });
        for name in w
            .gun_xmodel_names
            .into_iter()
            .chain([w.hand_xmodel_name])
            .flatten()
        {
            if let Some(n) = text(name) {
                self.viewmodel_names.entry(n).or_default().insert(index);
            }
        }
        for name in w.world_model_names.into_iter().flatten() {
            if let Some(n) = text(name) {
                self.weapon_world_names.entry(n).or_default().insert(index);
            }
        }
    }

    /// The name of the material loaded or aliased into `slot` (menus name their background materials this way).
    pub fn material_name(&self, slot: Ptr) -> Option<&str> {
        let i = self.materials.resolve(slot)?;
        self.material_info.get(i)?.name.as_deref()
    }

    /// The material a zone pointer slot (e.g. `GfxSurface.material`) refers to.
    /// The model (index into `models`) loaded or aliased into `slot`.
    pub fn resolve_model(&self, slot: Ptr) -> Option<usize> {
        self.xmodels.resolve(slot)
    }

    pub fn resolve_material(&self, slot: Ptr) -> Option<usize> {
        self.materials.resolve(slot)
    }

    /// The image (load index) a material samples as colour: its first texture with a colour semantic.
    pub fn colour_image(&self, material: usize) -> Option<usize> {
        let m = self.material_info.get(material)?;
        m.textures
            .iter()
            .filter(|(semantic, _)| COLOUR_SEMANTICS.contains(semantic))
            .find_map(|&(_, slot)| self.images.resolve(slot))
    }

    /// Hook for `AssetLinkSink::capture_raw_file`: a map's script names its HUD minimap material in
    /// `maps\mp\_compass::setupMiniMap("<material>")`.
    pub fn raw_file(&mut self, data: &[u8], zlib_compressed: bool) {
        let text = if zlib_compressed {
            match asset_transport::inflate_zlib(data) {
                Ok(t) => t,
                Err(_) => return,
            }
        } else {
            data.to_vec()
        };
        let text = String::from_utf8_lossy(&text);
        self.minimap_materials.extend(minimap_calls(&text));
        if let Some(r) = compass_max_range(&text) {
            self.compass_max_range = Some(r);
        }
    }

    /// Add string-table cells from another zone (the shared tables in `code_post_gfx_mp`).
    pub fn add_table_strings(&mut self, strings: impl IntoIterator<Item = String>) {
        self.table_strings.extend(strings);
    }

    /// The string-table cells seen so far (lower case).
    pub fn take_table_strings(&mut self) -> std::collections::HashSet<String> {
        std::mem::take(&mut self.table_strings)
    }

    /// Hook for `AssetLinkSink::capture_string_table`: remember every cell, so materials named in a table are known.
    pub fn string_table(&mut self, s: &ZoneStream<'_>, header: Ptr) {
        let pb = s.pointer_bytes();
        let table_name = match s.ptr_at(header, 0) {
            Ok(ZonePtr::Offset(p)) => s.cstr(s.resolve_alias(p)).ok().map(str::to_ascii_lowercase),
            _ => None,
        };
        let mut cells = Vec::new();
        let columns = s.i32_at(header, pb).unwrap_or(0).max(0) as usize;
        let rows = s.i32_at(header, pb + 4).unwrap_or(0).max(0) as usize;
        // StringTableCell is { const char *string; int hash } (x86: 8 bytes, x64: 16).
        let cell_size = if pb == 4 { 8 } else { 16 };
        let Ok(ZonePtr::Offset(arr)) = s.ptr_at(header, pb + 8) else {
            return;
        };
        let arr = s.resolve_alias(arr);
        for i in 0..columns.saturating_mul(rows) {
            let text = match s.ptr_at(arr, i * cell_size) {
                Ok(ZonePtr::Offset(p)) => s.cstr(s.resolve_alias(p)).unwrap_or(""),
                _ => "",
            };
            if !text.is_empty() {
                self.table_strings.insert(text.to_ascii_lowercase());
            }
            cells.push(text.to_owned());
        }
        if let Some(n) = table_name {
            self.tables
                .entry(n)
                .or_insert(crate::ui_tables::Table { columns, cells });
        }
    }

    /// A model referenced while a weapon loads belongs to that weapon: gun, hand and world models, and also the
    /// world clip, knife, rocket and projectile models, which IW4L loads without recording a name.
    fn credit_model_to_weapon(&mut self, model: usize) {
        if let (Some(Owner::Weapon(w)), Some(m)) = (self.owner, self.models.get_mut(model)) {
            m.weapons.insert(w);
        }
    }

    /// Hook for `AssetLinkSink::remember_xmodel_name`.
    pub fn remember_xmodel_name(&mut self, slot: Ptr, insert_slot: Option<Ptr>, name: Ptr) {
        for s in std::iter::once(slot).chain(insert_slot) {
            self.xmodel_names.insert(s, name);
        }
    }

    /// Hooks for IW4L's shared XModelSurfs (a later model's LOD points at a set an earlier model loaded).
    pub fn remember_xmodel_surfaces(&mut self, slot: Ptr, surfaces: Ptr) {
        self.xmodel_surfaces.insert(slot, surfaces);
    }
    pub fn remember_xmodel_surface_name(&mut self, slot: Ptr, name: Ptr) {
        self.xmodel_surface_names.insert(slot, name);
    }
    pub fn xmodel_surfaces(&self, slot: Ptr) -> Option<Ptr> {
        self.xmodel_surfaces.get(&slot).copied()
    }
    pub fn xmodel_surface_name(&self, slot: Ptr) -> Option<Ptr> {
        self.xmodel_surface_names.get(&slot).copied()
    }

    /// Hook for `AssetLinkSink::xmodel_name_ptr`.
    pub fn xmodel_name_ptr(&self, slot: Ptr) -> Option<Ptr> {
        self.xmodel_names.get(&slot).copied()
    }

    /// Hook for `AssetLinkSink::loaded` (images are recorded by the caller through `image_loaded`).
    pub fn loaded(
        &mut self,
        s: &ZoneStream<'_>,
        ty: AssetType,
        slot: Ptr,
        insert_slot: Option<Ptr>,
    ) {
        match ty {
            AssetType::Material => self.material_loaded(s, slot, insert_slot),
            AssetType::TechniqueSet => {
                let name = s
                    .latest_technique_set()
                    .and_then(|g| g.name)
                    .and_then(|p| s.cstr(s.resolve_alias(p)).ok())
                    .map(str::to_owned);
                let i = self.techset_names.len();
                self.techset_names.push(name);
                self.techset_slots
                    .push(s.latest_technique_set().map_or(0, |g| g.technique_slots));
                self.techsets.loaded(slot, insert_slot, i);
            }
            AssetType::XModel => self.xmodel_loaded(s, slot, insert_slot),
            AssetType::Weapon => self.weapon_loaded(s),
            AssetType::LoadedSound => self.sounds.loaded_slot(slot, insert_slot),
            AssetType::Fx => {
                if let Some(n) = self.last_fx_name.take() {
                    if let Some(i) = insert_slot {
                        self.fx_slots.insert(i, n.clone());
                    }
                    self.fx_slots.insert(slot, n);
                }
            }
            _ => {}
        }
    }

    /// Hook for `AssetLinkSink::capture_fx`: the alias names of the effect's sound elements (IW4L
    /// `fastfile_iw4::load::fx` layout: element type at 176, visual count at 177, visuals at 188 / 200 x64; a
    /// sound visual is a pointer to its alias name, an array of them when there are several).
    pub fn fx_captured(&mut self, s: &ZoneStream<'_>, g: &fastfile_iw4::FxEffectDefGeometry) {
        use asset_iw4::size as sz;
        let name = |p: Ptr, field: usize| match s.ptr_at(p, field) {
            Ok(fastfile_iw4::ZonePtr::Offset(q)) => s
                .cstr(s.resolve_alias(q))
                .ok()
                .filter(|n| !n.is_empty())
                .map(str::to_owned),
            _ => None,
        };
        let Some(own) = g
            .name
            .and_then(|p| s.cstr(s.resolve_alias(p)).ok())
            .filter(|n| !n.is_empty())
            .map(str::to_ascii_lowercase)
        else {
            return;
        };
        let mut fx = FxSounds::default();
        if let Some(arr) = g.elem_defs {
            for i in 0..g.elem_def_count {
                let p = arr.at(i * s.layout(sz::FX_ELEM_DEF, 288));
                // Effects the element starts on impact, on death and while it lives.
                for field in [s.layout(216, 232), s.layout(220, 240), s.layout(224, 248)] {
                    fx.children.extend(name(p, field));
                }
                if let Some(b) = fx_blocker(s, p, i < g.looping_count.max(0) as usize) {
                    fx.blockers.push(b);
                }
                let (Ok(ty), Ok(n)) = (s.u8_at(p, 176), s.u8_at(p, 177)) else {
                    continue;
                };
                // Sound and runner visuals are names (an alias, an effect).
                let into = match ty {
                    sz::fx_elem::SOUND => &mut fx.sounds,
                    sz::fx_elem::RUNNER => &mut fx.children,
                    _ => continue,
                };
                let vis = p.at(s.layout(188, 200));
                if n == 1 {
                    into.extend(name(vis, 0));
                } else if n > 1
                    && let Ok(fastfile_iw4::ZonePtr::Offset(a)) = s.ptr_at(vis, 0)
                {
                    let a = s.resolve_alias(a);
                    for k in 0..usize::from(n) {
                        into.extend(name(a.at(k * s.layout(sz::FX_ELEM_VISUALS, 8)), 0));
                    }
                }
            }
        }
        for c in &mut fx.children {
            c.make_ascii_lowercase();
        }
        // msecLoopingLife (x64: IW4L reads it at 16; x86 through the header view).
        fx.looping_life_ms = if s.wire_format() == fastfile_iw4::Iw4WireFormat::X64 {
            s.i32_at(g.header, 16).unwrap_or(0)
        } else {
            s.slice_at(g.header, 0, fx_iw4::FX_EFFECT_DEF_SIZE)
                .ok()
                .and_then(fx_iw4::effect_def_view)
                .map_or(0, |v| v.msec_looping_life)
        };
        self.fx_defs.entry(own.clone()).or_insert(fx);
        self.last_fx_name = Some(own);
    }

    /// The sight blockers of effect `name` and its children, each with its effect's looping life.
    pub fn fx_blockers(&self, name: &str, depth: u32, out: &mut Vec<(FxBlocker, i32)>) {
        let Some(fx) = self.fx_defs.get(name) else {
            return;
        };
        out.extend(fx.blockers.iter().map(|b| (*b, fx.looping_life_ms)));
        if depth > 0 {
            for c in &fx.children {
                self.fx_blockers(c, depth - 1, out);
            }
        }
    }

    /// The first sound effect `name` plays, its own or a child effect's (depth-first, a few levels down).
    fn fx_sound(&self, name: &str, depth: u32) -> Option<String> {
        let fx = self.fx_defs.get(name)?;
        if let Some(s) = fx.sounds.first() {
            return Some(s.clone());
        }
        if depth == 0 {
            return None;
        }
        fx.children.iter().find_map(|c| self.fx_sound(c, depth - 1))
    }

    /// The explosion sound weapon `i` plays when neither it nor its explosion effect names one: IW4L
    /// `audio::explosion_surface_aliases` (grenades `grenade_explode_<surface>`, rockets `rocket_explode_<surface>`,
    /// each falling back to `_default`; one sound per weapon here, so the default surface).
    pub fn impact_explosion_sound(&self, i: usize) -> Option<String> {
        let prefix = match self.weapon_impact_types.get(i)? {
            8 => "grenade_explode",
            9 => "rocket_explode",
            _ => return None,
        };
        Some(format!("{prefix}_default"))
    }

    /// Hook for `AssetLinkSink::alias`.
    pub fn alias(&mut self, ty: AssetType, slot: Ptr, target: Ptr) {
        match ty {
            AssetType::Image => self.images.alias(slot, target),
            AssetType::TechniqueSet => self.techsets.alias(slot, target),
            AssetType::XModel => {
                self.xmodels.alias(slot, target);
                if let Some(m) = self.xmodels.resolve(slot) {
                    self.credit_model_to_weapon(m);
                }
            }
            AssetType::Material => {
                self.materials.alias(slot, target);
                if let Some(i) = self.materials.resolve(slot) {
                    self.use_material(i);
                }
            }
            AssetType::LoadedSound => self.sounds.alias_slot(slot, target),
            AssetType::Fx => {
                if let Some(n) = self.fx_slots.get(&target).cloned() {
                    self.fx_slots.insert(slot, n);
                }
            }
            _ => {}
        }
    }

    /// Per image (by load index): its usage, or `None` when no material in the zone uses it as colour.
    /// The second value says whether any material uses the image at all (as colour or not).
    pub fn image_usage(&mut self) -> Vec<(Option<ImageUsage>, bool)> {
        for model in &mut self.models {
            if let Some(w) = self.viewmodel_names.get(&model.name) {
                model.viewmodel = true;
                model.weapons.extend(w.iter().copied());
            }
            // Third-person weapon models only appear while someone carries the weapon.
            if let Some(w) = self.weapon_world_names.get(&model.name) {
                model.weapons.extend(w.iter().copied());
            }
        }
        let known: std::collections::HashSet<&str> =
            self.models.iter().map(|m| m.name.as_str()).collect();
        self.unresolved_model_links += self
            .viewmodel_names
            .keys()
            .filter(|n| !known.contains(n.as_str()))
            .count();
        for (i, model) in self.models.iter().enumerate() {
            for &m in &model.materials {
                if let Some(info) = self.material_info.get_mut(m) {
                    info.users.insert(Owner::Model(i));
                }
            }
        }
        let mut out: Vec<(Option<ImageUsage>, bool)> = vec![(None, false); self.image_count];
        let mut unresolved = 0;
        for m in &self.material_info {
            for &(semantic, slot) in &m.textures {
                let Some(i) = self.images.resolve(slot) else {
                    unresolved += 1;
                    continue;
                };
                let entry = &mut out[i];
                entry.1 = true;
                if !COLOUR_SEMANTICS.contains(&semantic) {
                    continue;
                }
                let (role, source) = material_role(m, &self.models, &self.minimap_materials);
                let usage = ImageUsage {
                    role,
                    alpha: m.alpha,
                    source,
                    group: material_group(m, &self.models, &self.table_strings),
                };
                entry.0 = Some(match entry.0.take() {
                    None => usage,
                    Some(prev) => merge(prev, usage),
                });
            }
        }
        self.unresolved_texture_links = unresolved;
        out
    }
}

/// The framebuffer blend of a material, from its state-bit rows: the first row that blends decides; otherwise alpha
/// test if any row tests; otherwise opaque. D3D blend factors: 1 ZERO, 2 ONE, 3 SRCCOLOR, 4 INVSRCCOLOR, 5 SRCALPHA,
/// 6 INVSRCALPHA, 9 DESTCOLOR.
pub fn blend_mode(rows: &[[u32; 2]]) -> BlendMode {
    for &row in rows {
        let w = row[0];
        if (w >> 8) & 7 == 0 {
            continue;
        }
        let (src, dst) = (w & 0xf, (w >> 4) & 0xf);
        return match (src, dst) {
            (1, 3) | (9, 1) => BlendMode::Multiply,
            (2 | 5, 2) => BlendMode::Additive,
            (2, 1) => BlendMode::Opaque,
            _ => BlendMode::Blend,
        };
    }
    match alpha_use(rows) {
        AlphaUse::Cutout { threshold } => BlendMode::AlphaTest(threshold),
        _ => BlendMode::Opaque,
    }
}

/// How the material's state bits use alpha: any alpha blend wins, then any alpha test, else unused.
pub fn alpha_use(rows: &[[u32; 2]]) -> AlphaUse {
    let mut test: Option<u8> = None;
    for &row in rows {
        let word0 = row[0];
        let op = (word0 >> 8) & 7;
        // D3DBLEND_SRCALPHA = 5, INVSRCALPHA = 6, DESTALPHA = 7, INVDESTALPHA = 8, SRCALPHASAT = 11.
        let reads_alpha = |f: u32| matches!(f, 5..=8 | 11);
        if op != 0 && (reads_alpha(word0 & 0xf) || reads_alpha((word0 >> 4) & 0xf)) {
            return AlphaUse::Blend;
        }
        if let Some(t) = alpha_test_from_state_bits(row) {
            let threshold = match t {
                Gfxs0AlphaTest::GreaterThanZero => 1,
                Gfxs0AlphaTest::LessThan128 | Gfxs0AlphaTest::GreaterEqual128 => 128,
            };
            test = Some(test.map_or(threshold, |t: u8| t.max(threshold)));
        }
    }
    test.map_or(AlphaUse::Unused, |threshold| AlphaUse::Cutout { threshold })
}

/// Role of a material from its users, camera region and sort key.
fn material_role(
    m: &MaterialInfo,
    models: &[ModelInfo],
    minimaps: &std::collections::HashSet<String>,
) -> (Role, &'static str) {
    if m.name
        .as_ref()
        .is_some_and(|n| minimaps.contains(&n.to_ascii_lowercase()))
    {
        return (Role::Minimap, "material: minimap (map script)");
    }
    if m.camera_region == CAMERA_REGION_DEPTH_HACK {
        return (Role::Viewmodel, "material: viewmodel (depth hack)");
    }
    if m.sort_key == SORT_KEY_SKY || m.sort_key == SORT_KEY_SKYBOX {
        return (Role::Sky, "material: sky sort key");
    }
    let mut best: Option<(Role, &'static str)> = None;
    for owner in &m.users {
        let pick = match owner {
            Owner::World => (Role::World, "material: world surface"),
            Owner::Model(i) => match models.get(*i) {
                Some(m) if m.viewmodel => (Role::Viewmodel, "material: weapon gun/hand model"),
                Some(m) if m.human => (Role::Character, "material: human skeleton model"),
                _ => (Role::Prop, "material: model"),
            },
            Owner::Effect => (Role::Effect, "material: effect"),
            Owner::Menu | Owner::Weapon(_) | Owner::Standalone | Owner::Other => continue,
        };
        best = Some(match best {
            Some(b) if rank(b.0) >= rank(pick.0) => b,
            _ => pick,
        });
    }
    // No user in this zone (HUD, menus, script): go by the technique set. IW4 technique set names may start with ','.
    best.unwrap_or_else(|| {
        let techset = m.techset.as_deref().unwrap_or("").trim_start_matches(',');
        if techset.starts_with("2d") {
            (Role::Ui, "material: 2d technique (no user in zone)")
        } else if techset.starts_with("effect") || m.camera_region == CAMERA_REGION_EMISSIVE {
            (Role::Effect, "material: effect technique (no user in zone)")
        } else {
            (Role::World, "material: no user in zone")
        }
    })
}

/// When a material's textures have to be in memory: anything a match always draws (world, non-weapon models,
/// effects) is `Match`; first-person models and weapon HUD icons go with their weapons; then materials named in a
/// string table (looked up by name), then menu-only materials. Anything else (HUD drawn by script or code) is `Match`.
fn material_group(
    m: &MaterialInfo,
    models: &[ModelInfo],
    table_strings: &std::collections::HashSet<String>,
) -> Group {
    let mut weapons = BTreeSet::new();
    let mut menu = false;
    for owner in &m.users {
        match owner {
            Owner::World | Owner::Effect => return Group::Match,
            Owner::Model(i) => match models.get(*i) {
                Some(model) if !model.weapons.is_empty() => {
                    weapons.extend(model.weapons.iter().copied())
                }
                _ => return Group::Match,
            },
            Owner::Weapon(w) => {
                weapons.insert(*w);
            }
            Owner::Menu => menu = true,
            Owner::Standalone | Owner::Other => {}
        }
    }
    let in_table = m
        .name
        .as_ref()
        .is_some_and(|n| table_strings.contains(&n.to_ascii_lowercase()));
    if !weapons.is_empty() {
        Group::Weapons(weapons)
    } else if in_table {
        Group::Catalog
    } else if menu {
        Group::Menu
    } else {
        Group::Match
    }
}

/// Combine two materials' groups for one image: needed at all times beats per weapon beats by name beats menus.
fn merge_group(a: Group, b: Group) -> Group {
    match (a, b) {
        (Group::Match, _) | (_, Group::Match) => Group::Match,
        (Group::Weapons(mut x), Group::Weapons(y)) => {
            x.extend(y);
            Group::Weapons(x)
        }
        (w @ Group::Weapons(_), _) | (_, w @ Group::Weapons(_)) => w,
        (Group::Catalog, _) | (_, Group::Catalog) => Group::Catalog,
        _ => Group::Menu,
    }
}

/// The number a map script sets `compassmaxrange` to (`setdvar( "compassmaxrange", "2200" )`).
pub fn compass_max_range(source: &str) -> Option<f32> {
    let lower = source.to_ascii_lowercase();
    let i = lower.find("\"compassmaxrange\"")?;
    let rest = &lower[i + "\"compassmaxrange\"".len()..];
    let rest = rest.trim_start().strip_prefix(',')?.trim_start();
    let num: String = rest
        .trim_start_matches('"')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok().filter(|v: &f32| *v > 0.0)
}

/// Material names passed as a string literal to `setupMiniMap(` in GSC source (lower case).
pub fn minimap_calls(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let mut out = Vec::new();
    for (i, call) in lower.match_indices("setupminimap(") {
        let rest = lower[i + call.len()..].trim_start();
        if let Some(rest) = rest.strip_prefix('"')
            && let Some(end) = rest.find('"')
            && end > 0
        {
            out.push(rest[..end].to_owned());
        }
    }
    out
}

/// Preference when one image has several roles: the larger size cap, then this fixed order.
fn rank(role: Role) -> (u32, u8) {
    let order = match role {
        Role::Viewmodel => 7,
        Role::Ui => 6,
        Role::Sky => 5,
        Role::Character => 4,
        Role::World => 3,
        Role::Prop => 2,
        Role::Effect => 1,
        Role::Mask => 0,
        Role::Minimap => 8,
        Role::Lightmap => 0,
    };
    (role.max_dim(), order)
}

/// Combine two materials' uses of one image: the stronger role, and the alpha use that needs the most.
fn merge(a: ImageUsage, b: ImageUsage) -> ImageUsage {
    let alpha = match (a.alpha, b.alpha) {
        (AlphaUse::Blend, _) | (_, AlphaUse::Blend) => AlphaUse::Blend,
        (AlphaUse::Cutout { threshold: x }, AlphaUse::Cutout { threshold: y }) => {
            AlphaUse::Cutout {
                threshold: x.min(y),
            }
        }
        (c @ AlphaUse::Cutout { .. }, _) | (_, c @ AlphaUse::Cutout { .. }) => c,
        _ => AlphaUse::Unused,
    };
    let (role, source) = if rank(b.role) > rank(a.role) {
        (b.role, b.source)
    } else {
        (a.role, a.source)
    };
    ImageUsage {
        role,
        alpha,
        source,
        group: merge_group(a.group, b.group),
    }
}

/// Apply a material's alpha use to decoded pixels before encoding.
pub fn apply_alpha(pixels: &mut gx_texture::image::Image, alpha: AlphaUse) {
    match alpha {
        AlphaUse::Unused => pixels.pixels.iter_mut().for_each(|p| p[3] = 255),
        AlphaUse::Cutout { threshold } => pixels
            .pixels
            .iter_mut()
            .for_each(|p| p[3] = if p[3] >= threshold { 255 } else { 0 }),
        AlphaUse::Blend => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ptr(offset: u32) -> Ptr {
        Ptr { block: 3, offset }
    }

    #[test]
    fn slot_aliases_resolve_through_chains() {
        let mut m = SlotMap::default();
        m.loaded(ptr(0), Some(ptr(4)), 7);
        m.alias(ptr(8), ptr(4));
        m.alias(ptr(12), ptr(8));
        assert_eq!(m.resolve(ptr(0)), Some(7));
        assert_eq!(m.resolve(ptr(12)), Some(7));
        assert_eq!(m.resolve(ptr(16)), None);
        // A loop must not hang.
        m.alias(ptr(20), ptr(24));
        m.alias(ptr(24), ptr(20));
        assert_eq!(m.resolve(ptr(20)), None);
        // A reused slot follows its newest link.
        m.loaded(ptr(12), None, 9);
        assert_eq!(m.resolve(ptr(12)), Some(9));
        m.alias(ptr(12), ptr(0));
        assert_eq!(m.resolve(ptr(12)), Some(7));
    }

    #[test]
    fn alpha_use_from_state_bits() {
        // Opaque: blend op disabled, alpha test disabled.
        let opaque = [0x0000_0800 | 0x12, 0];
        assert_eq!(alpha_use(&[opaque]), AlphaUse::Unused);
        // Additive ONE/ONE: blend on, but alpha is never read.
        let additive = [(1 << 8) | (2 << 4) | 2 | 0x800, 0];
        assert_eq!(alpha_use(&[additive]), AlphaUse::Unused);
        // SRCALPHA / INVSRCALPHA blend.
        let blend = [(1 << 8) | (6 << 4) | 5 | 0x800, 0];
        assert_eq!(alpha_use(&[opaque, blend]), AlphaUse::Blend);
        // Alpha test >= 128 (field 3 at bit 12, test enabled).
        let cutout = [0x3000, 0];
        assert_eq!(
            alpha_use(&[opaque, cutout]),
            AlphaUse::Cutout { threshold: 128 }
        );
        assert_eq!(alpha_use(&[cutout, blend]), AlphaUse::Blend);
    }

    #[test]
    fn merging_keeps_the_bigger_role_and_the_needier_alpha() {
        let world = ImageUsage {
            role: Role::World,
            alpha: AlphaUse::Unused,
            source: "w",
            group: Group::Match,
        };
        let prop = ImageUsage {
            role: Role::Prop,
            alpha: AlphaUse::Cutout { threshold: 128 },
            source: "p",
            group: Group::Match,
        };
        let m = merge(prop.clone(), world.clone());
        assert_eq!(
            (m.role, m.alpha),
            (Role::World, AlphaUse::Cutout { threshold: 128 })
        );
        assert_eq!(merge(world, prop).role, Role::World);
    }

    fn material(techset: &str, camera_region: u8, users: &[Owner]) -> MaterialInfo {
        MaterialInfo {
            name: None,
            blend: BlendMode::Opaque,
            cull: 0,
            polygon_offset: 0,
            camera_region,
            sort_key: 0,
            techset: Some(techset.into()),
            alpha: AlphaUse::Unused,
            textures: Vec::new(),
            users: users.iter().copied().collect(),
        }
    }

    #[test]
    fn roles_from_users_and_techniques() {
        let model = |human, viewmodel| ModelInfo {
            name: String::new(),
            human,
            viewmodel,
            weapons: BTreeSet::new(),
            materials: Vec::new(),
        };
        // 0: prop, 1: human, 2: prop, 3: weapon viewmodel (hands have human bones too; viewmodel wins).
        let models = [
            model(false, false),
            model(true, false),
            model(false, false),
            model(true, true),
        ];
        let minimaps = std::collections::HashSet::from(["compass_map_test".to_owned()]);
        let role = |m: &MaterialInfo| material_role(m, &models, &minimaps).0;
        assert_eq!(role(&material("w", 0, &[Owner::World])), Role::World);
        assert_eq!(role(&material("m", 0, &[Owner::Model(1)])), Role::Character);
        assert_eq!(role(&material("m", 0, &[Owner::Model(2)])), Role::Prop);
        assert_eq!(role(&material("m", 0, &[Owner::Model(3)])), Role::Viewmodel);
        // A texture shared by a viewmodel and its third-person prop keeps the viewmodel's size.
        assert_eq!(
            role(&material("m", 0, &[Owner::Model(0), Owner::Model(3)])),
            Role::Viewmodel
        );
        assert_eq!(
            role(&material("m", 0, &[Owner::Model(2), Owner::World])),
            Role::World
        );
        assert_eq!(
            role(&material("m", CAMERA_REGION_DEPTH_HACK, &[Owner::Model(2)])),
            Role::Viewmodel
        );
        // No user in the zone: the technique set decides; IW4 names may start with ','.
        assert_eq!(role(&material(",2d", 4, &[Owner::Standalone])), Role::Ui);
        assert_eq!(
            role(&material(",effect_blend", 2, &[Owner::Standalone])),
            Role::Effect
        );
        assert_eq!(
            role(&material("wc_l", 0, &[Owner::Standalone])),
            Role::World
        );
    }

    #[test]
    fn groups_from_users_and_tables() {
        let viewmodel = ModelInfo {
            name: "vm".into(),
            human: false,
            viewmodel: true,
            weapons: BTreeSet::from([4, 7]),
            materials: Vec::new(),
        };
        let prop = ModelInfo::default();
        let models = [viewmodel, prop];
        let tables = std::collections::HashSet::from(["callingcard_test".to_owned()]);
        let group = |name: Option<&str>, users: &[Owner]| {
            let mut m = material("2d", 4, users);
            m.name = name.map(str::to_owned);
            material_group(&m, &models, &tables)
        };
        assert_eq!(group(None, &[Owner::World]), Group::Match);
        assert_eq!(group(None, &[Owner::Model(1)]), Group::Match);
        assert_eq!(
            group(None, &[Owner::Model(0)]),
            Group::Weapons(BTreeSet::from([4, 7]))
        );
        assert_eq!(
            group(None, &[Owner::Weapon(2), Owner::Menu]),
            Group::Weapons(BTreeSet::from([2]))
        );
        // Names are matched case-insensitively against string-table cells.
        assert_eq!(
            group(Some("CallingCard_Test"), &[Owner::Standalone, Owner::Menu]),
            Group::Catalog
        );
        assert_eq!(
            group(Some("menu_bg"), &[Owner::Standalone, Owner::Menu]),
            Group::Menu
        );
        // Script/code HUD: no user in the zone, not in a table.
        assert_eq!(group(Some("hud_thing"), &[Owner::Standalone]), Group::Match);
        // A texture shared by a world material and a weapon stays resident.
        assert_eq!(
            merge_group(Group::Weapons(BTreeSet::from([1])), Group::Match),
            Group::Match
        );
        assert_eq!(merge_group(Group::Menu, Group::Catalog), Group::Catalog);
        assert_eq!(
            merge_group(
                Group::Weapons(BTreeSet::from([1])),
                Group::Weapons(BTreeSet::from([3]))
            ),
            Group::Weapons(BTreeSet::from([1, 3]))
        );
    }

    #[test]
    fn models_referenced_during_a_weapon_load_belong_to_it() {
        let mut links = Links::default();
        links.models.push(ModelInfo::default());
        links.owner = Some(Owner::Weapon(3));
        links.credit_model_to_weapon(0);
        links.owner = Some(Owner::Weapon(5));
        links.credit_model_to_weapon(0);
        // Outside a weapon load nothing is credited.
        links.owner = Some(Owner::World);
        links.credit_model_to_weapon(0);
        links.credit_model_to_weapon(9);
        assert_eq!(links.models[0].weapons, BTreeSet::from([3, 5]));
        // Such a model is a weapon model, but not a viewmodel unless a weapon names it as gun or hand model.
        assert!(!links.models[0].viewmodel);
    }

    #[test]
    fn minimap_material_from_map_script() {
        let gsc = r#"main() { maps\mp\_compass::setupMiniMap( "Compass_Map_MP_Test" ); setupminimap(var); }"#;
        assert_eq!(minimap_calls(gsc), ["compass_map_mp_test"]);
        assert_eq!(
            compass_max_range(r#"setdvar( "compassmaxrange", "1800" ); x = 2;"#),
            Some(1800.0)
        );
        assert_eq!(compass_max_range("setdvar(\"r_x\", \"1\");"), None);
        assert!(minimap_calls("no call here").is_empty());
        let minimaps = std::collections::HashSet::from(["compass_map_mp_test".to_owned()]);
        let mut m = material(",2d", 4, &[Owner::Standalone]);
        m.name = Some("compass_map_mp_test".into());
        assert_eq!(material_role(&m, &[], &minimaps).0, Role::Minimap);
        m.name = Some("hud_other".into());
        assert_eq!(material_role(&m, &[], &minimaps).0, Role::Ui);
    }

    #[test]
    fn blend_modes_from_state_bits() {
        let row = |src: u32, dst: u32| [(1 << 8) | (dst << 4) | src | 0x800, 0];
        assert_eq!(blend_mode(&[row(5, 6)]), BlendMode::Blend);
        assert_eq!(blend_mode(&[row(2, 2)]), BlendMode::Additive);
        assert_eq!(blend_mode(&[row(5, 2)]), BlendMode::Additive);
        assert_eq!(blend_mode(&[row(9, 1)]), BlendMode::Multiply);
        assert_eq!(blend_mode(&[row(1, 3)]), BlendMode::Multiply);
        assert_eq!(blend_mode(&[[0x800, 0]]), BlendMode::Opaque);
        assert_eq!(blend_mode(&[[0x3000, 0]]), BlendMode::AlphaTest(128));
    }

    #[test]
    fn alpha_is_applied() {
        let mut img = gx_texture::image::Image::new(1, 2, [9, 9, 9, 100]);
        img.pixels[1][3] = 200;
        let mut cut = img.clone();
        apply_alpha(&mut cut, AlphaUse::Cutout { threshold: 128 });
        assert_eq!((cut.pixels[0][3], cut.pixels[1][3]), (0, 255));
        apply_alpha(&mut img, AlphaUse::Unused);
        assert!(img.pixels.iter().all(|p| p[3] == 255));
    }
}

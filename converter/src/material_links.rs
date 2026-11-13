//! Who uses each image: walk a zone's material → image links and the assets that use each material.
//!
//! IW4L reports every asset pointer it follows: `loaded(ty, slot, insert_slot)` when the asset body is inline, and
//! `alias(ty, slot, target)` when the slot points at one loaded earlier. Mapping slots to assets (the same scheme as
//! IW4L's `assets/src/lane/helpers.rs`) links each material texture slot to its image.
//!
//! A material is "used by" the top-level zone entry whose load referenced it: GfxWorld (world surfaces and skies),
//! XModel (characters and props), Fx/ImpactFx/FxWorld (effects). In the retail x86 zones every one of those is a
//! top-level entry, so this is exact there; in x64 rewrites models nest inside the world and count as world.

use std::collections::{BTreeSet, HashMap};

use asset_iw4::material::{
    CAMERA_REGION_DEPTH_HACK, CAMERA_REGION_EMISSIVE, SORT_KEY_SKY, SORT_KEY_SKYBOX,
};
use asset_iw4::state_bits::{Gfxs0AlphaTest, alpha_test_from_state_bits};
use fastfile_iw4::{AssetType, Ptr, ScriptStrings, ZoneStream};
use gx_texture::policy::Role;

/// Material texture semantics that carry colour the Wii keeps (2D, function, colour map).
const COLOUR_SEMANTICS: [u8; 3] = [0, 1, 2];
/// `MaterialTextureDef.semantic` value for water (its image sits inside a water struct).
const TS_WATER_MAP: u8 = 11;

/// Bones that only a human skeleton has (body or head).
const HUMAN_BONES: [&str; 4] = ["j_spinelower", "j_spineupper", "j_head", "j_neck"];

/// The zone entry that referenced a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    World,
    /// Top-level XModel, by zone entry index.
    Model(usize),
    Effect,
    /// The material's own top-level entry (used by code, script or menus, not by anything in this zone).
    Standalone,
    Other,
}

impl Owner {
    pub fn for_entry(ty: AssetType, index: usize) -> Owner {
        match ty {
            AssetType::GfxWorld => Owner::World,
            AssetType::XModel => Owner::Model(index),
            AssetType::Fx | AssetType::ImpactFx | AssetType::FxWorld => Owner::Effect,
            AssetType::Material => Owner::Standalone,
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
    pub camera_region: u8,
    pub sort_key: u8,
    pub techset: Option<String>,
    pub alpha: AlphaUse,
    /// (semantic, image slot) per texture.
    textures: Vec<(u8, Ptr)>,
    pub users: BTreeSet<Owner>,
}

/// Everything a zone says about one image's use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageUsage {
    pub role: Role,
    pub alpha: AlphaUse,
    /// How the role was decided, for the report.
    pub source: &'static str,
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

#[derive(Default)]
pub struct Links {
    pub owner: Option<Owner>,
    strings: Option<ScriptStrings>,
    images: SlotMap,
    materials: SlotMap,
    techsets: SlotMap,
    techset_names: Vec<Option<String>>,
    pub material_info: Vec<MaterialInfo>,
    /// Zone entry index of each top-level XModel → has a human skeleton.
    pub human_models: HashMap<usize, bool>,
    /// Image index assigned to each loaded image, in load order (filled by the caller through `image_loaded`).
    image_count: usize,
    pub unresolved_texture_links: usize,
}

impl Links {
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
        let techset = g
            .technique_set
            .and_then(|slot| self.techsets.resolve(slot))
            .and_then(|i| self.techset_names.get(i).cloned().flatten());
        let index = self.material_info.len();
        self.material_info.push(MaterialInfo {
            camera_region: g.camera_region,
            sort_key: g.sort_key,
            techset,
            alpha: alpha_use(&rows),
            textures,
            users: BTreeSet::new(),
        });
        self.materials.loaded(slot, insert_slot, index);
        self.use_material(index);
    }

    fn xmodel_loaded(&mut self, s: &ZoneStream<'_>) {
        let Some(Owner::Model(entry)) = self.owner else {
            return;
        };
        let (Some(g), Some(strings)) = (s.xmodel(), self.strings) else {
            return;
        };
        let human = g.bone_names.is_some_and(|names| {
            (0..g.num_bones).any(|i| {
                s.u16_at(names, i * 2)
                    .ok()
                    .and_then(|id| strings.get(s, id))
                    .is_some_and(|bone| HUMAN_BONES.contains(&bone))
            })
        });
        // Nested models finish first; the top-level model is the last one seen.
        self.human_models.insert(entry, human);
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
                self.techsets.loaded(slot, insert_slot, i);
            }
            AssetType::XModel => self.xmodel_loaded(s),
            _ => {}
        }
    }

    /// Hook for `AssetLinkSink::alias`.
    pub fn alias(&mut self, ty: AssetType, slot: Ptr, target: Ptr) {
        match ty {
            AssetType::Image => self.images.alias(slot, target),
            AssetType::TechniqueSet => self.techsets.alias(slot, target),
            AssetType::Material => {
                self.materials.alias(slot, target);
                if let Some(i) = self.materials.resolve(slot) {
                    self.use_material(i);
                }
            }
            _ => {}
        }
    }

    /// Per image (by load index): its usage, or `None` when no material in the zone uses it as colour.
    /// The second value says whether any material uses the image at all (as colour or not).
    pub fn image_usage(&mut self) -> Vec<(Option<ImageUsage>, bool)> {
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
                let (role, source) = material_role(m, &self.human_models);
                let usage = ImageUsage {
                    role,
                    alpha: m.alpha,
                    source,
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
fn material_role(m: &MaterialInfo, human: &HashMap<usize, bool>) -> (Role, &'static str) {
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
            Owner::Model(entry) if human.get(entry) == Some(&true) => {
                (Role::Character, "material: human skeleton model")
            }
            Owner::Model(_) => (Role::Prop, "material: model"),
            Owner::Effect => (Role::Effect, "material: effect"),
            Owner::Standalone | Owner::Other => continue,
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
        };
        let prop = ImageUsage {
            role: Role::Prop,
            alpha: AlphaUse::Cutout { threshold: 128 },
            source: "p",
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
        let human = HashMap::from([(1, true), (2, false)]);
        let role = |m: &MaterialInfo| material_role(m, &human).0;
        assert_eq!(role(&material("w", 0, &[Owner::World])), Role::World);
        assert_eq!(role(&material("m", 0, &[Owner::Model(1)])), Role::Character);
        assert_eq!(role(&material("m", 0, &[Owner::Model(2)])), Role::Prop);
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

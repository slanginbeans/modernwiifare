//! Phase 1: a map zone's textures (GfxImage) through the Wii encoders (`shared/gx_texture`).
//!
//! For each image the zone loads: decide whether the Wii needs it, take its pixels from the zone (in-zone
//! `GfxImageLoadDef`) or from `images/<name>.iwi` in `main/*.iwd`, decode the top mip, and encode it with
//! `gx_texture::policy::convert`. Identical sources are encoded once per role.
//!
//! Role and alpha come from the materials that use the image (`material_links`): world, model (character or prop),
//! effect, viewmodel or sky, and whether the material blends, alpha-tests or ignores alpha. Images no material in the
//! zone uses fall back to a role guessed from the name.
//!
//! The `.gxt` files are converted game data and stay under the git-ignored `out/`. The JSON report is content-free
//! (counts, bytes, quality; no names), like `inventory` and `map`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use asset_transport::{IwdIndex, ZoneMemory, fnv1a64};
use fastfile_iw4::{
    AssetLinkSink, AssetSink, AssetType, Ptr, ScriptStrings, ZoneStream, load_asset_at_observed,
    load_zone, parse_zone_header,
};
use gx_texture::image::{Image, resize};
use gx_texture::image::{coverage, cut_to_coverage, mip_chain};
use gx_texture::policy::{Options, Role, convert_with};
use serde::Serialize;

use crate::inventory::IW4L_REV;
use crate::material_links::{AlphaUse, Group, ImageUsage, Links, Owner, apply_alpha};
use crate::pixel_decode;
use crate::texture::{psnr, texture_format_name};

/// Report format version. Bump when fields change meaning.
/// v2: roles and alpha from materials (`role_sources`, `alpha_uses`, `name_role_changes`).
/// v3: alpha-tested textures keep their coverage (`cutouts`); their PSNR is measured against the same cut.
/// v4: models credited through their material handles; weapon gun/hand models are viewmodels (`models`, `viewmodels`).
/// v5: load-by-need groups (`groups`): match, per weapon, catalog (by name from string tables), menu.
pub const REPORT_VERSION: u32 = 5;

/// When textures have to be in memory, in bytes (see `material_links::Group`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct GroupStats {
    /// Group label -> (textures, Wii bytes). Weapon textures shared by several weapons count once.
    pub totals: BTreeMap<&'static str, (usize, u64)>,
    /// Weapons with first-person textures or HUD icons of their own.
    pub weapons: usize,
    /// Bytes per weapon (a texture counts for every weapon that uses it): median and largest.
    pub weapon_median_bytes: u64,
    pub weapon_max_bytes: u64,
    /// `match` plus the 8 weapons with the most bytes (4 players x 2 weapons, worst case), shared textures once.
    pub match_plus_worst_8_weapons_bytes: u64,
}

/// One converted texture, for the runtime to load by need. Holds names, so it stays under `out/` like the `.gxt`s.
#[derive(Clone, Debug, Serialize)]
pub struct IndexEntry {
    pub file: String,
    pub role: &'static str,
    pub format: &'static str,
    pub bytes: u64,
    pub group: &'static str,
    /// Weapon names, for the `weapon` group.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weapons: Vec<String>,
    /// The zone's images (load order) this texture stands for: the converted one and identical duplicates.
    pub images: Vec<usize>,
    /// Cube face (`px`..`nz`) for sky faces, empty otherwise.
    #[serde(skip_serializing_if = "str::is_empty")]
    pub face: &'static str,
}

/// Alpha-tested textures: how well the 1-bit alpha keeps the full-size coverage at level 0.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CutoutStats {
    pub textures: usize,
    /// Sum over textures of |level-0 coverage - full-size coverage| (fraction of texels), with coverage kept.
    pub coverage_error_sum: f64,
    /// The same for a plain cut of the resized alpha at the material threshold (the old behaviour).
    pub naive_coverage_error_sum: f64,
    /// Over every mip level of every alpha-tested texture: number of levels, and the summed coverage error with
    /// coverage kept and with a plain cut. Distant (small) levels are where plain cuts lose foliage.
    pub mip_levels: usize,
    pub mip_coverage_error_sum: f64,
    pub naive_mip_coverage_error_sum: f64,
    /// Worst single mip level, with coverage kept and with a plain cut.
    pub worst_mip_coverage_error: f64,
    pub naive_worst_mip_coverage_error: f64,
}

/// IW4 `TextureSemantic` values that the Wii does not take as textures.
const TS_NORMAL_MAP: u8 = 5;
const TS_SPECULAR_MAP: u8 = 8;
/// IW4 `MapType` for plain 2D images.
const MAPTYPE_2D: u8 = 3;
/// IW4 `MapType` for cube maps (six faces).
const MAPTYPE_CUBE: u8 = 5;

/// One GfxImage as the zone describes it.
#[derive(Clone, Debug)]
pub struct ZoneImage {
    pub name: String,
    pub semantic: u8,
    pub category: u8,
    pub map_type: u8,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    /// D3D format of the in-zone payload (0 when streamed).
    pub d3d_format: u32,
    /// In-zone top-mip-first pixel data; empty when the image streams from an IWI.
    pub payload: Vec<u8>,
    /// How the zone's materials use the image as colour, if any does.
    pub usage: Option<ImageUsage>,
    /// Whether any material in the zone references the image (as colour or not).
    pub used_by_material: bool,
}

/// The images and material links of a walked zone.
#[derive(Default)]
pub struct Collector {
    pub images: Vec<ZoneImage>,
    pub links: Links,
}

impl AssetLinkSink for Collector {
    fn loaded(
        &mut self,
        s: &ZoneStream<'_>,
        ty: AssetType,
        slot: Ptr,
        insert_slot: Option<Ptr>,
    ) -> fastfile_iw4::Result<()> {
        if ty != AssetType::Image {
            self.links.loaded(s, ty, slot, insert_slot);
            return Ok(());
        }
        let Some(g) = s.latest_image() else {
            return Ok(());
        };
        self.links.image_loaded(slot, insert_slot);
        let name = g
            .name
            .and_then(|p| s.cstr(s.resolve_alias(p)).ok())
            .unwrap_or_default()
            .to_owned();
        let payload = if g.source_len > 0 {
            s.source_slice(g.source_offset, g.source_len)?.to_vec()
        } else {
            Vec::new()
        };
        self.images.push(ZoneImage {
            name,
            semantic: g.semantic,
            category: g.category,
            map_type: g.map_type,
            width: u32::from(g.width),
            height: u32::from(g.height),
            depth: u32::from(g.depth),
            d3d_format: g.format,
            payload,
            usage: None,
            used_by_material: false,
        });
        Ok(())
    }

    fn alias(&mut self, ty: AssetType, slot: Ptr, target: Ptr) -> fastfile_iw4::Result<()> {
        self.links.alias(ty, slot, target);
        Ok(())
    }

    fn remember_xmodel_name(&mut self, slot: Ptr, insert_slot: Option<Ptr>, name: Ptr) {
        self.links.remember_xmodel_name(slot, insert_slot, name);
    }

    fn xmodel_name_ptr(&self, slot: Ptr) -> Option<Ptr> {
        self.links.xmodel_name_ptr(slot)
    }

    fn capture_xanim(
        &mut self,
        s: &ZoneStream<'_>,
        geometry: fastfile_iw4::XAnimPartsGeometry,
    ) -> fastfile_iw4::Result<()> {
        self.links.xanim_loaded(s, geometry);
        Ok(())
    }

    fn remember_xmodel_surfaces(&mut self, slot: Ptr, surfaces: Ptr) {
        self.links.remember_xmodel_surfaces(slot, surfaces);
    }

    fn remember_xmodel_surface_name(&mut self, slot: Ptr, name: Ptr) {
        self.links.remember_xmodel_surface_name(slot, name);
    }

    fn xmodel_surfaces(&self, slot: Ptr) -> Option<Ptr> {
        self.links.xmodel_surfaces(slot)
    }

    fn xmodel_surface_name(&self, slot: Ptr) -> Option<Ptr> {
        self.links.xmodel_surface_name(slot)
    }

    fn capture_loaded_sound(
        &mut self,
        s: &ZoneStream<'_>,
        header: Ptr,
        pcm: Ptr,
        data_len: usize,
    ) -> fastfile_iw4::Result<()> {
        self.links.sounds.loaded_sound(s, header, pcm, data_len);
        Ok(())
    }

    fn bind_last_loaded_to_sound_file(&mut self, file: Ptr) -> fastfile_iw4::Result<()> {
        self.links.sounds.bind_file(file);
        Ok(())
    }

    fn capture_sound(
        &mut self,
        s: &ZoneStream<'_>,
        list: Ptr,
        count: usize,
        head: Option<Ptr>,
    ) -> fastfile_iw4::Result<()> {
        self.links.sounds.sound_list(s, list, count, head);
        Ok(())
    }

    fn capture_localize(&mut self, name: &str, value: &[u8]) -> fastfile_iw4::Result<()> {
        let v = String::from_utf8_lossy(value);
        self.links
            .localize
            .entry(name.to_owned())
            .or_insert_with(|| v.trim_end_matches(' ').to_owned());
        Ok(())
    }

    fn linked_asset_name(&self, slot: Ptr) -> Option<&str> {
        self.links.material_name(slot)
    }

    fn capture_font(&mut self, rec: &fastfile_iw4::FontCapture<'_>) -> fastfile_iw4::Result<()> {
        self.links.ui.font(rec);
        Ok(())
    }

    fn capture_menu_def(
        &mut self,
        rec: &fastfile_iw4::MenuDefCapture<'_>,
    ) -> fastfile_iw4::Result<()> {
        self.links.ui.menu_def(rec);
        Ok(())
    }

    fn capture_menu_item_layout(
        &mut self,
        rec: &fastfile_iw4::MenuItemLayout<'_>,
    ) -> fastfile_iw4::Result<()> {
        self.links.ui.item(rec);
        Ok(())
    }

    fn capture_item_visible_exp(
        &mut self,
        menu: &str,
        item: &str,
        dump: &str,
    ) -> fastfile_iw4::Result<()> {
        self.links.ui.visible(menu, item, dump);
        Ok(())
    }

    fn capture_menu_script(
        &mut self,
        menu: &str,
        item: &str,
        kind: fastfile_iw4::MenuScriptKind,
        script: &str,
    ) -> fastfile_iw4::Result<()> {
        self.links.ui.script(menu, item, kind, script);
        Ok(())
    }

    fn capture_string_table(
        &mut self,
        s: &ZoneStream<'_>,
        header: Ptr,
    ) -> fastfile_iw4::Result<()> {
        self.links.string_table(s, header);
        Ok(())
    }

    fn capture_raw_file(
        &mut self,
        name: &str,
        data: &[u8],
        zlib_compressed: bool,
    ) -> fastfile_iw4::Result<()> {
        self.links.raw_file(data, zlib_compressed);
        // The game-type scripts the match rules are read from (`rules.rs`).
        if crate::rules::SCRIPTS.contains(&name) {
            let bytes = if zlib_compressed {
                asset_transport::inflate_zlib(data).unwrap_or_default()
            } else {
                data.to_vec()
            };
            let text = String::from_utf8_lossy(&bytes)
                .trim_end_matches('\0')
                .to_owned();
            self.links.scripts.insert(name.to_owned(), text);
        }
        if name == crate::weapons::LOCHIT_RAWFILE {
            self.links.lochit = crate::weapons::parse_lochit(data, zlib_compressed);
        }
        Ok(())
    }
}

struct Driver(Collector);

impl AssetSink for Driver {
    fn set_script_strings(&mut self, strings: ScriptStrings) {
        self.0.links.set_strings(strings);
    }

    fn load_asset(
        &mut self,
        s: &mut ZoneStream<'_>,
        _index: usize,
        ty: AssetType,
        slot: Ptr,
    ) -> fastfile_iw4::Result<()> {
        self.0.links.owner = Some(match ty {
            // Weapons get their load-order index, which `Links::weapon_loaded` assigns the same way.
            AssetType::Weapon => Owner::Weapon(self.0.links.weapons.len()),
            _ => Owner::for_entry(ty),
        });
        load_asset_at_observed(s, ty, slot, &mut self.0).map(|_| ())
    }
}

/// What the material walk found, for the report.
#[derive(Clone, Debug, Default)]
pub struct LinkStats {
    pub materials: usize,
    pub unresolved_texture_links: usize,
    pub models: usize,
    pub viewmodels: usize,
    pub unresolved_model_links: usize,
    /// Weapon names in load order (`Group::Weapons` indexes them).
    pub weapons: Vec<String>,
}

/// Walk a zone and keep its links (shared by `collect` and `table_strings`).
fn walk(image: &[u8], tables: &HashSet<String>) -> Result<Collector, String> {
    Ok(walk_with(image, tables, |_, _| ())?.0)
}

/// Walk a zone, then call `after` with the stream (its blocks still loaded) and the collected links, e.g. to read the
/// GfxWorld. Returns the image list and links, plus what `after` returns.
pub fn walk_with<T>(
    image: &[u8],
    tables: &HashSet<String>,
    after: impl FnOnce(&ZoneStream<'_>, &Links) -> T,
) -> Result<(Collector, T), String> {
    let header = parse_zone_header(image).map_err(|e| format!("{e:?}"))?;
    let mut memory = ZoneMemory::for_header(&header);
    let mut stream = memory.stream(image).map_err(|e| format!("{e:?}"))?;
    let mut driver = Driver(Collector::default());
    driver.0.links.add_table_strings(tables.iter().cloned());
    load_zone(&mut stream, &mut driver).map_err(|e| format!("{e:?}"))?;
    let t = after(&stream, &driver.0.links);
    Ok((driver.0, t))
}

/// Every string-table cell of a zone (lower case). Used to know, while converting another zone, which materials the
/// shared tables (calling cards, emblems, icon tables in `code_post_gfx_mp`) look up by name.
pub fn table_strings(image: &[u8]) -> Result<HashSet<String>, String> {
    Ok(walk(image, &HashSet::new())?.links.take_table_strings())
}

/// Every GfxImage loaded by an inflated zone (top level and nested), in load order, with its material usage.
/// `tables`: string-table cells from other zones, on top of the zone's own.
pub fn collect(
    image: &[u8],
    tables: &HashSet<String>,
) -> Result<(Vec<ZoneImage>, LinkStats), String> {
    let driver = Driver(walk(image, tables)?);
    let Collector {
        mut images,
        mut links,
    } = driver.0;
    for (img, (usage, used)) in images.iter_mut().zip(links.image_usage()) {
        img.usage = usage;
        img.used_by_material = used;
    }
    let stats = LinkStats {
        materials: links.material_info.len(),
        unresolved_texture_links: links.unresolved_texture_links,
        models: links.models.len(),
        viewmodels: links.models.iter().filter(|m| m.viewmodel).count(),
        unresolved_model_links: links.unresolved_model_links,
        weapons: std::mem::take(&mut links.weapons),
    };
    Ok((images, stats))
}

/// What to do with an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Role, the material's alpha use (`None` when no material uses it), and where the role came from.
    Encode(Role, Option<AlphaUse>, &'static str),
    Skip(&'static str),
}

pub fn plan(img: &ZoneImage) -> Plan {
    if img.name.starts_with('*') {
        return Plan::Skip("engine-generated (lightmap, probe): handled with the world");
    }
    // IW4 GfxImageCategory: 1 auto-generated, 5 water (the simulation fills it), 6 render target, 7 temp. The engine
    // makes these at run time; there is no file to convert.
    if matches!(img.category, 1 | 5 | 6 | 7) {
        return Plan::Skip("generated at run time (water, render target): no data to convert");
    }
    // IW4 names a reference to an image that another zone holds with a leading ','; it has no header data here.
    if img.name.starts_with(',') {
        return Plan::Skip("reference to an image in another zone: converted with that zone");
    }
    if img.name.starts_with('$') {
        return Plan::Skip("engine built-in ($white...): made on the Wii");
    }
    match img.semantic {
        TS_NORMAL_MAP => return Plan::Skip("normal map: baked into lighting"),
        TS_SPECULAR_MAP => return Plan::Skip("specular map: dropped"),
        _ => {}
    }
    if img.map_type == MAPTYPE_CUBE {
        // The Wii has no cube maps. A sky cube becomes six 2D faces (a skybox); other cubes are environment
        // reflections, which the Wii does not draw.
        return match &img.usage {
            Some(u) if u.role == Role::Sky => Plan::Encode(u.role, Some(u.alpha), u.source),
            _ => Plan::Skip("environment cube map: reflections are not drawn on the Wii"),
        };
    }
    if img.map_type != MAPTYPE_2D || img.depth > 1 {
        return Plan::Skip("3D (volume) map: not used on the Wii");
    }
    match &img.usage {
        Some(u) => Plan::Encode(u.role, Some(u.alpha), u.source),
        None if img.used_by_material => {
            Plan::Skip("materials use it only as normal/specular/detail: not a colour map")
        }
        None => Plan::Encode(
            role_from_name(&img.name),
            None,
            "name (no material in zone)",
        ),
    }
}

/// Fallback role from the image name, for images no material in the zone uses (script, menus, code).
pub fn role_from_name(name: &str) -> Role {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if n.starts_with("sky") || n.contains("_sky") {
        Role::Sky
    } else if has(&["viewmodel", "viewhands"]) || n.starts_with("weapon_") {
        Role::Viewmodel
    } else if has(&[
        "hud",
        "compass",
        "cardicon",
        "cardtitle",
        "ui_",
        "minimap",
        "loadscreen",
    ]) {
        Role::Ui
    } else if n.starts_with("fx_")
        || has(&[
            "smoke",
            "fire",
            "flash",
            "spark",
            "explosion",
            "glow",
            "tracer",
        ])
    {
        Role::Effect
    } else if has(&["_body", "body_", "head_", "_head", "hands", "gear_"]) {
        Role::Character
    } else if n.starts_with("com_") || has(&["vehicle", "_prop", "prop_", "debris"]) {
        Role::Prop
    } else {
        Role::World
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RoleTotal {
    pub textures: usize,
    pub source_rgba_bytes: u64,
    pub wii_bytes: u64,
    pub psnr_min_db: Option<f64>,
    /// Mean over finite values (lossless textures are counted in `lossless`).
    pub psnr_mean_db: Option<f64>,
    pub lossless: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct TexturesReport {
    pub report_version: u32,
    pub iw4l_rev: &'static str,
    pub map: String,
    /// Path relative to the game folder, `/`-separated.
    pub zone_path: String,
    pub images: usize,
    pub in_zone: usize,
    pub streamed: usize,
    pub skipped: BTreeMap<&'static str, usize>,
    pub failed: BTreeMap<String, usize>,
    pub duplicates: usize,
    pub encoded: usize,
    pub wii_bytes: u64,
    pub by_role: BTreeMap<&'static str, RoleTotal>,
    pub by_wii_format: BTreeMap<&'static str, (usize, u64)>,
    pub by_source_format: BTreeMap<&'static str, usize>,
    pub semantic_counts: BTreeMap<u8, usize>,
    pub map_type_counts: BTreeMap<u8, usize>,
    pub category_counts: BTreeMap<u8, usize>,
    /// Materials walked, and material texture slots that did not resolve to an image.
    pub materials: usize,
    pub unresolved_texture_links: usize,
    /// Models walked, how many are weapon gun/hand models, and model links (material handles, weapon model slots)
    /// that did not resolve.
    pub models: usize,
    pub viewmodels: usize,
    pub unresolved_model_links: usize,
    /// Encoded textures by where their role came from.
    pub role_sources: BTreeMap<&'static str, usize>,
    /// Encoded textures by how their materials use alpha ("none" when no material uses them).
    pub alpha_uses: BTreeMap<&'static str, usize>,
    /// Encoded textures whose material role differs from the old name guess: (name role, material role) -> count.
    pub name_role_changes: BTreeMap<String, usize>,
    pub cutouts: CutoutStats,
    pub groups: GroupStats,
}

fn alpha_label(a: Option<AlphaUse>) -> &'static str {
    match a {
        None => "none (no material)",
        Some(AlphaUse::Unused) => "unused -> dropped",
        Some(AlphaUse::Cutout { .. }) => "alpha test -> 1 bit",
        Some(AlphaUse::Blend) => "blend -> kept",
    }
}

/// (file suffix, image) pairs, source format label, source hash.
type SourcePixels = (Vec<(&'static str, Image)>, &'static str, u64);

/// Pixels of one image (from the zone, or from its IWI) as (file suffix, image) pairs: one for a 2D image, six for a
/// cube map (`_px`, `_nx`, `_py`, `_ny`, `_pz`, `_nz`), plus a label of the source format and a hash of the source.
fn source_pixels(img: &ZoneImage, iwds: Option<&IwdIndex>) -> Result<SourcePixels, String> {
    let cube = img.map_type == MAPTYPE_CUBE;
    if !img.payload.is_empty() {
        if cube {
            return Err("in-zone cube map (not supported yet)".into());
        }
        let fmt = pixel_decode::d3d_format(img.d3d_format)?;
        let pixels = pixel_decode::decode(&img.payload, img.width, img.height, fmt)?;
        return Ok((vec![("", pixels)], fmt.name(), fnv1a64(&img.payload)));
    }
    let iwds = iwds.ok_or("streamed image, but no main/*.iwd index")?;
    let file = iwds
        .image_candidates(&img.name)
        .and_then(<[_]>::last)
        .ok_or("streamed image: IWI not found in main/*.iwd")?;
    let bytes = file.read()?;
    if cube {
        let (faces, label) = pixel_decode::decode_iwi_cube(&bytes)?;
        let faces = pixel_decode::CUBE_FACES.into_iter().zip(faces).collect();
        return Ok((faces, label, fnv1a64(&bytes)));
    }
    let (pixels, label) = pixel_decode::decode_iwi(&bytes)?;
    Ok((vec![("", pixels)], label, fnv1a64(&bytes)))
}

/// Strip anything that is not safe in a file name.
fn file_stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "unnamed".into() } else { s }
}

/// Convert every image of `zone_image` (an inflated zone). `.gxt` files go to `out_dir`.
pub fn convert_zone(
    zone_image: &[u8],
    map: &str,
    zone_path: &str,
    iwds: Option<&IwdIndex>,
    out_dir: &Path,
    verbose: bool,
    tables: &HashSet<String>,
) -> Result<(TexturesReport, Vec<IndexEntry>), String> {
    let (images, stats) = collect(zone_image, tables)?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let mut report = TexturesReport {
        report_version: REPORT_VERSION,
        iw4l_rev: IW4L_REV,
        map: map.to_owned(),
        zone_path: zone_path.to_owned(),
        images: images.len(),
        in_zone: images.iter().filter(|i| !i.payload.is_empty()).count(),
        streamed: images.iter().filter(|i| i.payload.is_empty()).count(),
        skipped: BTreeMap::new(),
        failed: BTreeMap::new(),
        duplicates: 0,
        encoded: 0,
        wii_bytes: 0,
        by_role: BTreeMap::new(),
        by_wii_format: BTreeMap::new(),
        by_source_format: BTreeMap::new(),
        semantic_counts: BTreeMap::new(),
        map_type_counts: BTreeMap::new(),
        category_counts: BTreeMap::new(),
        materials: stats.materials,
        unresolved_texture_links: stats.unresolved_texture_links,
        models: stats.models,
        viewmodels: stats.viewmodels,
        unresolved_model_links: stats.unresolved_model_links,
        role_sources: BTreeMap::new(),
        alpha_uses: BTreeMap::new(),
        name_role_changes: BTreeMap::new(),
        cutouts: CutoutStats::default(),
        groups: GroupStats::default(),
    };
    let mut psnr_sums: HashMap<&'static str, (f64, usize)> = HashMap::new();
    let mut index: Vec<IndexEntry> = Vec::new();
    // (weapons that need it, bytes) per weapon-group texture.
    let mut weapon_textures: Vec<(BTreeSet<usize>, u64)> = Vec::new();
    // Source hash -> index entries already made from it (one per cube face).
    let mut seen: HashMap<(u64, &'static str, Option<AlphaUse>), Vec<usize>> = HashMap::new();
    let mut used_stems: HashMap<String, usize> = HashMap::new();

    for (image_index, img) in images.iter().enumerate() {
        *report.semantic_counts.entry(img.semantic).or_default() += 1;
        *report.map_type_counts.entry(img.map_type).or_default() += 1;
        *report.category_counts.entry(img.category).or_default() += 1;
        let (role, alpha, source) = match plan(img) {
            Plan::Skip(why) => {
                if verbose {
                    let users = img.usage.as_ref().map_or("no colour use", |u| u.source);
                    println!(
                        "  SKIPPED {why} [map type {}, {}x{}x{}, {users}]: {}",
                        img.map_type, img.width, img.height, img.depth, img.name
                    );
                }
                *report.skipped.entry(why).or_default() += 1;
                continue;
            }
            Plan::Encode(role, alpha, source) => (role, alpha, source),
        };
        let (faces, src_fmt, hash) = match source_pixels(img, iwds) {
            Ok(x) => x,
            Err(e) => {
                if verbose {
                    println!(
                        "  FAILED {e} [category {}, map type {}]: {}",
                        img.category, img.map_type, img.name
                    );
                }
                *report.failed.entry(e).or_default() += 1;
                continue;
            }
        };
        if let Some(entries) = seen.get(&(hash, role.name(), alpha)) {
            for &e in entries {
                index[e].images.push(image_index);
            }
            report.duplicates += 1;
            continue;
        }
        let first_entry = index.len();
        for (face, mut pixels) in faces {
            // Alpha-tested textures keep their smooth alpha: the encoder cuts it per mip level, keeping coverage.
            let cutout = match alpha {
                Some(AlphaUse::Cutout { threshold }) => Some(threshold),
                Some(a) => {
                    apply_alpha(&mut pixels, a);
                    None
                }
                None => None,
            };
            *report.by_source_format.entry(src_fmt).or_default() += 1;
            *report.role_sources.entry(source).or_default() += 1;
            *report.alpha_uses.entry(alpha_label(alpha)).or_default() += 1;
            let guessed = role_from_name(&img.name);
            if alpha.is_some() && guessed != role {
                *report
                    .name_role_changes
                    .entry(format!("{} -> {}", guessed.name(), role.name()))
                    .or_default() += 1;
            }

            let c = convert_with(&pixels, role, Options { cutout });
            let level0 = c.decode_level(0);
            let mut reference = resize(&pixels, c.width, c.height);
            if let (Some(threshold), Some(l0)) = (cutout, &level0) {
                // Coverage: full size, our cut (level 0 as decoded), and a plain cut of the resized alpha at the threshold.
                let full = coverage(&pixels, threshold);
                let naive = coverage(&reference, threshold);
                let ours = coverage(l0, 128);
                let k = &mut report.cutouts;
                k.textures += 1;
                k.coverage_error_sum += f64::from((ours - full).abs());
                k.naive_coverage_error_sum += f64::from((naive - full).abs());
                // Every mip level: ours as decoded, against a plain cut of the same smooth mip chain.
                let smooth = mip_chain(&reference, 1);
                for (level, plain) in (0..c.levels).zip(&smooth) {
                    let Some(dec) = c.decode_level(level) else {
                        break;
                    };
                    let e_ours = f64::from((coverage(&dec, 128) - full).abs());
                    let e_plain = f64::from((coverage(plain, threshold) - full).abs());
                    k.mip_levels += 1;
                    k.mip_coverage_error_sum += e_ours;
                    k.naive_mip_coverage_error_sum += e_plain;
                    k.worst_mip_coverage_error = k.worst_mip_coverage_error.max(e_ours);
                    k.naive_worst_mip_coverage_error =
                        k.naive_worst_mip_coverage_error.max(e_plain);
                }
                // Judge colour against the same cut, so the score measures the encoder, not the (intended) 1-bit alpha.
                cut_to_coverage(&mut reference, full);
            }
            let q = level0.map(|level0| psnr(&reference, &level0, c.format));

            let mut stem = format!(
                "{}{}{face}",
                file_stem(&img.name),
                if face.is_empty() { "" } else { "_" }
            );
            let n = used_stems.entry(stem.clone()).or_default();
            *n += 1;
            if *n > 1 {
                stem = format!("{stem}~{n}");
            }
            let gxt = out_dir.join(format!("{stem}.gxt"));
            std::fs::write(&gxt, gx_texture::container::write(&c))
                .map_err(|e| format!("{}: {e}", gxt.display()))?;

            let bytes = c.bytes() as u64;
            let group = img.usage.as_ref().map_or(Group::Match, |u| u.group.clone());
            let weapons: Vec<String> = match &group {
                Group::Weapons(w) => w
                    .iter()
                    .filter_map(|&i| stats.weapons.get(i).cloned())
                    .collect(),
                _ => Vec::new(),
            };
            let g = report.groups.totals.entry(group.label()).or_default();
            g.0 += 1;
            g.1 += bytes;
            if let Group::Weapons(w) = &group {
                weapon_textures.push((w.clone(), bytes));
            }
            index.push(IndexEntry {
                file: format!("{stem}.gxt"),
                role: role.name(),
                format: texture_format_name(&c),
                bytes,
                group: group.label(),
                weapons,
                images: vec![image_index],
                face,
            });
            if verbose {
                // Console only, for tuning: names never go into the report.
                println!(
                    "  {:<9} {:<5} {:>4}x{:<4} -> {:<6} {:>4}x{:<4} {:>6.1} KiB {:>6} dB  alpha {:?}  [{} | {} | {}] c{}  {}",
                    role.name(),
                    src_fmt,
                    pixels.width,
                    pixels.height,
                    texture_format_name(&c),
                    c.width,
                    c.height,
                    bytes as f64 / 1024.0,
                    q.map_or("-".into(), |d| format!("{d:.1}")),
                    c.traits.alpha,
                    source,
                    alpha_label(alpha),
                    group.label(),
                    img.category,
                    img.name
                );
            }
            report.encoded += 1;
            report.wii_bytes += bytes;
            let f = report
                .by_wii_format
                .entry(texture_format_name(&c))
                .or_default();
            f.0 += 1;
            f.1 += bytes;
            let t = report.by_role.entry(role.name()).or_default();
            t.textures += 1;
            t.source_rgba_bytes += pixels.pixels.len() as u64 * 4;
            t.wii_bytes += bytes;
            match q {
                Some(db) if db.is_finite() => {
                    t.psnr_min_db = Some(t.psnr_min_db.map_or(db, |m| m.min(db)));
                    let s = psnr_sums.entry(role.name()).or_default();
                    s.0 += db;
                    s.1 += 1;
                }
                Some(_) => t.lossless += 1,
                None => {}
            }
        }
        seen.insert(
            (hash, role.name(), alpha),
            (first_entry..index.len()).collect(),
        );
    }
    for (role, (sum, n)) in psnr_sums {
        if let Some(t) = report.by_role.get_mut(role) {
            t.psnr_mean_db = Some(sum / n as f64);
        }
    }
    report.groups = group_stats(&report.groups.totals, &weapon_textures);
    Ok((report, index))
}

/// Per-weapon memory from the weapon-group textures, and the worst case of `match` plus 8 weapons.
fn group_stats(
    totals: &BTreeMap<&'static str, (usize, u64)>,
    weapon_textures: &[(BTreeSet<usize>, u64)],
) -> GroupStats {
    let mut per_weapon: BTreeMap<usize, u64> = BTreeMap::new();
    for (weapons, bytes) in weapon_textures {
        for &w in weapons {
            *per_weapon.entry(w).or_default() += bytes;
        }
    }
    let mut sizes: Vec<(u64, usize)> = per_weapon.iter().map(|(&w, &b)| (b, w)).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let worst: BTreeSet<usize> = sizes.iter().take(8).map(|&(_, w)| w).collect();
    let worst_bytes: u64 = weapon_textures
        .iter()
        .filter(|(weapons, _)| !weapons.is_disjoint(&worst))
        .map(|(_, b)| b)
        .sum();
    let mut sorted: Vec<u64> = sizes.iter().map(|&(b, _)| b).collect();
    sorted.sort_unstable();
    GroupStats {
        totals: totals.clone(),
        weapons: per_weapon.len(),
        weapon_median_bytes: sorted.get(sorted.len() / 2).copied().unwrap_or(0),
        weapon_max_bytes: sorted.last().copied().unwrap_or(0),
        match_plus_worst_8_weapons_bytes: totals.get("match").map_or(0, |t| t.1) + worst_bytes,
    }
}

pub fn render_text(r: &TexturesReport) -> String {
    use std::fmt::Write as _;
    let kib = |b: u64| b as f64 / 1024.0;
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Modern Wiifare zone textures (report v{}, IW4L {})",
        r.report_version,
        &r.iw4l_rev[..7]
    );
    let _ = writeln!(s, "Zone: {} ({})", r.zone_path, r.map);
    let _ = writeln!(
        s,
        "images: {} ({} in zone, {} streamed from IWI) -> encoded {}, duplicates {}, skipped {}, failed {}",
        r.images,
        r.in_zone,
        r.streamed,
        r.encoded,
        r.duplicates,
        r.skipped.values().sum::<usize>(),
        r.failed.values().sum::<usize>()
    );
    let _ = writeln!(s, "Wii texture memory: {:.2} MiB", mib(r.wii_bytes));
    let g = &r.groups;
    let groups: Vec<String> = g
        .totals
        .iter()
        .map(|(k, (n, b))| format!("{k} {n} / {:.2} MiB", mib(*b)))
        .collect();
    let _ = writeln!(s, "Load by need: {}", groups.join(", "));
    if g.weapons > 0 {
        let _ = writeln!(
            s,
            "  {} weapons: median {:.2} MiB, largest {:.2} MiB each; match + worst 8 weapons {:.2} MiB",
            g.weapons,
            mib(g.weapon_median_bytes),
            mib(g.weapon_max_bytes),
            mib(g.match_plus_worst_8_weapons_bytes)
        );
    }
    let _ = writeln!(
        s,
        "\n{:<10} {:>5} {:>12} {:>10} {:>9} {:>9} {:>8}",
        "role", "count", "source MiB", "Wii KiB", "PSNR min", "mean", "lossless"
    );
    for (role, t) in &r.by_role {
        let db = |v: Option<f64>| v.map_or("-".into(), |d| format!("{d:.1}"));
        let _ = writeln!(
            s,
            "{role:<10} {:>5} {:>12.2} {:>10.0} {:>9} {:>9} {:>8}",
            t.textures,
            mib(t.source_rgba_bytes),
            kib(t.wii_bytes),
            db(t.psnr_min_db),
            db(t.psnr_mean_db),
            t.lossless
        );
    }
    let _ = writeln!(s, "\nWii formats:");
    for (f, (n, b)) in &r.by_wii_format {
        let _ = writeln!(s, "  {f:<7} {n:>5}  {:>9.0} KiB", kib(*b));
    }
    let _ = writeln!(s, "Source formats: {:?}", r.by_source_format);
    let _ = writeln!(
        s,
        "\nMaterials walked: {}, texture links not resolved: {}",
        r.materials, r.unresolved_texture_links
    );
    let _ = writeln!(
        s,
        "Models walked: {} ({} weapon gun/hand models), model links not resolved: {}",
        r.models, r.viewmodels, r.unresolved_model_links
    );
    let _ = writeln!(s, "Role from:");
    for (src, n) in &r.role_sources {
        let _ = writeln!(s, "  {n:>5}  {src}");
    }
    let _ = writeln!(s, "Alpha:");
    for (a, n) in &r.alpha_uses {
        let _ = writeln!(s, "  {n:>5}  {a}");
    }
    if r.cutouts.textures > 0 {
        let n = r.cutouts.textures as f64;
        let k = &r.cutouts;
        let levels = k.mip_levels.max(1) as f64;
        let _ = writeln!(
            s,
            "Alpha-tested: {} textures. Coverage error, kept vs plain cut: level 0 mean {:.1}% vs {:.1}%; \
             all {} mip levels mean {:.1}% vs {:.1}%, worst {:.0}% vs {:.0}%",
            k.textures,
            100.0 * k.coverage_error_sum / n,
            100.0 * k.naive_coverage_error_sum / n,
            k.mip_levels,
            100.0 * k.mip_coverage_error_sum / levels,
            100.0 * k.naive_mip_coverage_error_sum / levels,
            100.0 * k.worst_mip_coverage_error,
            100.0 * k.naive_worst_mip_coverage_error
        );
    }
    if !r.name_role_changes.is_empty() {
        let _ = writeln!(s, "Changed from the name guess (name -> material):");
        for (change, n) in &r.name_role_changes {
            let _ = writeln!(s, "  {n:>5}  {change}");
        }
    }
    if !r.skipped.is_empty() {
        let _ = writeln!(s, "\nSkipped:");
        for (why, n) in &r.skipped {
            let _ = writeln!(s, "  {n:>5}  {why}");
        }
    }
    if !r.failed.is_empty() {
        let _ = writeln!(s, "\nFailed:");
        for (why, n) in &r.failed {
            let _ = writeln!(s, "  {n:>5}  {why}");
        }
    }
    let _ = writeln!(
        s,
        "\nsemantic {:?} | map type {:?} | category {:?}",
        r.semantic_counts, r.map_type_counts, r.category_counts
    );
    s
}

/// `main/` of the install, for streamed images.
pub fn main_dir(game: &Path) -> Option<PathBuf> {
    let main = game.join("main");
    main.is_dir().then_some(main)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(name: &str, semantic: u8, map_type: u8) -> ZoneImage {
        ZoneImage {
            name: name.into(),
            semantic,
            category: 3,
            map_type,
            width: 4,
            height: 4,
            depth: 1,
            d3d_format: 0,
            payload: Vec::new(),
            usage: None,
            used_by_material: false,
        }
    }

    #[test]
    fn plans_from_semantic_and_type() {
        assert!(matches!(
            plan(&img("brick_wall", 2, MAPTYPE_2D)),
            Plan::Encode(Role::World, None, _)
        ));
        assert!(matches!(
            plan(&img("brick_wall_nml", TS_NORMAL_MAP, MAPTYPE_2D)),
            Plan::Skip(_)
        ));
        assert!(matches!(
            plan(&img("brick_wall_spc", TS_SPECULAR_MAP, MAPTYPE_2D)),
            Plan::Skip(_)
        ));
        assert!(matches!(
            plan(&img("*lightmap0_primary", 0, MAPTYPE_2D)),
            Plan::Skip(_)
        ));
        assert!(matches!(plan(&img("$white", 2, MAPTYPE_2D)), Plan::Skip(_)));
        // A cube nothing draws as sky is an environment reflection: skipped.
        assert!(matches!(plan(&img("sky_cube", 2, 5)), Plan::Skip(_)));
        // A cube a sky material uses becomes six sky faces.
        let mut sky = img("test_sky", 2, MAPTYPE_CUBE);
        sky.usage = Some(ImageUsage {
            role: Role::Sky,
            alpha: AlphaUse::Unused,
            source: "material: sky sort key",
            group: Group::Match,
        });
        assert!(matches!(plan(&sky), Plan::Encode(Role::Sky, _, _)));
        // Water simulation images are made at run time.
        let mut water = img("watersetup0", 2, MAPTYPE_2D);
        water.category = 5;
        assert!(matches!(plan(&water), Plan::Skip(_)));
        // References to images held by another zone have no data here.
        assert_eq!(
            plan(&img(",shadow", 2, 0)),
            Plan::Skip("reference to an image in another zone: converted with that zone")
        );
        // 3D maps are not used.
        let mut volume = img("test_volume", 2, 4);
        volume.depth = 16;
        assert!(matches!(plan(&volume), Plan::Skip(_)));
    }

    #[test]
    fn material_usage_beats_the_name() {
        // Named like a world texture, but a material uses it on a character with alpha test.
        let mut i = img("test_concrete", 2, MAPTYPE_2D);
        i.usage = Some(ImageUsage {
            role: Role::Character,
            alpha: AlphaUse::Cutout { threshold: 128 },
            source: "material: human skeleton model",
            group: Group::Match,
        });
        i.used_by_material = true;
        assert_eq!(
            plan(&i),
            Plan::Encode(
                Role::Character,
                Some(AlphaUse::Cutout { threshold: 128 }),
                "material: human skeleton model"
            )
        );
        // Referenced by materials, but never as colour: skipped even with a colour semantic on the image.
        let mut j = img("test_detail", 2, MAPTYPE_2D);
        j.used_by_material = true;
        assert!(matches!(plan(&j), Plan::Skip(_)));
    }

    #[test]
    fn provisional_roles() {
        assert_eq!(role_from_name("sky_test_ft"), Role::Sky);
        assert_eq!(role_from_name("weapon_test_col"), Role::Viewmodel);
        assert_eq!(role_from_name("hud_test"), Role::Ui);
        assert_eq!(role_from_name("fx_test"), Role::Effect);
        assert_eq!(role_from_name("mp_body_test"), Role::Character);
        assert_eq!(role_from_name("com_test_crate"), Role::Prop);
        assert_eq!(role_from_name("test_concrete"), Role::World);
    }

    #[test]
    fn in_zone_bgra_converts_without_iwds() {
        let mut i = img("test_wall", 2, MAPTYPE_2D);
        i.d3d_format = 21;
        i.payload = [10u8, 20, 30, 255].repeat(16);
        let (pixels, fmt, _) = source_pixels(&i, None).unwrap();
        assert_eq!(fmt, "bgra8");
        assert_eq!(pixels.len(), 1);
        assert_eq!(pixels[0].0, "");
        assert_eq!(pixels[0].1.pixels[0], [30, 20, 10, 255]);
        let streamed = img("test_streamed", 2, MAPTYPE_2D);
        assert!(source_pixels(&streamed, None).is_err());
    }

    #[test]
    fn group_stats_per_weapon_and_worst_case() {
        let totals = BTreeMap::from([("match", (3, 1000)), ("weapon", (3, 600))]);
        // Texture A (300 bytes) shared by weapons 0 and 1; B (200) only weapon 1; C (100) only weapon 2.
        let textures = [
            (BTreeSet::from([0, 1]), 300),
            (BTreeSet::from([1]), 200),
            (BTreeSet::from([2]), 100),
        ];
        let g = group_stats(&totals, &textures);
        assert_eq!(g.weapons, 3);
        // Per weapon: 0 -> 300, 1 -> 500, 2 -> 100.
        assert_eq!((g.weapon_median_bytes, g.weapon_max_bytes), (300, 500));
        // All three fit in the worst 8; shared texture counted once.
        assert_eq!(g.match_plus_worst_8_weapons_bytes, 1000 + 600);
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("a/b c~d"), "a_b_c_d");
        assert_eq!(file_stem(""), "unnamed");
    }
}

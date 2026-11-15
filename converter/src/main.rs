//! `mwf-convert`: the host-side converter. It runs on the user's own PC against their own install.
//!
//! Commands:
//!   mwf-convert inventory <game folder> [--out <report.json>]
//!       Lists every zone (.ff) with sizes and asset counts per type, flags the multiplayer maps,
//!       and writes a JSON report (default: out/inventory.json). The report contains only
//!       relative paths, sizes and counts, no game content, so it is safe to share.
//!   mwf-convert map <game folder> [--map <zone>] [--zone <file.ff>] [--out <report.json>]
//!       Walks one map zone (default mp_rust) with IW4L's loader and measures every asset entry: stream,
//!       resident and image/sound payload bytes, per type. Content-free like `inventory`.
//!       Default output: out/map_<zone>.json.
//!   mwf-convert textures <game folder> [--map <zone>] [--zone <file.ff>] [--out <folder>] [--verbose]
//!       Converts every texture (GfxImage) of one map zone to Wii `.gxt` files, taking pixels from the zone or
//!       from main/*.iwd, and writes a content-free report.json beside them. Default folder: out/textures/<zone>.
//!       --verbose prints one line per texture (with its name) to the console only.
//!   mwf-convert pack <game folder> [--map <zone> | all] [--zone <file.ff>] [--out <file.wpk>] [--force] [--slim-views] [--tiled-uv] [--small-dl] [--small-props] [--adpcm] [--slim-anims]
//!       Builds the Wii pack of one map: its textures (as `textures` does), lightmaps, materials, world geometry as
//!       display lists, skybox and spawns. Default output: out/packs/<zone>.wpk (+ a content-free .json report).
//!   mwf-convert preview <file.wpk> [--spawn <n>] [--pos x,y,z --yaw <deg> --pitch <deg>] [--out <file.png>]
//!       Software reference render of a pack from a spawn point (or a given view), reading only the pack the way the
//!       Wii does. 640x480, horizontal FOV 65. Default output: out/preview/<pack name>_<spawn>.png.
//!   mwf-convert texture <image.png> [--role <role>] [--out <folder>]
//!       Converts one PNG to a Wii texture (`.gxt`) plus a `_preview.png` of what the Wii shows,
//!       and prints the chosen format, size and quality. Default output folder: out/textures.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;

mod collision;
mod fx;
mod inventory;
mod light_grid;
mod map_walk;
mod material_links;
mod menu_preview;
mod pack;
mod pixel_decode;
mod player_anims;
mod preview;
mod report;
mod rules;
mod simplify;
mod sounds;
mod stamp;
mod static_models;
mod tex_report;
mod texture;
mod ui_bake;
mod ui_capture;
mod ui_tables;
mod viewmodel;
mod visibility;
mod weapons;
mod zone_textures;

/// The zone holding the shared multiplayer string tables.
const SHARED_TABLES_ZONE: &str = "code_post_gfx_mp";

const USAGE: &str = "usage:\n  mwf-convert inventory <game folder> [--out <report.json>]\n  \
     mwf-convert map <game folder> [--map <zone>] [--zone <file.ff>] [--out <report.json>]\n  \
     mwf-convert textures <game folder> [--map <zone>] [--zone <file.ff>] [--out <folder>] [--verbose]\n  \
     mwf-convert pack <game folder> [--map <zone> | all] [--zone <file.ff>] [--out <file.wpk>] [--force] [--slim-views] [--tiled-uv] [--small-dl] [--small-props] [--adpcm] [--slim-anims]\n  \
     mwf-convert report <log file>...\n  \
     mwf-convert preview <file.wpk> [--spawn <n>] [--pos x,y,z --yaw d --pitch d] [--out <file.png>]\n      [--viewmodel <view.wpk> --anim <slot> --time <s>] [--character <common.wpk> --team 0|1
      [--pmove idle|run|walk|sprint|idlecr|...] [--ptype autorifle|pistol|...] [--pclass rifle|...]
      [--pevent fireweapon|reload|death|...] [--paim <pitch>] [--pgun <n>] [--pturn <deg>]]
      [--fxpack <common.wpk> --fx <n> --time <s>]\n  \
     mwf-convert texture <image.png> [--role world|prop|character|viewmodel|effect|sky|ui|mask|minimap] [--out <folder>]";

/// IW4L's `ZoneStream` keeps the latest record of every asset type inline, so walking a zone needs far more stack than
/// Windows gives the main thread (1 MB). The commands run on a worker thread with room to spare.
const WORKER_STACK: usize = 256 << 20;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::thread::Builder::new()
        .name("mwf-convert".into())
        .stack_size(WORKER_STACK)
        .spawn(move || run(&args))
        .and_then(|t| {
            t.join()
                .map_err(|_| std::io::Error::other("converter thread panicked"))
        })
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            ExitCode::FAILURE
        })
}

fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("inventory") => run_inventory(&args[1..]),
        Some("map") => run_map(&args[1..]),
        Some("textures") => run_textures(&args[1..]),
        Some("pack") => run_pack(&args[1..]),
        Some("common") => run_common(&args[1..]),
        Some("menu-preview") => {
            let Some(path) = args.get(1) else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            let result = std::fs::read(path)
                .map_err(|e| format!("{path}: {e}"))
                .and_then(|b| {
                    std::fs::create_dir_all("out/preview").map_err(|e| e.to_string())?;
                    let mut written = Vec::new();
                    for screen in menu_preview::SCREENS {
                        let img = menu_preview::render(&b, screen)?;
                        let out = PathBuf::from(format!("out/preview/menu_{screen}.png"));
                        preview::write_png(&out, &img)?;
                        written.push(out);
                    }
                    let img = menu_preview::render_hud(&b)?;
                    let out = PathBuf::from("out/preview/menu_hud.png");
                    preview::write_png(&out, &img)?;
                    written.push(out);
                    // With a map pack too: that map's HUD minimap at its first spawn.
                    if let Some(map) = args.get(2) {
                        let m = std::fs::read(map).map_err(|e| format!("{map}: {e}"))?;
                        let img = menu_preview::render_minimap(&b, &m)?;
                        let out = PathBuf::from("out/preview/menu_minimap.png");
                        preview::write_png(&out, &img)?;
                        written.push(out);
                    }
                    Ok(written)
                });
            match result {
                Ok(written) => {
                    for out in written {
                        println!("menu preview -> {}", out.display());
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("report") => match report::run(&args[1..]) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        Some("preview") => run_preview(&args[1..]),
        Some("texture") => run_texture(&args[1..]),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command `{other}`\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run_inventory(args: &[String]) -> ExitCode {
    let mut game: Option<PathBuf> = None;
    let mut out = PathBuf::from("out/inventory.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => match it.next() {
                Some(p) => out = PathBuf::from(p),
                None => {
                    eprintln!("--out needs a path\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ if game.is_none() => game = Some(PathBuf::from(a)),
            _ => {
                eprintln!("unexpected argument `{a}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(game) = game else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if !game.is_dir() {
        eprintln!("not a folder: {}", game.display());
        return ExitCode::from(2);
    }

    let report = inventory::scan(&game);
    print!("{}", inventory::render_text(&report));

    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("could not create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&out, json) {
                eprintln!("could not write {}: {e}", out.display());
                return ExitCode::FAILURE;
            }
            println!("\nReport written to {}", out.display());
        }
        Err(e) => {
            eprintln!("could not serialise report: {e}");
            return ExitCode::FAILURE;
        }
    }
    if report.zones.is_empty() {
        eprintln!(
            "No .ff zone files found. Point this at the game folder (the one containing `zone`)."
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Arguments shared by the per-map commands: `<game folder> [--map <zone>] [--zone <file.ff>] [--out <path>]`.
#[derive(Clone)]
struct MapArgs {
    game: PathBuf,
    map: String,
    zone: Option<PathBuf>,
    out: Option<PathBuf>,
    /// Rebuild even when the output is up to date.
    force: bool,
    /// World mesh simplification (`--simplify <ratio>`, `--simplify-error <units>`).
    quality: simplify::WorldQuality,
    /// Smaller weapon files (`--slim-views`, or `MWF_SLIM_VIEWS=1`): keyframed clips with i16 translations, no
    /// texture mips, sounds at 32 kHz. Off by default while it's tested on hardware.
    slim_views: bool,
    /// Map packs: batches whose UVs span too far are split into UV tiles so they fit fixed-point vertices
    /// (`--tiled-uv`, or `MWF_TILED_UV=1`). Off by default while it's tested on hardware.
    tiled_uv: bool,
    /// Map packs: u8 display-list indices for batches of at most 256 vertices, and no colour index where every
    /// vertex is white (`--small-dl`, or `MWF_SMALL_DL=1`). Off by default while it's tested on hardware.
    small_dl: bool,
    /// Map packs: small props start at their second level of detail (`--small-props`, or `MWF_SMALL_PROPS=1`;
    /// `pack::first_lod`). Off by default while it's tested on hardware.
    small_props: bool,
    /// Player animations in `common.wpk` cut to the frames interpolation can't rebuild within
    /// `wpk::player_anim::SLIM` (`--slim-anims`, or `MWF_SLIM_ANIMS=1`). Off by default while it's tested on hardware.
    slim_anims: bool,
    /// Sounds (`common.wpk` and the weapon files) as 4-bit DSP ADPCM, 3.5x smaller than 16-bit PCM, decoded on the
    /// Wii as they play (`--adpcm`, or `MWF_ADPCM=1`). Off by default while it's tested on hardware.
    adpcm: bool,
}

/// Environment variable `name` set to 1 / true / yes / on.
fn env_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// `MWF_SLIM_VIEWS` set to 1 / true / yes / on.
fn slim_views_env() -> bool {
    env_on("MWF_SLIM_VIEWS")
}

fn parse_map_args(args: &[String]) -> Result<MapArgs, ExitCode> {
    let mut game: Option<PathBuf> = None;
    let mut map = String::from("mp_rust");
    let mut zone: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut force = false;
    let mut slim_views = slim_views_env();
    let mut tiled_uv = env_on("MWF_TILED_UV");
    let mut small_dl = env_on("MWF_SMALL_DL");
    let mut small_props = env_on("MWF_SMALL_PROPS");
    let mut adpcm = env_on("MWF_ADPCM");
    let mut slim_anims = env_on("MWF_SLIM_ANIMS");
    let mut quality = simplify::WorldQuality::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--force" {
            force = true;
            continue;
        }
        if a == "--slim-views" {
            slim_views = true;
            continue;
        }
        if a == "--tiled-uv" {
            tiled_uv = true;
            continue;
        }
        if a == "--small-dl" {
            small_dl = true;
            continue;
        }
        if a == "--small-props" {
            small_props = true;
            continue;
        }
        if a == "--adpcm" {
            adpcm = true;
            continue;
        }
        if a == "--slim-anims" {
            slim_anims = true;
            continue;
        }
        let target = match a.as_str() {
            "--map" | "--zone" | "--out" | "--simplify" | "--simplify-error" => a.as_str(),
            _ if game.is_none() => {
                game = Some(PathBuf::from(a));
                continue;
            }
            _ => {
                eprintln!("unexpected argument `{a}`\n{USAGE}");
                return Err(ExitCode::from(2));
            }
        };
        let Some(value) = it.next() else {
            eprintln!("{target} needs a value\n{USAGE}");
            return Err(ExitCode::from(2));
        };
        match target {
            "--map" => map = value.clone(),
            "--zone" => zone = Some(PathBuf::from(value)),
            "--simplify" => quality.ratio = value.parse().unwrap_or(1.0f32).clamp(0.05, 1.0),
            "--simplify-error" => quality.max_error = value.parse().unwrap_or(2.0f32).max(0.0),
            _ => out = Some(PathBuf::from(value)),
        }
    }
    let Some(game) = game else {
        eprintln!("{USAGE}");
        return Err(ExitCode::from(2));
    };
    Ok(MapArgs {
        game,
        map,
        zone,
        out,
        force,
        quality,
        slim_views,
        tiled_uv,
        small_dl,
        small_props,
        adpcm,
        slim_anims,
    })
}

/// Locate and inflate the map's zone. Returns the inflated bytes, a game-relative label and the file size.
fn open_map_zone(a: &MapArgs) -> Result<(Vec<u8>, String, u64), ExitCode> {
    let zone = match a
        .zone
        .clone()
        .map_or_else(|| map_walk::locate_zone(&a.game, &a.map), Ok)
    {
        Ok(z) => z,
        Err(e) => {
            eprintln!("{e}");
            return Err(ExitCode::FAILURE);
        }
    };
    // Relative label only: reports must not carry the install's absolute location.
    let label = zone
        .strip_prefix(&a.game)
        .unwrap_or(&zone)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    match asset_transport::open_zone(&zone) {
        Ok(i) => {
            let file_bytes = std::fs::metadata(&zone).map(|m| m.len()).unwrap_or(0);
            Ok((i.bytes, label, file_bytes))
        }
        Err(e) => {
            eprintln!("could not open {label}: {e}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn write_json(out: &std::path::Path, report: &impl serde::Serialize) -> ExitCode {
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("could not create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    let written = serde_json::to_string_pretty(report)
        .map_err(|e| e.to_string())
        .and_then(|json| std::fs::write(out, json).map_err(|e| e.to_string()));
    match written {
        Ok(()) => {
            println!("\nReport written to {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("could not write {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}

fn run_map(args: &[String]) -> ExitCode {
    let a = match parse_map_args(args) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (image, label, file_bytes) = match open_map_zone(&a) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let report = match map_walk::walk(&image, &a.map, &label, file_bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("walking {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", map_walk::render_text(&report));
    let out = a
        .out
        .unwrap_or_else(|| PathBuf::from(format!("out/map_{}.json", a.map)));
    write_json(&out, &report)
}

fn run_textures(args: &[String]) -> ExitCode {
    let verbose = args.iter().any(|a| a == "--verbose");
    let args: Vec<String> = args.iter().filter(|a| *a != "--verbose").cloned().collect();
    let a = match parse_map_args(&args) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (image, label, _) = match open_map_zone(&a) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let out_dir = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("out/textures/{}", a.map)));
    match convert_textures(&a, &image, &label, &out_dir, verbose) {
        Ok(_) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// The shared string tables (calling cards, emblems, icon tables) in code_post_gfx_mp: materials they name are looked
/// up by name at run time, so they go in the `catalog` group.
fn shared_tables(a: &MapArgs) -> std::collections::HashSet<String> {
    if a.map == SHARED_TABLES_ZONE {
        return Default::default();
    }
    let Ok(path) = map_walk::locate_zone(&a.game, SHARED_TABLES_ZONE) else {
        return Default::default();
    };
    match asset_transport::open_zone(&path)
        .map_err(|e| e.to_string())
        .and_then(|z| zone_textures::table_strings(&z.bytes))
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("warning: could not read {SHARED_TABLES_ZONE} string tables ({e})");
            Default::default()
        }
    }
}

/// Convert a zone's textures into `out_dir` (`.gxt`, `index.json`, `report.json`).
fn convert_textures(
    a: &MapArgs,
    image: &[u8],
    label: &str,
    out_dir: &std::path::Path,
    verbose: bool,
) -> Result<
    (
        Vec<zone_textures::IndexEntry>,
        std::collections::HashSet<String>,
    ),
    ExitCode,
> {
    // Streamed images live in main/*.iwd; without it only in-zone images convert.
    let iwds = match zone_textures::main_dir(&a.game).map(|m| asset_transport::IwdIndex::open(&m)) {
        Some(Ok(index)) => Some(index),
        Some(Err(e)) => {
            eprintln!("warning: could not index main/*.iwd ({e}); streamed images will fail");
            None
        }
        None => None,
    };
    let tables = shared_tables(a);
    let (report, index) = match zone_textures::convert_zone(
        image,
        &a.map,
        label,
        iwds.as_deref(),
        out_dir,
        verbose,
        &tables,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("converting textures of {label}: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    print!("{}", zone_textures::render_text(&report));
    println!(
        "
Wii textures (.gxt) written to {}",
        out_dir.display()
    );
    // The index names textures and weapons, so it is converted data like the .gxt files (git-ignored under out/).
    if write_json(&out_dir.join("index.json"), &index) != ExitCode::SUCCESS
        || write_json(&out_dir.join("report.json"), &report) != ExitCode::SUCCESS
    {
        return Err(ExitCode::FAILURE);
    }
    Ok((index, tables))
}

/// The 16 multiplayer maps of the base game (`pack --map all`).
const BASE_MAPS: [&str; 16] = [
    "mp_afghan",
    "mp_boneyard",
    "mp_brecourt",
    "mp_checkpoint",
    "mp_derail",
    "mp_estate",
    "mp_favela",
    "mp_highrise",
    "mp_invasion",
    "mp_nightshift",
    "mp_quarry",
    "mp_rundown",
    "mp_rust",
    "mp_subbase",
    "mp_terminal",
    "mp_underpass",
];

fn run_pack(args: &[String]) -> ExitCode {
    let a = match parse_map_args(args) {
        Ok(a) => a,
        Err(code) => return code,
    };
    if a.map == "all" {
        let mut failed = Vec::new();
        for m in BASE_MAPS {
            println!("== {m}");
            let one = MapArgs {
                map: m.to_owned(),
                out: None,
                zone: None,
                ..a.clone()
            };
            if run_pack_one(one) != ExitCode::SUCCESS {
                failed.push(m);
            }
        }
        println!("== common");
        let mut common_args = vec![a.game.display().to_string()];
        if a.force {
            common_args.push("--force".to_owned());
        }
        if a.slim_views {
            common_args.push("--slim-views".to_owned());
        }
        if a.adpcm {
            common_args.push("--adpcm".to_owned());
        }
        if a.slim_anims {
            common_args.push("--slim-anims".to_owned());
        }
        if run_common(&common_args) != ExitCode::SUCCESS {
            failed.push("common");
        }
        println!(
            "Packed {} of {} maps into out/packs/",
            BASE_MAPS.len() - failed.len(),
            BASE_MAPS.len()
        );
        if !failed.is_empty() {
            eprintln!("failed: {}", failed.join(", "));
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
    run_pack_one(a)
}

/// The zone the weapon sounds are read from: MW2 MP's sound aliases and their loaded sounds live in
/// `localized_common_mp` (`common_mp` only names them).
const SOUND_ZONE: &str = "localized_common_mp";

/// Zones with the Create-a-Class tables and their localized names.
const CLASS_ZONES: [&str; 2] = ["code_post_gfx_mp", "localized_code_post_gfx_mp"];

/// Walk zone `zone` of the install for its links (sounds, tables, localized strings), or `None` when unreadable.
fn walk_zone(a: &MapArgs, zone: &str) -> Option<material_links::Links> {
    let args = MapArgs {
        map: zone.to_owned(),
        zone: None,
        ..a.clone()
    };
    let (image, label, _) = open_map_zone(&args).ok()?;
    match zone_textures::walk_with(&image, &HashSet::new(), |_, _| ()) {
        Ok((c, ())) => Some(c.links),
        Err(e) => {
            eprintln!("reading {label}: {e}");
            None
        }
    }
}

/// The data every map shares, from `common_mp`: weapons (combat numbers) for now. â†’ `out/packs/common.wpk`.
/// MW2's menu fonts and screens from the UI zones (`ui_capture::UI_ZONES`), their textures added to `tex`.
/// Bake the menus from the UI zones. The same walk collects the game-type scripts the match rules come from into
/// `scripts` (a later zone's copy wins: `patch_mp` over `common_mp`).
fn build_ui(
    a: &MapArgs,
    tex: &mut pack::Textures,
    scripts: &mut HashMap<String, String>,
) -> Option<(wpk::ui::UiPack, ui_bake::BakeReport)> {
    struct Zone {
        links: material_links::Links,
        dir: PathBuf,
        files_by_image: HashMap<usize, String>,
    }
    let mut zones = Vec::new();
    let mut ui = ui_capture::UiCapture::default();
    let mut localize = HashMap::new();
    for z in ui_capture::UI_ZONES {
        let args = MapArgs {
            map: z.to_owned(),
            zone: None,
            ..a.clone()
        };
        let Ok((image, label, _)) = open_map_zone(&args) else {
            continue;
        };
        let dir = PathBuf::from(format!("out/textures/{z}"));
        let Ok((index, tables)) = convert_textures(&args, &image, &label, &dir, false) else {
            continue;
        };
        let Ok((c, ())) = zone_textures::walk_with(&image, &tables, |_, _| ()) else {
            continue;
        };
        let mut files_by_image = HashMap::new();
        for e in &index {
            for &i in &e.images {
                files_by_image.entry(i).or_insert_with(|| e.file.clone());
            }
        }
        let mut links = c.links;
        ui.absorb(std::mem::take(&mut links.ui));
        for (k, v) in std::mem::take(&mut links.localize) {
            localize.entry(k).or_insert(v);
        }
        scripts.extend(std::mem::take(&mut links.scripts));
        zones.push(Zone {
            links,
            dir,
            files_by_image,
        });
    }
    if ui.menus.is_empty() && ui.fonts.is_empty() {
        return None;
    }
    // The class panel's pictures and icons (`wpk::ui::image_key`), from MW2's tables: a weapon's picture
    // (statsTable column 6 by base name, column 4), an attachment's icon (attachmentTable 6 by 4), a perk's,
    // deathstreak's or equipment's icon (perkTable 3 by 1). Pictures are drawn about 200 × 100, icons 36 × 36 on the
    // 640 × 480 screen: capped at these many texels across.
    const CLASS_PICTURE_MAX: u32 = 96;
    const CLASS_ICON_MAX: u32 = 48;
    let mut class_images: Vec<(String, String)> = Vec::new();
    for z in &zones {
        // Only what Create-a-Class offers: the class weapons (and special grenades), every attachment, and the
        // perkTable rows a class can take.
        let bases = ui_tables::weapon_bases(&z.links.tables);
        let offered = |table: &str, t: &ui_tables::Table, r: usize| match table {
            "mp/statstable.csv" => {
                t.cell(r, 2) == "weapon_grenade" || bases.iter().any(|b| b == t.cell(r, 4))
            }
            "mp/perktable.csv" => matches!(
                t.cell(r, 5),
                "perk1" | "perk2" | "perk3" | "perk4" | "equipment"
            ),
            _ => true,
        };
        for (table, kind, key, image) in [
            ("mp/statstable.csv", "weapon", 4, 6),
            ("mp/attachmenttable.csv", "attachment", 4, 6),
            ("mp/perktable.csv", "perk", 1, 3),
        ] {
            let Some(t) = z.links.tables.get(table) else {
                continue;
            };
            for r in 1..t.rows() {
                let (name, material) = (t.cell(r, key), t.cell(r, image));
                let k = format!("img:{kind}:{name}");
                if offered(table, t, r)
                    && !name.is_empty()
                    && !material.is_empty()
                    && !material.eq_ignore_ascii_case("specialty_null")
                    && !class_images.iter().any(|(x, _)| *x == k)
                {
                    class_images.push((k, material.to_owned()));
                }
            }
        }
    }
    let capped: HashMap<String, u32> = class_images
        .iter()
        .map(|(k, m)| {
            let max = if k.starts_with("img:weapon:") {
                CLASS_PICTURE_MAX
            } else {
                CLASS_ICON_MAX
            };
            (m.to_ascii_lowercase(), max)
        })
        .collect();
    // A material's colour texture, from the first zone that has the material (class pictures capped).
    let lookup = |name: &str, tex: &mut pack::Textures| -> Option<u16> {
        if name.eq_ignore_ascii_case("white") {
            return Some(tex.add_white());
        }
        let cap = capped.get(&name.to_ascii_lowercase()).copied();
        zones.iter().find_map(|z| {
            let m = z.links.material_info.iter().position(|m| {
                m.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })?;
            let img = z.links.colour_image(m)?;
            let file = z.files_by_image.get(&img)?;
            if let Some(max) = cap {
                tex.add_file_capped(&z.dir, file, wpk::records::Wrap::Clamp, max)
                    .ok()
            } else {
                tex.add_file(&z.dir, file, wpk::records::Wrap::Clamp).ok()
            }
        })
    };
    // The HUD's images: MW2's hitmarker (`damage_feedback`), each weapon's HUD icon (`weapon:<name>`) and each
    // killstreak's icon (`streak:<name>`: killstreakTable column 14).
    let mut extra: Vec<(String, String)> =
        vec![("damage_feedback".into(), "damage_feedback".into())];
    // One key per icon: the shortest weapon name using it (the base weapon; attachment variants share its icon, and
    // the HUD falls back to the base name).
    let mut by_icon: HashMap<String, String> = HashMap::new();
    for z in &zones {
        for (n, icon) in z.links.weapons.iter().zip(&z.links.weapon_hud_icons) {
            if let Some(m) = icon.as_ref().filter(|m| !n.is_empty() && !m.is_empty()) {
                let e = by_icon.entry(m.clone()).or_insert_with(|| n.clone());
                if n.len() < e.len() {
                    *e = n.clone();
                }
            }
        }
    }
    let mut weapon_icons: Vec<(String, String)> = by_icon
        .into_iter()
        .map(|(m, n)| (format!("weapon:{n}"), m))
        .collect();
    weapon_icons.sort();
    extra.extend(weapon_icons);
    for z in &zones {
        if let Some(t) = z.links.tables.get("mp/killstreaktable.csv") {
            for r in 1..t.rows() {
                let (name, icon) = (t.cell(r, 1), t.cell(r, 14));
                let key = format!("streak:{name}");
                if !name.is_empty()
                    && icon.starts_with("specialty_")
                    && !extra.iter().any(|(k, _)| *k == key)
                {
                    extra.push((key, icon.to_owned()));
                }
            }
        }
    }
    extra.extend(class_images);
    Some(ui_bake::bake(&ui, &localize, &extra, |name| {
        lookup(name, tex)
    }))
}

fn run_common(args: &[String]) -> ExitCode {
    let mut a = match parse_map_args(args) {
        Ok(a) => a,
        Err(code) => return code,
    };
    a.map = "common_mp".to_owned();
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("out/packs/common.wpk"));
    let view_dir = out
        .parent()
        .map_or_else(|| PathBuf::from("views"), |d| d.join("views"));
    let mut zones = vec!["common_mp", SOUND_ZONE];
    zones.extend(CLASS_ZONES);
    zones.extend(ui_capture::UI_ZONES);
    let st = stamp_of(&a, &zones)
        .with("slim-views", a.slim_views)
        .with("adpcm", a.adpcm)
        .with("slim-anims", a.slim_anims);
    if a.slim_views {
        println!(
            "common: slim weapon files (keyframed clips, no gun mips, sounds at {} Hz)",
            sounds::SLIM_RATE
        );
    }
    if a.adpcm {
        println!("common: sounds as 4-bit DSP ADPCM");
    }
    let views_there = std::fs::read_dir(&view_dir).is_ok_and(|mut d| d.next().is_some());
    if !a.force && views_there && st.up_to_date(&out) {
        println!(
            "common: up to date ({} and {})",
            out.display(),
            view_dir.display()
        );
        return ExitCode::SUCCESS;
    }
    // Create-a-Class: MW2's class tables and the names they use (read first: they list the weapons to convert).
    let mut localize = HashMap::new();
    let mut tables = HashMap::new();
    for zone in CLASS_ZONES {
        match walk_zone(&a, zone) {
            Some(l) => {
                localize.extend(l.localize);
                tables.extend(l.tables);
            }
            None => eprintln!("warning: {zone} unreadable: no class data"),
        }
    }
    let bases = ui_tables::weapon_bases(&tables);
    let (image, label, _) = match open_map_zone(&a) {
        Ok(x) => x,
        Err(code) => return code,
    };
    // Textures of the zone (the viewmodels' come from here).
    let tex_dir = PathBuf::from("out/textures/common_mp");
    let (index, table_cells) = match convert_textures(&a, &image, &label, &tex_dir, false) {
        Ok(x) => x,
        Err(code) => return code,
    };
    // The first-person model of every weapon a class can carry.
    let walked = zone_textures::walk_with(&image, &table_cells, |s, links| {
        let mut views = Vec::new();
        let mut failed = 0usize;
        // The weapons in the characters' hands: each base weapon's world model, its textures shared.
        let mut held: Vec<(String, wpk::held::Model)> = Vec::new();
        let mut held_tex = pack::Textures::default();
        for base in &bases {
            let name = format!("{base}_mp");
            let Some(w) = links.weapons.iter().position(|n| *n == name) else {
                continue;
            };
            let view = &links.weapon_views[w];
            if let Some(m) = view
                .world
                .as_deref()
                .and_then(|n| links.models.iter().position(|x| x.name == n))
            {
                match viewmodel::build_held(
                    s,
                    links,
                    m,
                    &view.hide,
                    &index,
                    &tex_dir,
                    &mut held_tex,
                ) {
                    Ok(model) => held.push((base.clone(), model)),
                    Err(e) => eprintln!("warning: {base}'s third-person model: {e}"),
                }
            }
            let mut tex = pack::Textures::default();
            tex.base_level_only = a.slim_views;
            match viewmodel::build(s, links, w, &index, &tex_dir, &mut tex) {
                Ok((vm, report)) => {
                    let view = &links.weapon_views[w];
                    let notes = sounds::view_notes(s, links, view);
                    views.push(View {
                        base: base.clone(),
                        name,
                        vm,
                        report,
                        tex,
                        sounds: view.sounds.clone(),
                        notes,
                    });
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("warning: a viewmodel failed: {e}");
                }
            }
        }
        // MW2's player animation script and the animations it plays (by bone name), for the characters.
        // The player-anim types of what a player can hold: the classes' weapons and every offhand (grenades,
        // equipment).
        let used_types = links
            .weapon_combat
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let name = links.weapons[*i].as_str();
                let offhand = links
                    .weapon_equip
                    .get(*i)
                    .is_some_and(|e| e.offhand_class != 0);
                offhand
                    || bases
                        .iter()
                        .any(|b| name.strip_suffix("_mp") == Some(b.as_str()))
            })
            .filter_map(|(_, w)| u32::try_from(w.player_anim_type).ok())
            .filter(|&t| t < 64)
            .fold(0u64, |b, t| b | 1 << t);
        let has = |n: &str| links.xanims.iter().any(|(x, _)| x.eq_ignore_ascii_case(n));
        let parsed = player_anims::parse(
            links
                .scripts
                .get(player_anims::SCRIPT)
                .map_or("", String::as_str),
            links
                .scripts
                .get(player_anims::TYPES)
                .map_or("", String::as_str),
            used_types,
            &has,
        );
        let anims = viewmodel::bake_player_anims(s, links, &parsed.anims);
        (views, failed, (anims, parsed), (held, held_tex))
    });
    let (mut collector, (views, failed, player_anims, (held, mut held_tex))) = match walked {
        Ok(x) => x,
        Err(e) => {
            eprintln!("reading {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The weapons' sound aliases and clips come from the localized zone.
    let mut capture = std::mem::take(&mut collector.links.sounds);
    match walk_zone(&a, SOUND_ZONE) {
        Some(l) => {
            capture.absorb(l.sounds);
            for (k, v) in l.localize {
                localize.entry(k).or_insert(v);
            }
        }
        None => eprintln!("warning: no weapon sounds ({SOUND_ZONE} unreadable)"),
    }
    let links = &collector.links;
    // Weapons with their recoil, sorted by name, first of each name kept.
    type Row = (
        wpk::weapon::WeaponRecord,
        (
            wpk::weapon::KickRecord,
            wpk::weapon::MoveRecord,
            wpk::weapon::AimRecord,
            wpk::weapon::FpvRecord,
            wpk::weapon::EquipRecord,
            [Option<String>; 4],
        ),
    );
    let mut both: Vec<Row> = links
        .weapons
        .iter()
        .zip(&links.weapon_combat)
        .zip(
            links
                .weapon_kick
                .iter()
                .zip(&links.weapon_moves)
                .zip(&links.weapon_aims)
                .zip(&links.weapon_fpv)
                .zip(&links.weapon_equip)
                .zip(links.weapon_world_sounds.iter().enumerate()),
        )
        .filter(|((n, _), _)| !n.is_empty())
        .map(|((n, c), (((((k, m), a), v), e), (i, ws)))| {
            // MW2's per-location damage table, baked into each weapon as IW4L does.
            let c = weapons::with_global_location(*c, links.lochit);
            (
                wpk::weapon::WeaponRecord::new(n, c),
                (*k, *m, *a, *v, *e, {
                    // Grenades without their own explosion sound use the impact table's.
                    let mut ws = ws.clone();
                    if ws[0].is_none() {
                        ws[0] = links.impact_explosion_sound(i);
                    }
                    ws
                }),
            )
        })
        .collect();
    both.sort_by_key(|(w, _)| w.name);
    both.dedup_by(|x, y| x.0.name == y.0.name);
    let (weapons, extra): (Vec<_>, Vec<_>) = both.into_iter().unzip();
    let kicks: Vec<_> = extra.iter().map(|e| e.0).collect();
    let moves: Vec<_> = extra.iter().map(|e| e.1).collect();
    let aims: Vec<_> = extra.iter().map(|e| e.2).collect();
    let fpvs: Vec<_> = extra.iter().map(|e| e.3).collect();
    // Thrown weapons' world sounds (explosions, bounces, pull back, throw) in the common pack's own bank.
    let mut world_bank = sounds::Bank::default();
    world_bank.adpcm = a.adpcm;
    let equips: Vec<_> = extra
        .iter()
        .map(|e| {
            let mut r = e.4;
            if r.offhand_class != 0 {
                for (slot, name) in r.sounds.iter_mut().zip(&e.5) {
                    if let Some(n) = name {
                        *slot = world_bank.add(&capture, n);
                    }
                }
            }
            r
        })
        .collect();
    // Footsteps (IW4L `audio::aliases`): `step_<gait>[_plr]_<surface>`, else the gait's `_default`, per gait (sprint,
    // run, walk, prone), listener (others, you) and surface type.
    let mut steps = vec![wpk::sound::NONE; wpk::sound::STEP_COUNT];
    for (g, gait) in ["sprint", "run", "walk", "prone"].iter().enumerate() {
        for (l, plr) in ["", "_plr"].iter().enumerate() {
            let fallback = format!("step_{gait}{plr}_default");
            let fallback_idx = world_bank.add(&capture, &fallback);
            for (s, surf) in movement_iw4::SURFACE_TYPE_NAMES.iter().enumerate() {
                let name = format!("step_{gait}{plr}_{surf}");
                let idx = if capture.aliases.contains_key(&name) {
                    world_bank.add(&capture, &name)
                } else {
                    fallback_idx
                };
                steps[wpk::sound::step_index(g, l, s)] = idx;
            }
        }
    }
    let files: HashSet<String> = weapons.iter().map(|w| w.name().to_owned()).collect();
    println!(
        "hit-location damage table: {}",
        if links.lochit.is_some() {
            "found"
        } else {
            "not found (every location x1)"
        }
    );
    let (catalog, cr) = ui_tables::catalog(&tables, &localize, &files);
    println!(
        "classes: {} weapons ({} without a weapon file), {} attachments, {} perks, {} default and {} custom classes, {} names missing",
        cr.weapons,
        cr.weapons_without_file,
        cr.attachments,
        cr.perks,
        catalog.defaults.len(),
        catalog.customs.len(),
        cr.missing_names
    );
    for t in &cr.missing_tables {
        eprintln!("warning: class table {t} not found");
    }
    let mut b = wpk::PackBuilder::new("common");
    b.section(wpk::tag::STAMP, 0, st.encode());
    b.section(
        wpk::tag::WEAPONS,
        weapons.len() as u32,
        wpk::records::encode(&weapons),
    );
    b.section(
        wpk::tag::KICK,
        kicks.len() as u32,
        wpk::records::encode(&kicks),
    );
    b.section(
        wpk::tag::WEAPON_MOVE,
        moves.len() as u32,
        wpk::records::encode(&moves),
    );
    b.section(
        wpk::tag::WEAPON_AIM,
        aims.len() as u32,
        wpk::records::encode(&aims),
    );
    b.section(
        wpk::tag::WEAPON_FPV,
        fpvs.len() as u32,
        wpk::records::encode(&fpvs),
    );
    b.section(
        wpk::tag::WEAPON_EQUIP,
        equips.len() as u32,
        wpk::records::encode(&equips),
    );
    // Smoke: the sight-blocking elements of each thrown weapon's explosion effect (the first weapon of each name, as
    // the sorted list keeps).
    let blockers: Vec<wpk::weapon::BlockerRecord> = weapons
        .iter()
        .enumerate()
        .filter(|(k, _)| equips.get(*k).is_some_and(|e| e.offhand_class != 0))
        .flat_map(|(k, w)| {
            let i = links.weapons.iter().position(|n| n == w.name());
            i.and_then(|i| links.weapon_blockers.get(i))
                .into_iter()
                .flatten()
                .map(move |b| wpk::weapon::BlockerRecord {
                    weapon: k as u32,
                    looping: b.looping,
                    spawn_a: b.spawn_a,
                    spawn_b: b.spawn_b,
                    delay_ms: b.delay_ms,
                    life_ms: b.life_ms,
                    offset_radius: b.offset_radius,
                    offset_height: b.offset_height,
                    fade_in: b.fade_in,
                    fade_out: b.fade_out,
                    size: b.size,
                    alpha: b.alpha,
                })
        })
        .collect();
    println!("smoke: {} sight-blocking elements", blockers.len());
    b.section(
        wpk::tag::WEAPON_BLOCKERS,
        blockers.len() as u32,
        wpk::records::encode(&blockers),
    );
    println!(
        "world sounds: {} aliases, {} variants, {:.2} MiB",
        world_bank.aliases.len(),
        world_bank.variants.len(),
        world_bank.data.len() as f64 / (1024.0 * 1024.0)
    );
    b.section(
        wpk::tag::SOUND_ALIASES,
        world_bank.aliases.len() as u32,
        wpk::records::encode(&world_bank.aliases),
    );
    b.section(
        wpk::tag::SOUND_VARIANTS,
        world_bank.variants.len() as u32,
        wpk::records::encode(&world_bank.variants),
    );
    let world_sounds = world_bank.report();
    b.section(
        wpk::tag::SOUND_DATA,
        sounds::data_count(&world_bank),
        std::mem::take(&mut world_bank.data),
    );
    // Player animations by bone name.
    let ((panm, panb, missing_anims), parsed) = player_anims;
    let full_panm = wpk::player_anim::encode(&panm.clips);
    let panm_bytes = if a.slim_anims {
        let slim: Vec<_> = panm
            .clips
            .iter()
            .map(|c| wpk::player_anim::slim(c, wpk::player_anim::SLIM))
            .collect();
        let b = wpk::player_anim::encode(&slim);
        println!(
            "player animations: slim ({:.2} MiB -> {:.2} MiB, within {}° / {} units)",
            full_panm.len() as f64 / (1024.0 * 1024.0),
            b.len() as f64 / (1024.0 * 1024.0),
            wpk::player_anim::SLIM.rot_deg,
            wpk::player_anim::SLIM.trans
        );
        b
    } else {
        full_panm.clone()
    };
    let player_anim_report = PlayerAnimReport {
        clips: panm.clips.len(),
        slim: a.slim_anims,
        full_bytes: full_panm.len() as u64,
        bytes: panm_bytes.len() as u64,
    };
    println!(
        "player animations: {} clips ({:.2} MiB), {} bones, {} missing; script: {} items ({} dropped, {} skipped), {} unresolved",
        panm.clips.len(),
        panm_bytes.len() as f64 / (1024.0 * 1024.0),
        panb.len(),
        missing_anims,
        parsed.items,
        parsed.dropped_items,
        parsed.skipped_items,
        parsed.unresolved,
    );
    if parsed.script.moves.is_empty() {
        eprintln!(
            "warning: no player animation script ({}): characters stand still",
            player_anims::SCRIPT
        );
    }
    // Effects: the class weapons' flashes and explosions, every thrown weapon's explosion, and the impact table's
    // rows for every impact type those weapons have, with the effects their runners start.
    let class_weapon = |i: usize| {
        let name = links.weapons[i].as_str();
        bases.iter().any(|b| name == format!("{b}_mp"))
    };
    let offhand = |i: usize| {
        links
            .weapon_equip
            .get(i)
            .is_some_and(|e| e.offhand_class != 0)
    };
    let mut fx_roots: Vec<String> = Vec::new();
    let mut rows: Vec<usize> = Vec::new();
    for i in 0..links.weapons.len() {
        let (class, off) = (class_weapon(i), offhand(i));
        if !class && !off {
            continue;
        }
        if let Some(f) = links.weapon_fx.get(i) {
            let pick: &[usize] = if class { &[0, 1, 2] } else { &[2] };
            fx_roots.extend(pick.iter().filter_map(|&k| f[k].clone()));
        }
        if let Some(r) = links
            .weapon_impact_types
            .get(i)
            .and_then(|&t| fx_iw4::impact_table_row(t, false))
            && !rows.contains(&r)
        {
            rows.push(r);
        }
    }
    let impact_table: Vec<[Option<String>; wpk::fx::IMPACT_COLUMNS]> = links
        .impact_table
        .iter()
        .take(wpk::fx::IMPACT_ROWS)
        .enumerate()
        .map(|(r, row)| {
            if rows.contains(&r) {
                row.clone()
            } else {
                std::array::from_fn(|_| None)
            }
        })
        .collect();
    fx_roots.extend(impact_table.iter().flatten().flatten().cloned());
    let mut fx_tex = pack::Textures::default();
    let baked_fx = fx::bake(links, &fx_roots, &index, &tex_dir, &mut fx_tex);
    let fx_defs = baked_fx.encode();
    let mut fx_weapons: Vec<u8> = Vec::with_capacity(weapons.len() * 6);
    for w in &weapons {
        let f = links
            .weapons
            .iter()
            .position(|n| n == w.name())
            .and_then(|i| links.weapon_fx.get(i));
        for k in 0..3 {
            let i = baked_fx.index(f.and_then(|f| f[k].as_deref()));
            fx_weapons.extend_from_slice(&i.to_be_bytes());
        }
    }
    println!(
        "effects: {} ({} elements, {} left out, {} not in the zone), impact table {} rows, {} textures, {:.2} MiB",
        baked_fx.len(),
        baked_fx.elems,
        baked_fx.dropped_elems,
        baked_fx.missing,
        links.impact_table.len(),
        fx_tex.records.len(),
        (fx_defs.len() + fx_tex.data.len()) as f64 / (1024.0 * 1024.0),
    );
    b.section(wpk::tag::FX_DEFS, baked_fx.len() as u32, fx_defs);
    b.section(wpk::tag::FX_WEAPONS, weapons.len() as u32, fx_weapons);
    b.section(
        wpk::tag::FX_IMPACTS,
        wpk::fx::IMPACT_ROWS as u32,
        fx::impact_section(&baked_fx, &impact_table),
    );
    b.section(
        wpk::tag::FX_TEXTURES,
        fx_tex.records.len() as u32,
        wpk::records::encode(&fx_tex.records),
    );
    b.section(
        wpk::tag::FX_TEXTURE_DATA,
        0,
        std::mem::take(&mut fx_tex.data),
    );
    // The weapons in the characters' hands: each weapon record takes its base weapon's model (the longest base
    // its name starts with).
    let held_index: Vec<u16> = weapons
        .iter()
        .map(|w| {
            let n = w.name();
            held.iter()
                .enumerate()
                .filter(|(_, (base, _))| {
                    n == format!("{base}_mp") || n.starts_with(&format!("{base}_"))
                })
                .max_by_key(|(_, (base, _))| base.len())
                .map_or(wpk::records::NONE, |(i, _)| i as u16)
        })
        .collect();
    let held_models: Vec<wpk::held::Model> = held.into_iter().map(|(_, m)| m).collect();
    let held_bytes = wpk::held::encode(&held_models);
    println!(
        "weapons in hand: {} models ({} vertices), {} textures, {:.2} MiB; {} of {} weapons have one",
        held_models.len(),
        held_models.iter().map(|m| m.positions.len()).sum::<usize>(),
        held_tex.records.len(),
        (held_bytes.len() + held_tex.data.len()) as f64 / (1024.0 * 1024.0),
        held_index
            .iter()
            .filter(|&&i| i != wpk::records::NONE)
            .count(),
        weapons.len(),
    );
    b.section(wpk::tag::HELD_MODELS, held_models.len() as u32, held_bytes);
    b.section(
        wpk::tag::HELD_INDEX,
        held_index.len() as u32,
        held_index.iter().flat_map(|v| v.to_be_bytes()).collect(),
    );
    b.section(
        wpk::tag::HELD_TEXTURES,
        held_tex.records.len() as u32,
        wpk::records::encode(&held_tex.records),
    );
    b.section(
        wpk::tag::HELD_TEXTURE_DATA,
        0,
        std::mem::take(&mut held_tex.data),
    );
    b.section(
        wpk::tag::PLAYER_ANIM_SCRIPT,
        (parsed.script.moves.len() + parsed.script.events.len()) as u32,
        parsed.script.encode(),
    );
    b.section(wpk::tag::PLAYER_ANIMS, panm.clips.len() as u32, panm_bytes);
    b.section(
        wpk::tag::PLAYER_ANIM_BONES,
        panb.len() as u32,
        wpk::view::encode_names(&panb),
    );
    b.section(
        wpk::tag::FOOTSTEPS,
        steps.len() as u32,
        steps.iter().flat_map(|v| v.to_be_bytes()).collect(),
    );
    // Bullet penetration: each weapon's type and multiplier (first weapon of each name), and the depth table.
    let pens: Vec<wpk::weapon::PenRecord> = weapons
        .iter()
        .map(|w| {
            links
                .weapons
                .iter()
                .position(|n| n == w.name())
                .and_then(|i| links.weapon_pen.get(i).copied())
                .unwrap_or_default()
        })
        .collect();
    b.section(
        wpk::tag::WEAPON_PEN,
        pens.len() as u32,
        wpk::records::encode(&pens),
    );
    match links.pen_table {
        Some(t) => {
            b.section(
                wpk::tag::PEN_TABLE,
                (4 * 31) as u32,
                t.iter().flatten().flat_map(|v| v.to_be_bytes()).collect(),
            );
            println!("bullet penetration table: found");
        }
        None => println!("bullet penetration table: not found (bullets stop at walls)"),
    }
    b.section(wpk::tag::CLASSES, 0, catalog.encode());
    // Menus: MW2's fonts and baked screens, with their textures.
    let mut ui_tex = pack::Textures::default();
    ui_tex.role = "ui";
    let mut scripts = HashMap::new();
    let built = build_ui(&a, &mut ui_tex, &mut scripts);
    if let Some((u, _)) = &built {
        let count = |p: &str| u.icons.iter().filter(|(k, _)| k.starts_with(p)).count();
        println!(
            "HUD icons: {} weapons, {} killstreaks, hitmarker {}",
            count("weapon:"),
            count("streak:"),
            if count("damage_feedback") > 0 {
                "found"
            } else {
                "missing"
            }
        );
    }
    let rules = rules::read(&scripts);
    println!(
        "match rules: {} of {} values read from the game-type scripts ({} scripts found)",
        rules.found.count_ones() + rules.found_team.count_ones() + rules.found_fx.count_ones(),
        wpk::rules::found::ALL.count_ones()
            + wpk::rules::found::ALL_TEAM.count_ones()
            + wpk::rules::found::ALL_FX.count_ones(),
        scripts.len()
    );
    let missing = rules::missing(&rules);
    if !missing.is_empty() {
        println!("  kept MW2's stock value for: {}", missing.join(", "));
    }
    b.section(wpk::tag::RULES, 0, rules.encode());
    match built {
        Some((ui, r)) => {
            println!(
                "compass icons: {} of {}",
                ui.icons.len(),
                ui_bake::ICONS.len()
            );
            println!(
                "menus: {} fonts ({} glyphs), {} screens ({} images), {} textures, {} materials without a texture",
                r.fonts,
                r.glyphs,
                r.screens,
                r.quads,
                ui_tex.records.len(),
                r.missing_textures
            );
            b.section(
                wpk::tag::TEXTURES,
                ui_tex.records.len() as u32,
                wpk::records::encode(&ui_tex.records),
            );
            b.section(wpk::tag::TEXTURE_DATA, 0, std::mem::take(&mut ui_tex.data));
            b.section(wpk::tag::UI, 0, ui.encode());
        }
        None => eprintln!("warning: no menu data (UI zones unreadable)"),
    }
    let bytes = b.finish();
    if !write_file(&out, &bytes) {
        return ExitCode::FAILURE;
    }
    let mut report = CommonReport {
        pack_bytes: bytes.len() as u64,
        sections: section_sizes(&bytes),
        sounds: world_sounds,
        player_anims: player_anim_report,
        textures: tex_report::breakdown(&held_tex.records, &held_tex.roles),
        ..CommonReport::default()
    };
    report.textures.merge(&ui_tex.records, &ui_tex.roles);
    println!(
        "Modern Wiifare common pack: {} weapons, {:.2} MiB -> {}",
        weapons.len(),
        bytes.len() as f64 / (1024.0 * 1024.0),
        out.display()
    );
    // One file per first-person weapon: its model, textures and sounds (`views/<weapon>.wpk`, loaded when a class
    // carries it).
    let (mut total, mut variants, mut missing_sounds, mut missing_clips) = (0usize, 0, 0, 0);
    let count = views.len();
    let views_report = &mut report.views;
    for mut v in views {
        let mut bank = sounds::Bank::default();
        bank.max_rate = a.slim_views.then_some(sounds::SLIM_RATE);
        bank.adpcm = a.adpcm;
        let snd = sounds::weapon_sounds(&capture, &mut bank, &v.sounds, &v.notes);
        let w = weapons.iter().position(|x| x.name() == v.name).unwrap_or(0);
        let mut b = wpk::PackBuilder::new(&v.base);
        b.section(
            wpk::tag::TEXTURES,
            v.tex.records.len() as u32,
            wpk::records::encode(&v.tex.records),
        );
        b.section(wpk::tag::TEXTURE_DATA, 0, std::mem::take(&mut v.tex.data));
        // `count` = the weapon's record index in WEAP.
        let view_blob = if a.slim_views {
            v.vm.encode_compact(wpk::view::Tolerance::SLIM)
        } else {
            v.vm.encode()
        };
        b.section(wpk::tag::VIEWMODEL, w as u32, view_blob);
        b.section(wpk::tag::WEAPON_SOUNDS, w as u32, snd.encode());
        b.section(
            wpk::tag::SOUND_ALIASES,
            bank.aliases.len() as u32,
            wpk::records::encode(&bank.aliases),
        );
        b.section(
            wpk::tag::SOUND_VARIANTS,
            bank.variants.len() as u32,
            wpk::records::encode(&bank.variants),
        );
        views_report.sounds.merge(&bank.report());
        views_report.textures.merge(&v.tex.records, &v.tex.roles);
        b.section(
            wpk::tag::SOUND_DATA,
            sounds::data_count(&bank),
            std::mem::take(&mut bank.data),
        );
        let bytes = b.finish();
        if !write_file(&view_dir.join(format!("{}.wpk", v.base)), &bytes) {
            return ExitCode::FAILURE;
        }
        total += bytes.len();
        views_report.files += 1;
        for (k, v) in section_sizes(&bytes) {
            *views_report.sections.entry(k).or_default() += v;
        }
        variants += bank.variants.len();
        missing_sounds += bank.missing;
        missing_clips += v.report.missing_clips;
    }
    println!(
        "first-person weapons: {count} files ({failed} failed), {:.2} MiB, {variants} sound variants ({missing_sounds} aliases without a clip), {missing_clips} animations missing -> {}",
        total as f64 / (1024.0 * 1024.0),
        view_dir.display()
    );
    report.views.bytes = total as u64;
    print!(
        "common pack {}weapon files {}",
        tex_report::render_text(&report.textures),
        tex_report::render_text(&report.views.textures)
    );
    for (what, s) in [
        ("common", &report.sounds),
        ("weapon files", &report.views.sounds),
    ] {
        println!(
            "{what} sounds ({}): {} clips, {:.0} s, {:.2} MiB",
            s.encoding,
            s.clips,
            s.seconds,
            s.bytes as f64 / (1024.0 * 1024.0)
        );
    }
    write_json(&out.with_extension("json"), &report)
}

/// What `common.wpk` and the weapon files hold (content-free: sizes and counts, no names). Written beside the pack.
#[derive(Default, serde::Serialize)]
struct CommonReport {
    pack_bytes: u64,
    sections: BTreeMap<String, u64>,
    /// Held weapons' and menus' textures.
    textures: tex_report::Breakdown,
    /// The world sounds (footsteps, explosions, throws).
    sounds: sounds::BankReport,
    player_anims: PlayerAnimReport,
    views: ViewFilesReport,
}

/// `PANM`: its clips, and its size with and without `--slim-anims`.
#[derive(Default, serde::Serialize)]
struct PlayerAnimReport {
    clips: usize,
    slim: bool,
    full_bytes: u64,
    bytes: u64,
}

/// All `views/<weapon>.wpk` together.
#[derive(Default, serde::Serialize)]
struct ViewFilesReport {
    files: usize,
    bytes: u64,
    sections: BTreeMap<String, u64>,
    textures: tex_report::Breakdown,
    sounds: sounds::BankReport,
}

/// Each section's bytes by tag (summed when a tag repeats).
fn section_sizes(bytes: &[u8]) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    if let Ok(p) = wpk::Pack::parse(bytes, false) {
        for (t, len) in p.section_sizes() {
            *m.entry(String::from_utf8_lossy(&t).into_owned())
                .or_default() += len as u64;
        }
    }
    m
}

/// A first-person weapon read from `common_mp`, written to its own file once its sounds are known.
struct View {
    base: String,
    name: String,
    vm: wpk::view::ViewModel,
    report: viewmodel::ViewReport,
    tex: pack::Textures,
    sounds: sounds::WeaponSoundNames,
    notes: Vec<(u8, String, f32)>,
}

/// Write `bytes` to `path`, creating its folder; false (with a message) on failure.
fn write_file(path: &std::path::Path, bytes: &[u8]) -> bool {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("could not create {}: {e}", dir.display());
        return false;
    }
    if let Err(e) = std::fs::write(path, bytes) {
        eprintln!("could not write {}: {e}", path.display());
        return false;
    }
    true
}

/// The stamp of an output made from these zones of the install (zones that aren't there are left out).
fn stamp_of(a: &MapArgs, zones: &[&str]) -> stamp::Stamp {
    let paths: Vec<PathBuf> = zones
        .iter()
        .filter_map(|z| map_walk::locate_zone(&a.game, z).ok())
        .collect();
    stamp::Stamp::of(&paths)
}

/// The player animations `common.wpk` carries: MW2's body animation for each player animation slot.
fn run_pack_one(a: MapArgs) -> ExitCode {
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("out/packs/{}.wpk", a.map)));
    // The map's zone, and the zone with the shared tables its textures are looked up in.
    let st = match &a.zone {
        Some(z) => stamp::Stamp::of(std::slice::from_ref(z)),
        None => stamp_of(&a, &[&a.map, "code_post_gfx_mp"]),
    }
    .with("tiled-uv", a.tiled_uv)
    .with("small-dl", a.small_dl)
    .with("small-props", a.small_props);
    if !a.force && st.up_to_date(&out) {
        println!("{}: up to date ({})", a.map, out.display());
        return ExitCode::SUCCESS;
    }
    let (image, label, _) = match open_map_zone(&a) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let tex_dir = PathBuf::from(format!("out/textures/{}", a.map));
    let (index, tables) = match convert_textures(&a, &image, &label, &tex_dir, false) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let walked = zone_textures::walk_with(&image, &tables, pack::read_world);
    let (collector, world) = match walked {
        Ok((c, Ok(w))) => (c, w),
        Ok((_, Err(e))) | Err(e) => {
            eprintln!("reading the world of {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (bytes, report) = match pack::build(
        &a.map,
        &world,
        &collector.links,
        &collector.images,
        &index,
        &tex_dir,
        &st,
        &a.quality,
        &pack::Options {
            tiled_uv: a.tiled_uv,
            small_dl: a.small_dl,
            small_props: a.small_props,
        },
    ) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("building the pack of {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("could not create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(&out, &bytes) {
        eprintln!("could not write {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    print!(
        "
{}",
        pack::render_text(&report)
    );
    println!("Pack written to {}", out.display());
    write_json(&out.with_extension("json"), &report)
}

fn run_texture(args: &[String]) -> ExitCode {
    let mut input: Option<PathBuf> = None;
    let mut role = gx_texture::Role::World;
    let mut out = PathBuf::from("out/textures");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--role" => match it.next().and_then(|r| gx_texture::Role::parse(r)) {
                Some(r) => role = r,
                None => {
                    eprintln!("--role needs one of the roles below\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--out" => match it.next() {
                Some(p) => out = PathBuf::from(p),
                None => {
                    eprintln!("--out needs a folder\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ if input.is_none() => input = Some(PathBuf::from(a)),
            _ => {
                eprintln!("unexpected argument `{a}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(input) = input else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match texture::convert_file(&input, role, &out) {
        Ok((summary, gxt, preview)) => {
            println!("{} as {}:", input.display(), role.name());
            println!("{}", texture::describe(&summary));
            println!("wrote {}\nwrote {}", gxt.display(), preview.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run_preview(args: &[String]) -> ExitCode {
    let mut file: Option<PathBuf> = None;
    let (mut spawn, mut pos, mut yaw, mut pitch, mut out) = (0usize, None, None, 0.0f32, None);
    let (mut view_pack, mut anim, mut anim_time): (Option<PathBuf>, u8, f32) =
        (None, wpk::view::slot::IDLE, 0.0);
    let (mut char_pack, mut char_team): (Option<PathBuf>, usize) = (None, 0);
    let mut char_shot = preview::CharacterShot::default();
    let (mut fx_index, mut fx_pack): (Option<u16>, Option<PathBuf>) = (None, None);
    // `--fxweapon <name>:<0 world flash | 1 view flash | 2 explosion>` picks the effect by weapon.
    let mut fx_weapon: Option<String> = None;
    // `--fximpact <IW4 impact type>:<surface type>` picks the impact table's effect.
    let mut fx_impact: Option<String> = None;
    // An IW4 name's index in `names` (any case), else `default`.
    let name_index = |n: &str, names: &[&str], default: u8| {
        names
            .iter()
            .position(|x| x.eq_ignore_ascii_case(n))
            .map_or(default, |i| i as u8)
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--spawn" => spawn = value().parse().unwrap_or(0),
            "--pos" => {
                let v: Vec<f32> = value()
                    .split(',')
                    .filter_map(|t| t.trim().parse().ok())
                    .collect();
                pos = (v.len() == 3).then(|| [v[0], v[1], v[2]]);
            }
            "--yaw" => yaw = value().parse().ok(),
            "--pitch" => pitch = value().parse().unwrap_or(0.0),
            "--out" => out = Some(PathBuf::from(value())),
            "--viewmodel" => view_pack = Some(PathBuf::from(value())),
            "--anim" => anim = value().parse().unwrap_or(wpk::view::slot::IDLE),
            "--character" => char_pack = Some(PathBuf::from(value())),
            "--team" => char_team = value().parse().unwrap_or(0),
            "--pmove" => char_shot.movetype = name_index(&value(), anim_iw4::ANIM_MT_NAMES, 1),
            "--ptype" => {
                char_shot.anim_type = name_index(&value(), anim_iw4::ANIM_PLAYERANIMTYPE_NAMES, 4)
            }
            "--pclass" => {
                char_shot.class = name_index(&value(), anim_iw4::ANIM_WEAPONCLASS_NAMES, 0)
            }
            "--pevent" => char_shot.event = Some(name_index(&value(), anim_iw4::ANIM_ET_NAMES, 2)),
            "--paim" => char_shot.aim_pitch = value().parse().unwrap_or(0.0),
            "--pgun" => char_shot.gun = value().parse().ok(),
            "--pturn" => char_shot.turn = value().parse().unwrap_or(0.0),
            "--fx" => fx_index = value().parse().ok(),
            "--fxweapon" => fx_weapon = Some(value()),
            "--fximpact" => fx_impact = Some(value()),
            "--fxpack" => fx_pack = Some(PathBuf::from(value())),
            "--time" => anim_time = value().parse().unwrap_or(0.0),
            _ if file.is_none() => file = Some(PathBuf::from(a)),
            _ => {
                eprintln!("unexpected argument `{a}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let (eye, view_yaw) = match pos {
        Some(p) => (p, yaw.unwrap_or(0.0)),
        None => match preview::spawns(&bytes).map(|s| s.get(spawn).copied()) {
            Ok(Some(s)) => (
                [s.origin[0], s.origin[1], s.origin[2] + 60.0],
                yaw.unwrap_or(s.yaw),
            ),
            Ok(None) => {
                eprintln!("the pack has no spawn {spawn}; use --pos");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let fx_index = fx_index.or_else(|| {
        let (t, surf) = fx_impact.as_deref()?.split_once(':')?;
        let row = fx_iw4::impact_table_row(t.parse().ok()?, false)?;
        let b = std::fs::read(fx_pack.as_ref()?).ok()?;
        let p = wpk::Pack::parse(&b, true).ok()?;
        let at = (row * wpk::fx::IMPACT_COLUMNS + surf.parse::<usize>().ok()?) * 2;
        let fx = p.section(wpk::tag::FX_IMPACTS)?.data;
        let i = u16::from_be_bytes([*fx.get(at)?, *fx.get(at + 1)?]);
        println!("impact {t} on {surf}: effect {i}");
        Some(i)
    });
    let fx_index = fx_index.or_else(|| {
        let (name, kind) = fx_weapon.as_deref()?.split_once(':')?;
        let b = std::fs::read(fx_pack.as_ref()?).ok()?;
        let p = wpk::Pack::parse(&b, true).ok()?;
        let w = wpk::records::iter::<wpk::weapon::WeaponRecord>(p.section(wpk::tag::WEAPONS)?.data)
            .position(|w| w.name() == name)?;
        let at = (w * 3 + kind.parse::<usize>().ok()?) * 2;
        let fx = p.section(wpk::tag::FX_WEAPONS)?.data;
        let i = u16::from_be_bytes([*fx.get(at)?, *fx.get(at + 1)?]);
        println!("{name}: effect {i}");
        Some(i)
    });
    let view = preview::View {
        eye,
        yaw: view_yaw,
        pitch,
        width: 640,
        height: 480,
        hfov_deg: 65.0,
        character: char_pack
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| preview::CharacterShot {
                common: b,
                team: char_team,
                time: anim_time,
                ..char_shot
            }),
        effect: fx_pack
            .zip(fx_index)
            .and_then(|(p, i)| std::fs::read(p).ok().map(|b| (b, i, anim_time))),
        viewmodel: match view_pack {
            Some(p) => match std::fs::read(&p) {
                Ok(b) => Some((b, anim, anim_time)),
                Err(e) => {
                    eprintln!("{}: {e}", p.display());
                    return ExitCode::FAILURE;
                }
            },
            None => None,
        },
    };
    let t = std::time::Instant::now();
    let (img, stats) = match preview::render(&bytes, &view) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = out.unwrap_or_else(|| PathBuf::from(format!("out/preview/{stem}_{spawn}.png")));
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = preview::write_png(&out, &img) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    println!(
        "{} from ({:.0}, {:.0}, {:.0}) yaw {:.0}: {} batches drawn, {} culled, {} models past draw distance, {} triangles, {} pixels, {} ms",
        out.display(),
        eye[0],
        eye[1],
        eye[2],
        view_yaw,
        stats.batches_drawn,
        stats.batches_culled,
        stats.models_culled,
        stats.triangles,
        stats.pixels_drawn,
        t.elapsed().as_millis()
    );
    ExitCode::SUCCESS
}

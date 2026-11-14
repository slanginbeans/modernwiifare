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
//!   mwf-convert pack <game folder> [--map <zone> | all] [--zone <file.ff>] [--out <file.wpk>] [--force]
//!       Builds the Wii pack of one map: its textures (as `textures` does), lightmaps, materials, world geometry as
//!       display lists, skybox and spawns. Default output: out/packs/<zone>.wpk (+ a content-free .json report).
//!   mwf-convert preview <file.wpk> [--spawn <n>] [--pos x,y,z --yaw <deg> --pitch <deg>] [--out <file.png>]
//!       Software reference render of a pack from a spawn point (or a given view), reading only the pack the way the
//!       Wii does. 640x480, horizontal FOV 65. Default output: out/preview/<pack name>_<spawn>.png.
//!   mwf-convert texture <image.png> [--role <role>] [--out <folder>]
//!       Converts one PNG to a Wii texture (`.gxt`) plus a `_preview.png` of what the Wii shows,
//!       and prints the chosen format, size and quality. Default output folder: out/textures.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;

mod collision;
mod inventory;
mod light_grid;
mod map_walk;
mod material_links;
mod menu_preview;
mod pack;
mod pixel_decode;
mod preview;
mod report;
mod rules;
mod sounds;
mod stamp;
mod static_models;
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
     mwf-convert pack <game folder> [--map <zone> | all] [--zone <file.ff>] [--out <file.wpk>] [--force]\n  \
     mwf-convert report <log file>...\n  \
     mwf-convert preview <file.wpk> [--spawn <n>] [--pos x,y,z --yaw d --pitch d] [--out <file.png>]\n  \
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
}

fn parse_map_args(args: &[String]) -> Result<MapArgs, ExitCode> {
    let mut game: Option<PathBuf> = None;
    let mut map = String::from("mp_rust");
    let mut zone: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--force" {
            force = true;
            continue;
        }
        let target = match a.as_str() {
            "--map" | "--zone" | "--out" => a.as_str(),
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
    // A material's colour texture, from the first zone that has the material.
    let lookup = |name: &str, tex: &mut pack::Textures| -> Option<u16> {
        if name.eq_ignore_ascii_case("white") {
            return Some(tex.add_white());
        }
        zones.iter().find_map(|z| {
            let m = z.links.material_info.iter().position(|m| {
                m.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })?;
            let img = z.links.colour_image(m)?;
            let file = z.files_by_image.get(&img)?;
            tex.add_file(&z.dir, file, wpk::records::Wrap::Clamp).ok()
        })
    };
    Some(ui_bake::bake(&ui, &localize, |name| lookup(name, tex)))
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
    let st = stamp_of(&a, &zones);
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
        for base in &bases {
            let name = format!("{base}_mp");
            let Some(w) = links.weapons.iter().position(|n| *n == name) else {
                continue;
            };
            let mut tex = pack::Textures::default();
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
        (views, failed)
    });
    let (mut collector, (views, failed)) = match walked {
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
    b.section(
        wpk::tag::SOUND_DATA,
        0,
        std::mem::take(&mut world_bank.data),
    );
    b.section(wpk::tag::CLASSES, 0, catalog.encode());
    // Menus: MW2's fonts and baked screens, with their textures.
    let mut ui_tex = pack::Textures::default();
    let mut scripts = HashMap::new();
    let built = build_ui(&a, &mut ui_tex, &mut scripts);
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
    for mut v in views {
        let mut bank = sounds::Bank::default();
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
        b.section(wpk::tag::VIEWMODEL, w as u32, v.vm.encode());
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
        b.section(wpk::tag::SOUND_DATA, 0, std::mem::take(&mut bank.data));
        let bytes = b.finish();
        if !write_file(&view_dir.join(format!("{}.wpk", v.base)), &bytes) {
            return ExitCode::FAILURE;
        }
        total += bytes.len();
        variants += bank.variants.len();
        missing_sounds += bank.missing;
        missing_clips += v.report.missing_clips;
    }
    println!(
        "first-person weapons: {count} files ({failed} failed), {:.2} MiB, {variants} sound variants ({missing_sounds} aliases without a clip), {missing_clips} animations missing -> {}",
        total as f64 / (1024.0 * 1024.0),
        view_dir.display()
    );
    ExitCode::SUCCESS
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

fn run_pack_one(a: MapArgs) -> ExitCode {
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("out/packs/{}.wpk", a.map)));
    // The map's zone, and the zone with the shared tables its textures are looked up in.
    let st = match &a.zone {
        Some(z) => stamp::Stamp::of(std::slice::from_ref(z)),
        None => stamp_of(&a, &[&a.map, "code_post_gfx_mp"]),
    };
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
    let view = preview::View {
        eye,
        yaw: view_yaw,
        pitch,
        width: 640,
        height: 480,
        hfov_deg: 65.0,
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

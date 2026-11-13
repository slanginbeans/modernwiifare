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
//!   mwf-convert texture <image.png> [--role <role>] [--out <folder>]
//!       Converts one PNG to a Wii texture (`.gxt`) plus a `_preview.png` of what the Wii shows,
//!       and prints the chosen format, size and quality. Default output folder: out/textures.

use std::path::PathBuf;
use std::process::ExitCode;

mod inventory;
mod map_walk;
mod texture;

const USAGE: &str = "usage:\n  mwf-convert inventory <game folder> [--out <report.json>]\n  \
     mwf-convert map <game folder> [--map <zone>] [--zone <file.ff>] [--out <report.json>]\n  \
     mwf-convert texture <image.png> [--role world|prop|character|viewmodel|effect|sky|ui|mask] [--out <folder>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("inventory") => run_inventory(&args[1..]),
        Some("map") => run_map(&args[1..]),
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

fn run_map(args: &[String]) -> ExitCode {
    let mut game: Option<PathBuf> = None;
    let mut map = String::from("mp_rust");
    let mut zone: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let target = match a.as_str() {
            "--map" | "--zone" | "--out" => a.as_str(),
            _ if game.is_none() => {
                game = Some(PathBuf::from(a));
                continue;
            }
            _ => {
                eprintln!("unexpected argument `{a}`\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        let Some(value) = it.next() else {
            eprintln!("{target} needs a value\n{USAGE}");
            return ExitCode::from(2);
        };
        match target {
            "--map" => map = value.clone(),
            "--zone" => zone = Some(PathBuf::from(value)),
            _ => out = Some(PathBuf::from(value)),
        }
    }
    let Some(game) = game else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let zone = match zone.map_or_else(|| map_walk::locate_zone(&game, &map), Ok) {
        Ok(z) => z,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // Relative label only: the report must not carry the install's absolute location.
    let label = zone
        .strip_prefix(&game)
        .unwrap_or(&zone)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let image = match asset_transport::open_zone(&zone) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("could not open {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let file_bytes = std::fs::metadata(&zone).map(|m| m.len()).unwrap_or(0);
    let report = match map_walk::walk(&image.bytes, &map, &label, file_bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("walking {label}: {e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", map_walk::render_text(&report));

    let out = out.unwrap_or_else(|| PathBuf::from(format!("out/map_{map}.json")));
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("could not create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    let written = serde_json::to_string_pretty(&report)
        .map_err(|e| e.to_string())
        .and_then(|json| std::fs::write(&out, json).map_err(|e| e.to_string()));
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

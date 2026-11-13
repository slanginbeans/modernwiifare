//! `mwf-convert`: the host-side converter. It runs on the user's own PC against their own install.
//!
//! Commands:
//!   mwf-convert inventory <game folder> [--out <report.json>]
//!       Lists every zone (.ff) with sizes and asset counts per type, flags the multiplayer maps,
//!       and writes a JSON report (default: out/inventory.json). The report contains only
//!       relative paths, sizes and counts, no game content, so it is safe to share.

use std::path::PathBuf;
use std::process::ExitCode;

mod inventory;

const USAGE: &str = "usage: mwf-convert inventory <game folder> [--out <report.json>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("inventory") => run_inventory(&args[1..]),
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

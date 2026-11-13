//! `mwf-convert`: the host converter.
//!
//! Phase 0 / Spike D only does an inventory: it finds the user's IW4x install, reports the zone layout, then opens one
//! map zone through IW4L (`asset_transport` inflates it, `fastfile_iw4` walks it) and lists every asset with its size.
//!
//! Everything it writes goes under `out/` (git-ignored). Asset names come from the user's own data, so the report
//! must never be committed (CLAUDE.md rule 1).

mod inventory;
mod layout;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: mwf-convert inventory --game <IW4x install dir> [--map <zone>] [--out <dir>] [--zone <file.ff>]

  --game   folder that holds iw4x.exe and zone/ (your own install)
  --map    zone to inventory (default: mp_rust, the smallest base MP map)
  --out    output folder (default: out); the report goes to <out>/spike_d/<map>.tsv
  --zone   inventory this .ff instead of zone/english/<map>.ff (name the report with --map)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or_else(|| USAGE.to_owned())?;
    if cmd != "inventory" {
        return Err(USAGE.to_owned());
    }
    let mut game: Option<PathBuf> = None;
    let mut map = String::from("mp_rust");
    let mut out = PathBuf::from("out");
    let mut zone: Option<PathBuf> = None;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--game" => game = Some(PathBuf::from(value()?)),
            "--map" => map = value()?,
            "--out" => out = PathBuf::from(value()?),
            "--zone" => zone = Some(PathBuf::from(value()?)),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown flag {other}\n{USAGE}")),
        }
    }
    let game = game.ok_or_else(|| format!("--game is required\n{USAGE}"))?;

    let layout = layout::scan(&game)?;
    print!("{}", layout.render());

    let zone_path = match zone {
        Some(path) => path,
        None => layout::locate_zone(&game, &map)?,
    };
    println!("\n== Zone {map} ==");
    println!("Inventorying: {}", zone_path.display());

    let t0 = std::time::Instant::now();
    let image = asset_transport::open_zone(&zone_path).map_err(|e| e.to_string())?;
    let inflate_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    let inv = inventory::take(&image.bytes).map_err(|e| format!("walking {map}: {e}"))?;
    let walk_ms = t1.elapsed().as_millis();

    let file_len = std::fs::metadata(&zone_path).map(|m| m.len()).unwrap_or(0);
    print!("{}", inv.render_summary(file_len, inflate_ms, walk_ms));

    let dir = out.join("spike_d");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tsv = dir.join(format!("{map}.tsv"));
    std::fs::write(&tsv, inv.render_tsv()).map_err(|e| format!("{}: {e}", tsv.display()))?;
    println!(
        "\nPer-asset table: {} (git-ignored; never commit it)",
        tsv.display()
    );
    Ok(())
}

//! Install-layout check: does IW4L recognise an IW4x install, and how are its zones laid out?

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use asset_transport::{GamesRoot, ZoneGame};

const IW4_VERSION: u32 = fastfile_iw4::ZONE_VERSION_PC;

#[derive(Default)]
pub struct ZoneDir {
    pub files: usize,
    pub bytes: u64,
    /// fastfile version -> count (0 = not a readable IWff envelope)
    pub versions: BTreeMap<u32, usize>,
}

pub struct Layout {
    pub root: PathBuf,
    pub iw4l_detects_iw4: bool,
    pub has_iw4x_dll: bool,
    pub zone_dirs: BTreeMap<String, ZoneDir>,
    pub iwds: Vec<(String, u64)>,
}

pub fn scan(game: &Path) -> Result<Layout, String> {
    if !game.join("zone").is_dir() {
        return Err(format!(
            "{} has no zone/ folder; point --game at the install root",
            game.display()
        ));
    }
    let mut zone_dirs: BTreeMap<String, ZoneDir> = BTreeMap::new();
    let mut pending = vec![game.join("zone")];
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ff"))
            {
                continue;
            }
            let rel = dir
                .strip_prefix(game)
                .unwrap_or(&dir)
                .to_string_lossy()
                .replace('\\', "/");
            let d = zone_dirs.entry(rel).or_default();
            d.files += 1;
            d.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            let v = asset_transport::peek_zone_version(&path).unwrap_or(0);
            *d.versions.entry(v).or_default() += 1;
        }
    }
    let mut iwds = Vec::new();
    if let Ok(entries) = std::fs::read_dir(game.join("main")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".iwd") {
                iwds.push((name, entry.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    iwds.sort();
    Ok(Layout {
        root: game.to_owned(),
        iw4l_detects_iw4: asset_transport::folder_holds_game(game, ZoneGame::Iw4),
        has_iw4x_dll: game.join("iw4x.dll").is_file(),
        zone_dirs,
        iwds,
    })
}

/// Pick the stock zone and report what IW4L's own search picks, since an IW4x install can also hold custom maps of
/// the same name under `usermaps/` or `mods/`. Current IW4x moves the retail x86 zones to `zone/iw4x/x86/english/`
/// and rewrites `zone/english/` as x64; the retail x86 bytes are preferred (Spike D, docs/spikes/results.md).
pub fn locate_zone(game: &Path, map: &str) -> Result<PathBuf, String> {
    let iw4l = asset_transport::find_zone_file(&GamesRoot(game.to_owned()), map);
    match &iw4l {
        Ok(found) => {
            println!("IW4L find_zone_file picks: {}", found.path.display());
            if let Some(note) = &found.alias_note {
                println!("IW4L alias note: {note}");
            }
        }
        Err(e) => println!("IW4L find_zone_file: {e}"),
    }
    let file = format!("{map}.ff");
    let candidates = [
        game.join("zone")
            .join("iw4x")
            .join("x86")
            .join("english")
            .join(&file),
        game.join("zone").join("english").join(&file),
    ];
    if let Some(stock) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(stock);
    }
    iw4l.map(|f| f.path)
}

pub fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

impl Layout {
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "== Install {} ==", self.root.display());
        let _ = writeln!(s, "IW4L folder_holds_game(Iw4): {}", self.iw4l_detects_iw4);
        let _ = writeln!(s, "iw4x.dll present: {}", self.has_iw4x_dll);
        let _ = writeln!(
            s,
            "{:<32} {:>6} {:>10}  versions",
            "zone dir", "files", "MiB"
        );
        for (dir, d) in &self.zone_dirs {
            let versions: Vec<String> = d
                .versions
                .iter()
                .map(|(v, n)| match *v {
                    IW4_VERSION => format!("IW4 0x{v:x} x{n}"),
                    0 => format!("unreadable x{n}"),
                    _ => format!("0x{v:x} x{n}"),
                })
                .collect();
            let _ = writeln!(
                s,
                "{dir:<32} {:>6} {:>10.1}  {}",
                d.files,
                mib(d.bytes),
                versions.join(", ")
            );
        }
        let iwd_total: u64 = self.iwds.iter().map(|(_, n)| n).sum();
        let _ = writeln!(
            s,
            "main/*.iwd: {} files, {:.1} MiB",
            self.iwds.len(),
            mib(iwd_total)
        );
        s
    }
}

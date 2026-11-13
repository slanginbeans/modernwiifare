//! End-to-end checks of `mwf-convert inventory` on a synthetic folder (no game data).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mwf_convert_test_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn reports_unreadable_zones_without_crashing_or_leaking_paths() {
    let game = temp_dir("game");
    fs::create_dir_all(game.join("zone/english")).unwrap();
    // Not a real fastfile: must be reported as an error, not a crash.
    fs::write(
        game.join("zone/english/mp_fake.ff"),
        b"definitely not a fastfile",
    )
    .unwrap();
    fs::write(game.join("zone/english/readme.txt"), b"ignored").unwrap();
    let out = game.join("report.json");

    let run = Command::new(env!("CARGO_BIN_EXE_mwf-convert"))
        .args([
            "inventory",
            game.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("mp_fake"), "{stdout}");
    assert!(stdout.contains("ERROR"), "{stdout}");

    let json = fs::read_to_string(&out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["totals"]["zones"], 1);
    assert_eq!(v["totals"]["zones_failed"], 1);
    assert_eq!(v["zones"][0]["path"], "zone/english/mp_fake.ff");
    assert_eq!(v["zones"][0]["kind"], "mp_map");
    // The report must not contain the absolute location of the user's install.
    let parent = game.parent().unwrap().to_string_lossy().into_owned();
    assert!(!json.contains(&parent), "absolute path leaked into report");
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn empty_folder_fails_with_hint() {
    let game = temp_dir("empty");
    let run = Command::new(env!("CARGO_BIN_EXE_mwf-convert"))
        .args([
            "inventory",
            game.to_str().unwrap(),
            "--out",
            game.join("r.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("No .ff zone files found"));
    let _ = fs::remove_dir_all(&game);
}

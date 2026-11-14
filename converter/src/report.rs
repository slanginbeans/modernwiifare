//! `mwf-convert report <log>...`: a short summary of the Wii's test logs (`game_core::telemetry` format) to paste
//! back: the session header and load, frame-time percentiles and the CPU breakdown, the worst seconds and spikes,
//! and every unusual event. Logs from a crash (no `P` line) are summarised from their rows.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

/// `key=value` pairs of a log line (after its record word).
fn fields(line: &str) -> BTreeMap<&str, &str> {
    line.split_whitespace()
        .skip(1)
        .filter_map(|kv| kv.split_once('='))
        .collect()
}

fn num(v: Option<&&str>) -> f64 {
    v.and_then(|s| s.split('/').next())
        .and_then(|s| s.trim_end_matches('k').parse().ok())
        .unwrap_or(0.0)
}

/// Events that are routine and only counted.
const ROUTINE: [&str; 6] = [
    "hit",
    "kill",
    "log_write",
    "pause",
    "switch",
    "class_change",
];

pub fn summarize(name: &str, text: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let _ = writeln!(out, "== {name}");
    if let Some(h) = lines.iter().find(|l| l.starts_with("H ")) {
        let _ = writeln!(out, "{}", &h[2..]);
    }
    let rows: Vec<BTreeMap<&str, &str>> = lines
        .iter()
        .filter(|l| l.starts_with("S "))
        .map(|l| fields(l))
        .collect();
    let spikes: Vec<&&str> = lines.iter().filter(|l| l.starts_with("X ")).collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unusual = Vec::new();
    let mut write_max_us = 0.0f64;
    for l in lines.iter().filter(|l| l.starts_with("E ")) {
        let what = l.split_whitespace().nth(2).unwrap_or("?");
        *counts.entry(what).or_default() += 1;
        if what == "log_write" {
            write_max_us = write_max_us.max(num(fields(l).get("us")));
        }
        if !ROUTINE.contains(&what) {
            unusual.push(*l);
        }
    }
    match lines.iter().rev().find(|l| l.starts_with("P ")) {
        Some(p) => {
            let _ = writeln!(out, "summary: {}", &p[2..]);
        }
        None => {
            let _ = writeln!(
                out,
                "summary: none (the session ended without one: crash, power off or reset) - from {} rows:",
                rows.len()
            );
            if !rows.is_empty() {
                let n = rows.len() as f64;
                let mut keys: Vec<&str> = rows[0].keys().copied().filter(|k| *k != "t").collect();
                keys.sort();
                let avg: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        format!(
                            "{k}={:.2}",
                            rows.iter().map(|r| num(r.get(k))).sum::<f64>() / n
                        )
                    })
                    .collect();
                let _ = writeln!(out, "  avg {}", avg.join(" "));
            }
        }
    }
    // The five worst seconds by average frame time.
    let mut worst: Vec<&BTreeMap<&str, &str>> = rows.iter().collect();
    worst.sort_by(|a, b| num(b.get("fr")).total_cmp(&num(a.get("fr"))));
    if !worst.is_empty() {
        let _ = writeln!(out, "worst seconds:");
        for r in worst.iter().take(5) {
            let line: Vec<String> = r.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let _ = writeln!(out, "  {}", line.join(" "));
        }
    }
    let _ = writeln!(out, "spikes over 33 ms: {} logged", spikes.len());
    let mut by_frame: Vec<&&&str> = spikes.iter().collect();
    by_frame.sort_by(|a, b| num(fields(b).get("fr")).total_cmp(&num(fields(a).get("fr"))));
    for s in by_frame.iter().take(5) {
        let _ = writeln!(out, "  {}", &s[2..]);
    }
    let c: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let _ = writeln!(out, "events: {}", c.join(" "));
    if write_max_us > 0.0 {
        let _ = writeln!(out, "slowest log write: {:.1} ms", write_max_us / 1000.0);
    }
    for u in unusual.iter().take(30) {
        let _ = writeln!(out, "  {}", &u[2..]);
    }
    if unusual.len() > 30 {
        let _ = writeln!(out, "  ... {} more", unusual.len() - 30);
    }
    out
}

/// Summaries of `paths` (log files), in order.
pub fn run(paths: &[String]) -> Result<String, String> {
    let mut out = String::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
        let name = Path::new(p)
            .file_name()
            .map_or(p.as_str(), |n| n.to_str().unwrap_or(p));
        out.push_str(&summarize(name, &text));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_a_log_and_a_crashed_one() {
        let log = "H build=abc map=mp_x bots=5\n\
                   E t=0.500 loaded ms=4200 mem1_free=3000k\n\
                   S t=1 fps=60 fr=16.50/18.00 cpu=9.00 gpu=3.00 bot=2.00 tris=1000 mem1=3000k\n\
                   S t=2 fps=40 fr=25.00/40.00 cpu=20.00 gpu=3.00 bot=9.00 tris=1000 mem1=2900k\n\
                   X t=2.100 fr=40.00 cpu=30.00 bot=20.00\n\
                   E t=2.500 kill by=3 on=0\n\
                   E t=3.000 log_write bytes=900 us=12000 ok=true\n";
        let crashed = summarize("0001.log", log);
        assert!(crashed.contains("build=abc map=mp_x"));
        assert!(crashed.contains("summary: none"));
        assert!(crashed.contains("fr=20.75"), "{crashed}");
        assert!(crashed.lines().any(|l| l.contains("fr=25.00/40.00")));
        assert!(crashed.contains("events: kill=1 loaded=1 log_write=1"));
        assert!(crashed.contains("slowest log write: 12.0 ms"));
        assert!(crashed.contains("loaded ms=4200"));
        let done = summarize("0002.log", &format!("{log}P t=3 frames=120 p50=16.25\n"));
        assert!(done.contains("summary: t=3 frames=120 p50=16.25"));
    }
}

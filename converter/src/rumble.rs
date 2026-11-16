//! MW2's rumbles from the user's raw files (`rumble/<name>`: `RUMBLE\duration\..\lowRumbleFile\..\highRumbleFile\..`,
//! and the two `RUMBLEGRAPHFILE` graphs it names), read as IW4L's `audio::rumble::Rumble::prepare` reads them
//! (ported; acc57a5). The Wii plays them with `game_core::hud_mw2::Rumbler`.

use std::collections::HashMap;

/// Rumble `name` from the raw files (by raw file name). None when it or a graph is missing or malformed.
pub fn read(raw: &HashMap<String, String>, name: &str) -> Option<wpk::ui::Rumble> {
    let text = |n: &str| {
        raw.get(&format!("rumble/{n}"))
            .map(|t| t.trim_end_matches('\0'))
    };
    let mut fields = text(name)?.split('\\');
    if fields.next()? != "RUMBLE" {
        return None;
    }
    let (mut duration, mut low, mut high) = (None, None, None);
    let (mut range, mut fade_with_distance, mut broadcast) = (0.0f32, false, false);
    while let Some(key) = fields.next() {
        let value = fields.next()?.trim();
        match key {
            "duration" => duration = value.parse::<f32>().ok(),
            "lowRumbleFile" => low = Some(value),
            "highRumbleFile" => high = Some(value),
            "range" => range = value.parse().ok()?,
            "fadeWithDistance" => fade_with_distance = value.parse::<i32>().ok()? != 0,
            "broadcast" => broadcast = value.parse::<i32>().ok()? != 0,
            _ => {}
        }
    }
    let duration = duration.filter(|d| d.is_finite() && *d > 0.0)?;
    if !range.is_finite() || range < 0.0 || (broadcast && fade_with_distance && range <= 0.0) {
        return None;
    }
    let graph = |n: Option<&str>| -> Option<Vec<(f32, f32)>> {
        let mut t = text(n.filter(|n| !n.is_empty())?)?.split_whitespace();
        if t.next()? != "RUMBLEGRAPHFILE" {
            return None;
        }
        let count = t
            .next()?
            .parse::<usize>()
            .ok()
            .filter(|&c| c > 0 && c <= 16)?;
        let mut points: Vec<(f32, f32)> = Vec::with_capacity(count);
        for _ in 0..count {
            let mut num = || t.next()?.parse::<f32>().ok().filter(|v| v.is_finite());
            let (x, y) = (num()?, num()?);
            if points.last().is_some_and(|&(p, _)| p > x) {
                return None;
            }
            points.push((x, y));
        }
        Some(points)
    };
    Some(wpk::ui::Rumble {
        name: name.to_owned(),
        duration_ms: (duration * 1000.0).max(1.0) as u32,
        low: graph(low)?,
        high: graph(high)?,
        range,
        fade_with_distance,
        broadcast,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(files: &[(&str, &str)]) -> HashMap<String, String> {
        files
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn reads_a_rumble_and_its_graphs() {
        let r = raw(&[
            (
                "rumble/test_fire",
                "RUMBLE\\duration\\0.25\\range\\0\\fadeWithDistance\\0\\broadcast\\0\\lowRumbleFile\\t_l.rmb\\highRumbleFile\\t_h.rmb\0",
            ),
            (
                "rumble/t_l.rmb",
                "RUMBLEGRAPHFILE\n\n2\n0.0000 1.0000\n1.0000 0.0000\n",
            ),
            ("rumble/t_h.rmb", "RUMBLEGRAPHFILE\n\n1\n0 0.5\n"),
        ]);
        let got = read(&r, "test_fire").expect("reads");
        assert_eq!(got.duration_ms, 250);
        assert_eq!(got.low, vec![(0.0, 1.0), (1.0, 0.0)]);
        assert_eq!(got.high, vec![(0.0, 0.5)]);
        assert!(!got.broadcast);
    }

    #[test]
    fn refuses_what_iw4l_refuses() {
        let base = "RUMBLE\\duration\\0.25\\lowRumbleFile\\a\\highRumbleFile\\a";
        let g = ("rumble/a", "RUMBLEGRAPHFILE 2 0.5 1 0.2 0");
        // Unordered graph points.
        assert!(read(&raw(&[("rumble/x", base), g]), "x").is_none());
        // No duration.
        let ok_g = ("rumble/a", "RUMBLEGRAPHFILE 1 0 1");
        assert!(
            read(
                &raw(&[
                    ("rumble/x", "RUMBLE\\lowRumbleFile\\a\\highRumbleFile\\a"),
                    ok_g
                ]),
                "x"
            )
            .is_none()
        );
        // Broadcast faded with distance needs a range.
        let b = "RUMBLE\\duration\\1\\broadcast\\1\\fadeWithDistance\\1\\lowRumbleFile\\a\\highRumbleFile\\a";
        assert!(read(&raw(&[("rumble/x", b), ok_g]), "x").is_none());
        assert!(read(&raw(&[("rumble/x", base), ok_g]), "x").is_some());
    }
}

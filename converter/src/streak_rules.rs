//! Killstreak rules from the user's own MW2 killstreak scripts (rawfiles in the MP zones), read as text at convert
//! time (ADR 0007: read for values, never run): crate odds and capture times (`_airdrop.gsc`), the Predator's launch
//! (`_remotemissile.gsc`), the sentry (`_autosentry.gsc`), the air strikes (`_airstrike.gsc`, `_harrier.gsc`), the
//! helicopters (`_helicopter.gsc`) and the AC-130 (`_ac130.gsc`). Values the scripts don't yield keep MW2's stock
//! values (`wpk::streaks::StreakRules::default`); `found` says which were read. The scripts never leave `out/`.

use std::collections::HashMap;

use wpk::streaks::{NAMES, StreakRules, found};

use crate::rules::{assignment, calls, function_body, number, strip_comments};

pub const AIRDROP: &str = "maps/mp/killstreaks/_airdrop.gsc";
pub const REMOTE_MISSILE: &str = "maps/mp/killstreaks/_remotemissile.gsc";
pub const SENTRY: &str = "maps/mp/killstreaks/_autosentry.gsc";
pub const AIRSTRIKE: &str = "maps/mp/killstreaks/_airstrike.gsc";
pub const HARRIER: &str = "maps/mp/killstreaks/_harrier.gsc";
pub const HELICOPTER: &str = "maps/mp/killstreaks/_helicopter.gsc";
pub const AC130: &str = "maps/mp/killstreaks/_ac130.gsc";
pub const SCRIPTS: [&str; 7] = [
    AIRDROP,
    REMOTE_MISSILE,
    SENTRY,
    AIRSTRIKE,
    HARRIER,
    HELICOPTER,
    AC130,
];

/// Every `name = <number>;` in order.
fn assignments(text: &str, name: &str) -> Vec<f32> {
    text.split([';', '{', '}', '\n'])
        .filter_map(|l| {
            let (lhs, rhs) = l.split_once('=')?;
            if lhs.trim() != name || rhs.starts_with('=') {
                return None;
            }
            number(rhs)
        })
        .collect()
}

/// The first number argument of the first `func(...)` call in `text`.
fn first_arg(text: &str, func: &str) -> Option<f32> {
    calls(text, func, 0)
        .into_iter()
        .find_map(|(_, a)| a.first().and_then(|s| number(s)))
}

/// `losRadiusDamage( <pos + (x,y,z)>, radius, max, min, ... )`: radius, max and min.
fn radius_damage(body: &str) -> Option<(f32, f32, f32)> {
    let at = body.find("losRadiusDamage(")?;
    let call = &body[at..at + body[at..].find(';')?];
    let rest = &call[call.find("),")? + 2..];
    let n: Vec<f32> = rest.split(',').take(3).filter_map(number).collect();
    (n.len() == 3).then(|| (n[0], n[1], n[2]))
}

/// The crate weights of `drop_type` (`addCrateType( drop, crate, getDvarInt( "scr_..", <w> ), fn )` or a literal
/// weight), ammo then by [`NAMES`]. None unless ammo and every killstreak are listed.
fn crate_odds(text: &str, drop_type: &str) -> Option<(f32, [f32; 15])> {
    let mut ammo = None;
    let mut odds = [None; 15];
    for (_, a) in calls(text, "addCrateType", 0) {
        if a.len() < 3 || a[0] != drop_type {
            continue;
        }
        let w = if a[2].starts_with("getDvar") {
            a.get(3).and_then(|s| number(s))
        } else {
            number(&a[2])
        };
        let Some(w) = w else { continue };
        if a[1] == "ammo" {
            ammo = Some(w);
        } else if let Some(i) = wpk::streaks::index(&a[1]) {
            odds[i] = Some(w);
        }
    }
    // The airdrops themselves never come out of a crate.
    for name in ["airdrop", "airdrop_mega"] {
        let i = wpk::streaks::index(name).unwrap_or(0);
        odds[i] = odds[i].or(Some(0.0));
    }
    let all: Option<Vec<f32>> = odds.iter().copied().collect();
    Some((ammo?, all?.try_into().ok()?))
}

/// Read the killstreak rules from the scripts (by rawfile name).
pub fn read(scripts: &HashMap<String, String>) -> StreakRules {
    let mut r = StreakRules::default();
    let get = |name: &str| scripts.get(name).map(|t| strip_comments(t));
    if let Some(t) = get(AIRDROP) {
        if let Some((ammo, odds)) = crate_odds(&t, "airdrop") {
            r.crate_ammo = ammo;
            r.crate_odds = odds;
            r.found |= found::CRATE_ODDS;
        }
        if let Some((ammo, odds)) = crate_odds(&t, "airdrop_mega") {
            r.mega_ammo = ammo;
            r.mega_odds = odds;
            r.found |= found::MEGA_ODDS;
        }
        let owner = function_body(&t, "crateOwnerCaptureThink").and_then(|b| {
            calls(b, "useHoldThink", 0)
                .into_iter()
                .find_map(|(_, a)| a.get(1).and_then(|s| number(s)))
        });
        let other = function_body(&t, "useHoldThink").and_then(|b| assignment(b, "self.useTime"));
        if let (Some(o), Some(x)) = (owner, other) {
            r.crate_owner_use_s = o / 1000.0;
            r.crate_other_use_s = x / 1000.0;
            r.found |= found::CRATE_USE;
        }
        if let Some(v) = function_body(&t, "dropTimeOut")
            .and_then(|b| first_arg(b, "waitLongDurationWithHostMigrationPause"))
        {
            r.crate_timeout_s = v;
            r.found |= found::CRATE_TIMEOUT;
        }
        let dist = function_body(&t, "getPathStart").and_then(|b| assignment(b, "lbHalfDistance"));
        let height =
            function_body(&t, "getFlyHeightOffset").and_then(|b| assignment(b, "lbFlyHeight"));
        let fast = function_body(&t, "heliSetup").and_then(|b| first_arg(b, "Vehicle_SetSpeed"));
        let fly_by = function_body(&t, "doFlyBy");
        let slow = fly_by.and_then(|b| first_arg(b, "Vehicle_SetSpeed"));
        let wait = fly_by.and_then(|b| first_arg(b, "wait"));
        if let (Some(d), Some(h), Some(f), Some(s), Some(w)) = (dist, height, fast, slow, wait) {
            r.drop_distance = d;
            r.drop_height = h;
            r.drop_fast_mph = f;
            r.drop_slow_mph = s;
            r.drop_fast_s = w;
            r.found |= found::DROP_FLIGHT;
        }
        if let Some(b) = function_body(&t, "doC130FlyBy")
            && let (Some(d), Some(s)) = (
                assignment(b, "planeHalfDistance"),
                assignment(b, "planeFlySpeed"),
            )
        {
            r.c130_distance = d;
            r.c130_speed = s;
            r.found |= found::C130;
        }
    }
    // The Predator: the last values `init` sets are its `else` (every map but the two it names).
    if let Some(t) = get(REMOTE_MISSILE)
        && let Some(b) = function_body(&t, "init")
        && let (Some(up), Some(back), Some(ahead)) = (
            assignments(b, "level.missileRemoteLaunchVert").last(),
            assignments(b, "level.missileRemoteLaunchHorz").last(),
            assignments(b, "level.missileRemoteLaunchTargetDist").last(),
        )
    {
        r.predator_up = *up;
        r.predator_back = *back;
        r.predator_ahead = *ahead;
        r.found |= found::PREDATOR;
    }
    if let Some(t) = get(SENTRY) {
        if let (Some(timeout), Some(heat), Some(health)) = (
            assignment(&t, "SENTRY_TIME_OUT"),
            assignment(&t, "SENTRY_OVERHEAT_TIME"),
            assignment(&t, "self.health"),
        ) {
            r.sentry_timeout_s = timeout;
            r.sentry_overheat_s = heat;
            r.sentry_health = health;
            r.found |= found::SENTRY;
        }
        let s = |f: &str| {
            assignment(
                &t,
                &format!("level.sentrySettings[ \"sentry_minigun\" ].{f}"),
            )
        };
        if let (Some(a), Some(b), Some(c), Some(d)) =
            (s("burstMin"), s("burstMax"), s("pauseMin"), s("pauseMax"))
        {
            r.sentry_burst_min = a;
            r.sentry_burst_max = b;
            r.sentry_pause_min = c;
            r.sentry_pause_max = d;
            r.found |= found::SENTRY_BURST;
        }
    }
    if let Some(t) = get(AIRSTRIKE) {
        // `callStrike` sets the stealth bomber's run first, then the others' (each with the old height system's
        // fallback height and bomb distance).
        if let Some(b) = function_body(&t, "callStrike") {
            let half = assignments(b, "planeHalfDistance");
            let speed = assignments(b, "planeFlySpeed");
            let height = assignments(b, "planeFlyHeight");
            let explode = assignments(b, "planeBombExplodeDistance");
            // The last two of each (`planeBombExplodeDistance = 0;` comes first).
            let two = |v: &[f32]| (v.len() >= 2).then(|| (v[v.len() - 2], v[v.len() - 1]));
            if let (Some(h), Some(s), Some(y), Some((_, e))) =
                (two(&half), two(&speed), two(&height), two(&explode))
            {
                (r.stealth_distance, r.strike_distance) = h;
                (r.stealth_speed, r.strike_speed) = s;
                (r.stealth_height, r.strike_height) = y;
                r.strike_explode_distance = e;
                r.found |= found::AIRSTRIKE | found::STEALTH;
            }
        }
        if let Some(b) = function_body(&t, "callStrike_bombEffect")
            && let (Some(n), Some(lo), Some(hi), Some((rad, max, min))) = (
                assignment(b, "repeat"),
                assignment(b, "minAngles"),
                assignment(b, "maxAngles"),
                radius_damage(b),
            )
        {
            r.strike_traces = n;
            r.strike_min_angle = lo;
            r.strike_max_angle = hi;
            r.strike_radius = rad;
            r.strike_max_damage = max;
            r.strike_min_damage = min;
            r.found |= found::AIRSTRIKE_BOMBS;
        }
        if let Some(b) = function_body(&t, "callStrike_bomb")
            && let (Some(acc), Some((rad, max, min))) =
                (assignment(b, "accuracyRadius"), radius_damage(b))
        {
            r.stealth_accuracy = acc;
            r.stealth_radius = rad;
            r.stealth_max_damage = max;
            r.stealth_min_damage = min;
            r.found |= found::STEALTH_BOMBS;
        }
    }
    if let Some(t) = get(HARRIER) {
        let stay = function_body(&t, "harrierTimer")
            .and_then(|b| first_arg(b, "waitLongDurationWithHostMigrationPause"));
        let range = t
            .find("distance( potentialTarget.origin, self.origin ) >")
            .and_then(|i| {
                let rest = &t[i..];
                number(rest[rest.find('>')? + 1..].split(')').next()?)
            });
        if let (Some(h), Some(s), Some(n), Some(rg)) = (
            assignment(&t, "harrier.health"),
            stay,
            assignment(&t, "numShots"),
            range,
        ) {
            r.harrier_health = h;
            r.harrier_stay_s = s;
            r.harrier_burst = n;
            r.harrier_range = rg;
            r.found |= found::HARRIER;
        }
    }
    if let Some(t) = get(HELICOPTER) {
        if let (Some(h), Some(rg), Some(clip), Some(rl), Some(tg)) = (
            assignment(&t, "level.heli_maxhealth"),
            assignment(&t, "level.heli_visual_range"),
            assignment(&t, "level.heli_turretClipSize"),
            assignment(&t, "level.heli_turretReloadTime"),
            assignment(&t, "level.heli_targeting_delay"),
        ) {
            r.heli_health = h;
            r.heli_range = rg;
            r.heli_clip = clip;
            r.heli_reload_s = rl;
            r.heli_targeting_s = tg;
            r.found |= found::HELI;
        }
        // The flight switch: the Chopper Gunner's ("minigun") stay first, the default's last.
        let stays: Vec<f32> = calls(&t, "heli_leave_on_timeout", 0)
            .into_iter()
            .filter_map(|(_, a)| a.first().and_then(|s| number(s)))
            .collect();
        if stays.len() >= 2 {
            r.gunner_stay_s = stays[0];
            r.heli_stay_s = stays[stays.len() - 1];
            r.found |= found::HELI_STAY;
        }
    }
    if let Some(t) = get(AC130) {
        let reload =
            |w: &str| assignment(&t, &format!("level.weaponReloadTime[ \"ac130_{w}_mp\" ]"));
        if let (Some(stay), Some(turn), Some(a), Some(b), Some(c)) = (
            assignment(&t, "level.ac130_use_duration"),
            assignment(&t, "level.ac130_Speed[ \"rotate\" ]"),
            reload("105mm"),
            reload("40mm"),
            reload("25mm"),
        ) {
            r.ac130_stay_s = stay;
            r.ac130_turn_s = turn;
            r.ac130_reload_s = [a, b, c];
            r.found |= found::AC130;
        }
    }
    r
}

/// The names of the values `r` did not read from the scripts.
pub fn missing(r: &StreakRules) -> Vec<&'static str> {
    [
        (found::CRATE_ODDS, "care package odds"),
        (found::MEGA_ODDS, "emergency airdrop odds"),
        (found::CRATE_USE, "crate capture times"),
        (found::CRATE_TIMEOUT, "crate time-out"),
        (found::DROP_FLIGHT, "drop helicopter run"),
        (found::C130, "C-130 run"),
        (found::PREDATOR, "Predator launch"),
        (found::SENTRY, "sentry time and health"),
        (found::SENTRY_BURST, "sentry bursts"),
        (found::AIRSTRIKE, "air strike run"),
        (found::AIRSTRIKE_BOMBS, "air strike bombs"),
        (found::STEALTH, "stealth bomber run"),
        (found::STEALTH_BOMBS, "stealth bombs"),
        (found::HARRIER, "Harrier"),
        (found::HELI, "helicopter"),
        (found::HELI_STAY, "helicopter stay"),
        (found::AC130, "AC-130"),
    ]
    .into_iter()
    .filter(|(bit, _)| r.found & bit == 0)
    .map(|(_, n)| n)
    .collect()
}

/// [`NAMES`] count, for the report.
pub const STREAKS: usize = NAMES.len();

#[cfg(test)]
mod tests {
    use super::*;

    /// Made-up script text in the shapes the readers expect (not MW2's scripts; their values differ on purpose).
    #[test]
    fn reads_values_in_the_scripts_shapes() {
        let mut s = HashMap::new();
        let mut airdrop = String::from("init()\n{\n");
        airdrop.push_str(
            "\taddCrateType( \"airdrop\", \"ammo\", getDvarInt( \"scr_airdrop_ammo\", 20 ), ::ammoCrateThink );\n",
        );
        for (i, n) in NAMES.iter().enumerate() {
            if !n.starts_with("airdrop") {
                airdrop.push_str(&format!(
                    "\taddCrateType( \"airdrop\", \"{n}\", getDvarInt( \"scr_airdrop_{n}\", {i} ), ::k );\n"
                ));
            }
        }
        airdrop.push_str("\taddCrateType( \"nuke_drop\", \"nuke\", 100, ::nukeCrateThink );\n}\n");
        airdrop.push_str(
            "crateOwnerCaptureThink()\n{\n\tif ( !useHoldThink( player, 400 ) )\n\t\tcontinue;\n}\n\
             useHoldThink( player, useTime ) \n{\n\tif ( isDefined( useTime ) )\n\t\tself.useTime = useTime;\n\telse\n\t\tself.useTime = 2500;\n}\n\
             dropTimeOut( dropCrate, owner )\n{\n\tmaps\\mp\\gametypes\\_hostmigration::waitLongDurationWithHostMigrationPause( 80.0 );\n}\n\
             getPathStart( coord, yaw )\n{\n\tlbHalfDistance = 14000;\n}\n\
             getFlyHeightOffset( dropSite )\n{\n\tlbFlyHeight = 800;\n}\n\
             heliSetup( owner, pathStart, pathGoal )\n{\n\tchopper Vehicle_SetSpeed( 240, 175 );\n}\n\
             doFlyBy( owner, dropSite, dropYaw, dropType, heightAdjustment )\n{\n\twait ( 3 );\n\tchopper Vehicle_SetSpeed( 70, 40 );\n}\n\
             doC130FlyBy( owner, dropSite, dropYaw, dropType )\n{\n\tplaneHalfDistance = 20000;\n\tplaneFlySpeed = 1800;\n}\n",
        );
        s.insert(AIRDROP.to_owned(), airdrop);
        s.insert(
            REMOTE_MISSILE.to_owned(),
            "init()\n{\n\tif ( a )\n\t{\n\t\tlevel.missileRemoteLaunchVert = 1;\n\t\tlevel.missileRemoteLaunchHorz = 2;\n\t\tlevel.missileRemoteLaunchTargetDist = 3;\n\t}\n\telse\n\t{\n\t\tlevel.missileRemoteLaunchVert = 12000;\n\t\tlevel.missileRemoteLaunchHorz = 6000;\n\t\tlevel.missileRemoteLaunchTargetDist = 1400;\n\t}\n}\n"
                .to_owned(),
        );
        s.insert(
            SENTRY.to_owned(),
            "SENTRY_TIME_OUT = 80.0;\nSENTRY_OVERHEAT_TIME = 7.0;\ninit()\n{\n\tlevel.sentrySettings[ \"sentry_minigun\" ].burstMin = 10;\n\tlevel.sentrySettings[ \"sentry_minigun\" ].burstMax = 100;\n\tlevel.sentrySettings[ \"sentry_minigun\" ].pauseMin = 0.1;\n\tlevel.sentrySettings[ \"sentry_minigun\" ].pauseMax = 0.3;\n}\nsentry_init()\n{\n\tself.health = 900;\n}\n"
                .to_owned(),
        );
        s.insert(
            AIRSTRIKE.to_owned(),
            "callStrike( lifeId, owner, coord, yaw )\n{\n\tif ( stealth )\n\t{\n\t\tplaneHalfDistance = 11000;\n\t\tplaneFlySpeed = 1900;\n\t\tif ( x )\n\t\t{\n\t\t\tplaneFlyHeight = 900;\n\t\t\tplaneBombExplodeDistance = 1400;\n\t\t}\n\t\telse\n\t\t{\n\t\t\tplaneFlyHeight = heightEnt.origin[2];\n\t\t}\n\t}\n\telse\n\t{\n\t\tplaneHalfDistance = 23000;\n\t\tplaneFlySpeed = 6500;\n\t\tplaneFlyHeight = 800;\n\t\tplaneBombExplodeDistance = 1300;\n\t}\n}\n\
             callStrike_bombEffect( a )\n{\n\trepeat = 10;\n\tminAngles = 4;\n\tmaxAngles = 50;\n\t\tthread losRadiusDamage( traceHit + (0,0,16), 500, 190, 25, owner, bomb, \"artillery_mp\" );\n}\n\
             callStrike_bomb( coord, owner, offset, showFx )\n{\n\taccuracyRadius = 500;\n\tlosRadiusDamage( bombPoint + (0,0,16), 800, 290, 45, owner, self, \"stealth_bomb_mp\" );\n}\n"
                .to_owned(),
        );
        s.insert(
            HARRIER.to_owned(),
            "beginHarrier()\n{\n\tharrier.health = 2900;\n}\nharrierTimer()\n{\n\tmaps\\mp\\gametypes\\_hostmigration::waitLongDurationWithHostMigrationPause( 40 );\n}\nfire()\n{\n\tnumShots = 20;\n}\ncheck()\n{\n\tif ( distance( potentialTarget.origin, self.origin ) > 8000 )\n\t\treturn false;\n}\n"
                .to_owned(),
        );
        s.insert(
            HELICOPTER.to_owned(),
            "init()\n{\n\tlevel.heli_maxhealth \t= 1400;\t// x\n\tlevel.heli_targeting_delay \t= 0.4;\n\tlevel.heli_turretReloadTime = 1.4;\n\tlevel.heli_turretClipSize \t= 30;\n\tlevel.heli_visual_range \t= 3000;\n}\nfly()\n{\n\tchopper thread heli_leave_on_timeout( 35.0 );\n\tchopper thread heli_leave_on_timeout( 55.0 );\n\tchopper thread heli_leave_on_timeout( 55.0 );\n}\n"
                .to_owned(),
        );
        s.insert(
            AC130.to_owned(),
            "init()\n{\n\tlevel.ac130_use_duration = 35;\n\tlevel.ac130_Speed[ \"rotate\" ] = 60;\n\tlevel.weaponReloadTime[ \"ac130_25mm_mp\" ] = 1.0;\n\tlevel.weaponReloadTime[ \"ac130_40mm_mp\" ] = 2.0;\n\tlevel.weaponReloadTime[ \"ac130_105mm_mp\" ] = 4.0;\n}\n"
                .to_owned(),
        );
        let r = read(&s);
        assert_eq!(missing(&r), Vec::<&str>::new());
        assert_eq!(r.crate_ammo, 20.0);
        assert_eq!(r.crate_odds[wpk::streaks::index("sentry").unwrap()], 3.0);
        assert_eq!(r.crate_odds[wpk::streaks::index("airdrop").unwrap()], 0.0);
        assert_eq!((r.crate_owner_use_s, r.crate_other_use_s), (0.4, 2.5));
        assert_eq!(r.crate_timeout_s, 80.0);
        assert_eq!(
            (
                r.drop_distance,
                r.drop_height,
                r.drop_fast_mph,
                r.drop_slow_mph,
                r.drop_fast_s
            ),
            (14000.0, 800.0, 240.0, 70.0, 3.0)
        );
        assert_eq!((r.c130_distance, r.c130_speed), (20000.0, 1800.0));
        assert_eq!(
            (r.predator_up, r.predator_back, r.predator_ahead),
            (12000.0, 6000.0, 1400.0)
        );
        assert_eq!(
            (r.sentry_timeout_s, r.sentry_health, r.sentry_burst_max),
            (80.0, 900.0, 100.0)
        );
        assert_eq!(
            (r.stealth_distance, r.strike_distance, r.strike_height),
            (11000.0, 23000.0, 800.0)
        );
        assert_eq!(
            (r.strike_traces, r.strike_radius, r.strike_min_damage),
            (10.0, 500.0, 25.0)
        );
        assert_eq!(r.strike_explode_distance, 1300.0);
        assert_eq!((r.stealth_accuracy, r.stealth_max_damage), (500.0, 290.0));
        assert_eq!(
            (
                r.harrier_health,
                r.harrier_stay_s,
                r.harrier_burst,
                r.harrier_range
            ),
            (2900.0, 40.0, 20.0, 8000.0)
        );
        assert_eq!(
            (r.heli_health, r.heli_clip, r.gunner_stay_s, r.heli_stay_s),
            (1400.0, 30.0, 35.0, 55.0)
        );
        assert_eq!(
            (r.ac130_stay_s, r.ac130_turn_s, r.ac130_reload_s),
            (35.0, 60.0, [4.0, 2.0, 1.0])
        );
    }
}

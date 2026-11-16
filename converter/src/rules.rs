//! Match rules from the user's own MW2 game-type scripts (rawfiles in the MP zones), read as text at convert time:
//! health and its regeneration (`_tweakables.gsc`, `_healthoverlay.gsc`) and free-for-all's limits and points
//! (`dm.gsc`). Values the scripts don't yield keep MW2's stock values (`wpk::rules::Rules::default`); `found` says
//! which were read. The scripts themselves never leave `out/` (they are game data).

use std::collections::HashMap;

use wpk::rules::{Rules, found};

pub const TWEAKABLES: &str = "maps/mp/gametypes/_tweakables.gsc";
pub const HEALTH_OVERLAY: &str = "maps/mp/gametypes/_healthoverlay.gsc";
pub const FFA: &str = "maps/mp/gametypes/dm.gsc";
/// The scripts the rules come from (captured from the zones' rawfiles).
pub const WEAPONS: &str = "maps/mp/gametypes/_weapons.gsc";
pub const PERK_FUNCTIONS: &str = "maps/mp/perks/_perkfunctions.gsc";
pub const UAV: &str = "maps/mp/killstreaks/_uav.gsc";
pub const TDM: &str = "maps/mp/gametypes/war.gsc";
pub const RANK: &str = "maps/mp/gametypes/_rank.gsc";
/// The scripts the rules come from (captured from the zones' rawfiles).
pub const FLASH: &str = "maps/mp/_flashgrenades.gsc";
pub const SHOCK_CONCUSSION: &str = "shock/concussion_grenade_mp.shock";
pub const SHOCK_FLASH: &str = "shock/flashbang_mp.shock";
pub const DOM: &str = "maps/mp/gametypes/dom.gsc";
pub const EMP: &str = "maps/mp/killstreaks/_emp.gsc";
pub const NUKE: &str = "maps/mp/killstreaks/_nuke.gsc";
pub const PERKS: &str = "maps/mp/perks/_perks.gsc";
pub const SCRIPTS: [&str; 15] = [
    PERKS,
    DOM,
    EMP,
    NUKE,
    TWEAKABLES,
    HEALTH_OVERLAY,
    FFA,
    WEAPONS,
    PERK_FUNCTIONS,
    UAV,
    TDM,
    RANK,
    FLASH,
    SHOCK_CONCUSSION,
    SHOCK_FLASH,
];

/// The scripts the perk, deathstreak, last stand and killcam rules come from (`wpk::life_rules`), besides some of
/// [`SCRIPTS`].
pub const DAMAGE: &str = "maps/mp/gametypes/_damage.gsc";
pub const CLASS: &str = "maps/mp/gametypes/_class.gsc";
pub const LIFE_SCRIPTS: [&str; 2] = [DAMAGE, CLASS];

/// Read the perk, deathstreak, last stand and killcam rules (`LIFE`) from the scripts (by rawfile name).
pub fn read_life(scripts: &HashMap<String, String>) -> wpk::life_rules::LifeRules {
    use wpk::life_rules::{LifeRules, found as lf};
    let mut r = LifeRules::default();
    let get = |name: &str| scripts.get(name).map(|t| strip_comments(t));
    let secs = |s: &str| number(s).map(|v| (v * 1000.0).round() as u32);
    if let Some(t) = get(DAMAGE) {
        // `Callback_PlayerLastStand`: `lastStandTimer( 20, true )` (Final Stand) and, last, the normal last stand's
        // `lastStandTimer( 10, false )`.
        if let Some(body) = function_body(&t, "Callback_PlayerLastStand") {
            let timers = calls(body, "lastStandTimer", 0);
            if let Some(ms) = timers
                .iter()
                .find(|(_, a)| a.get(1).is_some_and(|b| b == "true"))
                .and_then(|(_, a)| secs(&a[0]))
            {
                r.final_stand_ms = ms;
                r.found |= lf::FINAL_STAND;
            }
            if let Some(ms) = timers
                .iter()
                .rev()
                .find(|(_, a)| a.get(1).is_some_and(|b| b == "false"))
                .and_then(|(_, a)| secs(&a[0]))
            {
                r.last_stand_ms = ms;
                r.found |= lf::LAST_STAND;
            }
        }
        // `lastStandAllowSuicide`: `if ( gettime() - pressStartTime > 700 )`.
        if let Some(body) = function_body(&t, "lastStandAllowSuicide")
            && let Some(i) = body.find("pressStartTime >")
            && let Some(v) = body[i + "pressStartTime >".len()..]
                .split(')')
                .next()
                .and_then(number)
        {
            r.give_up_ms = v as u32;
            r.found |= lf::GIVE_UP;
        }
    }
    // Painkiller (`setCombatHigh`): its waits add up to its time.
    if let Some(t) = get(PERK_FUNCTIONS) {
        if let Some(body) = function_body(&t, "setCombatHigh") {
            let waits: Vec<f32> = calls(body, "wait", 0)
                .into_iter()
                .filter_map(|(_, a)| a.first().and_then(|s| number(s)))
                .collect();
            if !waits.is_empty() {
                r.painkiller_ms = (waits.iter().sum::<f32>() * 1000.0).round() as u32;
                r.found |= lf::PAINKILLER;
            }
        }
        // `giveOneManArmyClass`: `changeDuration = 3.0;` with `specialty_omaquickchange`, else `6.0`.
        if let Some(body) = function_body(&t, "giveOneManArmyClass") {
            let set: Vec<f32> = body
                .match_indices("changeDuration =")
                .filter_map(|(i, m)| number(body[i + m.len()..].split(';').next()?))
                .collect();
            if let [quick, full, ..] = set[..] {
                r.oma_quick_ms = (quick * 1000.0).round() as u32;
                r.oma_ms = (full * 1000.0).round() as u32;
                r.found |= lf::OMA;
            }
        }
    }
    // Painkiller's divisor (`cac_modified_damage`: `damage = damage/3;` in the `specialty_combathigh` block).
    if let Some(t) = get(PERKS)
        && let Some(at) = t.find("\"specialty_combathigh\"")
        && let Some(i) = t[at..].find("damage = damage/")
        && let Some(v) = t[at + i + "damage = damage/".len()..]
            .split(';')
            .next()
            .and_then(number)
        && v > 0.0
    {
        r.painkiller_divisor = v;
        r.found |= lf::PAINKILLER_DIVISOR;
    }
    // Scrambler Pro: `if ( player _hasPerk( "specialty_delaymine" ) ) wait 3.0;` in the claymore's trip.
    if let Some(t) = get(WEAPONS)
        && let Some(at) = t.find("\"specialty_delaymine\"")
    {
        let rest = &t[at..];
        let wait = rest.find("wait").and_then(|i| {
            let s = rest[i + 4..].trim_start().trim_start_matches('(');
            number(s.split([';', ')']).next()?)
        });
        if let Some(v) = wait {
            r.delay_mine_ms = (v * 1000.0).round() as u32;
            r.found |= lf::DELAY_MINE;
        }
    }
    // Hardline (`setKillstreaks`): `modifier = -1;`.
    if let Some(t) = get(CLASS)
        && let Some(body) = function_body(&t, "setKillstreaks")
        && let Some(v) = assignment(body, "modifier").filter(|v| *v != 0.0)
    {
        r.hardline_modifier = v as i32;
        r.found |= lf::HARDLINE;
    }
    // Killcams: `registerTweakable( "game", "allowkillcam", "scr_game_allowkillcam", 1 )`.
    if let Some(t) = get(TWEAKABLES)
        && let Some(v) = calls(&t, "registerTweakable", 0)
            .into_iter()
            .find(|(_, a)| a.len() >= 4 && a[1] == "allowkillcam")
            .and_then(|(_, a)| number(&a[3]))
    {
        r.killcam = v != 0.0;
        r.found |= lf::KILLCAM;
    }
    r
}

/// The names of the values `r` did not read from the scripts.
pub fn missing_life(r: &wpk::life_rules::LifeRules) -> Vec<&'static str> {
    use wpk::life_rules::found as lf;
    [
        (lf::LAST_STAND, "last stand time"),
        (lf::FINAL_STAND, "final stand time"),
        (lf::GIVE_UP, "last stand give-up hold"),
        (lf::PAINKILLER, "painkiller time"),
        (lf::PAINKILLER_DIVISOR, "painkiller divisor"),
        (lf::OMA, "one man army change"),
        (lf::DELAY_MINE, "scrambler pro claymore delay"),
        (lf::HARDLINE, "hardline modifier"),
        (lf::KILLCAM, "killcam allowed"),
    ]
    .into_iter()
    .filter(|(bit, _)| r.found & bit == 0)
    .map(|(_, name)| name)
    .collect()
}

/// The text without `/* */` and `//` comments (scanned in order, so a `//*` line comment isn't a block).
pub(crate) fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let line = rest.find("//");
        let block = rest.find("/*");
        match (line, block) {
            (Some(l), b) if b.is_none_or(|b| l < b) => {
                out.push_str(&rest[..l]);
                rest = rest[l..].find('\n').map_or("", |e| &rest[l + e..]);
            }
            (_, Some(b)) => {
                out.push_str(&rest[..b]);
                rest = rest[b + 2..]
                    .find("*/")
                    .map_or("", |e| &rest[b + 2 + e + 2..]);
            }
            _ => {
                out.push_str(rest);
                return out;
            }
        }
    }
}
/// The body of function `name`'s definition (a line starting with `name(`), up to its closing brace at the start
/// of a line.
pub(crate) fn function_body<'t>(text: &'t str, name: &str) -> Option<&'t str> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with(name)
            && line.trim_start()[name.len()..]
                .trim_start()
                .starts_with('(')
            && !line.contains(';')
        {
            let body = &text[at + line.len()..];
            let end = body.find("\n}").map_or(body.len(), |e| e + 2);
            return Some(&body[..end]);
        }
        at += line.len();
    }
    None
}

/// The scale in the first `setMoveSpeedScale( <scale> * ... )` after offset `from` of `text`.
fn speed_scale_after(text: &str, from: usize) -> Option<f32> {
    let (_, a) = calls(text, "setMoveSpeedScale", from).into_iter().next()?;
    number(a.first()?.split('*').next()?)
}

/// Every call of `func` (any module prefix) with its arguments, unquoted and trimmed, from offset `from` on.
pub(crate) fn calls(text: &str, func: &str, from: usize) -> Vec<(usize, Vec<String>)> {
    let mut out = Vec::new();
    let mut at = from;
    while let Some(i) = text[at..].find(func).map(|i| i + at) {
        at = i + func.len();
        let before_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let rest = text[at..].trim_start();
        if !before_ok || !rest.starts_with('(') {
            continue;
        }
        let open = text.len() - rest.len();
        let Some(close) = text[open..].find(')') else {
            break;
        };
        let args = text[open + 1..open + close]
            .split(',')
            .map(|a| a.trim().trim_matches('"').to_owned())
            .collect();
        out.push((i, args));
    }
    out
}

pub(crate) fn number(s: &str) -> Option<f32> {
    s.trim().trim_end_matches(';').trim().parse().ok()
}

/// The value of the first `name = <number>;`.
pub(crate) fn assignment(text: &str, name: &str) -> Option<f32> {
    text.split([';', '{', '}', '\n']).find_map(|l| {
        let (lhs, rhs) = l.split_once('=')?;
        if lhs.trim() != name || rhs.starts_with('=') {
            return None;
        }
        number(rhs)
    })
}

/// The names of the values `r` did not read from the scripts.
pub fn missing(r: &Rules) -> Vec<&'static str> {
    [
        (found::MAX_HEALTH, "max health"),
        (found::REGEN_DELAY, "regen delay"),
        (found::REGEN_STEP, "regen step"),
        (found::REGEN_INTERVAL, "regen interval"),
        (found::HURT_CUTOFF, "hurt cutoff"),
        (found::VERY_HURT_WAIT, "very hurt wait"),
        (found::TIME_LIMIT, "time limit"),
        (found::SCORE_LIMIT, "score limit"),
        (found::KILL_SCORE, "kill points"),
        (found::HEADSHOT_SCORE, "headshot points"),
        (found::ASSIST_SCORE, "assist points"),
        (found::CLASS_SPEED, "move speed by weapon class"),
        (found::LIGHTWEIGHT_SPEED, "Lightweight speed"),
        (found::UAV_TIME, "UAV time"),
        (found::TDM_TIME_LIMIT, "TDM time limit"),
        (found::TDM_SCORE_LIMIT, "TDM score limit"),
    ]
    .into_iter()
    .filter(|(bit, _)| r.found & bit == 0)
    .map(|(_, name)| name)
    .chain(
        [
            (found::TEAM_KILL_SCORE, "team kill points"),
            (found::TEAM_HEADSHOT_SCORE, "team headshot points"),
        ]
        .into_iter()
        .filter(|(bit, _)| r.found_team & bit == 0)
        .map(|(_, name)| name),
    )
    .chain(
        [
            (found::FLASH, "flashbang duration"),
            (found::STUN, "stun duration"),
            (found::SHOCK_FLASH, "flashbang shock"),
            (found::SHOCK_STUN, "stun shock"),
            (found::CLAYMORE, "claymore trip"),
            (found::EXPLOSIVES, "explosives per player"),
            (found::DOM, "Domination"),
            (found::EMP, "EMP duration"),
            (found::NUKE, "nuke timing"),
            (found::PERK_DAMAGE, "perk damage"),
        ]
        .into_iter()
        .filter(|(bit, _)| r.found_fx & bit == 0)
        .map(|(_, name)| name),
    )
    .collect()
}

/// Read the rules from the scripts (by rawfile name). Returns the rules and how many values were read.
pub fn read(scripts: &HashMap<String, String>) -> Rules {
    let mut r = Rules::default();
    let get = |name: &str| scripts.get(name).map(|t| strip_comments(t));
    if let Some(t) = get(TWEAKABLES) {
        for (_, a) in calls(&t, "registerTweakable", 0) {
            if a.len() < 4 || a[0] != "player" {
                continue;
            }
            match (a[1].as_str(), number(&a[3])) {
                ("maxhealth", Some(v)) if v > 0.0 => {
                    r.max_health = v as i32;
                    r.found |= found::MAX_HEALTH;
                }
                ("healthregentime", Some(v)) if v >= 0.0 => {
                    r.regen_delay_ms = (v * 1000.0) as u32;
                    r.found |= found::REGEN_DELAY;
                }
                _ => {}
            }
        }
    }
    if let Some(t) = get(HEALTH_OVERLAY) {
        if let Some(v) = assignment(&t, "level.healthOverlayCutoff") {
            r.hurt_cutoff = v;
            r.found |= found::HURT_CUTOFF;
        }
        // The regeneration loop: its step, then the loop's first wait, then the very-hurt wait after the hit.
        if let Some(at) = t.find("regenRate") {
            if let Some(v) = assignment(&t[at..], "regenRate") {
                r.regen_step = v;
                r.found |= found::REGEN_STEP;
            }
            if let Some(v) = calls(&t, "wait", at)
                .first()
                .and_then(|(_, a)| a.first().and_then(|s| number(s)))
            {
                r.regen_interval_ms = (v * 1000.0).round() as u32;
                r.found |= found::REGEN_INTERVAL;
            }
            // `hurtTime + <ms>` (not `hurtTime += ...`).
            let wait = t[at..].match_indices("hurtTime +").find_map(|(i, m)| {
                let digits: String = t[at + i + m.len()..]
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse().ok()
            });
            if let Some(v) = wait {
                r.very_hurt_wait_ms = v;
                r.found |= found::VERY_HURT_WAIT;
            }
        }
    }
    if let Some(t) = get(WEAPONS)
        && let Some(body) = function_body(&t, "updateMoveSpeedScale")
    {
        let mut speeds = r.class_speed;
        let mut all = true;
        for (k, class) in wpk::rules::SPEED_CLASSES.iter().enumerate() {
            let key = if *class == "riotshield" {
                "\"riotshield_mp\"".to_owned()
            } else {
                format!("case \"{class}\"")
            };
            match body.find(&key).and_then(|i| speed_scale_after(body, i)) {
                Some(v) => speeds[k] = v,
                None => all = false,
            }
        }
        if all {
            r.class_speed = speeds;
            r.found |= found::CLASS_SPEED;
        }
    }
    if let Some(t) = get(PERK_FUNCTIONS)
        && let Some(v) =
            function_body(&t, "setLightWeight").and_then(|b| assignment(b, "self.moveSpeedScaler"))
    {
        r.lightweight_speed = v;
        r.found |= found::LIGHTWEIGHT_SPEED;
    }
    if let Some(v) = get(UAV).and_then(|t| assignment(&t, "level.radarViewTime")) {
        r.uav_time_ms = (v * 1000.0) as u32;
        r.found |= found::UAV_TIME;
    }
    // Flashbang duration (`monitorFlash`): `duration = amount_distance * amount_angle * <seconds>;`, the look amount
    // clamps `amount_angle < a` / `amount_angle > b`, and `duration < <min>`.
    if let Some(t) = get(FLASH)
        && let Some(body) = function_body(&t, "monitorFlash")
    {
        let after = |key: &str| -> Option<f32> {
            let i = body.find(key)? + key.len();
            number(body[i..].split([';', ')', '\n']).next()?)
        };
        if let (Some(sec), Some(lo), Some(hi), Some(min)) = (
            after("amount_distance * amount_angle *"),
            after("amount_angle <"),
            after("amount_angle >"),
            after("duration <"),
        ) {
            r.flash_seconds = sec;
            r.flash_angle_min = lo;
            r.flash_angle_full = hi;
            r.flash_min_seconds = min;
            r.found_fx |= found::FLASH;
        }
    }
    // Stun (`onWeaponDamage`, the concussion case): `radius = <r>;` and `time = <base> + ( <extra> * scale );`.
    if let Some(t) = get(WEAPONS)
        && let Some(body) = function_body(&t, "onWeaponDamage")
        && let Some(at) = body.find("\"concussion_grenade_mp\"")
    {
        let case = &body[at..];
        let radius = assignment(case, "radius");
        let time = case.find("time =").and_then(|i| {
            let expr = case[i + "time =".len()..].split(';').next()?;
            let (base, rest) = expr.split_once('+')?;
            let extra = rest.trim().trim_start_matches('(').split('*').next()?;
            Some((number(base)?, number(extra)?))
        });
        if let (Some(radius), Some((base, extra))) = (radius, time) {
            r.stun_radius = radius;
            r.stun_base = base;
            r.stun_extra = extra;
            r.found_fx |= found::STUN;
        }
    }
    // Claymores (`init`): `claymoreDetectionConeAngle = 70;`, `level.claymoreDetectionMinDist = 20;`,
    // `level.claymoreDetectionGracePeriod = .75;`, `level.claymoreDetonateRadius = 192;`.
    if let Some(t) = get(WEAPONS) {
        if let (Some(cone), Some(min), Some(grace), Some(radius)) = (
            assignment(&t, "claymoreDetectionConeAngle"),
            assignment(&t, "level.claymoreDetectionMinDist"),
            assignment(&t, "level.claymoreDetectionGracePeriod"),
            assignment(&t, "level.claymoreDetonateRadius"),
        ) {
            r.claymore_cone_deg = cone;
            r.claymore_min_dist = min;
            r.claymore_grace_ms = (grace * 1000.0).round() as u32;
            r.claymore_radius = radius;
            r.found_fx |= found::CLAYMORE;
        }
        // `level.maxPerPlayerExplosives = max( getIntProperty( "scr_maxPerPlayerExplosives", 2 ), 1 );`: the
        // property's default (no dvar set), at least the floor.
        if let Some(at) = t.find("level.maxPerPlayerExplosives")
            && let Some((_, a)) = calls(&t, "getIntProperty", at).into_iter().next()
            && a.first().is_some_and(|n| n == "scr_maxPerPlayerExplosives")
            && let Some(v) = a.get(1).and_then(|s| number(s))
        {
            r.max_explosives = (v as u32).max(1);
            r.found_fx |= found::EXPLOSIVES;
        }
    }
    // Domination: the limits (`registerTimeLimitDvar` minutes, `registerScoreLimitDvar`), a capture's points
    // (`registerScoreInfo( "capture", n )`), the flags' use time (`setUseTime( s )` in `domFlags`) and the score
    // tick (the `wait ( s )` in `updateDomScores`).
    if let Some(t) = get(DOM) {
        let first_arg = |f: &str, k: usize| {
            calls(&t, f, 0)
                .first()
                .and_then(|(_, a)| a.get(k).and_then(|s| number(s)))
        };
        let capture = calls(&t, "registerScoreInfo", 0)
            .into_iter()
            .find(|(_, a)| a.first().is_some_and(|n| n == "capture"))
            .and_then(|(_, a)| a.get(1).and_then(|s| number(s)));
        let use_s = function_body(&t, "domFlags").and_then(|b| {
            calls(b, "setUseTime", 0)
                .first()
                .and_then(|(_, a)| a.first().and_then(|s| number(s)))
        });
        let tick_s = function_body(&t, "updateDomScores").and_then(|b| {
            calls(b, "wait", 0)
                .first()
                .and_then(|(_, a)| a.first().and_then(|s| number(s)))
        });
        if let (Some(tl), Some(sl), Some(cap), Some(u), Some(tick)) = (
            first_arg("registerTimeLimitDvar", 1),
            first_arg("registerScoreLimitDvar", 1),
            capture,
            use_s,
            tick_s,
        ) {
            r.dom_time_limit_ms = (tl * 60_000.0) as u32;
            r.dom_score_limit = sl as u32;
            r.dom_capture_score = cap as u32;
            r.dom_use_ms = (u * 1000.0) as u32;
            r.dom_tick_ms = (tick * 1000.0) as u32;
            r.found_fx |= found::DOM;
        }
    }
    // Perk damage (`initPerkDvars`): `getIntProperty( "perk_bulletDamage", <percent> )` and the explosive one.
    if let Some(t) = get(PERKS) {
        let pct = |key: &str| {
            calls(&t, "getIntProperty", 0)
                .into_iter()
                .find(|(_, a)| a.first().is_some_and(|n| n == key))
                .and_then(|(_, a)| a.get(1).and_then(|s| number(s)))
        };
        if let (Some(b), Some(e)) = (pct("perk_bulletDamage"), pct("perk_explosiveDamage")) {
            r.perk_bullet_damage = b / 100.0;
            r.perk_explosive_damage = e / 100.0;
            r.found_fx |= found::PERK_DAMAGE;
        }
    }
    // EMP: `EMP_JamTeam( otherTeam, <seconds>, delay )`.
    if let Some(v) = get(EMP).and_then(|t| {
        calls(&t, "EMP_JamTeam", 0)
            .into_iter()
            .find_map(|(_, a)| a.get(1).and_then(|s| number(s)))
    }) {
        r.emp_ms = (v * 1000.0) as u32;
        r.found_fx |= found::EMP;
    }
    // Nuke: `setDvarIfUninitialized( "scr_nukeTimer", <s> )` and `delaythread_nuke( (level.nukeTimer + <s>), ::nukeDeath )`.
    if let Some(t) = get(NUKE) {
        let timer = calls(&t, "setDvarIfUninitialized", 0)
            .into_iter()
            .find(|(_, a)| a.first().is_some_and(|n| n == "scr_nukeTimer"))
            .and_then(|(_, a)| a.get(1).and_then(|s| number(s)));
        let death = t.find("::nukeDeath").and_then(|i| {
            let before = &t[..i];
            let plus = before.rfind('+')?;
            number(before[plus + 1..].split(')').next()?)
        });
        if let (Some(timer), Some(death)) = (timer, death) {
            r.nuke_timer_ms = (timer * 1000.0) as u32;
            r.nuke_death_ms = (death * 1000.0) as u32;
            r.found_fx |= found::NUKE;
        }
    }
    for (name, bit, slot) in [
        (SHOCK_FLASH, found::SHOCK_FLASH, &mut r.shock_flash),
        (SHOCK_CONCUSSION, found::SHOCK_STUN, &mut r.shock_stun),
    ] {
        if let Some(p) = scripts
            .get(name)
            .and_then(|t| hud_iw4::ShockParams::parse(t).ok())
        {
            *slot = wpk::rules::ShockFx {
                screen_type: p.screen_type,
                blur_blend_ms: p.blur_blend_ms,
                blur_fade_ms: p.blur_fade_ms,
                white_fade_ms: p.white_fade_ms,
                shot_fade_ms: p.shot_fade_ms,
                movement: p.movement,
                look: wpk::rules::ShockLook {
                    affect: p.look.affect,
                    fade_ms: p.look.fade_ms,
                    sensitivity: p.look.mouse_sensitivity,
                    max_pitch_speed: p.look.max_pitch_speed,
                    max_yaw_speed: p.look.max_yaw_speed,
                },
            };
            r.found_fx |= bit;
        }
    }
    if let Some(t) = get(TDM) {
        if let Some(v) = calls(&t, "registerTimeLimitDvar", 0)
            .first()
            .and_then(|(_, a)| a.get(1).and_then(|s| number(s)))
        {
            r.tdm_time_limit_ms = (v * 60_000.0) as u32;
            r.found |= found::TDM_TIME_LIMIT;
        }
        if let Some(v) = calls(&t, "registerScoreLimitDvar", 0)
            .first()
            .and_then(|(_, a)| a.get(1).and_then(|s| number(s)))
        {
            r.tdm_score_limit = v as u32;
            r.found |= found::TDM_SCORE_LIMIT;
        }
    }
    // Team-mode points: the registrations inside `_rank.gsc`'s `if ( level.teamBased )` block.
    if let Some(t) = get(RANK)
        && let Some(at) = t.find("level.teamBased")
    {
        let block = &t[at..];
        let block = &block[..block.find("else").unwrap_or(block.len())];
        for (key, bit) in [
            ("kill", found::TEAM_KILL_SCORE),
            ("headshot", found::TEAM_HEADSHOT_SCORE),
        ] {
            if let Some(v) = calls(block, "registerScoreInfo", 0)
                .into_iter()
                .find(|(_, a)| a.first().is_some_and(|k| k == key))
                .and_then(|(_, a)| a.get(1).and_then(|s| number(s)))
            {
                if key == "kill" {
                    r.team_kill_score = v as u32;
                } else {
                    r.team_headshot_score = v as u32;
                }
                r.found_team |= bit;
            }
        }
    }
    if let Some(t) = get(FFA) {
        if let Some(v) = calls(&t, "registerTimeLimitDvar", 0)
            .first()
            .and_then(|(_, a)| a.get(1).and_then(|s| number(s)))
        {
            r.time_limit_ms = (v * 60_000.0) as u32;
            r.found |= found::TIME_LIMIT;
        }
        if let Some(v) = calls(&t, "registerScoreLimitDvar", 0)
            .first()
            .and_then(|(_, a)| a.get(1).and_then(|s| number(s)))
        {
            r.score_limit = v as u32;
            r.found |= found::SCORE_LIMIT;
        }
        // The first registration of each (the stock rules; hardcore variants come later).
        for (key, bit) in [
            ("kill", found::KILL_SCORE),
            ("headshot", found::HEADSHOT_SCORE),
            ("assist", found::ASSIST_SCORE),
        ] {
            let v = calls(&t, "registerScoreInfo", 0)
                .into_iter()
                .find(|(_, a)| a.first().is_some_and(|k| k == key))
                .and_then(|(_, a)| a.get(1).and_then(|s| number(s)));
            if let Some(v) = v {
                let v = v as u32;
                match key {
                    "kill" => r.kill_score = v,
                    "headshot" => r.headshot_score = v,
                    _ => r.assist_score = v,
                }
                r.found |= bit;
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up scripts in GSC's syntax (not MW2's text; other values than MW2's).
    #[test]
    fn reads_life_rules_from_scripts() {
        let mut s = HashMap::new();
        s.insert(
            DAMAGE.to_owned(),
            "Callback_PlayerLastStand( a, b )\n{\n\tif ( x )\n\t\tself thread lastStandTimer( 25, true );\n\
             /* self thread lastStandTimer( 3, false ); */\n\telse\n\t\tself thread lastStandTimer( 12, false );\n}\n\
             lastStandAllowSuicide()\n{\n\tif ( gettime() - pressStartTime > 900 )\n\t\tbreak;\n}\n"
                .to_owned(),
        );
        s.insert(
            PERK_FUNCTIONS.to_owned(),
            "setCombatHigh()\n{\n\tself.x fadeOverTime( 1.0 );\n\twait( 6 );\n\twait( 1.5 );\n}\n\
             giveOneManArmyClass( c )\n{\n\tif ( q )\n\t\tchangeDuration = 2.5;\n\telse\n\t\tchangeDuration = 5.0;\n}\n"
                .to_owned(),
        );
        s.insert(
            PERKS.to_owned(),
            "if ( victim _hasperk( \"specialty_combathigh\" ) )\n{\n\tdamage = damage/4;\n}\n"
                .to_owned(),
        );
        s.insert(
            WEAPONS.to_owned(),
            "if ( player _hasPerk( \"specialty_delaymine\" ) )\n\twait 2.5;\nelse\n\twait level.g;\n".to_owned(),
        );
        s.insert(
            CLASS.to_owned(),
            "setKillstreaks( a, b, c )\n{\n\tif ( h )\n\t\tmodifier = -2;\n\telse\n\t\tmodifier = 0;\n}\n".to_owned(),
        );
        s.insert(
            TWEAKABLES.to_owned(),
            "registerTweakable( \"game\", \"allowkillcam\", \"scr_game_allowkillcam\", 0 );"
                .to_owned(),
        );
        let r = read_life(&s);
        assert_eq!(
            (r.final_stand_ms, r.last_stand_ms, r.give_up_ms),
            (25_000, 12_000, 900)
        );
        assert_eq!((r.painkiller_ms, r.painkiller_divisor), (7500, 4.0));
        assert_eq!(
            (r.oma_quick_ms, r.oma_ms, r.delay_mine_ms),
            (2500, 5000, 2500)
        );
        assert_eq!((r.hardline_modifier, r.killcam), (-2, false));
        assert_eq!(r.found, wpk::life_rules::found::ALL);
        assert!(missing_life(&r).is_empty());
        let none = read_life(&HashMap::new());
        assert_eq!(none, wpk::life_rules::LifeRules::default());
        assert_eq!(missing_life(&none).len(), 9);
    }

    #[test]
    fn reads_rules_from_scripts() {
        let mut s = HashMap::new();
        s.insert(
            TWEAKABLES.to_owned(),
            "registerTweakable(\"team\",\"x\",\"y\",1);\n// registerTweakable( \"player\", \"maxhealth\", \"a\", 7 );\n\
             registerTweakable( \"player\",  \"maxhealth\", \"a\", 120 ); //*\nregisterTweakable(\"player\",\"healthregentime\",\"b\",2.5);"
                .to_owned(),
        );
        s.insert(
            HEALTH_OVERLAY.to_owned(),
            "init() { level.healthOverlayCutoff = 0.4; wait ( 9 ); }\nregen()\n{\n regenRate = 0.2;\n for(;;)\n {\n  wait ( 0.1 );\n  hurtTime += x;\n  if ( t > hurtTime + 2500 ) x = 1;\n }\n}"
                .to_owned(),
        );
        s.insert(
            FFA.to_owned(),
            "registerTimeLimitDvar( level.gameType, 12, 0, 60 );\nregisterScoreLimitDvar( level.gameType, 2000, 0, 9 );\n\
             a\\b::registerScoreInfo( \"kill\", 75 );\nregisterScoreInfo( \"assist\", 5 );\nregisterScoreInfo( \"kill\", 1 );"
                .to_owned(),
        );
        s.insert(
            WEAPONS.to_owned(),
            "x()\n{\n self updateMoveSpeedScale( \"primary\" );\n}\n\nupdateMoveSpeedScale( t )\n{\n /*\n self.moveSpeedScaler = 9;\n */\n\
             if ( w == \"riotshield_mp\" )\n { self setMoveSpeedScale( .7 * self.moveSpeedScaler ); }\n switch ( c )\n {\n\
             case \"rifle\": self setMoveSpeedScale( 0.9 * s ); break;\n case \"pistol\": self setMoveSpeedScale( 1.0 * s ); break;\n\
             case \"mg\": self setMoveSpeedScale( 0.85 * s ); break;\n case \"smg\": self setMoveSpeedScale( 1.05 * s ); break;\n\
             case \"spread\": self setMoveSpeedScale( .9 * s ); break;\n case \"rocketlauncher\": self setMoveSpeedScale( 0.75 * s ); break;\n\
             case \"sniper\": self setMoveSpeedScale( 0.98 * s ); break;\n }\n}\n"
                .to_owned(),
        );
        s.insert(
            PERK_FUNCTIONS.to_owned(),
            "setOther()\n{\n self.moveSpeedScaler = 2;\n}\n\nsetLightWeight()\n{\n self.moveSpeedScaler = 1.2;\n}\n".to_owned(),
        );
        s.insert(
            UAV.to_owned(),
            "init()\n{\n level.radarViewTime = 25; // time\n level.uavBlockTime = 9;\n}\n"
                .to_owned(),
        );
        s.insert(
            TDM.to_owned(),
            "main()\n{\n registerTimeLimitDvar( level.gameType, 15, 0, 9 );\n registerScoreLimitDvar( level.gameType, 7000, 0, 9 );\n}\n"
                .to_owned(),
        );
        s.insert(
            RANK.to_owned(),
            "init()\n{\n if ( level.teamBased )\n {\n  registerScoreInfo( \"kill\", 90 );\n  registerScoreInfo( \"headshot\", 80 );\n }\n else\n {\n  registerScoreInfo( \"kill\", 7 );\n }\n}\n"
                .to_owned(),
        );
        s.insert(
            FLASH.to_owned(),
            "monitorFlash()\n{\n if ( amount_angle < 0.4 )\n  amount_angle = 0.4;\n else if ( amount_angle > 0.9 )\n  amount_angle = 1;\n duration = amount_distance * amount_angle * 5;\n if ( duration < 0.3 )\n  continue;\n}\n"
                .to_owned(),
        );
        let weapons = s.get(WEAPONS).cloned().unwrap_or_default();
        s.insert(
            WEAPONS.to_owned(),
            weapons
                + "\nonWeaponDamage( a )\n{\n switch( a )\n {\n  case \"concussion_grenade_mp\":\n   radius = 400;\n   time = 1.5 + ( 3 * scale );\n  break;\n }\n}\n",
        );
        s.insert(
            SHOCK_FLASH.to_owned(),
            "bg_shock_screenType \"flashed\"\nbg_shock_screenBlurBlendTime \"0.2\"\nbg_shock_screenBlurBlendFadeTime \"1\"\nbg_shock_screenFlashWhiteFadeTime \"2\"\nbg_shock_screenFlashShotFadeTime \"1\"\nbg_shock_viewKickPeriod \"1\"\nbg_shock_viewKickRadius \"0\"\nbg_shock_viewKickFadeTime \"1\"\nbg_shock_sound \"0\"\nbg_shock_lookControl \"0\"\nbg_shock_lookControl_maxpitchspeed \"90\"\nbg_shock_lookControl_maxyawspeed \"90\"\nbg_shock_lookControl_mousesensitivityscale \"1\"\nbg_shock_lookControl_fadeTime \"1\"\nbg_shock_movement \"1\"\n"
                .to_owned(),
        );
        let r = read(&s);
        assert_eq!(r.uav_time_ms, 25_000);
        assert_eq!(
            (
                r.flash_seconds,
                r.flash_angle_min,
                r.flash_angle_full,
                r.flash_min_seconds
            ),
            (5.0, 0.4, 0.9, 0.3)
        );
        assert_eq!(
            (r.stun_radius, r.stun_base, r.stun_extra),
            (400.0, 1.5, 3.0)
        );
        assert_eq!((r.tdm_time_limit_ms, r.tdm_score_limit), (900_000, 7000));
        assert_eq!((r.team_kill_score, r.team_headshot_score), (90, 80));
        assert_eq!(
            r.class_speed,
            [0.9, 1.0, 0.85, 1.05, 0.9, 0.75, 0.98, 0.7],
            "the definition, not the call; the block comment ignored"
        );
        assert_eq!(r.lightweight_speed, 1.2);
        assert_eq!(r.max_health, 120, "the commented-out line is ignored");
        assert_eq!(r.regen_delay_ms, 2500);
        assert_eq!(r.hurt_cutoff, 0.4);
        assert_eq!(r.regen_step, 0.2);
        assert_eq!(
            r.regen_interval_ms, 100,
            "the loop's wait, not the one before it"
        );
        assert_eq!(r.very_hurt_wait_ms, 2500);
        assert_eq!((r.time_limit_ms, r.score_limit), (720_000, 2000));
        assert_eq!((r.kill_score, r.assist_score), (75, 5));
        assert_eq!(r.headshot_score, Rules::default().headshot_score);
        assert_eq!(r.found, found::ALL & !found::HEADSHOT_SCORE);
    }

    #[test]
    fn no_scripts_keeps_stock_rules() {
        let r = read(&HashMap::new());
        assert_eq!(r, Rules::default());
    }
}

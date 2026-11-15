//! MW2's destructible types, read as values from `common_scripts/_destructible_types*.gsc` (ADR 0007: read, never
//! run). Each `makeType*` function's `switch` maps a map entity's `destructible_type` to a type function; those are
//! straight lists of `destructible_create` / `_state` / `_part` / `_fx` / `_loopfx` / `_explode` / `_healthdrain` /
//! `_splash_damage_scaler` / `_sound` calls (and calls to other type functions), with string and number arguments,
//! locals and `+` concatenation. This module evaluates exactly that much and records what the calls describe, as
//! `_destructible.gsc` stores it: a type is parts, a part is states, and the effects, explosion and drain listed after a
//! state belong to it (they happen when it breaks).
//!
//! Anything else in an argument (animations, `level` fields, function results) is `undefined`.

use std::collections::HashMap;

/// The type scripts, in the order MW2 consults them (`makeType`, then the DLC ones).
pub const SCRIPTS: [&str; 3] = [
    "common_scripts/_destructible_types.gsc",
    "common_scripts/_destructible_types_dlc.gsc",
    "common_scripts/_destructible_types_dlc2.gsc",
];

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Undefined,
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Value {
    fn str(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }
    fn truthy(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Num(n) => Some(*n != 0.0),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    Punct(char),
}

fn tokenize(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && (c.get(i + 1) == Some(&'*') || c.get(i + 1) == Some(&'#')) {
            // Block comments and developer blocks (`/# ... #/`).
            let close = if c[i + 1] == '*' { '*' } else { '#' };
            i += 2;
            while i + 1 < c.len() && !(c[i] == close && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if ch == '#' {
            // Preprocessor lines (`#include`, `#using_animtree`) and `#animtree`.
            if c[i + 1..].iter().take(7).collect::<String>() == "animtre" {
                i += 1;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Ident("undefined".into()));
            } else {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
        } else if ch == '"' {
            let mut s = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                }
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if ch.is_ascii_digit()
            || (ch == '.' && c.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            let s: String = c[start..i].iter().collect();
            out.push(Tok::Num(s.parse().unwrap_or(0.0)));
        } else if ch.is_alphabetic() || ch == '_' || ch == '%' {
            // `%anim` references read as one (undefined) word.
            let start = i;
            i += 1;
            while i < c.len()
                && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '\\' || c[i] == ':')
            {
                i += 1;
            }
            let w: String = c[start..i].iter().collect();
            out.push(Tok::Ident(if ch == '%' { "undefined".into() } else { w }));
        } else {
            out.push(Tok::Punct(ch));
            i += 1;
        }
    }
    out
}

/// A function: its parameters and body tokens.
struct Func {
    params: Vec<String>,
    body: Vec<Tok>,
}

/// Top-level functions `name( params ) { body }`.
fn functions(toks: &[Tok]) -> HashMap<String, Func> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < toks.len() {
        let (Tok::Ident(name), Some(Tok::Punct('('))) = (&toks[i], toks.get(i + 1)) else {
            i += 1;
            continue;
        };
        let mut j = i + 2;
        let mut params = Vec::new();
        while j < toks.len() && toks[j] != Tok::Punct(')') {
            if let Tok::Ident(p) = &toks[j] {
                params.push(p.to_ascii_lowercase());
            }
            j += 1;
        }
        j += 1;
        if toks.get(j) != Some(&Tok::Punct('{')) {
            i = j;
            continue;
        }
        let start = j + 1;
        let mut depth = 1;
        j += 1;
        while j < toks.len() && depth > 0 {
            match toks[j] {
                Tok::Punct('{') => depth += 1,
                Tok::Punct('}') => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        out.insert(
            name.to_ascii_lowercase(),
            Func {
                params,
                body: toks[start..j - 1].to_vec(),
            },
        );
        i = j;
    }
    out
}

/// What a `destructible_fx` call records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawFx {
    pub group: u8,
    pub tag: Option<String>,
    pub fx: String,
    pub tag_angles: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawExplode {
    pub range: f64,
    pub min_damage: f64,
    pub max_damage: f64,
    pub continue_damage: bool,
    pub origin_offset: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawState {
    pub tag: Option<String>,
    pub model: Option<String>,
    pub health: Option<f64>,
    pub attackers: Option<String>,
    pub cause: Option<String>,
    pub fx: Vec<RawFx>,
    /// (tag, effect, seconds between).
    pub loop_fx: Vec<(Option<String>, String, f64)>,
    /// (amount, seconds between).
    pub drain: Option<(f64, f64)>,
    pub explode: Option<RawExplode>,
    pub sounds: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawType {
    pub splash_scale: Option<f64>,
    /// Part 0 is the body.
    pub parts: Vec<Vec<RawState>>,
}

struct Eval<'a> {
    funcs: &'a HashMap<String, Func>,
    types: Vec<RawType>,
    depth: u32,
}

fn arg(args: &[Value], i: usize) -> &Value {
    args.get(i).unwrap_or(&Value::Undefined)
}

impl Eval<'_> {
    fn state(&mut self) -> Option<&mut RawState> {
        self.types.last_mut()?.parts.last_mut()?.last_mut()
    }

    fn new_state(a: &[Value]) -> RawState {
        RawState {
            tag: arg(a, 0).str(),
            model: arg(a, 1).str().map(|m| m.to_ascii_lowercase()),
            health: arg(a, 2).num(),
            attackers: arg(a, 3).str(),
            cause: arg(a, 5).str(),
            ..RawState::default()
        }
    }

    /// A call to `name` with `args`: a recorded builtin, or another type function.
    fn call(&mut self, name: &str, a: &[Value]) {
        match name {
            "destructible_create" => {
                // (type, tagName, health, validAttackers, validDamageZone, validDamageCause): the body's first state.
                self.types.push(RawType {
                    splash_scale: None,
                    parts: vec![vec![RawState {
                        tag: arg(a, 1).str(),
                        model: None,
                        health: arg(a, 2).num(),
                        attackers: arg(a, 3).str(),
                        cause: arg(a, 5).str(),
                        ..RawState::default()
                    }]],
                });
            }
            "destructible_part" => {
                if let Some(t) = self.types.last_mut() {
                    t.parts.push(vec![Self::new_state(a)]);
                }
            }
            "destructible_state" => {
                if let Some(p) = self.types.last_mut().and_then(|t| t.parts.last_mut()) {
                    p.push(Self::new_state(a));
                }
            }
            "destructible_fx" => {
                let fx = RawFx {
                    tag: arg(a, 0).str(),
                    fx: arg(a, 1).str().unwrap_or_default(),
                    tag_angles: arg(a, 2).truthy().unwrap_or(true),
                    group: arg(a, 4).num().unwrap_or(0.0) as u8,
                };
                if let Some(s) = self.state() {
                    s.fx.push(fx);
                }
            }
            "destructible_loopfx" => {
                let l = (
                    arg(a, 0).str(),
                    arg(a, 1).str().unwrap_or_default(),
                    arg(a, 2).num().unwrap_or(1.0),
                );
                if let Some(s) = self.state() {
                    s.loop_fx.push(l);
                }
            }
            "destructible_healthdrain" => {
                let d = (
                    arg(a, 0).num().unwrap_or(0.0),
                    arg(a, 1).num().unwrap_or(0.0),
                );
                if let Some(s) = self.state() {
                    s.drain = Some(d);
                }
            }
            "destructible_explode" => {
                // (force_min, force_max, rangeSP, rangeMP, mindamage, maxdamage, continueDamage, originOffset, ...)
                let e = RawExplode {
                    range: arg(a, 3).num().unwrap_or(0.0),
                    min_damage: arg(a, 4).num().unwrap_or(0.0),
                    max_damage: arg(a, 5).num().unwrap_or(0.0),
                    continue_damage: arg(a, 6).truthy().unwrap_or(false),
                    origin_offset: arg(a, 7).num(),
                };
                if let Some(s) = self.state() {
                    s.explode = Some(e);
                }
            }
            "destructible_splash_damage_scaler" => {
                if let Some(t) = self.types.last_mut() {
                    t.splash_scale = arg(a, 0).num();
                }
            }
            "destructible_sound" => {
                if let (Some(n), Some(s)) = (arg(a, 0).str(), self.state()) {
                    s.sounds.push(n);
                }
            }
            _ => {
                if let Some(f) = self.funcs.get(name)
                    && name.starts_with(|c: char| c.is_ascii_alphabetic())
                    && !name.starts_with("destructible_")
                    && self.depth < 8
                {
                    let env: HashMap<String, Value> =
                        f.params.iter().cloned().zip(a.iter().cloned()).collect();
                    self.depth += 1;
                    self.run(&f.body, env);
                    self.depth -= 1;
                }
            }
        }
    }

    /// Evaluate `toks[*i..]` as an expression: terms joined by `+`.
    fn expr(&mut self, toks: &[Tok], i: &mut usize, env: &HashMap<String, Value>) -> Value {
        let mut v = self.term(toks, i, env);
        while toks.get(*i) == Some(&Tok::Punct('+')) {
            *i += 1;
            let r = self.term(toks, i, env);
            v = match (v, r) {
                (Value::Num(a), Value::Num(b)) => Value::Num(a + b),
                (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                (Value::Str(a), Value::Num(b)) => Value::Str(format!("{a}{b}")),
                (Value::Num(a), Value::Str(b)) => Value::Str(format!("{a}{b}")),
                _ => Value::Undefined,
            };
        }
        v
    }

    fn term(&mut self, toks: &[Tok], i: &mut usize, env: &HashMap<String, Value>) -> Value {
        let Some(t) = toks.get(*i) else {
            return Value::Undefined;
        };
        *i += 1;
        match t {
            Tok::Str(s) => Value::Str(s.clone()),
            Tok::Num(n) => Value::Num(*n),
            Tok::Punct('-') => match self.term(toks, i, env) {
                Value::Num(n) => Value::Num(-n),
                _ => Value::Undefined,
            },
            Tok::Punct('(') => {
                // A parenthesised expression, or a vector `( x, y, z )` (undefined).
                let v = self.expr(toks, i, env);
                let mut vector = false;
                while *i < toks.len() && toks[*i] != Tok::Punct(')') {
                    vector = true;
                    *i += 1;
                }
                *i += 1;
                if vector { Value::Undefined } else { v }
            }
            Tok::Ident(w) => {
                let lw = w.to_ascii_lowercase();
                if toks.get(*i) == Some(&Tok::Punct('(')) {
                    // A function result: skip its arguments.
                    let mut depth = 0;
                    while *i < toks.len() {
                        match toks[*i] {
                            Tok::Punct('(') => depth += 1,
                            Tok::Punct(')') => {
                                depth -= 1;
                                if depth == 0 {
                                    *i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        *i += 1;
                    }
                    return Value::Undefined;
                }
                // `a.b.c`, `a[ b ]`: undefined.
                let mut accessed = false;
                while matches!(toks.get(*i), Some(Tok::Punct('.' | '['))) {
                    accessed = true;
                    let open = toks[*i] == Tok::Punct('[');
                    *i += 1;
                    if open {
                        while *i < toks.len() && toks[*i] != Tok::Punct(']') {
                            *i += 1;
                        }
                    }
                    *i += 1;
                }
                if accessed {
                    return Value::Undefined;
                }
                match lw.as_str() {
                    "undefined" => Value::Undefined,
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => env.get(&lw).cloned().unwrap_or(Value::Undefined),
                }
            }
            _ => Value::Undefined,
        }
    }

    /// Run a straight-line body: assignments, calls, and the `switch` of a `makeType` (not here: see [`cases`]).
    fn run(&mut self, toks: &[Tok], mut env: HashMap<String, Value>) {
        let mut i = 0;
        while i < toks.len() {
            match (&toks[i], toks.get(i + 1)) {
                (Tok::Ident(name), Some(Tok::Punct('='))) => {
                    i += 2;
                    let v = self.expr(toks, &mut i, &env);
                    env.insert(name.to_ascii_lowercase(), v);
                }
                (Tok::Ident(name), Some(Tok::Punct('('))) => {
                    let name = name.to_ascii_lowercase();
                    i += 2;
                    let mut args = Vec::new();
                    while i < toks.len() && toks[i] != Tok::Punct(')') {
                        args.push(self.expr(toks, &mut i, &env));
                        if toks.get(i) == Some(&Tok::Punct(',')) {
                            i += 1;
                        } else if i < toks.len() && toks[i] != Tok::Punct(')') {
                            // Something we can't read: skip to the closing parenthesis.
                            while i < toks.len() && toks[i] != Tok::Punct(')') {
                                i += 1;
                            }
                        }
                    }
                    i += 1;
                    self.call(&name, &args);
                }
                _ => i += 1,
            }
        }
    }
}

/// A `makeType*` function's cases: (type name, the statements up to its `break`).
fn cases(body: &[Tok]) -> Vec<(String, Vec<Tok>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == Tok::Ident("case".into())
            && let Some(Tok::Str(name)) = body.get(i + 1)
        {
            // Fall-through cases share the statements.
            let mut names = vec![name.clone()];
            i += 3;
            while body.get(i) == Some(&Tok::Ident("case".into())) {
                if let Some(Tok::Str(n)) = body.get(i + 1) {
                    names.push(n.clone());
                }
                i += 3;
            }
            let start = i;
            while i < body.len()
                && body[i] != Tok::Ident("break".into())
                && body[i] != Tok::Ident("case".into())
            {
                i += 1;
            }
            for n in names {
                out.push((n, body[start..i].to_vec()));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Every type the scripts define, by the `destructible_type` value that selects it (lower case).
pub fn parse(sources: &[&str]) -> HashMap<String, RawType> {
    let mut funcs = HashMap::new();
    for s in sources {
        for (k, v) in functions(&tokenize(s)) {
            funcs.entry(k).or_insert(v);
        }
    }
    let mut out = HashMap::new();
    let mut makers: Vec<&String> = funcs.keys().filter(|k| k.starts_with("maketype")).collect();
    makers.sort();
    for m in makers {
        for (name, stmts) in cases(&funcs[m].body) {
            let mut e = Eval {
                funcs: &funcs,
                types: Vec::new(),
                depth: 0,
            };
            let mut env = HashMap::new();
            env.insert("destructibletype".to_owned(), Value::Str(name.clone()));
            e.run(&stmts, env);
            if let Some(t) = e.types.pop() {
                out.entry(name.to_ascii_lowercase()).or_insert(t);
            }
        }
    }
    out
}

/// A map's destructibles, built for its pack (`wpk::destructible`).
#[derive(Default)]
pub struct Built {
    pub models: Vec<wpk::held::Model>,
    pub boxes: Vec<Vec<wpk::destructible::CollBox>>,
    pub types: Vec<wpk::destructible::Type>,
    pub instances: Vec<wpk::destructible::Instance>,
    pub tex: crate::pack::Textures,
    pub fx: crate::fx::Baked,
    pub fx_tex: crate::pack::Textures,
    /// Placed objects left out: their type isn't in the scripts, or their model isn't in the zone.
    pub skipped: usize,
}

/// Destructible models: at most this many vertices (the first LOD under it), textures this many texels across.
const LIMITS: crate::viewmodel::RigidLimits = crate::viewmodel::RigidLimits {
    max_vertices: 1500,
    texture_max: 128,
    role: "destructible",
};

/// IW4 `XModelCollSurf_s` (44 bytes): `collTris`, `numCollTris`, `mins[3]`, `maxs[3]`, `boneIdx`, `contents`,
/// `surfFlags`.
const COLL_SURF: usize = 44;

/// Model `m`'s collision surfaces as boxes (model space).
fn coll_boxes(
    s: &fastfile_iw4::ZoneStream<'_>,
    links: &crate::material_links::Links,
    m: usize,
) -> Vec<wpk::destructible::CollBox> {
    let Some(g) = links.model_geometry.get(m) else {
        return Vec::new();
    };
    let Some(base) = g.coll_surfs else {
        return Vec::new();
    };
    (0..usize::try_from(g.num_coll_surfs).unwrap_or(0).min(64))
        .filter_map(|i| {
            let p = base.at(i * COLL_SURF);
            let f = |o| s.f32_at(p, o).ok();
            Some(wpk::destructible::CollBox {
                mins: [f(8)?, f(12)?, f(16)?],
                maxs: [f(20)?, f(24)?, f(28)?],
                contents: s.u32_at(p, 36).ok()?,
                surface_flags: s.u32_at(p, 40).ok()?,
            })
        })
        .collect()
}

/// IW4's `AnglesToAxis` for an entity's `angles` (pitch, yaw, roll in degrees): forward, left, up.
pub fn axes(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp, sy, cy, sr, cr) = (p.sin(), p.cos(), y.sin(), y.cos(), r.sin(), r.cos());
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    [forward, right.map(|v| -v), up]
}

fn vec3(s: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = s
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

type Shown = (u16, crate::viewmodel::Tags);

fn find_tag(tags: &crate::viewmodel::Tags, name: Option<&str>) -> Option<([f32; 3], [f32; 3])> {
    name.and_then(|n| tags.iter().find(|t| t.0.eq_ignore_ascii_case(n)))
        .map(|t| (t.1, t.2))
}

/// One state as the pack has it; effects carry indices into `fx_names` until baked.
fn state(
    body: &[RawState],
    shown: &[Shown],
    i: usize,
    fx_name: &mut dyn FnMut(&str) -> u16,
) -> wpk::destructible::State {
    use wpk::destructible::{Explode, LoopFx, Spot, State, attackers, cause};
    let st = &body[i];
    let tags = &shown[i].1;
    let spot = |name: Option<&str>, fx: u16, tag_angles: bool, group: u8| {
        let at = find_tag(tags, name);
        Spot {
            effect: fx,
            group,
            // `PlayFX` facing up unless it plays on the tag with the tag's angles.
            world_up: !(tag_angles && at.is_some()),
            origin: at.map_or([0.0; 3], |a| a.0),
            forward: at.map_or([0.0, 0.0, 1.0], |a| a.1),
        }
    };
    let fx = st
        .fx
        .iter()
        .map(|f| spot(f.tag.as_deref(), fx_name(&f.fx), f.tag_angles, f.group))
        .collect();
    let loop_fx = st
        .loop_fx
        .iter()
        .map(|(t, f, rate)| LoopFx {
            spot: spot(t.as_deref(), fx_name(f), true, 0),
            rate_ms: (rate * 1000.0).max(50.0) as u32,
        })
        .collect();
    // The explosion goes off at the next state's tag (else the origin), raised (80 by default).
    let explode = st.explode.as_ref().map(|x| Explode {
        origin: body
            .get(i + 1)
            .and_then(|n| find_tag(&shown[i + 1].1, n.tag.as_deref()))
            .map_or([0.0; 3], |a| a.0),
        lift: x.origin_offset.unwrap_or(80.0) as f32,
        range: x.range as f32,
        min_damage: x.min_damage as f32,
        max_damage: x.max_damage as f32,
        continue_damage: x.continue_damage,
    });
    State {
        model: shown[i].0,
        health: st.health.map_or(-1, |h| h as i32),
        splash_at: st
            .tag
            .as_deref()
            .map(|n| find_tag(tags, Some(n)).map_or([0.0; 3], |a| a.0)),
        attackers: match st.attackers.as_deref() {
            Some("player_only") => attackers::PLAYER_ONLY,
            Some("no_player") => attackers::NO_PLAYER,
            _ => attackers::ANY,
        },
        cause: match st.cause.as_deref() {
            Some("no_melee") => cause::NO_MELEE,
            Some("splash") => cause::SPLASH,
            _ => cause::ANY,
        },
        fx,
        loop_fx,
        drain: st
            .drain
            .filter(|d| d.0 > 0.0 && d.1 > 0.0)
            .map(|(a, every)| (a as i32, (every * 1000.0) as u32)),
        explode,
    }
}

/// Build the destructibles placed in `entities` (MW2's `destructible_type` key on a model entity) from the types in
/// `raw`, with models and effects from the zone.
pub fn build(
    s: &fastfile_iw4::ZoneStream<'_>,
    links: &crate::material_links::Links,
    entities: &[std::collections::BTreeMap<String, String>],
    raw: &HashMap<String, RawType>,
    index: &[crate::zone_textures::IndexEntry],
    tex_dir: &std::path::Path,
) -> Built {
    use wpk::destructible::{Instance, Type};
    use wpk::records::NONE;

    let mut out = Built::default();
    // Models by name: (pack index, tags), or None when it isn't in the zone.
    let mut models: HashMap<String, Option<Shown>> = HashMap::new();
    let mut model = |name: &str, out: &mut Built| -> Option<Shown> {
        if let Some(m) = models.get(name) {
            return m.clone();
        }
        let built = links
            .models
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
            .and_then(|m| {
                let (model, tags) = crate::viewmodel::build_rigid(
                    s,
                    links,
                    m,
                    &[],
                    &LIMITS,
                    index,
                    tex_dir,
                    &mut out.tex,
                )
                .ok()?;
                out.models.push(model);
                out.boxes.push(coll_boxes(s, links, m));
                Some(((out.models.len() - 1) as u16, tags))
            });
        models.insert(name.to_owned(), built.clone());
        built
    };
    // Effects: names first, baked once at the end.
    let mut fx_names: Vec<String> = Vec::new();
    let mut fx_name = |n: &str| -> u16 {
        let n = n.to_ascii_lowercase();
        match fx_names.iter().position(|x| *x == n) {
            Some(i) => i as u16,
            None => {
                fx_names.push(n);
                (fx_names.len() - 1) as u16
            }
        }
    };
    // Types by (type, model).
    let mut types: HashMap<(String, String), u16> = HashMap::new();
    for e in entities {
        let (Some(kind), Some(base)) = (e.get("destructible_type"), e.get("model")) else {
            continue;
        };
        let kind = kind.to_ascii_lowercase();
        let base = base.to_ascii_lowercase();
        let Some(origin) = e.get("origin").and_then(|o| vec3(o)) else {
            continue;
        };
        let angles = e
            .get("angles")
            .and_then(|a| vec3(a))
            .or_else(|| {
                e.get("angle")
                    .and_then(|a| a.trim().parse().ok())
                    .map(|y| [0.0, y, 0.0])
            })
            .unwrap_or([0.0; 3]);
        let key = (kind.clone(), base.clone());
        let t = match types.get(&key) {
            Some(&t) => t,
            None => {
                let (Some(rt), Some(first)) = (raw.get(&kind), model(&base, &mut out)) else {
                    out.skipped += 1;
                    continue;
                };
                let body = rt.parts.first().map_or(&[][..], Vec::as_slice);
                // Each state's model: its own, else the one before.
                let mut shown: Vec<Shown> = Vec::with_capacity(body.len());
                for st in body {
                    let prev = shown.last().cloned().unwrap_or_else(|| first.clone());
                    let m = st
                        .model
                        .as_deref()
                        .and_then(|n| model(n, &mut out))
                        .unwrap_or(prev);
                    shown.push(m);
                }
                let states = (0..body.len())
                    .map(|i| state(body, &shown, i, &mut fx_name))
                    .collect();
                out.types.push(Type {
                    splash_scale: rt.splash_scale.unwrap_or(0.0) as f32,
                    states,
                });
                let t = (out.types.len() - 1) as u16;
                types.insert(key, t);
                t
            }
        };
        out.instances.push(Instance {
            kind: t,
            origin,
            axes: axes(angles),
        });
    }
    // The effects, then each spot's name index turned into the baked effect.
    out.fx = crate::fx::bake(links, &fx_names, index, tex_dir, &mut out.fx_tex);
    let resolve = |i: u16, fx: &crate::fx::Baked| {
        if i == NONE {
            NONE
        } else {
            fx.index(fx_names.get(usize::from(i)).map(String::as_str))
        }
    };
    for t in &mut out.types {
        for st in &mut t.states {
            for f in &mut st.fx {
                f.effect = resolve(f.effect, &out.fx);
            }
            for l in &mut st.loop_fx {
                l.spot.effect = resolve(l.spot.effect, &out.fx);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up type script in the shape of MW2's (no game data).
    const SCRIPT: &str = r#"
#include common_scripts\_destructible;
#using_animtree( "destructibles" );

makeType( destructibleType )
{
	infoIndex = getInfoIndex( destructibleType );
	if ( infoIndex >= 0 )
		return infoIndex;
	switch ( destructibleType )
	{
		case "toy_box":
			toy_box( destructibleType );
			break;
		case "toy_lamp_a":
		case "toy_lamp_b":
			toy_lamp( "_a" );
			break;
	}
}

toy_box( destructibleType )
{
	/* a box */
	destructible_create( destructibleType, "tag_origin", 150, undefined, 32, "no_melee" );
		destructible_splash_damage_scaler( 15 );
			destructible_fx( "tag_fx", "props/box_explode", undefined, undefined, undefined, 1 );
			destructible_loopfx( "tag_fx", "smoke/box_smoke", 0.4 );
			destructible_healthdrain( 12, 0.2 );
			destructible_explode( 20, 2000, 32, 48, 10, 60, undefined, 0 ); // a comment
			destructible_anim( %box_anim, #animtree, "setanimknob" );
		destructible_state( undefined, "box_dest", undefined, undefined, "no_melee" );
		destructible_part( "tag_door", "box_door", 4, undefined, undefined, undefined, 1.0, 1.0 );
}

toy_lamp( suffix )
{
	tag = "tag_bulb";
	destructible_create( "lamp" + suffix, tag, 20, "player_only" );
		destructible_fx( tag + "_fx", "props/lamp_pop", false );
		destructible_state( tag + "_d", "lamp" + suffix + "_broken", 60 );
		destructible_state( undefined );
}
"#;

    #[test]
    fn reads_types_from_the_switch() {
        let t = parse(&[SCRIPT]);
        assert_eq!(t.len(), 3);
        let b = &t["toy_box"];
        assert_eq!(b.splash_scale, Some(15.0));
        assert_eq!(b.parts.len(), 2);
        let body = &b.parts[0];
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].health, Some(150.0));
        assert_eq!(body[0].cause.as_deref(), Some("no_melee"));
        assert_eq!(body[0].fx[0].fx, "props/box_explode");
        assert!(body[0].fx[0].tag_angles);
        assert_eq!(
            body[0].loop_fx[0],
            (Some("tag_fx".into()), "smoke/box_smoke".into(), 0.4)
        );
        assert_eq!(body[0].drain, Some((12.0, 0.2)));
        let e = body[0].explode.as_ref().unwrap();
        assert_eq!((e.range, e.min_damage, e.max_damage), (48.0, 10.0, 60.0));
        assert_eq!(e.origin_offset, Some(0.0));
        assert_eq!(body[1].model.as_deref(), Some("box_dest"));
        assert_eq!(body[1].health, None);
        assert_eq!(b.parts[1][0].tag.as_deref(), Some("tag_door"));
        // Fall-through cases, parameters and concatenation.
        let l = &t["toy_lamp_b"];
        assert_eq!(l, &t["toy_lamp_a"]);
        let body = &l.parts[0];
        assert_eq!(body[0].attackers.as_deref(), Some("player_only"));
        assert_eq!(body[0].fx[0].tag.as_deref(), Some("tag_bulb_fx"));
        assert!(!body[0].fx[0].tag_angles);
        assert_eq!(body[1].tag.as_deref(), Some("tag_bulb_d"));
        assert_eq!(body[1].model.as_deref(), Some("lamp_a_broken"));
        assert_eq!(body[2], RawState::default());
    }
}

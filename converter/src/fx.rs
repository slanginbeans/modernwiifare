//! Effects for `common.wpk` (`wpk::fx`): the effects the game plays (each class weapon's muzzle flashes and
//! explosion, every thrown weapon's explosion, the impact table's rows for the bullets the weapons fire) and the
//! child effects their runner elements start, read from the zone's `FxEffectDef`s (`material_links::FxElemCapture`,
//! IW4L's `fx_iw4` views). Sprite elements keep their material's colour image (top mips left out down to
//! [`TEXTURE_MAX`]) and blend.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use wpk::fx::{Effect, Elem, IMPACT_COLUMNS, IMPACT_ROWS, Vec3Range, kind};
use wpk::records::{NONE, Wrap};

use crate::material_links::{BlendMode, FxElemCapture, Links};
use crate::pack::Textures;
use crate::zone_textures::IndexEntry;

/// Effect textures are at most this many texels across.
const TEXTURE_MAX: u32 = 64;
/// Runner children followed this many levels down.
const DEPTH: u32 = 4;
/// IW4 velocity-sample size and where its world graph starts.
const VEL_SAMPLE: usize = 0x60;
const VEL_WORLD: usize = 0x30;

/// One element with its samples owned (the encoder borrows them).
struct OwnedElem {
    elem: Elem<'static>,
    vis: Vec<u8>,
}

/// The baked effects.
#[derive(Default)]
pub struct Baked {
    effects: Vec<Vec<OwnedElem>>,
    by_name: HashMap<String, u16>,
    pub missing: usize,
    pub elems: usize,
    pub dropped_elems: usize,
}

impl Baked {
    /// The effect named `name` (lower case), if baked.
    pub fn index(&self, name: Option<&str>) -> u16 {
        name.and_then(|n| self.by_name.get(&n.to_ascii_lowercase()))
            .copied()
            .unwrap_or(NONE)
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// The `FXDF` section.
    pub fn encode(&self) -> Vec<u8> {
        let effects: Vec<Effect> = self
            .effects
            .iter()
            .map(|e| Effect {
                elems: e
                    .iter()
                    .map(|o| Elem {
                        vis: &o.vis,
                        ..o.elem.clone()
                    })
                    .collect(),
            })
            .collect();
        wpk::fx::encode(&effects)
    }
}

fn graph(raw: &[u8], count: usize, world: bool) -> Vec<Vec3Range> {
    (0..count)
        .filter_map(|i| {
            let at = i * VEL_SAMPLE + if world { VEL_WORLD } else { 0 };
            let r = raw.get(at..at + 24)?;
            let f = |k: usize| f32::from_le_bytes(r[k * 4..k * 4 + 4].try_into().unwrap_or([0; 4]));
            Some(Vec3Range {
                base: [f(0), f(1), f(2)],
                amplitude: [f(3), f(4), f(5)],
            })
        })
        .collect()
}

/// Bake `roots` and the effects their runners start. `index` / `tex_dir`: the zone's converted textures.
pub fn bake(
    links: &Links,
    roots: &[String],
    index: &[IndexEntry],
    tex_dir: &Path,
    tex: &mut Textures,
) -> Baked {
    let mut files_by_image: HashMap<usize, &IndexEntry> = HashMap::new();
    for e in index {
        for &i in &e.images {
            files_by_image.entry(i).or_insert(e);
        }
    }
    let mut out = Baked::default();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();
    let enqueue = |out: &mut Baked, queue: &mut VecDeque<(String, u32)>, n: &str, depth: u32| {
        let n = n.to_ascii_lowercase();
        if let Some(&i) = out.by_name.get(&n) {
            return i;
        }
        if links.fx_elems(&n).is_none() {
            out.missing += 1;
            return NONE;
        }
        let i = out.by_name.len() as u16;
        out.by_name.insert(n.clone(), i);
        queue.push_back((n, depth));
        i
    };
    for r in roots {
        enqueue(&mut out, &mut queue, r, 0);
    }
    while let Some((name, depth)) = queue.pop_front() {
        let mut elems = Vec::new();
        for c in links.fx_elems(&name).unwrap_or(&[]) {
            let FxElemCapture {
                view: v,
                looping,
                atlas,
                vel,
                vis,
                materials,
                runners,
            } = c;
            let (texture, blend) = match v.elem_type {
                kind::BILLBOARD | kind::ORIENTED | kind::TAIL | kind::CLOUD => {
                    // Screen distortion (heat haze) needs the frame behind it: left out.
                    let Some(&m) = materials.first().filter(|&&m| {
                        !links.material_info[m]
                            .techset
                            .as_deref()
                            .is_some_and(|t| t.trim_start_matches(',').starts_with("distortion"))
                    }) else {
                        out.dropped_elems += 1;
                        continue;
                    };
                    let texture = links
                        .colour_image(m)
                        .and_then(|img| files_by_image.get(&img))
                        .and_then(|e| {
                            tex.add_file_capped(tex_dir, &e.file, Wrap::Clamp, TEXTURE_MAX)
                                .ok()
                        })
                        .unwrap_or(NONE);
                    let blend = effect_blend(&links.material_info[m]);
                    (texture, blend as u8)
                }
                kind::RUNNER if depth < DEPTH => {
                    let Some(child) = runners.first() else {
                        out.dropped_elems += 1;
                        continue;
                    };
                    let i = enqueue(&mut out, &mut queue, child, depth + 1);
                    if i == NONE {
                        out.dropped_elems += 1;
                        continue;
                    }
                    (i, 0)
                }
                _ => {
                    out.dropped_elems += 1;
                    continue;
                }
            };
            let vel_count = usize::from(v.vel_interval_count) + 1;
            let vis_len = (usize::from(v.vis_state_interval_count) + 1) * wpk::fx::VIS_SAMPLE;
            if vis.len() < vis_len && v.elem_type != kind::RUNNER {
                out.dropped_elems += 1;
                continue;
            }
            let vel_local = if fx_iw4::elem_uses_vel_local(v.flags) {
                graph(vel, vel_count, false)
            } else {
                Vec::new()
            };
            let vel_world = if fx_iw4::elem_uses_vel_world(v.flags) {
                graph(vel, vel_count, true)
            } else {
                Vec::new()
            };
            out.elems += 1;
            elems.push(OwnedElem {
                elem: Elem {
                    kind: v.elem_type,
                    flags: v.flags,
                    looping: *looping,
                    texture,
                    blend,
                    spawn: [v.spawn_a, v.spawn_b],
                    delay_ms: [v.spawn_delay_msec_base, v.spawn_delay_msec_amplitude],
                    life_ms: [v.life_span_msec_base, v.life_span_msec_amplitude],
                    origin: v.spawn_origin,
                    offset_radius: [v.spawn_offset_radius_base, v.spawn_offset_radius_amplitude],
                    offset_height: [v.spawn_offset_height_base, v.spawn_offset_height_amplitude],
                    angles: v.spawn_angles,
                    angular_velocity: v.angular_velocity,
                    initial_rotation: v.initial_rotation,
                    gravity: [v.gravity_base, v.gravity_amplitude],
                    atlas: *atlas,
                    vis_intervals: if v.elem_type == kind::RUNNER {
                        0
                    } else {
                        v.vis_state_interval_count
                    },
                    vel_local,
                    vel_world,
                    vis: &[],
                },
                vis: if v.elem_type == kind::RUNNER {
                    vec![0; wpk::fx::VIS_SAMPLE]
                } else {
                    vis[..vis_len].to_vec()
                },
            });
        }
        let i = usize::from(out.by_name[&name]);
        if out.effects.len() <= i {
            out.effects.resize_with(i + 1, Vec::new);
        }
        out.effects[i] = elems;
    }
    out
}

/// An effect material's blend: by its technique set's name (IW4's effect techsets say how they blend: `..._add`,
/// `..._screen`, `..._multiply`, `..._blend`), else its colour pass's state bits.
fn effect_blend(m: &crate::material_links::MaterialInfo) -> wpk::records::Blend {
    use wpk::records::Blend;
    let t = m.techset.as_deref().unwrap_or("").to_ascii_lowercase();
    if t.contains("add") || t.contains("screen") {
        return Blend::Additive;
    }
    if t.contains("multiply") {
        return Blend::Multiply;
    }
    if t.contains("blend") {
        return Blend::Blend;
    }
    match m.blend {
        BlendMode::Additive => Blend::Additive,
        BlendMode::Multiply => Blend::Multiply,
        _ => Blend::Blend,
    }
}

/// The impact table section (`FXIM`): the baked rows, NONE elsewhere.
pub fn impact_section(baked: &Baked, table: &[[Option<String>; IMPACT_COLUMNS]]) -> Vec<u8> {
    let mut b = Vec::with_capacity(IMPACT_ROWS * IMPACT_COLUMNS * 2);
    for row in 0..IMPACT_ROWS {
        for col in 0..IMPACT_COLUMNS {
            let n = table.get(row).and_then(|r| r[col].as_deref());
            b.extend_from_slice(&baked.index(n).to_be_bytes());
        }
    }
    b
}

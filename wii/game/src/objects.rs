//! Destructible objects on the Wii (the map pack's `DST*`, `game_core::destructible`): their models drawn rigidly at
//! each object's place in its current state, unlit (×1) like the characters for now.

use alloc::vec::Vec;

use game_core::destructible::Objects;
use game_core::gfx::{DrawState, Gfx, Mtx34};
use game_core::viewmodel as vmod;
use wpk::held::Guns;

use crate::ffi;

/// Objects further than this from the eye aren't drawn.
const DRAW_DISTANCE: f32 = 3000.0;

pub struct Props {
    /// The map's light lattice (none: unlit, ×1).
    pub lattice: Option<wpk::light::LightLattice<'static>>,
    models: Guns<'static>,
    /// Each model's reach from its origin (for culling).
    radius: Vec<f32>,
    textures: Vec<i32>,
}

impl Props {
    /// The map's objects (`None` without any) and what draws them (textures created through `gfx`).
    pub fn load(map: &'static [u8], gfx: &mut impl Gfx) -> Option<(Props, Objects)> {
        let pack = wpk::Pack::parse_prefix(map).ok()?;
        let models = Guns::parse(pack.section(wpk::tag::DEST_MODELS)?.data)?;
        let types = wpk::destructible::decode_types(pack.section(wpk::tag::DEST_TYPES)?.data)?;
        let boxes = wpk::destructible::decode_boxes(pack.section(wpk::tag::DEST_BOXES)?.data)?;
        let instances =
            wpk::destructible::decode_instances(pack.section(wpk::tag::DEST_INSTANCES)?.data);
        let objects = Objects::new(types, boxes, &instances);
        if objects.is_empty() {
            return None;
        }
        let radius = (0..models.len())
            .map(|i| {
                models.model(i).map_or(0.0, |m| {
                    m.positions
                        .chunks_exact(12)
                        .map(|p| {
                            let f = |k: usize| wpk::be::f32(p, k * 4);
                            libm::sqrtf(f(0) * f(0) + f(1) * f(1) + f(2) * f(2))
                        })
                        .fold(0.0, f32::max)
                })
            })
            .collect();
        let textures = crate::effects::create_textures(
            &pack,
            wpk::tag::DEST_TEXTURES,
            wpk::tag::DEST_TEXTURE_DATA,
            gfx,
        );
        Some((
            Props {
                lattice: None,
                models,
                radius,
                textures,
            },
            objects,
        ))
    }

    /// Draw `objects` seen through `view` (world to eye) from `eye` looking along `forward`.
    pub fn draw(
        &self,
        gfx: &mut impl Gfx,
        view: &Mtx34,
        eye: [f32; 3],
        forward: [f32; 3],
        objects: &Objects,
    ) {
        for i in 0..objects.len() {
            let Some((inst, model)) = objects.draw(i) else {
                continue;
            };
            let Some(m) = self.models.model(usize::from(model)) else {
                continue;
            };
            let r = self.radius.get(usize::from(model)).copied().unwrap_or(0.0);
            let d: [f32; 3] = core::array::from_fn(|k| inst.origin[k] - eye[k]);
            let ahead = d[0] * forward[0] + d[1] * forward[1] + d[2] * forward[2];
            let dist2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if ahead < -r || dist2 > (DRAW_DISTANCE + r) * (DRAW_DISTANCE + r) {
                continue;
            }
            let a = inst.axes;
            let o = inst.origin;
            let world: Mtx34 = [
                [a[0][0], a[1][0], a[2][0], o[0]],
                [a[0][1], a[1][1], a[2][1], o[1]],
                [a[0][2], a[1][2], a[2][2], o[2]],
            ];
            unsafe {
                ffi::mwf_world_view();
                ffi::mwf_view_arrays(m.positions.as_ptr(), m.attributes.as_ptr());
            }
            gfx.pos_matrix(&vmod::mul(view, &world));
            // Lit by the light lattice at its origin (as static models are by the light grid).
            let tint = game_core::light::tint_at(self.lattice.as_ref(), o);
            let mut last: Option<DrawState> = None;
            for k in 0..m.draws() {
                let Some(dr) = m.draw(k) else { continue };
                let state = DrawState {
                    tex: self
                        .textures
                        .get(usize::from(dr.texture))
                        .copied()
                        .unwrap_or(-1),
                    lightmap: -1,
                    blend: dr.blend,
                    alpha_ref: dr.alpha_ref,
                    cull: 0,
                    tint,
                };
                if last != Some(state) {
                    gfx.world_state(&state);
                    last = Some(state);
                }
                gfx.call_dl(dr.dl);
            }
        }
    }
}

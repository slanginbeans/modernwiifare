//! MW2's effects on the Wii (`common.wpk` `FXDF`, `game_core::fx`): bullet impacts by surface, explosions and muzzle
//! flashes in the world, and the first-person muzzle flash in the gun's own space (it moves with the view). The map
//! pack's own effects (`DSFX`: what its destructible objects play) run beside them.

use alloc::vec::Vec;

use game_core::camera::Camera;
use game_core::fx::{Batch, Fx};
use game_core::gfx::{Gfx, Mtx34, SpriteVert, TexDesc};
use wpk::fx::{Effect, IMPACT_COLUMNS};
use wpk::records::{self, NONE, TextureRecord, texture_flags};

/// Which of a weapon's effects (`FXWP`).
pub const WORLD_FLASH: usize = 0;
pub const VIEW_FLASH: usize = 1;
pub const EXPLOSION: usize = 2;

const IDENTITY: Mtx34 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// Create the textures of a pack's texture sections `records` / `data` (in MEM2 for the run).
pub fn create_textures(
    pack: &wpk::Pack<'static>,
    records: wpk::Tag,
    data: wpk::Tag,
    gfx: &mut impl Gfx,
) -> Vec<i32> {
    let (Some(r), Some(d)) = (pack.section(records), pack.section(data)) else {
        return Vec::new();
    };
    let txda = d.data;
    records::iter::<TextureRecord>(r.data)
        .map(|r| {
            let Some(data) = txda.get(r.data_off as usize..(r.data_off + r.data_len) as usize)
            else {
                return -1;
            };
            let alpha = if r.flags & texture_flags::ALPHA_PLANE != 0 {
                txda.get(r.alpha_off as usize..(r.alpha_off + r.alpha_len) as usize)
                    .map_or(core::ptr::null(), |a| a.as_ptr())
            } else {
                core::ptr::null()
            };
            gfx.tex_create(&TexDesc {
                data: data.as_ptr(),
                alpha,
                width: r.width,
                height: r.height,
                format: r.format,
                levels: r.levels,
                wrap: r.wrap,
            })
        })
        .collect()
}

/// The effect definitions, the effects playing and the buffers to draw them.
pub struct Effects {
    pub defs: Vec<Effect<'static>>,
    weapons: &'static [u8],
    impacts: &'static [u8],
    textures: Vec<i32>,
    /// In the world, and in the first-person model's space.
    pub world: Fx,
    pub view: Fx,
    /// The map's effects: definitions, textures, and those playing.
    map: Option<(Vec<Effect<'static>>, Vec<i32>, Fx)>,
    order: Vec<(u8, u16, u16)>,
    verts: Vec<SpriteVert>,
    batches: Vec<Batch>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    b.get(i * 2..i * 2 + 2)
        .map_or(NONE, |x| u16::from_be_bytes([x[0], x[1]]))
}

impl Effects {
    /// The common pack's effects (their textures created through `gfx`), or `None` with an older pack.
    pub fn load(common: &'static [u8], gfx: &mut impl Gfx) -> Option<Effects> {
        let pack = wpk::Pack::parse(common, false).ok()?;
        let defs = wpk::fx::decode(pack.section(wpk::tag::FX_DEFS)?.data)?;
        Some(Effects {
            defs,
            weapons: pack.section(wpk::tag::FX_WEAPONS)?.data,
            impacts: pack.section(wpk::tag::FX_IMPACTS)?.data,
            textures: create_textures(&pack, wpk::tag::FX_TEXTURES, wpk::tag::FX_TEXTURE_DATA, gfx),
            world: Fx::new(0x2545_f491),
            view: Fx::new(0x9e37_79b9),
            map: None,
            order: Vec::with_capacity(game_core::fx::MAX_PARTICLES),
            verts: Vec::with_capacity(game_core::fx::MAX_PARTICLES * 4),
            batches: Vec::with_capacity(64),
        })
    }

    /// Weapon record `weapon`'s effect `which` ([`WORLD_FLASH`], [`VIEW_FLASH`], [`EXPLOSION`]), or NONE.
    pub fn weapon(&self, weapon: u32, which: usize) -> u16 {
        u16_at(self.weapons, weapon as usize * 3 + which)
    }

    /// The impact table's effect for a bullet of IW4 impact type `impact_type` on IW4 surface type `surface` (or on
    /// a player: `flesh`), or NONE.
    pub fn impact(&self, impact_type: i32, surface: u8, flesh: bool) -> u16 {
        let Some(row) = fx_iw4::impact_table_row(impact_type, false) else {
            return NONE;
        };
        let col = if flesh {
            fx_iw4::FX_IMPACT_NONFLESH_COUNT
        } else {
            usize::from(surface).min(fx_iw4::FX_IMPACT_NONFLESH_COUNT - 1)
        };
        u16_at(self.impacts, row * IMPACT_COLUMNS + col)
    }

    /// Play `effect` in the world (NONE plays nothing).
    pub fn play(&mut self, effect: u16, origin: [f32; 3], forward: [f32; 3], now: u32) {
        if effect != NONE {
            self.world.play(&self.defs, effect, origin, forward, now);
        }
    }

    /// Add the map pack's effects (`DSFX`; none in an older pack).
    pub fn load_map(&mut self, map: &'static [u8], gfx: &mut impl Gfx) {
        let Ok(pack) = wpk::Pack::parse_prefix(map) else {
            return;
        };
        let Some(defs) = pack
            .section(wpk::tag::DEST_FX)
            .and_then(|s| wpk::fx::decode(s.data))
        else {
            return;
        };
        let textures = create_textures(
            &pack,
            wpk::tag::DEST_FX_TEXTURES,
            wpk::tag::DEST_FX_TEXTURE_DATA,
            gfx,
        );
        self.map = Some((defs, textures, Fx::new(0x7f4a_7c15)));
    }

    /// Play the map's effect `effect` in the world (NONE plays nothing).
    pub fn play_map(&mut self, effect: u16, origin: [f32; 3], forward: [f32; 3], now: u32) {
        if let Some((defs, _, fx)) = self.map.as_mut()
            && effect != NONE
        {
            fx.play(defs, effect, origin, forward, now);
        }
    }

    /// Play `effect` in the first-person model's (eye) space.
    pub fn play_view(&mut self, effect: u16, origin: [f32; 3], forward: [f32; 3], now: u32) {
        if effect != NONE {
            self.view.play(&self.defs, effect, origin, forward, now);
        }
    }

    fn draw(&mut self, gfx: &mut impl Gfx, view: &Mtx34, world: bool) {
        let textures = &self.textures;
        Self::draw_batches(gfx, view, world, textures, &self.verts, &self.batches);
    }

    fn draw_batches(
        gfx: &mut impl Gfx,
        view: &Mtx34,
        world: bool,
        textures: &[i32],
        verts: &[SpriteVert],
        batches: &[Batch],
    ) {
        for b in batches {
            let tex = textures.get(usize::from(b.texture)).copied().unwrap_or(-1);
            gfx.sprites(
                view,
                tex,
                b.blend,
                &verts[b.first..b.first + b.count],
                world,
            );
        }
    }

    /// Advance and draw the world's effects (after the world and the characters).
    pub fn draw_world(&mut self, gfx: &mut impl Gfx, cam: &Camera, now: u32) {
        if let Some((defs, textures, fx)) = self.map.as_mut() {
            fx.update(defs, now);
            if fx.particles() > 0 {
                fx.sprites(
                    defs,
                    now,
                    cam.right(),
                    cam.up(),
                    cam.eye,
                    &mut self.order,
                    &mut self.verts,
                    &mut self.batches,
                );
                Self::draw_batches(gfx, &cam.view(), true, textures, &self.verts, &self.batches);
            }
        }
        self.world.update(&self.defs, now);
        if self.world.particles() == 0 {
            return;
        }
        self.world.sprites(
            &self.defs,
            now,
            cam.right(),
            cam.up(),
            cam.eye,
            &mut self.order,
            &mut self.verts,
            &mut self.batches,
        );
        self.draw(gfx, &cam.view(), true);
    }

    /// Advance and draw the first-person effects (right after the first-person model, in its projection).
    pub fn draw_view(&mut self, gfx: &mut impl Gfx, now: u32) {
        self.view.update(&self.defs, now);
        if self.view.particles() == 0 {
            return;
        }
        // Eye space: x right, y up, looking down -z.
        self.view.sprites(
            &self.defs,
            now,
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0; 3],
            &mut self.order,
            &mut self.verts,
            &mut self.batches,
        );
        self.draw(gfx, &IDENTITY, false);
    }
}

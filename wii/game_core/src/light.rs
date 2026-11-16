//! Light for the models that move (the viewmodel, characters, destructible objects): the map's light lattice
//! (`wpk::light`, IW4's light grid sampled at convert time with IW4L's atapoint lookup) at each model's lighting
//! origin, as the konst colour the shim's tint stage multiplies the model by (as static models already are).
//!
//! The viewmodel's lighting origin is IW4L `lighting_iw4::viewmodel_lighting_origin` (Apache-2.0, rev `acc57a5`;
//! `lighting_iw4` itself isn't a Wii dependency): the eye (origin + view height) moved by the lean
//! (`math_iw4::add_lean_to_position`, IW4's 16° view roll and 20-unit lean distance for the view weapon). A
//! character's is its middle (origin + [`CHARACTER_LIGHT_HEIGHT`]).

use wpk::light::LightLattice;

/// IW4L `viewmodel_origin.rs`: `VIEWWEAPON_LEAN_VIEW_ROLL`, `VIEWWEAPON_LEAN_DIST`.
const VIEWWEAPON_LEAN_VIEW_ROLL: f32 = 16.0;
const VIEWWEAPON_LEAN_DIST: f32 = 20.0;
/// Where a standing character is lit from, above its feet (about its middle).
pub const CHARACTER_LIGHT_HEIGHT: f32 = 36.0;

/// IW4L `viewmodel_lighting_origin`.
pub fn viewmodel_origin(origin: [f32; 3], view_height: f32, view_yaw: f32, leanf: f32) -> [f32; 3] {
    math_iw4::add_lean_to_position(
        [origin[0], origin[1], origin[2] + view_height],
        view_yaw,
        leanf,
        VIEWWEAPON_LEAN_VIEW_ROLL,
        VIEWWEAPON_LEAN_DIST,
    )
}

/// The tint (`DrawState::tint`) for a model lit at `at`: 0 (untinted, ×1) without a lattice or light there.
pub fn tint_at(lattice: Option<&LightLattice<'_>>, at: [f32; 3]) -> u32 {
    lattice
        .and_then(|l| l.sample(at))
        .map_or(0, wpk::light::tint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewmodel_origin_is_the_eye_moved_by_the_lean() {
        assert_eq!(
            viewmodel_origin([10.0, 20.0, 0.0], 60.0, 0.0, 0.0),
            [10.0, 20.0, 60.0]
        );
        // Leaning right (positive) at yaw 0 moves it along -y (IW4's right), by up to the lean distance.
        let p = viewmodel_origin([0.0; 3], 60.0, 0.0, 1.0);
        assert!(p[1] < -1.0 && p[1] > -VIEWWEAPON_LEAN_DIST - 0.1, "{p:?}");
    }

    #[test]
    fn tint_follows_the_lattice() {
        let b = wpk::light::encode([0.0; 3], [100.0; 3], [1, 1, 1], &[[64, 64, 64, 255]]);
        let l = LightLattice::decode(&b).unwrap();
        assert_eq!(tint_at(Some(&l), [0.0; 3]), 0x4040_40ff);
        assert_eq!(tint_at(None, [0.0; 3]), 0);
    }
}

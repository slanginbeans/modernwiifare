//! Per-role size limits and format choice (see the `iw4-asset-pipeline` skill).

use alloc::vec::Vec;

use crate::codec;
use crate::format::TexFormat;
use crate::image::{Image, fit_pow2, mip_chain, resize};

/// What a texture is used for. Decides its size cap, format family and whether it gets mips.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    World,
    Prop,
    Character,
    Viewmodel,
    Effect,
    Sky,
    Ui,
    /// Grey masks / detail maps.
    Mask,
}

impl Role {
    pub const ALL: [Role; 8] = [
        Role::World,
        Role::Prop,
        Role::Character,
        Role::Viewmodel,
        Role::Effect,
        Role::Sky,
        Role::Ui,
        Role::Mask,
    ];

    /// Longest side after conversion.
    pub const fn max_dim(self) -> u32 {
        match self {
            Role::World | Role::Character => 256,
            Role::Prop | Role::Effect | Role::Mask => 128,
            Role::Viewmodel | Role::Sky | Role::Ui => 512,
        }
    }

    pub const fn mipmapped(self) -> bool {
        !matches!(self, Role::Ui)
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL
            .into_iter()
            .find(|r| r.name().eq_ignore_ascii_case(s))
    }

    pub const fn name(self) -> &'static str {
        match self {
            Role::World => "world",
            Role::Prop => "prop",
            Role::Character => "character",
            Role::Viewmodel => "viewmodel",
            Role::Effect => "effect",
            Role::Sky => "sky",
            Role::Ui => "ui",
            Role::Mask => "mask",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Opaque,
    /// On/off only (cut-outs). A few anti-aliased edge texels are tolerated.
    Binary,
    Smooth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Traits {
    pub grey: bool,
    pub alpha: Alpha,
}

pub fn analyze(img: &Image) -> Traits {
    let grey = img.pixels.iter().all(|p| {
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        (r - g).abs() <= 2 && (g - b).abs() <= 2
    });
    let n = img.pixels.len().max(1);
    let opaque = img.pixels.iter().filter(|p| p[3] >= 250).count();
    let clear = img.pixels.iter().filter(|p| p[3] <= 5).count();
    let mid = n - opaque - clear;
    let alpha = if opaque == n {
        Alpha::Opaque
    } else if mid * 50 <= n {
        Alpha::Binary
    } else {
        Alpha::Smooth
    };
    Traits { grey, alpha }
}

/// Format for a texture of this role and content.
pub fn choose_format(role: Role, t: Traits) -> TexFormat {
    let hi = matches!(role, Role::Ui | Role::Viewmodel);
    match (t.grey, t.alpha) {
        (true, Alpha::Opaque) => {
            if hi {
                TexFormat::I8
            } else {
                TexFormat::I4
            }
        }
        (true, _) => {
            if hi {
                TexFormat::IA8
            } else {
                TexFormat::IA4
            }
        }
        (false, Alpha::Opaque) if role == Role::Ui => TexFormat::Rgb565,
        (false, Alpha::Binary) if role == Role::Ui => TexFormat::Rgb5a3,
        (false, Alpha::Opaque | Alpha::Binary) => TexFormat::Cmpr,
        (false, Alpha::Smooth) => TexFormat::Rgb5a3,
    }
}

/// A texture ready for the Wii: level 0 first, then each mip, back to back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub format: TexFormat,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    pub traits: Traits,
    pub data: Vec<u8>,
}

impl Converted {
    /// Decode level `level` (for previews and checks).
    pub fn decode_level(&self, level: u32) -> Option<Image> {
        if level >= self.levels {
            return None;
        }
        let mut off = 0;
        for l in 0..level {
            off += self
                .format
                .level_size((self.width >> l).max(1), (self.height >> l).max(1));
        }
        codec::decode(
            &self.data[off..],
            (self.width >> level).max(1),
            (self.height >> level).max(1),
            self.format,
        )
    }
}

/// Smallest mip worth storing: below one block the level is mostly padding.
fn min_mip_dim(fmt: TexFormat) -> u32 {
    fmt.block_dims().0.min(fmt.block_dims().1)
}

/// Resize to the role's cap (power of two), pick the format, build mips, and encode.
pub fn convert(img: &Image, role: Role) -> Converted {
    let traits = analyze(img);
    let format = choose_format(role, traits);
    let (w, h) = fit_pow2(img.width, img.height, role.max_dim());
    let base = resize(img, w, h);
    let chain = if role.mipmapped() {
        mip_chain(&base, min_mip_dim(format))
    } else {
        alloc::vec![base]
    };
    let mut data = Vec::with_capacity(format.chain_size(w, h, chain.len() as u32));
    for level in &chain {
        data.extend_from_slice(&codec::encode(level, format));
    }
    Converted {
        format,
        width: w,
        height: h,
        levels: chain.len() as u32,
        traits,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis() {
        let opaque = Image::new(4, 4, [10, 20, 30, 255]);
        assert_eq!(
            analyze(&opaque),
            Traits {
                grey: false,
                alpha: Alpha::Opaque
            }
        );
        let cutout = Image::from_fn(10, 10, |x, _| [9, 9, 9, if x < 5 { 0 } else { 255 }]);
        assert_eq!(
            analyze(&cutout),
            Traits {
                grey: true,
                alpha: Alpha::Binary
            }
        );
        let smooth = Image::from_fn(16, 16, |x, y| [200, 10, 10, ((x + y) * 8) as u8]);
        assert_eq!(analyze(&smooth).alpha, Alpha::Smooth);
    }

    #[test]
    fn format_choices() {
        let t = |grey, alpha| Traits { grey, alpha };
        assert_eq!(
            choose_format(Role::World, t(false, Alpha::Opaque)),
            TexFormat::Cmpr
        );
        assert_eq!(
            choose_format(Role::Prop, t(false, Alpha::Binary)),
            TexFormat::Cmpr
        );
        assert_eq!(
            choose_format(Role::Effect, t(false, Alpha::Smooth)),
            TexFormat::Rgb5a3
        );
        assert_eq!(
            choose_format(Role::Mask, t(true, Alpha::Opaque)),
            TexFormat::I4
        );
        assert_eq!(
            choose_format(Role::Ui, t(true, Alpha::Smooth)),
            TexFormat::IA8
        );
        assert_eq!(
            choose_format(Role::Ui, t(false, Alpha::Opaque)),
            TexFormat::Rgb565
        );
        assert_eq!(
            choose_format(Role::Viewmodel, t(true, Alpha::Opaque)),
            TexFormat::I8
        );
    }

    #[test]
    fn convert_world_texture() {
        let src = Image::from_fn(1024, 1024, |x, y| [(x / 4) as u8, (y / 4) as u8, 128, 255]);
        let c = convert(&src, Role::World);
        assert_eq!((c.format, c.width, c.height), (TexFormat::Cmpr, 256, 256));
        // 256 -> 8: six levels; size matches the format maths.
        assert_eq!(c.levels, 6);
        assert_eq!(c.data.len(), TexFormat::Cmpr.chain_size(256, 256, 6));
        let l0 = c.decode_level(0).unwrap();
        assert_eq!((l0.width, l0.height), (256, 256));
        let l5 = c.decode_level(5).unwrap();
        assert_eq!((l5.width, l5.height), (8, 8));
        assert!(c.decode_level(6).is_none());
    }

    #[test]
    fn ui_has_no_mips_and_is_deterministic() {
        let src = Image::from_fn(300, 100, |x, y| [x as u8, y as u8, 7, 255]);
        let a = convert(&src, Role::Ui);
        assert_eq!((a.levels, a.width, a.height), (1, 256, 128));
        assert_eq!(a, convert(&src, Role::Ui));
    }

    #[test]
    fn role_names_roundtrip() {
        for r in Role::ALL {
            assert_eq!(Role::parse(r.name()), Some(r));
        }
        assert_eq!(Role::parse("WORLD"), Some(Role::World));
        assert_eq!(Role::parse("nope"), None);
    }
}

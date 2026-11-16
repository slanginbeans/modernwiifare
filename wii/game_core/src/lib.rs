//! The hardware-free half of the Wii game: everything here is plain Rust, tested on the host, and drives the GPU only
//! through the [`gfx::Gfx`] trait (implemented over the C shim in `wii/game`, and by a recorder in tests).
//!
//! - [`camera`]: IW4 (Z-up) camera to GX view matrices, frustum test.
//! - [`world`]: a loaded pack's textures, materials and batches, and the per-frame draw order.
//! - [`sky`]: skybox face quads in D3D cube orientation.
//! - [`fly`]: free-fly camera on the GameCube pad.
//! - [`collision`]: IW4L traces over the pack's `CLIP` section.
//! - [`player`]: pad -> `UserCmd` -> IW4L `pmove`.
//! - [`hud`]: screen-space rectangles and a 3×5 debug font.
//! - [`gun`]: IW4L weapon state machine + hitscan bullets against the map.
//! - [`damage`]: range falloff, hit locations, player hit volumes.
//! - [`nav`], [`bot`], [`arena`]: walk graph, bot brains, free-for-all match against bots.
//! - [`loadout`]: Create-a-Class rules over the converted class catalog.
//! - [`menu`]: pad-driven menu lists.
//! - [`weapon_sound`]: which alias a weapon event or a first-person notetrack plays.
#![no_std]

extern crate alloc;

pub mod arena;
pub mod auto_distance;
pub mod autoplay;
pub mod bot;
pub mod camera;
pub mod character;
pub mod collision;
pub mod controls;
pub mod damage;
pub mod debug;
pub mod destructible;
pub mod dom;
pub mod fly;
pub mod fx;
pub mod gfx;
pub mod glass;
pub mod grenade;
pub mod gun;
pub mod hud;
pub mod hud_fx;
pub mod hud_mw2;
pub mod item;
pub mod killcam;
pub mod killcam_hud;
pub mod killstreaks;
pub mod life;
pub mod light;
pub mod loadout;
pub mod lock;
pub mod look;
pub mod mantle_path;
pub mod marks;
pub mod menu;
pub mod minimap;
pub mod nav;
pub mod options;
pub mod perks;
pub mod player;
pub mod shield;
pub mod skin_kernels;
pub mod sky;
pub mod smoke;
pub mod telemetry;
pub mod tracer;
pub mod ui_page;
pub mod ui_text;
pub mod view_kick;
pub mod viewmodel;
pub mod weapon_anim;
pub mod weapon_sound;
pub mod weapon_view;
pub mod world;

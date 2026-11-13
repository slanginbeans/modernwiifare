//! Spike E: IW4L's no_std gameplay/format crates on the Wii (big-endian PPC32).
//!
//! 1. Re-exports every no_std IW4L crate, so `cargo build --target powerpc-unknown-linux-gnu`
//!    proves they all compile for the Wii.
//! 2. [`movement_trace`] runs IW4L's real `pmove` + `trace_box` collision over a scripted input
//!    sequence in a synthetic test room and hashes the player state every tick. The same hash on
//!    the host (x86) and on the Wii means IW4L's movement is bit-for-bit deterministic across both.
//!
//! The room and all tuning values here are our own placeholders, not game data. Real tuning comes
//! from the user's weapon defs and dvars via the converter.
#![no_std]

pub use anim_iw4;
pub use asset_iw4;
pub use clipmap_iw4;
pub use dpvs_iw4;
pub use fastfile_iw4;
pub use fx_iw4;
pub use gamemode_iw4;
pub use hud_iw4;
pub use input_iw4;
pub use killcam_iw4;
pub use lighting_iw4;
pub use math_iw4;
pub use movement_iw4;
pub use playerstate_iw4;
pub use structured_data_iw4;
pub use trace_iw4;
pub use weapon_iw4;

mod movement_trace;
pub use movement_trace::{MovementTrace, REFERENCE_HASH, TRACE_TICKS, movement_trace};

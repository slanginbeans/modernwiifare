//! Phase 0 spikes, linked into two .dol files by `wii/Makefile`:
//! - `spike_a_gx.dol`: toolchain self-tests, then a GX scene driven by the GameCube pad.
//! - `spike_b_net.dol`: system-link discovery over the USB LAN adapter.
#![no_std]

extern crate alloc;

pub mod ffi;
pub mod rt;
mod spike_a;
mod spike_b;

#[unsafe(no_mangle)]
pub extern "C" fn mwf_spike_gx_main() -> i32 {
    spike_a::run()
}

#[unsafe(no_mangle)]
pub extern "C" fn mwf_spike_net_main() -> i32 {
    spike_b::run()
}

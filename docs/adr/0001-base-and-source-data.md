# ADR 0001: Build on IW4L; convert from PC (IW4x) data; use the 360 copy as reference

Status: **accepted** (2026-11-13)

## Decision
- **Code base:** IW4L (Rust, Apache-2.0), consumed as pinned upstream crates. We do not use the 360 static recomp or any decompilation as a code base.
- **Converter input:** the user's **PC MP install (IW4x)**. IW4L's readers target PC fastfiles.
- **Reference build:** the user's **Xbox 360 disc** under mw2-recompiled, to check feel. Only its behaviour is observed. No code is taken from it, which keeps us Apache-2.0 clean.

## Why
- IW4L contains no Activision code. It is written in the same language we will ship, and most of its gameplay core is already `no_std`.
- Read-only audit of the IW4L snapshot at `acc57a5` (2026-10-16): 18 crates are `#![no_std]` and have **no build.rs, no `unsafe`, and no native-endian byte handling**. The crates are math, trace, playerstate, movement, clipmap, weapon, gamemode, entity, anim, input, killcam, hud, dpvs, fx, lighting, asset, fastfile and structured_data. That makes them strong candidates to compile for big-endian 32-bit PowerPC unchanged. *This has not been compiled for PowerPC yet.* That is Phase 0 Spike E.
- The Bevy/wgpu/tokio/quinn layers (sim orchestration, renderer, net, audio, UI) cannot run on a Wii. We replace those.

## What we reuse vs replace
| IW4L layer | Wii runtime | Host converter |
|---|---|---|
| `*_iw4` no_std gameplay crates | **reuse** | reuse |
| `fastfile_iw4`, `asset_iw4`, `clipmap_iw4`, `dpvs_iw4` | not needed at runtime | **reuse** |
| `asset_*` (Bevy-coupled extraction) | no | **reuse on host** (Bevy is fine on PC) |
| `sim` (bevy_ecs world, `sim::step`, GSC runtime) | **port**: lift `step` + `script` out of bevy_ecs into a fork crate `wii_sim` | n/a |
| `render_*` (wgpu) | **replace** with a GX backend | reuse `render_frame`/`render_material` concepts |
| `net` (quinn/tokio/QUIC master) | **replace** with UDP system link | n/a |
| `audio` (cpal/symphonia) | **replace** with ASND/AESND | reuse `asset_audio` for decoding |
| `ui`, `hud`, `console`, `bootstrap` | **replace** with lean Wii frontend; reuse `hud_iw4` logic | n/a |

## Consequences
- The Wii build uses **nightly Rust + `build-std`** with a custom PowerPC EABI target, linking libogc through `ogc-sys`/`ogc-rs` (rust-wii). IW4L itself stays on stable. Our host crates stay on stable too.
- We track upstream IW4L. Patches that make crates more portable go upstream first, and we carry a fork only when upstream declines. See the `iw4l-upstream` skill.
- ~~To check later: whether IW4L's install detection finds an **IW4x** install layout and its extra `iw4x_*` zones.~~
  Checked in Spike D (2026-11-13): yes. IW4x keeps the retail x86 zones under `zone/iw4x/x86/` and rewrites
  `zone/english` as x64. IW4L reads both. Its zone search can prefer a same-named `usermaps/` map, so the converter
  picks paths itself. See `docs/spikes/results.md`.

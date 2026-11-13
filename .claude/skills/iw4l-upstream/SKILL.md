---
name: iw4l-upstream
description: How modernwiifare consumes, ports and patches IW4L (the Apache-2.0 Rust MW2 runtime) — pinning, the no_std boundary, lifting sim::step out of bevy_ecs, building IW4L crates for the Wii's PowerPC target, and upstreaming fixes. Use whenever adding or bumping an IW4L dependency, porting an IW4L crate, or debugging a difference between IW4L behaviour and the Wii build.
---

# Working with IW4L upstream

Upstream: https://github.com/vladtrc/iw4L (Apache-2.0, Rust 2024 edition, stable toolchain).
The ADR 0001 audit was done at `acc57a5`.

## Dependency policy
- Consume IW4L crates as **git dependencies pinned to a commit** in the workspace `Cargo.toml`
  (`rev = "..."`), never a branch. Bump deliberately, one PR per bump, with the parity tests re-run.
- When a crate needs a change: (1) open the patch upstream, (2) meanwhile point a `[patch]` entry at our fork branch,
  (3) delete the patch once upstream merges. Every live patch is listed in `docs/upstream-patches.md`.
- Keep the IW4L `NOTICE` attribution. Copied files keep their headers and are listed in `docs/references.md`.

## The no_std boundary
Wii-side crates are `#![no_std]` + `alloc`. The upstream crates already in that set are:
math, trace, playerstate, movement, clipmap, weapon, gamemode, entity, anim, input, killcam, hud, dpvs, fx, lighting,
asset, fastfile and structured_data (all `*_iw4`).
- Before using a new upstream crate on the Wii, check it for `std`, `bevy`, `glam` with std features, `tokio`, `rayon`
  and threads. Anything there stays host-only.
- `f64` is present in movement/weapon/clipmap. Broadway's FPU does scalar double natively, so **do not** convert to f32
  for speed without a parity trace proving no feel change.
- Build-check the set for big-endian 32-bit (`powerpc-unknown-linux-gnu`, `cargo check`) in CI. This is the cheap
  smoke test before the real Wii target.

## Lifting the simulation
IW4L funnels all authoritative change through `sim::step(world, tick, input, msec, reason)`, which server, prediction
and replay all share. Our `wii_sim` crate keeps **that exact contract** (TickInput → TickResult) but stores the world in
plain arenas instead of bevy_ecs. Rules:
1. Port system by system, keeping upstream function names so diffs stay readable.
2. For every ported system, a host test runs IW4L's original and ours on the same inputs and compares `TickResult` hashes.
3. The GSC runtime (`sim::script`) moves in the same way. Its IR is produced on the host by the converter (ADR 0002).

## Wii target build
- Per ADR 0006, IW4L crates compile for `powerpc-unknown-linux-gnu` on stable as part of our Wii staticlib. No nightly is needed
  unless the ADR 0006 fallback is taken.

# Upstream IW4L issues and patches

Pinned rev: `acc57a5c9dd027db8eb5ef17c792fa69f059daa1`. See the `iw4l-upstream` skill for the policy.

## Open

### `asset_transport::find_zone_file` can pick a usermap over the stock zone
- On an IW4x install, `find_zone_file(root, "mp_rust")` returned `usermaps/pcrust/mp_rust.ff` instead of
  `zone/english/mp_rust.ff`: the first IW4 file of that name found by the directory walk wins.
- Proposed upstream patch: rank `zone/<lang>/` (and `zone/iw4x/x86/<lang>/`) above `usermaps/` and `mods/`, or
  return all candidates.
- Workaround here: `converter/src/layout.rs` picks `zone/english/<map>.ff` itself. No patch carried.

### `entity_iw4` pulls `std` through `asset_core`
- `entity_iw4` is `#![no_std]` but depends on `asset_core`, which is not. `entity_iw4` uses it only to re-export
  `asset_core::GlassPaneBasis` (`entity_iw4/src/glass.rs`).
- `asset_core`'s std use is small: `BTreeMap`/`BTreeSet` (available in `alloc`), plus `std::path::Path` and `std::error::Error`.
- Proposed upstream patch: make `asset_core` `no_std` + `alloc` behind a default `std` feature, or move `GlassPaneBasis`
  into a no_std crate.
- Until then, `entity_iw4` is left out of the Wii build (`wii/iw4l_probe`). No patch carried yet.
- On IW4L's `major-overhaul` branch (below) `entity_iw4` depends on `libm` only, which would settle this.

## Watching: the `major-overhaul` branch
Reviewed at `5856e16` (2026-11-10): five commits past our pin, mostly one "Major overhaul" commit (1,441 files). Still
Apache-2.0, and the crates we use are still `no_std`. Not on IW4L's `master`; we stay on the pin. What would change
for us if we moved:
- **Recoil kick doubles.** `weapon_iw4::kick` `CAMERA_ASSET_AMPLITUDE` 0.020 → 0.040, `MODEL_ASSET_AMPLITUDE` 0.015 →
  0.030, unexplained. `weapon_fire_recoil` scales linearly with them, so the debug menu's RECOIL KICK = IW4L NEW (X2)
  (`DebugSettings::kick_x2`, `Gun::kick_scale`) plays the branch's kick on the pinned crate, for a side-by-side
  comparison with the user's copy (`feel-parity`). Adopt only if it matches.
- **Stepping up changes.** `movement_iw4::contact_solver::traverse` now tries smaller rises when a low lintel blocks
  the full step, and rejects endpoints that start inside solid. Likely a real fix, but it is movement: a parity trace
  first.
- **New, usable on the Wii:** `marks_iw4` (IW4's impact-mark pool, clipping onto world and model surfaces, budgets;
  `no_std`, no dependencies), so it can be pinned alone for bullet holes and scorch marks.
- **New, not for the Wii:** `net` (QUIC on tokio and rustls), `bots` (host-only, `std`).

# Upstream IW4L issues and patches

Pinned rev: `acc57a5c9dd027db8eb5ef17c792fa69f059daa1`. See the `iw4l-upstream` skill for the policy.

## Open

### `entity_iw4` pulls `std` through `asset_core`
- `entity_iw4` is `#![no_std]` but depends on `asset_core`, which is not. `entity_iw4` uses it only to re-export
  `asset_core::GlassPaneBasis` (`entity_iw4/src/glass.rs`).
- `asset_core`'s std use is small: `BTreeMap`/`BTreeSet` (available in `alloc`), plus `std::path::Path` and `std::error::Error`.
- Proposed upstream patch: make `asset_core` `no_std` + `alloc` behind a default `std` feature, or move `GlassPaneBasis`
  into a no_std crate.
- Until then, `entity_iw4` is left out of the Wii build (`wii/iw4l_probe`). No patch carried yet.

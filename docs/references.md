# External references

Record **every** project we read, borrow from, or test against, with its license. See the `asset-hygiene` skill.

| Project | What it is | License | How we use it |
|---|---|---|---|
| [IW4L](https://github.com/vladtrc/iw4L) | From-scratch Rust runtime that reads the user's MW2 MP install: FastFile readers, sim, GSC runtime, netcode | Apache-2.0 | **Code base.** Pinned crates plus ported `sim` (ADR 0001). Audited at `acc57a5` |
| [ogc-rs / ogc-sys](https://github.com/rust-wii/ogc-rs) | Rust bindings for libogc | see repo | Wii platform layer |
| [libogc](https://github.com/devkitPro/libogc) / devkitPPC | Wii homebrew SDK and toolchain | see repo | Platform, linker |
| [mw2-recompiled](https://github.com/PaulCombal/mw2-recompiled) | 360 static recomp (disc 1.0.557 + TU6) for PC. MP and system link playable | GPL-3.0-only | **Behavioural reference only, no code** (ADR 0004) |
| [IW4x](https://github.com/iw4x/iw4x-client) | PC MW2 MP client | see repo | The user's PC data source and a reference build |
| [COD6-NX](https://github.com/GreenieSparkles/COD6-NX) | 360 recomp → Switch | GPL-3.0 / BSD-3 parts | Lessons only |
| OpenAssetTools, ZoneTool, [codresearch.dev](https://codresearch.dev) | IW asset tooling and wiki | check per repo | Format knowledge (IW4L already credits OAT) |

**Excluded:** leaked IW source, private decompilations (e.g. momo5502's AI-assisted MW2 decomp), repos of dumped game scripts or assets.

# Files copied or adapted from IW4L
| Our file | Adapted from (IW4L `acc57a5`, Apache-2.0) | What |
|---|---|---|
| `converter/src/pixel_decode.rs` | `crates/asset_material/src/material_images/decode.rs` | D3D and IWI format tables, IWI v8 top-mip range, BCn decode through `bcdec_rs`. Carried because `asset_material` pulls in Bevy |

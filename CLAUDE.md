# modernwiifare: project rules

A non-commercial fan demake of the multiplayer side of MW2 (2009) for the Nintendo Wii, with 4-player system link over USB LAN, played on the GameCube controller.
It is built on **IW4L** (Apache-2.0 Rust runtime) and on data the **user converts from their own copy** of the game.
The repository holds code, tools and docs only. License: Apache-2.0.

## Hard rules (never break)
1. **No game data in git.** Never commit anything extracted from, converted from, or derived from the
   retail game: fastfiles (`.ff`), `.iwd`, `.xex`, ISOs, GSC scripts or their compiled IR, weapon/attachment text, string
   tables, textures, models, sounds, converted Wii packs, captured parity traces, or screenshots of them. Run
   `tools/check_no_assets.sh` before every commit (see the `asset-hygiene` skill).
2. **No leaked, private or GPL source.** No leaked Activision/IW source, no private decompilations, and no code from
   mw2-recompiled (GPL-3.0; behavioural reference only, ADR 0004).
3. **Attribution.** Keep IW4L's NOTICE. List every borrowed file and live upstream patch in `docs/`.
4. **Multiplayer only.** Do not port campaign or Spec Ops content.
5. **Feel changes need evidence.** Any change to movement, weapon or timing code needs a parity trace (`feel-parity` skill).

## Decisions (docs/adr/)
0001 IW4L base, PC (IW4x) data in, 360 copy as reference · 0002 user's GSC via IW4L's runtime ·
0003 4 players · 0004 Apache-2.0 · 0005 GameCube pad first, USB pads stretch, wired LAN ·
0006 stable Rust staticlib (powerpc-unknown-linux-gnu) + C shim, linked by devkitPPC

## Planned layout
- `converter/`: host Rust tool `mwf-convert` (stable). Uses IW4L reader/asset crates and writes to `out/` (git-ignored).
  `inventory <game folder>`: per-zone sizes and asset counts (Spike D). Reports must stay content-free: relative paths,
  sizes and counts only. On Windows, `inventory.bat` runs it with a double-click.
  `map <game folder> [--map <zone>]`: walks one map zone with IW4L's loader, per-type stream/resident/payload bytes
  (Spike D). Prefers the retail x86 zone in `zone/iw4x/x86/english/` over IW4x's x64 rewrite in `zone/english/`.
  `texture <png> --role <role>`: one PNG to a Wii `.gxt` + `_preview.png` (output under `out/`).
  `textures <game folder> [--map <zone>]`: every GfxImage of a map zone (in-zone or IWI) to `.gxt` under
  `out/textures/<zone>/` with a content-free `report.json`. Role and alpha come from the materials that use each image
  (`material_links.rs`); the name is only a fallback. See `docs/textures.md`.
- `wii/`: Wii runtime (stable Rust, no_std + alloc, ADR 0006), builds `.dol`s with `make -C wii`.
  - `platform/`: the C shim over libogc (`shim.h` is the whole API Rust sees; keep `spikes/src/ffi.rs` in sync).
  - `syslink/`: system-link protocol (no_std, host-tested). `spikes/`: Phase 0 spike programs.
  - Later: `wii_sim` (IW4L `sim::step` lifted off bevy_ecs) · `gx_render` · `snd` · `front` (menus/HUD).
- `shared/`: code used by both the converter and the Wii runtime (no_std).
  - `gx_texture/`: GX formats (I4..RGBA8, CMPR) encode/decode, gamma-correct resize + mips, per-role format policy,
    `.gxt` container. CMPR blends 5/8+3/8 like Dolphin (`cmpr::BLEND_NEAR`); confirm on hardware.
    `convert_with(.., Options { cutout })`: alpha-tested textures get a ≥256 cap and keep their coverage per mip.
    Smooth-alpha colour textures (not UI) are split: CMPR colour + `alpha_plane` (I4); `.gxt` flag bit 0. The
    renderer must sample both (colour from one unit, alpha from the I4 unit). See `docs/textures.md`.
  - Later: the pack format.
- `tools/`: asset guard, budget report, Dolphin launch.
- `docs/`: ADRs, budgets, deviations, references, upstream patches.

## Build and test
- Host: `cargo test`, `cargo clippy --all-targets`, `cargo fmt --all --check`.
- Wii Rust half: `cargo build --release -p mwf_spikes --target powerpc-unknown-linux-gnu`.
- PPC32 tests under qemu (750CL): `cargo test -p iw4l_probe --target powerpc-unknown-linux-gnu`
  (needs `qemu-user gcc-powerpc-linux-gnu libc6-dev-powerpc-cross`).
- Full `.dol`: `make -C wii hbc` (needs devkitPro). In the cloud session devkitPro is not reachable, so the `.dol` builds run in
  GitHub Actions (`wii` job, `devkitpro/devkitppc` image) and come out as a downloadable artifact.

## Skills
`iw4l-upstream` · `wii-platform` · `iw4-asset-pipeline` · `systemlink-netcode` · `feel-parity` · `asset-hygiene`

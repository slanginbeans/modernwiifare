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
0001 IW4L base, PC (IW4x) data in, 360 copy as reference · 0002 superseded by 0007 ·
0003 4 players · 0004 Apache-2.0 · 0005 GameCube pad first, USB pads stretch, wired LAN ·
0006 stable Rust staticlib (powerpc-unknown-linux-gnu) + C shim, linked by devkitPPC ·
0007 native gameplay in `wii/game_core` on IW4L's no_std crates; MW2's scripts are read for values at convert time,
never run (supersedes 0002)

## Layout
- `converter/`: host Rust tool `mwf-convert` (stable). Uses IW4L reader/asset crates and writes to `out/` (git-ignored).
  `inventory <game folder>`: per-zone sizes and asset counts (Spike D). Reports must stay content-free: relative paths,
  sizes and counts only. On Windows, `inventory.bat` runs it with a double-click.
  `map <game folder> [--map <zone>]`: walks one map zone with IW4L's loader, per-type stream/resident/payload bytes
  (Spike D). Prefers the retail x86 zone in `zone/iw4x/x86/english/` over IW4x's x64 rewrite in `zone/english/`.
  `texture <png> --role <role>`: one PNG to a Wii `.gxt` + `_preview.png` (output under `out/`).
  `pack <game folder> --map <zone>|all`: map packs `out/packs/<map>.wpk`; `all` also writes `common.wpk` (weapons,
  classes, rules, menus, characters) and `views/<weapon>.wpk`. `common`: those alone. `--slim-views` (or
  `MWF_SLIM_VIEWS=1`): smaller weapon files, off by default (`docs/pack.md`, "Slim weapon files").
  `--tiled-uv` / `--small-dl` / `--small-props` (`MWF_TILED_UV`, `MWF_SMALL_DL`, `MWF_SMALL_PROPS`): smaller map packs, off by default (`docs/pack.md`,
  "Smaller map packs"; the Wii side is `mwf_world_desc` in the shim, called before every world draw).
  `--adpcm` (`MWF_ADPCM`): sounds as 4-bit DSP ADPCM (`shared/wpk/src/adpcm.rs`), decoded while they play by the shim's
  ASND voice callback; off by default (`docs/pack.md`, "ADPCM sounds"). `--slim-anims` (`MWF_SLIM_ANIMS`): keyframed
  player animations (`wpk::player_anim::slim`), off by default. Pack reports: `<map>.json` has a texture
  breakdown (`tex_report.rs`); `common.json` covers `common.wpk` and the weapon files. `rules.rs` reads values from the
  user's scripts (ADR 0007). `preview <pack.wpk>`: software render of a pack (also the viewmodel and characters).
  `menu-preview`: menus. `report <log>...`: summarises the Wii's SD-card match logs (`report.bat`).
  `textures <game folder> [--map <zone>]`: every GfxImage of a map zone (in-zone or IWI) to `.gxt` under
  `out/textures/<zone>/` with a content-free `report.json`. Role and alpha come from the materials that use each image
  (`material_links.rs`); the name is only a fallback. Each texture also gets a load-by-need group (match / weapon /
  catalog / menu), listed with its file in `index.json` (git-ignored; it names assets). See `docs/textures.md`.
  Packs carry a stamp (`converter/src/stamp.rs`): bump `REVISION` whenever the converter's output changes, or
  `build.bat` / `pack --map all` keep the old packs.
- `wii/`: Wii runtime (stable Rust, no_std + alloc, ADR 0006), builds `.dol`s with `make -C wii`.
  - `platform/`: the C shim over libogc (`shim.h` is the whole API Rust sees; keep `spikes/src/ffi.rs` in sync).
  - `game_core/`: the game, `no_std` and host-tested: camera, world draw order, collision, player (IW4L pmove), gun
    (IW4L weapon state machine), damage, life, bots and walk graph, match loop and modes (FFA, TDM, Domination),
    grenades, perks, killstreaks, minimap, menus and UI pages, viewmodel animation, characters, telemetry.
    Draws only through the `gfx::Gfx` trait.
  - `game/`: the Wii staticlib for `mwf_game.dol`: `Gfx` over the shim, sound (ASND), file loading, main loop.
    See `docs/runtime.md`.
  - `syslink/`: system-link protocol (no_std, host-tested, Phase 5). `spikes/`: Phase 0 spike programs.
    `iw4l_probe/`: the IW4L no_std crate set and the cross-arch movement trace.
- `shared/`: code used by both the converter and the Wii runtime (no_std).
  - `gx_texture/`: GX formats (I4..RGBA8, CMPR) encode/decode, gamma-correct resize + mips, per-role format policy,
    `.gxt` container. CMPR blends 5/8+3/8 like Dolphin (`cmpr::BLEND_NEAR`); confirm on hardware.
    `convert_with(.., Options { cutout })`: alpha-tested textures get a ≥256 cap and keep their coverage per mip.
    Smooth-alpha colour textures (not UI) are split: CMPR colour + `alpha_plane` (I4); `.gxt` flag bit 0. The
    renderer must sample both (colour from one unit, alpha from the I4 unit). See `docs/textures.md`.
    `Role::Minimap`: the HUD minimap (512, no mips, split when its alpha is smooth). The converter finds it through
    the map script's `setupMiniMap("...")` call in the zone's rawfiles.
    `Role::Sky` has no mips. A sky cube map becomes six face textures `<name>_px/_nx/_py/_ny/_pz/_nz` (D3D order).
  - `wpk/`: the `.wpk` pack format (zero-copy, big-endian, 32-byte aligned sections), written by the converter and read
    in place on the Wii. See `docs/pack.md`.
- `tools/`: asset guard, Windows/Docker build helpers (`winbuild.sh`, `docker-build.sh`, `mwf-convert.ps1`).
- `docs/`: ADRs, `runtime.md`, `pack.md`, `textures.md`, `perf.md`, `gaps.md`, `testing.md`, references, upstream patches.
- Root: `build.bat` (update, build, convert, package `dist\apps\mwf_game`; switches combine: `noupdate nodata
  nopause clean showargs`; env `MWF_SLIM_VIEWS`, `MWF_CONVERT_FLAGS`, `MWF_ASSUME_YES`, `MWF_GAME`, `MWF_DEVKITPRO`),
  `builder.bat` (a window over build.bat: `tools/builder-gui.ps1`, WinForms; `-SelfTest` runs in CI; keep it ASCII,
  Windows PowerShell 5.1 reads BOM-less files as ANSI), `inventory.bat`, `report.bat`.

## Build and test
- Host: `cargo test`, `cargo clippy --all-targets`, `cargo fmt --all --check`.
- Wii Rust half: `cargo build --release -p mwf_game -p mwf_spikes --target powerpc-unknown-linux-gnu`
  (`game_core` is plain `no_std` Rust: `cargo test -p game_core` on the host).
- PPC32 tests under qemu (750CL): `cargo test -p iw4l_probe --target powerpc-unknown-linux-gnu`
  (needs `qemu-user gcc-powerpc-linux-gnu libc6-dev-powerpc-cross`).
- Full `.dol`: `make -C wii package` (needs devkitPro) → `dist/apps/mwf_game` plus the spikes; on Windows `build.bat`. In the cloud session devkitPro is not reachable, so the `.dol` builds run in
  GitHub Actions (`wii` job, `devkitpro/devkitppc` image) and come out as a downloadable artifact.

## Skills
`iw4l-upstream` · `wii-platform` · `iw4-asset-pipeline` · `systemlink-netcode` · `feel-parity` · `asset-hygiene`

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
0003 4 players · 0004 Apache-2.0 · 0005 GameCube pad first, USB pads stretch, wired LAN

## Planned layout
- `converter/`: host Rust tool (stable). Uses IW4L reader/asset crates and writes Wii packs to `out/` (git-ignored).
- `wii/`: Wii runtime (Rust nightly, no_std + alloc, `ogc-rs`/libogc), builds a `.dol`.
  - `wii_sim`: IW4L `sim::step` lifted off bevy_ecs · `gx_render` · `syslink` · `snd` · `front` (menus/HUD).
- `shared/`: pack format definitions used by both the converter and the Wii runtime.
- `tools/`: asset guard, budget report, Dolphin launch.
- `docs/`: ADRs, budgets, deviations, references, upstream patches.

## Skills
`iw4l-upstream` · `wii-platform` · `iw4-asset-pipeline` · `systemlink-netcode` · `feel-parity` · `asset-hygiene`

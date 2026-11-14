# modernwiifare roadmap

**Goal:** MW2 (2009) multiplayer running natively on the Wii, played on the **GameCube controller**, built on **IW4L** and on
data each user converts from their own copy. Assets are downsized to fit; feel matches the reference builds.
4-player **system link over USB LAN** comes once the game itself plays well.

**Priority order:** get the game running and feeling right on one Wii first (a local match against bots).
Networking comes after that.

**Where we are (2026-11-14):** a single-Wii game exists in code. All 16 maps convert and fit MEM2. FFA, TDM and
Domination run against bots, with MW2's guns, perks, grenades, equipment, some killstreaks and MW2-style menus. It has
been built and tested on the host and in CI, **but not yet run on a Wii**. The next step is the first hardware test
round (`docs/testing.md`). Detailed gap list: `docs/gaps.md`. Performance plan: `docs/perf.md`.

Decisions: [ADR 0001](docs/adr/0001-base-and-source-data.md) IW4L base, PC/IW4x data, 360 reference ·
[0002](docs/adr/0002-gameplay-scripts.md) *(superseded by 0007)* ·
[0003](docs/adr/0003-player-count.md) 4 players · [0004](docs/adr/0004-license.md) Apache-2.0 ·
[0005](docs/adr/0005-input-and-hardware.md) GameCube pad first, USB pads stretch, wired LAN ·
[0006](docs/adr/0006-wii-rust-build.md) stable Rust staticlib + C shim, linked by devkitPPC ·
[0007](docs/adr/0007-native-gameplay.md) native gameplay on IW4L's crates, script values read as data

## Architecture

```
  User's PC install (IW4x) ──► converter `mwf-convert` (host Rust; build.bat runs it)
                                 ├─ IW4L fastfile/asset crates read the zones (retail x86 zones preferred)
                                 ├─ textures → GX formats (shared/gx_texture); world, models, collision, visibility,
                                 │  light grid, walk graph, characters, sounds → packs (shared/wpk)
                                 ├─ values from the user's scripts and tables (rules, weapons, classes, menus)
                                 └─ out/packs: <map>.wpk ×16, common.wpk, views/<weapon>.wpk   (never committed)
                                               │ SD card: sd:/apps/mwf_game/
  Wii runtime mwf_game.dol (stable Rust, no_std + alloc) ◄─────────┘
   ├─ wii/game_core : the game, host-tested. IW4L no_std crates (pmove, weapon state machine, collision, HUD maths)
   │                  + match loop, bots, damage, grenades, perks, killstreaks, modes, menus, telemetry
   ├─ wii/game      : the Wii side: Gfx over the C shim, sound (ASND), file loading, main loop
   ├─ wii/platform  : C shim over libogc (GX world / UI / skinned draws, MEM2 allocator, SD files, pad, sound)
   └─ wii/syslink   : UDP LAN discovery protocol (Phase 5)

  Reference builds for feel: user's 360 disc under mw2-recompiled · user's PC copy under IW4x · IW4L on host
```

**Spike E result (2026-11-13):** 17 IW4L crates build std-free for the Wii's CPU. IW4L's real `pmove` + collision gives a
**bit-identical** 1,200-tick movement trace on x86-64 and on an emulated PowerPC 750CL (hash `98b5d201ad2741d5`, checked in CI).
`entity_iw4` is excluded for now: it pulls in std through `asset_core` (see `docs/upstream-patches.md`).

---

## Phase 0: Foundations and risk spikes
- [x] ADRs 0001–0007
- [x] Workspace, pinned IW4L git deps, CI (asset guard, fmt/clippy/tests, PPC32 build, qemu 750CL tests, `.dol` build,
      Windows converter + `.bat` checks)
- [ ] **Spike A, toolchain:** Rust `no_std` `.dol` (self-tests + GX scene on the GameCube pad). *Builds in CI, awaiting a
      hardware run. Superseded in practice by the first `mwf_game` test round, which covers the same ground.*
- [ ] **Spike C, memory:** usable MEM1/MEM2 on hardware. *The game logs free MEM1/MEM2 at start and after loading
      (`docs/testing.md`). Write `docs/budgets.md` from the first round.*
- [x] **Spike D, data:** converter reads the user's IW4x install via IW4L (`inventory`, `map`). *Done 2026-11-13
      (`docs/spikes/results.md`). IW4x moves the retail x86 zones to `zone/iw4x/x86/` and rewrites `zone/english` as x64.*
- [x] **Spike E, portability:** IW4L crates build for the Wii. Movement trace is bit-identical x86 vs PPC 750CL (qemu).
      *Remaining: same hash on a real Wii (shown on the Spike A screen).*
- [x] ~~**Spike F, scripts**~~: dropped with ADR 0007 (no GSC runtime on the Wii).
- **Exit:** A and C on hardware.

## Phase 1: Converter
- [x] Pack format `shared/wpk` (zero-copy, big-endian, versioned, hashed, 32-byte aligned sections). See `docs/pack.md`.
- [x] Textures (`shared/gx_texture`, `docs/textures.md`): all GX formats incl. CMPR, mips, roles and alpha from materials,
      cut-out coverage per mip, smooth alpha as CMPR + I4 plane, minimap, sky faces, load-by-need groups.
- [x] World: TEV recipe table, triangle strips, lightmaps, skybox, spawns, objectives.
- [x] Collision (`CLIP`, unsimplified), visibility cells and portals (IW4 dpvs), walk graph for bots.
- [x] Static models: instanced, light-grid lit, IW4 LODs.
- [x] Fixed-point vertices (pack v3): **all 16 maps fit MEM2, 18.9–38.5 MiB**.
- [x] `common.wpk` (weapons lossless, classes, rules, menus, fonts) and `views/<weapon>.wpk` (first-person model,
      animations, textures, sounds) for all 45 weapons.
- [x] Player characters (`CHAR`, `PANM`).
- [x] Incremental conversion: stamped packs, only what changed is rebuilt (`converter/src/stamp.rs`).
- [ ] Budget report that fails the build when over budget (today: `pack` prints MEM2 and warns above 44 MiB).

## Phase 2: Renderer and "walk the map"
- [x] GX world renderer: cells/portals, display lists, TEV recipes, sky, static models, viewmodel pass, skinned characters.
- [x] First-person movement with IW4L `movement_iw4` on converted collision; players block each other.
- [x] GameCube pad input: IW4L's gamepad look (dead zone, curve, turn acceleration), aim assist, ADS zoom, console-style
      crouch / prone, sprint from full stick (range learned from use).
- [ ] Fog and per-map draw distance (`LOD_SCALE`) as 60 fps levers, set from hardware numbers.
- **Exit:** walk the map at a stable frame rate **on real hardware**, with movement trace = IW4L host trace.

## Phase 3: Weapons and a local match ("the game works")
- [x] Weapons through IW4L's weapon state machine with each weapon's numbers: spread, recoil, ADS, switching, melee
      and lunge, shotgun pellets, bullet penetration.
- [x] First-person model: MW2's viewmodel anim tree, sway and bob; weapon sounds (fire, reload, notetracks) via ASND.
- [x] Damage: range falloff, hit locations, MW2's damage numbers; health regen and fall damage from the user's scripts.
- [x] Bots: walk graph, IW4L's bot motor and sight, A* routes, at IW4's 50 ms server step.
- [x] Match loop: FFA and TDM with the scripts' limits and spawn weighting, private-match Game Setup.
- [x] Test telemetry: per-match logs on the SD card, `report.bat` summaries (`docs/testing.md`).
- [ ] **First hardware test round** (the five scenarios in `docs/testing.md`).
- **Exit:** a full TDM match against bots on one Wii, with feel traces matching IW4L exactly and the 360 reference within tolerance.

## Phase 4: Game breadth (single Wii)
- [ ] Modes: ~~dm~~, ~~war~~, ~~dom~~ done; then sd, sab, koth, dd, ctf.
- [x] Create-a-Class with MW2's rules (everything unlocked), saved to SD; all 45 weapons; attachment combat numbers.
- [ ] Perks: Pro upgrades, perk bits, move speed, Stopping Power and Danger Close done. Not yet: Scavenger, Hardline,
      Cold-Blooded / Scrambler / Ninja against radar and sound, Last Stand, One Man Army, deathstreaks.
- [x] Equipment: frag, semtex, throwing knife, claymore, C4, flashbang, stun, smoke.
- [ ] Killstreaks: ~~UAV~~, ~~Counter-UAV~~, ~~EMP~~, ~~Tactical Nuke~~ done; then Care Package, Predator, Sentry,
      airstrikes, helicopters, AC-130. Each one gets a budget line.
- [x] MW2-style menus baked from the user's menus: main, class select, pause, Create-a-Class, scoreboard.
- [ ] Killcam and final killcam (reuse `killcam_iw4`); local progression saved to SD.
- [ ] Character detail: leg/torso blend, aim pitch, held weapon, per-class bodies, death animations.

## Phase 5: System link, 4 players
- [ ] Hardware run of Spike B (discovery code already in `wii/spikes`, built in CI).
- [ ] `syslink`: discovery, lobby, protocol/build/pack-hash handshake.
- [ ] Host-authoritative match over the LAN on `game_core`'s fixed-rate steps: usercmds, delta snapshots, client
      prediction, lag compensation (`systemlink-netcode` skill).
- **Exit:** a full 4-Wii TDM match with no desync or crash, plus the per-tick state-hash checker passing.

## Phase 6: Performance and content (ongoing)
- [ ] Hardware profiling from the logs: world vertex load, CPU/GPU pipelining, draw submit, viewmodel skinning,
      bots (`docs/perf.md`, `docs/gaps.md`). Target 60 fps; 30 fps render with 60 Hz steps as the per-map fallback.
- [ ] USB gamepad support (Spike G first: HID class, then per-device drivers) through the same input path.
- [ ] Optional DLC maps if present in the user's install.

## Phase 7: Source-only release
- [x] One-step build: `build.bat` builds, converts what changed, and fills `dist\apps\mwf_game` for the SD card.
- [ ] User docs (SD layout, Homebrew Channel, USB LAN setup, pack-hash mismatch help), vWii check.

---

## Top risks
1. **Untested on hardware.** Everything above is host- and CI-tested only. Known unknowns (`docs/runtime.md`): GX
   cull winding, sound playing from MEM2, the CMPR blend, trace and bot cost on Broadway.
2. **Frame rate.** Heavy maps draw ~3× a 60 fps geometry budget (`docs/perf.md`); five bots are estimated at 3–8 ms.
3. **Breadth by hand** (ADR 0007): each remaining mode and killstreak is ported natively. Mitigation: IW4L's crates
   for the mechanics, script values for the numbers, host tests per system.
4. **Upstream churn**: IW4L is young and moving fast. Mitigation: pinned revs, deliberate bumps, upstream patches first.
5. **Legal exposure**. Mitigation: no data or GPL/leaked code, non-commercial, source-only. *(This is not legal advice.)*

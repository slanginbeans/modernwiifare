# modernwiifare roadmap

**Goal:** MW2 (2009) multiplayer running natively on the Wii, played on the **GameCube controller**, built on **IW4L** and on
data each user converts from their own copy. Assets are downsized to fit; feel matches the reference builds.
4-player **system link over USB LAN** comes once the game itself plays well.

**Priority order:** get the game running and feeling right on one Wii first (a local match against bots).
Networking comes after that.

Decisions: [ADR 0001](docs/adr/0001-base-and-source-data.md) IW4L base, PC/IW4x data, 360 reference ·
[0002](docs/adr/0002-gameplay-scripts.md) user's GSC via IW4L runtime · [0003](docs/adr/0003-player-count.md) 4 players ·
[0004](docs/adr/0004-license.md) Apache-2.0 ·
[0005](docs/adr/0005-input-and-hardware.md) GameCube pad first, USB pads stretch, wired LAN ·
[0006](docs/adr/0006-wii-rust-build.md) stable Rust staticlib + C shim, linked by devkitPPC

## Architecture

```
  User's PC install (IW4x) ──► converter (host Rust, stable)
                                 ├─ IW4L fastfile/asset crates: read zones
                                 ├─ downsize: CMPR textures, quantized display lists, ADPCM, BVH collision
                                 ├─ GSC → IW4L script IR
                                 └─ out/packs/*.wpk + pack hash   (never committed)
                                               │ SD card
  Wii runtime (stable Rust, no_std+alloc, C shim over libogc) ◄──┘
   ├─ IW4L no_std crates, reused as-is: movement, weapon, clipmap, trace, playerstate, gamemode, anim, hud, fx...
   ├─ wii_sim   : IW4L `sim::step` + GSC runtime + bots, lifted off bevy_ecs
   ├─ gx_render : new GX/TEV backend           (replaces wgpu render_*)
   ├─ snd       : ASND/AESND                   (replaces cpal)
   ├─ front     : menus, class editor, match setup (replaces Bevy UI)
   └─ syslink   : UDP LAN discovery + snapshots (replaces quinn/tokio net; later phase)

  Reference builds for feel: user's 360 disc under mw2-recompiled · user's PC copy under IW4x · IW4L on host
```

**Spike E result (2026-11-13):** 17 IW4L crates build std-free for the Wii's CPU. IW4L's real `pmove` + collision gives a
**bit-identical** 1,200-tick movement trace on x86-64 and on an emulated PowerPC 750CL (hash `98b5d201ad2741d5`, checked in CI).
`entity_iw4` is excluded for now: it pulls in std through `asset_core` (see `docs/upstream-patches.md`).

---

## Phase 0: Foundations and risk spikes
- [x] ADRs 0001–0006
- [x] Workspace, pinned IW4L git deps, CI (asset guard, fmt/clippy/tests, PPC32 build, qemu 750CL tests, `.dol` build artifact)
- [ ] **Spike A, toolchain:** Rust `no_std` `.dol` (self-tests + GX scene on the GameCube pad). *Builds in CI, awaiting a hardware run.*
- [ ] **Spike C, memory:** usable MEM1/MEM2 after IOS + libogc + Rust runtime (Spike A prints them). Write `docs/budgets.md`.
- [x] **Spike D, data:** converter reads the user's IW4x install via IW4L: `mwf-convert inventory` lists every zone's
      sizes and asset counts (double-click `inventory.bat`), and `mwf-convert map` walks one map's assets with sizes.
      *Done 2026-11-13 on the user's install and `mp_rust` (`docs/spikes/results.md`). IW4x moves the retail x86 zones
      to `zone/iw4x/x86/` and rewrites `zone/english` as x64. IW4L reads both.*
- [x] **Spike E, portability:** IW4L crates build for the Wii. Movement trace is bit-identical x86 vs PPC 750CL (qemu).
      *Remaining: same hash on a real Wii (shown on the Spike A screen).*
- [ ] **Spike F, scripts:** IR size + heap for `war` (TDM) on one map (ADR 0002 budget: ≤ 3 MB MEM1).
- **Exit:** A, C, D, E, F pass or have an accepted fallback; budgets drafted.

## Phase 1: Converter MVP
- [ ] Pack format in `shared/` (zero-copy, big-endian, versioned, hashed).
- [x] Texture encoders (`shared/gx_texture`): all GX formats incl. CMPR, mips, per-role size/format policy, `mwf-convert texture`.
- [x] Zone textures through the encoders: `mwf-convert textures` (in-zone + IWI sources, skips normal/spec maps).
      *`mp_rust`: 608 images, 0 failures. See `docs/textures.md`.*
- [x] Texture roles and alpha from materials (users: world / human-skeleton model / model / effect; state bits: blend,
      alpha test, opaque). *`mp_rust`: 11.7 → 7.0 MiB on the Wii.*
- [x] Cut-outs: size cap ≥ 256 and alpha-test coverage kept at every mip level. *`mp_rust`: 7.6 MiB; worst mip
      coverage error 1% (plain cut: 47%).*
- [x] Smooth alpha split into CMPR colour + I4 alpha plane (8 bpp instead of RGB5A3's 16; UI keeps RGB5A3).
      *`mp_rust`: 6.7 MiB; split textures halved and +5.1 dB. The renderer needs a two-texture TEV recipe for them.*
- [x] `common_mp` textures: 1,624 images, 0 failures (wavelet IWIs decoded; weapon gun/hand models → viewmodel).
      *46.9 MiB: UI 30.1, viewmodels 14.0.*
- [x] Load by need: each texture's group (match / weapon / catalog / menu) from the zones; `index.json` per zone.
      *`common_mp`: match 8.2 MiB, worst case in a match 12.5 MiB (every model a weapon references goes with it).
      The runtime must load groups from the index.*
- [ ] Materials → TEV recipe table; world → cell-chunked display lists + lightmaps;
      clipmap → Wii collision layout; spawns/entities; weapon defs (lossless); GSC IR.
- [ ] `out/report.json` budget report. The build fails when over budget. Deterministic output.
- **Exit:** one small map's pack fits its budget and loads into a Wii viewer.

## Phase 2: Renderer and "walk the map"
- [ ] `gx_render`: cells/portals via `dpvs_iw4`, display lists, TEV recipes, sky, fog, viewmodel pass.
- [ ] First-person controller using IW4L `movement_iw4` on converted collision.
- [ ] Input: **GameCube pad** → binding table → IW4L `input_iw4` usercmds. Stick/C-stick curves, analog trigger thresholds, sprint scheme.
- **Exit:** walk the map at a stable 30 fps on real hardware, with movement trace = IW4L host trace.

## Phase 3: `wii_sim`, weapons, and a local match ("the game works")
- [ ] Lift `sim::step` off bevy_ecs. Run each ported system against upstream with matching `TickResult` hashes on the host.
- [ ] Weapons (IW4L `weapon_iw4` driven by converted defs), damage, hit boxes, skinned characters, anim, audio banks.
- [ ] GSC runtime on the Wii running `war` (TDM).
- [ ] Bots (IW4L `bots`, lifted off Bevy) so a match can be played on one Wii. Measure their CPU cost.
- **Exit:** a full TDM match against bots on one Wii, with feel traces matching IW4L exactly and the 360 reference within tolerance.

## Phase 4: Game breadth (single Wii)
- [ ] Modes already scripted upstream: dm, war, dom, sd, sab, ctf, koth, dd. Bring them up in that order.
- [ ] Create-a-class, perks, deathstreaks, attachments, equipment.
- [ ] Killstreaks, cheapest first: UAV, Care Package, Counter-UAV, Predator, Sentry, airstrikes, then helicopters, AC-130 and Nuke.
      Each one gets a budget line.
- [ ] Killcam and final killcam (reuse `killcam_iw4`). Frontend: menus, match setup, local progression saved to SD.

## Phase 5: System link, 4 players
- [ ] Hardware run of Spike B (discovery code already in `wii/spikes`, built in CI).
- [ ] `syslink`: discovery, lobby, protocol/build/pack-hash handshake.
- [ ] Listen server via `sim::step(AuthorityFrame)`, client prediction via `PredictNew`, delta snapshots, lag compensation.
- **Exit:** a full 4-Wii TDM match with no desync or crash, plus the per-tick state-hash checker passing.

## Phase 6: Content and optimisation (ongoing)
- [ ] Remaining base maps in waves, ordered by size. Large maps may need extra decimation or streaming.
- [ ] USB gamepad support (Spike G first: HID class, then per-device drivers) through the same binding table.
- [ ] Hardware profiling: skinning with paired singles, collision queries, GX FIFO, IOS socket cost, GSC hot paths.
- [ ] Optional DLC maps if present in the user's install.

## Phase 7: Source-only release
- [ ] One command: `cargo xtask convert --game <IW4x path>` then `cargo xtask dol`.
- [ ] User docs (SD layout `sd:/modernwiifare/`, Homebrew Channel, USB LAN setup, pack-hash mismatch help), vWii check.

---

## Top risks
1. **Memory** (~88 MB total vs content made for 512 MB machines). Mitigation: budgets from day one, enforced by the converter.
2. **GSC runtime + bots CPU cost on 729 MHz**. Mitigation: Spike F, then targeted native replacements.
3. **Porting `sim` off bevy_ecs** without drift. Mitigation: hash-compared `TickResult` against upstream for every system.
4. **Upstream churn**: IW4L is young and moving fast. Mitigation: pinned revs, deliberate bumps, upstream patches first.
5. **Rust on Wii**: stock PPC32 target linked by devkitPPC (ADR 0006). Builds and links; first hardware run pending.
6. **Legal exposure**. Mitigation: no data or GPL/leaked code, non-commercial, source-only. *(This is not legal advice.)*

# modernwiifare roadmap

**Goal:** MW2 (2009) multiplayer running natively on the Wii with **4-player system link over USB LAN**, played on the **GameCube controller**, built on **IW4L** and on
data each user converts from their own copy. Assets are downsized to fit; feel matches the reference builds.

Decisions: [ADR 0001](docs/adr/0001-base-and-source-data.md) IW4L base, PC/IW4x data, 360 reference ·
[0002](docs/adr/0002-gameplay-scripts.md) user's GSC via IW4L runtime · [0003](docs/adr/0003-player-count.md) 4 players ·
[0004](docs/adr/0004-license.md) Apache-2.0 ·
[0005](docs/adr/0005-input-and-hardware.md) GameCube pad first, USB pads stretch, wired LAN

## Architecture

```
  User's PC install (IW4x) ──► converter (host Rust, stable)
                                 ├─ IW4L fastfile/asset crates: read zones
                                 ├─ downsize: CMPR textures, quantized display lists, ADPCM, BVH collision
                                 ├─ GSC → IW4L script IR
                                 └─ out/packs/*.wpk + pack hash   (never committed)
                                               │ SD/USB
  Wii runtime (Rust nightly, no_std+alloc, ogc-rs/libogc) ◄──────┘
   ├─ IW4L no_std crates, reused as-is: movement, weapon, clipmap, trace, playerstate, entity, gamemode, anim, hud, fx...
   ├─ wii_sim   : IW4L `sim::step` + GSC runtime, lifted off bevy_ecs
   ├─ gx_render : new GX/TEV backend           (replaces wgpu render_*)
   ├─ syslink   : UDP LAN discovery + snapshots (replaces quinn/tokio net)
   ├─ snd       : ASND/AESND                   (replaces cpal)
   └─ front     : menus, lobby, class editor   (replaces Bevy UI)

  Reference builds for feel: user's 360 disc under mw2-recompiled · user's PC copy under IW4x · IW4L on host
```

The audit already done: 18 IW4L gameplay/format crates are `no_std` with no `unsafe`, no build scripts and no
native-endian I/O, so they are good candidates to compile for the Wii unchanged. Not compiled yet: that is Spike E.

---

## Phase 0: Foundations and risk spikes (≈3–5 weeks)
- [x] ADRs 0001–0005
- [ ] Workspace: `converter/`, `shared/`, `wii/` with pinned IW4L git deps. CI: host tests, `cargo check` on
      `powerpc-unknown-linux-gnu`, and a Wii `.dol` build in the `devkitpro/devkitppc` image. Asset guard as a CI step.
- [ ] **Spike A, toolchain:** Rust `no_std` hello-world `.dol` through ogc-rs. Draw a GX triangle on Dolphin **and** a real Wii.
- [ ] **Spike B, system link:** UDP broadcast discovery between 2+ Wiis over **USB LAN adapters** (Wi-Fi as a side check) and between 2 Dolphin instances.
- [ ] **Spike C, memory:** measure usable MEM1/MEM2 after IOS + libogc + Rust runtime. Write `docs/budgets.md`.
- [ ] **Spike D, data:** converter reads the user's IW4x install via IW4L and lists one small map's assets with sizes. Check that the IW4x zone layout is handled.
- [ ] **Spike E, portability:** build `movement_iw4` + `weapon_iw4` + `clipmap_iw4` for the Wii target. Run a movement trace
      on Dolphin and compare it bit-for-bit with the same trace on the host.
- [ ] **Spike G, USB pad (timeboxed):** enumerate a USB gamepad alongside the LAN adapter and read HID reports (ADR 0005).
- [ ] **Spike F, scripts:** IR size + heap for `war` (TDM) on one map with 4 players (ADR 0002 budget: ≤ 3 MB MEM1).
- **Exit:** all spikes pass or have an accepted fallback; budgets drafted.

## Phase 1: Converter MVP (≈5–7 weeks)
- [ ] Pack format in `shared/` (zero-copy, big-endian, versioned, hashed).
- [ ] Textures → CMPR/I4/RGB5A3; materials → TEV recipe table; world → cell-chunked display lists + lightmaps;
      clipmap → Wii collision layout; spawns/entities; weapon defs (lossless); GSC IR.
- [ ] `out/report.json` budget report. The build fails when over budget. Deterministic output.
- **Exit:** one small map's pack fits its budget and loads into a Wii viewer.

## Phase 2: Renderer and "walk the map" (≈6–8 weeks)
- [ ] `gx_render`: cells/portals via `dpvs_iw4`, display lists, TEV recipes, sky, fog, viewmodel pass.
- [ ] First-person controller using IW4L `movement_iw4` on converted collision.
- [ ] Input: **GameCube pad** → binding table → IW4L `input_iw4` usercmds. Stick/C-stick curves, analog trigger thresholds, sprint scheme.
- **Exit:** walk the map at a stable 30 fps on real hardware, with movement trace = IW4L host trace.

## Phase 3: `wii_sim` + solo sandbox (≈8–10 weeks)
- [ ] Lift `sim::step` off bevy_ecs. Run each ported system against upstream with matching `TickResult` hashes on the host.
- [ ] Weapons (IW4L `weapon_iw4` driven by converted defs), damage, hit boxes, skinned characters, anim.
- [ ] GSC runtime on the Wii running `war` with local bot-free players. Audio banks.
- **Exit:** a solo shooting range whose feel traces match IW4L exactly and the 360 reference within tolerance.

## Phase 4: System link, 4 players (≈6–8 weeks)
- [ ] `syslink`: discovery, lobby, protocol/build/pack-hash handshake.
- [ ] Listen server via `sim::step(AuthorityFrame)`, client prediction via `PredictNew`, delta snapshots sized for b/g Wi-Fi, lag compensation.
- [ ] TDM + FFA on 1–2 maps, 2 → 4 Wiis.
- **Exit:** a full 4-Wii TDM match with no desync or crash, plus the per-tick state-hash checker passing.

## Phase 5: Game breadth (≈8–12 weeks)
- [ ] Modes already scripted upstream: dm, war, dom, sd, sab, ctf, koth, dd. Bring them up in that order.
- [ ] Create-a-class, perks, deathstreaks, attachments, equipment.
- [ ] Killstreaks, cheapest first: UAV, Care Package, Counter-UAV, Predator, Sentry, airstrikes, then helicopters, AC-130 and Nuke.
      Each one gets a budget line.
- [ ] Killcam and final killcam (reuse `killcam_iw4`).

## Phase 6: Content and optimisation (ongoing)
- [ ] Remaining base maps in waves, ordered by size. Large maps may need extra decimation or streaming.
- [ ] USB gamepad support (HID class first, then per-device drivers) through the same binding table.
- [ ] Hardware profiling: skinning with paired singles, collision queries, GX FIFO, IOS socket cost, GSC hot paths.
- [ ] Frontend polish, local progression saved to SD. Optional DLC maps if present in the user's install.

## Phase 7: Source-only release
- [ ] One command: `cargo xtask convert --game <IW4x path>` then `cargo xtask dol`.
- [ ] User docs (SD layout `sd:/modernwiifare/`, Homebrew Channel, USB LAN setup, pack-hash mismatch help), vWii check.

---

## Top risks
1. **Memory** (~88 MB total vs content made for 512 MB machines). Mitigation: budgets from day one, enforced by the converter.
2. **GSC runtime cost on 729 MHz**. Mitigation: Spike F, then targeted native replacements.
3. **Porting `sim` off bevy_ecs** without drift. Mitigation: hash-compared `TickResult` against upstream for every system.
4. **Upstream churn**: IW4L is young and moving fast. Mitigation: pinned revs, deliberate bumps, upstream patches first.
5. **Rust on Wii maturity** (nightly, Tier-3-style custom target, ogc-rs last released 2022 but still maintained). Mitigation: Spike A early; C shims where needed.
6. **Legal exposure**. Mitigation: no data or GPL/leaked code, non-commercial, source-only. *(This is not legal advice.)*

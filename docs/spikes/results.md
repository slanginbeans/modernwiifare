# Phase 0 spike results

## Spike A: Rust toolchain + GX
| Field | Result |
|---|---|
| Build (sha) | |
| Wii model / region / TV mode / aspect | |
| Self-tests (all OK?) | |
| IW4L movement trace: hash / ms | |
| Background colour | |
| Scene smooth / controls respond | |
| START returns to HBC | |
| MEM1 free (KB) / MEM2 free (KB) | |
| Notes | |

## Spike D: converter reads the IW4x install (2026-11-13)
Run on the user's own IW4x install (Windows host) with `mwf-convert map` (`converter/src/map_walk.rs`). Its JSON
report is content-free (sizes and counts, no names) and stays in the git-ignored `out/`. Only aggregate numbers are
recorded here.

| Field | Result |
|---|---|
| IW4L rev | `acc57a5` (`asset_transport` inflates, `fastfile_iw4` walks) |
| IW4L detects the install (`folder_holds_game(Iw4)`) | yes |
| Map | `mp_rust` (smallest base MP map, 48.7 MiB `.ff`) |
| Walk | every asset entry loads, no errors. Inflate ~0.6 s, walk 25–35 ms (release) |
| Asset names resolved | all but 1 entry (one `impactfx`) |
| XFile blocks (x86 original) | virtual 30.5 MiB, vertex 12.4, index 2.1, runtime 0.2, temp 8.0 (scratch). 53.2 MiB total |
| Biggest costs (x86 original) | gfxworld 23.1 MiB resident · xmodels (126) 18.4 MiB · image pixels (608) 22.4 MiB · loaded sounds (112) 21.8 MiB · clipmap 2.1 MiB · pixel shaders 0.9 MiB |

**IW4x zone layout (answers the open check in ADR 0001):**
- The current IW4x launcher **rewrites the retail zones**. `zone/english/*.ff` are converted to the **x64 wire format**,
  and the retail x86 originals are moved to `zone/iw4x/x86/{english,dlc,patch}/` (see `zone-conversion.log` in the
  install). IW4L reads both formats.
- The x64 rewrite nests assets inline, so `mp_rust` shows 480 top-level entries (images and sounds counted inside the
  gfxworld and sound entries) against 2,582 in the x86 original. Same zone header totals (53.8 vs 53.2 MiB).
- `zone/iw4x/x86/dlc` also holds non-IW4 fastfiles (versions 0x115, 0x13c, 0x13f, 0x14c, 0x167, 0x168). IW4L only
  takes IW4 (0x114), so the converter must skip the rest.
- **IW4L's `find_zone_file` picked `usermaps/pcrust/mp_rust.ff`** (a custom map with the same name) over the stock
  `zone/english/mp_rust.ff`. The converter therefore picks `zone/english/<map>.ff` itself and only reports IW4L's
  choice. Logged in `docs/upstream-patches.md`.
- `main/` holds 15 `.iwd` archives (4.9 GiB). They were not needed for this inventory.
- `mwf-convert inventory` over the whole install (134 s): 389 zones, 77 `mp_*` maps, 332 read. All stock zones in
  `zone/english/` and `zone/iw4x/x86/english/` read. The 57 failures are 26 custom `usermaps/` zones in IW4x's own
  container (magic `IW4x`, not `IWff`) plus the 31 non-IW4 files in `zone/iw4x/x86/dlc`. Both are out of scope.

**Decision for Phase 1:** convert from the x86 originals in `zone/iw4x/x86/` when present, since they are the retail
bytes and give a per-asset breakdown, and fall back to `zone/english/`.

**Verdict:** pass. Rough Wii pressure for one map is ~45 MiB of world, model and collision data plus ~44 MiB of
textures and sound before downsizing. That feeds Spike C (`docs/budgets.md`).

## First game test round (2026-11-15)
Two `mwf_game` matches on the user's Wii, build `cfb39c57`, five bots, default class, 4:3, logs summarised with
`mwf-convert report`. No crashes. The numbers below are what the logs show. The logs themselves are not committed.

| | `mp_rust` (FFA, 308 s, 11,136 frames) | `mp_terminal` (109 s, 3,228 frames) |
|---|---|---|
| Load | 2.4 s | 4.4 s |
| Free at start | MEM1 20.7 MB, MEM2 43.7 MB | same |
| Free in play | MEM1 15.9 MB, MEM2 19.7 MB | MEM1 12.7 MB, **MEM2 2.75 MB** |
| Frame p50 / p90 / p99 | 25.5 / 28.3 / 32.0 ms (~39 fps) | 31.8 / 44.5 / 57.5 ms (~31 fps) |
| Frames over 16.7 / 33.3 ms | 99.9% / 0.6% | 98.2% / 35.4% |
| Average CPU / GPU wait | 10.4 / 15.0 ms: **GPU-bound** | 25.7 / 6.0 ms: **CPU-bound** |
| Biggest CPU cost | bots 2.8 ms (spikes to 195 ms) | world submission 18.9 ms (up to 42 ms) |
| Gameplay events | 44 kills, 31 hits, 20 explosions, 5 killstreaks | 2 kills, 1 grenade, 1 melee |

Findings and what was done:
- **Bot-respawn spikes** (`mp_rust`: 47 frames over 33 ms, the worst 129-195 ms, nearly all in the arena's bot
  step outside the think / move / gun timers): the spawn choice re-ran every spawn's sight traces on every
  comparison. Fixed by `arena::best_spawn` (lazy traces, same picks).
- **AUTO draw distance did nothing on `mp_terminal`**: it only shrank for GPU-bound frames. It now also shrinks
  when world submission is the frame's CPU cost.
- **`mp_terminal` MEM2 is tight**: 2.75 MB free in play. Watch class changes (weapon files) on the biggest maps.
- **Characters** cost 8-16 ms of CPU with several bodies in view (`bod`).
- **Stick range** seen: main stick up to ~106, C-stick ~91, triggers ~210 (of 255).
- Weapon switching: repeated Y presses were the tester double-tapping; switching worked normally.

## Spike B: system link discovery
| Field | Result |
|---|---|
| Build (sha) | |
| Number of Wiis / adapters / switch or router | |
| SO_BROADCAST / bind results | |
| Limited broadcast LIM% | |
| Directed broadcast DIR% | |
| RTT min/avg/max (ms) | |
| 10-minute soak: errors? | |
| Wi-Fi (optional) | |
| Dolphin (optional) | |
| Notes | |

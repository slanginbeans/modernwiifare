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

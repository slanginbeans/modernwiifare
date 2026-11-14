# The Wii game runtime (`mwf_game.dol`)

## Try it
1. Convert the maps on your PC: `mwf-convert pack "<your MW2 / IW4x folder>" --map all` → `out/packs/<map>.wpk` for
   all 16 multiplayer maps (or `--map mp_rust` for one).
2. Copy the `.wpk` files to the SD card in **`sd:/modernwiifare/maps/`** (the `.json` reports are not needed).
3. Copy `apps/mwf_game` from the build (`make -C wii package` → `dist/apps`, or CI's `modernwiifare-sd-card`) to
   `sd:/apps/`, and start **Modern Wiifare** from the Homebrew Channel. GameCube controller in port 1.

The game opens on a map list (D-pad or main stick up/down, **A** plays, **START** quits). In a map, **START**
returns to the list.

Controls, walking (IW4L movement and the map's collision): main stick move, C-stick look, **A** jump, **B** crouch
(held), **Z** sprint, **L** ADS. **D-pad up** toggles free fly (main stick, C-stick, L/R triggers down/up, B fast);
leaving free fly drops the player where the camera is. **Y** next spawn point, **D-pad down** toggles the
performance overlay (fps, frame / CPU / GPU-wait ms, triangles (of which static models), draws, state changes,
visible cells, models, position; see `docs/perf.md`),
**START** back to the map list. A pack without a `CLIP` section starts in free fly.

Solid colour screens (START goes back to the list): red = no SD card or no such pack, magenta = the pack failed its
checks (or is from an older converter: convert again), orange = out of memory or texture slots.

## Memory
The pack's GPU data (textures, vertices, display lists) is read into MEM2 and used in place; the sections decoded at
load (visibility, cell tables, collision) are written last by the converter, read into a temporary MEM1 buffer,
decoded into MEM1 and the buffer freed. `mwf-convert pack` prints the MEM2 part and warns above 44 MiB. Leaving a map
releases its MEM2 (bump allocator mark/release) and its textures.

## Structure
| Part | What |
|---|---|
| `wii/game_core` | `no_std`, host-tested: camera (IW4 Z-up → GX eye space), frustum test, skybox quads, free-fly controller, pack world loading and per-frame draw order through the `Gfx` trait; collision backend (`collision`) and the player (`player`: pad → `UserCmd` → IW4L `pmove`) |
| `wii/game` | Wii staticlib: `Gfx` over the C shim, allocator, panic screen, main loop |
| `wii/platform/shim_world.c` | MEM2 bump allocator, SD loading (libfat) with `DCFlushRange`, GX texture objects (+ I4 alpha planes), world vertex format (index16, lightmap UV u16 frac 15), TEV recipes, blend / alpha test / Z, display-list calls, skybox quads |

Per frame: one `pmove` with the frame's milliseconds, then the skybox (Z off), visible opaque and alpha-tested
world batches grouped by material and lightmap, then static model instances (each within its draw distance and in
view: view × placement loaded into the position matrix, its surfaces' display lists, its light-grid colour as a TEV
konst ×2), then blended world and model surfaces back to front. The pack is loaded once into MEM2 and
used in place; collision is decoded once into the MEM1 heap (about 2.6 MiB for `mp_rust`), where traces are fastest.

## Movement status
- `pmove` is IW4L's, unchanged; the context uses MW2's MP dvar defaults (190 speed, 800 gravity, jump 39, sprint ×1.5).
- Traces: world brushes through the BSP plus the triangle mesh (`clipmap_iw4::trace_brush_and_mesh`). Glass is
  always solid. Not yet: brush models (doors, movers), other players, penetration recovery.
- C-stick look speed (200°/s yaw, 140°/s pitch at full tilt, linear past a 12-count dead zone) is a placeholder. It
  must be tuned against the 360 version (`feel-parity` skill) before it is called right.

## Not verified on hardware yet
- GX front-face winding: `MWF_GX_CULL_BACK` in `shim_world.c` assumes GX's back faces match IW4's (D3D, clockwise
  front). If walls vanish and their insides show, swap the two defines.
- Draw cost of 180-375k triangles per view (world + static models) (`mwf-convert preview` reports the count); visibility cells and fixed-point
  vertices are the next performance steps.
- Trace cost on Broadway: on the host one `pmove` on `mp_rust` takes about 20 µs, so a 750CL should stay well under
  a millisecond, but this is unmeasured.

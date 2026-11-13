# The Wii game runtime (`mwf_game.dol`)

## Try it
1. Convert the map on your PC: `mwf-convert pack "<your MW2 / IW4x folder>" --map mp_rust` → `out/packs/mp_rust.wpk`.
2. Copy it to the SD card as **`sd:/modernwiifare/maps/mp_rust.wpk`**.
3. Copy `apps/mwf_game` from the build (`make -C wii package` → `dist/apps`, or CI's `modernwiifare-sd-card`) to
   `sd:/apps/`, and start **Modern Wiifare** from the Homebrew Channel. GameCube controller in port 1.

Controls: main stick move, C-stick look, L/R triggers down/up, B (held) fast, Y next spawn point, START exits.
Solid colour screens: red = SD or pack not found, magenta = the pack failed its checks, orange = out of memory or
texture slots.

## Structure
| Part | What |
|---|---|
| `wii/game_core` | `no_std`, host-tested: camera (IW4 Z-up → GX eye space), frustum test, skybox quads, free-fly controller, pack world loading and per-frame draw order through the `Gfx` trait |
| `wii/game` | Wii staticlib: `Gfx` over the C shim, allocator, panic screen, main loop |
| `wii/platform/shim_world.c` | MEM2 bump allocator, SD loading (libfat) with `DCFlushRange`, GX texture objects (+ I4 alpha planes), world vertex format (index16, lightmap UV u16 frac 15), TEV recipes, blend / alpha test / Z, display-list calls, skybox quads |

Per frame: skybox (Z off), visible opaque and alpha-tested batches grouped by material and lightmap, then blended
batches back to front. The pack is loaded once into MEM2 and used in place.

## Not verified on hardware yet
- GX front-face winding: `MWF_GX_CULL_BACK` in `shim_world.c` assumes GX's back faces match IW4's (D3D, clockwise
  front). If walls vanish and their insides show, swap the two defines.
- Draw cost of ~180k triangles per view (`mwf-convert preview` reports the count); visibility cells and fixed-point
  vertices are the next performance steps.

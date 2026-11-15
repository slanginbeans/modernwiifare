# Performance: can Modern Wiifare run at 60 fps?

Goal: 60 fps (16.7 ms per frame) at 640×480, four players. This page is the estimate before measuring on hardware.
The in-game overlay (frame, CPU, GPU-wait times; triangles; draws) exists to replace these estimates with numbers.

## The precedent
Activision's own Wii ports of the IW engine (*Modern Warfare: Reflex*, *Black Ops*, *MW3*) targeted **30 fps**, with
reviews reporting dips under load. They had Nintendo's SDK and engine-level access. 60 fps on the same hardware
needs a smaller scene than those games drew, not a cleverer renderer.

## Hardware budget per frame at 60 fps
| Resource | Peak | What it means at 60 fps | Our worst view today (`mp_rust`) |
|---|---|---|---|
| Hollywood vertex/setup | quoted ~18 M triangles/s (strips, simple vertices); real games far lower | ~150k strip triangles is an optimistic ceiling; plan for **≤ 100k** | 180–375k triangles as **indexed lists** |
| Vertex fetch | no post-transform cache: an indexed triangle list transforms **every** index (3 per triangle); strips ~1.1 | ≤ ~150k vertex transforms | 0.55–1.1 M vertex transforms |
| Memory bandwidth (vertex + index fetch) | MEM1 ~2.7 GB/s, MEM2 ~4 GB/s, shared with CPU and textures | ≤ ~5 MB of geometry traffic per frame | ~31 MB (28-byte float vertices, 8-byte indices, from MEM2) |
| Fill rate | 972 Mpixel/s peak, ~half with two textures | 640×480 × overdraw 3 ≈ 0.9 M pixels: **fine** | fine |
| CPU (Broadway 729 MHz) | ~12M cycles per frame | movement ~0.5 ms, culling 3.4k instances ~0.3 ms, submit ~1 ms: **fine** today; weapons, sim, 4 players, net and sound must fit in ~8 ms | ~2 ms (estimate) |
| CPU ↔ GPU overlap | today serial: CPU builds, then waits for the GPU (`GX_DrawDone`), then vsync | frame = CPU + GPU; pipelined it is max(CPU, GPU) | serial |

**Verdict:** today's renderer is roughly 4–8× over a 60 fps budget on `mp_rust`, set by geometry (vertex count and
fetch bandwidth), not pixels or CPU. 60 fps is plausible for the core of multiplayer after the steps below. It is not
plausible at PC/360 draw distances and prop density on every map.

## What gets us there (in order of payoff per effort)
1. **Triangle strips** in the converter (GX `0x98`). ~2.5× fewer vertex transforms and index bytes for the same
   triangles. No visual cost.
2. **Fixed-point vertices**: positions s16 (per-batch scale and offset), UVs s16, lightmap UVs u16. 28 → ~16 bytes.
   Hot geometry in MEM1 where it fits. No visible cost (sub-millimetre position error).
3. **Visibility**: IW4's cells and portals (`dpvs_iw4`). Only draw what the current cell can see. Big win indoors,
   smaller on open maps.
4. **Model LODs**: pack LOD 0–2 per model, choose by distance (IW4's own `lodDist`, scaled).
5. **Pipelined frames**: triple-buffered XFB, no `GX_DrawDone` stall: frame time = max(CPU, GPU).

## What would have to be sacrificed (from least to most visible)
1. **Clutter draw distance**: small props (shrubs, rocks, cables, debris) at 50–70% of PC distance; LOD switches earlier.
2. **Effects**: particle caps per effect, no soft particles; no dynamic lights or shadows (already gone with
   normal/specular maps).
3. **Viewmodel** at ~3–4k triangles (PC ~6–10k).
4. **Fog** pulled in on the largest maps so the far plane can come closer.
5. **Last resort, per map**: 30 fps render with movement still run at 60 Hz steps (two `pmove` per frame), so jump
   height and strafe feel stay at the 360's 60 fps behaviour. IW movement depends on frame time, so this keeps feel
   parity even when rendering cannot.

Not sacrificed: collision (never simplified), movement and weapon timings, 640×480 output.

## Log
| Step | `mp_rust` numbers |
|---|---|
| Baseline (indexed lists) | world 193k triangles = 580k display-list vertices; models 79k = 237k |
| Strips (opaque, alpha test, additive, multiply; alpha-blend keeps its authored order) | world 408k display-list vertices (blended lists included), models 139k: **-35% vertices sent**, pack 22.7 → 20.8 MiB |

| Visibility cells (IW4 dpvs: camera cell, portal walk through the frustum) | `mp_rust` is **one cell** (open map): no gain. `mp_terminal` (10 cells, 90 portals): every spawn is in a cell, 55/57 see part of the map; triangles per view 726k → 656k (-10%) |
| Static model LODs (IW4 `lodDist`; past the last LOD the model isn't drawn) | `mp_terminal` per view: static models 412k → 91k triangles (-78%); total 656k → **334k** (world 244k). Pack grows (all LODs): `mp_rust` 20.8 → 24.8 MiB, `mp_terminal` 47 MiB |

| Collision and visibility out of MEM2 (decoded from a temporary MEM1 buffer) | `mp_checkpoint` MEM2 51.7 → 44.3 MiB |
| Fixed-point vertices (18 B; float fallback for batches that don't fit) | all 16 maps in MEM2: **18.9–38.5 MiB** (was up to 51.7); `mp_checkpoint` 38.5, `mp_rust` 18.9. Models 100% compact; world batches ~60% compact (the rest: UV spans over ±128 or very large batches) |

| Bots (5): `pmove`, sight traces, gun, routes each frame | host: ~0.4 ms per frame for all five on `mp_rust`; Broadway is perhaps 10-20× slower, so **3-8 ms**: measure on hardware, and lower `BOTS` or think every other frame if it doesn't fit |

| World mesh simplification (meshoptimizer, `pack --simplify <ratio> --simplify-error <units>`; locked where batches meet; UV, lightmap UV and colour weighed) | `mp_rust` world 193k triangles: 161k at 0.5 / 4 units, 155k at 0.3 / 8, 151k at 0.2 / 16 (**-17 to -22%**). From 8 units up the textures on curved props warp and the ground shows patches where vertex colours (IW4's terrain blend weights) moved. MW2's world meshes are already low-poly: the count comes from many small surfaces, not dense ones. **Off by default**: too little gain for what it costs to look at. |

| Pipelined frames (`shim_gx.c`: the frame end waits for the previous frame's draw-sync token, shows it at the next retrace, queues this one's copy and token; three XFBs; at most one frame per retrace; first-person and character positions double-buffered; MEM2 / texture release waits for the GPU) | frame time max(CPU, GPU) instead of CPU + GPU, one frame of extra display latency. Expected on `mp_rust` from the last log (CPU 13-20 ms, GPU ~21 ms): ~21 ms frames, a steady 30 fps instead of 20-30. **Not yet measured on hardware.** |
| Static-model draw order and tint-only state (`mwf_world_tint`); opaque world batches presorted at load | Instances of one model draw back to back with one material, changing one konst register instead of reprogramming TEV and reloading textures. The overlay's STATE is now full+tint changes. **Not yet measured on hardware.** |
| Vertex welding and border-first strips (converter, REVISION 27) | `mp_rust`: world display-list vertices 408k -> 402k for 193k triangles, model ones 281k -> 262k, MEM2 -1 MB. |
| Skinning straight into MEM2 with multiplied weights (first-person model, characters) | One copy of every skinned position and a divide per weighted influence gone each frame. **Not yet measured on hardware** (`vm`, `bod`). |
| meshoptimizer strips for opaque batches where shorter (REVISION 28) | `mp_rust`: world display-list vertices 402k -> 393k, model ones 262k -> 254k. |
| First-person rigid vertices placed by GX bone matrices (matrix index per vertex, ten per draw) | Only blended vertices (arms, hands) are skinned on the CPU. **Not yet measured on hardware** (`vm`). |
| Weapon files streamed during the death screen (192 KB a frame from 1 s after death) | The respawn no longer stops for a 4-8 MB SD read when the class changed (whatever is left loads then). |
| Hardware round, build `cfb39c57` (`mp_rust` 5 min, `mp_terminal` 109 s; `docs/spikes/results.md`) | `mp_rust`: p50 25.5 ms, p99 32 ms, GPU-bound (GPU wait 15 ms, CPU 10.4 ms); bot-respawn spikes 50-195 ms. `mp_terminal`: p50 31.8 ms, p99 57.5 ms, 35% of frames over 33 ms, **CPU-bound in world submission** (up to 42 ms at 449k triangles / 2,362 draws, no GPU wait); characters 8-16 ms with several bodies in view |
| Spawn choice traces lazily (`arena::best_spawn`): each spawn's weight once, sight traces only for spawns that can still win (same pick as before; 2,000 random FFA + team cases) | Host test: 5.5× fewer sight traces (470,784 → 85,584). Removes the 50-195 ms bot-respawn frames' cause. **Not yet measured on hardware** (`bot`). |
| AUTO draw distance also shrinks when world submission is the CPU cost (≥ 35% of a missed frame and ≥ 4 ms) | `mp_terminal` was CPU-bound, so AUTO (GPU wait only) never moved. **Not yet measured on hardware.** |
| Slim weapon files (toggle: `--slim-views` / `MWF_SLIM_VIEWS=1`; `docs/pack.md`): keyframed clips with i16 translations, no gun mips, sounds at 32 kHz | Aim: the two weapon files in MEM2 (~2 × 1.8 MiB; `mp_terminal` had 2.75 MB free in play) well under 1 MiB each. Host tests: tracks baked from keys every 6 frames shrink > 3× within 0.02° / 0.005 units; noise never grows; textures lose their mip chain (~25%); 44.1/48 kHz sounds shrink 1.4-1.5×. **Real sizes and feel not yet measured** (needs the user's conversion and a hardware round). |
| ADPCM sounds (toggle: `--adpcm` / `MWF_ADPCM=1`; `docs/pack.md`): every clip as 4-bit DSP ADPCM, decoded 2048 samples at a time by the ASND voice callback | Aim: `common.wpk`'s 5.94 MiB of world sounds (`SNDD`) down to ~1.7 MiB, and the weapon files' sounds 3.5× smaller; costs 128 KiB of MEM1 buffers and a little audio-interrupt time (integer decode). Host tests: ~32 dB signal-to-noise, piecewise decode matches one pass. **Not yet heard on hardware.** |
| Smaller map packs (toggles `--tiled-uv` / `--small-dl`; `docs/pack.md`). From the user's reports: `mp_terminal` holds 4.67 MiB of float world vertices in 83 of 1,045 batches, and display lists of 7.85 MiB at ~8.4 bytes per vertex sent | Aim: those float batches to 18-byte vertices (up to ~1.7 MiB on `mp_terminal`, ~2 MiB on `mp_checkpoint`), and display lists down by a quarter to a half where batches are small or white (~1.5-2.5 MiB a map). Less vertex and index traffic per frame too. Host tests: a 500-repeat floor quantises without seams in < 2× the vertices; every layout decodes to the same triangles. **Real sizes and looks not yet measured.** |

Where that leaves us: `mp_terminal` spends ~244k world + ~91k model triangles per view, still ~3× the 60 fps
budget. Memory is solved for every map.

**Draw distance and model detail** (`World::draw_distance`, `World::lod_scale`; debug menu DRAW DISTANCE
full / 6000 / 4000 / 3000 / 2000 units / AUTO (follows the frame time: `game_core::auto_distance`), MODEL DETAIL normal / low / lower / lowest = LOD distance ×1 / 1.5 / 2 / 3,
MAP FOG on / off). A shorter draw distance moves the far plane in: world batches past it are culled by their bounds,
model instances by their spheres, and GX linear fog ends at the far plane, so geometry fades out instead of popping.
The fog is the map's own where the pack has it (`FOGD`: MW2's `setExpFog` from `maps/createart/<map>_art.gsc`, as a
line through the same start and the same density at the halfway distance; GX fog has no opacity cap, so it keeps
thickening past halfway); otherwise it fades to the clear colour from 60% of the distance. The sky is still drawn,
unfogged. These are the per-map 60 fps levers; the values to ship come from hardware logs (triangles and GPU wait
at each step).

IW4 geometry is cut by UV seams and small surfaces, so strips average ~1.7 vertices per triangle, not ~1.1. Better
vertex welding on the converter side (merge vertices identical in every attribute) is a later step.

## How we decide
The overlay (D-pad down cycles it) shows frame ms, CPU ms, GPU-wait ms, triangles and draws. After each step above,
walk the same spawns and write the numbers here. A step that doesn't move the numbers gets reverted.

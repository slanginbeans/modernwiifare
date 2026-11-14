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

Where that leaves us: `mp_terminal` spends ~244k world + ~91k model triangles per view, still ~3× the 60 fps
budget. Memory is solved for every map. Next: world draw distance (fog) and `LOD_SCALE` (in `game_core::world`) as
the per-map 60 fps levers, set from hardware numbers.

IW4 geometry is cut by UV seams and small surfaces, so strips average ~1.7 vertices per triangle, not ~1.1. Better
vertex welding on the converter side (merge vertices identical in every attribute) is a later step.

## How we decide
The overlay (D-pad down cycles it) shows frame ms, CPU ms, GPU-wait ms, triangles and draws. After each step above,
walk the same spawns and write the numbers here. A step that doesn't move the numbers gets reverted.

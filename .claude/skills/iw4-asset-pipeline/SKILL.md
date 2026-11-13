---
name: iw4-asset-pipeline
description: How the modernwiifare converter reads the user's own MW2 data (IW4 fastfiles/zones) and downsizes each asset type for the Wii. Use when working in converter/, adding an asset type, changing a pack format, or making a budget/quality trade-off.
---

# IW4 → Wii asset pipeline

The converter is a host program, never run on the Wii. It reads the user's install and writes `out/<platform>/` packs
in Wii-native formats. Everything it writes is git-ignored.

## Inputs
- Per ADR 0001, the source is the user's **PC MP install (IW4x)**. The converter is Rust and reads through IW4L's
  `fastfile_iw4` / `asset_iw4` / `asset_*` crates (Bevy is fine on the host). Do not write a second fastfile parser.
- GSC rawfiles are compiled to IW4L's script IR on the host and stored in the pack (ADR 0002).
- Fastfiles are zlib-compressed zones on PC (header `IWffu100`, version 276 for IW4). 360 zones differ in compression and
  endianness, so check before assuming.
- Use public format knowledge (IW4L docs, OpenAssetTools, ZoneTool, codresearch.dev). Record the license of anything borrowed
  in `docs/references.md`.

## Per-asset policy
| IW4 asset | Wii output | Downsizing rules |
|---|---|---|
| GfxImage | CMPR / I4 / IA8 / RGB5A3 + mips | Use a per-category max size (world 256, props 128, viewmodel 512, UI as needed). Drop normal/spec maps and bake their contribution into lighting. Dedupe by hash. |
| Material / Techset | TEV recipe id + texture refs | Map each techset family to one of a small fixed set of TEV recipes. Log unmapped techsets and never silently drop them. |
| XModel | Quantized display lists, ≤2 LODs | Keep the original LOD chain, but pick the 2 that fit budget. Characters target ~1.5–3k tris, the viewmodel ~4–6k. Split by bone palette (10 matrices per draw). |
| XAnimParts | Quantized keyframes | Reduce the keyframe rate where the curve error is below a threshold. Keep viewmodel anims at **full rate** because that is where the feel lives. |
| GfxWorld (map) | Cell/portal-chunked display lists + lightmaps | Decimate meshes that are not visible from playable space. Downscale lightmaps to CMPR or bake to vertex colour. Keep the original cells/portals for culling. |
| clipMap (collision) | Simplified BVH of brushes/tris | **Never simplify playable collision.** Movement feel depends on it. Only strip collision that players cannot reach. |
| ComWorld / MapEnts / spawns | Compact binary tables | Lossless. |
| Weapon defs / attachments | Binary structs | **Lossless.** These are the gameplay tuning. |
| Sounds (LoadedSound / streamed) | DSP-ADPCM, 22–32 kHz | Prioritise weapon fire, footsteps and callouts. Ambience can go mono and lower-rate. Music streams from SD. |
| FX (FxEffectDef) | Simplified particle defs | Cap particles per effect and use additive quads. Keep muzzle flash and impacts readable. |
| Rawfiles (GSC, configs) | Per ADR 0002 (GSC VM vs native port) | Never committed. |
| Menus / fonts | Rebuilt UI | Re-lay out for 480p. Re-rasterise fonts to the Wii font atlas. |

## Process rules
1. Every converter run writes `out/report.json` with per-map memory use against `docs/budgets.md`. The build fails when the budget is exceeded.
2. Conversion is deterministic: the same input gives byte-identical output. This makes the 2-Wii system link reproducible,
   because both consoles must load identical packs. Embed a pack hash and check it in the lobby handshake.
3. Write unit tests against **synthetic** zones produced by our own writer, never real data.
4. Quality knobs live in one `converter/profile.toml`, not scattered constants.

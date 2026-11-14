# The Wii pack (`.wpk`)

`mwf-convert pack <game folder> --map <zone>` writes `out/packs/<zone>.wpk` (git-ignored; it holds the user's converted
game data) and a content-free `out/packs/<zone>.json`. The format lives in `shared/wpk` (`no_std`), shared by the
converter (writer) and the Wii runtime (zero-copy reader).

## Container
Big-endian. A 64-byte header (magic `MWFP`, version, section count, FNV-1a content hash over everything after the
header, file length, map name), a table of 16-byte section entries `{ tag, offset, length, record count }`, then the
sections, **each 32-byte aligned**, so GX can use texture data and display lists where they lie once the file is in
MEM2 (after `DCFlushRange`).

## Sections (version 1)
| Tag | Records | Content |
|---|---|---|
| `TEXT` | `TextureRecord` (32 B) | GX format, levels, alpha-plane flag, wrap, size, offsets into `TXDA` |
| `TXDA` | – | GX-tiled texture data and I4 alpha planes, 32-byte aligned |
| `MATL` | `MaterialRecord` (16 B) | TEV recipe, blend (opaque / alpha test / blend / additive / multiply), alpha ref, cull, colour texture, sort key |
| `WVTX` | `WorldVertex` (28 B) | interleaved: position f32×3, RGBA8, UV f32×2, lightmap UV u16×2 (frac 15) |
| `WBAT` | `BatchRecord` (48 B) | vertex base, vertex count, material, lightmap texture, display-list range in `WDLS`, triangles, bounds |
| `WDLS` | – | GX display lists: `0x90` (triangles, VTXFMT0), u16 count, per vertex u16 indices for POS/CLR0/TEX0/TEX1 (all the same, relative to the batch's vertex base), NOP-padded to 32 |
| `SKYB` | `Skybox` (16 B) | six sky face textures, D3D cube order |
| `SPWN` | `SpawnRecord` (24 B) | origin, yaw, kind (TDM / DM), team, initial flag |
| `MVTX` | `WorldVertex` (28 B) | static model vertices in model space (same format as `WVTX`; lightmap UV unused) |
| `MSRF` | `BatchRecord` (48 B) | static model surfaces: vertex range in `MVTX`, material, display list in `WDLS`, model-space bounds |
| `MODL` | `ModelRecord` (24 B) | a model's run of `MSRF` surfaces and its model-space bounding sphere |
| `SMDL` | `InstanceRecord` (56 B) | a placed static model: model, draw distance, light-grid colour (RGBA8 at half intensity), model-to-world 3x4 matrix |
| `CLIP` | one blob (`wpk::clip::Clip`) | collision, unsimplified: a table of 16 arrays (count, offset), then pooled brush planes + surface flags, brushes (contents, glass), BSP nodes, leaves, leaf brushes, mesh vertices / triangles / walkable-edge bits / triangle flags / AABB trees / partitions / borders / roots, brush models. Decoded and validated at load into the `clipmap_iw4` types IW4L traces with |

## How the world is built (`converter/src/pack.rs`)
- Surfaces are batched by vertex group (consecutive surfaces spanning under 65,536 vertices, so u16 indices reach
  them), 1024-unit grid cell (for frustum culling), material and lightmap. Sky-material surfaces are dropped; the
  skybox replaces them.
- Lightmaps: IW4 lights by `(page0 + page1)²` in linear space; in gamma space that is `page0 + page1`, stored halved
  (TEV ×2). The pages come from the zone images IW4 names `*lightmap<i>_secondary` (BGRA, two pages stacked).
- Spawns: `mp_tdm_spawn*` and `mp_dm_spawn*` entities from the map's entity string.

## Checking a pack without a Wii
`mwf-convert preview <file.wpk> [--spawn n]` renders a 640×480 view from a spawn point in software, reading only the
pack the way the Wii does: textures decoded from their GX bytes, triangles from the display-list bytes, shading by the
TEV recipes (colour × lightmap × 2, alpha test, blend, additive, multiply) and the sky as a D3D cube sampled with the
world-space view ray. Debug switches: `MWF_PREVIEW_NOCULL=1`, `MWF_PREVIEW_OPAQUE=1`.

Findings it caught:
- A material's look comes from its **first colour-band technique** (slot 4 up), as IW4L reads it. The later "lit"
  slots (37–39) are additive per-light passes; taking those made most of the world additive.
- **Winding:** IW4 front faces have positive signed area in screen space with y pointing down (clockwise as
  seen on screen, D3D's default). The GX renderer must cull to match (check `GX_SetCullMode` with this in Dolphin).

## `mp_rust` (first pack, 2026-11-13)
14.5 MiB: textures 3.8, display lists 4.4, vertices 6.3. 235,386 vertices, 193,276 triangles in 171 batches, 75
materials, 3 lightmaps, skybox, 51 spawns. Next: fixed-point positions and UVs (smaller vertices), visibility cells,
static models, collision.

With collision (same day): 17.1 MiB. `CLIP` is 2.6 MiB: 2,668 brushes (19,959 planes), 704 BSP nodes, 723 leaves,
68,408 mesh triangles, 18 brush models. The host test `game_core::player::tests::spawns_stand_on_the_real_map`
puts a player on each of the 51 spawns with IW4L `pmove`: all land, and all run forward without falling through.

With static models (same day): 22.7 MiB. 3,356 instances of 76 models: 97,932 model-space vertices (2.6 MiB) and
79,102 triangles stored once, 727,810 triangles if every instance were drawn. Instancing keeps the pack small; draw
distance and the view frustum leave 180-375k triangles (world + models) per view in the preview, which is the next
performance problem (model LODs, visibility cells).

## Static models (`converter/src/static_models.rs`)
- Instances come from `GfxWorld` static model draw instances (origin, axis, scale, model, draw distance).
- Each model's LOD 0 surfaces: packed vertices (position, colour, half-float UV) and triangle lists; material from
  the model's handle array. A model can reuse an XModelSurfs set an earlier model loaded; the zone walk keeps those
  through IW4L's `remember_xmodel_surfaces` / `xmodel_surfaces` hooks.
- Light: IW4 lights static models from the world light grid. `light_grid.rs` (IW4L's sampler) samples it at each
  instance's lighting origin; the linear result for a white surface goes back to gamma (square root) and is stored
  at half intensity, multiplied ×2 on the GPU like lightmaps.
- Static model collision (XModel coll surfaces) is for bullet and sight traces only; player clip for props is
  compiled into the clipMap, which `CLIP` already carries. It comes with weapons.

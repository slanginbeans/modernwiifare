# The Wii pack (`.wpk`)

`mwf-convert pack <game folder> --map <zone>` writes `out/packs/<zone>.wpk` (git-ignored; it holds the user's converted
game data) and a content-free `out/packs/<zone>.json`. The format lives in `shared/wpk` (`no_std`), shared by the
converter (writer) and the Wii runtime (zero-copy reader).

## Container
Big-endian. A 64-byte header (magic `MWFP`, version, section count, FNV-1a content hash over everything after the
header, file length, map name), a table of 16-byte section entries `{ tag, offset, length, record count }`, then the
sections, **each 32-byte aligned**, so GX can use texture data and display lists where they lie once the file is in
MEM2 (after `DCFlushRange`).

## Sections (version 3)
Version 3 (2026-11-13): fixed-point vertices. Version 2: LOD tables in `ModelRecord`. Older packs are rejected
(magenta screen on the Wii); convert again.
| Tag | Records | Content |
|---|---|---|
| `TEXT` | `TextureRecord` (32 B) | GX format, levels, alpha-plane flag, wrap, size, offsets into `TXDA` |
| `TXDA` | – | GX-tiled texture data and I4 alpha planes, 32-byte aligned |
| `MATL` | `MaterialRecord` (16 B) | TEV recipe, blend (opaque / alpha test / blend / additive / multiply), alpha ref, cull, colour texture, sort key |
| `WVTX` | `WorldVertex` (28 B) | interleaved: position f32×3, RGBA8, UV f32×2, lightmap UV u16×2 (frac 15) |
| `WBAT` | `BatchRecord` (48 B) | vertex base, vertex count, material, lightmap texture, display-list range in `WDLS`, triangles, bounds |
| `WDLS` | – | GX display lists: `0x90` (triangles, VTXFMT0), u16 count, per vertex u16 indices for POS/CLR0/TEX0/TEX1 (all the same, relative to the batch's vertex base), NOP-padded to 32 |
| `SKYB` | `Skybox` (16 B) | six sky face textures, D3D cube order |
| `MINI` | `Minimap` (28 B) | the HUD minimap: its texture, northwest / southeast corners (MW2's `setupMiniMap` frame from the `minimap_corner` entities), north yaw, `compassmaxrange` |
| `FOGD` | `Fog` (24 B) | the map's distance fog: start, halfway distance, colour (RGB 0..1), max opacity, from the first `setExpFog` with literal values in the zone's scripts (the art script). Optional |
| `SPWN` | `SpawnRecord` (24 B) | origin, yaw, kind (TDM / DM), team, initial flag |
| `OBJS` | `ObjectiveRecord` (24 B) | game-mode objectives: kind (Domination flag), label (A, B, C ... from `script_label`), the trigger's origin, radius and height |
| `CHAR` | `view::encode_characters` | the player characters, allies then axis: each team's body (its first assault body) with its first head attached (shared bones by name), skinned surfaces of LOD 3 (or the nearest more detailed), textures in this pack, and the bone names |
| `MVTX` | `WorldVertex` (28 B) | static model vertices in model space (same format as `WVTX`; lightmap UV unused) |
| `MSRF` | `BatchRecord` (48 B) | static model surfaces: vertex range in `MVTX`, material, display list in `WDLS`, model-space bounds |
| `MODL` | `ModelRecord` (68 B) | model-space bounding sphere and up to 4 LODs: each a run of `MSRF` surfaces and IW4's switch distance (`lodDist`); past the last, the model is not drawn |
| `SMDL` | `InstanceRecord` (56 B) | a placed static model: model, draw distance, light-grid colour (RGBA8 at half intensity), model-to-world 3x4 matrix |
| `WVTQ` / `MVTQ` | `CompactVertex` (18 B) | fixed-point world / static model vertices: position s16 × 3 at 1/8 unit around the batch's origin, RGBA8, UV s16 × 2 (12, 10 or 8 fraction bits: GX vertex formats 3, 4, 5), lightmap UV u16 × 2 |
| `WBQZ` / `MSQZ` | `QuantRecord` (16 B) per batch / model surface | which array and GX vertex format its vertices use, its display-list layout (`flags`, byte 1: `wpk::quant::dl_flags`), and the origin the renderer adds through the position matrix |
| `VISD` | one blob (`wpk::vis::Vis`) | IW4 visibility: dpvs planes and node stream (point → cell), cells (bounds, portal run), portals (plane, neighbour, vertices, hull axis) |
| `WBCL` / `SMCL` | u16 per batch / instance | the visibility cell of each `WBAT` batch and `SMDL` instance (`0xFFFF`: none, drawn when in the frustum) |
| `NAVG` | one blob (`wpk::nav::NavGraph`), decoded at load | the bots' walk graph: floor points (feet positions) and moves (target, cost, walk or one-way drop), baked from the collision by `game_core::nav::bake` |
| `CLIP` | one blob (`wpk::clip::Clip`) | collision, unsimplified: a table of 16 arrays (count, offset), then pooled brush planes + surface flags, brushes (contents, glass), BSP nodes, leaves, leaf brushes, mesh vertices / triangles / walkable-edge bits / triangle flags / AABB trees / partitions / borders / roots, brush models. Decoded and validated at load into the `clipmap_iw4` types IW4L traces with |

## Rebuilding only what changed
Every pack the converter writes starts with a `STMP` section (12 bytes): the converter's output revision
(`converter/src/stamp.rs`, `REVISION`) and a fingerprint of the zone files it read (their names, sizes and
modification times; no game content). `pack` and `common` skip an output whose stamp matches. **Bump `REVISION`
whenever a change alters what the converter writes**, so everyone's packs are rebuilt on their next run. The Wii
ignores the section.

## `common.wpk` and `views/*.wpk` (`mwf-convert common`)

`common.wpk`:

| Tag | Records | Content |
|---|---|---|
| `WEAP` | `WeaponRecord` | every weapon's combat numbers (IW4L `CapturedCombatInput`), sorted by name |
| `KICK` | `KickRecord` (80 B) | recoil, parallel to `WEAP` |
| `WFPV` | `FpvRecord` (224 B) | how each first-person weapon moves, parallel to `WEAP`: view sway (hip / ADS), idle sway, stance and movement offsets, crouched / prone offsets, ADS aim pitch and bob factors |
| `WEQP` | `EquipRecord` (324 B) | thrown / launched weapons, parallel to `WEAP` (zero for guns): offhand class, fuse and cooking, blast radius and inner / outer damage, throw speeds, arming distance, stickiness, direct-hit (impact) damage, the blast's cone angle, bounce factors per surface type, and its world sounds (explosion, bounce, pull back, throw: alias indices into this pack's `SNDA`). The explosion is the weapon's own, else its explosion effect's sound element, else IW4L's engine alias (`grenade_explode_default` / `rocket_explode_default`) |
| `SNDA` / `SNDV` / `SNDD` | as in `views/*.wpk` | the thrown weapons' world sounds |
| `WPEN` | `PenRecord` (8 B) | bullet penetration per weapon, parallel to `WEAP`: `penetrateType`, `penetrateMultiplier` |
| `PENT` | f32 × 124 | the penetration depth table (`info/bullet_penetration_mp`), by type (none, small, medium, large) and surface type |
| `STEP` | u16 × 248 | footstep aliases (indices into this pack's `SNDA`) by gait (sprint, run, walk, prone), listener (others, you) and surface type |
| `WSMK` | `BlockerRecord` (92 B) | sight-blocking elements of thrown weapons' explosion effects (smoke): weapon index, one-shot / looping, particle count, delay and life ranges, spawn offset radius and height, distance fade, size and alpha at five points of a particle's life |
| `CLSS` | one blob (`wpk::classes::Catalog`) | Create-a-Class: weapons (group, display name, attachment mask), attachments and the pairs that clash, perks / equipment / deathstreaks by slot, special grenades, killstreaks, MW2's 5 default classes and 5 starting custom classes |
| `RULE` | one blob (`wpk::rules::Rules`) | match rules read from the user's game-type scripts (`converter/src/rules.rs`): max health, regeneration (delay, step, tick, hurt cutoff, very-hurt wait), free-for-all time and score limits, points for a kill / headshot / assist, move speed by the primary weapon's class and Lightweight's scaler, flashbang / stun durations and shocks, the claymore's trip (cone, minimum distance, grace period, radius) and explosives per player, and which values were read |
| `PANM` / `PANB` | `wpk::player_anim` (read in place) / bone names | every animation MW2's player animation script can play for the weapons a class can hold (about 260 `pb_*` / `pt_*` clips, 3.6 MiB): tracks by bone name; key frames where interpolation is off by more than ~0.9 degrees / 0.3 units, rotations as three i16 (w ≥ 0), translation deltas as i16 / 32, fingers held at their first pose; no rotation keeps the bind rotation |
| `PSCR` | `wpk::anim_script::Script` | MW2's player animation script (`mp/playeranim.script`, `mp/playeranimtypes.txt`): for each movement type and event, its items in order (conditions with IW4's numbers, commands: body part, `PANM` clip, duration, blend time). Items that can't match here (turrets, last stand, anim types no weapon has) are left out |
| `GUNM` / `GUNI` | `wpk::held` (read in place) / u16 per `WEAP` record | the weapons in the characters' hands: each base weapon's MW2 world model at its first LOD of at most 900 vertices (attachment parts the base weapon hides left out), bind pose with the origin at `tag_origin`, as GX arrays and strip display lists (vertex format 6); every weapon record's model (its longest base name) |
| `GTEX` / `GTXD` | as `TEXT` / `TXDA` | the held weapons' textures, top mip levels left out down to 64 texels |
| `UIPK` | one blob (`wpk::ui::UiPack`, also: owner-draw rects such as the minimap's, and named compass icons) | the menus: fonts (glyph tables over atlas textures), screens (textured quads on 640 × 480, the button style, text labels with their kind: MW2 text, title, or a slot for live text; the back button's rect), and a plain white texture for filled bars |
| `TEXT` / `TXDA` | as in map packs | the menus' textures (font atlases, backgrounds, gradients, bars) |

`views/<weapon>.wpk`, one per weapon a class can carry (its base name, e.g. `views/m4.wpk`):

| Tag | Records | Content |
|---|---|---|
| `TEXT` / `TXDA` | as in map packs | the first-person model's textures |
| `VIEW` | one blob (`wpk::view::ViewModel`), `count` = the weapon's `WEAP` index | skinned first-person model and its clips |
| `WSND` | one blob (`wpk::sound::WeaponSounds`) | the alias of each weapon event (fire, last shot, empty, reload, empty reload, raise, first raise, put away, melee swipe) and notetrack sounds (anim slot, time fraction, alias) |
| `SNDA` | `AliasRecord` (4 B) | a sound alias: a run of `SNDV` variants |
| `SNDV` | `VariantRecord` (40 B) | a clip in `SNDD` (offset, padded length, rate), volume and pitch ranges, probability weight, falloff distances (min / max) |
| `SNDD` | – | mono 16-bit big-endian PCM, each clip 32-byte aligned and padded to 32 (ASND DMA) |

### Smaller map packs (toggles, off by default)
Two switches on `pack` (or in `builder.bat`), each folded into the map pack's stamp so turning it on or off rebuilds the
maps:
- **`--tiled-uv`** (`MWF_TILED_UV=1`): a batch that doesn't fit fixed-point vertices because one connected piece
  spans 128 or more texture repeats (wide tiled floors, roads, walls) is split into UV tiles
  (`wpk::quant::quantize_tiled`, 96 repeats, else 32, else 8). Each tile gets its own whole-number UV shift. Vertices
  on tile borders are duplicated: the same position, with UVs that differ by whole numbers and so sample the same with
  repeat wrapping. No seams, 18 bytes instead of 28 per vertex. The pack report counts `tiled_batches` and why any
  batch still stays float (`float_position`, `float_triangle_uv`, `float_vertices`).
- **`--small-dl`** (`MWF_SMALL_DL=1`): a batch with at most 256 vertices gets u8 display-list indices
  (`dl_flags::IDX8`), and one whose vertex colours are all white drops the colour index (`dl_flags::NO_COLOR`; the
  shim's `mwf_world_desc` then feeds the colour channel a white register). A display-list vertex costs 8 bytes in the
  old layout, 6 without colour, 4 with u8 indices, 3 with both. The report counts `dl_idx8_batches` and
  `dl_no_color_batches`.

Older packs have `flags` 0 and draw exactly as before.

### Slim weapon files (toggle, off by default)
`mwf-convert pack ... --slim-views` / `common ... --slim-views`, or `MWF_SLIM_VIEWS=1` in the environment (so
`build.bat` picks it up: `set MWF_SLIM_VIEWS=1` before running it). The option is part of the pack stamp, so turning it
on or off rebuilds `common.wpk` and the weapon files on the next run. With it on, each `views/<weapon>.wpk` gets:
- **Compact clip tracks** (`VIEW` header flag `wpk::view::flags::COMPACT_TRACKS`): only the frames linear interpolation
  can't reproduce within `Tolerance::SLIM` (0.02° rotation, 0.005 units translation, checked against the decoder's own
  output), each with its frame number (none when every frame is kept), and translations as i16 with a per-track scale
  (f32 when a track's range is too wide for that). `ViewModel::decode` expands them back to every frame, so the
  runtime's samplers and the in-memory model are unchanged; only the file in MEM2 shrinks.
- **No texture mips** on the first-person model (level 0 only, and level 0 of any I4 alpha plane). The shim turns
  mipmapping off for single-level textures.
- **Sounds at 32 kHz** where the source is faster (a Blackman-windowed sinc low-pass just under 16 kHz); slower clips
  are untouched.

The converter prints the total size of the weapon files; compare a run with and without the option.

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

## Display lists
- Opaque, alpha-tested, additive and multiply surfaces are stripped (`wpk::strips`, greedy, winding-preserving); the
  pack build decodes every list again and checks every triangle and its winding. Alpha-blended surfaces keep a plain
  triangle list in the authored order, which is their layering order.
- Depth pull (`MaterialRecord.flags & POLYGON_OFFSET`): decals (IW4 polygon offset) and every layer drawn over
  coplanar geometry without writing depth (blend, add, multiply) are pulled towards the eye by a factor of
  `1 - level / 2048` in eye space. Same pixels, slightly smaller depth. GX has no polygon offset, and PC's layering
  relies on bit-identical depth from identical vertices, which strips (rotated triangles) don't keep.

## Vertices (`wpk::quant`)
Every batch owns its vertices (renumbered from 0). They are stored fixed-point when they fit: positions within
±4096 units of an origin snapped to a 64-unit grid (so a vertex shared by two batches rounds identically: no cracks),
and UVs within ±128 after removing a whole-number offset per connected piece (repeat wrapping samples the same).
The rest stay float, snapped to the same 1/8-unit grid so coplanar layers stay coplanar across formats. Static models
use one origin per model. `MWF_NO_QUANT=1` turns quantization off (A/B checks in the preview).

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

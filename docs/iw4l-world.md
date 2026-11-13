# Reading a map zone for the Wii: IW4L reference

Where the converter gets world geometry, lightmaps, static models, visibility, collision, movement and spawns from IW4L
(rev `acc57a5`). Offsets are x86 retail zones (`s.layout(x86, x64)`). Values in zones are little-endian; the pack is
big-endian. `asset_world`, `asset_model` and `asset_material` depend on Bevy, so their reading logic is copied (with
attribution in `docs/references.md`), not depended on.

## Walking a zone
`asset_transport::open_zone` → `ZoneMemory::for_header` → `memory.stream(&bytes)` → `fastfile_iw4::load_zone(&mut s, sink)`.
The sink's `load_asset` calls `load_asset_at_observed(s, ty, slot, self)` and implements `AssetLinkSink::{loaded, alias}`.
After a load: `s.gfx_world()`, `s.clip_map()`, `s.map_ents()`, `s.xmodel()` (latest only). Readers: `s.f32_at`,
`u32_at`, `u16_at`, `ptr_at`, `cstr`, `slice_at`, `source_slice`.

## World draw (`GfxWorldGeometry`, `fastfile_iw4/src/zone.rs:485`; `load/gfxworld.rs`)
- `vertices` / `vertex_count`, `indices` / `index_count` (u16, local to the surface: vertex = `firstVertex + idx`),
  `surfaces` / `surface_count`, `lightmaps[32]` / `lightmap_count`, `light_grid`, smodel arrays, dpvs fields.
- **GfxWorldVertex (44 B):** xyz f32×3 @0, binormalSign f32 @12, colour u32 BGRA @16, texCoord f32×2 @20,
  lmapCoord f32×2 @28, normal packed @36, tangent packed @40. Packed unit vector: `scale = (b3 + 192) / 32385`,
  `xyz = (b0..2 − 127) · scale` (`dpvs_iw4::skin_unpack_unit_vec`).
- **GfxSurface (24 B):** firstVertex u32 @4, vertexCount u16 @8, triCount u16 @10, baseIndex u32 @12, Material* slot
  @16, lightmapIndex u8 @20, reflectionProbe u8 @21, primaryLight u8 @22, flags u8 @23. Bounds in `surfaces_bounds`
  (24 B: mid f32×3, half f32×3). Reference readers: `asset_world/src/world_mesh.rs:134` (`build_world_mesh`),
  `world_draw.rs:683` (`build_world_draw`).

## Lightmaps (`asset_world/src/world_draw.rs:1134-1347`)
Lightmap array at draw+16/+20, 8 B per `GfxLightmapPair { primary, secondary }`.
- **Secondary:** D3D format 21 (A8R8G8B8, BGRA bytes), two pages stacked vertically: top = page0 ("ambient"),
  bottom = page1 ("directional").
- **Primary:** format 50 (L8), sun-shadow mask, same aspect as one secondary page.
- `asset_material::lightmap_bake`: `out = (page0·inv_n + page1·ndotl)²`; with a flat normal this is
  `(page0 + page1)²`. Sun term (primary mask × sun colour) comes on top.

## Static models
- `smodel_draw_insts` (76 B): origin f32×3 @0, axis f32[3][3] @12, scale @0x30, XModel* slot @0x34, cullDist u16 @0x38,
  lighting handle @0x3a, flags @0x3e, packed lighting @0x40. `smodel_insts` (36 B): bounds, lighting origin @0x18.
- The model slot fires `loaded`/`alias` for XModel (link it like images and materials).
- Static models are lit from the light grid (`lighting_iw4`), not vertex colours.
- `XModelGeometry`: `lod_xsurfaces[4]`, `lod_numsurfs`, `lod_surf_index`, `material_handles`, `num_lods`.
  **XSurface (64 B):** vertCount u16 @2, triCount u16 @4, triIndices* @12 (u16 local), verts0* @28.
  **GfxPackedVertex (32 B):** xyz f32×3 @0, colour BGRA @16, texcoord halves @20 (u = high 16 bits, v = low),
  normal packed @24, tangent packed @28.

## Visibility (`dpvs_iw4`, no_std)
Planes and nodes → `DpvsPlanes::cell_for_point`; cells and portals → `visit_cells`; per-cell AABB trees →
`add_aabb_tree_surfaces_in_frustum`. Reference extraction: `world_draw.rs:160, 1807-2000` (`extract_dpvs`).

## Collision (`clipmap_iw4`, no_std + alloc)
`ClipMapRef { nodes, leaves, leafbrushes, brushes }`, `ClipMeshTables { verts, tri_indices, tri flags, aabb_trees,
partitions, borders, aabb_roots }`, `TraceExtents::new(start, end, mins, maxs, mask)`,
`trace_brush_and_mesh(&map, &mesh, &ext, &glass) -> Trace`, static models (`ClipStaticModel`, `trace_static_model`).
Built from a zone by `asset_world::build_clip_collision` (`clip_collision.rs:528`): brushes get 6 axial planes from
their bounds plus side planes; then BSP, mesh and cmodel tables.

## Movement
`movement_iw4::pmove(&mut PlayerState, &mut UserCmd, PmoveSingleContext, &impl CollisionBackend, lengths, root)`.
`CollisionBackend` needs only `trace(GroundTraceInput) -> Trace`. Template: `wii/iw4l_probe/src/movement_trace.rs`
(context values, player state seed, tick loop).

## Spawn points
Entity string from `map_ents` (`asset_world/src/map_entities.rs`: `parse_entities`, `parse_spawn_points`). IW4L's list
lacks `mp_tdm_spawn`; the converter keeps its own classname list.

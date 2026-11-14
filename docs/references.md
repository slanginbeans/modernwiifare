# External references

Record **every** project we read, borrow from, or test against, with its license. See the `asset-hygiene` skill.

| Project | What it is | License | How we use it |
|---|---|---|---|
| [IW4L](https://github.com/vladtrc/iw4L) | From-scratch Rust runtime that reads the user's MW2 MP install: FastFile readers, sim, GSC runtime, netcode | Apache-2.0 | **Code base.** Pinned crates plus ported `sim` (ADR 0001). Audited at `acc57a5` |
| [ogc-rs / ogc-sys](https://github.com/rust-wii/ogc-rs) | Rust bindings for libogc | see repo | Wii platform layer |
| [libogc](https://github.com/devkitPro/libogc) / devkitPPC | Wii homebrew SDK and toolchain | see repo | Platform, linker |
| [mw2-recompiled](https://github.com/PaulCombal/mw2-recompiled) | 360 static recomp (disc 1.0.557 + TU6) for PC. MP and system link playable | GPL-3.0-only | **Behavioural reference only, no code** (ADR 0004) |
| [IW4x](https://github.com/iw4x/iw4x-client) | PC MW2 MP client | see repo | The user's PC data source and a reference build |
| [COD6-NX](https://github.com/GreenieSparkles/COD6-NX) | 360 recomp → Switch | GPL-3.0 / BSD-3 parts | Lessons only |
| OpenAssetTools, ZoneTool, [codresearch.dev](https://codresearch.dev) | IW asset tooling and wiki | check per repo | Format knowledge (IW4L already credits OAT) |

**Excluded:** leaked IW source, private decompilations (e.g. momo5502's AI-assisted MW2 decomp), repos of dumped game scripts or assets.

# Files copied or adapted from IW4L
| Our file | Adapted from (IW4L `acc57a5`, Apache-2.0) | What |
|---|---|---|
| `converter/src/pixel_decode.rs` | `crates/asset_material/src/material_images/decode.rs` | D3D and IWI format tables, IWI v8 top-mip range, BCn decode through `bcdec_rs`, the wavelet IWI level loop (`decode_wavelet_iwi`, calling `asset_iw4`'s wavelet decoder). Carried because `asset_material` pulls in Bevy |
| `converter/src/collision.rs` | `crates/asset_world/src/clip_collision.rs` | IW4 clipMap → brushes (axial + side planes, surface flags), BSP, leaf brushes, mesh tables, brush models (`build_clip_collision` and helpers). Carried because `asset_world` depends on Bevy; static-model collision and IW5/T5 paths left out |
| `wii/game_core/src/player.rs` | `crates/sim` pmove context values (dvar defaults) | Only the numbers; the movement itself is IW4L's `movement_iw4::pmove`, called unchanged |
| `converter/src/light_grid.rs` | `crates/asset_model/src/model_lighting.rs` | Light grid view and per-point sampling (`OwnedLightGrid`, `sample_light_grid`, tile helpers), with Bevy's task pool left out |
| `converter/src/static_models.rs` | `crates/asset_world/src/model_mesh.rs`, `crates/asset_model/src/model_skel.rs` | Static model draw-instance layout, XSurface / packed vertex reading, shared XModelSurfs by name. Written fresh on those layouts |
| `converter/src/weapons.rs` | `crates/asset_game/src/weapon_catalog/capture_iw4.rs`, `combat.rs` | WeaponGeometry → `CapturedCombatInput`, field for field |
| `converter/src/viewmodel.rs` | `crates/asset_model/src/model_skel.rs` (bones, bind pose, `decode_surface_skin`), `crates/xmodel_runtime/src/dobj.rs` (attached skeletons), `crates/asset_anim/src/xanim_catalog.rs` (`capture_xanim`), `crates/anim_iw4/src/hide.rs` (hidden parts) | First-person model and animations; clips decoded with `xmodel_runtime::AnimClip` |
| `wii/game_core/src/damage.rs` | `crates/sim/src/bullet.rs` (`bullet_damage_at_distance`), `crates/weapon_iw4/src/fire_weapon.rs` (location scale) | Range falloff ported; hit volumes are our own approximation |
| `converter/src/weapons.rs` (location table) | `crates/asset_game/src/lochit.rs` (`parse_lochit_info_string`), `crates/asset_game/src/weapon_catalog/combat.rs` (`bake_location_damage`) | `mp_lochit_dmgtable` parsed and baked into each weapon |
| `wii/game_core/src/nav.rs` | `crates/bots/src/nav.rs` (grid, step, walkable floor, drops, `nearest_within`) | Same approach, written fresh (no jumps, mantles or doors); A* ours |
| `wii/game_core/src/bot.rs` | `crates/bots/src/motor.rs` (turn rate / acceleration limits, fire cone, `wish_to_move`), `crates/bots/src/nav.rs` (`roam_node` band) | Motor ported; targeting, memory and route following written fresh in IW4L's sense → intent → motor shape |
| `converter/src/sounds.rs` | `crates/asset_audio/src/sound_catalog/capture_iw4.rs` (LoadedSound header and alias row fields, loaded-sound binding), `crates/asset_audio/src/sound_catalog.rs` (`loaded_name_for_offset`), `crates/asset_game/src/weapon_catalog/capture_merge.rs` (`read_script_string_map`) | Sound aliases and in-zone PCM; notetrack sound map |
| `wii/game_core/src/weapon_sound.rs` | `crates/weapon_iw4/src/event_sound.rs` (`reload_insert_event`, `play_note_mapped_sound_alias`) | Which alias a weapon event or notetrack plays |
| `converter/src/material_links.rs` | `crates/assets/src/lane/helpers.rs` (slot map scheme), `crates/render_material/src/state.rs` (blend bits) | Slot → asset links from `loaded`/`alias`; reading blend op and factors from GfxStateBits word 0. Written fresh, same approach |

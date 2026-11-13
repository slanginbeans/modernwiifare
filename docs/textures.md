# Zone textures on the Wii

`mwf-convert textures <game folder> [--map <zone>]` takes every GfxImage a map zone loads, gets its pixels from the zone
or from `images/<name>.iwi` in `main/*.iwd`, and encodes it with `shared/gx_texture` (`converter/src/zone_textures.rs`).
The `.gxt` files and `report.json` go to the git-ignored `out/textures/<zone>/`. The report holds counts, bytes and
quality only. `--verbose` prints one line per texture, with its name, to the console only.

## What is converted
| GfxImage | Decision |
|---|---|
| image semantic normal map (5) or specular map (8) | skipped (normal maps are baked into lighting) |
| name `*...` (lightmaps, reflection probes) | skipped here: converted with the world |
| name `$...` (`$white`, ...) | skipped: made on the Wii |
| name `,...` (reference to an image another zone holds; no data here) | skipped: converted with that zone |
| cube map used by a sky material | six 2D faces, sky role (below) |
| any other cube map (environment reflections) | skipped: the Wii does not draw reflections |
| 3D (volume) map | skipped: not used on the Wii |
| used by materials, but never as colour | skipped |
| used by a material as colour (semantic 2D, function or colour map) | encoded; role and alpha from the materials |
| used by no material in the zone | encoded; role guessed from the name (fallback) |

Sources: in-zone D3D formats DXT1/3/5, BGRA8, BGRX8, A8, L8, LA8; IWI v8 formats 1–5 and DXT1/3/5. Wavelet IWIs and BC5
are not decoded yet (none were needed for `mp_rust`). Identical sources are encoded once per role and alpha use.

## Roles and alpha from materials (`converter/src/material_links.rs`)
IW4L reports every asset pointer it follows (`loaded` for inline assets, `alias` for references to earlier ones). Slot
maps link each material texture slot to its image. Each material is credited to the top-level zone entry whose load
referenced it.

**Role** of a material, then of each image (several materials: the larger size cap wins):
| Material | Role |
|---|---|
| named in a map script's `setupMiniMap("...")` call (zone rawfiles) | minimap: 512, no mips, smooth alpha split into CMPR + I4 |
| camera region "depth hack" | viewmodel |
| sort key sky / skybox | sky |
| used by the GfxWorld | world |
| used by an XModel that a weapon lists as its gun or hand model | viewmodel |
| used by an XModel whose skeleton has human bones (`j_spinelower`, `j_head`, ...) | character |
| used by any other XModel | prop |
| used by Fx / ImpactFx / FxWorld | effect |
| no user in the zone, technique set `2d` (IW4 names may start with `,`) | UI |
| no user in the zone, `effect*` technique set or emissive camera region | effect |

**Alpha** from the material's state bits, applied to the pixels before encoding:
| Material state | Texture alpha |
|---|---|
| any pass blends with source/destination alpha | kept: smooth colour alpha is split into CMPR + I4 (below); UI keeps RGB5A3 |
| alpha test (> 0, < 128, ≥ 128) | 1 bit with the full-size coverage kept at every mip level (below), so CMPR |
| opaque or additive (ONE/ONE) | dropped, so opaque CMPR |

Models are credited through their own material handle arrays, wherever they load (top level, inside a weapon or the
world). Weapons name their gun and hand models through IW4L's `remember_xmodel_name` / `xmodel_name_ptr` hooks, which
the converter implements. World and effect users are credited by the top-level entry whose load referenced the
material. MW2's viewmodel materials do not use the depth-hack camera region (the engine applies it when drawing), so the
weapon link is what finds viewmodels.

Sources also include wavelet-compressed IWIs (formats 6–10), decoded with IW4L's `asset_iw4` wavelet decoder.

## Alpha-tested (cut-out) textures
Foliage, fences and gratings are mostly detail thinner than a texel once downscaled. Cutting the averaged alpha at a
fixed level thins them out, worst in the small mip levels used at a distance. `gx_texture::policy::convert_with` with
`Options { cutout: Some(threshold) }` therefore:
- caps the texture at `Role::cutout_max_dim()`: at least 256, so props and effects are not cut to 128;
- builds the mips from the smooth alpha, then cuts each level so that the same fraction of texels passes the alpha
  test as at full size (`image::cut_to_coverage`). Where fine detail averaged to one even alpha, just enough of those
  texels are turned on in 4×4 Bayer order.

Their PSNR is measured against the resized source cut the same way, so it scores colour; coverage is reported apart.

## Sky cube maps
GX has no cube maps. A cube a sky material uses (sort key sky / skybox) is decoded face by face from its IWI (the six
top-mip faces are stored back to back in D3D order and end the top-mip range) and written as six 2D textures
`<name>_px`, `_nx`, `_py`, `_ny`, `_pz`, `_nz` (+X, −X, +Y, −Y, +Z, −Z) in the sky role, for the renderer to draw as a
skybox. The sky role has no mips: the sky is always magnified at 480p. `mp_rust`'s sky: 1024² faces → six 512² CMPR
faces, 768 KiB in all, 41.0–43.6 dB. 256² faces would be 192 KiB if a map needs the room.

## Smooth alpha: CMPR colour + I4 alpha plane
A colour texture with smooth alpha (decals, stains, glass, foam), except UI, is stored *split*: the colour as opaque
CMPR (4 bpp) and the alpha as an I4 plane (4 bpp) of the same size and mip count. That is 8 bpp against RGB5A3's 16, and
better on both channels: RGB5A3 falls back to 4-4-4 colour and 3-bit alpha wherever alpha is not full, while the split
keeps CMPR's 565 colour endpoints and 16 alpha levels. UI stays RGB5A3, since CMPR's 4×4 blocks soften icon and text edges.

- `gx_texture::policy::Converted.alpha_plane` holds the I4 plane; `decode_level` merges both as the Wii shows them.
- `.gxt`: flag bit 0 in the header, then a second header (I4, same size and levels) and the plane; still 32-byte aligned.
- **Renderer (`gx_render`, later):** bind the colour map and the I4 map to two texture units; take colour from the first
  and alpha from the second (GX reads I4 as I,I,I,I) in the TEV.

## `mp_rust` (2026-11-13, retail x86 zone)
608 GfxImages: 16 in the zone, 592 streamed from IWIs. All found and decoded, 0 failures, about 47 s (release, 2 cores).
374 skipped: 171 normal maps, 179 specular maps, 15 lightmaps/probes, 7 cubes, 1 built-in, 1 used only as detail.
282 materials walked, every material texture link resolved.

| Step | Wii texture memory | CMPR | CMPR + I4 | RGB5A3 |
|---|---|---|---|---|
| Name-based roles (first run) | 11.72 MiB | 185 textures, 5,692 KiB | – | 49, 6,304 KiB |
| Roles and alpha from materials | 7.03 MiB | 207, 4,602 KiB | – | 26, 2,597 KiB |
| + cut-outs keep size and coverage | 7.56 MiB | 207, 5,146 KiB | – | 26, 2,597 KiB |
| + smooth alpha split into CMPR + I4 | 6.71 MiB | 207, 5,146 KiB | 19, 875 KiB | 7 (UI), 848 KiB |
| + minimap role | 6.46 MiB | 207, 5,146 KiB | 20, 1,131 KiB | 6 (UI), 336 KiB |
| **+ sky cube as six faces** | **7.21 MiB** | 213, 5,914 KiB | 20, 1,131 KiB | 6 (UI), 336 KiB |

Roles (material-based): world 65, prop 90, character 71, UI 7, effect 1. With name guesses they were world 177,
character 46, prop 6, viewmodel 3, effect 2, UI 1.

Split textures: the 19 decals, stains, glass and foam textures halved (1,750 → 874 KiB) and scored 5.1 dB better on
average (−4.5 to +10.5). The one that lost is a team icon drawn as an effect (42.3 → 37.8 dB): flat icon art suits CMPR
blocks less.

Alpha uses: 162 textures dropped alpha (opaque or additive material), 26 alpha-tested, 46 blended. The name guess was wrong
for 116 textures, mostly world → prop (78) and world → character (28).

Cut-outs (26 textures, 154 mip levels): coverage error with coverage kept is 0.1% mean, 1% worst level; a plain cut would
be 4.3% mean and 47% on its worst level. The 19 prop cut-outs (trees, bushes, grass, flags) doubled in size (e.g. a tree
card 64×128 → 128×256), which costs 544 KiB in all.

Quality (PSNR against the resized source; cut-outs against the same cut): character mean 41.1 dB, prop 40.6, world 38.6.

## `common_mp` (2026-11-13, retail x86 zone)
The shared multiplayer zone: weapons, HUD, menus, effects. 1,624 GfxImages, all streamed from IWIs (2 of them wavelet),
**0 failures**, about 15 s. 1,808 materials and 736 models walked, every link resolved; 292 models are weapon gun or hand
models. 1,249 textures encoded into **46.9 MiB**. That is over half of the Wii's ~88 MB, so it cannot all be resident.

| Role | Textures | Wii MiB | At cap 256 | At cap 128 |
|---|---|---|---|---|
| UI | 903 | 30.1 | 19.8 | 9.9 |
| viewmodel | 126 | 14.0 | 4.3 | 1.2 |
| effect | 134 | 1.5 | | |
| prop | 74 | 0.8 | | |
| world, sky | 10 | 0.3 | | |

UI by kind (from names, for sizing only):

| UI kind | Textures | Wii MiB |
|---|---|---|
| full-screen overlays (scopes, AC-130, gas mask, ...) | 44 | 8.2 |
| calling cards and emblems (menus only) | 430 | 8.1 |
| weapon icons | 94 | 5.5 |
| perk and killstreak icons | 86 | 5.2 |
| HUD, compass, other | 249 | 3.1 |

815 of the 903 UI textures are RGB5A3 (27.9 MiB). The viewmodel figure covers every MP weapon; one player only ever
holds a few of them at a time.

## Load by need
Every encoded texture gets a group saying when it has to be in memory, decided from what the zones say
(`material_links::Group`, merged over all materials that use the image):

| Group | Textures | Loaded |
|---|---|---|
| `match` | used by the world, by a model no weapon lists, by an effect, or by nothing in the zone (HUD drawn by script/code) | throughout a match |
| `weapon` | used only by models a weapon references (gun, hand, world, world clip, knife, rocket, projectile) or as the weapon's HUD icons (kill, d-pad) | while the weapon is in someone's loadout |
| `catalog` | material named in a string table: the zone's own and the shared ones in `code_post_gfx_mp` (calling cards, emblems, icon tables) | by name, when shown |
| `menu` | used only by menus | in the front end |

When materials disagree, `match` beats `weapon` beats `catalog` beats `menu`. `mwf-convert textures` reads
`code_post_gfx_mp`'s string tables before converting a zone, and writes `index.json` beside the `.gxt` files: per
texture its file, role, format, bytes, group and weapon names (converted data, git-ignored like the textures). The
content-free `report.json` holds the bytes per group, per weapon, and "match + the 8 heaviest weapons" (4 players ×
2 weapons, worst case).

| `common_mp` group | Textures | MiB |
|---|---|---|
| **match** | 407 | **8.2** |
| weapon | 225 | 19.1 (1,193 weapons: median 0.02, largest 3.75 each) |
| catalog | 540 | 13.2 (calling cards and emblems 7.3, weapon and perk icons 5.8) |
| menu | 77 | 6.4 |

Worst case in a match: **match + 8 heaviest weapons = 12.5 MiB** (from 46.9 resident), plus the few calling cards on
screen. `mp_rust`: match 6.4 MiB, catalog 0.35 MiB.

## Findings to act on
1. ~~Cut-out foliage suffers at the prop size cap.~~ Fixed: cut-outs get at least 256 and keep their coverage (above).
2. ~~Smooth alpha is over a third of the memory.~~ Fixed: split into CMPR + I4 (above). Needs the two-texture TEV
   recipe in the renderer.
3. ~~The minimap is the largest texture (UI, 512² RGB5A3, 512 KiB).~~ Fixed: the material a map script passes to
   `setupMiniMap(...)` gets the `minimap` role. It keeps 512 (the HUD shows a zoomed part of it at about 1:1 at 480p) and
   no mips, but is split into CMPR + I4 like other pictures: 256 KiB, and 38.2 dB instead of 29.6 (RGB5A3's 4-bit colour
   was the loss).
4. ~~Sky: the `mp_rust` sky is a cube map, skipped.~~ Converted as six faces (above). Most "cube or 3D" skips were in
   fact references to other zones' images (`,` names); they are labelled as such now.
5. ~~`common_mp` needs a residency plan (46.9 MiB).~~ Done: load by need (above), 13.7 MiB worst case in a match. The
   runtime still has to load groups from `index.json` (front end, loadout changes, calling cards on screen).
6. ~~About 3 MiB of `weapon_*` textures land in `match`.~~ Fixed: every model referenced while a weapon loads (world
   clip, knife, rocket and projectile models too, which IW4L loads without a name) belongs to that weapon; 1.6 MiB moved.
   The 2.1 MiB left is correct or not linkable from zone data: 22 HUD/menu weapon icons drawn by script (1.2 MiB), weapon
   skins shared with back-slung "stow" models and the S&D bomb, which scripts attach (0.8 MiB), and small props.

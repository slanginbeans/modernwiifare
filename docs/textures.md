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
| map type not 2D, or depth > 1 (cubes, 3D) | skipped: not supported yet |
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
| camera region "depth hack" | viewmodel |
| sort key sky / skybox | sky |
| used by the GfxWorld | world |
| used by an XModel whose skeleton has human bones (`j_spinelower`, `j_head`, ...) | character |
| used by any other XModel | prop |
| used by Fx / ImpactFx / FxWorld | effect |
| no user in the zone, technique set `2d` (IW4 names may start with `,`) | UI |
| no user in the zone, `effect*` technique set or emissive camera region | effect |

**Alpha** from the material's state bits, applied to the pixels before encoding:
| Material state | Texture alpha |
|---|---|
| any pass blends with source/destination alpha | kept (RGB5A3 when smooth) |
| alpha test (> 0, < 128, ≥ 128) | 1 bit with the full-size coverage kept at every mip level (below), so CMPR |
| opaque or additive (ONE/ONE) | dropped, so opaque CMPR |

In the retail x86 zones every world, model and effect is a top-level entry, so the crediting is exact there. In IW4x's
x64 rewrites, models nest inside the world and would count as world.

## Alpha-tested (cut-out) textures
Foliage, fences and gratings are mostly detail thinner than a texel once downscaled. Cutting the averaged alpha at a
fixed level thins them out, worst in the small mip levels used at a distance. `gx_texture::policy::convert_with` with
`Options { cutout: Some(threshold) }` therefore:
- caps the texture at `Role::cutout_max_dim()`: at least 256, so props and effects are not cut to 128;
- builds the mips from the smooth alpha, then cuts each level so that the same fraction of texels passes the alpha
  test as at full size (`image::cut_to_coverage`). Where fine detail averaged to one even alpha, just enough of those
  texels are turned on in 4×4 Bayer order.

Their PSNR is measured against the resized source cut the same way, so it scores colour; coverage is reported apart.

## `mp_rust` (2026-11-13, retail x86 zone)
608 GfxImages: 16 in the zone, 592 streamed from IWIs. All found and decoded, 0 failures, about 47 s (release, 2 cores).
374 skipped: 171 normal maps, 179 specular maps, 15 lightmaps/probes, 7 cubes, 1 built-in, 1 used only as detail.
282 materials walked, every material texture link resolved.

| | Name-based roles (first run) | Material-based roles and alpha | + cut-outs keep size and coverage |
|---|---|---|---|
| Textures encoded | 235 | 234 | 234 |
| **Wii texture memory** | **11.72 MiB** | **7.03 MiB** | **7.56 MiB** |
| CMPR | 185 textures, 5,692 KiB | 207 textures, 4,602 KiB | 207 textures, 5,146 KiB |
| RGB5A3 | 49 textures, 6,304 KiB | 26 textures, 2,597 KiB | 26 textures, 2,597 KiB |
| Roles | world 177, character 46, prop 6, viewmodel 3, effect 2, UI 1 | world 65, prop 90, character 71, UI 7, effect 1 | same |

Alpha uses: 162 textures dropped alpha (opaque or additive material), 26 alpha-tested, 46 blended. The name guess was wrong
for 116 textures, mostly world → prop (78) and world → character (28).

Cut-outs (26 textures, 154 mip levels): coverage error with coverage kept is 0.1% mean, 1% worst level; a plain cut would
be 4.3% mean and 47% on its worst level. The 19 prop cut-outs (trees, bushes, grass, flags) doubled in size (e.g. a tree
card 64×128 → 128×256), which costs 544 KiB in all.

Quality (PSNR against the resized source; cut-outs against the same cut): character mean 40.8 dB, prop 40.1, world 38.1.

## Findings to act on
1. ~~Cut-out foliage suffers at the prop size cap.~~ Fixed: cut-outs get at least 256 and keep their coverage (above).
2. **Smooth alpha is still over a third of the memory.** 26 RGB5A3 textures (decals, stains, foam, HUD icons) use 2.5 of
   7 MiB. Options: cap blended world decals one size lower, or store colour as CMPR plus alpha as I4 (8 bpp total) and
   combine them in the TEV recipe.
3. **The minimap is the largest texture** (UI, 512² RGB5A3, 512 KiB). It is real HUD art; consider 256² or I4/IA4.
4. **Sky and effects are mostly absent from this zone.** The `mp_rust` sky is a cube map (skipped for now), and most
   effect images live in `common_mp`. Converting the shared zones comes next.

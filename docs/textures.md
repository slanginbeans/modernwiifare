# Zone textures on the Wii

`mwf-convert textures <game folder> [--map <zone>]` takes every GfxImage a map zone loads, gets its pixels from the zone
or from `images/<name>.iwi` in `main/*.iwd`, and encodes it with `shared/gx_texture` (`converter/src/zone_textures.rs`).
The `.gxt` files and `report.json` go to the git-ignored `out/textures/<zone>/`. The report holds counts, bytes and
quality only. `--verbose` prints one line per texture, with its name, to the console only.

## What is converted
| GfxImage | Decision |
|---|---|
| semantic normal map (5) | skipped: baked into lighting |
| semantic specular map (8) | skipped |
| name `*...` (lightmaps, reflection probes) | skipped here: converted with the world |
| name `$...` (`$white`, ...) | skipped: made on the Wii |
| map type not 2D, or depth > 1 (cubes, 3D) | skipped: not supported yet |
| everything else | encoded with a role from the image name (**provisional**, see below) |

Sources: in-zone D3D formats DXT1/3/5, BGRA8, BGRX8, A8, L8, LA8; IWI v8 formats 1–5 and DXT1/3/5. Wavelet IWIs and BC5
are not decoded yet (none were needed for `mp_rust`). Identical sources are encoded once per role.

## First run: `mp_rust` (2026-11-13, retail x86 zone)
- 608 GfxImages: 16 in the zone, 592 streamed from IWIs. **All found and decoded, 0 failures.** 47 s (release, 2 cores).
- 373 skipped: 171 normal maps, 179 specular maps, 15 lightmaps/probes, 7 cubes, 1 built-in.
- 235 encoded into **11.7 MiB** of Wii texture memory (sources: 341 MiB as RGBA8 at full size).

| Wii format | Textures | KiB | Source |
|---|---|---|---|
| CMPR | 185 | 5,692 | 175 DXT1 (opaque), 9 DXT3/5 with cut-out alpha, 1 RGB8 |
| RGB5A3 | 49 | 6,304 | DXT3/DXT5 with smooth alpha |
| IA4 | 1 | 1 | grey with alpha |

Quality (PSNR against the resized source, visible colour + alpha): world mean 39.4 dB, character 40.3, viewmodel 41.0.

## Findings to act on
1. **Smooth alpha costs half the memory.** 49 RGB5A3 textures (decals, stains, foam, fences) use 54% of the map's texture
   memory: RGB5A3 is 16 bpp against CMPR's 4. Options: cap them one size lower, or store colour as CMPR plus alpha as I4
   (8 bpp total) and combine them in the TEV recipe. Decide with the material work.
2. **Thin cut-out textures suffer from the downscale.** A metal floor grating (512 → 256) scores 18.4 dB: its fine alpha
   grid blurs on resize and CMPR's 1-bit alpha then cuts it badly. Cut-out textures (gratings, fences, foliage) may need to
   keep their size or use a sharper alpha resize.
3. **Roles should come from materials, not names.** The map's minimap image is the largest single texture (UI role,
   512² RGB5A3, 512 KiB). Name-based roles are a stopgap. The next step is to walk material → techset → image and the
   world/xmodel/viewmodel users of each material.
4. **Does alpha mean transparency?** Some colour maps carry other data in alpha. Only the material (alpha test/blend vs
   opaque technique) can tell, and an opaque material lets its texture go to CMPR.

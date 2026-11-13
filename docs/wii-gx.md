# Wii GX notes for the renderer

Facts the converter and `gx_render` rely on, with sources. Researched 2026-11-13. "(uncertain)" means inferred or folklore;
check on hardware.

Sources: libogc [`gx.h`](https://github.com/devkitPro/libogc/blob/master/gc/ogc/gx.h) /
[`gx.c`](https://github.com/devkitPro/libogc/blob/master/libogc/gx.c), [Dolphin](https://github.com/dolphin-emu/dolphin),
[WiiBrew memory map](https://wiibrew.org/wiki/Memory_map), [WiiBrew display issues](https://wiibrew.org/wiki/Display_Issues).

## Display lists (built by the converter, called by the runtime)
- Buffer 32-byte aligned, length a multiple of 32, tail padded with `0x00` (GX NOP). `GX_CallDispList(ptr, len)` emits
  `0x40, phys addr u32, len u32`; MEM2 addresses are fine (physical `0x1xxxxxxx`).
- **`DCFlushRange(ptr, len)` after loading from SD and before the first call.** Dolphin does not model the CPU cache, so a
  missing flush works in Dolphin and breaks on hardware.
- Draw command bytes (big-endian): `prim | vtxfmt` then `u16 vertex count`, then per vertex the attributes in this order:
  PNMTXIDX, TEXnMTXIDX, POS, NRM, CLR0, CLR1, TEX0..7. INDEX8 = 1 byte, INDEX16 = BE u16, direct = components.
  Primitives: quads `0x80`, triangles `0x90`, strip `0x98`, fan `0xA0`, lines `0xA8`, line strip `0xB0`, points `0xB8`.
- Keep only draw commands in prebuilt lists; set VCD/VAT/arrays in code before calling. Avoid index `0xFFFF` (uncertain:
  Dolphin may treat the max index as a skip).
- Max 65535 vertices per draw (u16 count).

## Vertex formats
- `GX_SetVtxDesc(attr, GX_NONE|DIRECT|INDEX8|INDEX16)`, `GX_SetVtxAttrFmt(fmt, attr, comp, size, frac)`, 8 VAT slots.
- Positions `GX_POS_XYZ` + `GX_S16` with frac 0–31; texcoords `GX_TEX_ST` + `GX_S16` with frac; normals S8 (frac 6) or S16
  (frac 14); colours RGB565 / RGB8 / RGBA4 / RGBA6 / RGBA8.
- `GX_SetArray(attr, ptr, stride)`: stride is a u8 (≤ 255); 32-byte alignment helps. Flush arrays; `GX_InvVtxCache()` when
  arrays change.

## TEV recipes
Stage: `out = (d + (a·(1−c) + b·c) + bias) · scale`. 16 stages, 8 texmaps, 8 texgens, 4 konst colours.
`GX_SetTexCoordGen(GX_TEXCOORDn, GX_TG_MTX2x4, GX_TG_TEXn, GX_IDENTITY)` passes texcoords through.

| Recipe | Setup |
|---|---|
| diffuse × lightmap × 2 | 2 texgens, 0 chans. S0: (TEXCOORD0, TEXMAP0) C=TEXC, A=TEXA. S1: (TEXCOORD1, TEXMAP1) C = CPREV·TEXC (`ColorIn(ZERO, CPREV, TEXC, ZERO)`), `CS_SCALE_2`, A=APREV |
| CMPR colour + I4 alpha | S0: (TEXCOORD0, TEXMAP0) C=TEXC. S1: (TEXCOORD0, TEXMAP1) C=CPREV, A=TEXA. I4 decodes as I,I,I,I, no swap table |
| cut-out | `GX_SetAlphaCompare(GX_GREATER, ref, GX_AOP_AND, GX_ALWAYS, 0)` + `GX_SetZCompLoc(GX_FALSE)` (Z after texturing); restore `ZCompLoc(GX_TRUE)` for opaque |
| alpha blend | `GX_SetBlendMode(GX_BM_BLEND, GX_BL_SRCALPHA, GX_BL_INVSRCALPHA, GX_LO_CLEAR)`, `GX_SetZMode(TRUE, LEQUAL, FALSE)` |
| additive | `GX_BM_BLEND, GX_BL_ONE, GX_BL_ONE` (or SRCALPHA, ONE) |

DSTALPHA blending needs the `GX_PF_RGBA6_Z24` EFB format.

## Textures
- Data 32-byte aligned, ≤ 1024², power of two for mips/repeat. `GX_InitTexObj` + `GX_InitTexObjLOD` (filters, LOD 0–10),
  `GX_LoadTexObj` into TEXMAP0–7. MEM2 textures are fine. Flush after loading.
- TMEM is a 1 MB cache; sort draws by texture. `GX_InvalidateTexAll` costs about 512 GP clocks.
- Dolphin decodes CMPR with 3/8–5/8 interpolation and colour 3 = average + transparent when c1 ≤ c2. `gx_texture::cmpr`
  matches this (`BLEND_NEAR`).

## Camera
GX is right-handed, looks down −Z, +Y up (OpenGL style); `Mtx` is a row-major 3×4 times column vectors. IW4 is Z-up
(X forward, Y left), also right-handed: use `guLookAt(eye, up = (0,0,1), eye + forward)`, or map engine view space to
GX view as `x_gx = −y, y_gx = z, z_gx = −x`. Check `GX_SetCullMode(GX_CULL_BACK)` winding with a test triangle
(uncertain which way GX treats front faces).

## Frame, video, memory
- EFB max 640×528: render 640×480. `GX_CopyDisp` to one of two XFBs, then `VIDEO_SetNextFramebuffer/Flush/WaitVSync`.
  Non-blocking completion: `GX_SetDrawDone` + callback, or draw-sync tokens.
- 480p via `VIDEO_GetPreferredMode` when progressive is enabled; 16:9 when `CONF_GetAspectRatio() == CONF_ASPECT_16_9`
  (anamorphic 640×480, aspect 16/9 in `guPerspective`).
- Planning budget (estimate, measure on hardware): 50–150k textured triangles per frame at 30 fps from display lists.
  Fill rate about 972 Mpixel/s theoretical.
- MEM1 24 MB (keep FIFO, XFBs, heaps there); MEM2 usable about `0x90000800`–`0x933E0000` (~51–52 MB, IOS-dependent) for
  textures, geometry and display lists. Use our own MEM2 heap; do not rely on newlib malloc spanning both.

## Dolphin automation
`Dolphin.exe -b -u <user dir> -e game.dol -C Main.Interface.UsePanicHandlers=False -C Main.Interface.ConfirmStop=False`
(portable build under `D:\rust\wii-tools` on the dev PC). Frames as PNGs: `-C Main.Movie.DumpFrames=True
-C GFX.Settings.DumpFramesAsImages=True` → `<user>/Dump/Frames`. Batch mode still opens a render window. The .dol should
exit after N frames for unattended runs. Dolphin does not model the CPU cache, TMEM timing or GPU throughput.

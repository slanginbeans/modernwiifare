---
name: wii-platform
description: Nintendo Wii hardware budgets and homebrew toolchain (devkitPPC, libogc, GX/TEV, WPAD, IOS networking, Dolphin, wiiload). Use when writing or reviewing anything in src/, choosing data formats the runtime will consume, setting memory/perf budgets, or testing on Dolphin or real hardware.
---

# Wii platform guide

## Hardware facts (design to these)
| Part | Spec | What it means for us |
|---|---|---|
| CPU "Broadway" | PowerPC 750CL @ 729 MHz, 32 KB L1 I/D, 256 KB L2, **paired-single** FPU | Big-endian, like Xenon. Use paired singles (`ps_*` intrinsics/asm) for skinning, vector math and culling. Keep hot loops cache-sized. No SIMD beyond paired singles. |
| MEM1 | 24 MB 1T-SRAM (fast) | Code, heaps, game state, display lists, hot geometry |
| MEM2 | 64 MB GDDR3, the top part reserved by IOS (assume ~52 MB usable; measure with `SYS_GetArena2Hi/Lo`) | Textures, map geometry, animation, sound banks |
| GPU "Hollywood" | 243 MHz, fixed-function **TEV** (16 stages, 8 tex maps), 3 MB eDRAM (≈2 MB EFB + 1 MB texture cache) | No programmable shaders. Every IW4 material must collapse into a TEV recipe. EFB max 640×528, so the target is 640×480 (or 854×480 anamorphic for 16:9). |
| Video | 480i/480p, 50/60 Hz | Target **30 fps render**. Run the sim at a fixed rate (decide via ADR). |
| Audio | DSP with hardware ADPCM decoding. ASND/AESND in libogc mix in software on the DSP | Keep ~16–24 voices. Store sounds as DSP-ADPCM or 16-bit PCM at 22–32 kHz. |
| Network | **USB LAN adapter (primary, wired)**, built-in 802.11b/g as a fallback. BSD-like sockets through IOS (`net_init`, `net_socket`, ...) | Socket calls go through IPC to IOS, so they are slow. Batch sends and avoid per-packet syscalls in the frame loop. |
| Storage | SD (front slot) and USB via libfat | Packs are read from **`sd:/modernwiifare/`** by default. The Wii has only 2 USB ports, and the LAN adapter takes one, so USB storage and a USB controller compete for the other. Stream big assets. SD reads are only a few MB/s, so budget load times. |

## Texture formats (GX)
- `CMPR` is 4 bpp and DXT1-like with 1-bit alpha. Use it for most diffuse/lightmap data.
- `I4`/`I8`/`IA4`/`IA8` are intensity maps. Use them for alpha masks, detail maps and spec masks.
- `RGB5A3` is 16 bpp with 3-bit alpha. Use it for real alpha textures (foliage, UI, FX). There is **no DXT5**.
  As an alternative, use CMPR colour plus an I4 alpha sampled in a second TEV stage.
- `RGB565` is for UI where CMPR artefacts would show. Use `RGBA8` sparingly.
- Normal maps are not practical at scale. Bake them into lighting, or use the GX "bump" indirect stage on a few hero surfaces only.
- Texture dimensions are powers of two, up to 1024. Keep most world textures at 128–256 and the viewmodel at 256–512.

## Geometry
- Quantize vertices. Positions go to s16 with a per-chunk scale, normals to s8, UVs to s16. Use indexed vertex attributes from display lists (`GX_BeginDispList`).
- Bake static world geometry into display lists per cell/cluster at **convert time**, not at runtime.
- Skinned meshes have a GX matrix palette of 10 position matrices per draw. Split meshes by bone set in the converter,
  or CPU-skin with paired singles into a ring buffer (flush with `DCFlushRange`).

## Toolchain
- Install devkitPro `devkitPPC` and `libogc` (`dkp-pacman -S wii-dev`). CI uses the `devkitpro/devkitppc` Docker image.
- The runtime is **Rust**: nightly + `rust-src`, `-Z build-std=core,alloc`, a custom PowerPC EABI target JSON for the 750CL
  (hard float, big-endian), and the devkitPPC linker. libogc is reached through `ogc-sys`/`ogc-rs` (github.com/rust-wii).
  The `iw4l-upstream` skill covers how IW4L crates fit into this.
- Small C shims are fine where libogc macros don't bind cleanly (for example GX FIFO helpers). Build them with `-mrvl -mcpu=750 -meabi -mhard-float`.
- Input (ADR 0005): **`PAD` GameCube controller is the target.** USB gamepads go through libogc's USB API (IOS58) as a stretch goal. Wiimote/Classic via `WPAD` are optional extras.
- Run on Dolphin for fast iteration (`tools/` will hold a launch script). On real hardware, launch through the Homebrew Channel
  and push builds with `wiiload` over the network.

## Golden rules
1. Every converter output format is designed for **zero runtime conversion**. The runtime only memcpys, maps, or DMAs it.
2. Every subsystem gets an explicit memory budget in `docs/budgets.md`, checked in debug builds.
3. Profile on real hardware before calling something "fast". Dolphin timing is not representative, especially for IPC and GX FIFO.
4. Never `malloc` in the frame loop. Use per-frame linear allocators plus fixed pools.

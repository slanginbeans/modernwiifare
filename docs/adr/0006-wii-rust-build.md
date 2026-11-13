# ADR 0006: Wii Rust build uses the stock PPC32 target, linked by devkitPPC through a thin C shim

Status: **accepted for Phase 0; confirm after Spike A on hardware** (2026-11-13)

## Decision
- Rust Wii code compiles as a `no_std` **staticlib** for the stock target `powerpc-unknown-linux-gnu`,
  with `-C target-cpu=750 -C relocation-model=static` (`.cargo/config.toml`), on **stable** Rust (pinned in `rust-toolchain.toml`).
- devkitPPC's `powerpc-eabi-gcc` links it with libogc into an ELF, and `elf2dol` turns that into a `.dol`.
- libogc is reached through a small hand-written C layer (`wii/platform/shim.h`), not ogc-rs/bindgen.

## Why
- The prebuilt `core`/`alloc` for that target are big-endian PPC32 with hard float, which is the same machine model as Broadway.
  This avoids nightly, `-Z build-std` and a custom target JSON. It matches ADR 0001's "stay on stable" for host crates.
- Many GX calls are `static inline` FIFO writes in libogc headers, which bindgen can't bind usefully. A C shim is simpler
  and faster, and gives Rust a stable, narrow API.
- ogc-rs has not had a release since 2022. We can still adopt it later behind the same shim API.

## Known differences and how they're handled
| Issue | Handling |
|---|---|
| The prebuilt `core` has unwind landing pads referencing `_Unwind_Resume` / `rust_eh_personality` | Built with `panic = "abort"`. Weak stubs in `shim_common.c` satisfy the linker and are never reached. |
| SysV (16-byte) vs EABI (8-byte) stack alignment | Rust code keeps 16-byte alignment internally. Calls into C are compatible. The Spike A self-test `hard-float FFI args` checks argument passing. |
| No PIC/GOT allowed in a fixed-address `.dol` | `relocation-model=static`. |
| Heap | Rust's global allocator calls newlib `memalign`/`free` (MEM1 arena). Later phases replace this with budgeted pools. |

## Verified so far
- The staticlib builds and passes clippy for the target. Its only external symbols are newlib (`memalign`, `free`, `memcpy`,
  `memset`), the `mwf_*` shim, and the two weak unwind stubs (checked with `llvm-nm`).
- Not yet verified: the devkitPPC link (CI `wii` job) and running on hardware (Spike A).

## Fallback
If the link or ABI proves unsound: switch to a custom `powerpc-unknown-eabi` target JSON with nightly + `-Z build-std=core,alloc`.
The shim API stays the same.

# Phase 0 spikes: how to run them

## Get the builds
Each push builds both spikes in GitHub Actions (job `wii`). Open the run, download the artifact
`modernwiifare-spikes-<sha>`, and unzip it. Inside `hbc/apps/` are two folders:

```
mwf_spike_a_gx/   boot.dol  meta.xml
mwf_spike_b_net/  boot.dol  meta.xml
```

Copy both folders to `SD:/apps/` and launch from the Homebrew Channel. Alternatively, with devkitPro installed locally:

```bash
make -C wii hbc
```

```bash
WIILOAD=tcp:192.168.1.50 make -C wii run-b
```

(`wiiload` sends the `.dol` straight to a Wii sitting in the Homebrew Channel. Replace the IP with your Wii's.)

## Spike A: Rust toolchain + GX
Run it on one Wii with a GameCube controller in port 1.

1. A text screen lists the self-tests, each `OK` or `FAIL`, plus free MEM1/MEM2. It also runs IW4L's movement code
   for 1,200 ticks and shows the time taken and the result hash. **IW4L movement matches x86 reference** must say `OK`
   (this is Spike E on real hardware).
2. Press **A** (or wait 10 s). A spinning coloured tetrahedron appears.
   - Main stick rotates it, C-stick moves it, **A** cycles colours, **START**/**RESET** exits to the HBC.
   - The background is **green** if all self-tests passed, **red** if any failed.

Record in `docs/spikes/results.md`: the self-test screen (a phone photo is fine), the background colour, whether the scene is
smooth, whether the controls respond, whether START returns to the HBC, and the MEM1/MEM2 numbers. Note 4:3 vs 16:9 and the TV mode.

## Spike D: converter reads your IW4x install
Runs on your PC, not the Wii. It needs Rust (stable, see `rust-toolchain.toml`). Two commands:

```bash
cargo run -p mwf_convert --release -- inventory "D:/path/to/Call of Duty Modern Warfare 2"
```

```bash
cargo run -p mwf_convert --release -- map "D:/path/to/Call of Duty Modern Warfare 2" --map mp_rust
```

`inventory` (or double-click `inventory.bat`) lists every zone's sizes and asset counts into `out/inventory.json`.
`map` walks one map zone (`mp_rust` by default, or `--zone <file.ff>`) and measures each asset's bytes per type into
`out/map_<zone>.json`. It prefers the retail x86 zone in `zone/iw4x/x86/english/` and falls back to `zone/english/`.
Both reports contain sizes and counts only, never names or content.

*Windows without Visual Studio:* build for `x86_64-pc-windows-gnullvm` with [llvm-mingw](https://github.com/mstorsjo/llvm-mingw)
(UCRT) as linker and C compiler (IW4L's `diag` pulls in mimalloc, which is C). The plain `windows-gnu` toolchain
links, but its binaries crash at start-up here.

## Spike B: system link discovery
*Deferred to Phase 5. The game comes first. The build is kept in CI so it can be run any time.*

Run it on **2 or more Wiis** with USB LAN adapters on the same switch/router at the same time.

What each screen shows:
- This Wii's IP, mask and subnet broadcast, plus whether `SO_BROADCAST` and `bind` returned 0 (ok).
- Send counters for the two broadcast styles: **limited** (255.255.255.255) and **directed** (e.g. 192.168.1.255).
- A peer table: `LIM%` and `DIR%` are the percentage of that peer's beacons received over each style.
  RTT is min/avg/max for the unicast reply. `BUILD` should say `same`.

Pass criteria:
- [ ] Every Wii lists every other Wii within ~2 s of starting.
- [ ] At least one broadcast style shows ~100% on wired LAN. Record which one(s).
- [ ] RTT on wired LAN averages a few ms (record min/avg/max).
- [ ] Runs for 10 minutes without send/recv errors climbing.
- [ ] Optional: repeat with one Wii on Wi-Fi.
- [ ] Optional: two Dolphin instances on one PC. Record whether they see each other.

If a Wii shows `Network init FAILED`, note the number shown, and check the adapter in Wii Settings → Internet.

Record results in `docs/spikes/results.md` (a template is already there).

# Modern Wiifare

A non-commercial fan demake of the **multiplayer** of Modern Warfare 2 (2009) for the Nintendo Wii, with 4-player Wii-to-Wii system link (USB LAN adapter), played on the GameCube controller.
Built on [IW4L](https://github.com/vladtrc/iw4L) (Apache-2.0).

This repository contains **no game code or data**. You need your own copy of the game. A host-side converter turns your
install into Wii-sized data packs on your machine. Not affiliated with or endorsed by Activision or Infinity Ward.

- Plan: [ROADMAP.md](ROADMAP.md)
- Rules for contributors (and Claude): [CLAUDE.md](CLAUDE.md)
- Third-party references and licenses: [docs/references.md](docs/references.md)

## Building (Windows)
Double-click **`build.bat`**. It updates from GitHub, builds, and puts the Homebrew Channel apps in `dist\apps`.
Copy that `apps` folder to the root of the SD card.
It needs devkitPro with "Wii Development" ticked (it offers to install Rust for you), or Docker Desktop.
`build.bat noupdate` skips the update and `build.bat clean` deletes the output.

On Linux/macOS with Docker: `./build.sh`. No installs at all: download `modernwiifare-sd-card` from the newest
**ci** run under the repo's Actions tab.

No game files are needed for the current builds. Later, the converter will read your own MW2/IW4x install on your PC,
and its output stays on your PC and SD card.

Status: Phase 0 (foundations). See the roadmap.

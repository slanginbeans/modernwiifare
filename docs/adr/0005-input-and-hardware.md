# ADR 0005: GameCube controller first, USB gamepads as a stretch goal, wired LAN

Status: **accepted** (2026-11-13)

## Test hardware
Several Wiis with the Homebrew Channel, each with a **USB LAN adapter**. Wired LAN is the supported system-link setup.
Wi-Fi is best-effort.

## Input
1. **GameCube controller (`PAD`): the primary and fully supported layout.** It must be complete and tuned before anything else ships.
   - Default layout mirrors the 360 defaults where possible: main stick = move, C-stick = look, R analog = fire,
     L analog = ADS, A = jump, B = crouch/prone (hold), X = reload/use, Y = switch weapon, Z = lethal/tactical,
     D-pad = killstreaks/attachment, Start = menu, main-stick click is unavailable so sprint = double-tap/full-tilt (decide in Phase 2).
   - The C-stick is small, so look curves, deadzones and aim assist get tuned specifically for it (`docs/input.md`).
2. **USB gamepads: a stretch goal (Phase 6).** Use libogc's USB API (requires IOS58, which the Homebrew Channel provides).
   - Constraint: the Wii has **2 USB ports**, and the LAN adapter takes one, so there is at most one USB pad per Wii, and then no USB storage.
     Packs therefore live on **SD**.
   - Start with standard HID-class gamepads (generic report-descriptor parsing). Device families with their own protocols
     (for example vendor-class XInput pads, or pads needing a special init) are added per device, behind a small driver table.
   - Spike G (Phase 0, timeboxed to 2 days): enumerate a USB pad next to the LAN adapter and read input reports.
3. Wiimote/Nunchuk/Classic (`WPAD`) is not planned. It can be added later through the same input abstraction.

## Consequences
- The runtime input layer maps every device into IW4L `input_iw4` usercmds through one binding table. Devices never leak into sim code.
- Controls UI shows GameCube glyphs by default.

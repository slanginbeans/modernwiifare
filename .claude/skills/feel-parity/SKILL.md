---
name: feel-parity
description: Measure and preserve MW2's game feel in modernwiifare — movement, weapon handling, ADS, recoil, aim assist, timings — by comparing against a reference build running from the user's own copy. Use when implementing or tuning pmove, weapons, camera, input or animation timing, or when a change might alter how the game plays.
---

# Feel parity

"Feel" here means measurable things. Do not tune by eye alone.

## Reference build
There are three references, in order of trust:
1. The user's **Xbox 360 disc** under **mw2-recompiled**. This is the original console code, and the closest to the console pad feel we want.
2. The user's **PC install under IW4x**. Same engine and data, useful for quick captures.
3. **IW4L on the host.** Our direct parent. Wii vs IW4L differences must be zero. IW4L vs 360 differences are upstream issues.
Only observe behaviour and capture numbers. **No code** from mw2-recompiled (GPL-3.0, ADR 0004).

## What to match, in priority order
1. **Movement**: ground speed per weapon class, acceleration and friction, jump height and air control, sprint duration and recovery,
   crouch/prone transition times, mantle, ladder, fall damage.
   Capture position/velocity traces at fixed inputs and compare the curves.
2. **Weapon timings**: ADS in/out time, fire rate, reload (tactical/empty), raise/drop/sprint-out times. Read them **from the weapon
   defs in the user's data**, so the converter carries them over exactly.
3. **Recoil and spread**: hip spread by stance and movement, ADS spread, view kick patterns, idle sway.
4. **Damage model**: damage, range falloff, hit-location multipliers, penetration. All of these are data-driven, so keep them lossless.
5. **Camera**: FOV (MW2 console ≈65, verify), viewmodel FOV, bob, landing dip.
6. **Readability**: hit markers, footstep audio, killcam timing.

## Rules
- The simulation runs at a fixed tick regardless of render fps. A 30 fps render must not change weapon or movement timings.
- Keep pmove and weapon code free of platform calls so it can be **unit-tested on the host**. Traces from the reference build
  are compared in CI against our host build. The trace files are generated locally and never committed, because they come from game data.
- Input mapping is the one area where the Wii **must** differ. The **GameCube pad** is the target. Its octagonal-gate main stick and
  small C-stick need response curves and deadzones that land close to the 360 pad's aim feel. The analog triggers (ADS/fire) need
  thresholds. Track every parameter, plus aim assist, in `docs/input.md`, and tune with real players.
- For every intentional deviation (for example, killstreaks cut for performance), write a line in `docs/deviations.md`.

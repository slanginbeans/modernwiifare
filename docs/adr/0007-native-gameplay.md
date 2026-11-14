# ADR 0007: Native gameplay on IW4L's crates; script values read as data

Status: **accepted** (2026-11-14). Supersedes [ADR 0002](0002-gameplay-scripts.md). Records what the code already does.

## Decision
- Gameplay runs as **native Rust** in `wii/game_core` (`no_std`, host-tested), built directly on IW4L's `no_std` crates:
  `movement_iw4` (pmove), `weapon_iw4` (weapon state machine), `clipmap_iw4` / `trace_iw4` (collision), `hud_iw4`,
  `playerstate_iw4`, `input_iw4` and the rest of the set checked by `wii/iw4l_probe`.
- MW2's game-type and perk **scripts are not run** on the Wii. The converter reads their **values** at convert time
  (`converter/src/rules.rs`): health and regen, mode limits and points, perk damage, UAV, EMP and nuke timings, flash and
  stun durations, and so on. The values go into the pack (`wpk::rules`, `RULE`). The scripts never leave `out/`.
  Where a script yields nothing, MW2's stock value is used, and the pack records which values were found.
- IW4L's `sim` crate is **not** lifted off bevy_ecs. `game_core` owns the match loop (fixed 60 Hz player steps, bots at
  IW4's 50 ms server step) and calls IW4L's functions the way IW4L's `sim::step` does.

## Why
- The 729 MHz CPU has to fit rendering, five bots and the player in a frame. A general GSC interpreter (IW4L's runtime
  is ~23k lines on Bevy) adds interpreter cost and a large heap for logic that is mostly fixed rules.
- Reading numbers from the user's own scripts keeps MW2's tuning without shipping or interpreting the scripts.
- Every system can be unit-tested on the host, and the same code builds for the Wii.

## Consequences
- Each game mode and killstreak is ported by hand, in the order in `ROADMAP.md`. The script it mirrors is named in the code.
- Spike F (GSC IR size and heap) is dropped.
- Feel parity rules are unchanged: movement and weapon code are IW4L's own, and changes still need a trace
  (`feel-parity` skill).
- Behaviour that can't come from script values (spawn logic, capture rules, killstreak flow) follows the script's logic
  by reading it as reference. The script text itself is never copied into the repository.

## Revisit if
- A mode or feature needs so much scripted logic that porting it is impractical. Then consider running that one
  script through IW4L's runtime on the host to generate data, never on the Wii.

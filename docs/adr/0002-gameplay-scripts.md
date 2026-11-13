# ADR 0002: Run the user's own GSC through IW4L's script runtime

Status: **accepted, pending the memory measurement in Phase 0 Spike F** (2026-11-13)

## Decision
IW4L already compiles the user's own MP GSC rawfiles (from `common_mp` and the map zones) into an IR and runs them inside
`sim::script` (~23k lines). Its design rule is "gameplay policy belongs to loaded GSC; Rust supplies primitives". We reuse it
and do not hand-port game modes to native code.

## Plan
1. On the **host converter**, compile the GSC to IW4L's IR and serialise it into the pack. The Wii never parses GSC text.
   The scripts are user data, so the serialised IR is git-ignored like every other pack.
2. On the Wii, `wii_sim` hosts the IR interpreter without bevy_ecs.
3. If a mode's script cost is too high on Broadway (729 MHz), replace **individual hot natives or functions** with Rust.
   Log each one in `docs/deviations.md`.

## Risk / exit criteria
- Spike F: measure the IR size plus the runtime heap for `war` (TDM) on one map with 4 players. Budget: ≤ 3 MB of MEM1.
- If the measurement is far over budget, reopen this ADR and consider native modes for TDM/FFA only.

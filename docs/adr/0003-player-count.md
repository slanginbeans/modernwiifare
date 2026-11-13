# ADR 0003: 4 players

Status: **accepted** (2026-11-13)

- The target is **4 players over system link** (one player per Wii, listen-server host). Development steps are 2 → 4.
- Every budget (sim CPU, skinned characters, snapshot bandwidth, memory for player state) is sized for 4.
  Anything that only works at 4 is fine. Going above 4 is a later, separate ADR.
- Modes and spawn logic come from GSC unchanged. Small-team playlists (1v1 / 2v2 TDM, FFA, S&D) come first.

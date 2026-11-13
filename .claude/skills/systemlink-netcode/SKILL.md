---
name: systemlink-netcode
description: Design and implementation guide for Wii-to-Wii system link (LAN) in modernwiifare — discovery, lobby, IW-style snapshot/usercmd netcode, lag compensation, and testing with Dolphin and real hardware. Use when touching src/net, the lobby, or anything that changes the simulation's determinism or tick.
---

# System link netcode

## Model (mirror IW4's architecture, sized down)
- **Listen server**: one Wii hosts and simulates authoritatively. Clients send `usercmd`s at render rate (30 Hz) and
  receive delta-compressed **snapshots** at the server snapshot rate (take the starting value from the reference build and verify it).
- Clients predict locally with the **same pmove code** as the server. Reconcile on each snapshot.
- The server rewinds hitboxes for hitscan (lag compensation), as IW4 does. Remote players are interpolated ~1–2 snapshots behind.
- There is no host migration in v1. If the host leaves, the match ends.

## Transport
- The **USB LAN adapter (wired)** is the primary, supported setup. Wi-Fi is best-effort. UDP goes through IOS sockets. Discovery uses a UDP **broadcast** "who's hosting?" on a fixed port.
  The host replies with its name, map, mode, player count, build hash and pack hash.
- **Milestone 0 risk:** confirm early that broadcast and `SO_BROADCAST` work through libogc/IOS on real hardware, both over Wi-Fi
  and over the USB LAN adapter. If they don't, fall back to subnet-directed broadcast, then to manual IP entry.
- Size packets to stay under ~1200 bytes. Wireless b/g packet loss is real, so design snapshots to survive drops (delta against the last acked state).
- Socket calls are IPC into IOS and are expensive. Do one receive drain and one send per tick, and keep them off the GX critical path where possible.

## Lobby handshake (reject early)
`protocol version` → `build hash` (.dol) → `pack hash` (converted data) → `map present` → join.
A mismatched pack hash means the users converted different game versions or profiles. Show a clear message.

## Player counts
Start at 2 (1v1), then 4, then aim for the target in ADR 0003. Server CPU per extra player is mostly pmove plus hit tests,
and snapshot bandwidth grows with entity count. Measure both.

## Testing
- **Dolphin**: two instances on one PC. Verify early whether Dolphin's emulated socket layer passes the broadcast through. If it
  doesn't, use loopback IPs with manual connect for development.
- **Real hardware**: up to 4 Wiis with USB LAN adapters on one switch/router, launched through the Homebrew Channel. Push builds with `wiiload`.
  Wi-Fi is tested occasionally.
- Network conditioner: put a debug-build option on the host that injects latency, jitter and loss.
- Determinism checks: debug builds log per-tick player state hashes on client and server. A tool diffs them to find prediction mismatches.

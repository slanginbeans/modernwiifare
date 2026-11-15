# Testing on the Wii

Hardware tests produce a log on the SD card; `report.bat` turns it into a summary to paste back. No photos or
hand-written notes needed for numbers.

## Each test round
1. `build.bat` (builds, converts what changed, fills `dist\apps\mwf_game`). Copy `dist\apps\mwf_game` into the SD
   card's `apps` folder.
2. Play the scenarios below. Each match writes `apps\mwf_game\logs\NNNN.log` (written when you pause, die, end the
   match, or the game panics; never in the middle of play, because an SD write takes tens of milliseconds).
3. Put the card in the PC, double-click `report.bat`. The summary is printed, saved to `out\logs\summary.txt` and put
   on the clipboard: paste it into the chat, with one line per match on anything the numbers can't show (looked
   wrong, felt wrong, sounded wrong).

## Scenarios (one match each, ~2 minutes is enough)
| # | Map | What to do | What it measures |
|---|---|---|---|
| 1 | `mp_rust` | Default class. Stand at spawn 20 s, then walk the map, fight the bots | baseline frame time on a small map, bot cost |
| 2 | `mp_terminal` | Same | a heavy map (most triangles, many cells) |
| 3 | `mp_checkpoint` | Same | the biggest MEM2 user |
| 4 | any | Change class from the pause menu twice, die, respawn; switch weapons | weapon-file loads (hitches), memory after swaps |
| 5 | any | Full match to 30 kills or 10 minutes | memory over time, fragmentation, stability |

Also say per round: did it crash (the log then has no summary line), did sound play, did any walls vanish.

## What the log records (`game_core::telemetry`)
- Header: build, map, bots, class, free MEM1 / MEM2 at start; then the load time and memory after loading.
- Every second: fps, average / worst frame, CPU time and GPU wait (frames are pipelined: the wait for the *previous* frame's GPU work at the end of this one; near 0 means the CPU is the slower side, large means the GPU is), and the CPU split: input, player movement, gun,
  bots (all the arena's work; `bai` / `bmv` / `bgun` are the bots' thinking, moving and shooting within it), world draw submit, other players' bodies (characters: pose, CPU skinning and draw submit; or the placeholder boxes), viewmodel, HUD; triangles and draws; free memory (every 5 s).
- Every frame over 33 ms (up to 20 a minute): its full breakdown.
- Events: hits and kills involving you, map load, load failures (with the file error), pause, class change, weapon
  switch, log writes (with their own time), panics (with the message), match end.
- At the end: frame-time percentiles (p50 / p90 / p95 / p99), share of frames over 16.7 ms and 33 ms, the average
  CPU split over the match, and event counts.

`mwf-convert report <log>...` prints the same summary for any log files (`report.bat X:\path\0007.log`).

## Debug menu
DEBUG on the main menu (and in the pause menu) sets up test conditions: bot count and skill, passive or frozen bots, god mode, infinite ammo, every killstreak, any primary weapon, overlays (hit boxes, bot goals, objectives, bullet paths), and switches to isolate costs in the log (character models, world models, sound). The settings are saved to `<app folder>/debug.dat`; delete it to go back to the defaults. Say which debug settings were on when you send a log.

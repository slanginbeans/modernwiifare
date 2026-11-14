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
- Every second: fps, average / worst frame, CPU and GPU-wait time, and the CPU split: input, player movement, gun,
  bots, world draw submit, bot bodies, viewmodel, HUD; triangles and draws; free memory (every 5 s).
- Every frame over 33 ms (up to 20 a minute): its full breakdown.
- Events: hits and kills involving you, map load, load failures (with the file error), pause, class change, weapon
  switch, log writes (with their own time), panics (with the message), match end.
- At the end: frame-time percentiles (p50 / p90 / p95 / p99), share of frames over 16.7 ms and 33 ms, the average
  CPU split over the match, and event counts.

`mwf-convert report <log>...` prints the same summary for any log files (`report.bat X:\path\0007.log`).

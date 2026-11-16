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

## Scripted runs (menus and HUD)
`<app folder>/autoplay.txt`, when it is on the card, plays the GameCube pad from a script and saves screenshots
(`game_core::autoplay`; the Wii side is `wii/game/src/autoplay.rs`): one command a line, `wait N`,
`press A+UP [N]`, `hold` / `release`, `stick X Y`, `cstick X Y`, `triggers L R`, `times N <command>`,
`shot NAME` (the next frame as `<app folder>/shots/NAME.bmp`), `log TEXT` (a line in the match log), `warp X Y Z YAW`
(in a match: the player to that map spot, dropped onto the floor, facing YAW degrees), `quit`
(switches the console off). Frames are pad reads: one a frame in the menus and the match, none while a map loads,
so a script runs the same on every machine. Delete `autoplay.txt` afterwards, or the game plays it on every start.

`tools/autoplay/ks_tour.txt` calls killstreaks from the debug menu's GIVE lines (care package, emergency airdrop with
its crates' capture, Predator, sentry, precision airstrike); `tools/autoplay/ks_air.txt` the stealth bomber, attack
helicopter, Chopper Gunner and AC-130 (split so the streaks' own kills don't end the match first);
`tools/autoplay/ks_bots.txt` leaves a match to the bots for four minutes (their killstreaks show in the log as
`killstreak bot=N used=...`). Both toggle debug settings, so delete `debug.dat` from the card before each run.

`tools/autoplay/menu_tour.txt` walks every menu and the HUD: the lobby, Game Setup, the map, mode and rules pages,
Create-a-Class (the class page, the weapon type, weapon, attachment and perk popups, Rename, Reset Class), the
Playercard popup and the Killstreak Rewards page, a Team Deathmatch (the class menu as it starts), the START menu
over the game, Choose Class, Options (the safe area at 80% and back), End Game and
its question, back to the lobby and the main menu.
`tools/autoplay/av_fx.txt` checks audio and effects in a Team Deathmatch (fire at the floor and ahead, knife, the
class's tactical, then the bots' fight); `av_fx_wall.txt` fires into the nearest wall up close (bullet marks) and knifes
it. Dolphin runs without audio, so the match log's `avfx_counts` lines say what played.

`tools/autoplay/feel.txt` (mp_rust) checks the movement and camera feel: a mantle up a crate (the spot from
`player::tests::mantles_on_the_real_map`), a jump's landing dip, the view bob while running, and holding breath on
the Scout Sniper class's scoped rifle. The match log gets `warp`, `mantle_start` / `mantle_end`, `land_dip` and
`breath_hold` / `breath_release` lines.

`tools/autoplay/weapons_rpg.txt`, `weapons_javelin.txt`, `weapons_alt.txt` and `weapons_shield.txt` test the
launchers (a rocket into a bot; the Javelin's lock and top attack), the underbarrel launcher (Grenadier class, Z +
D-pad left), and the riot shield and akimbo (Riot Control class); their match logs record `launch`, `javelin`,
`alt_request`, `shield_block` and `akimbo_fire` events. `weapons_akimbo_alt.txt` and `weapons_gl_reload.txt` check the
first-person animations of the underbarrel shotgun (switch, fire, reload), akimbo (each gun firing, the pair's reload,
both magazines on the HUD) and the underbarrel launcher (raise, fire, reload, drop); they need custom classes the
defaults don't have: a `classes.dat` beside `boot.dol` whose first custom class is an M4 with the underbarrel shotgun
and akimbo USPs, the second an M4 with the grenade launcher (make them in Create-a-Class and copy the card's
`classes.dat` to `out\dolphin\sd\apps\mwf_game\`). Their logs show `view_companion <file> ok=1` for each companion
file loaded.

`tools/autoplay/hits_world.txt` plays a free-for-all on the first map (`-Maps mp_terminal` has glass): turns, walks,
fires in bursts and holds Y now and then. Check the match log for `hit`, `hitvol` (hit volume cost every 5 s),
`glass` (pieces at load), `glass_shattered`, `item_drop`, `ammo_pickup`, `scavenged` and `weapon_swap`.
`tools/autoplay/hits_swap.txt` uses the in-game debug page (passive and frozen bots, god mode, the hit box overlay,
BRING A BOT IN FRONT) to shoot a bot with the drawn bones and take its weapon with a Y hold, three times. Its toggles
assume the debug defaults: delete `<app folder>/debug.dat` (`out\dolphin\sd\apps\mwf_game\debug.dat`) first.

The match HUD over the play view (`hud_fx`): `tools/autoplay/hud_match.txt` (Team Deathmatch with the Barrett from the
debug menu: the scope overlay, teammates' names, three kills with the kill feed's icons and the killstreak splash, the
EMP's flash), `hud_match_rifle.txt` (the default M4: MW2's crosshair at rest and firing, a bot's hits with the hit
arrows and blood, a kill, the EMP's flash) and `hud_hits.txt` (free-for-all, standing still until the bots hit you: the
hit arrows and blood). The EMP static shows in `mwf-convert menu-preview` (`menu_hud_scope.png`). Rumble can't be seen
in Dolphin.

Bots take a random default class each life, so scripts that need a steady opponent start with the in-game debug
page's last line, BOTS: RIFLES ONLY (`last_stand.txt`, `hud_match.txt`, `hud_match_rifle.txt`): a launcher's blast
never puts you in last stand, and a riot shield stops the shots meant to kill. For kills on a bot brought in front
they also turn on BOTS DON'T ATTACK and BOTS FROZEN, so it stays where it was put, and off again afterwards.

On the PC, `tools/dolphin-test.ps1 -Dolphin <Dolphin.exe>` (or `MWF_DOLPHIN`) runs a script in Dolphin on the
built game (`dist\apps\mwf_game`, `out\packs`): it sets up its own Dolphin profile and an SD card folder under
`out\dolphin` (your Dolphin settings are untouched), boots the game, waits for `quit` and converts the screenshots
to `out\dolphin\shots\*.png`. Options: `-Script <file>`, `-Maps mp_rust,mp_terminal`. Dolphin shows the whole
frame; a TV crops its edges (overscan), which is what the safe area is for.

## Debug menu
DEBUG on the main menu (and in the in-game menu) sets up test conditions: bot count and skill, passive or frozen bots, god mode, infinite ammo, every killstreak, any primary weapon, overlays (hit boxes, bot goals, objectives, bullet paths), and switches to isolate costs in the log (character models, world models, sound). The settings are saved to `<app folder>/debug.dat`; delete it to go back to the defaults. Say which debug settings were on when you send a log.

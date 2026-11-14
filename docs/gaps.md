# Gaps against IW4L / MW2, and the plan (audit of 2026-11-14)

Three read-only audits compared the Wii build with IW4L (gameplay, menus, per-frame cost). Ranked findings, what is
done, and what is next. Priority: gameplay feel and frame rate first, looks last.

## Done in this round
- **Test telemetry** (`docs/testing.md`): per-match logs on the SD card, `report.bat` summaries.
- **Fixed-rate simulation.** The player's movement and gun step at 60 Hz (16/17 ms commands, as IW4 at 60 fps) from
  an accumulator, so fire rate no longer drops with the frame rate. Bots run as IW4's server runs them: a step every
  50 ms, a decision every 100 ms, staggered across frames, drawn between steps (also several times cheaper).
- **The weapon reaches pmove** (IW4L `sim::step` / `advance_weapon_command`): `ps.weapon`, weapon state and the
  weapon step's `pm_flags` are copied into the player state; the pmove context comes from the weapon's facts (move
  and ADS move speed, sprint time from `sprintDurationScale` — new `WMOV` section — ADS rates and whether ADS is
  allowed, melee delays, scope, hold breath); fire and reload reach pmove (they end a sprint). IW4L's sprint delay 0
  and last-stand crawl 0.15.
- **Shotguns fire every pellet** (IW4L `pellet_count`, times the barrels for double-barrel fire).
- **Hits slow the victim** (IW4L `update_damage_timer`).
- **Bot sight as IW4L's sensor**: 90° field of view, 5906-unit range.
- **Bot route cost**: a grid index for the nearest walk-graph point, one A* scratch reused by every search (no
  per-route allocation), at most one new route per frame.
- **Allocator**: `malloc` / `realloc` instead of `memalign` for every allocation.
- **Static models**: world bounds computed once at load, not every frame.

- **Controller look and aim** (2026-11-14): IW4L's `pad_look_frame` (radial dead zone, look curve, ADS look speed,
  turn acceleration) and aim assist (slowdown, lock-on, ADS snap) with each weapon's ranges (`WAIM`); stick range
  learned from use (full forward reaches the sprint threshold); ADS zoom with `hud_iw4::calc_fov_from_ads`.

## Next, gameplay (ranked)
1. ~~Viewmodel ADS pose~~ done with the full viewmodel anim tree (every slot, timer-fitted rates, ADS overlay,
   per-bone blend) and MW2's weapon placement (view sway, bob, idle sway, movement offsets). Still to confirm: the
   stick range from the logs; the first-person lighting from the light grid.
2. ~~Weapon switching through IW4L~~ done: the gun asks for the other weapon (`usercmd.weapon`), IW4L drops it,
   the other takes the hand and raises (first raise once a life); refused mid-melee, cancelled by asking again.
3. **Melee**: done without the lunge. The melee button goes through IW4L's hand driver (`weapon_hands`:
   `weapon_try_melee`, swing and hit times); the hit is IW4L `fire_weapon_melee`'s nine traces (64 units) against
   the players' boxes, melee damage plus 0..4. Bots swing when you're within reach. Not yet: the lunge
   (`melee_charge` target and `movement_iw4::melee_charge_move`).
4. ~~Players block each other~~ done: pmove's traces sweep against every other living player's hull with
   `clipmap_iw4::transformed_temp_capsule_trace` (bullets and sight still see the world only).
5. ~~Health regeneration~~ done: MW2's `_healthoverlay.gsc` loop step by step (`life::Life::regen`), the numbers
   read from the user's scripts (`RULE`).
6. **Bullet penetration** (`weapon_iw4::penetration`; needs surface types and the depth table from the converter).
7. **Hit volumes**: turn the boxes with the body's yaw now; the character models' hit boxes later.
8. **FFA rules**: points (a kill, the headshot bonus), score and time limits from the user's `dm.gsc`, ties to fewer
   deaths: done. Spawns follow MW2's free-for-all weighting (`_spawnlogic.gsc`: distance to everyone, enemies in
   sight, the nearest enemy, the last spawn used): done. Still to do: the playlist's overrides (the stock script
   limit is 1000 points).
9. Crouch as MW2's console toggle; prone.
12. **Grenades**: frag-type lethals done (`game_core::grenade`, IW4L's throw / bounce / fuse / blast; bots throw
    them). Not yet: throwing knife, claymore, C4, tactical insertion, blast shield; flash / stun (IW4L
    `apply_flashbang_blast` and the shock) and smoke; the grenade and explosion models, effects and sounds; the
    grenade danger indicator; Danger Close.
11. **Minimap and killstreaks**: the HUD minimap (map pack `MINI`: MW2's `setupMiniMap` frame, the script's
    `compassmaxrange`; turned with the view; firing pings), kill streaks with your class's streak costs, the UAV
    (radar sweep and ping fade from IW4L's compass, duration from `_uav.gsc`), bots' UAVs. Not yet: the other
    streaks (care package, predator, sentry, harrier, airstrike, helicopters, AC-130, EMP, nuke), Counter-UAV,
    Cold-Blooded / Scrambler on the radar, the minimap's fading edge and scan-line pass order.
10. **Perks** (`game_core::perks`): a class's three perks and their Pro perks (perkTable's upgrade column;
    everything unlocked) become IW4's `ps.perks` bits that IW4L's movement and weapon code read (Marathon,
    Sleight of Hand, Steady Aim and their Pros, Commando's flag, ...); move speed by the primary's class and
    Lightweight's scaler come from the user's `_weapons.gsc` / `_perkfunctions.gsc` (`RULE`); Stopping Power adds the
    engine's 40% to bullet damage. Not yet: Scavenger packs, Hardline and the killstreak perks, Cold-Blooded /
    Scrambler / Ninja against radar and sound (no UAV or footstep audio yet), Danger Close and Last Stand (no
    explosives or last stand yet), One Man Army, deathstreaks.

## Next, performance (ranked by expected milliseconds; measure with the logs first)
1. **World vertex load** (~12-25 ms on big maps): `LOD_SCALE`, fog / far plane per map, vertex welding, more compact
   batches.
2. **Run the CPU while the GPU draws**: today `GX_DrawDone` then vsync serialise them, so frame = CPU + GPU and any
   frame over 16.7 ms becomes 33.3 ms. Pipeline with draw-sync tokens and three frame buffers (needs double buffers
   for the viewmodel's skinned positions, and the weapon-file swap after the GPU is done).
3. Draw submit: a tint-only state change for static models (today every instance reprograms all of TEV), shared
   quantisation origins, opaque batches presorted at load.
4. Viewmodel: skin straight into MEM2 with pre-scaled weights, GX matrices for rigid parts, strip its triangles.
5. Weapon-file loads only at map start (today a class change loads at the next spawn: a hitch).
6. Finer telemetry: per-bot, trace counts, A* expansions, world draw stages, the end-of-frame split, GPU metrics.

## Menus like MW2's
Done (2026-11-14): fonts and the main menu screen baked (`wpk::ui`, `UIPK`), drawn on the Wii as textured quads.
Then each screen's own layout: class select (`changeclass`), pause (`class`), Create-a-Class (`menu_cac_assault`) and
the scoreboard (`page_scoreboard`), with MW2's localized title / header text, filled bars, the back button's place
for the footer hint, and empty text slots filled live (the class panel: weapons, perks, equipment; the pause
screen's map and mode). The layout code is shared by the Wii and the PC preview (`game_core::ui_page`).
Not yet: weapon / perk icons in the class panel, the `popup_cac_*` choosers' own look (choices use the
Create-a-Class screen), MW2's Create-a-Class grouping (attachments inside the weapon choice, killstreaks on their
own page; ours lists every field), the scrolling cloud layers (left out: they are moved by scripts and are the
costliest fills), animations, 16:9 layouts.

### Plan
IW4L reads the real menu assets (`fastfile_iw4` hooks: `capture_menu_def`, `capture_menu_item_layout`, `capture_font`
…; zones `code_post_gfx_mp`, `localized_code_post_gfx_mp`, `localized_ui_mp`, `common_mp`, `patch_mp`) and runs them
with a script/expression engine on Bevy. For the Wii: **bake the screens at convert time**, keep a small runtime.
1. Fonts: capture MW2's fonts (glyph tables + atlas), convert the atlases to GX I4/IA4, draw text as textured quads
   (a new textured 2D path in the shim; `hud_iw4::font` helpers for `^N` colours).
2. UI images: the menu and catalog textures (weapon and perk icons) the chosen screens use.
3. Layouts: capture menuDefs/items, resolve rects to 640×480 (`hud_iw4::ScreenPlacement`, 4:3 and 16:9), resolve
   text to localized strings, fold expressions at convert time; only state-dependent items stay dynamic.
4. Screens in order: main menu, class select, pause, scoreboard, Create-a-Class (the most scripted), HUD.
Baked layouts, fonts and atlases are game data: they live in `out/` and on the SD card, never in git.

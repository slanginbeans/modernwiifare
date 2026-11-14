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
1. **Viewmodel ADS pose** (the gun moving to its sights) and confirm the stick range from the logs.
2. **Weapon switching through IW4L** (`weapon_check_for_change`: drop, then raise; refused in blocking states).
3. **Melee** (`weapon_iw4::melee`, the lunge in `movement_iw4::melee_charge_move`).
4. **Players block each other** (`clipmap_iw4::transformed_temp_capsule_trace` per fighter in pmove traces).
5. **Health regeneration** with the delay and rate read from the user's game scripts (not guessed).
6. **Bullet penetration** (`weapon_iw4::penetration`; needs surface types and the depth table from the converter).
7. **Hit volumes**: turn the boxes with the body's yaw now; the character models' hit boxes later.
8. **FFA rules**: IW4L `gamemode_iw4::ffa` scores (50 a kill, limit from the user's playlist / scripts), ties to
   fewer deaths; spawns scored by distance (`SPAWN_IDEAL_DIST` 1600, `SPAWN_BAD_DIST` 1200) and line of sight.
9. Crouch as MW2's console toggle; prone.

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

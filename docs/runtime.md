# The Wii game runtime (`mwf_game.dol`)

## Try it
**Easiest:** keep this repo folder next to your `Call of Duty Modern Warfare 2` folder (or set `MWF_GAME` to it) and
double-click `build.bat`. It builds the game, converts every map and weapon that is missing or out of date (a pack
made by the current converter from unchanged game files is skipped, so later runs take seconds), and puts it all in
`dist\apps\mwf_game`. Copy that one folder into the SD card's `apps` folder (so the card has
`apps\mwf_game\boot.dol` with its `maps\`, `views\` and `common.wpk`). `build.bat nodata` skips the conversion.
The converter is built for `x86_64-pc-windows-gnullvm` with llvm-mingw (`tools\mwf-convert.ps1`; the plain Windows
GNU toolchain can't build IW4L's zone reader): it uses `rust\env.ps1` at the root of the repo's drive when there is
one (or `MWF_RUST_ENV`), and otherwise offers to download a pinned llvm-mingw into `.toolchain\`.

By hand:
1. Convert the maps on your PC: `mwf-convert pack "<your MW2 / IW4x folder>" --map all` → `out/packs/<map>.wpk` for
   all 16 multiplayer maps (or `--map mp_rust` for one).
   Packs that are already up to date are skipped (`--force` rebuilds them).
   `--map all` also writes `out/packs/common.wpk` (weapons and classes) and `out/packs/views/*.wpk` (each weapon's
   first-person model, textures and sounds); or run `mwf-convert common "<folder>"` for those alone.
2. The packs live in the game's own app folder: map `.wpk` files in **`sd:/apps/mwf_game/maps/`**, `common.wpk` in
   **`sd:/apps/mwf_game/`**, the weapon files in **`sd:/apps/mwf_game/views/`** (~80 MiB for all 45; the `.json`
   reports are not needed). Without `common.wpk` you walk unarmed and Create-a-Class is off. The game finds its folder
   from the path the Homebrew Channel starts it with, so a USB copy (`usb:/apps/mwf_game/`) works too; started
   without one (wiiload, emulators) it uses `sd:/apps/mwf_game`.
3. Copy `mwf_game` from the build (`make -C wii package` → `dist/apps/mwf_game`, or CI's `modernwiifare-sd-card`) to
   `sd:/apps/`, and start **Modern Wiifare** from the Homebrew Channel. GameCube controller in port 1.

## Menus
The menus use MW2's own look, converted from your copy into `common.wpk` (`converter/src/ui_bake.rs`): its fonts
(glyph tables over their atlases) and five of its screens, each with its images, gradients, title and header text,
button column (position, font, text placement, selection bar) and the back button's place:
- main menu (`main_text`): also the map choice;
- class select (`changeclass`): the class panel on the right shows the focused class's weapons, perks and equipment;
- Create-a-Class (`menu_cac_assault`): the panel shows the class being edited, or previews the focused choice;
- pause (`class`): with the map and the mode on the right;
- scoreboard (`page_scoreboard`): MW2's columns (score, kills, assists, deaths); free-for-all scores 50 a kill.

The layout is `game_core::ui_page`, shared with `mwf-convert menu-preview <common.wpk>`, which renders every screen
with sample content to `out/preview/menu_<screen>.png` to check them on the PC. Without menu data in `common.wpk`
the menus fall back to the block font.
Every menu is a list: D-pad or main stick up / down, **A** chooses, **B** goes back.
- **Main menu**: Play, Create a Class, Quit.
- **Play** → choose a map → **Game Setup** (as MW2's private match: game type free-for-all or team deathmatch, score
  limit, time limit; each starts at your scripts' value for the mode, A cycles the choices) → choose a class (your
  five custom classes, then MW2's five default classes) → the match.
- **Team deathmatch** (`war.gsc`): you and two bots against three (allies tinted blue); team-mode points (100 a kill
  from `_rank.gsc`) go to the team's score; no friendly fire (MW2's default); bots and aim assist ignore teammates;
  spawns follow MW2's `getSpawnpoint_NearTeam` (far from enemies, near allies, out of enemy sight); teammates always
  show on the minimap; the scoreboard ends in victory, defeat or a draw. The minimap also shows grenades and
  explosions.
- **Create a Class**: pick one of the five custom classes, then change its primary and secondary weapons, their
  attachments, equipment, special grenade, perks 1-3, deathstreak and three killstreaks. Everything is unlocked.
  The rules are MW2's: primaries are assault rifles, SMGs, LMGs, sniper rifles and the riot shield; secondaries are
  machine pistols, shotguns, handguns and launchers; each weapon takes only its own attachments, never two that
  clash; a second attachment needs Bling in perk 1 (taken as Bling Pro, so the secondary gets one too). Leaving a
  class saves all five to `classes.dat` in the app folder, by item name (a re-converted `common.wpk` still reads it).
  The classes start as MW2's own starting custom classes. Names, weapons and perks come from your copy's tables
  (statsTable, attachmentTable, perkTable, killstreakTable, classTable) and their localized names.
- In a match, **START** pauses: Resume, Change Class (applies at your next spawn, as in MW2) or End Game.

Look and aim are IW4L's gamepad model (`game_core::look`): a 12% radial dead zone, its look curve, turn rates of
260 / 90 °/s at the hip and 90 / 55 °/s in ADS (slower when zoomed), turn acceleration, and aim assist with each
weapon's ranges (slowdown over a target, lock-on that follows it, a short snap when you start aiming down sights).
The GameCube sticks stop short of their full range, so each stick's full deflection is learned from how far you push
it (once fully pushed, full forward reaches IW4's sprint threshold). ADS zooms to each weapon's own field of view
(IW4L `calc_fov`), and the first-person gun comes up to its sights through the weapon's ADS clips (see below).

Controls in a match (IW4L movement and the map's collision): main stick move, C-stick look, **A** jump, **B** crouch
(held), **Z** sprint, **L** ADS, **R** fire, **X** reload, **Y** switch weapon (the gun drops, then the other raises,
as in MW2), **D-pad right** melee, **D-pad up** lethal grenade (hold to cook), **D-pad down** tactical grenade,
**D-pad left** calls in your newest killstreak reward. **Z + D-pad up** toggles free fly (main
stick, C-stick, L/R triggers down/up, B fast); leaving free fly drops the player where the camera is.
**Z + D-pad down** toggles the performance overlay (fps, frame / CPU /
GPU-wait ms, triangles (of which static models), draws, state changes, visible cells, models, position; see
`docs/perf.md`). A pack without a `CLIP` section starts in free fly.

Grenades: your class's frag-type equipment (frag, semtex) is thrown through IW4L's offhand states (pull back, cook
while held, throw) and flies as IW4L's sim flies it (throw speed, gravity, the weapon's per-surface bounce, the fuse;
`WEQP` in `common.wpk`); the blast hurts everyone in reach, the thrower too, by IW4L's falloff and line-of-sight
scaling. Bots throw frags at enemies they see at grenade range. Grenades and blasts show as placeholder boxes until
the projectile models and explosion effects are converted, and the explosion has no sound yet. Semtex sticks where
it lands. Special grenades: the flashbang blinds by distance and where you look (IW4L's flashbang amounts, then
`_flashgrenades.gsc`'s duration; a white-out by the shock file's fade times; flashed bots see nothing); the stun
grenade's shock slows movement for `2 + 4 × closeness` seconds (`_weapons.gsc`). Teammates' flashes and stuns
don't affect you. Throwing knives, claymores, C4, tactical insertion, blast shield, smoke and deathstreaks are
chosen and saved but do nothing yet.
Weapons with attachments use the attachment variant's combat numbers (`m4_acog_mp` ...) but the base weapon's
first-person model.

Solid colour screens (START goes back to the menu): red = no SD card or no such pack, magenta = the pack failed its
checks (or is from an older converter: convert again), orange = out of memory or texture slots.

## Memory
The two weapon files of the spawned class load into MEM2 above the map (about 2 × 1.8 MiB) and are swapped when a
spawn brings other weapons (MEM2 and texture handles released back to a mark taken after the map).
The pack's GPU data (textures, vertices, display lists) is read into MEM2 and used in place; the sections decoded at
load (visibility, cell tables, collision) are written last by the converter, read into a temporary MEM1 buffer,
decoded into MEM1 and the buffer freed. `mwf-convert pack` prints the MEM2 part and warns above 44 MiB. Leaving a map
releases its MEM2 (bump allocator mark/release) and its textures.

## Structure
| Part | What |
|---|---|
| `wii/game_core` | `no_std`, host-tested: camera (IW4 Z-up → GX eye space), frustum test, skybox quads, free-fly controller, pack world loading and per-frame draw order through the `Gfx` trait; collision backend (`collision`) and the player (`player`: pad → `UserCmd` → IW4L `pmove`) |
| `wii/game` | Wii staticlib: `Gfx` over the C shim, allocator, panic screen, main loop |
| `wii/platform/shim_world.c` | MEM2 bump allocator, SD loading (libfat) with `DCFlushRange`, GX texture objects (+ I4 alpha planes), world vertex format (index16, lightmap UV u16 frac 15), TEV recipes, blend / alpha test / Z, display-list calls, skybox quads |

Per frame: one `pmove` with the frame's milliseconds, then the skybox (Z off), visible opaque and alpha-tested
world batches grouped by material and lightmap, then static model instances (each within its draw distance and in
view: view × placement loaded into the position matrix, its surfaces' display lists, its light-grid colour as a TEV
konst ×2), then blended world and model surfaces back to front. The pack is loaded once into MEM2 and
used in place; collision is decoded once into the MEM1 heap (about 2.6 MiB for `mp_rust`), where traces are fastest.

## Gun
The class's weapons from `common.wpk` (the M4A1 `m4_mp` in the figures below), run by IW4L's weapon state machine (`weapon_iw4::weapon_ordinary`: raise,
fire rate, bursts, reload, rechamber), so its timings are the PC game's: 70 ms per round, quantized to whole frames
like IW4 (every 5th frame at 60 fps), 30-round magazine, 2.03 s / 2.36 s (empty) reload. Bullets are traced against
the map (IW4 `MASK_SHOT`); impacts show as yellow markers for 400 ms. HUD: crosshair, magazine / reserve, weapon.
Aim spread is IW4L's: the cone grows while firing, moving or turning and decays at rest, by stance and ADS; each
bullet leaves in a random direction inside it, and the crosshair ticks sit on the cone's edge.
Recoil is IW4L's model with each weapon's own kick numbers (`KICK` in `common.wpk`): every shot kicks the view
(capped at 8°, settling over ~80 ms springs; later shots follow the kicked view, so the gun climbs as in MW2) and
the viewmodel (gun kick within the weapon's maximum pitch / yaw).
A shot shows a muzzle flash for 50 ms at the gun's `tag_flash`: a placeholder glow in two HUD rectangles, until
MW2's view flash effect (FxEffectDef) is converted.
Sounds: the gun plays MW2's player-view sounds through libogc's ASND mixer (16 voices): fire, empty click, reload
and raise from the weapon's events, and the reload's clip-out / clip-in / chamber sounds from the first-person
animation's notetracks (mapped through the weapon's `notetrackSoundMap`). Each play picks a variant, volume and
pitch from the alias's ranges, as IW4 does. They come from `localized_common_mp` (where MW2 MP keeps its sound
aliases and clips), stored in `common.wpk` as mono 16-bit PCM at the original 44.1 kHz (~0.5 MiB for the M4).
No distance falloff or panning yet: there are no other players to hear.
Not yet: damage to players, other weapons and attachments.

## First-person model
Each weapon's view hands and gun (`views/<weapon>.wpk`: every animation slot the weapon names, e.g. 22 for the
M4A1), posed and skinned on the CPU each frame and drawn after the world in the front slice of the depth range, so
it never clips into walls.
- Animation (`game_core::weapon_anim`, IW4L's viewmodel anim tree): IW4L's weapon state machine says which clip
  starts (raise, first raise, quick / empty raise and drop, fire, ADS fire, last shot, rechamber, reload, empty
  reload, shotgun reload start / loop / end, melee, sprint in / loop / out ...); each action clip plays at the rate
  that fits its weapon timer (a reload lasts the weapon's reload time; Sleight of Hand halves it), idle waits for an
  action clip to finish; the ADS up / down clips are scrubbed by how far the gun is up as overlays, so the gun comes
  to its sights; every weighted clip is blended per bone.
- Placement (`game_core::weapon_view`, IW4L's `weapon_placement_assemble`): the gun lags behind turning (the
  weapon's sway numbers, hip and ADS), bobs with your steps, sways at idle, shifts with stance and movement
  (`standMove` .. `proneRot`), and kicks with recoil. The numbers come from `WFPV` in `common.wpk`.
Parts the base weapon hides (sights, heartbeat sensor) are left out at conversion. Lighting is flat for now (MW2
lights it from the light grid). Check it on the PC:
`mwf-convert preview <map.wpk> --viewmodel out/packs/common.wpk [--anim <slot> --time <s>]` (slot 9 = reload).
Not measured yet: the CPU cost of skinning 7.6k vertices per frame on Broadway.

## Bots and damage (free-for-all)
A match is a free-for-all against 5 bots (`BOTS` in `wii/game/src/lib.rs`), first to 30 kills or the 10-minute
clock (MW2 FFA's defaults), then a scoreboard. The top right shows your kills and the leader's, the left side a kill
feed, and hits show MW2's hitmarker.
- **Damage** is each weapon's own: IW4L's range falloff (full damage to its max-damage range, down to its minimum at
  its min-damage range) times the hit location's multiplier from MW2's `mp_lochit_dmgtable` (read from your copy and
  baked into each weapon by the converter, as IW4L does). Bullets hit the first player in their path before the wall
  behind. Players' hit volumes are a stack of boxes (head, neck, upper and lower torso, arms, upper and lower legs)
  sized from IW4's hull and view heights, standing or crouched, until the character models are converted: where a
  shot lands on the body is approximated, the damage numbers are not.
- **Bots** play with the same IW4L movement and weapon code as you, their guns from MW2's five default classes. They
  see 90° in front of them up to 5906 units with a clear line (IW4L's bot sensor), hear gunfire within 2000 units,
  step every 50 ms and decide every 100 ms like IW4's server bots, react after about a third of a second, turn at limited speed (IW4L's bot motor), fire inside an 8° cone with a small aim error that settles,
  strafe while fighting, chase what they saw or heard, and otherwise roam the map. They walk a graph the converter
  bakes from the map's collision (`NAVG` in the pack, flooded out from the spawn points on a 48-unit grid) with A*
  routes, and jump when stuck. Not yet: mantling, ladders, doors, grenades, knifing, their weapon sounds, and other
  players blocking movement (players pass through each other).
- **Bodies**: bots are drawn as shaded boxes, one per hit volume (skin-coloured head and neck), until the character
  models are converted.
- Spawns: the free-for-all spawn point furthest from everyone alive.
- Maps converted before this need converting again for the walk graph; without one, bots stand and fight.

## Life and match
Health 100; fall damage from IW4L's crash land (falls under 128 units are free, 300 and over kill); dropping 2048
units below the lowest spawn point kills (MW2 uses trigger volumes). A death shows "YOU DIED", then you respawn at
the spawn MW2's free-for-all weighting picks after 3 s, with a fresh gun. Health regenerates as MW2's
`_healthoverlay.gsc` does (numbers read from your scripts). A red vignette shows missing health and flashes on
damage; the bottom left shows the match clock (the time limit from your `dm.gsc`) and deaths. A free-flying player
(D-pad up) is out of the match: the bots ignore it.
The minimap (top left, MW2's `minimap_fullscreen` layout) turns with your view; enemies who fire show for 2 s.
Killstreaks: kills in a row since your last death earn your class's streaks at their killstreakTable cost; the UAV
(D-pad left) runs MW2's radar sweep for its script time (every enemy marked as the sweep passes). Bots call in a UAV
at its cost too. The other streaks are earned but not in this build yet.
Placeholder until MW2's game-type scripts run (GSC, ADR 0002): the 3 s respawn (MW2: death cam / killcam flow).

## Movement status
- `pmove` is IW4L's, unchanged; the context uses MW2's MP dvar defaults (190 speed, 800 gravity, jump 39, sprint ×1.5).
- Traces: world brushes through the BSP plus the triangle mesh (`clipmap_iw4::trace_brush_and_mesh`). Glass is
  always solid. Not yet: brush models (doors, movers), other players, penetration recovery.
- C-stick look speed (200°/s yaw, 140°/s pitch at full tilt, linear past a 12-count dead zone) is a placeholder. It
  must be tuned against the 360 version (`feel-parity` skill) before it is called right.

## Not verified on hardware yet
- GX front-face winding: `MWF_GX_CULL_BACK` in `shim_world.c` assumes GX's back faces match IW4's (D3D, clockwise
  front). If walls vanish and their insides show, swap the two defines.
- Draw cost of 180-375k triangles per view (world + static models) (`mwf-convert preview` reports the count); visibility cells and fixed-point
  vertices are the next performance steps.
- Trace cost on Broadway: on the host one `pmove` on `mp_rust` takes about 20 µs, so a 750CL should stay well under
  a millisecond, but this is unmeasured.
- Sound from MEM2: ASND plays the clips in place from `common.wpk` in MEM2. If the gun is silent, the DSP may not
  reach MEM2 there, and the clips need copying to MEM1.

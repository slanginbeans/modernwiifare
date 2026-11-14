# The Wii game runtime (`mwf_game.dol`)

## Try it
1. Convert the maps on your PC: `mwf-convert pack "<your MW2 / IW4x folder>" --map all` → `out/packs/<map>.wpk` for
   all 16 multiplayer maps (or `--map mp_rust` for one).
   `--map all` also writes `out/packs/common.wpk` (weapons and classes) and `out/packs/views/*.wpk` (each weapon's
   first-person model, textures and sounds); or run `mwf-convert common "<folder>"` for those alone.
2. The packs live in the game's own app folder: map `.wpk` files in **`sd:/apps/mwf_game/maps/`**, `common.wpk` in
   **`sd:/apps/mwf_game/`**, the weapon files in **`sd:/apps/mwf_game/views/`** (~80 MiB for all 45; the `.json`
   reports are not needed). Without `common.wpk` you walk unarmed and Create-a-Class is off. The game finds its folder
   from the path the Homebrew Channel starts it with, so a USB copy (`usb:/apps/mwf_game/`) works too; started
   without one (wiiload, emulators) it uses `sd:/apps/mwf_game`.
3. Copy `apps/mwf_game` from the build (`make -C wii package` → `dist/apps`, or CI's `modernwiifare-sd-card`) to
   `sd:/apps/`, and start **Modern Wiifare** from the Homebrew Channel. GameCube controller in port 1.

## Menus
Every menu is a list: D-pad or main stick up / down, **A** chooses, **B** goes back.
- **Main menu**: Play, Create a Class, Quit.
- **Play** → choose a map → choose a class (your five custom classes, then MW2's five default classes) → the match.
- **Create a Class**: pick one of the five custom classes, then change its primary and secondary weapons, their
  attachments, equipment, special grenade, perks 1-3, deathstreak and three killstreaks. Everything is unlocked.
  The rules are MW2's: primaries are assault rifles, SMGs, LMGs, sniper rifles and the riot shield; secondaries are
  machine pistols, shotguns, handguns and launchers; each weapon takes only its own attachments, never two that
  clash; a second attachment needs Bling in perk 1 (taken as Bling Pro, so the secondary gets one too). Leaving a
  class saves all five to `classes.dat` in the app folder, by item name (a re-converted `common.wpk` still reads it).
  The classes start as MW2's own starting custom classes. Names, weapons and perks come from your copy's tables
  (statsTable, attachmentTable, perkTable, killstreakTable, classTable) and their localized names.
- In a match, **START** pauses: Resume, Change Class (applies at your next spawn, as in MW2) or End Game.

Controls in a match (IW4L movement and the map's collision): main stick move, C-stick look, **A** jump, **B** crouch
(held), **Z** sprint, **L** ADS, **R** fire, **X** reload, **Y** switch weapon. **D-pad up** toggles free fly (main
stick, C-stick, L/R triggers down/up, B fast); leaving free fly drops the player where the camera is. **D-pad left**
next spawn point (with the class's weapons), **D-pad down** toggles the performance overlay (fps, frame / CPU /
GPU-wait ms, triangles (of which static models), draws, state changes, visible cells, models, position; see
`docs/perf.md`). A pack without a `CLIP` section starts in free fly.

Perks, equipment, special grenades, deathstreaks and killstreaks are chosen and saved but do nothing in a match yet.
Weapons with attachments use the attachment variant's combat numbers (`m4_acog_mp` ...) but the base weapon's
first-person model. Switching weapons raises the other one at once (no drop animation of the first yet).

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
The M4A1's view hands and gun (`common.wpk`, 8.4k triangles, 101 bones, 17 animations), posed and skinned on the CPU
each frame (`game_core::viewmodel`) and drawn after the world in the front slice of the depth range, so it never
clips into walls. Which animation plays comes from IW4L's weapon state machine (raise, fire, reload, empty reload,
sprint, drop...), so the timing is the game's. Parts the base weapon hides (sights, heartbeat sensor) are left out at
conversion. Lighting is flat for now (MW2 lights it from the light grid). Check it on the PC:
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
  see in front of them (150°) with a clear line, hear gunfire within 2000 units, react after about a third of a
  second, turn at limited speed (IW4L's bot motor), fire inside an 8° cone with a small aim error that settles,
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
the next spawn point after 3 s, with a fresh gun. A red vignette shows missing health and flashes on damage; the
bottom left shows the match clock (10 minutes, IW4L's FFA limit) and deaths. A free-flying player (D-pad up) is out
of the match: the bots ignore it.
Placeholders until MW2's game-type scripts run (GSC, ADR 0002): the 3 s respawn (MW2: death cam / killcam flow) and
no health regeneration.

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

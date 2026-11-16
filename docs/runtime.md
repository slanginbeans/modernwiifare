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
(glyph tables over their atlases) and its console screens, each with its images, gradients, title and header text,
button column (position, font, text placement, selection bar) and the back button's place:
- main menu (`main_text`);
- the private match lobby (`menu_xboxlive_privatelobby`): the players in MW2's rows on the right (team-coloured in team
  modes), the player count, the map and the game mode in its slots;
- the Game Setup popup (`popup_gamesetup`, its frame sized for its three buttons) over the dimmed lobby;
- the map page (`settings_map`) and the game mode page (`settings_gametype`): the focused map's picture (MW2's`n  `preview_<map>` from your `main/*.iwd`, also in the lobby under the map's name), the map's or mode's name and
  MW2's description (`MPUI_DESC_MAP_*`, the game type table's strings) in the right-hand panel;
- the rules rows (`settings_quick_dm`): Game Rules, Options and Debug, each value drawn in its row;
- class select (`changeclass`): the class panel on the right shows the focused class's weapons, perks and equipment;
- Create-a-Class (`menu_cac_assault`): MW2's class page, the class being edited in its panel; over it MW2's popups
  (`popup_cac_weapon_primary` for the weapon types, `popup_cac_assault_primary` for the weapons with the focused
  one's picture, `popup_primary_attachments`, `popup_cac_perk1` for every one-item choice with the focused item's
  icon, name and description and its Pro version's, `popup_cac_template` for Reset Class). A popup's frame is sized
  to its rows as MW2's scripts do (`od::POPUP_FRAME`), and choices are listed in the order MW2's popups list them
  (`order:cac`);
- the killstreak page (`menu_cas_popup`): MW2's rows with their check boxes and kills, the focused killstreak's name,
  icon and description, and the three chosen at the bottom; the playercard popup (`popup_callsign`) it opens from;
- the in-game START menu (`class`): with the map and the mode on the right;
- the yes / no popup (`popup_endgame`): MW2's End Game question;
- scoreboard (`page_scoreboard`): MW2's columns (score, kills, assists, deaths); free-for-all scores 50 a kill.
In a match the menus draw over the last frame of the game (`mwf_grab_screen` keeps it at half size), dimmed as
MW2 does, instead of a blank screen.

The layout is `game_core::ui_page`, shared with `mwf-convert menu-preview <common.wpk>`, which renders every screen
with sample content to `out/preview/menu_<screen>.png` to check them on the PC. Without menu data in `common.wpk`
the menus fall back to the block font.
Every menu is a list: D-pad or main stick up / down (held, it repeats after 0.4 s, then every 90 ms), **A** chooses,
**B** goes back (each with MW2's menu sounds: `mouse_over` on a move, `mouse_submenu_over` in popups, `mouse_click`
on a choice, from the UI zones); on a line with a value, left / right change it (A steps it forward). The menus ignore A, B and START
until they are let go when they open from a press (START opening the in-game menu doesn't also close it, on either
pad). The flow is the Xbox 360's private match:
- **Main menu**: Private Match, Create a Class, Options, Debug, Quit.
- **Private Match** → the lobby: **Start Game**, **Game Setup** (the popup: Change Map, Change Game Mode, Game Rules:
  time limit and score limit, each starting at your scripts' value for the mode, the bots (0-7) and their difficulty),
  **Create a Class**, **Playercard** (MW2's callsign popup: Title and Emblem are greyed out, there is no progression;
  **Killstreak Rewards** picks your three killstreaks, saved to `killstreaks.dat`, for every class as in MW2). Start
  Game loads the map and, as MW2 does when you join, asks for your class over the first frame (your five custom
  classes, then MW2's five default classes; B keeps the one last chosen); the match ends back in the lobby.
- **Team deathmatch** (`war.gsc`): you and two bots against three (allies tinted blue); team-mode points (100 a kill
  from `_rank.gsc`) go to the team's score; no friendly fire (MW2's default); bots and aim assist ignore teammates;
  spawns follow MW2's `getSpawnpoint_NearTeam` (far from enemies, near allies, out of enemy sight); teammates always
  show on the minimap; the scoreboard ends in victory, defeat or a draw. The minimap also shows grenades and
  explosions.
- **Create a Class**: pick one of the five custom classes, then MW2's class page: **Primary** / **Secondary** (the
  weapon type popup, the weapon popup, then the attachment popup, and with Bling a second one; B steps back a popup),
  **Equipment**, **Special Grenade**, **Perk 1-3**, **Deathstreak** (each its popup), **Rename** (letter by letter:
  up / down change the letter, left / right move, A keeps it, B leaves it), **Reset Class** (copies one of MW2's
  default classes, keeping the name). Everything is unlocked. Not here: camouflage (the guns would need their camo
  textures).
  The rules are MW2's: primaries are assault rifles, SMGs, LMGs, sniper rifles and the riot shield; secondaries are
  machine pistols, shotguns, handguns and launchers; each weapon takes only its own attachments, never two that
  clash; a second attachment needs Bling in perk 1 (taken as Bling Pro, so the secondary gets one too). Leaving a
  class saves all five to `classes.dat` in the app folder, by item name (a re-converted `common.wpk` still reads it).
  The classes start as MW2's own starting custom classes. Names, weapons and perks come from your copy's tables
  (statsTable, attachmentTable, perkTable, killstreakTable, classTable) and their localized names.
- In a match, **START** opens MW2's in-game menu over the frozen game: Choose Class (applies at your next spawn, as in
  MW2), Options (back to this menu after), End Game (MW2's yes / no question first, No focused), Debug; B or START
  resumes.

Look and aim are IW4L's gamepad model (`game_core::look`): a 12% radial dead zone, its look curve, turn rates of
260 / 90 °/s at the hip and 90 / 55 °/s in ADS (slower when zoomed), turn acceleration, and aim assist with each
weapon's ranges (slowdown over a target, lock-on that follows it, a short snap when you start aiming down sights).
The GameCube sticks stop short of their full range, so each stick's full deflection is learned from how far you push
it (once fully pushed, full forward reaches IW4's sprint threshold). ADS zooms to each weapon's own field of view
(IW4L `calc_fov`), and the first-person gun comes up to its sights through the weapon's ADS clips (see below).

Controls in a match (IW4L movement and the map's collision): main stick move, C-stick look, **A** jump, **B** stance (tap: crouch / stand, hold 0.3 s: prone / stand; IW4L's console stance button)
(held), **Z** sprint / hold breath (aimed with a scoped rifle; one button for both, as MW2's console `+breath_sprint`), **L** ADS, **R** fire, **X** reload, **Y** switch weapon (the gun drops, then the other raises,
as in MW2), **D-pad right** melee, **D-pad up** lethal grenade (hold to cook), **D-pad down** tactical grenade,
**D-pad left** calls in your newest killstreak reward. **Z + D-pad up** toggles free fly (main
stick, C-stick, L/R triggers down/up, B fast); leaving free fly drops the player where the camera is.
**Z + D-pad down** toggles the performance overlay (fps, frame / CPU /
GPU-wait ms, triangles (of which static models), draws, state changes, visible cells, models, position; see
`docs/perf.md`). A pack without a `CLIP` section starts in free fly.

Domination (Game Setup): the map's flags (`OBJS`) are taken as `_gameobjects.gsc` takes a proximity objective: the first team inside one it may take claims it, the claim fills at `min(players, 4)` times real time while nobody else is inside, and is lost when they all leave; a full claim (`dom.gsc`'s use time) gives the flag to the team and the capture points to the first in. Every score tick each held flag adds a point to its team; kills don't. Bots without an enemy to chase head for the nearest flag their team doesn't hold and wait inside it. The HUD line shows each flag's holder and your capture progress.

Counter-UAV (a class killstreak, D-pad left): up as long as a UAV (`_uav.gsc`), it jams your enemies' radar: theirs dims and shows nothing but themselves, and bots under it lose their UAV's knowledge. An enemy's jams yours the same way.

EMP: jams the enemies' radar as a Counter-UAV does, for `_emp.gsc`'s jam time (60 s in MW2's). Not yet: its HUD blackout and its effect on other killstreaks. Tactical Nuke: `_nuke.gsc`'s countdown on the HUD, then everyone dies (`nukeDeath`'s delay after it) and the match ends with its caller (their team) winning.

Player characters: the other players are MW2's bodies for the map's factions (`CHAR`: each team's assault body and head, a low LOD so the Wii can skin several; the head's vertices are in its own model space, so its bones, those it shares with the body too, unbind with the head's own bind poses and follow the body's), animated by MW2's player animation script (`PSCR`, the user's `mp/playeranim.script` baked at convert time; clips `PANM` read in place from MEM2, bound by bone name at load). Each frame a player's movement type (IW4L's `footsteps_anim_move_type`), weapon (player-anim type and class), ADS, strafing and firing pick the script's first matching item; its animation plays on the legs, the torso or both, cross-fading over IW4's blend time. Firing, reload, melee and grenade throws play their event's torso animation for its duration; a death plays the death item matching the hit location, the direction the damage came from and whether an explosion did it, and the body lies there until the respawn. IW4's controller tags bend the spine to the aim pitch and turn the torso from the legs, which follow the view once it is 20 degrees away. Skinned on the CPU and drawn at the player's position, turned with the legs; the weapon it holds (the common pack's `GUNM`: its world model, read in place) is drawn on its `tag_weapon_right` while it lives. Players behind you aren't posed or skinned (their animation state still runs). Without characters in the pack, the placeholder boxes are drawn. Hit detection still uses the boxes.

The match HUD (`game_core::hud_mw2`) uses MW2's fonts and HUD images from `common.wpk` (`UIPK` icons: each weapon's `hudIcon` as `weapon:<name>`, the killstreak table's icons as `streak:<name>`, MW2's hitmarker `damage_feedback`), the ammo counter's rounds `ammo_counter_*`), laid out by MW2's own SD-TV HUD menus (`UIPK` HUD layouts: `weaponbar_sd`, `scorebar_sd`, `dpad_sd`, kept in MW2's virtual coordinates with their alignment) and placed at run time with IW4L's `hud_iw4::ScreenPlacement` inside the safe area from Options (MW2's `safeArea_adjusted`; 90% by default), so nothing sits in a TV's overscan. Bottom right, the weapon bar: its backdrop, the magazine as rounds (`hud_iw4` clip pips by weapon class: bullets, rifle rounds, shells, rockets, belt), spent rounds dimmed, the reserve, the weapon's name, the lethal and tactical grenades stacked one icon a grenade, MW2's pulsing Reload / Low ammo / No ammo at the centre, and the d-pad's killstreak ready to call in. Bottom left, the score bar (`hud_iw4` scorebar): the clock in its dial (amber under a minute, red under 30 s), your score's bar and the leader's (or the enemy team's) filling towards the score limit with the scores beside them, and Winning / Losing / Tied in turn with the game mode; Domination's flags and the kill feed above it. The minimap sits top left inside the safe area (pings past its frame are left out). MW2's hitmarker and a `+points` popup at the crosshair, notices and the nuke's countdown at the top, and the respawn countdown when dead. HUD text is shadowed as MW2's is. Without the UI in the pack the plain HUD is drawn. `mwf-convert menu-preview <common.wpk>` renders it with sample content (`out/preview/menu_hud.png`).

**Xbox 360 wired pad (USB):** plug it into either USB port, before or during a match. Its light shows player 1 when
the game has found it. Both pads can be used at once, and the 360 pad also drives the menus (D-pad or left stick, **A**
accepts, **B** / **START** / **BACK** go back). It uses MW2's own 360 layouts (`game_core::controls`):

| Button | Default | Tactical |
|---|---|---|
| **A** | jump / stand | jump / stand |
| **B** | crouch, hold for prone | melee |
| **X** | reload, hold to use, double tap for C4 | the same |
| **Y** | switch weapon | switch weapon |
| **LB** / **RB** | tactical / lethal grenade (hold to cook) | the same |
| **LT** / **RT** | aim down sights / fire | the same |
| left stick click | sprint / hold breath | sprint / hold breath |
| right stick click | melee | crouch, hold for prone |
| D-pad right | killstreak reward | the same |
| **START** | pause | pause |
| **BACK** + D-pad up / down | free fly / performance overlay (developer) | the same |

The driver (`wii/platform/shim_usbpad.c`) needs IOS58, which the Homebrew Channel starts apps on. It finds pads by the
360 pad's USB interface (class 0xFF, subclass 0x5D, protocol 0x01), so third-party wired pads work too, but it only
opens devices from vendors that make them (Microsoft, Mad Catz, PDP, Hori, PowerA and others). That way it never
touches the USB LAN adapter. It reads input reports in the background, and when the pad is unplugged it looks for one
again about once a second. Triggers count as pulled past 30 of 255 (XInput's threshold), and the sticks go through
the same learned range and IW4L dead zone as the GameCube's. Not handled yet: the wireless receiver and rumble.

Options (OPTIONS on the main and pause menus, MW2's console Controls page; saved to `<app folder>/options.dat`): look sensitivity and aim-down-sight sensitivity (1-10, MW2's scale; 3 is the default), look inversion, target assist, button layout (Default, or Tactical: on the GameCube pad stance on D-pad right and melee on B; on the 360 pad stance
on the right stick click and melee on B), stick layout (Default, or Southpaw: move with the C-stick), safe area (80-100%, 90% by default: MW2's console safe-area adjustment, so the HUD stays inside a TV's overscan).

Debug menu (DEBUG on the main menu, and on the in-game menu during a match; A or left / right changes a setting; saved to `<app folder>/debug.dat`):
- Match: bots don't attack, bots frozen, god mode (no damage, no fall or out-of-world deaths), infinite ammo (magazine, reserve and grenades every frame), all killstreaks (every reward at the start of each life), primary weapon (any catalog weapon instead of the class's, no attachments; at the next spawn).
- Overlays (see-through boxes): hit boxes (head and neck red, body blue), bot goals (yellow posts where each bot is walking), objectives (Domination flag triggers, coloured by holder), bullet paths (your last six shots for 3 s, through walls).
- Performance: perf overlay at match start, character models (off: the placeholder boxes), world models, sound.
- In a match only: teleport to the next spawn or objective, bring a bot in front of you, kill all bots, UAV / Counter-UAV / EMP / nuke now, every killstreak reward, reset the match clock, write the log now. Actions are recorded in the match log (`debug` events).

Footsteps: each player's bob cycle wrapping is a step (IW4L), sounding `step_<gait>[_plr]_<surface>` (sprint, run, walk / crouch, prone; the surface under the feet; `_default` when a surface has none; `STEP` in `common.wpk`); yours as you hear them, others' where they are.

Grenades: your class's frag-type equipment (frag, semtex) is thrown through IW4L's offhand states (pull back, cook
while held, throw) and flies as IW4L's sim flies it (throw speed, gravity, the weapon's per-surface bounce, the fuse;
`WEQP` in `common.wpk`); the blast hurts everyone in reach, the thrower too, by IW4L's falloff and line-of-sight
scaling. Bots throw frags at enemies they see at grenade range. Grenades show as placeholder boxes until the projectile
models are converted; their explosions are MW2's effects (below). Their sounds play in the world (pull back, throw, the bounce once it hits hard enough, the explosion), panned and faded by the alias's distances. Semtex sticks where
it lands. Special grenades: the flashbang blinds by distance and where you look (IW4L's flashbang amounts, then
`_flashgrenades.gsc`'s duration; a white-out by the shock file's fade times; flashed bots see nothing); the stun
grenade's shock caps how fast you can turn (its `.shock` file's look control: 75°/s in MW2's, easing off over its last two seconds; the stick doesn't take the file's mouse sensitivity scale) and slows movement for `2 + 4 × closeness` seconds (`_weapons.gsc`). Teammates' flashes and stuns
don't affect you. The throwing knife flies as IW4L's knife does (no fuse; it sticks in what it hits square, skips off what it glances, lies where it slides to a stop), kills whoever it passes through with the weapon's damage, then drops; hold X over a knife you can carry (within 90 units, in sight) for a quarter second to pick it back up (IW4L's use-item rule; X doesn't reload while one is in reach). Claymores and C4 stick as IW4L's equipment does (a claymore to the ground, facing your throw; C4 to anything) and, without a timed detonation, lose their fuse once stuck. A claymore trips on an enemy moving inside its trigger, far enough ahead and inside its cone, in its sight (_weapons.gsc's radius, cone, minimum distance and grace period), then goes off with the weapon's 60° blast cone; C4 goes off when you double-tap X (pressed under 0.5 s, again within 0.5 s, watchC4AltDetonate). Two of each at once (scr_maxPerPlayerExplosives): a third blows the oldest; your charges go when you respawn. The smoke grenade waits until it lands (IW4L), then lays MW2's smoke: the particles of its effect's sight-blocking elements (WSMK: how many, where around the blast, their delays, lives, size and alpha curves), and a sight line through them loses each particle's alpha as IW4L's `get_client_visibility` takes it (bots can't see you once it drops under IW4L's threshold: about 2.75 to 10 s after it lands). Deviation: MW2's smoke elements have no distance fade, which IW4L's `vis_blocker_add_prepared` treats as not blocking at all; here they block at full strength. The particles show as see-through grey boxes until the smoke sprites are converted, and don't drift. Tactical insertion, tactical insertion, blast shield, smoke and deathstreaks are
chosen and saved but do nothing yet.
Weapons with attachments use the attachment variant's combat numbers (`m4_acog_mp` ...) but the base weapon's
first-person model.

Solid colour screens (START goes back to the menu): red = no SD card or no such pack, magenta = the pack failed its
checks (or is from an older converter: convert again), orange = out of memory or texture slots.

## Memory
The two weapon files of the spawned class load into MEM2 above the map (about 2 × 1.8 MiB) and are swapped when a
spawn brings other weapons (MEM2 and texture handles released back to a mark taken after the map).
The pack's GPU data (textures, vertices, display lists) is read into MEM2 and used in place; the sections decoded at
load (visibility, cell tables, collision, walk graph) are written last by the converter, read into a temporary MEM1
buffer, decoded into MEM1 and the buffer freed. Only the part before them stays in memory, so everything read in place
(characters, destructibles, objectives) is written before them (the converter's self-check refuses a pack that
doesn't) and read with `Pack::parse_prefix`. `mwf-convert pack` prints the MEM2 part and warns above 44 MiB. Leaving a map
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
konst ×2), then blended world and model surfaces back to front. The far plane is the debug menu's draw
distance (full by default); world draws and characters are fogged (the map's `FOGD` fog, and always when the draw
distance is short, ending at the far plane), the sky, first-person model, HUD and menus are not. The pack is loaded once into MEM2 and
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
The camera's feel (`game_core::view_kick`, IW4L's `weapon_iw4::view_bob` put together as IW4L's first-person camera
does): the eye bobs with your steps (up and down and sideways by stance, sprint and ADS; Lightweight bobs less),
dips on a landing as deep as the fall (`crash_land_view_dip`: falls over 12 units, at most 24 units, back over 450
ms), kicks away from a hit (towards the attacker's side; a fall or your own grenade kicks it up, IW4's 0.2° a point,
5° to 90°, over 100 ms out and 400 ms back) and, aimed with a scope, sways at idle by the weapon's ADS idle numbers.
Holding breath (Z / left stick click while fully aimed; IW4L's `movement_iw4` breath timer, 4.5 s, 5 s longer with
`specialty_holdbreath`) stills the sway; when the breath runs out the sway is worse until it recovers, and the
viewmodel's ADS idle sway follows the same scale. The angles also move your aim (the screen's centre is where shots
go); the eye's bob and dip only move the drawn camera. Memory: about 70 bytes of state, no pack data. IW4L's values;
a trace against MW2 is still to do.
Effects (`effects.rs`, `game_core::fx`, the common pack's `FXDF`): MW2's own effect definitions, played as IW4
plays their one-shot elements (count, delay and life ranges, spawn origin and sphere / cylinder offsets, the local
and world velocity graphs, gravity) and drawn as textured sprites whose size, colour and alpha follow the elements'
visual-state samples through IW4L's `fx_iw4::evaluate_*` (billboards, oriented sprites, tails, clouds; runners start
their child effects). Bullets that strike the world play the impact table's effect for the weapon's impact type and
the surface type; bullets that strike a player, its flesh effect. Explosions play the weapon's explosion effect, or
the impact table's for its impact type and the surface below. The other players' shots play their weapon's world
flash at the held gun's `tag_flash`; yours plays its view flash in the first-person model's space (it moves with
the view). Sprites are depth-tested, not depth-written: multiply first, then alpha blend, then additive. Not yet:
looping elements (lingering smoke, fire), decals and impact marks, models (shells, debris models), lights, trails,
screen distortion, collision. Without effects in the pack the old placeholders show (a glow for your flash, a box
for explosions).
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
  sized from IW4's hull and view heights, standing, crouched or prone (the standing boxes squashed to the hull), until the character models are converted: where a
  shot lands on the body is approximated, the damage numbers are not.
- **Bots** play with the same IW4L movement and weapon code as you, their guns from MW2's five default classes. They
  see 90° in front of them up to 5906 units with a clear line (IW4L's bot sensor), hear gunfire within 2000 units,
  step every 50 ms and decide every 100 ms like IW4's server bots, react after about a third of a second, turn at limited speed (IW4L's bot motor), fire inside an 8° cone with a small aim error that settles,
  strafe while fighting, chase what they saw or heard, and otherwise roam the map. They walk a graph the converter
  bakes from the map's collision (`NAVG` in the pack, flooded out from the spawn points on a 48-unit grid) with A*
  routes, and jump when stuck. Not yet: mantling, ladders, doors, grenades, knifing, their weapon sounds, and other
  players blocking movement (players pass through each other).
- **Bodies**: MW2's character models (see "Player characters"); the shaded boxes, one per hit volume, only without
  characters in the pack or with them turned off in the debug menu.
- **Destructible objects** (`game_core::destructible`, map pack `DST*`): MW2's cars, electric boxes, air conditioners,
  newspaper stands, copiers, fans and the like, placed as the map places them, with MW2's own models, health, effects
  and explosions read from its `_destructible_types*.gsc` at convert time (ADR 0007). Their main body runs MW2's chain
  of states: bullets do the weapon's damage at that range (four times for shotguns), explosions their damage times the
  type's splash scale or 13 and less 0.4 × the distance (only while the state names a tag, as in MW2), melee only
  where a state allows it; damage past a state's health carries on. A broken state plays its effects, starts its
  repeating smoke or fire and its health drain (a burning car counts down by itself), and may explode 0.1 s later:
  radius damage like a grenade's, from the last player who damaged it (that player gets the kills; with nobody, the
  victim's own), which also reaches other objects (chain reactions). The model then becomes the next state's (the
  wreck). Their collision boxes block movement and stop bullets (with an impact effect for the box's surface).
  Not yet: separate parts (tires, glass, doors, hoods), pieces flying off, their own sounds (the frag's explosion
  stands in), the destroy and idle animations (fans, chickens), light-grid lighting (drawn ×1), grenades bouncing off
  them, the glass windows (`glass_large` brush models).
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
Placeholder until the death cam / killcam flow is ported (ADR 0007): the 3 s respawn.

## Movement status
- `pmove` is IW4L's, unchanged; the context uses MW2's MP dvar defaults (190 speed, 800 gravity, jump 39, sprint ×1.5).
- Mantles: jump (or walk with Marathon Pro's auto-mantle) at a ledge whose brush is marked for mantling climbs it
  (on mp_rust 72 mantle brushes, 80 of the faces tried by `player::tests::mantles_on_the_real_map` climbable). The
  climb's path is IW4L's `CreateAnimsMantleRootDelta`: straight up by the ledge's height band and 16 units in over
  400 ms, then 31 units over the top over 200 ms for a mantle over; MW2 moves along the mantle clips' root tracks
  (IW4L `sim::mantle_xanim`), which aren't converted yet. Nothing animates the mantle (the viewmodel is lowered by
  pmove's weapon state; the third-person mantle clips aren't played).
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
- Xbox 360 pad over USB: written against libogc's USBv5 API and the pad's documented report format, but not yet
  run on a Wii. Check that the light comes on, that the menus and a match respond, and that system link over the USB
  LAN adapter still works with the pad plugged in.
- Sound from MEM2: ASND plays the clips in place from `common.wpk` in MEM2. If the gun is silent, the DSP may not
  reach MEM2 there, and the clips need copying to MEM1.

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

EMP: jams the enemies' radar as a Counter-UAV does, for `_emp.gsc`'s jam time (60 s in MW2's). While an enemy EMP is up your HUD is blacked out as MW2's is: no minimap, no crosshair (IW4's `OTHER_FLAGS` 0x400 gate), and `hud_fullscreen`'s static (`ac130_overlay_grain` at its forecolour 0.9 1 1 0.3) over the screen, its grain moving each frame. When any EMP goes off everyone's screen flashes (`_emp.gsc`'s `coup_sunblind`: in over 0.1 s, out over 3 s; a white fade until vision sets are ported). Not yet: its effect on other killstreaks and the scopes' EMP overlay materials. Tactical Nuke: `_nuke.gsc`'s countdown on the HUD, then everyone dies (`nukeDeath`'s delay after it) and the match ends with its caller (their team) winning.

Player characters: the other players are MW2's bodies for the map's factions (`CHAR`: each team's assault body and head, a low LOD so the Wii can skin several; the head's vertices are in its own model space, so its bones, those it shares with the body too, unbind with the head's own bind poses and follow the body's), animated by MW2's player animation script (`PSCR`, the user's `mp/playeranim.script` baked at convert time; clips `PANM` read in place from MEM2, bound by bone name at load). Each frame a player's movement type (IW4L's `footsteps_anim_move_type`), weapon (player-anim type and class), ADS, strafing and firing pick the script's first matching item; its animation plays on the legs, the torso or both, cross-fading over IW4's blend time. Firing, reload, melee and grenade throws play their event's torso animation for its duration; a death plays the death item matching the hit location, the direction the damage came from and whether an explosion did it, and the body lies there until the respawn. IW4's controller tags bend the spine to the aim pitch and turn the torso from the legs, which follow the view once it is 20 degrees away. Skinned on the CPU and drawn at the player's position, turned with the legs; the weapon it holds (the common pack's `GUNM`: its world model, read in place) is drawn on its `tag_weapon_right` while it lives. Players behind you aren't posed or skinned (their animation state still runs). Without characters in the pack, the placeholder boxes are drawn. Hit detection still uses the boxes.

The match HUD (`game_core::hud_mw2`) uses MW2's fonts and HUD images from `common.wpk` (`UIPK` icons: each weapon's `hudIcon` as `weapon:<name>`, the killstreak table's icons as `streak:<name>`, MW2's hitmarker `damage_feedback`), the ammo counter's rounds `ammo_counter_*`), laid out by MW2's own SD-TV HUD menus (`UIPK` HUD layouts: `weaponbar_sd`, `scorebar_sd`, `dpad_sd`, kept in MW2's virtual coordinates with their alignment) and placed at run time with IW4L's `hud_iw4::ScreenPlacement` inside the safe area from Options (MW2's `safeArea_adjusted`; 90% by default), so nothing sits in a TV's overscan. Bottom right, the weapon bar: its backdrop, the magazine as rounds (`hud_iw4` clip pips by weapon class: bullets, rifle rounds, shells, rockets, belt), spent rounds dimmed, the reserve, the weapon's name, the lethal and tactical grenades stacked one icon a grenade, MW2's pulsing Reload / Low ammo / No ammo at the centre, and the d-pad's killstreak ready to call in. Bottom left, the score bar (`hud_iw4` scorebar): the clock in its dial (amber under a minute, red under 30 s), your score's bar and the leader's (or the enemy team's) filling towards the score limit with the scores beside them, and Winning / Losing / Tied in turn with the game mode; Domination's flags and the kill feed above it. The minimap sits top left inside the safe area (pings past its frame are left out). MW2's hitmarker and a `+points` popup at the crosshair, notices and the nuke's countdown at the top, and the respawn countdown when dead. HUD text is shadowed as MW2's is. Without the UI in the pack the plain HUD is drawn. `mwf-convert menu-preview <common.wpk>` renders it with sample content (`out/preview/menu_hud.png`, and `menu_hud_scope.png` looking down a sniper scope under an EMP).

Over the play view (`game_core::hud_fx`, after IW4L's `hud` crate and `hud_iw4`; the numbers are IW4L's, a trace against MW2 is still to do): MW2's crosshair, from the weapon's own `reticleSide` / `reticleCenter` materials (`UIPK` HUD weapons: each weapon's HUD facts, a variant left out when it matches its base weapon), its four sides at `hud_iw4::calc_reticle_spread`'s gap for the stance's spread cone and the aim spread scale, shrinking with the ADS transition (`hip_crosshair_trans_scale`), fading towards 0.5 as spread grows (`calc_reticle_alpha`), and hidden by `hip_crosshair_visible` (reloading, sprinting, switching, flashbanged, fully aimed, EMP). The sniper scope (`hud_iw4::iris`): the weapon's `overlayMaterial` comes up over the last `adsZoomInFrac` of the ADS move (`calc_ads_overlay_zoom`), as one quad or four mirrored ones for a small overlay with black filling the rest of the screen, and the gun is not drawn while it is up (`viewweapon_drawgun`). The hit-direction arrows (IW4L `hud::damage_direction`): MW2's `hit_direction` round the crosshair towards whoever hit you, 12 slots, 6 s each (`cg_hudDamageIconTime`), fading over their second half, not while scoped. Blood: MW2's `splatter_alt` over the screen at `hud_iw4::blood_overlay_lerp`'s intensity (1 - health at once, back at 0.3 a second). The kill feed is MW2's game message window 0 (`hud_iw4::gamemsg`: bottom left at 6, -60 in the safe area, 4 lines of 18, 5 s each fading in over 250 ms and out over 500 ms): killer, the killer's weapon's icon (its `hudIcon`: IW4L's `killIcon` slot and name don't read back on the x86 zones; wide or square by `killIconRatio`, flipped by `flipKillIcon`; MW2's `killiconheadshot` for a head shot, `killiconsuicide`), victim. Earning a killstreak shows MW2's splash top centre (`mp/splashTable.csv`'s row for the streak: "&&1 Kill Streak!" with the kills, the description with D-pad Left for the key, its duration; the killstreak table's icon, as the row has none). In team modes your teammates' names float over their heads while in sight (`hud_iw4::overhead_*`: 82 above the origin, scaled from 1 at 64 to 0.7 at 512, fading out 1.5 s after they leave your sight).

Rumble (Options > Vibration, on by default): MW2's rumbles from the user's raw files (`rumble/<name>` and its two graphs, read as IW4L's `audio::rumble` reads them; `UIPK` rumbles), each weapon's shot rumble by its class among MW2's `*_fire` rumbles (assault, sniper, heavygun, smg, shotgun, pistol; `silencer_fire` for silenced weapons: IW4's `fireRumble` field isn't captured, ours) and `_damage.gsc`'s `damage_heavy` when you are hit, mixed as IW4L mixes them (the strongest playing rumble per motor). The GameCube pad's motor is on or off (libogc `PAD_ControlMotor` through the shim's `mwf_pad_rumble`), so it is switched on for a share of frames matching the strength; the 360 pad gets both motors' strength (`mwf_x360_rumble`, its USB rumble report, sent when it changes). The rumble stops on pause and when the match ends. Cost: `common.wpk`'s UI data grows by 45 KB (527 weapon entries, 8 rumbles, the splash texts) and 17 small textures (crosshairs capped at 32 texels, scope overlays at 256, the full-screen images at 128, the headshot and suicide kill icons at 64); the weapons' HUD icons, now also the kill icons, are capped at 64 texels, which saves 148 KB, so the pack is 103 KB smaller than before (16.93 MB). Decoded, the weapon table takes about 55 KB of the heap.

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
on the right stick click and melee on B), stick layout (Default, or Southpaw: move with the C-stick), safe area (80-100%, 90% by default: MW2's console safe-area adjustment, so the HUD stays inside a TV's overscan), vibration (the pad's rumble; on by default).

Debug menu (DEBUG on the main menu, and on the in-game menu during a match; A or left / right changes a setting; saved to `<app folder>/debug.dat`):
- Match: bots don't attack, bots frozen, god mode (no damage, no fall or out-of-world deaths), infinite ammo (magazine, reserve and grenades every frame), all killstreaks (every reward at the start of each life), primary weapon (any catalog weapon instead of the class's, no attachments; at the next spawn).
- Overlays (see-through boxes): hit boxes (head and neck red, body blue), bot goals (yellow posts where each bot is walking), objectives (Domination flag triggers, coloured by holder), bullet paths (your last six shots for 3 s, through walls).
- Performance: perf overlay at match start, character models (off: the placeholder boxes), world models, sound.
- In a match only: teleport to the next spawn or objective, bring a bot in front of you (an enemy when one is alive; it
  turns to you at once: a fresh last-seen contact in its memory), kill all bots, UAV / Counter-UAV / EMP / nuke now, every killstreak reward, reset the match clock, write the log now, GIVE one killstreak (care package to AC-130: on top of your list for D-pad left), and BOTS: RIFLES ONLY (the last line:
  every bot carries the M4 alone for the rest of the match, no launchers, riot shields, secondaries or grenades). Actions are recorded in the match log (`debug` events).

Footsteps: each player's bob cycle wrapping is a step (IW4L), sounding `step_<gait>[_plr]_<surface>` (sprint, run, walk / crouch, prone; the surface under the feet; `_default` when a surface has none; `STEP` in `common.wpk`); yours as you hear them, others' where they are.

Grenades: your class's frag-type equipment (frag, semtex) is thrown through IW4L's offhand states (pull back, cook
while held, throw) and flies as IW4L's sim flies it (throw speed, gravity, the weapon's per-surface bounce, the fuse;
`WEQP` in `common.wpk`); the blast hurts everyone in reach, the thrower too, by IW4L's falloff and line-of-sight
scaling. Bots throw frags at enemies they see at grenade range. Grenades show as placeholder boxes until the projectile
models are converted; their explosions are MW2's effects (below). Their sounds play in the world (pull back, throw, the bounce once it hits hard enough, the explosion), panned and faded by the alias's distances. Semtex sticks where
it lands. Special grenades: the flashbang blinds by distance and where you look (IW4L's flashbang amounts, then
`_flashgrenades.gsc`'s duration; a white-out by the shock file's fade times; flashed bots see nothing); the stun
grenade's shock caps how fast you can turn (its `.shock` file's look control: 75°/s in MW2's, easing off over its last two seconds; the stick doesn't take the file's mouse sensitivity scale) and slows movement for `2 + 4 × closeness` seconds (`_weapons.gsc`). Teammates' flashes and stuns
don't affect you. The throwing knife flies as IW4L's knife does (no fuse; it sticks in what it hits square, skips off what it glances, lies where it slides to a stop), kills whoever it passes through with the weapon's damage, then drops; hold X over a knife you can carry (within 90 units, in sight) for a quarter second to pick it back up (IW4L's use-item rule; X doesn't reload while one is in reach). Claymores and C4 stick as IW4L's equipment does (a claymore to the ground, facing your throw; C4 to anything) and, without a timed detonation, lose their fuse once stuck. A claymore trips on an enemy moving inside its trigger, far enough ahead and inside its cone, in its sight (_weapons.gsc's radius, cone, minimum distance and grace period), then goes off with the weapon's 60° blast cone; C4 goes off when you double-tap X (pressed under 0.5 s, again within 0.5 s, watchC4AltDetonate). Two of each at once (scr_maxPerPlayerExplosives): a third blows the oldest; your charges go when you respawn. The smoke grenade waits until it lands (IW4L), then lays MW2's smoke: the particles of its effect's sight-blocking elements (WSMK: how many, where around the blast, their delays, lives, size and alpha curves), and a sight line through them loses each particle's alpha as IW4L's `get_client_visibility` takes it (bots can't see you once it drops under IW4L's threshold: about 2.75 to 10 s after it lands). Deviation: MW2's smoke elements have no distance fade, which IW4L's `vis_blocker_add_prepared` treats as not blocking at all; here they block at full strength. The particles show as see-through grey boxes until the smoke sprites are converted, and don't drift. Tactical insertion and blast shield are
chosen and saved but do nothing yet (deathstreaks: see Life and match).
Weapons with attachments use the attachment variant's combat numbers (`m4_acog_mp` ...) but the base weapon's
first-person model. Burst weapons (the FAMAS, the M16) get IW4L's host-rule burst cooldown (200 ms,
`WeaponHostRules`, as IW4L's `ResolvedCombat` does); before, the converter wrote 0, IW4L refused their facts and a
class carrying one spawned with its secondary out.

Launchers (`game_core::grenade::Grenade::launch` / `fly`, `Arena::launch` / `missile_hits`): a projectile weapon (IW4
`WEAPTYPE_PROJECTILE`) fires a projectile instead of a bullet, in the same spread cone (IW4L `fire_accepted_shot`,
`spread_direction_on_plane`). Rockets (RPG, AT4, Stinger, Javelin) leave at the weapon's projectile speed plus your
velocity, each axis truncated to whole units, and fly straight (IW4L `fire_missile`, `TR_LINEAR`); grenade launchers
(the Thumper, the underbarrel launchers) are thrown like grenades (IW4L `grenade_launch_velocity`) and fall under
gravity, with no fuse when they have an arming distance (IW4L `grenade_deadlines`). Each step traces the world, then the
players along the step (not the shooter): a body takes the weapon's impact damage (IW4L `direct_hits`); past the
arming distance (`projectileActivateDist`) a projectile with `projImpactExplode` explodes there, through the same
radius damage and explosion effect as grenades (the sound is the frag's: launchers' own explosion sounds would add
2.8 MiB to `common.wpk`); before it, it is a dud (no blast); armed without impact
explosion it bounces. A rocket that hits nothing goes after 60 s. `WMSL` carries `projImpactExplode`, the guidance type,
the ignition delay and the lock-on rules. The Javelin locks on a point as MW2's `_javelin.gsc` does (`game_core::lock`:
full ADS, 12 eye-trace points 25 ms apart within 400 units, then 1150 ms with the point in the reticle, not under 1100
units away; stock numbers, not yet read from the user's script); locked, it leaves 0.3 up and 2.5 units right, falls,
climbs and dives on the point (IW4L `missile_guidance::steer`, top attack; IW4L slerps yaw / pitch rotations, here the
direction turns along the great circle by the same share). The gun now hands IW4L's weapon step the ADS fraction
(`WeaponCmd::f_weapon_pos_frac`; it was 0): ADS-only weapons (the Javelin) wait from the hip for the time left to aim
in (IW4L `ads_fire_only_delay_ms`) and fire at once aimed in, and the ADS fire / rechamber animations, the start of the
reduced-kick window and the melee check see the real fraction. The Javelin and Stinger need a lock to fire when their
weapon file says so (`requireLockonToFire`). The Stinger only locks onto killstreak aircraft (`level.stingerTargets`),
which aren't in this build, so it has nothing to lock onto. Bots fire their launchers the same way. Projectiles show as
the grenades' placeholder boxes until projectile models are converted; no smoke trail.

Alternate weapons: a weapon file's `altWeapon` (the underbarrel grenade launcher `gl_<weapon>_mp`, the underbarrel
shotgun `<weapon>_shotgun_attach_mp`) is carried as its own gun with its own ammunition (`arsenal.rs`: primary,
secondary and each one's alternate). D-pad left on the 360 pad (MW2's `+actionslot 3`; Z + D-pad left on the GameCube
pad, whose D-pad left calls killstreaks) switches to it and back through IW4L's alternate switch (`alternate_switch`:
the weapon's `altDropTime`, then the alternate's `altRaiseTime`); Y from either goes to the other weapon. The alternate
shows its weapon's first-person model with its own animations (`gl_<weapon>_mp` and `<weapon>_shotgun_attach_mp` name
their own clips on the weapon's model): its idle, fire, reload (the shotgun's segmented one too), raise and drop, from
the weapon file's companion (`views/<base>@<set>.wpk`, `docs/pack.md`), fitted to the alternate's own timers; slots it
shares with its weapon play the weapon's clips. It plays its own sounds (fire, reload, notetracks) from the companion.
A weapon picked up off the ground brings its own alternate.

Akimbo (`<weapon>_akimbo_mp`): both guns step through IW4L's `weapon_hands` (`last_weapon_hand` 1), each with its own
magazine and the reserve shared (IW4L `spawn_clip_stock`); as MW2 binds a pad for akimbo, the fire trigger fires the
right gun and the aim trigger the left (it never aims down the sights: the player gets no ADS with a pair). In first
person each gun is the weapon's model posed by its own hand's clips: IW4's `szXAnimsRightHanded` and
`szXAnimsLeftHanded` each animate the whole hands-and-gun rig with one arm holding the gun on `tag_weapon` (the other
arm out of view), so the left gun is the model drawn a second time with its own anim tree, driven by the left hand's
`weap_anim` (it fires, reloads and raises apart from the right), its own muzzle flash and the same placement. The HUD
shows both magazines (MW2's weapon bar item `AMMO_CLIP_GRAPHIC_LEFT`), and the low-ammo warning looks at both (IW4L
`draw_player_weapon_low_ammo_warning`). The dual-wield flag comes from the weapon data: IW4's `noDualWield` (WeaponDef
byte 0x66b, IW4L's `no_dual_wield`) is clear on every akimbo file and set on its base; IW4L's IW4 capture still says
`dual_wield: false`, so the converter reads it as IW4L's `fpv` reads `dual_animation`: `noDualWield` clear and both
hands' idle animations named (the killstreak and vehicle weapons that also have it clear name none).

Riot shield (`game_core::shield`): carried in the hands it covers the front, as the other weapon the back (MW2's
`trackRiotShield`: `tag_weapon_left` / `tag_shield_back`), for you and for bots (a bot holding one). A bullet that meets
it before the body stops there with no damage, striking the shield's surface (IW4L `segment_is_shield`, hit location
19, surface 29); a projectile explodes on it without its impact damage. Blasts: MW2's scripts don't scale explosive
damage for the shield (`_damage.gsc` only launches the dropped shield); the engine's attached shield model blocks.
IW4L's radius damage doesn't model that, so here the shield hides what it covers from the blast's visibility traces
(`can_damage_player_vis_scale`). The shield is a box in the player's frame, as wide as the body's hit boxes (IW4L poses the
shield model's collision bones, which aren't converted); a trace against MW2 is still to do. Not yet: shield ricochets (IW4L
bounces some bullets off it), melee against it, the shield model on the character's left arm or back (the held weapon
draws on `tag_weapon_right`; only bots are drawn and they carry one weapon).

These follow IW4L's code (ported: `sim/src/missile.rs` `fire_missile`, `sim/src/equipment.rs` `think_projectile`,
`grenade_deadlines`, `bounce_missile`, `sim/src/missile_guidance.rs` `steer` / `turn_toward`, `sim/src/shield.rs`,
`sim/src/combat.rs` `segment_is_shield`, `weapon_iw4` `weapon_hands` / `weapon_change`; `console/src/binds.rs`
`gameplay_binding` for the akimbo triggers) with host tests against its values (the alternates' and akimbo hands'
clips are fitted to their weapon's IW4L timers: `weapon_anim` `real_companion_sets_play_in_their_weapons_times`); a
trace against MW2 is still to do.

Solid colour screens (START goes back to the menu): red = no SD card or no such pack, magenta = the pack failed its
checks (or is from an older converter: convert again), orange = out of memory or texture slots.

## Memory
The two weapon files of the spawned class load into MEM2 above the map (about 2 × 1.8 MiB) and are swapped when a
spawn brings other weapons (MEM2 and texture handles released back to a mark taken after the map).
Launchers, alternates, akimbo and the shield add 14 KiB of MEM2 (`common.wpk`'s `WMSL`, 12 bytes a weapon), and the
projectiles, the second hand and the Javelin's lock a few hundred bytes of MEM1. The akimbo hands' and alternates'
clips (and an alternate's sounds) are in companion files loaded only for a class that carries them (MW2 data, default
converter flags, MEM2): an underbarrel grenade launcher 0.28-0.33 MiB (60-115 KiB of clips, the rest its sounds at
32 kHz), an underbarrel shotgun 0.39-0.50 MiB (80-90 KiB of clips), an akimbo pair 0.08-0.19 MiB (clips only) plus the
left gun's skinned positions (2 × 12 bytes a vertex, 0.13-0.16 MiB). So a class costs at most about 0.83 MiB more MEM2
(a rifle with the underbarrel shotgun and an akimbo secondary), a class without them nothing. The clips also decode
into the MEM1 heap at about the size of their `VCLP` section (key-framed tracks stay keyed): at most about 0.27 MiB
for such a class. The weapon files themselves grow by their `VSET` (under 1 KiB).
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
the map (IW4 `MASK_SHOT`); impacts show as yellow markers for 400 ms. HUD: crosshair (MW2's, see the HUD above; four ticks without the UI in the pack), magazine / reserve, weapon.
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
the view). Sprites are depth-tested, not depth-written: multiply first, then alpha blend, then additive.
Looping elements (the smoke grenade's lingering cloud, fires) spawn on IW4's schedule while their effect's looping
life lasts (IW4L `fx_iw4::looping_spawn_schedule`, `looping_catchup_begin`; IW4L `fx::spawn_looping_partial`): one
particle every `spawn[0]` ms, up to `spawn[1]`. Caps: 24 loops at once, 30 s at most each (IW4's "until stopped"
loops too: nothing stops an effect by handle yet), and looping spawns wait while the 384-particle pool is over
three quarters full so impacts and flashes keep their room. The smoke grenade's cloud is its explosion effect's own
looping sprites (the grey boxes only show without effects in the pack); its sight blocking is still `smoke.rs`'s.
Not measured yet: the fill-rate cost of a full smoke cloud on hardware (large see-through sprites).
Tracers (`game_core::tracer`, `FXTR` / `FXWT`): each class weapon's IW4 `TracerDef` (material, draw interval, speed,
beam length and width, colours). Ported from IW4L `render_fx::tracer`: a shooter draws one every `drawInterval`
shots (IW4L `TracerDrawGate::should_spawn`), none for paths under `FX_TRACER_MIN_DIST`, yours at most
`FX_TRACER_FIRST_PERSON_MAX_WIDTH` wide; it runs from the muzzle at its speed and its beam is clipped at the hit.
Colours along the beam from `fx_iw4::beam_sample_color`; drawn as one camera-facing additive quad (the screw wiggle
is left out; MW2's MP tracers have none).
Bullet marks (`game_core::marks`, the impact effect's first decal element: its colour material and size): a pool of
48 for 30 s (fading over the last second), laid on the surface the bullet hit. The Wii has no world triangles on
the CPU, so where IW4 clips a mark against the drawn triangles (IW4L `marks_iw4::mark_clip_world_surfaces`) the mark
here is a square on the hit plane with a random roll, cut back where the surface ends: short collision traces across
each side find how far the plane goes, and IW4L's `marks_iw4::mark_chop_world_poly_behind_plane` chops the square
there. Drawn right after the world through the shim's decal pass (`mwf_decals`: fogged, lifted 0.25 units and pulled
towards the eye in window depth). Not yet: marks on players and models, exit marks of penetrating bullets.
Not yet: models (shells, debris models), lights, trails, screen distortion, collision. Without effects in the pack the
old placeholders show (a glow for your flash, a box for explosions).
Sounds: the gun plays MW2's player-view sounds through libogc's ASND mixer (16 voices): fire, empty click, reload
and raise from the weapon's events, and the reload's clip-out / clip-in / chamber sounds from the first-person
animation's notetracks (mapped through the weapon's `notetrackSoundMap`). Each play picks a variant, volume and
pitch from the alias's ranges, as IW4 does. They come from `localized_common_mp` (where MW2 MP keeps its sound
aliases and clips), stored in `common.wpk` as mono 16-bit PCM at the original 44.1 kHz (~0.5 MiB for the M4).
World sounds fade by the alias's distances and pan by where they are.
Other players' gunfire: the world variant of each weapon's fire sound (IW4 `fireSound`; IW4L `select_fire_alias` for
someone else's shot) at their gun's muzzle, from `WSNW` in `common.wpk` (the weapons the bots carry: the default classes'
primaries with their attachments; resampled to 14 kHz and their quiet tails cut: heard from afar, and MEM2 is short). Each shooter holds one ASND voice: a new
shot stops their previous one (`sound::Voice`, the shim's `mwf_snd_stop_if`), so a firefight can't take all 16.
MW2's hitmarker sound (`MP_hit_alert`, IW4L `gamemode_iw4::HIT_ALERT_ALIAS`, `SNDM`) plays with the hitmarker, once a
frame. Your knife plays IW4's melee cues (IW4L's IW4 melee cue policy, knife first): `melee_knife_hit_body` on a
player, `melee_knife_hit_other` when the swing reaches the world, nothing on air (the swipe comes from the animation).
Bullet impacts would play their impact effect's sound element (`Effect::sound`, two a frame at most), but MW2's
impact table effects carry none in the user's data (the converter reports `effect sounds: 0 of 0 named`) and IW4L has
no other bullet impact sound code, so impacts are silent for now. Not yet: the bots' knife cues, destructible objects' own sounds (their effects live in the map
zone's bank, which no pack carries yet).
Not yet: damage to players, other weapons and attachments.

Light for moving models (`game_core::light`, map pack `LGRD`, `wpk::light`): the converter samples the map's light
grid on a regular lattice over the world's bounds (IW4L's atapoint lookup at each point, `static_models::
light_lattice`: the colour a white model gets there, as static models; 96 × 96 × 64 units, coarser until at most
12,288 points), and the Wii blends the eight points around a model's lighting origin (unlit points left out). The
viewmodel, the characters (at their middle, 36 units up; the weapon in the hand too) and the destructible objects
(at their origin) take it as their konst colour. Without a lattice they stay ×1. Not yet: IW4's per-texel model
lighting (one colour per model here), the sun's direction, dynamic lights.

Memory this adds (MEM2): the weapon world sounds in `common.wpk` (the converter prints `weapon world sounds: N MiB`),
the impact marks' decal textures (at most 32 texels), the light lattice (at most 48 KiB a map; 26 KiB on mp_rust),
the tracer definitions (108 bytes each) and the mark pool (48 marks, 9 KiB); the match log's `avfx` line at load gives
the lattice, tracers and pool, `avfx_counts` every 5 s what played (see `docs/gaps.md` for the measured totals).

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
Parts the base weapon hides (sights, heartbeat sensor) are left out at conversion. Lighting: the map's light lattice
(`LGRD`, below) at IW4's viewmodel lighting origin (the eye moved by the lean; IW4L
`lighting_iw4::viewmodel_lighting_origin`, ported in `game_core::light`), as a TEV konst colour (`DrawState::tint`)
the way static models are lit. Check it on the PC:
`mwf-convert preview <map.wpk> --viewmodel out/packs/common.wpk [--anim <slot> --time <s>]` (slot 9 = reload).
Not measured yet: the CPU cost of skinning 7.6k vertices per frame on Broadway.

## Bots and damage (free-for-all)
A match is a free-for-all against 5 bots (`BOTS` in `wii/game/src/lib.rs`), first to 30 kills or the 10-minute
clock (MW2 FFA's defaults), then a scoreboard. The top right shows your kills and the leader's, the left side a kill
feed, and hits show MW2's hitmarker.
- **Damage** is each weapon's own: IW4L's range falloff (full damage to its max-damage range, down to its minimum at
  its min-damage range) times the hit location's multiplier from MW2's `mp_lochit_dmgtable` (read from your copy and
  baked into each weapon by the converter, as IW4L does).
- **Hit volumes** are oriented boxes (IW4L's `ray_obb`). Your shots test a drawn body's bones: each character bone's
  box from MW2's own model (`XBoneInfo` bounds and `partClassification` hit location, map pack `CHHB`), posed as the
  body is drawn, used while it was drawn in the last 50 ms (what you see is what you hit). Bots' shots, and bodies
  not drawn (behind you, or you), test a stand-in column of boxes (head, neck, upper and lower torso, arms, upper and
  lower legs) sized from IW4's hull, squashed to the stance's height and turned with the view's yaw. A broad test
  around each body comes first; the match log's `hitvol` line gives the cost every 5 s (bodies tested, of them on
  bones, boxes, µs).
- **Through players** (IW4L `fire_penetrate` / `fire_extended`): every player along the shot is hit in order. A
  penetrating weapon loses `thickness / depth` through each body (the flesh depth for its type from
  `bullet_penetration_mp`, times its multiplier), as through walls, until nothing is left; a non-penetrating rifle
  bullet (`bRifleBullet`) goes on at half damage each time; anything else stops at the first body. Pack `WPEN`
  carries the rifle flag (packs from before need converting again; they read as no flag). A destructible object stops
  the bullet. Damage numbers follow IW4L; a trace against MW2 is still to do.
- **Glass** (`game_core::glass`, IW4L `entity_iw4::glass`'s rules): the map's breakable panes are its clip brushes
  that carry a glass piece. Bullets go through intact panes and each pane takes the bullet's damage at that
  distance; 25 weakens it (a weakened pane may fall apart by itself 3 s to 10 min later, IW4's random collapse),
  100 shatters it; a melee swing does 1000, grenades and exploding objects 4 × their damage within 256 units.
  A shattered pane drops out of every trace: bullets, bots' sight and movement pass. The break: a spray of glass
  impacts over the pane. MW2's glass pieces aren't drawn (they're FX glass); a faint stand-in box shows each intact
  pane within 1500 units (brighter when weakened) and goes when it shatters. Not yet: the shatter pieces, the
  breaking sound, the glass pane drawn as MW2 draws it.
- **Weapons on the ground** (`game_core::item`, IW4L `sim/src/item.rs`; MW2's `_weapons.gsc` rules): a player who
  dies drops the weapon in hand when its clip has rounds (its reserve capped at the maximum), thrown a little as
  IW4L throws it; it falls, lands and stays 60 s. Killed by someone else, they also drop a Scavenger bag. At most 16
  items; the oldest goes. Walking over a weapon you carry tops up its reserve (IW4L's split: reserve first, what
  doesn't fit stays); walking over a bag with Scavenger gives a clip's worth of reserve to each weapon and one more of
  each grenade. Looking at a weapon you don't carry within 128 units shows "HOLD Y TO SWAP FOR ..."; hold Y 250 ms to
  take it with its rounds in place of the one in hand, which is left where it lay (a tap of Y still switches). The
  new weapon's first-person file loads at the swap (a short hitch). Bots drop weapons and bags and take ammunition
  by touch; they don't swap. Dropped weapons are drawn with their third-person model on its side; bags as a small
  stand-in box (no bag model is packed). Not yet: the pickup and Scavenger sounds, the HUD's weapon icon in the hint.
- Memory: the per-bone boxes are 28 bytes a bone in the map pack (102 for `mp_terminal`'s two characters, 2.8 KiB; bones without a hit location, the attached head's own, are left out),
  and up to 64 posed boxes per fighter on the heap (about 4 KiB each); glass state is 8 bytes and a 24-byte box per
  piece; items 16 × 80 bytes. Together well under the round's 0.2 MB.
- **Bots** play with the same IW4L movement and weapon code as you, each life with one of MW2's default classes at
  random (its primary in hand, its secondary held back for IW4L's reload-or-switch choice below). They
  see 90° in front of them up to 5906 units with a clear line (IW4L's bot sensor), hear gunfire within 2000 units,
  step every 50 ms and decide every 100 ms like IW4's server bots, react after about a third of a second, turn at limited speed (IW4L's bot motor), fire inside an 8° cone with a small aim error that settles,
  strafe while fighting, chase what they saw or heard, and otherwise roam the map. They walk a graph the converter
  bakes from the map's collision (`NAVG` in the pack, flooded out from the spawn points on a 48-unit grid) with A*
  routes, and jump when stuck. From IW4L's `bots` crate (logic ported, `game_core::bot`): what they know of their
  enemies is IW4L's contact memory (seen this think, last seen, forgotten after 4 s; a lost contact is a lead to go and
  look at for 2 s), each contact's confidence fading once out of sight and heard ones at half (ours); each bot has
  IW4L's aggression and self-preservation from its seed; wounded (at or under `20 + 20 x self-preservation` health)
  and out of sight, or out of rounds in a fight, it backs off 192 units from the threat crouched (IW4L's Recover), and
  crouches to fight under 60 health; with the magazine empty it switches to its loaded secondary when that is quicker
  than a reload and an enemy is in sight, or when nothing is left to load (IW4L `weapon::choose`). They throw frags
  and knife (`bot.rs`). Not yet: mantling, ladders, doors, their weapon sounds, IW4L's task scoring as a whole (Hunt /
  Investigate / Fight / Recover / TouchObj: only the parts above). Their perks: see Perks below.
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
  stands in), the destroy and idle animations (fans, chickens), grenades bouncing off
  them, the glass windows (`glass_large` brush models).
- Spawns: the free-for-all spawn point furthest from everyone alive.
- Maps converted before this need converting again for the walk graph; without one, bots stand and fight.

## Life and match
Health 100; fall damage from IW4L's crash land (falls under 128 units are free, 300 and over kill); dropping 2048
units below the lowest spawn point kills (MW2 uses trigger volumes). A death runs MW2's death sequence (IW4L
`killcam_iw4::DeathSequence`: a quarter second to watch yourself die, a double tap of X cancels the killcam, the
1.5 s death delay), then the killcam (`game_core::killcam`, below), then you respawn at the spawn MW2's
free-for-all weighting picks, with a fresh gun (no respawn delay: `scr_player_respawndelay` is 0 in MW2). Health regenerates as MW2's
`_healthoverlay.gsc` does (numbers read from your scripts). A red vignette shows missing health and flashes on
damage; the bottom left shows the match clock (the time limit from your `dm.gsc`) and deaths. A free-flying player
(D-pad up) is out of the match: the bots ignore it.
The minimap (top left, MW2's `minimap_fullscreen` layout) turns with your view; enemies who fire show for 2 s.
Killstreaks: kills in a row since your last death earn your class's streaks at their killstreakTable cost; D-pad left
calls the newest. The UAV runs MW2's radar sweep for its script time (every enemy marked as the sweep passes); the
Counter-UAV, EMP and nuke are below. The rest is `game_core::killstreaks`, written natively from MW2's killstreak
scripts' values (ADR 0007: the converter's `streak_rules.rs` reads `_airdrop`, `_remotemissile`, `_autosentry`,
`_airstrike`, `_harrier`, `_helicopter` and `_ac130.gsc` into `KSRL`; stock values with an older pack) with IW4L's
native pieces where it has them; every reward is owned by a fighter, so bots call the same ones:
- **Care package / emergency airdrop**: the marker (`airdrop_marker_mp`, thrown with its own grenade numbers, going
  off where it comes to rest) brings a Little Bird from 15 000 units out at a random heading, 850 above the marker,
  fast for 2 s and then slow, braking onto a point 50 units short of it, where it drops the crate (about 7 s; MW2's
  `doFlyBy`); the emergency airdrop's C-130 flies the thrower's line at 2000 units a second and drops four crates as it
  passes over. Crates fall, crush whoever they land on (the owner's kill) and hold a reward rolled from MW2's crate odds
  (`getRandomCrateType`) or ammo. Hold X by one to take it: half a second for its owner, 3 s for anyone else (enemies
  too); unclaimed crates go after 90 s. A crate's reward joins your list for D-pad left; ammo refills both guns.
- **Predator missile**: launched 14 000 up and 7000 behind you at a point 1500 ahead; the view rides it. The C-stick
  steers (IW4L `remote_missile::steer`: 15 degrees a second of pitch, 20 of yaw, pitch kept between 1 and 87), it holds
  3000 units a second and R once boosts it to 6000 (IW4L `advance`); it passes through sky brushes and blows up on what
  it hits with `remotemissile_projectile_mp`'s radius and damage. Your body stays where it was.
- **Sentry gun**: carried as a preview (green where IW4L's sentry placement solver lets it stand, red where not); R
  sets it down. It runs IW4L's turret logic (nearest enemy in range, arcs and sight; 360 degrees a second turning, 2
  degree on-target cone, 30 a second scanning) and MW2's fire: a 0.05 s spin-up, 20-120 round bursts at
  `sentry_minigun_mp`'s rate with 0.15-0.35 s pauses, overheating after 8 s of fire until cool (a second a second), gone
  after 90 s.
- **Precision airstrike, Harrier strike, stealth bomber**: the target screen shows the whole minimap (north up): the
  main stick moves the cursor, the C-stick turns the run's heading, A or R calls it, B gives the reward back. Precision
  and Harrier send two planes 1.5-2.5 s apart at 7000 a second, each cluster bomb bursting into 12 traces pitched 55 to
  5 degrees ahead (512 radius, 200 to 30 damage); the stealth bomber carpets the line through the target (a bomb every
  0.1 s within 4500 units, each within 512 of its path, 896 radius, 300 to 50). The Harrier then hovers over the target
  for 45 s firing 25-round bursts of `harrier_20mm_mp`.
- **Attack helicopter, Pave Low**: in from the map's edge, circling the map's middle 1200 above it for 60 s (stand-in
  path: the maps' helicopter nodes are not converted), the gun (`cobra_20mm_mp`, the Pave Low's `pavelow_minigun_mp`)
  firing 40-round clips at the nearest enemy in 3500 units, starting 96 units off and converging 0.05 a shot, 0.5-2 s
  between clips. Bullets wear them down through their armour (a third: `heli_armor_bulletdamage`); 1500 health, the
  Pave Low twice that.
- **Chopper Gunner, AC-130**: your view rides the helicopter (40 s) or the AC-130 (40 s, circling the map's middle once
  every 70 s, 4000 out and 6000 up as a stand-in for its rig); the C-stick aims, R fires (`cobra_player_minigun_mp`;
  the AC-130's 105, 40 and 25 mm with their 5, 3 and 1.5 s reloads, Y for the next gun). You are out of the fight
  below meanwhile (bots ignore you).
Views from the sky draw the whole map without fog or the portal walk (`World::skip_vis`). The world models are
stand-in boxes (crates, the Little Bird, the C-130, planes, bombs, helicopters, the sentry); not converted yet: their
models, sounds and effects beyond the weapons' explosions, the kill cams, the artillery shellshock, flares, shooting
down the AC-130 or sentries, EMP's effect on killstreaks, MW2's HUD for each (the Predator's and AC-130's overlays).
Bots hold a set of three (`BOT_SETS` in `wii/game/src/streaks.rs`, MW2's default UAV / care package / Predator first)
at the killstreakTable costs and call each 1.5 s after earning it: the radar ones at once, markers thrown ahead of
them (they take crates in reach), sentries set down, Predators flown onto the nearest enemy, strikes on it, the
helicopters and AC-130 flown for them. Memory: the rules section is 352 bytes; the killstreaks in play are a few
kilobytes at most (well under the 0.3 MB this work may add to MEM2).
Killcam: every player's pose (position, view, animation inputs, weapon) is recorded each server frame (50 ms) into a
ring in MEM2 (`wii/game/src/replay.rs`: 10 s, 200 frames of 272 bytes a player; the log's `killcam_ring bytes=`:
327 KB with 5 bots in Dolphin, 436 KB with the most, 7), with the bullet impacts. After the
death delay the last seconds replay from your killer's eye: MW2's camtime chain (5 s before the death with no respawn
wait, 2 s after; IW4L `camtime`, `plan_window`, trimmed to what the ring holds by `recalc_after_first_frame`), the
other players' bodies at their recorded poses (their own animation state), the killer's weapon held in front of the eye
(its world model: the killer's first-person files aren't loaded), their muzzle flashes and the impacts. MW2's killcam
HUD (`game_core::killcam_hud`, `_killcam.gsc`'s elements): KILLCAM, the camtime countdown in tenths, who killed you
with what, "Press X to respawn"; a fresh press of X skips (IW4L `SkipEdge`). At the match's end the final killcam
replays the last kill (MW2's "game winning kill": 4 s of camtime, `doFinalKillCamFX`'s slow motion to a quarter
around the kill, no skip), then the scoreboard. Not yet: the killer's playercard, sounds in the replay, explosions
and grenades in it, MW2's spectator HUD hiding.
Perks beyond movement and damage (`game_core::perks`, applied in `game_core::arena`; numbers read from your
`_damage.gsc`, `_perkfunctions.gsc`, `_perks.gsc`, `_weapons.gsc`, `_class.gsc` and `_tweakables.gsc` into
`common.wpk`'s `LIFE`):
- Cold-Blooded: off the UAV's radar (yours and the bots'; IW4L `radar_contact_trail_visible` on the perk bits).
- Scrambler: enemies within 800 units see their minimap fade (full static within 350; IW4L `radar_jam_intensity`);
  bots under a UAV lose it past half. Its Pro: a claymore you trip waits 3 s.
- Ninja Pro: your steps are silent to the others and the bots don't hear them (bots hear other steps within 600
  units: ours, MW2's bots are players). MW2 plays faint `qstep_` aliases; this build plays none.
- Hardline: killstreaks a kill sooner (`setKillstreaks`' modifier); its Pro: deathstreaks a death sooner.
- One Man Army: Y opens the class menu (the match waits, unlike MW2's), then 6 s (3 s with its Pro) of "Changing
  Kit" with the weapon down, and the class is yours where you stand.
- Last Stand: a killing bullet or fall (not a headshot, explosion or melee: `mayDoLastStand`) puts you down at 1
  health, crawling (pmove's `LAST_STAND` flag and crawl scale, IW4L `sim::script_player`), the class's pistol in
  hand (bots: their sidearm or a Beretta), no grenades without its Pro, for 10 s; then you bleed out (the kill is the
  one who put you down's), or hold X 0.7 s to give up.
- Bomb Squad: enemy C4, claymores, frags and semtex marked through walls (`waypoint_bombsquad`).
- Silenced weapons: their shots don't ping the enemies' minimap and the bots don't hear them.
- Deathstreaks (deaths in a row without a kill, perkTable's count): Copycat (Y during the killcam takes your killer's
  class for one life; bots copy at once), Painkiller (a third of the damage for 10 s after spawning, its icon and
  timer on the HUD), Martyrdom (a live frag dropped where you die), Final Stand (down as Last Stand with your
  weapons, back up at full health after 20 s, once a life).
Bots carry their default class's perks and deathstreak (Lightweight's speed, Stopping Power ...).
Not yet: Cold-Blooded against the killstreaks that target players (they don't check it yet), Ninja against heartbeat sensors,
bots' One Man Army, a Beretta for a class without a pistol in your Last Stand, MW2's revive (Diehard mode).
A feel trace against MW2 is still to do for all of these: the numbers are the scripts', the code IW4L's.

## Movement status
- `pmove` is IW4L's, unchanged; the context uses MW2's MP dvar defaults (190 speed, 800 gravity, jump 39, sprint ×1.5).
- Mantles: jump (or walk with Marathon Pro's auto-mantle) at a ledge whose brush is marked for mantling climbs it
  (on mp_rust 72 mantle brushes, 80 of the faces tried by `player::tests::mantles_on_the_real_map` climbable). The
  climb's path is IW4L's `CreateAnimsMantleRootDelta`: straight up by the ledge's height band and 16 units in over
  400 ms, then 31 units over the top over 200 ms for a mantle over; MW2 moves along the mantle clips' root tracks
  (IW4L `sim::mantle_xanim`), which aren't converted yet. Nothing animates the mantle (the viewmodel is lowered by
  pmove's weapon state; the third-person mantle clips aren't played).
- Traces: world brushes through the BSP plus the triangle mesh (`clipmap_iw4::trace_brush_and_mesh`). Glass blocks
  until it shatters (see Glass). Not yet: brush models (doors, movers), penetration recovery.
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

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

## Audit against IW4L acc57a5 (2026-11-15)
Three read-only reviews (gameplay, presentation, network / input) against the pinned IW4L. IW4L runs MW2's scripts
for modes, killstreaks, last stand and deathstreaks; we don't (ADR 0007), so for those it offers primitives only.
Feel changes in this round take IW4L's code as the reference (host tests against its values); a trace against MW2
is still to do for each.

Bugs: ~~**mantle** moves nothing~~ fixed (2026-11-15): pmove gets IW4L's `CreateAnimsMantleRootDelta`, so a mantle
climbs over 400 / 600 ms (with the zero delta the player was put on the ledge at once and stood there for the mantle's
length). Mantles trigger on our collision (mp_rust: 72 mantle brushes). Still straight lines: MW2's paths are the
mantle clips' root tracks (IW4L `sim::mantle_xanim`), not converted; no mantle animation plays.
~~Launchers fire hitscan bullets~~ fixed: projectiles from IW4L `fire_missile` / `think_projectile`, guidance
(`steer`) and the Javelin's point lock from `_javelin.gsc` (`docs/runtime.md`, Launchers). Not yet: projectile models
and smoke trails, the lock's HUD box and sounds, the Stinger's lock (it locks only onto killstreak aircraft, none here),
the lock script's numbers read from the user's script, glass punch-through, the RPG's wobble.

Gameplay, IW4L has code: ~~camera view bob / landing dip / hit view kick / idle sway and hold breath~~ done
(2026-11-15, `game_core::view_kick` over `weapon_iw4::view_bob`; breath on the sprint button as MW2's console
`+breath_sprint`; IW4L's values, a trace against MW2 still to do; not yet: the viewmodel's own landing dip,
`viewweapon_land_origin_z`); ~~riot shield blocking~~ done (a box in front or on the back:
bullets stop, blasts are hidden; not yet the shield model's own collision, ricochets, melee, the model on the
character); weapon drops and pickups, Scavenger (`sim` item); ~~underbarrel grenade launcher / shotgun~~ done (D-pad
left; not yet their own first-person animations) and ~~akimbo~~ done (two hands, shared reserve; not yet the left
gun's first-person model, both magazines on the HUD, a dual-wield flag read from the data); hit boxes turned with the body and bullets through players (`sim`
bullet_collision, `fire_extended`); glass (`entity_iw4::glass`); killcam (`killcam_iw4` timing; we need a pose ring);
killstreak primitives (sentry turrets, Predator `remote_missile`, helicopters, care package marker; the flow is
hand-written from MW2's scripts); perks and deathstreaks with no effect yet (Cold-Blooded, Scrambler, Ninja,
Hardline, One Man Army, Last Stand, Copycat, Painkiller, Martyrdom, Final Stand); silenced weapons off the radar;
bot upgrades from IW4L `bots` (memory confidence, weapon switching, low-health recover, varied loadouts, perks).
Since done (hit detection and world objects, IW4L's code as the reference, a trace against MW2 still to do): hit
boxes turned with the body and per-bone boxes from the character models, bullets through players, glass, weapon
drops and pickups with the Y-hold swap, Scavenger bags (`docs/runtime.md`, "Bots and damage").

Presentation, IW4L has code: other players' gunfire, `MP_hit_alert`, melee sounds; looping effect elements
(`fx_iw4::looping_spawn_schedule`: lingering smoke, fire) and tracers (`fx_iw4::beam`); light-grid lighting for the
viewmodel and characters (`lighting_iw4` atapoint, viewmodel origin); HUD: sniper scope overlay (`hud_iw4::iris`),
MW2's crosshair (`hud_iw4::crosshair`), damage direction, blood / pain (`hud_iw4::blood`), overhead names, kill-feed
weapon icons, EMP static; bullet marks (`marks_iw4`); rumble. Not on the Wii's GPU: shadow maps, depth of field,
glow, colour grading, glass shatter.

Network: IW4L has no LAN transport (internet relay over QUIC); its snapshot deltas (`PS_NETFIELDS`), acked
baselines, prediction / reconcile, remote interpolation and lag compensation are logic to port into `wii/syslink`
when system link is next (not in this round).

We have and IW4L lacks: native Domination capture, bots that throw grenades / melee / strafe, native UAV /
Counter-UAV / EMP / nuke, destructibles with chain explosions, LAN discovery, GameCube and 360 pad drivers,
scripted Dolphin runs, the PC menu preview.

## Next, gameplay (ranked)
1. ~~Viewmodel ADS pose~~ done with the full viewmodel anim tree (every slot, timer-fitted rates, ADS overlay,
   per-bone blend) and MW2's weapon placement (view sway, bob, idle sway, movement offsets). Still to confirm: the
   stick range from the logs; the first-person lighting from the light grid.
2. ~~Weapon switching through IW4L~~ done: the gun asks for the other weapon (`usercmd.weapon`), IW4L drops it,
   the other takes the hand and raises (first raise once a life); refused mid-melee, cancelled by asking again.
3. **Melee**: done, lunge included. The melee button goes through IW4L's hand driver (`weapon_hands`:
   `weapon_try_melee`, swing and hit times); the hit is IW4L `fire_weapon_melee`'s nine traces (64 units) against
   the players' boxes, melee damage plus 0..4. Bots swing when you're within reach. The lunge: done (IW4L's client `melee_charge_target`,
   the weapon step's charge start, pmove's `melee_charge_move`).
4. ~~Players block each other~~ done: pmove's traces sweep against every other living player's hull with
   `clipmap_iw4::transformed_temp_capsule_trace` (bullets and sight still see the world only).
5. ~~Health regeneration~~ done: MW2's `_healthoverlay.gsc` loop step by step (`life::Life::regen`), the numbers
   read from the user's scripts (`RULE`).
6. ~~Bullet penetration~~ done: IW4L `fire_penetrate` through world walls (the user's `info/bullet_penetration_mp` depths by type and surface, each weapon's type and multiplier; thickness from a trace back from the far side; up to five walls; damage scaled by what is left). Through players: done (penetrating weapons lose thickness / flesh depth, non-penetrating rifle bullets go on at half, IW4L `fire_penetrate` / `fire_extended`). Glass panes: done (bullets go through intact panes and damage them). Not yet: impact marks on exits; a trace against MW2 for the collateral numbers.
7. ~~Hit volumes~~ done: oriented boxes; your shots use the drawn character's bones (MW2's `XBoneInfo` and `partClassification`, `CHHB`), bots' shots and undrawn bodies a stand-in column turned with the view yaw (cost in the log's `hitvol`). Not yet: bones for bodies you don't see (bots shooting each other or you), the riot shield's box beyond the stand-in (`game_core::shield`), a trace against MW2. Character models: drawn (`CHAR`), animated by MW2's own player animation script (`PSCR`, `PANM`): legs and torso animations by movement type, weapon type and stance, firing, reloading, melee, grenade throws, jumps (backwards too), hard landings (falling faster than 200 units/s), going prone and getting up, the hit flinch and stumble (pmove's damage timer picks FLINCH / STUMBLE movement types, as IW4L's `footsteps_anim_move_type`) and death animations (by hit location, direction and explosive damage), cross-fades with IW4's blend times, the spine turned to the aim pitch and the torso's yaw (IW4's controller tags), legs that follow the view after 20 degrees. The weapon in their hands: done (each weapon's world model on `tag_weapon_right`, `GUNM`). Not yet: the riot shield model on the left arm and back (the shield blocks already, `game_core::shield`), attachments on the held model, per-class bodies (riot, sniper, ghillie) and random heads, ragdolls and bodies that stay after the respawn, shellshock and stun torso animations, playback rate from the move speed, light-grid lighting.
8. **FFA rules**: points (a kill, the headshot bonus), score and time limits from the user's `dm.gsc`, ties to fewer
   deaths: done. Spawns follow MW2's free-for-all weighting (`_spawnlogic.gsc`: distance to everyone, enemies in
   sight, the nearest enemy, the last spawn used): done. Still to do: the playlist's overrides (the stock script
   limit is 1000 points).
9. ~~Crouch as MW2's console toggle; prone~~ done (tap / hold, IW4L's stance button). Footsteps done. Not yet: Ninja's quieter steps, bots hearing steps.
13. **Team deathmatch**: done (teams, team score, friendly fire off, team spawns, Game Setup with the scripts'
    limits). The stock script limits are low (500 points, 5 kills); MW2's retail playlists aren't readable from
    the install (`mp_playlists_dlc2`'s playlist is empty), so Game Setup offers longer limits. Domination: done (the map's flags from ``OBJS``; ``_gameobjects.gsc``'s proximity capture, ``min(players, 4)`` times real time, paused while contested; ``dom.gsc``'s limits, capture points, use time and score tick; bots head for flags they don't hold; HUD flag status). Not in Domination yet: MW2's flag-weighted spawns (TDM spawns are used), flag models, defend / assault points. Not yet: the other
    modes (Search and Destroy, Headquarters, Sabotage, Demolition, Capture the Flag), team start spawns
    for the first spawn, team killstreaks shared by the team (the UAV is yours alone).
12. **Grenades**: frag, semtex, the throwing knife (its hit, stick and pickup), claymore (trip and cone) and C4 (double-tap detonation) (lethal), flashbang, stun and smoke (tactical; the smoke's sight blocking) done (`game_core::grenade`, IW4L's throw /
    bounce / fuse / blast and flashbang amounts, the scripts' flash and stun durations, the shock files' white-out
    and slowdown; bots throw frags). Not yet: shooting charges to set them off, the C4 detonator in hand, the claymore's trip sound, Bomb Squad (Scavenger bags give a grenade of each kind: done), tactical insertion, blast shield; smoke sprites and drift;
    the shock's blur and tinnitus; the grenade models (explosion effects: done, MW2's own, one-shot elements); looping effect elements (lingering smoke, fire), impact marks; per-surface explosion sounds; the grenade
    danger indicator; Danger Close.
11. **Minimap and killstreaks**: the HUD minimap (map pack `MINI`: MW2's `setupMiniMap` frame, the script's
    `compassmaxrange`; turned with the view; firing pings), kill streaks with your class's streak costs, the UAV
    (radar sweep and ping fade from IW4L's compass, duration from `_uav.gsc`), bots' UAVs. EMP and the nuke: done. Counter-UAV: done (``_uav.gsc``: as long as a UAV, jams the enemies' radar: your minimap dims and shows nothing but you; bots under it lose their UAV's knowledge). Not yet: the other
    streaks (care package, predator, sentry, harrier, airstrike, helicopters, AC-130), EMP's HUD blackout, bots calling Counter-UAVs / EMPs / nukes,
    Cold-Blooded / Scrambler on the radar, the minimap's fading edge and scan-line pass order.
10. **Perks** (`game_core::perks`): a class's three perks and their Pro perks (perkTable's upgrade column;
    everything unlocked) become IW4's `ps.perks` bits that IW4L's movement and weapon code read (Marathon,
    Sleight of Hand, Steady Aim and their Pros, Commando's flag, ...); move speed by the primary's class and
    Lightweight's scaler come from the user's `_weapons.gsc` / `_perkfunctions.gsc` (`RULE`); Stopping Power and Danger Close add `_perks.gsc`'s
    `perk_bulletDamage` / `perk_explosiveDamage` percent to bullet / explosive damage. Scavenger bags: done (`game_core::item`). Not yet: Hardline and the killstreak perks, Cold-Blooded /
    Scrambler / Ninja against radar and sound, Last Stand, One Man Army, deathstreaks (the bits are set; nothing
    reads them).

**Destructible objects**: the main body of each (states, damage rules, effects, repeating effects, health drain,
explosions with chain reactions, model swaps, collision boxes) is done (`game_core::destructible`, map pack `DST*`).
Not yet: separate parts (tires, glass, doors), physics pieces, their sounds, destroy and idle animations, glass
windows, light-grid lighting, grenades bouncing off them. Memory: about 1 MiB (Checkpoint) to 1.7 MiB (Invasion) of
MEM2 per map; measure the frame cost of drawing 200 objects on hardware (`wld` section).

## Next, performance (ranked by expected milliseconds; measure with the logs first)
1. **World vertex load** (~12-25 ms on big maps): draw distance with fog and model LOD scale are in (debug menu),
   with an AUTO draw distance that follows the frame time (shrinks while GPU-bound frames miss 60 fps, grows back
   while they hold it; 2500 units at least) in place of per-map defaults; fixed per-map values can still come from
   hardware logs. Vertex welding is in (`mp_rust`: -1% world, -6% model
   display-list vertices, -1 MB MEM2); strips now start at border triangles (another -0.7%). IW4's world is many small
   surfaces with few shared edges, so strips stay short. Opaque batches now take meshoptimizer's strips (turning with
   repeated-vertex triangles) where they send fewer vertices: `mp_rust` world 402k -> 393k, models 262k -> 254k
   (~2.0 vertices per world triangle).
2. ~~**Run the CPU while the GPU draws**~~ done (draw-sync tokens, three XFBs, skinned positions double-buffered;
   frame = max(CPU, GPU), capped at one per retrace; not yet measured on hardware).
3. Draw submit: done: opaque static-model surfaces draw surface by surface, so instances of one model change only
   their light colour (`mwf_world_tint`, one konst register; the overlay's STATE shows full+tint changes); opaque
   world batches presorted at load (a frame sorts only models and blended surfaces). Not yet: shared quantisation
   origins.
4. Viewmodel and characters: skinned straight into their MEM2 buffers with a multiply for the weights (no copy, no
   divide). The viewmodel's rigid vertices (one bone at full weight) are placed by GX with their bone's matrix (a
   matrix index per vertex, at most ten per draw); only blended vertices are skinned on the CPU. Not yet: the same
   for characters, strips for the viewmodel's triangles.
5. ~~Weapon-file hitch at spawn~~ done differently: a whole class set is ~4-8 MB (files average 2.1 MB), too much to
   keep every class's loaded, so the next life's files stream in during the death screen (192 KB a frame, from 1 s
   after death); what's left at the respawn loads then.
6. Finer telemetry: the bots' CPU split into thinking, moving and shooting (`bai` / `bmv` / `bgun`, inside `bot`).
   Not yet: trace counts, A* expansions, world draw stages, GPU metrics. Bot trims wait for these numbers.

## Menus like MW2's
Done (2026-11-14): fonts and the main menu screen baked (`wpk::ui`, `UIPK`), drawn on the Wii as textured quads.
Then each screen's own layout: class select (`changeclass`), pause (`class`), Create-a-Class (`menu_cac_assault`) and
the scoreboard (`page_scoreboard`), with MW2's localized title / header text, filled bars, the back button's place
for the footer hint, and empty text slots filled live (the class panel: weapons, perks, equipment; the pause
screen's map and mode). The layout code is shared by the Wii and the PC preview (`game_core::ui_page`).
The class panel's pictures and icons: done (MW2's own image slots from the menus' material expressions: weapon pictures, attachment, perk, deathstreak, equipment and special grenade icons). Not yet: camouflage (needs the
guns' camo textures), the scrolling cloud layers (left out: they are moved by scripts and are the costliest fills),
animations, 16:9 layouts.
Done (2026-11-15): the Xbox 360's private match flow on MW2's console screens (lobby with the players, the Game Setup
popup, map / game mode / rules pages, the in-game START menu over the frozen game, MW2's End Game question), held
directions repeat, left / right change values, and MW2's SD HUD layout (`weaponbar_sd`, `scorebar_sd`, `dpad_sd`)
placed inside a safe area set in Options. Checked in Dolphin with `tools/dolphin-test.ps1`; not yet on a Wii. Not
yet: MW2's button glyphs in the footer hints. Map preview pictures and menu sounds: done (2026-11-15).
Done (2026-11-15): Create-a-Class in MW2's layout: the class page's buttons (Primary ... Deathstreak, Rename, Reset
Class), each choice in MW2's own popup (weapon type, weapon with its picture, attachments, perks with their icon,
description and Pro version, Reset Class's default classes), choices in MW2's order; killstreaks on MW2's Killstreak
Rewards page (from the lobby's Playercard popup), one set for the player; the class menu as the match starts. Left
out: Barracks (challenges, accolades, prestige: there is no progression) and the Playercard's Title and Emblem.

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

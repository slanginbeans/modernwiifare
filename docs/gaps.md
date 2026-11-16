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
The gap work landed on 2026-11-16 as seven merged branches (converter `REVISION` 39). The combined build ran every
pad script in `tools/autoplay/` in Dolphin. Every script got through, and their events show in the match logs.
Two misses on the first runs: you never went down in Last Stand (the script died to an explosion), and `hud_match`
got no killstreak splash (the bot brought in front carried a riot shield). Fixed the same day: the debug page's BOTS: RIFLES ONLY gives
those scripts steady opponents, and BRING A BOT IN FRONT brings an enemy that turns to you (`docs/testing.md`). Now
`last_stand` goes down in Last Stand, `hud_match` gets three kills and the splash, and `hud_match_rifle` gets the hit
arrows, the blood and a kill.
Guns, attachments and animations (merged 2026-11-16, converter `REVISION` 40, three branches): attachment parts per
variant and sight reticles, akimbo's left gun and the underbarrel alternates' own clips, mantle paths, the weapon's
landing dip, characters' playback rates, stun torso animations, the riot shield on the body, bodies that stay. Every
pad script ran in Dolphin with no panic; `weapons_akimbo_alt` and `weapons_gl_reload` need their custom
`classes.dat` (`docs/testing.md`) and then load their companion files and fire, reload and switch. MEM2 at load on
`mp_terminal` is 861 KiB free (1,803 KiB before this round): too little for the weapon files, so `mp_terminal` shows no
first-person gun (already so before this round); `mp_rust` has 18.3 MiB free, `mp_afghan` 7 MiB.

Bugs: ~~**mantle** moves nothing~~ fixed (2026-11-15): pmove gets IW4L's `CreateAnimsMantleRootDelta`, so a mantle
climbs over 400 / 600 ms (with the zero delta the player was put on the ledge at once and stood there for the mantle's
length). Mantles trigger on our collision (mp_rust: 72 mantle brushes). ~~Still straight lines~~ done (2026-11-16): the mantle
follows MW2's mantle clips' root tracks over their lengths (`MNTL`, `game_core::mantle_path`, as IW4L
`sim::mantle_xanim`; the real clips end on IW4L's straight-line points but rise first and take 333-833 ms up, 133-466 ms
over, against the flat 400 / 200), and characters play the script's `MANTLE_UP_*` / `MANTLE_OVER_*` clips.
~~Launchers fire hitscan bullets~~ fixed: projectiles from IW4L `fire_missile` / `think_projectile`, guidance
(`steer`) and the Javelin's point lock from `_javelin.gsc` (`docs/runtime.md`, Launchers). Not yet: projectile models
and smoke trails, the lock's HUD box and sounds, the Stinger's lock (it locks only onto killstreak aircraft, none here),
the lock script's numbers read from the user's script, glass punch-through, the RPG's wobble.

Gameplay, IW4L has code: ~~camera view bob / landing dip / hit view kick / idle sway and hold breath~~ done
(2026-11-15, `game_core::view_kick` over `weapon_iw4::view_bob`; breath on the sprint button as MW2's console
`+breath_sprint`; IW4L's values, a trace against MW2 still to do; ~~the viewmodel's own landing dip~~ done
2026-11-16, IW4L's `viewweapon_land_origin_z`); ~~riot shield blocking~~ done (a box in front or on the back:
bullets stop, blasts are hidden; not yet the shield model's own collision, ricochets, melee; ~~the model on the
character~~ done 2026-11-16, `tag_weapon_left` / `tag_shield_back`); weapon drops and pickups, Scavenger (`sim` item); ~~underbarrel grenade launcher / shotgun~~ done (D-pad
left; their own first-person animations and sounds from the weapon file's companion) and ~~akimbo~~ done (two hands,
shared reserve, the left gun drawn and animated by its own hand, both magazines on the HUD, the flag from IW4's
`noDualWield`; not yet: a trace against MW2); ~~attachments on the gun~~ done (first person and in hands, `VHID` /
`GUNH`; red dot and holographic reticles; not yet the thermal scope's thermal view, the heartbeat sensor's dots, a
readable ACOG chevron at 480p, the underbarrel alternates' held model: `gl_<weapon>_mp` records take the `gl` base's
model); hit boxes turned with the body and bullets through players (`sim`
bullet_collision, `fire_extended`); glass (`entity_iw4::glass`); ~~killcam~~ done (`killcam_iw4` timing over our pose ring);
killstreak primitives (sentry turrets, Predator `remote_missile`, helicopters, care package marker; the flow is
hand-written from MW2's scripts: done, see 11); ~~perks and deathstreaks~~ done (Cold-Blooded, Scrambler, Ninja Pro, Hardline,
One Man Army, Last Stand, Bomb Squad, Copycat, Painkiller, Martyrdom, Final Stand; silenced weapons off the radar;
bots carry their class's perks); ~~bot upgrades~~ from IW4L `bots` done (memory confidence, weapon switching,
low-health recover, varied loadouts in `bot.rs` / `arena.rs`, IW4L's values host-tested, a trace against MW2 still
to do).
Since done (hit detection and world objects, IW4L's code as the reference, a trace against MW2 still to do): hit
boxes turned with the body and per-bone boxes from the character models, bullets through players, glass, weapon
drops and pickups with the Y-hold swap, Scavenger bags (`docs/runtime.md`, "Bots and damage").

Presentation, IW4L has code: ~~other players' gunfire, `MP_hit_alert`, melee sounds;
looping effect elements (lingering smoke, fire); tracers; light-grid lighting for the viewmodel, characters and
destructible objects; bullet marks~~ done in this round (`docs/runtime.md`, Gun and First-person model; IW4L's code
as the reference, host-tested against its values; a trace against MW2 is still to do for each). Memory: see "Audio,
effects and lighting" below. Still to do there: bullet impact sounds (MW2's impact effects name none, IW4L has no other code for them), weapons other than the bots' default ones for others' gunfire, destructible objects' own sounds (their aliases live in the map
zone), the bots' knife cues, IW4's per-texel model lighting and the sun's direction, marks on models and players,
the tracer's screw wiggle, a fill-rate measurement of a full smoke cloud on hardware. HUD done (2026-11-16,
`hud_fx`; IW4L's numbers, a trace against MW2 still to do): sniper scope overlay, MW2's crosshair, damage direction,
blood, teammates' overhead names, kill-feed weapon icons, the killstreak splash, the EMP blackout and static, rumble.
Not in the HUD yet: the pain vision set and the splatter envelope (IW4L has the numbers, no caller), enemy names under
the crosshair, the scopes' EMP / low-res overlay variants (not captured), the cooking grenade's centre reticle, vision
sets (the EMP flash is a white fade), the weapons' own `killIcon` and `fireRumble` (not read back: `hudIcon` and a
rumble by class stand in), the viewmodel's `viewmodel_*` notetrack rumbles, rumble from nearby explosions
(`grenade_rumble`). Not on the Wii's GPU: shadow maps, depth of field,
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
   stick range from the logs. The first-person lighting: done (the light lattice at IW4's viewmodel lighting origin).
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
7. ~~Hit volumes~~ done: oriented boxes; your shots use the drawn character's bones (MW2's `XBoneInfo` and `partClassification`, `CHHB`), bots' shots and undrawn bodies a stand-in column turned with the view yaw (cost in the log's `hitvol`). Not yet: bones for bodies you don't see (bots shooting each other or you), the riot shield's box beyond the stand-in (`game_core::shield`), a trace against MW2. Character models: drawn (`CHAR`), animated by MW2's own player animation script (`PSCR`, `PANM`): legs and torso animations by movement type, weapon type and stance, firing, reloading, melee, grenade throws, jumps (backwards too), hard landings (falling faster than 200 units/s), going prone and getting up, the hit flinch and stumble (pmove's damage timer picks FLINCH / STUMBLE movement types, as IW4L's `footsteps_anim_move_type`) and death animations (by hit location, direction and explosive damage), cross-fades with IW4's blend times, the spine turned to the aim pitch and the torso's yaw (IW4's controller tags), legs that follow the view after 20 degrees. The weapon in their hands: done (each weapon's world model on `tag_weapon_right`, `GUNM`). Done 2026-11-16: the riot shield model on the left arm and back, bodies that stay after the respawn (the death pose held, a pool of four, the oldest reused), the shell shock and stun torso animations, the playback rate from the move speed (IW4L's `client_anim_playback_rate`, the cycle's phase kept between moving clips), the mantle clips. Attachments on the held model: done (`GUNH`). Not yet: per-class bodies (riot, sniper, ghillie) and random heads, ragdolls, a trace of the rates and mantle paths against MW2. Light-grid lighting: done (the light lattice at their middle, one colour each).
8. **FFA rules**: points (a kill, the headshot bonus), score and time limits from the user's `dm.gsc`, ties to fewer
   deaths: done. Spawns follow MW2's free-for-all weighting (`_spawnlogic.gsc`: distance to everyone, enemies in
   sight, the nearest enemy, the last spawn used): done. Still to do: the playlist's overrides (the stock script
   limit is 1000 points).
9. ~~Crouch as MW2's console toggle; prone~~ done (tap / hold, IW4L's stance button). Footsteps done; bots hear steps (600 units, ours), Ninja Pro's are silent. Not yet: MW2's faint `qstep_` aliases.
13. **Team deathmatch**: done (teams, team score, friendly fire off, team spawns, Game Setup with the scripts'
    limits). The stock script limits are low (500 points, 5 kills); MW2's retail playlists aren't readable from
    the install (`mp_playlists_dlc2`'s playlist is empty), so Game Setup offers longer limits. Domination: done (the map's flags from ``OBJS``; ``_gameobjects.gsc``'s proximity capture, ``min(players, 4)`` times real time, paused while contested; ``dom.gsc``'s limits, capture points, use time and score tick; bots head for flags they don't hold; HUD flag status). Not in Domination yet: MW2's flag-weighted spawns (TDM spawns are used), flag models, defend / assault points. Not yet: the other
    modes (Search and Destroy, Headquarters, Sabotage, Demolition, Capture the Flag), team start spawns
    for the first spawn, team killstreaks shared by the team (the UAV is yours alone).
12. **Grenades**: frag, semtex, the throwing knife (its hit, stick and pickup), claymore (trip and cone) and C4 (double-tap detonation) (lethal), flashbang, stun and smoke (tactical; the smoke's sight blocking) done (`game_core::grenade`, IW4L's throw /
    bounce / fuse / blast and flashbang amounts, the scripts' flash and stun durations, the shock files' white-out
    and slowdown; bots throw frags). Not yet: shooting charges to set them off, the C4 detonator in hand, the claymore's trip sound, Bomb Squad (Scavenger bags give a grenade of each kind: done), tactical insertion, blast shield; the sight-blocking particles' drift (the smoke's look: done, its effect's looping sprites);
    the shock's blur and tinnitus; the grenade models (explosion effects: done, MW2's own, one-shot elements); per-surface explosion sounds; marks from explosions; the grenade
    danger indicator; Danger Close.
11. **Minimap and killstreaks**: the HUD minimap (map pack `MINI`: MW2's `setupMiniMap` frame, the script's
    `compassmaxrange`; turned with the view; firing pings), kill streaks with your class's streak costs, the UAV
    (radar sweep and ping fade from IW4L's compass, duration from `_uav.gsc`), bots' UAVs. EMP and the nuke: done. Counter-UAV: done (``_uav.gsc``: as long as a UAV, jams the enemies' radar: your minimap dims and shows nothing but you; bots under it lose their UAV's knowledge).
    The other streaks: done (`game_core::killstreaks`, values from the killstreak scripts in `KSRL`): care package and
    emergency airdrop (marker, Little Bird / C-130, crate odds, capture), Predator (IW4L `remote_missile`), sentry (IW4L
    placement and turret aim), precision airstrike, Harrier, stealth bomber (target screen on the minimap), attack
    helicopter, Pave Low, Chopper Gunner, AC-130; bots call every one of them, Counter-UAVs, EMPs and nukes included.
    A trace against MW2 is still to do for their timings. Not yet: their world models (stand-in boxes), sounds and
    effects, the maps' helicopter paths (stand-in circles), kill cams, flares, shooting down sentries and the AC-130,
    EMP's effect on killstreaks (its HUD blackout: done), the Predator / AC-130 overlays, the minimap's fading edge and
    scan-line pass order (Cold-Blooded and Scrambler on the radar: done).
10. **Perks** (`game_core::perks`): a class's three perks and their Pro perks (perkTable's upgrade column;
    everything unlocked) become IW4's `ps.perks` bits that IW4L's movement and weapon code read (Marathon,
    Sleight of Hand, Steady Aim and their Pros, Commando's flag, Cold-Blooded's, Ninja Pro's, Last Stand's ...); move speed by the primary's class and
    Lightweight's scaler come from the user's `_weapons.gsc` / `_perkfunctions.gsc` (`RULE`); Stopping Power and Danger Close add `_perks.gsc`'s
    `perk_bulletDamage` / `perk_explosiveDamage` percent to bullet / explosive damage. Done (`docs/runtime.md`, Life and match; numbers from
    the user's scripts in `LIFE`): Cold-Blooded and Scrambler on the radar, Ninja Pro's steps, Hardline and its Pro,
    One Man Army (the match waits on its menu), Last Stand and Final Stand, Bomb Squad, Scrambler Pro's claymore delay,
    the deathstreaks, bots' class perks, the killcam and the final killcam (pose ring at most 436 KB of MEM2, with 7 bots).
    Scavenger bags: done (`game_core::item`). Not yet: a Beretta for a class without a pistol in your Last Stand, sounds in the killcam, a feel
    trace of all of these against MW2.

**Destructible objects**: the main body of each (states, damage rules, effects, repeating effects, health drain,
explosions with chain reactions, model swaps, collision boxes) is done (`game_core::destructible`, map pack `DST*`).
Not yet: separate parts (tires, glass, doors), physics pieces, their sounds, destroy and idle animations, glass
windows, grenades bouncing off them (light-grid lighting: done, the light lattice at each object's origin). Memory: about 1 MiB (Checkpoint) to 1.7 MiB (Invasion) of
MEM2 per map; measure the frame cost of drawing 200 objects on hardware (`wld` section).

## Audio, effects and lighting (this round)
Done: other players' gunfire (each weapon's world fire sound, one voice per shooter), MW2's hit alert, your knife's
melee cues, looping effect elements (the smoke grenade's cloud,
fires), tracers, bullet marks clipped to their surface, the light lattice for the viewmodel, characters and objects.
See `docs/runtime.md`. Memory (MEM2), mp_rust, measured against main's
packs: `common.wpk` +635 KB (the bots' 11 weapons' world sounds at 14 kHz with tails cut 593 KB, the impact marks'
decal textures at most 32 texels 26 KB, `WSNW` 9.5 KB, `FXWT` 2.4 KB, effect headers 2 KB, `FXTR` 1 KB), the map's
light lattice 26 KB (6,534 points), the mark pool 9 KB: about 0.67 MB in all, within this round's 0.7 MB. The match
log's `avfx` line at load reports the lattice, tracer and mark pool sizes; `avfx_counts` every 5 s what played.
Checked in Dolphin (`tools/autoplay/av_fx.txt`, `av_fx_wall.txt`): others' gunfire, tracers, marks on a wall, the
smoke's looping cloud, the knife's wall cue; not on a Wii, and the hit alert not in a scripted run (no scripted hit).

## Next, performance (ranked by expected milliseconds; measure with the logs first)
Compared with how shipped Wii games got steady frame rates (2026-11-16): the build already has convert-time display
lists, quantized indexed vertices, strips, cells and portals, short TEV chains, CMPR, baked lighting, early Z for
opaque surfaces and CPU/GPU overlap. Done from that comparison: a steady 30 fps (Options > FRAME RATE, as the Wii's
Call of Duty ports ran) and the GPU's own counters in the match log (`docs/testing.md`), so a hardware log says
whether a map is GPU-, fill- or CPU-bound. Next from it, in order: per-instance CPU work in world submission
(GX-indexed matrix loads, fewer array rebinds and vertex-cache flushes), characters on the GX matrix palette with
frustum / distance culling and a far LOD, paired-singles maths for what stays on the CPU (and `opt-level=3`), the
built-but-off pack toggles (`--small-dl`, `--tiled-uv`, `--small-props`, `--slim-views`, `--adpcm`) for MEM2 on the
big maps, then TMEM preloading, CI palettes for UI, a particle coverage cap.
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
   world batches presorted at load (a frame sorts only models and blended surfaces). World submission cut
   (2026-11-16, `docs/perf.md`): GX state sent only where it changed, one vertex-cache flush a frame instead of one
   per array rebind, a 1 MiB GX FIFO (`mp_terminal` wrote ~415 KB a frame into 256 KB, stopping the CPU at the high
   watermark ~100 times a second), culling over compact records and cell / grid groups, a radix sort, one matrix
   product per instance: Dolphin `wld` -21% on `mp_terminal` and `mp_rust`, same pictures. Not yet: shared
   quantisation origins (most draws still load a matrix: each compact batch has its own origin), GX-indexed matrix
   loads, paired singles (Rust's PowerPC inline assembly is unstable; it would go in the C shim).
4. Viewmodel and characters: skinned straight into their MEM2 buffers with a multiply for the weights (no copy, no
   divide). The viewmodel's rigid vertices (one bone at full weight) are placed by GX with their bone's matrix (a
   matrix index per vertex, at most ten per draw); only blended vertices are skinned on the CPU. Not yet: the same
   for characters, strips for the viewmodel's triangles.
5. ~~Weapon-file hitch at spawn~~ done differently: a whole class set is ~4-8 MB (files average 2.1 MB), too much to
   keep every class's loaded, so the next life's files stream in during the death screen (192 KB a frame, from 1 s
   after death); what's left at the respawn loads then.
6. Finer telemetry: the bots' CPU split into thinking, moving and shooting (`bai` / `bmv` / `bgun`, inside `bot`);
   the world draw's stages (`wvis` / `wcul` / `wsrt` / `wsub`, inside `wld`); GX FIFO bytes a frame and high-watermark
   stops (`fifo` / `full`); GPU metrics. Not yet: trace counts, A* expansions. Bot trims wait for these numbers.

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

//! The killstreak rewards past the radar ones: MW2's flow written natively from the user's killstreak scripts' values
//! (ADR 0007; `wpk::streaks::StreakRules`, read by the converter's `streak_rules.rs`), on IW4L's native pieces where
//! it has them (the Predator's steering, sentry placement and turret aim). Every reward is owned by a fighter, so bots
//! call the same ones the player does.
//!
//! - [`airdrop`]: the care package and emergency airdrop: the marker throw, the Little Bird or C-130 run, the crates
//!   falling, their random reward or ammo, capture by holding use.
//! - [`predator`]: the Predator missile, steered from above (IW4L `remote_missile`).
//! - [`strikes`]: the precision airstrike, the Harrier strike's runs and the stealth bomber.
//! - [`bots`]: bots calling what they earn.
//! - [`air`]: the attack helicopter, Pave Low, the Harrier's hover, the Chopper Gunner and the AC-130.
//! - [`sentry`]: the sentry gun, carried, placed (IW4L sentry placement) and firing (IW4L turret aim).
//!
//! Weapons come from the common pack's weapon list by MW2's names ([`Weapons::from_records`]): their combat numbers
//! (turret damage, fire time) and their explosion numbers (radius, inner and outer damage).

use alloc::vec::Vec;

use wpk::streaks::StreakRules;

use crate::arena::Arena;

pub mod air;
pub mod airdrop;
pub mod bots;
pub mod placement;
pub mod predator;
pub mod sentry;
pub mod strikes;

/// A killstreak reward, in `wpk::streaks::NAMES` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streak {
    Uav,
    CounterUav,
    Airdrop,
    Sentry,
    Predator,
    Precision,
    Harrier,
    Helicopter,
    Mega,
    Stealth,
    PaveLow,
    ChopperGunner,
    Ac130,
    Emp,
    Nuke,
}

const ALL: [Streak; 15] = [
    Streak::Uav,
    Streak::CounterUav,
    Streak::Airdrop,
    Streak::Sentry,
    Streak::Predator,
    Streak::Precision,
    Streak::Harrier,
    Streak::Helicopter,
    Streak::Mega,
    Streak::Stealth,
    Streak::PaveLow,
    Streak::ChopperGunner,
    Streak::Ac130,
    Streak::Emp,
    Streak::Nuke,
];

impl Streak {
    /// The killstreak of MW2 script name `name` (`killstreakTable`).
    pub fn from_name(name: &str) -> Option<Streak> {
        wpk::streaks::index(name).map(|i| ALL[i])
    }

    pub fn name(self) -> &'static str {
        wpk::streaks::NAMES[self as usize]
    }

    /// The HUD's line when it is called in.
    pub fn called(self) -> &'static str {
        match self {
            Streak::Uav => "UAV ONLINE",
            Streak::CounterUav => "COUNTER-UAV ONLINE",
            Streak::Airdrop => "CARE PACKAGE INBOUND",
            Streak::Sentry => "SENTRY GUN READY: FIRE TO PLACE",
            Streak::Predator => "PREDATOR MISSILE: STEER, FIRE TO BOOST",
            Streak::Precision => "PRECISION AIRSTRIKE INBOUND",
            Streak::Harrier => "HARRIER STRIKE INBOUND",
            Streak::Helicopter => "ATTACK HELICOPTER INBOUND",
            Streak::Mega => "EMERGENCY AIRDROP INBOUND",
            Streak::Stealth => "STEALTH BOMBER INBOUND",
            Streak::PaveLow => "PAVE LOW INBOUND",
            Streak::ChopperGunner => "CHOPPER GUNNER",
            Streak::Ac130 => "AC-130 ABOVE",
            Streak::Emp => "EMP DEPLOYED",
            Streak::Nuke => "TACTICAL NUKE INCOMING",
        }
    }
}

/// A gun a killstreak fires (its weapon index and combat numbers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gunline {
    pub weapon: u32,
    pub facts: weapon_iw4::WeaponCombatFacts,
}

/// A killstreak explosion: its weapon index and the weapon's radius and damage (`EquipRecord`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blast {
    pub weapon: u32,
    pub radius: f32,
    pub inner: f32,
    pub outer: f32,
}

/// The killstreaks' weapons, found by MW2's names in the common pack (None when the pack lacks one).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weapons {
    /// `airdrop_marker_mp`, `airdrop_mega_marker_mp`: thrown (their `EquipRecord`s fly them).
    pub marker: Option<(u32, wpk::weapon::EquipRecord)>,
    pub mega_marker: Option<(u32, wpk::weapon::EquipRecord)>,
    /// `sentry_minigun_mp`.
    pub sentry: Option<Gunline>,
    /// `remotemissile_projectile_mp`.
    pub predator: Option<Blast>,
    /// `artillery_mp`, `stealth_bomb_mp`: the strikes' effects (their damage is the script's `losRadiusDamage`).
    pub artillery: Option<Blast>,
    pub stealth_bomb: Option<Blast>,
    /// Its launch speed (`projectileSpeed`; it then speeds up to the cruise speed).
    pub predator_speed: f32,
    /// The helicopters' guns: `cobra_20mm_mp`, `pavelow_minigun_mp`, `harrier_20mm_mp`, `cobra_player_minigun_mp`
    /// (with their explosions when they have one).
    pub heli_gun: Option<(Gunline, Option<Blast>)>,
    pub pavelow_gun: Option<(Gunline, Option<Blast>)>,
    pub harrier_gun: Option<(Gunline, Option<Blast>)>,
    pub gunner: Option<(Gunline, Option<Blast>)>,
    /// The AC-130's 105, 40 and 25 mm, and their projectile speeds.
    pub ac130: [Option<Blast>; 3],
    pub ac130_speed: [f32; 3],
}

impl Weapons {
    /// Look the killstreak weapons up in the common pack's `WEAP` records and `EQIP` records (same order).
    pub fn from_records(weapons: &[u8], equips: &[u8]) -> Weapons {
        let mut w = Weapons::default();
        for (i, rec) in wpk::records::iter::<wpk::weapon::WeaponRecord>(weapons).enumerate() {
            let equip = || wpk::records::get::<wpk::weapon::EquipRecord>(equips, i);
            let gun = || {
                weapon_iw4::WeaponCombatFacts::try_from_captured(rec.combat)
                    .ok()
                    .map(|facts| Gunline {
                        weapon: i as u32,
                        facts,
                    })
            };
            let blast = || {
                equip().map(|e| Blast {
                    weapon: i as u32,
                    radius: e.explosion_radius as f32,
                    inner: e.explosion_inner_damage as f32,
                    outer: e.explosion_outer_damage as f32,
                })
            };
            match rec.name() {
                "airdrop_marker_mp" => w.marker = equip().map(|e| (i as u32, e)),
                "airdrop_mega_marker_mp" => w.mega_marker = equip().map(|e| (i as u32, e)),
                "sentry_minigun_mp" => w.sentry = gun(),
                "artillery_mp" => w.artillery = blast(),
                "cobra_20mm_mp" => w.heli_gun = gun().map(|g| (g, blast())),
                "pavelow_minigun_mp" => w.pavelow_gun = gun().map(|g| (g, blast())),
                "harrier_20mm_mp" => w.harrier_gun = gun().map(|g| (g, blast())),
                "cobra_player_minigun_mp" => w.gunner = gun().map(|g| (g, blast())),
                n @ ("ac130_105mm_mp" | "ac130_40mm_mp" | "ac130_25mm_mp") => {
                    let k = match n {
                        "ac130_105mm_mp" => 0,
                        "ac130_40mm_mp" => 1,
                        _ => 2,
                    };
                    w.ac130[k] = blast();
                    w.ac130_speed[k] = equip().map_or(0.0, |e| e.projectile_speed as f32);
                }
                "stealth_bomb_mp" => w.stealth_bomb = blast(),
                "remotemissile_projectile_mp" => {
                    w.predator = blast();
                    w.predator_speed = equip().map_or(0.0, |e| e.projectile_speed as f32);
                }
                _ => {}
            }
        }
        w
    }
}

/// Something a killstreak did that the game shell shows or plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Note {
    /// Fighter `fighter` got `streak` (from a crate).
    Reward { fighter: usize, streak: Streak },
    /// Fighter `fighter` took an ammo crate (refill every weapon).
    Ammo { fighter: usize },
    /// Fighter `fighter` called in `streak`.
    Called { fighter: usize, streak: Streak },
    /// A crate came down at `origin`.
    CrateLanded { origin: [f32; 3] },
}

/// The map as the killstreaks need it: the middle of the play area (the minimap's), how far it spreads, and its floor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MapInfo {
    pub center: [f32; 3],
    pub radius: f32,
}

/// Every killstreak in play, and the rewards bots hold.
#[derive(Default)]
pub struct Streaks {
    pub rules: StreakRules,
    pub weapons: Weapons,
    pub map: MapInfo,
    pub drops: Vec<airdrop::Drop>,
    pub crates: Vec<airdrop::Crate>,
    /// Markers in flight: their weapon, thrower and whether it is the emergency airdrop's.
    pub markers: Vec<(u32, usize, bool)>,
    pub missiles: Vec<predator::Missile>,
    pub sentries: Vec<sentry::Sentry>,
    pub planes: Vec<strikes::Plane>,
    /// Second runs waiting for their time.
    pub pending_planes: Vec<(u32, strikes::Plane)>,
    pub bombs: Vec<strikes::Bomb>,
    pub shells: Vec<strikes::Shell>,
    pub helis: Vec<air::Heli>,
    pub gunship: Option<air::Gunship>,
    pub rounds: Vec<air::Round>,
    /// Harriers due after their runs: when, owner, the target and heading.
    pub pending_harriers: Vec<(u32, usize, [f32; 3], f32)>,
    /// Per fighter: the crate being captured and since when.
    pub capture: Vec<Option<(u32, u32)>>,
    /// Per fighter: earned and not yet used (bots; the player's list is the game shell's).
    pub rewards: Vec<Vec<Streak>>,
    /// Per bot: when its oldest reward gets called.
    pub bot_next_ms: Vec<Option<u32>>,
    /// Per fighter: the three killstreaks and their kill costs (bots).
    pub sets: Vec<[Option<(Streak, u32)>; 3]>,
    /// Since the last [`Streaks::take_notes`].
    pub notes: Vec<Note>,
    next_id: u32,
    rng: u32,
}

impl Streaks {
    pub fn new(rules: StreakRules, weapons: Weapons, map: MapInfo) -> Streaks {
        Streaks {
            rules,
            weapons,
            map,
            rng: 0x6d2b_79f5,
            ..Streaks::default()
        }
    }

    /// A random number in 0..1 (xorshift).
    pub fn random(&mut self) -> f32 {
        let mut x = self.rng | 1;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// What happened since the last call.
    pub fn take_notes(&mut self) -> Vec<Note> {
        core::mem::take(&mut self.notes)
    }

    pub(crate) fn fit(&mut self, fighters: usize) {
        if self.capture.len() < fighters {
            self.capture.resize(fighters, None);
            self.rewards.resize_with(fighters, Vec::new);
            self.sets.resize(fighters, [None; 3]);
        }
    }

    /// Fighter `who` made its `streak`-th kill in a row: the rewards of its set that cost exactly that.
    pub fn on_kill(&mut self, who: usize, streak: u32) {
        self.fit(who + 1);
        for (s, cost) in self.sets[who].into_iter().flatten() {
            if cost == streak {
                self.rewards[who].push(s);
            }
        }
    }
}

/// Distance squared.
pub(crate) fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum()
}

/// The unit vector along yaw `deg` (IW4: yaw 0 is +x, 90 is +y).
pub fn yaw_dir(deg: f32) -> [f32; 3] {
    let r = deg.to_radians();
    [libm::cosf(r), libm::sinf(r), 0.0]
}

impl Arena {
    /// Run every killstreak for `dt` seconds.
    pub fn update_killstreaks(
        &mut self,
        clip: &crate::collision::MapClip<'_>,
        dt: f32,
        now: u32,
        out: &mut Vec<crate::arena::Event>,
    ) {
        self.bots_use_killstreaks(clip, now);
        let mut s = core::mem::take(&mut self.streaks);
        s.fit(self.fighters.len());
        airdrop::update(self, &mut s, clip, dt, now, out);
        predator::update(self, &mut s, clip, dt, now, out);
        sentry::update(self, &mut s, clip, dt, now, out);
        strikes::update(self, &mut s, clip, dt, now, out);
        air::update(self, &mut s, clip, dt, now, out);
        // Kills made meanwhile (a crate falling on someone) earned rewards into the stand-in.
        let meanwhile = core::mem::replace(&mut self.streaks, s);
        for (i, r) in meanwhile.rewards.into_iter().enumerate() {
            self.streaks.fit(i + 1);
            self.streaks.rewards[i].extend(r);
        }
        self.streaks.notes.extend(meanwhile.notes);
    }
}

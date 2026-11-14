//! The weapons a player spawns with: the class's primary and secondary as IW4L guns, and their first-person files
//! (`views/<weapon>.wpk`: model, textures, sounds) loaded into MEM2.
//!
//! The files load above a MEM2 and texture mark taken after the map, so a class with other weapons releases them
//! and loads its own (the map's data below the marks stays).

use game_core::gun::{Gun, SwitchTarget};
use wpk::classes::{Catalog, Loadout, NONE};

use crate::view::View;
use crate::{AppPath, ffi, load_pack};

/// The weapon spawned with when there is no class data (MW2's first default class carries it).
const FALLBACK_WEAPON: &str = "m4";

/// The weapon sections of `common.wpk` (records parallel to `WEAP`).
#[derive(Clone, Copy)]
pub struct WeaponTables<'a> {
    pub weapons: &'a [u8],
    pub kicks: &'a [u8],
    pub moves: &'a [u8],
    pub aims: &'a [u8],
    /// Thrown weapons' numbers (`WEQP`; empty with an older pack).
    pub equips: &'a [u8],
}

/// The weapon fallback classes throw (MW2's frag).
const FALLBACK_LETHAL: &str = "frag_grenade_mp";
/// IW4 offhand class of frag grenades (the bots' lethal).
pub const OFFHAND_CLASS_FRAG: i32 = 1;
/// The grenades this build throws: frag and semtex (lethal), flashbang and stun (tactical). Throwing knives,
/// claymores, C4, tactical insertions, blast shields and smoke are not in yet.
const THROWN: [&str; 4] = [
    "frag_grenade_mp",
    "semtex_mp",
    "flash_grenade_mp",
    "concussion_grenade_mp",
];

/// The offhand row of grenade weapon `name` (with or without `_mp`), when it is one this build throws.
pub fn offhand_row(t: &WeaponTables<'_>, name: &str) -> Option<weapon_iw4::OffhandInvRow> {
    let full = if name.ends_with("_mp") {
        alloc::string::String::from(name)
    } else {
        alloc::format!("{name}_mp")
    };
    if !THROWN.contains(&full.as_str()) {
        return None;
    }
    let (index, rec) = wpk::weapon::find_indexed(t.weapons, &full)?;
    let e = wpk::records::get::<wpk::weapon::EquipRecord>(t.equips, index as usize)?;
    let f = weapon_iw4::WeaponCombatFacts::try_from_captured(rec.combat).ok()?;
    Some(weapon_iw4::OffhandInvRow {
        weapon: index,
        offhand_class: e.offhand_class,
        ammo: e.start_ammo.max(1),
        hold_fire_time_ms: e.hold_fire_time_ms,
        fire_time_ms: f.fire_time_ms,
        fire_delay_ms: f.fire_delay_ms,
        fuse_time_ms: e.fuse_time_ms,
        cook_off_hold: e.cook_off_hold,
        weap_type: f.weap_type,
        ..weapon_iw4::OffhandInvRow::default()
    })
}

pub struct Arsenal {
    pub guns: [Option<Gun>; 2],
    pub views: [Option<View>; 2],
    /// 0 primary, 1 secondary.
    pub active: usize,
    /// The weapon asked for (IW4 `usercmd.weapon`) while the active one drops.
    pending: Option<usize>,
    /// Which weapons were out this life (the first raise is the long one).
    used: [bool; 2],
    /// Base weapon names whose files are loaded.
    loaded: [heapless_name::Name; 2],
    mem2_mark: u32,
    tex_mark: i32,
}

/// Short fixed-size names (no allocation in the comparison).
mod heapless_name {
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    pub struct Name {
        len: u8,
        b: [u8; 31],
    }

    impl Name {
        pub fn new(s: &str) -> Name {
            let mut n = Name::default();
            let len = s.len().min(31);
            n.b[..len].copy_from_slice(&s.as_bytes()[..len]);
            n.len = len as u8;
            n
        }
        pub fn as_str(&self) -> &str {
            core::str::from_utf8(&self.b[..usize::from(self.len)]).unwrap_or("")
        }
    }
}

use heapless_name::Name;

impl Arsenal {
    /// Call after the map is loaded: the weapon files go above what is loaded now.
    pub fn new() -> Arsenal {
        Arsenal {
            guns: [None, None],
            views: [None, None],
            active: 0,
            pending: None,
            used: [true, false],
            loaded: [Name::default(); 2],
            mem2_mark: unsafe { ffi::mwf_mem2_mark() },
            tex_mark: unsafe { ffi::mwf_tex_mark() },
        }
    }

    /// Arm for a spawn with `class` (or the fallback weapon): fresh guns with full ammunition, raised; first-person
    /// files reloaded only when the weapons changed.
    pub fn equip(
        &mut self,
        gfx: &mut impl game_core::gfx::Gfx,
        t: &WeaponTables<'_>,
        cat: Option<&Catalog>,
        class: Option<&Loadout>,
    ) {
        // (base name, weapon file candidates) per slot.
        let mut slots: [(Name, alloc::vec::Vec<alloc::string::String>); 2] = Default::default();
        match (cat, class) {
            (Some(c), Some(l)) => {
                for (i, (w, atts)) in [
                    (l.primary, l.primary_attachments),
                    (l.secondary, l.secondary_attachments),
                ]
                .into_iter()
                .enumerate()
                {
                    if w == NONE {
                        continue;
                    }
                    let Some(item) = c.weapons.get(usize::from(w)) else {
                        continue;
                    };
                    let mut names = c.weapon_names(w, atts);
                    // A combination without its own file falls back to the bare weapon.
                    names.push(alloc::format!("{}_mp", item.name));
                    slots[i] = (Name::new(&item.name), names);
                }
            }
            _ => {
                slots[0] = (
                    Name::new(FALLBACK_WEAPON),
                    alloc::vec![alloc::format!("{FALLBACK_WEAPON}_mp")],
                );
            }
        }
        for (i, (_, names)) in slots.iter().enumerate() {
            self.guns[i] = names.iter().find_map(|n| {
                let (index, rec) = wpk::weapon::find_indexed(t.weapons, n)?;
                let mut g = Gun::with_index(&rec, index)?;
                g.set_kick(wpk::records::get::<wpk::weapon::KickRecord>(
                    t.kicks,
                    index as usize,
                ));
                g.set_moves(wpk::records::get(t.moves, index as usize));
                g.set_aim(wpk::records::get(t.aims, index as usize));
                Some(g)
            });
        }
        // Grenades: the class's equipment (lethal) and special grenade (tactical), full.
        let lethal_name = match (cat, class) {
            (Some(c), Some(l)) => c
                .perks
                .get(usize::from(l.equipment))
                .map(|i| i.name.clone()),
            _ => Some(alloc::string::String::from(FALLBACK_LETHAL)),
        };
        let tactical_name = match (cat, class) {
            (Some(c), Some(l)) => c
                .offhands
                .get(usize::from(l.offhand))
                .map(|i| i.name.clone()),
            _ => None,
        };
        let lethal = lethal_name.as_deref().and_then(|n| offhand_row(t, n));
        let tactical = tactical_name.as_deref().and_then(|n| offhand_row(t, n));
        let rows: alloc::vec::Vec<weapon_iw4::OffhandInvRow> =
            [lethal, tactical].into_iter().flatten().collect();
        // The lethal button throws the equipment's class, the tactical button the special grenade's.
        let (lc, tc) = (
            lethal.map_or(OFFHAND_CLASS_FRAG, |r| r.offhand_class),
            tactical.map_or(0, |r| r.offhand_class),
        );
        for g in self.guns.iter_mut().flatten() {
            g.set_offhands(&rows, lc, tc);
        }
        self.active = if self.guns[0].is_some() { 0 } else { 1 };
        self.pending = None;
        self.used = [self.active == 0, self.active == 1];
        let wanted = [slots[0].0, slots[1].0];
        if wanted != self.loaded {
            // Drop the old views before their MEM2 goes.
            self.views = [None, None];
            unsafe {
                ffi::mwf_mem2_release(self.mem2_mark);
                ffi::mwf_tex_release(self.tex_mark);
            }
            for (i, name) in wanted.iter().enumerate() {
                if name.as_str().is_empty() {
                    continue;
                }
                let file = AppPath::new(&alloc::format!("views/{}.wpk", name.as_str()));
                self.views[i] = file
                    .and_then(|p| load_pack(&p))
                    .and_then(|bytes| View::load(bytes, gfx));
            }
            self.loaded = wanted;
        }
    }

    /// Ask for the other weapon (IW4L's change: this one drops, then that one raises; asking again while it drops
    /// takes the request back). False when there is no other weapon.
    pub fn switch(&mut self) -> bool {
        let other = 1 - self.active;
        if self.guns[other].is_none() {
            return false;
        }
        self.pending = match self.pending {
            Some(_) => None,
            None => Some(other),
        };
        true
    }

    /// What the active gun's next step is asked to switch to.
    pub fn switch_target(&self) -> Option<SwitchTarget> {
        let o = self.pending?;
        self.guns[o].as_ref().map(|g| g.as_target(!self.used[o]))
    }

    /// After the active gun's step: when its drop finished, the other weapon takes the hand and raises.
    pub fn finish_switch(&mut self) {
        let a = self.active;
        let Some(hand) = self.guns[a].as_mut().and_then(Gun::take_switch) else {
            return;
        };
        let other = 1 - a;
        // The grenades go with the player to the other gun.
        let (lo, hi) = self.guns.split_at_mut(1);
        let (from, to) = if a == 0 {
            (&lo[0], &mut hi[0])
        } else {
            (&hi[0], &mut lo[0])
        };
        if let (Some(f), Some(t)) = (from.as_ref(), to.as_mut()) {
            t.take_offhands(f);
        }
        if let Some(g) = self.guns[other].as_mut() {
            g.adopt(&hand);
            self.active = other;
            self.used[other] = true;
            if let Some(v) = self.views[other].as_mut() {
                v.play_event_alias(wpk::sound::event::RAISE);
            }
        }
        self.pending = None;
    }

    pub fn gun(&self) -> Option<&Gun> {
        self.guns[self.active].as_ref()
    }
}

impl Default for Arsenal {
    fn default() -> Self {
        Arsenal::new()
    }
}

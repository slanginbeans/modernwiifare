//! The weapons a player spawns with: the class's primary and secondary as IW4L guns, and their first-person files
//! (`views/<weapon>.wpk`: model, textures, sounds) loaded into MEM2.
//!
//! The files load above a MEM2 and texture mark taken after the map, so a class with other weapons releases them
//! and loads its own (the map's data below the marks stays).

use game_core::gun::Gun;
use wpk::classes::{Catalog, Loadout, NONE};

use crate::view::View;
use crate::{AppPath, ffi, load_pack};

/// The weapon spawned with when there is no class data (MW2's first default class carries it).
const FALLBACK_WEAPON: &str = "m4";

pub struct Arsenal {
    pub guns: [Option<Gun>; 2],
    pub views: [Option<View>; 2],
    /// 0 primary, 1 secondary.
    pub active: usize,
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
        weapons: &[u8],
        kicks: &[u8],
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
                let (index, rec) = wpk::weapon::find_indexed(weapons, n)?;
                let mut g = Gun::with_index(&rec, index)?;
                g.set_kick(wpk::records::get::<wpk::weapon::KickRecord>(
                    kicks,
                    index as usize,
                ));
                Some(g)
            });
        }
        self.active = if self.guns[0].is_some() { 0 } else { 1 };
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

    /// Switch to the other weapon (it raises). False when there is none.
    pub fn switch(&mut self) -> bool {
        let other = 1 - self.active;
        let Some(g) = self.guns[other].as_mut() else {
            return false;
        };
        g.raise();
        self.active = other;
        if let Some(v) = self.views[other].as_mut() {
            v.play_event_alias(wpk::sound::event::RAISE);
        }
        true
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

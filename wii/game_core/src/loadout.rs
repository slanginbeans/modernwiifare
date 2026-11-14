//! Create-a-Class rules over the converted catalog (`wpk::classes`): the fields of a class, what each may hold, and
//! the fix-ups MW2's menus make when one changes. Everything is unlocked.
//!
//! - Primary: assault rifles, SMGs, LMGs, sniper rifles, riot shield. Secondary: machine pistols, shotguns,
//!   handguns, launchers.
//! - Attachments: the weapon's own list (statsTable), never two that `attachmentcombos` forbids. A second attachment
//!   needs Bling (`specialty_bling`) in perk 1; with everything unlocked Bling is taken as Bling Pro, so it opens the
//!   secondary's second attachment too.
//! - Perks by slot; equipment and deathstreak from their own slots; three different killstreaks.

use alloc::string::String;
use alloc::vec::Vec;

use wpk::classes::{Catalog, Loadout, NONE, group, slot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Primary,
    PrimaryAttachment(usize),
    Secondary,
    SecondaryAttachment(usize),
    Equipment,
    Offhand,
    Perk(usize),
    Deathstreak,
    Killstreak(usize),
}

/// The fields in menu order.
pub const FIELDS: [Field; 15] = [
    Field::Primary,
    Field::PrimaryAttachment(0),
    Field::PrimaryAttachment(1),
    Field::Secondary,
    Field::SecondaryAttachment(0),
    Field::SecondaryAttachment(1),
    Field::Equipment,
    Field::Offhand,
    Field::Perk(0),
    Field::Perk(1),
    Field::Perk(2),
    Field::Deathstreak,
    Field::Killstreak(0),
    Field::Killstreak(1),
    Field::Killstreak(2),
];

/// Our menu labels (not MW2's text).
pub fn label(f: Field) -> &'static str {
    match f {
        Field::Primary => "PRIMARY",
        Field::PrimaryAttachment(0) => "  ATTACHMENT",
        Field::PrimaryAttachment(_) => "  ATTACHMENT 2",
        Field::Secondary => "SECONDARY",
        Field::SecondaryAttachment(0) => "  ATTACHMENT",
        Field::SecondaryAttachment(_) => "  ATTACHMENT 2",
        Field::Equipment => "EQUIPMENT",
        Field::Offhand => "SPECIAL GRENADE",
        Field::Perk(0) => "PERK 1",
        Field::Perk(1) => "PERK 2",
        Field::Perk(_) => "PERK 3",
        Field::Deathstreak => "DEATHSTREAK",
        Field::Killstreak(0) => "KILLSTREAK 1",
        Field::Killstreak(1) => "KILLSTREAK 2",
        Field::Killstreak(_) => "KILLSTREAK 3",
    }
}

fn get(l: &Loadout, f: Field) -> u8 {
    match f {
        Field::Primary => l.primary,
        Field::PrimaryAttachment(i) => l.primary_attachments[i.min(1)],
        Field::Secondary => l.secondary,
        Field::SecondaryAttachment(i) => l.secondary_attachments[i.min(1)],
        Field::Equipment => l.equipment,
        Field::Offhand => l.offhand,
        Field::Perk(i) => l.perks[i.min(2)],
        Field::Deathstreak => l.deathstreak,
        Field::Killstreak(i) => l.killstreaks[i.min(2)],
    }
}

fn slot_of(l: &mut Loadout, f: Field) -> &mut u8 {
    match f {
        Field::Primary => &mut l.primary,
        Field::PrimaryAttachment(i) => &mut l.primary_attachments[i.min(1)],
        Field::Secondary => &mut l.secondary,
        Field::SecondaryAttachment(i) => &mut l.secondary_attachments[i.min(1)],
        Field::Equipment => &mut l.equipment,
        Field::Offhand => &mut l.offhand,
        Field::Perk(i) => &mut l.perks[i.min(2)],
        Field::Deathstreak => &mut l.deathstreak,
        Field::Killstreak(i) => &mut l.killstreaks[i.min(2)],
    }
}

/// Whether perk 1 is Bling.
pub fn has_bling(c: &Catalog, l: &Loadout) -> bool {
    c.perks
        .get(usize::from(l.perks[0]))
        .is_some_and(|p| p.name == "specialty_bling")
}

/// Whether a field can be changed now (second attachments need Bling and a first attachment).
pub fn enabled(c: &Catalog, l: &Loadout, f: Field) -> bool {
    let weapon_takes = |w: u8| {
        c.weapons
            .get(usize::from(w))
            .is_some_and(|w| w.attachments != 0)
    };
    match f {
        Field::PrimaryAttachment(0) => weapon_takes(l.primary),
        Field::SecondaryAttachment(0) => weapon_takes(l.secondary),
        Field::PrimaryAttachment(_) => {
            weapon_takes(l.primary) && has_bling(c, l) && l.primary_attachments[0] != 0
        }
        Field::SecondaryAttachment(_) => {
            weapon_takes(l.secondary) && has_bling(c, l) && l.secondary_attachments[0] != 0
        }
        _ => true,
    }
}

/// The display text of a field's value.
pub fn value(c: &Catalog, l: &Loadout, f: Field) -> String {
    let v = get(l, f);
    let name = |list: &[wpk::classes::Item]| {
        list.get(usize::from(v))
            .map_or_else(|| String::from("NONE"), |i| i.display.clone())
    };
    match f {
        Field::Primary | Field::Secondary => name(&c.weapons),
        Field::PrimaryAttachment(_) | Field::SecondaryAttachment(_) => name(&c.attachments),
        Field::Offhand => name(&c.offhands),
        Field::Killstreak(_) => name(&c.killstreaks),
        _ => name(&c.perks),
    }
}

/// Every value a field may take, in catalog order (attachment 0 = none first).
pub fn choices(c: &Catalog, l: &Loadout, f: Field) -> Vec<u8> {
    let idx = |n: usize| (0..n).map(|i| i as u8);
    match f {
        Field::Primary | Field::Secondary => {
            let primary = f == Field::Primary;
            idx(c.weapons.len())
                .filter(|&i| group::is_primary(c.weapons[usize::from(i)].kind) == primary)
                .collect()
        }
        Field::PrimaryAttachment(k) | Field::SecondaryAttachment(k) => {
            let (w, atts) = match f {
                Field::PrimaryAttachment(_) => (l.primary, l.primary_attachments),
                _ => (l.secondary, l.secondary_attachments),
            };
            let mask = c.weapons.get(usize::from(w)).map_or(0, |w| w.attachments);
            let other = atts[1 - k.min(1)];
            idx(c.attachments.len())
                .filter(|&a| {
                    a == 0
                        || (u32::from(a) < 32
                            && mask & (1 << a) != 0
                            && (k == 0 || c.compatible(a, other)))
                            && (k == 1 || other == 0 || c.compatible(a, other))
                })
                .collect()
        }
        Field::Equipment | Field::Perk(_) | Field::Deathstreak => {
            let want = match f {
                Field::Equipment => slot::EQUIPMENT,
                Field::Deathstreak => slot::DEATHSTREAK,
                Field::Perk(0) => slot::PERK1,
                Field::Perk(1) => slot::PERK2,
                _ => slot::PERK3,
            };
            idx(c.perks.len())
                .filter(|&i| c.perks[usize::from(i)].kind == want)
                .collect()
        }
        Field::Offhand => idx(c.offhands.len()).collect(),
        Field::Killstreak(k) => idx(c.killstreaks.len())
            .filter(|&i| !(0..3).any(|j| j != k && l.killstreaks[j] == i))
            .collect(),
    }
}

/// Set a field, then fix what the change made invalid (attachments the new weapon can't take or that clash, second
/// attachments without Bling).
pub fn set(c: &Catalog, l: &mut Loadout, f: Field, v: u8) {
    *slot_of(l, f) = v;
    fix(c, l);
}

/// Make a loadout valid: unknown items cleared, attachments within each weapon's list, compatible and in order.
pub fn fix(c: &Catalog, l: &mut Loadout) {
    let bling = has_bling(c, l);
    for (w, atts) in [
        (l.primary, &mut l.primary_attachments),
        (l.secondary, &mut l.secondary_attachments),
    ] {
        let mask = c.weapons.get(usize::from(w)).map_or(0, |w| w.attachments);
        for a in atts.iter_mut() {
            if *a == NONE || (*a != 0 && (u32::from(*a) >= 32 || mask & (1 << *a) == 0)) {
                *a = 0;
            }
        }
        if atts[0] == 0 || !bling || !c.compatible(atts[0], atts[1]) {
            atts[1] = 0;
        }
    }
}

/// The class panel's slots, in the order of MW2's `changeclass` / `menu_cac_*` menus: class name, primary (with its
/// attachments), secondary, perks 1 to 3, deathstreak, equipment, special grenade.
pub fn panel(cat: &Catalog, c: &Loadout, out: &mut Vec<String>) {
    use Field as F;
    let gun = |f: F, atts: [u8; 2], att: fn(usize) -> F| {
        let mut s = value(cat, c, f);
        for (k, &a) in atts.iter().enumerate() {
            if a != 0 {
                s.push_str(" + ");
                s.push_str(&value(cat, c, att(k)));
            }
        }
        s
    };
    out.push(c.name.clone());
    out.push(gun(F::Primary, c.primary_attachments, F::PrimaryAttachment));
    out.push(gun(
        F::Secondary,
        c.secondary_attachments,
        F::SecondaryAttachment,
    ));
    for k in 0..3 {
        out.push(value(cat, c, F::Perk(k)));
    }
    out.push(value(cat, c, F::Deathstreak));
    out.push(value(cat, c, F::Equipment));
    out.push(value(cat, c, F::Offhand));
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use wpk::classes::Item;

    fn it(name: &str, kind: u8, attachments: u32) -> Item {
        Item {
            name: name.to_string(),
            display: name.to_uppercase(),
            kind,
            attachments,
            upgrade: String::new(),
        }
    }

    /// A made-up catalog: two primaries, a pistol, attachments none / sight / dot / ammo (sight and dot clash).
    fn cat() -> Catalog {
        Catalog {
            weapons: vec![
                it("rifle", group::ASSAULT, 0b1110),
                it("shield", group::RIOT, 0),
                it("pistol", group::PISTOL, 0b1000),
            ],
            attachments: vec![
                it("none", 0, 0),
                it("sight", 0, 0),
                it("dot", 0, 0),
                it("ammo", 0, 0),
            ],
            incompatible: vec![0, 0b100, 0b10, 0],
            perks: vec![
                it("specialty_bling", slot::PERK1, 0),
                it("specialty_fast", slot::PERK1, 0),
                it("specialty_strong", slot::PERK2, 0),
                it("frag", slot::EQUIPMENT, 0),
            ],
            offhands: vec![it("flash", 0, 0)],
            killstreaks: vec![it("a", 0, 0), it("b", 0, 0), it("c", 0, 0), it("d", 0, 0)],
            defaults: vec![],
            customs: vec![],
        }
    }

    #[test]
    fn weapons_split_into_primary_and_secondary() {
        let c = cat();
        let l = Loadout::default();
        assert_eq!(choices(&c, &l, Field::Primary), [0, 1]);
        assert_eq!(choices(&c, &l, Field::Secondary), [2]);
        assert_eq!(choices(&c, &l, Field::Perk(0)), [0, 1]);
        assert_eq!(choices(&c, &l, Field::Equipment), [3]);
    }

    #[test]
    fn attachments_follow_the_weapon_combos_and_bling() {
        let c = cat();
        let mut l = Loadout::default();
        set(&c, &mut l, Field::Primary, 0);
        assert_eq!(choices(&c, &l, Field::PrimaryAttachment(0)), [0, 1, 2, 3]);
        set(&c, &mut l, Field::PrimaryAttachment(0), 1);
        // No Bling: no second attachment, and setting one is undone.
        assert!(!enabled(&c, &l, Field::PrimaryAttachment(1)));
        set(&c, &mut l, Field::PrimaryAttachment(1), 3);
        assert_eq!(l.primary_attachments, [1, 0]);
        set(&c, &mut l, Field::Perk(0), 0);
        assert!(enabled(&c, &l, Field::PrimaryAttachment(1)));
        // Sight and dot clash.
        assert_eq!(choices(&c, &l, Field::PrimaryAttachment(1)), [0, 3]);
        set(&c, &mut l, Field::PrimaryAttachment(1), 3);
        assert_eq!(l.primary_attachments, [1, 3]);
        // Dropping Bling drops the second attachment; a shield takes none.
        set(&c, &mut l, Field::Perk(0), 1);
        assert_eq!(l.primary_attachments, [1, 0]);
        set(&c, &mut l, Field::Primary, 1);
        assert_eq!(l.primary_attachments, [0, 0]);
        assert!(!enabled(&c, &l, Field::PrimaryAttachment(0)));
    }

    #[test]
    fn killstreaks_are_distinct() {
        let c = cat();
        let l = Loadout {
            killstreaks: [0, 1, NONE],
            ..Loadout::default()
        };
        assert_eq!(choices(&c, &l, Field::Killstreak(2)), [2, 3]);
        assert_eq!(choices(&c, &l, Field::Killstreak(0)), [0, 2, 3]);
        assert_eq!(value(&c, &l, Field::Killstreak(2)), "NONE");
        assert_eq!(value(&c, &l, Field::Killstreak(1)), "B");
    }
}

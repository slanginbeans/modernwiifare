//! Create-a-Class data (`CLSS` in `common.wpk`): what a class can hold and MW2's default classes, built at convert
//! time from the user's own tables (stats, attachments, perks, killstreaks, classes) with localized names.
//!
//! Lists are indexed by `u8`; [`NONE`] means "nothing". A [`Loadout`] refers to items by index; saved classes on the
//! SD card use item *names* ([`Catalog::encode_loadouts`]) so they survive a re-conversion.

use alloc::string::String;
use alloc::vec::Vec;

use crate::be::{self, Push};

pub const NONE: u8 = 0xFF;

/// Weapon groups (MW2 statsTable `weapon_*` groups). Primary: assault rifle, SMG, LMG, sniper rifle, riot shield;
/// secondary: machine pistol, shotgun, handgun, launcher.
pub mod group {
    pub const ASSAULT: u8 = 0;
    pub const SMG: u8 = 1;
    pub const LMG: u8 = 2;
    pub const SNIPER: u8 = 3;
    pub const RIOT: u8 = 4;
    pub const MACHINE_PISTOL: u8 = 5;
    pub const SHOTGUN: u8 = 6;
    pub const PISTOL: u8 = 7;
    pub const LAUNCHER: u8 = 8;
    pub const COUNT: u8 = 9;

    pub fn is_primary(g: u8) -> bool {
        g <= RIOT
    }

    /// The statsTable group name of `g`.
    pub fn table_name(g: u8) -> &'static str {
        [
            "weapon_assault",
            "weapon_smg",
            "weapon_lmg",
            "weapon_sniper",
            "weapon_riot",
            "weapon_machine_pistol",
            "weapon_shotgun",
            "weapon_pistol",
            "weapon_projectile",
        ]
        .get(usize::from(g))
        .copied()
        .unwrap_or("")
    }
}

/// Perk slots (perkTable `perk1`..`perk4`, `equipment`).
pub mod slot {
    pub const PERK1: u8 = 1;
    pub const PERK2: u8 = 2;
    pub const PERK3: u8 = 3;
    pub const DEATHSTREAK: u8 = 4;
    pub const EQUIPMENT: u8 = 5;
}

/// Something a class can hold: its script name (`m4`, `specialty_marathon`, `acog`), its display name, a kind
/// (weapon group or perk slot) and, for weapons, the attachments it takes (bit n = attachment n).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub display: String,
    pub kind: u8,
    pub attachments: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loadout {
    pub name: String,
    pub primary: u8,
    pub primary_attachments: [u8; 2],
    pub secondary: u8,
    pub secondary_attachments: [u8; 2],
    pub equipment: u8,
    pub perks: [u8; 3],
    pub offhand: u8,
    pub deathstreak: u8,
    pub killstreaks: [u8; 3],
}

impl Default for Loadout {
    fn default() -> Self {
        Loadout {
            name: String::new(),
            primary: NONE,
            primary_attachments: [NONE; 2],
            secondary: NONE,
            secondary_attachments: [NONE; 2],
            equipment: NONE,
            perks: [NONE; 3],
            offhand: NONE,
            deathstreak: NONE,
            killstreaks: [NONE; 3],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub weapons: Vec<Item>,
    /// Attachment 0 is "none".
    pub attachments: Vec<Item>,
    /// Per attachment: the attachments it can't be combined with (attachmentcombos `no`), bit per attachment.
    pub incompatible: Vec<u32>,
    pub perks: Vec<Item>,
    pub offhands: Vec<Item>,
    pub killstreaks: Vec<Item>,
    /// MW2's default classes (Grenadier ..).
    pub defaults: Vec<Loadout>,
    /// The custom classes a new profile starts with.
    pub customs: Vec<Loadout>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError;

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, DecodeError> {
        let v = *self.b.get(self.at).ok_or(DecodeError)?;
        self.at += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        let v = be::u16(self.b.get(..self.at + 2).ok_or(DecodeError)?, self.at);
        self.at += 2;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        let v = be::u32(self.b.get(..self.at + 4).ok_or(DecodeError)?, self.at);
        self.at += 4;
        Ok(v)
    }
    fn str(&mut self) -> Result<String, DecodeError> {
        let n = usize::from(self.u8()?);
        let s = self.b.get(self.at..self.at + n).ok_or(DecodeError)?;
        self.at += n;
        Ok(String::from_utf8_lossy(s).into_owned())
    }
}

fn put_str(o: &mut Vec<u8>, s: &str) {
    let mut n = s.len().min(255);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    o.push(n as u8);
    o.extend_from_slice(&s.as_bytes()[..n]);
}

fn put_items(o: &mut Vec<u8>, items: &[Item]) {
    o.u16(items.len() as u16);
    for i in items {
        put_str(o, &i.name);
        put_str(o, &i.display);
        o.push(i.kind);
        o.u32(i.attachments);
    }
}

fn get_items(r: &mut Reader<'_>) -> Result<Vec<Item>, DecodeError> {
    let n = r.u16()?;
    (0..n)
        .map(|_| {
            Ok(Item {
                name: r.str()?,
                display: r.str()?,
                kind: r.u8()?,
                attachments: r.u32()?,
            })
        })
        .collect()
}

fn loadout_bytes(l: &Loadout) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0] = l.primary;
    b[1..3].copy_from_slice(&l.primary_attachments);
    b[3] = l.secondary;
    b[4..6].copy_from_slice(&l.secondary_attachments);
    b[6] = l.equipment;
    b[7..10].copy_from_slice(&l.perks);
    b[10] = l.offhand;
    b[11] = l.deathstreak;
    b[12..15].copy_from_slice(&l.killstreaks);
    b
}

fn loadout_from(name: String, b: &[u8]) -> Loadout {
    Loadout {
        name,
        primary: b[0],
        primary_attachments: [b[1], b[2]],
        secondary: b[3],
        secondary_attachments: [b[4], b[5]],
        equipment: b[6],
        perks: [b[7], b[8], b[9]],
        offhand: b[10],
        deathstreak: b[11],
        killstreaks: [b[12], b[13], b[14]],
    }
}

fn put_loadouts(o: &mut Vec<u8>, ls: &[Loadout]) {
    o.u16(ls.len() as u16);
    for l in ls {
        put_str(o, &l.name);
        o.extend_from_slice(&loadout_bytes(l));
    }
}

fn get_loadouts(r: &mut Reader<'_>) -> Result<Vec<Loadout>, DecodeError> {
    let n = r.u16()?;
    (0..n)
        .map(|_| {
            let name = r.str()?;
            let b = r.b.get(r.at..r.at + 16).ok_or(DecodeError)?;
            r.at += 16;
            Ok(loadout_from(name, b))
        })
        .collect()
}

/// Version of the saved-classes file.
const SAVE_MAGIC: &[u8; 4] = b"MWCL";

impl Catalog {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        put_items(&mut o, &self.weapons);
        put_items(&mut o, &self.attachments);
        o.u16(self.incompatible.len() as u16);
        for m in &self.incompatible {
            o.u32(*m);
        }
        put_items(&mut o, &self.perks);
        put_items(&mut o, &self.offhands);
        put_items(&mut o, &self.killstreaks);
        put_loadouts(&mut o, &self.defaults);
        put_loadouts(&mut o, &self.customs);
        o
    }

    pub fn decode(b: &[u8]) -> Result<Catalog, DecodeError> {
        let mut r = Reader { b, at: 0 };
        let weapons = get_items(&mut r)?;
        let attachments = get_items(&mut r)?;
        let n = r.u16()?;
        let incompatible = (0..n).map(|_| r.u32()).collect::<Result<_, _>>()?;
        Ok(Catalog {
            weapons,
            attachments,
            incompatible,
            perks: get_items(&mut r)?,
            offhands: get_items(&mut r)?,
            killstreaks: get_items(&mut r)?,
            defaults: get_loadouts(&mut r)?,
            customs: get_loadouts(&mut r)?,
        })
    }

    /// Whether attachments `a` and `b` (indices, 0 = none) can go on one weapon.
    pub fn compatible(&self, a: u8, b: u8) -> bool {
        if a == 0 || b == 0 || a == NONE || b == NONE {
            return true;
        }
        if a == b {
            return false;
        }
        let no = self.incompatible.get(usize::from(a)).copied().unwrap_or(0);
        no & (1u32 << u32::from(b).min(31)) == 0
    }

    /// The weapon file name candidates for a weapon with attachments (IW4 `<base>[_<att>[_<att>]]_mp`; both orders
    /// of two attachments, since the file names use one fixed order).
    pub fn weapon_names(&self, weapon: u8, attachments: [u8; 2]) -> Vec<String> {
        let Some(w) = self.weapons.get(usize::from(weapon)) else {
            return Vec::new();
        };
        let att: Vec<&str> = attachments
            .iter()
            .filter(|&&a| a != 0 && a != NONE)
            .filter_map(|&a| self.attachments.get(usize::from(a)))
            .map(|a| a.name.as_str())
            .collect();
        let join = |parts: &[&str]| {
            let mut s = String::from(w.name.as_str());
            for p in parts {
                s.push('_');
                s.push_str(p);
            }
            s.push_str("_mp");
            s
        };
        match att.as_slice() {
            [] => alloc::vec![join(&[])],
            [a] => alloc::vec![join(&[a])],
            [a, b] => alloc::vec![join(&[a, b]), join(&[b, a])],
            _ => Vec::new(),
        }
    }

    fn name_of(list: &[Item], i: u8) -> &str {
        list.get(usize::from(i)).map_or("", |x| x.name.as_str())
    }

    fn index_of(list: &[Item], name: &str) -> u8 {
        if name.is_empty() {
            return NONE;
        }
        list.iter()
            .position(|x| x.name == name)
            .map_or(NONE, |i| i as u8)
    }

    /// Saved classes (SD card): item names, so they still match after the pack is rebuilt.
    pub fn encode_loadouts(&self, ls: &[Loadout]) -> Vec<u8> {
        let mut o = Vec::from(&SAVE_MAGIC[..]);
        o.push(ls.len() as u8);
        for l in ls {
            put_str(&mut o, &l.name);
            let w = |i| Self::name_of(&self.weapons, i);
            let a = |i| Self::name_of(&self.attachments, i);
            let p = |i| Self::name_of(&self.perks, i);
            for s in [
                w(l.primary),
                a(l.primary_attachments[0]),
                a(l.primary_attachments[1]),
                w(l.secondary),
                a(l.secondary_attachments[0]),
                a(l.secondary_attachments[1]),
                p(l.equipment),
                p(l.perks[0]),
                p(l.perks[1]),
                p(l.perks[2]),
                Self::name_of(&self.offhands, l.offhand),
                p(l.deathstreak),
                Self::name_of(&self.killstreaks, l.killstreaks[0]),
                Self::name_of(&self.killstreaks, l.killstreaks[1]),
                Self::name_of(&self.killstreaks, l.killstreaks[2]),
            ] {
                put_str(&mut o, s);
            }
        }
        o
    }

    /// Classes saved by [`Catalog::encode_loadouts`]; names no longer in the catalog become [`NONE`].
    pub fn decode_loadouts(&self, b: &[u8]) -> Result<Vec<Loadout>, DecodeError> {
        if b.get(..4) != Some(&SAVE_MAGIC[..]) {
            return Err(DecodeError);
        }
        let mut r = Reader { b, at: 4 };
        let n = r.u8()?;
        let mut out = Vec::new();
        for _ in 0..n {
            let name = r.str()?;
            let mut s: [String; 15] = Default::default();
            for x in &mut s {
                *x = r.str()?;
            }
            let w = |x: &str| Self::index_of(&self.weapons, x);
            let a = |x: &str| match Self::index_of(&self.attachments, x) {
                NONE => 0,
                i => i,
            };
            let p = |x: &str| Self::index_of(&self.perks, x);
            let k = |x: &str| Self::index_of(&self.killstreaks, x);
            out.push(Loadout {
                name,
                primary: w(&s[0]),
                primary_attachments: [a(&s[1]), a(&s[2])],
                secondary: w(&s[3]),
                secondary_attachments: [a(&s[4]), a(&s[5])],
                equipment: p(&s[6]),
                perks: [p(&s[7]), p(&s[8]), p(&s[9])],
                offhand: Self::index_of(&self.offhands, &s[10]),
                deathstreak: p(&s[11]),
                killstreaks: [k(&s[12]), k(&s[13]), k(&s[14])],
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn item(name: &str, kind: u8, attachments: u32) -> Item {
        Item {
            name: name.to_string(),
            display: name.to_uppercase(),
            kind,
            attachments,
        }
    }

    fn catalog() -> Catalog {
        let l = Loadout {
            name: "A".into(),
            primary: 0,
            primary_attachments: [1, 2],
            secondary: 1,
            secondary_attachments: [0, 0],
            equipment: 0,
            perks: [1, NONE, NONE],
            offhand: 0,
            deathstreak: NONE,
            killstreaks: [0, NONE, NONE],
        };
        Catalog {
            weapons: vec![
                item("rifle", group::ASSAULT, 0b110),
                item("pistol", group::PISTOL, 0),
            ],
            attachments: vec![
                item("none", 0, 0),
                item("scope", 0, 0),
                item("ammo", 0, 0),
                item("dot", 0, 0),
            ],
            incompatible: vec![0, 0b1000, 0, 0b10],
            perks: vec![
                item("frag", slot::EQUIPMENT, 0),
                item("specialty_fast", slot::PERK1, 0),
            ],
            offhands: vec![item("flash", 0, 0)],
            killstreaks: vec![item("radar", 0, 0)],
            defaults: vec![l.clone()],
            customs: vec![l],
        }
    }

    #[test]
    fn catalog_round_trips() {
        let c = catalog();
        assert_eq!(Catalog::decode(&c.encode()), Ok(c));
    }

    #[test]
    fn combos_and_weapon_names() {
        let c = catalog();
        assert!(!c.compatible(1, 3));
        assert!(!c.compatible(3, 1) || c.incompatible[3] & 0b10 == 0);
        assert!(c.compatible(1, 2));
        assert!(!c.compatible(2, 2));
        assert_eq!(c.weapon_names(0, [0, NONE]), vec!["rifle_mp".to_string()]);
        assert_eq!(
            c.weapon_names(0, [1, 2]),
            vec![
                "rifle_scope_ammo_mp".to_string(),
                "rifle_ammo_scope_mp".to_string()
            ]
        );
    }

    #[test]
    fn saved_classes_use_names() {
        let c = catalog();
        let saved = c.encode_loadouts(&c.customs);
        assert_eq!(c.decode_loadouts(&saved), Ok(c.customs.clone()));
        // A rebuilt catalog with the items in another order still finds them.
        let mut d = c.clone();
        d.attachments.swap(1, 2);
        let l = &d.decode_loadouts(&saved).unwrap()[0];
        assert_eq!(l.primary_attachments, [2, 1]);
        assert!(c.decode_loadouts(b"nope").is_err());
    }
}

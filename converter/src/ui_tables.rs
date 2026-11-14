//! Class data for the menus: MW2's own tables (stats, attachments, perks, killstreaks, default classes) and
//! localized names, read from the user's zones at convert time into a `wpk::classes::Catalog`.

use std::collections::{HashMap, HashSet};

use wpk::classes::{Catalog, Item, Loadout, NONE, group, slot};

/// A string table: `cells` row by row, `columns` wide.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub columns: usize,
    pub cells: Vec<String>,
}

impl Table {
    pub fn rows(&self) -> usize {
        self.cells.len().checked_div(self.columns).unwrap_or(0)
    }

    pub fn cell(&self, row: usize, col: usize) -> &str {
        if col >= self.columns {
            return "";
        }
        self.cells
            .get(row * self.columns + col)
            .map_or("", String::as_str)
    }
}

/// What `catalog` found (counts only: reports stay content-free).
#[derive(Debug, Default)]
pub struct CatalogReport {
    pub weapons: usize,
    pub weapons_without_file: usize,
    pub attachments: usize,
    pub perks: usize,
    pub missing_names: usize,
    pub missing_tables: Vec<&'static str>,
}

pub const STATS: &str = "mp/statstable.csv";
const ATTACHMENTS: &str = "mp/attachmenttable.csv";
const COMBOS: &str = "mp/attachmentcombos.csv";
const PERKS: &str = "mp/perktable.csv";
const KILLSTREAKS: &str = "mp/killstreaktable.csv";
const CLASSES: &str = "mp/classtable.csv";

fn item(name: &str, display: String, kind: u8, attachments: u32) -> Item {
    Item {
        name: name.to_owned(),
        display,
        kind,
        attachments,
    }
}

/// Base names of the weapons a class can carry (statsTable rows in a weapon group), in table order.
pub fn weapon_bases(tables: &HashMap<String, Table>) -> Vec<String> {
    let Some(stats) = tables.get(STATS) else {
        return Vec::new();
    };
    (1..stats.rows())
        .filter(|&r| (0..group::COUNT).any(|k| group::table_name(k) == stats.cell(r, 2)))
        .map(|r| stats.cell(r, 4).to_owned())
        .filter(|b| !b.is_empty())
        .collect()
}

/// The Create-a-Class catalog from MW2's tables (`code_post_gfx_mp`) and localized names. `weapon_files`: weapon
/// names in `WEAP`; weapons without a `<name>_mp` file are left out.
pub fn catalog(
    tables: &HashMap<String, Table>,
    localize: &HashMap<String, String>,
    weapon_files: &HashSet<String>,
) -> (Catalog, CatalogReport) {
    let mut report = CatalogReport::default();
    let empty = Table::default();
    let mut table = |n: &'static str| match tables.get(n) {
        Some(t) => t,
        None => {
            report.missing_tables.push(n);
            &empty
        }
    };
    let stats = table(STATS);
    let atts = table(ATTACHMENTS);
    let combos = table(COMBOS);
    let perks = table(PERKS);
    let streaks = table(KILLSTREAKS);
    let classes = table(CLASSES);
    let missing = std::cell::Cell::new(0usize);
    let text = |key: &str| -> String {
        match localize.get(key) {
            Some(v) => v.clone(),
            None => {
                missing.set(missing.get() + 1);
                key.to_owned()
            }
        }
    };
    let numeric = |t: &Table, r: usize| t.cell(r, 0).parse::<u32>().is_ok();

    // Attachments (0 = none; the menu's "done" row is not an attachment).
    let mut attachments: Vec<Item> = Vec::new();
    for r in 1..atts.rows() {
        let name = atts.cell(r, 4);
        if name.is_empty() || name == "done" {
            continue;
        }
        let it = item(name, text(atts.cell(r, 3)), 0, 0);
        if name == "none" {
            attachments.insert(0, it);
        } else {
            attachments.push(it);
        }
    }
    let att_index = |n: &str| attachments.iter().position(|a| a.name == n);
    // Incompatible pairs: row and column names in the combos table, "no" in a cell.
    let mut incompatible = vec![0u32; attachments.len()];
    for r in 1..combos.rows() {
        let Some(a) = att_index(combos.cell(r, 0)) else {
            continue;
        };
        for c in 1..combos.columns {
            if combos.cell(r, c) == "no"
                && let Some(b) = att_index(combos.cell(0, c))
                && b < 32
            {
                incompatible[a] |= 1 << b;
            }
        }
    }

    // Weapons and special grenades (statsTable).
    let mut weapons = Vec::new();
    let mut offhands = Vec::new();
    for r in 1..stats.rows() {
        let (g, base) = (stats.cell(r, 2), stats.cell(r, 4));
        if base.is_empty() {
            continue;
        }
        if g == "weapon_grenade" {
            // Frag and throwing knife are equipment (perkTable); the rest are special grenades.
            if base != "frag_grenade" && base != "throwingknife" {
                offhands.push(item(base, text(stats.cell(r, 3)), 0, 0));
            }
            continue;
        }
        let Some(kind) = (0..group::COUNT).find(|&k| group::table_name(k) == g) else {
            continue;
        };
        if !weapon_files.contains(&format!("{base}_mp")) {
            report.weapons_without_file += 1;
            continue;
        }
        let mut mask = 0u32;
        for c in 11..=21 {
            if let Some(a) = att_index(stats.cell(r, c))
                && a < 32
            {
                mask |= 1 << a;
            }
        }
        weapons.push(item(base, text(stats.cell(r, 3)), kind, mask));
    }

    // Perks, deathstreaks and equipment (perkTable rows with a numeric index).
    let mut perk_items = Vec::new();
    for r in 1..perks.rows() {
        if !numeric(perks, r) {
            continue;
        }
        let name = perks.cell(r, 1);
        let kind = match perks.cell(r, 5) {
            "perk1" => slot::PERK1,
            "perk2" => slot::PERK2,
            "perk3" => slot::PERK3,
            "perk4" => slot::DEATHSTREAK,
            "equipment" => slot::EQUIPMENT,
            _ => continue,
        };
        if name.is_empty() || name.starts_with('_') || name == "specialty_null" {
            continue;
        }
        perk_items.push(item(name, text(perks.cell(r, 2)), kind, 0));
    }

    // Killstreaks (rows with a numeric index, "none" left out).
    let mut killstreaks = Vec::new();
    for r in 1..streaks.rows() {
        let name = streaks.cell(r, 1);
        if !numeric(streaks, r) || name.is_empty() || name == "none" {
            continue;
        }
        killstreaks.push(item(name, text(streaks.cell(r, 2)), 0, 0));
    }

    // Classes: one column each, rows keyed by column 0. Columns 1-5: the default classes; 6-10: the custom classes a
    // new player starts with.
    let row = |key: &str| (0..classes.rows()).find(|&r| classes.cell(r, 0) == key);
    let pick = |list: &[Item], v: &str| {
        list.iter()
            .position(|x| x.name == v)
            .map_or(NONE, |i| i as u8)
    };
    let loadout = |c: usize, name: String| -> Loadout {
        let get = |key: &str| row(key).map_or("", |r| classes.cell(r, c));
        let att = |key: &str| match get(key) {
            "" | "none" => 0,
            v => att_index(v).map_or(0, |i| i as u8),
        };
        Loadout {
            name,
            primary: pick(&weapons, get("loadoutPrimary")),
            primary_attachments: [
                att("loadoutPrimaryAttachment"),
                att("loadoutPrimaryAttachment2"),
            ],
            secondary: pick(&weapons, get("loadoutSecondary")),
            secondary_attachments: [
                att("loadoutSecondaryAttachment"),
                att("loadoutSecondaryAttachment2"),
            ],
            equipment: pick(&perk_items, get("loadoutEquipment")),
            perks: [
                pick(&perk_items, get("loadoutPerk1")),
                pick(&perk_items, get("loadoutPerk2")),
                pick(&perk_items, get("loadoutPerk3")),
            ],
            offhand: pick(&offhands, get("loadoutOffHand")),
            deathstreak: pick(&perk_items, get("loadoutDeathStreak")),
            killstreaks: [
                pick(&killstreaks, get("loadoutStreak1")),
                pick(&killstreaks, get("loadoutStreak2")),
                pick(&killstreaks, get("loadoutStreak3")),
            ],
        }
    };
    let name_row = row("loadoutName");
    let defaults: Vec<Loadout> = (1..=5)
        .filter(|&c| c < classes.columns)
        .map(|c| loadout(c, text(name_row.map_or("", |r| classes.cell(r, c)))))
        .collect();
    let customs: Vec<Loadout> = (6..=10)
        .filter(|&c| c < classes.columns)
        .map(|c| loadout(c, text(&format!("CLASS_CUSTOM{}", c - 5))))
        .collect();

    report.weapons = weapons.len();
    report.attachments = attachments.len();
    report.perks = perk_items.len();
    report.missing_names = missing.get();
    (
        Catalog {
            weapons,
            attachments,
            incompatible,
            perks: perk_items,
            offhands,
            killstreaks,
            defaults,
            customs,
        },
        report,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(rows: &[&[&str]]) -> Table {
        let columns = rows[0].len();
        Table {
            columns,
            cells: rows
                .iter()
                .flat_map(|r| r.iter().map(|c| c.to_string()))
                .collect(),
        }
    }

    #[test]
    fn builds_a_catalog_from_synthetic_tables() {
        // Synthetic tables in MW2's layouts (made-up contents).
        let mut stats_row = vec![
            "1",
            "",
            "weapon_assault",
            "W_RIFLE",
            "rifle",
            "",
            "",
            "",
            "",
            "",
            "",
            "scope",
        ];
        stats_row.resize(22, "");
        let mut header = vec!["a"; 22];
        header[0] = "a0";
        let mut nofile = stats_row.clone();
        nofile[4] = "ghost";
        let mut grenade = vec!["3", "", "weapon_grenade", "W_FLASH", "flash_grenade"];
        grenade.resize(22, "");
        let mut tables = HashMap::new();
        tables.insert(
            STATS.to_owned(),
            t(&[&header, &stats_row, &nofile, &grenade]),
        );
        tables.insert(
            ATTACHMENTS.to_owned(),
            t(&[
                &["a0", "b", "c", "d", "e"],
                &["1", "", "none", "A_NONE", "none"],
                &["2", "", "rail", "A_SCOPE", "scope"],
                &["3", "", "rail", "A_DOT", "dot"],
            ]),
        );
        tables.insert(
            COMBOS.to_owned(),
            t(&[
                &["no", "scope", "dot"],
                &["scope", "no", "no"],
                &["dot", "no", "no"],
            ]),
        );
        tables.insert(
            PERKS.to_owned(),
            t(&[
                &["a", "b", "c", "d", "e", "f"],
                &["0", "specialty_fast", "P_FAST", "", "", "perk1"],
                &["1", "frag_mp", "P_FRAG", "", "", "equipment"],
            ]),
        );
        tables.insert(
            KILLSTREAKS.to_owned(),
            t(&[
                &["a", "b", "c"],
                &["0", "none", "K_NONE"],
                &["1", "radar", "K_RADAR"],
            ]),
        );
        tables.insert(
            CLASSES.to_owned(),
            t(&[
                &["loadoutName", "C1", "", "", "", "", "C1"],
                &["loadoutPrimary", "rifle", "", "", "", "", "rifle"],
                &["loadoutPrimaryAttachment", "scope", "", "", "", "", "none"],
                &["loadoutPerk1", "specialty_fast", "", "", "", "", ""],
                &["loadoutEquipment", "frag_mp", "", "", "", "", ""],
                &["loadoutOffHand", "flash_grenade", "", "", "", "", ""],
                &["loadoutStreak1", "radar", "", "", "", "", ""],
            ]),
        );
        let loc: HashMap<String, String> = [
            ("W_RIFLE", "Rifle"),
            ("C1", "Class One"),
            ("CLASS_CUSTOM1", "Mine"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .collect();
        let files: HashSet<String> = ["rifle_mp".to_owned()].into_iter().collect();
        let (c, r) = catalog(&tables, &loc, &files);
        assert!(r.missing_tables.is_empty());
        assert_eq!(c.weapons.len(), 1);
        assert_eq!(r.weapons_without_file, 1);
        assert_eq!(c.weapons[0].display, "Rifle");
        assert_eq!(c.weapons[0].attachments, 0b10);
        assert_eq!(c.attachments[0].name, "none");
        assert!(!c.compatible(1, 2));
        assert_eq!(c.offhands[0].name, "flash_grenade");
        assert_eq!(c.killstreaks.len(), 1);
        let d = &c.defaults[0];
        assert_eq!(d.name, "Class One");
        assert_eq!((d.primary, d.primary_attachments), (0, [1, 0]));
        assert_eq!(d.perks[0], 0);
        assert_eq!(d.equipment, 1);
        assert_eq!((d.offhand, d.killstreaks[0]), (0, 0));
        assert_eq!(c.customs[0].name, "Mine");
        assert_eq!(c.customs[0].primary_attachments, [0, 0]);
    }
}

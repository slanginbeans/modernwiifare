//! The player's options (MW2's Options > Controls on the console): look sensitivity, inverted look, aim assist,
//! the button layout and the stick layout, kept in the app folder (`options.dat`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::look::LookSettings;

const VERSION: u8 = 1;

/// MW2's console sensitivity runs from 1 (low) to 10 (insane); 3 is its default and our IW4L look curve's 1.0.
pub const SENSITIVITY_DEFAULT: u8 = 3;
pub const SENSITIVITY_MAX: u8 = 10;

/// Button layouts (MW2 console "Default" and "Tactical": Tactical swaps the stance and melee buttons).
pub mod layout {
    pub const DEFAULT: u8 = 0;
    pub const TACTICAL: u8 = 1;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub sensitivity: u8,
    /// Aim-down-sights sensitivity, the same 1..10 scale.
    pub ads_sensitivity: u8,
    pub invert: bool,
    /// MW2's target assist (slowdown, lock-on and the ADS snap).
    pub aim_assist: bool,
    pub layout: u8,
    /// Southpaw: move with the C-stick, look with the main stick.
    pub southpaw: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            sensitivity: SENSITIVITY_DEFAULT,
            ads_sensitivity: SENSITIVITY_DEFAULT,
            invert: false,
            aim_assist: true,
            layout: layout::DEFAULT,
            southpaw: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Sensitivity,
    AdsSensitivity,
    Invert,
    AimAssist,
    Layout,
    Southpaw,
    Defaults,
}

pub const ITEMS: [Item; 7] = [
    Item::Sensitivity,
    Item::AdsSensitivity,
    Item::Invert,
    Item::AimAssist,
    Item::Layout,
    Item::Southpaw,
    Item::Defaults,
];

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::Sensitivity => "LOOK SENSITIVITY",
            Item::AdsSensitivity => "AIM DOWN SIGHT SENSITIVITY",
            Item::Invert => "LOOK INVERSION",
            Item::AimAssist => "TARGET ASSIST",
            Item::Layout => "BUTTON LAYOUT",
            Item::Southpaw => "STICK LAYOUT",
            Item::Defaults => "RESTORE DEFAULTS",
        }
    }
}

impl Options {
    pub fn value(&self, item: Item) -> Option<String> {
        let on_off = |b: bool| String::from(if b { "ENABLED" } else { "DISABLED" });
        Some(match item {
            Item::Sensitivity => alloc::format!("{}", self.sensitivity),
            Item::AdsSensitivity => alloc::format!("{}", self.ads_sensitivity),
            Item::Invert => on_off(self.invert),
            Item::AimAssist => on_off(self.aim_assist),
            Item::Layout => String::from(if self.layout == layout::TACTICAL {
                "TACTICAL"
            } else {
                "DEFAULT"
            }),
            Item::Southpaw => String::from(if self.southpaw { "SOUTHPAW" } else { "DEFAULT" }),
            Item::Defaults => return None,
        })
    }

    pub fn cycle(&mut self, item: Item) {
        let step = |v: u8| if v >= SENSITIVITY_MAX { 1 } else { v + 1 };
        match item {
            Item::Sensitivity => self.sensitivity = step(self.sensitivity),
            Item::AdsSensitivity => self.ads_sensitivity = step(self.ads_sensitivity),
            Item::Invert => self.invert = !self.invert,
            Item::AimAssist => self.aim_assist = !self.aim_assist,
            Item::Layout => self.layout = (self.layout + 1) % 2,
            Item::Southpaw => self.southpaw = !self.southpaw,
            Item::Defaults => *self = Options::default(),
        }
    }

    /// The look settings these options give (sensitivity relative to MW2's default of 3).
    pub fn look(&self) -> LookSettings {
        let scale = |v: u8| f32::from(v.max(1)) / f32::from(SENSITIVITY_DEFAULT);
        LookSettings {
            sensitivity: scale(self.sensitivity),
            ads_sensitivity: scale(self.ads_sensitivity),
            invert: self.invert,
            aim_assist: if self.aim_assist { 2 } else { 0 },
            ..LookSettings::default()
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        alloc::vec![
            VERSION,
            self.sensitivity,
            self.ads_sensitivity,
            u8::from(self.invert),
            u8::from(self.aim_assist),
            self.layout,
            u8::from(self.southpaw),
        ]
    }

    pub fn decode(b: &[u8]) -> Option<Options> {
        if b.len() < 7 || b[0] != VERSION {
            return None;
        }
        let sens = |v: u8| v.clamp(1, SENSITIVITY_MAX);
        Some(Options {
            sensitivity: sens(b[1]),
            ads_sensitivity: sens(b[2]),
            invert: b[3] != 0,
            aim_assist: b[4] != 0,
            layout: b[5] % 2,
            southpaw: b[6] != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_and_map_to_look_settings() {
        let mut o = Options::default();
        assert_eq!(Options::decode(&o.encode()), Some(o));
        assert_eq!(o.look().sensitivity, 1.0);
        o.cycle(Item::Sensitivity);
        o.cycle(Item::Invert);
        o.cycle(Item::Layout);
        o.cycle(Item::AimAssist);
        assert_eq!(Options::decode(&o.encode()), Some(o));
        let l = o.look();
        assert!(l.sensitivity > 1.0 && l.invert && l.aim_assist == 0);
        // 10 wraps to 1.
        o.sensitivity = SENSITIVITY_MAX;
        o.cycle(Item::Sensitivity);
        assert_eq!(o.sensitivity, 1);
        o.cycle(Item::Defaults);
        assert_eq!(o, Options::default());
    }
}

//! The player's options (MW2's Options > Controls on the console): look sensitivity, inverted look, aim assist,
//! the button layout and the stick layout, vibration (the pad's rumble), and the HUD's safe area (MW2's console
//! safe-area adjustment, so the HUD stays inside a TV's overscan), kept in the app folder (`options.dat`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::look::LookSettings;

const VERSION: u8 = 1;

/// MW2's console sensitivity runs from 1 (low) to 10 (insane); 3 is its default and our IW4L look curve's 1.0.
pub const SENSITIVITY_DEFAULT: u8 = 3;
pub const SENSITIVITY_MAX: u8 = 10;

/// The HUD's safe area in percent of the screen (MW2's `safeArea_adjusted`): 80 .. 100 in steps of 2.
pub const SAFE_AREA_MIN: u8 = 80;
pub const SAFE_AREA_MAX: u8 = 100;
pub const SAFE_AREA_STEP: u8 = 2;

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
    /// The HUD's safe area, percent.
    pub safe_area: u8,
    /// MW2's console "Vibration": the pad rumbles on shots and hits.
    pub vibration: bool,
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
            safe_area: (crate::hud_mw2::SAFE_AREA_DEFAULT * 100.0 + 0.5) as u8,
            vibration: true,
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
    SafeArea,
    Vibration,
    Defaults,
}

pub const ITEMS: [Item; 9] = [
    Item::Sensitivity,
    Item::AdsSensitivity,
    Item::Invert,
    Item::AimAssist,
    Item::Layout,
    Item::Southpaw,
    Item::SafeArea,
    Item::Vibration,
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
            Item::SafeArea => "SAFE AREA",
            Item::Vibration => "VIBRATION",
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
            Item::SafeArea => alloc::format!("{}%", self.safe_area),
            Item::Vibration => on_off(self.vibration),
            Item::Defaults => return None,
        })
    }

    /// A: the next value (wrapping), or restore the defaults.
    pub fn cycle(&mut self, item: Item) {
        self.adjust(item, 1, true);
    }

    /// Left / right: the previous or next value (`dir` -1 or 1); `wrap` goes round at the ends.
    pub fn adjust(&mut self, item: Item, dir: i8, wrap: bool) {
        let step = |v: u8, lo: u8, hi: u8, by: u8| {
            if dir > 0 {
                match v.checked_add(by).filter(|&n| n <= hi) {
                    Some(n) => n,
                    None if wrap => lo,
                    None => hi,
                }
            } else {
                match v.checked_sub(by).filter(|&n| n >= lo) {
                    Some(n) => n,
                    None if wrap => hi,
                    None => lo,
                }
            }
        };
        match item {
            Item::Sensitivity => self.sensitivity = step(self.sensitivity, 1, SENSITIVITY_MAX, 1),
            Item::AdsSensitivity => {
                self.ads_sensitivity = step(self.ads_sensitivity, 1, SENSITIVITY_MAX, 1)
            }
            Item::Invert => self.invert = !self.invert,
            Item::AimAssist => self.aim_assist = !self.aim_assist,
            Item::Layout => self.layout = (self.layout + 1) % 2,
            Item::Southpaw => self.southpaw = !self.southpaw,
            Item::SafeArea => {
                self.safe_area = step(self.safe_area, SAFE_AREA_MIN, SAFE_AREA_MAX, SAFE_AREA_STEP)
            }
            Item::Vibration => self.vibration = !self.vibration,
            Item::Defaults => {
                if dir > 0 && wrap {
                    *self = Options::default()
                }
            }
        }
    }

    /// The HUD's safe area as a share of the screen.
    pub fn safe_area(&self) -> f32 {
        f32::from(self.safe_area.clamp(SAFE_AREA_MIN, SAFE_AREA_MAX)) / 100.0
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
            self.safe_area,
            u8::from(self.vibration),
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
            // Files from before the safe area option keep the default.
            safe_area: b.get(7).map_or(Options::default().safe_area, |&v| {
                v.clamp(SAFE_AREA_MIN, SAFE_AREA_MAX)
            }),
            // ... and from before vibration keep it on.
            vibration: b.get(8).is_none_or(|&v| v != 0),
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

    #[test]
    fn left_and_right_stop_at_the_ends() {
        let mut o = Options::default();
        assert_eq!(o.safe_area, 90);
        o.adjust(Item::SafeArea, 1, false);
        assert_eq!(o.value(Item::SafeArea).as_deref(), Some("92%"));
        for _ in 0..10 {
            o.adjust(Item::SafeArea, 1, false);
        }
        assert_eq!(o.safe_area, SAFE_AREA_MAX);
        o.adjust(Item::Sensitivity, -1, false);
        assert_eq!(o.sensitivity, 2);
        o.sensitivity = 1;
        o.adjust(Item::Sensitivity, -1, false);
        assert_eq!(o.sensitivity, 1);
        // Left / right never restore the defaults.
        o.adjust(Item::Defaults, 1, false);
        assert_eq!(o.sensitivity, 1);
        // Vibration toggles and is kept.
        o.cycle(Item::Vibration);
        assert_eq!(o.value(Item::Vibration).as_deref(), Some("DISABLED"));
        assert_eq!(
            Options::decode(&o.encode()).map(|d| d.vibration),
            Some(false)
        );
        assert_eq!(
            Options::decode(&o.encode()[..8]).map(|d| d.vibration),
            Some(true)
        );
        // Old files (no safe area byte) still read.
        assert_eq!(
            Options::decode(&o.encode()[..7]).map(|d| d.safe_area),
            Some(90)
        );
    }
}

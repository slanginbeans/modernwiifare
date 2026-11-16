//! Perk, deathstreak, last stand and respawn rules read from the user's own MW2 scripts at convert time (`LIFE` in
//! `common.wpk`, ADR 0007: read for values, never run): `_damage.gsc` (Last Stand and Final Stand timers, the hold to
//! give up), `_perkfunctions.gsc` (Painkiller's time, One Man Army's class change), `_perks.gsc` (Painkiller's damage
//! divisor), `_weapons.gsc` (Scrambler Pro's claymore delay), `_class.gsc` (Hardline's killstreak modifier) and
//! `_tweakables.gsc` (whether killcams are allowed). Values the scripts don't yield keep MW2's stock values
//! ([`LifeRules::default`]); `found` says which were read. A separate section from `RULE` so packs of either age
//! decode.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Bits of [`LifeRules::found`].
pub mod found {
    pub const LAST_STAND: u16 = 1;
    pub const FINAL_STAND: u16 = 1 << 1;
    pub const GIVE_UP: u16 = 1 << 2;
    pub const PAINKILLER: u16 = 1 << 3;
    pub const PAINKILLER_DIVISOR: u16 = 1 << 4;
    pub const OMA: u16 = 1 << 5;
    pub const DELAY_MINE: u16 = 1 << 6;
    pub const HARDLINE: u16 = 1 << 7;
    pub const KILLCAM: u16 = 1 << 8;
    pub const ALL: u16 = (1 << 9) - 1;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LifeRules {
    /// Last Stand's bleed-out (`lastStandTimer( 10, false )`).
    pub last_stand_ms: u32,
    /// Final Stand's wait before getting back up (`lastStandTimer( 20, true )`).
    pub final_stand_ms: u32,
    /// Holding use this long in Last Stand gives up (`lastStandAllowSuicide`: `> 700`).
    pub give_up_ms: u32,
    /// Painkiller (`setCombatHigh`: its two waits).
    pub painkiller_ms: u32,
    /// Painkiller's damage divisor (`cac_modified_damage`: `damage/3`).
    pub painkiller_divisor: f32,
    /// One Man Army's class change, and with its Pro perk (`giveOneManArmyClass`'s `changeDuration`).
    pub oma_ms: u32,
    pub oma_quick_ms: u32,
    /// A claymore tripped by a Scrambler Pro (`specialty_delaymine`) player waits this long (`wait 3.0`).
    pub delay_mine_ms: u32,
    /// Hardline's change to every killstreak's cost (`setKillstreaks`: `modifier = -1`).
    pub hardline_modifier: i32,
    /// Killcams on (`scr_game_allowkillcam`'s default).
    pub killcam: bool,
    pub found: u16,
}

impl Default for LifeRules {
    /// MW2's stock values.
    fn default() -> Self {
        LifeRules {
            last_stand_ms: 10_000,
            final_stand_ms: 20_000,
            give_up_ms: 700,
            painkiller_ms: 10_000,
            painkiller_divisor: 3.0,
            oma_ms: 6000,
            oma_quick_ms: 3000,
            delay_mine_ms: 3000,
            hardline_modifier: -1,
            killcam: true,
            found: 0,
        }
    }
}

impl LifeRules {
    pub const LEN: usize = 46;

    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(Self::LEN);
        for v in [
            self.last_stand_ms,
            self.final_stand_ms,
            self.give_up_ms,
            self.painkiller_ms,
        ] {
            o.u32(v);
        }
        o.f32(self.painkiller_divisor);
        for v in [
            self.oma_ms,
            self.oma_quick_ms,
            self.delay_mine_ms,
            self.hardline_modifier as u32,
            u32::from(self.killcam),
        ] {
            o.u32(v);
        }
        o.u16(self.found);
        o.zeros(Self::LEN - o.len());
        o
    }

    pub fn decode(b: &[u8]) -> Option<LifeRules> {
        if b.len() < Self::LEN {
            return None;
        }
        let u = |i: usize| be::u32(b, i * 4);
        Some(LifeRules {
            last_stand_ms: u(0),
            final_stand_ms: u(1),
            give_up_ms: u(2),
            painkiller_ms: u(3),
            painkiller_divisor: be::f32(b, 16),
            oma_ms: u(5),
            oma_quick_ms: u(6),
            delay_mine_ms: u(7),
            hardline_modifier: u(8) as i32,
            killcam: u(9) != 0,
            found: be::u16(b, 40),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let r = LifeRules {
            last_stand_ms: 12_000,
            hardline_modifier: -2,
            killcam: false,
            found: found::LAST_STAND | found::HARDLINE,
            ..LifeRules::default()
        };
        assert_eq!(r.encode().len(), LifeRules::LEN);
        assert_eq!(LifeRules::decode(&r.encode()), Some(r));
        assert!(LifeRules::decode(&r.encode()[..20]).is_none());
    }
}

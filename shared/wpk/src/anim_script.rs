//! MW2's player animation script (`common.wpk` `PSCR`), baked from the user's `mp/playeranim.script` at convert
//! time: for each movement type and each event, the script's items in order. Each item has conditions and commands.
//! The first item whose conditions all hold is the one used. An item with no commands matches and plays nothing.
//!
//! Condition and movement-type numbers are IW4's (IW4L `anim_iw4::ANIM_COND_*`, `ANIM_MT_*`, `ANIM_ET_*`).
//! A command's clip indexes the `PANM` clips.

use alloc::vec::Vec;

use crate::be::{self, Push};

/// Body parts a command plays on (IW4 `ANIM_BP_*`).
pub mod part {
    pub const LEGS: u8 = 1;
    pub const TORSO: u8 = 2;
    pub const BOTH: u8 = 3;
}

/// No blend time in the script (the client's own rule applies).
pub const NO_BLEND: i32 = -1;

/// Conditions IW4 knows (`ANIM_COND_*` 0..18).
pub const CONDITIONS: usize = 18;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Condition {
    /// IW4 `ANIM_COND_*`.
    pub index: u8,
    /// A bit set (any bit in common holds) rather than one value.
    pub bitflags: bool,
    pub bits: u64,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Command {
    /// [`part`].
    pub part: u8,
    /// Index into the `PANM` clips.
    pub clip: u16,
    /// How long an event's animation holds its body part (ms; 0 = the clip's length).
    pub duration_ms: i32,
    /// Blend time (ms) or [`NO_BLEND`].
    pub blend_ms: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    /// Uses a condition the game can't evaluate: never matches.
    pub skip: bool,
    pub conditions: Vec<Condition>,
    pub commands: Vec<Command>,
}

/// The whole script.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    /// (IW4 `ANIM_MT_*`, items) for the one state MP uses (`COMBAT`).
    pub moves: Vec<(u8, Vec<Item>)>,
    /// (IW4 `ANIM_ET_*`, items).
    pub events: Vec<(u8, Vec<Item>)>,
}

impl Script {
    pub fn move_items(&self, movetype: u8) -> &[Item] {
        self.moves
            .iter()
            .find(|(m, _)| *m == movetype)
            .map_or(&[], |(_, i)| i.as_slice())
    }

    pub fn event_items(&self, event: u8) -> &[Item] {
        self.events
            .iter()
            .find(|(e, _)| *e == event)
            .map_or(&[], |(_, i)| i.as_slice())
    }

    /// Every clip a command plays.
    pub fn clips(&self) -> impl Iterator<Item = u16> + '_ {
        self.moves
            .iter()
            .chain(&self.events)
            .flat_map(|(_, items)| items)
            .flat_map(|i| &i.commands)
            .map(|c| c.clip)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.u16(self.moves.len() as u16);
        b.u16(self.events.len() as u16);
        for (id, items) in self.moves.iter().chain(&self.events) {
            b.u8(*id);
            b.u8(0);
            b.u16(items.len() as u16);
            for it in items {
                b.u8(u8::from(it.skip));
                b.u8(it.conditions.len() as u8);
                b.u8(it.commands.len() as u8);
                b.u8(0);
                for c in &it.conditions {
                    b.u8(c.index);
                    b.u8(u8::from(c.bitflags));
                    b.u16(0);
                    b.u32(c.value as u32);
                    b.u32((c.bits >> 32) as u32);
                    b.u32(c.bits as u32);
                }
                for c in &it.commands {
                    b.u8(c.part);
                    b.u8(0);
                    b.u16(c.clip);
                    b.u32(c.duration_ms as u32);
                    b.u32(c.blend_ms as u32);
                }
            }
        }
        b
    }

    /// `None` when `b` is cut short.
    pub fn decode(b: &[u8]) -> Option<Script> {
        let need = |at: usize, n: usize| (at + n <= b.len()).then_some(());
        need(0, 4)?;
        let (nm, ne) = (usize::from(be::u16(b, 0)), usize::from(be::u16(b, 2)));
        let mut at = 4;
        let mut out = Script::default();
        for k in 0..nm + ne {
            need(at, 4)?;
            let id = b[at];
            let n = usize::from(be::u16(b, at + 2));
            at += 4;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                need(at, 4)?;
                let (skip, nc, nk) = (b[at] != 0, usize::from(b[at + 1]), usize::from(b[at + 2]));
                at += 4;
                need(at, nc * 16 + nk * 12)?;
                let conditions = (0..nc)
                    .map(|i| {
                        let p = at + i * 16;
                        Condition {
                            index: b[p],
                            bitflags: b[p + 1] != 0,
                            value: be::u32(b, p + 4) as i32,
                            bits: be::u64(b, p + 8),
                        }
                    })
                    .collect();
                at += nc * 16;
                let commands = (0..nk)
                    .map(|i| {
                        let p = at + i * 12;
                        Command {
                            part: b[p],
                            clip: be::u16(b, p + 2),
                            duration_ms: be::u32(b, p + 4) as i32,
                            blend_ms: be::u32(b, p + 8) as i32,
                        }
                    })
                    .collect();
                at += nk * 12;
                items.push(Item {
                    skip,
                    conditions,
                    commands,
                });
            }
            if k < nm {
                out.moves.push((id, items));
            } else {
                out.events.push((id, items));
            }
        }
        Some(out)
    }
}

/// The condition values one player has this frame (IW4's client anim conditions): which conditions are known, and
/// each one's value (a bit set for the bit-flag ones).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Conditions {
    pub written: u32,
    pub value: [u64; CONDITIONS],
}

impl Conditions {
    pub fn set_value(&mut self, cond: u8, v: u32) {
        if usize::from(cond) < CONDITIONS {
            self.written |= 1 << cond;
            self.value[usize::from(cond)] = u64::from(v);
        }
    }

    pub fn set_bit(&mut self, cond: u8, bit: u8) {
        if usize::from(cond) < CONDITIONS && bit < 64 {
            self.written |= 1 << cond;
            self.value[usize::from(cond)] |= 1 << bit;
        }
    }

    /// Whether every condition of `item` holds (IW4 `BG_EvaluateConditions`, as IW4L `sim::player_anim_script`).
    pub fn hold(&self, item: &Item) -> bool {
        !item.skip
            && item.conditions.iter().all(|c| {
                let i = usize::from(c.index);
                if i >= CONDITIONS {
                    return false;
                }
                if c.bitflags {
                    c.bits & self.value[i] != 0
                } else {
                    self.written & (1 << c.index) != 0 && self.value[i] as i32 == c.value
                }
            })
    }

    /// The first item of `items` whose conditions hold.
    pub fn first<'a>(&self, items: &'a [Item]) -> Option<&'a Item> {
        items.iter().find(|i| self.hold(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn script() -> Script {
        let cmd = |clip| Command {
            part: part::BOTH,
            clip,
            duration_ms: 0,
            blend_ms: NO_BLEND,
        };
        Script {
            moves: vec![(
                1,
                vec![
                    Item {
                        skip: false,
                        conditions: vec![Condition {
                            index: 0,
                            bitflags: true,
                            bits: 1 << 2,
                            value: 0,
                        }],
                        commands: vec![cmd(3)],
                    },
                    Item {
                        skip: false,
                        conditions: vec![Condition {
                            index: 12,
                            bitflags: false,
                            bits: 0,
                            value: 1,
                        }],
                        commands: vec![],
                    },
                    Item {
                        skip: false,
                        conditions: vec![],
                        commands: vec![cmd(0), cmd(1)],
                    },
                ],
            )],
            events: vec![(1, vec![Item::default()])],
        }
    }

    #[test]
    fn round_trips() {
        let s = script();
        assert_eq!(Script::decode(&s.encode()), Some(s.clone()));
        assert_eq!(Script::decode(&s.encode()[..10]), None);
        assert_eq!(s.clips().collect::<Vec<_>>(), [3, 0, 1]);
    }

    #[test]
    fn the_first_item_whose_conditions_hold() {
        let s = script();
        let items = s.move_items(1);
        let mut c = Conditions::default();
        // A pistol (anim type bit 2): the first item.
        c.set_bit(0, 2);
        assert_eq!(c.first(items).unwrap().commands[0].clip, 3);
        // Another type, akimbo unknown: the default.
        let mut c = Conditions::default();
        c.set_bit(0, 4);
        assert_eq!(c.first(items).unwrap().commands.len(), 2);
        // Akimbo: the empty item matches and plays nothing.
        c.set_value(12, 1);
        assert!(c.first(items).unwrap().commands.is_empty());
        assert!(s.move_items(7).is_empty());
    }
}

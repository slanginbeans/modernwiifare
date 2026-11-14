//! Which sound a weapon plays when: the player-view alias of each weapon event (IW4L `weapon_iw4::event_sound`
//! rules) and the notetrack sounds a first-person clip passes between two frames.

use weapon_iw4::WeaponTickEvent;
use wpk::sound::{NONE, Note, WeaponSounds, event};

/// The alias a weapon event plays, if any. `clip_after`: rounds in the magazine after the event.
/// - A shot plays the last-shot sound when it empties the magazine and the weapon has one (IW4 `fireLastSound`).
/// - A reload from empty plays the empty-reload sound, falling back to the reload sound (IW4L
///   `reload_insert_event`, weapon type 0 = bullet).
pub fn for_event(sounds: &WeaponSounds, ev: &WeaponTickEvent, clip_after: i32) -> Option<u16> {
    let pick = |e: usize| Some(sounds.events[e]).filter(|&a| a != NONE);
    match ev {
        WeaponTickEvent::ShotAccepted { .. } => (if clip_after <= 0 {
            pick(event::FIRE_LAST)
        } else {
            None
        })
        .or_else(|| pick(event::FIRE)),
        WeaponTickEvent::EmptyClick => pick(event::EMPTY_FIRE),
        WeaponTickEvent::ReloadStarted => (if clip_after <= 0 {
            pick(event::RELOAD_EMPTY)
        } else {
            None
        })
        .or_else(|| pick(event::RELOAD)),
        WeaponTickEvent::RaiseStarted => pick(event::RAISE),
        WeaponTickEvent::PutawayStarted => pick(event::PUTAWAY),
        _ => None,
    }
}

/// Notes of clip `slot` passed going from fraction `from` (exclusive; `None` when the clip just started, so notes
/// at 0 play) to `to` (inclusive). A looping clip that wrapped (`to < from`) plays the notes after `from` and up
/// to `to`.
pub fn notes_passed(
    notes: &[Note],
    slot: u8,
    from: Option<f32>,
    to: f32,
) -> impl Iterator<Item = u16> + '_ {
    notes
        .iter()
        .filter(move |n| n.slot == slot)
        .filter_map(move |n| {
            let t = n.time;
            let hit = match from {
                None => t <= to,
                Some(f) if to >= f => t > f && t <= to,
                Some(f) => t > f || t <= to,
            };
            hit.then_some(n.alias)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sounds() -> WeaponSounds {
        let mut events = [NONE; event::COUNT];
        events[event::FIRE] = 1;
        events[event::RELOAD] = 2;
        events[event::RELOAD_EMPTY] = 3;
        WeaponSounds {
            events,
            notes: vec![
                Note {
                    slot: 9,
                    time: 0.0,
                    alias: 10,
                },
                Note {
                    slot: 9,
                    time: 0.3,
                    alias: 11,
                },
                Note {
                    slot: 9,
                    time: 0.7,
                    alias: 12,
                },
                Note {
                    slot: 1,
                    time: 0.5,
                    alias: 13,
                },
            ],
        }
    }

    #[test]
    fn events_pick_their_alias() {
        let s = sounds();
        let shot = WeaponTickEvent::ShotAccepted { ammo_used: 1 };
        assert_eq!(for_event(&s, &shot, 5), Some(1));
        // No last-shot sound: the ordinary one.
        assert_eq!(for_event(&s, &shot, 0), Some(1));
        assert_eq!(for_event(&s, &WeaponTickEvent::ReloadStarted, 4), Some(2));
        assert_eq!(for_event(&s, &WeaponTickEvent::ReloadStarted, 0), Some(3));
        assert_eq!(for_event(&s, &WeaponTickEvent::EmptyClick, 0), None);
    }

    #[test]
    fn notes_play_once_as_the_clip_passes_them() {
        let n = sounds().notes;
        let got = |slot, from, to| notes_passed(&n, slot, from, to).collect::<alloc::vec::Vec<_>>();
        assert_eq!(got(9, None, 0.1), [10]);
        assert_eq!(got(9, Some(0.1), 0.3), [11]);
        assert_eq!(got(9, Some(0.3), 0.6), []);
        assert_eq!(got(9, Some(0.6), 1.0), [12]);
        // Wrapped loop of slot 1.
        assert_eq!(got(1, Some(0.9), 0.6), [13]);
        assert_eq!(got(1, Some(0.6), 0.9), []);
    }
}

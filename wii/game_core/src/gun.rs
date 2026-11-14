//! The player's gun: IW4L's weapon state machine (`weapon_iw4::weapon_ordinary`: raise, fire rate, bursts, reload,
//! rechamber) driven by the pad, with combat numbers from `common.wpk`, and bullets traced against the map's
//! collision (IW4 `MASK_SHOT`).
//!
//! Not yet: spread (bullets go exactly to the crosshair), recoil / view kick, penetration, damage to players, the
//! viewmodel and sounds. Timings are IW4L's and therefore the PC game's.

use playerstate_iw4::buttons;
use trace_iw4::Trace;
use weapon_iw4::{
    WeaponCmd, WeaponCombatFacts, WeaponHandState, WeaponTickEvent, spawn_weapon_hand,
    weapon_ordinary,
};
use wpk::weapon::WeaponRecord;

use crate::collision::MapClip;

/// IW4 `MASK_SHOT`.
pub const MASK_SHOT: u32 = 0x0280_6831;

/// What the gun did this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub start: [f32; 3],
    /// Where the bullet stopped (the impact, or the end of its range).
    pub end: [f32; 3],
    pub hit: bool,
    pub normal: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunInput {
    pub fire: bool,
    pub reload: bool,
    /// The player's `pm_flags` (sprinting blocks firing, as in IW4).
    pub pm_flags: u32,
}

pub struct Gun {
    pub name: [u8; wpk::weapon::NAME_LEN],
    facts: WeaponCombatFacts,
    hand: WeaponHandState,
    server_time: i32,
    old_buttons: u32,
    /// Shots fired since the gun was raised (for the HUD / tests).
    pub shots: u32,
}

impl Gun {
    /// A gun from its record, raised as at spawn (first raise). None when IW4L can't build its facts.
    pub fn new(rec: &WeaponRecord) -> Option<Gun> {
        Gun::with_index(rec, 1)
    }

    /// As [`Gun::new`], remembering the record's index in the weapon list (used to find its first-person model).
    pub fn with_index(rec: &WeaponRecord, index: u32) -> Option<Gun> {
        let facts = WeaponCombatFacts::try_from_captured(rec.combat).ok()?;
        if !facts.is_usable() {
            return None;
        }
        Some(Gun {
            name: rec.name,
            hand: spawn_weapon_hand(index, &facts, true),
            facts,
            server_time: 1000,
            old_buttons: 0,
            shots: 0,
        })
    }

    pub fn name(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    /// Rounds in the magazine and in reserve.
    pub fn ammo(&self) -> (i32, i32) {
        (self.hand.clip, self.hand.stock)
    }

    /// The first-person animation the weapon state machine wants: (IW4 anim slot, restart toggle). The toggle flips
    /// each time an animation is (re)started, so a repeated fire restarts the clip. Idle is the empty idle when the
    /// magazine is empty.
    pub fn anim(&self) -> (u8, bool) {
        let raw = self.hand.weap_anim as u32;
        let restart = raw & weapon_iw4::WEAP_ANIM_RESTART_BIT != 0;
        let slot = match weapon_iw4::slot_for_weap_anim_event(raw) {
            Some(s) => s as u8,
            None if self.hand.clip == 0 => wpk::view::slot::EMPTY_IDLE,
            None => wpk::view::slot::IDLE,
        };
        (slot, restart)
    }

    /// Index of this weapon in `common.wpk`'s weapon list (set by whoever built the gun).
    pub fn weapon_index(&self) -> u32 {
        self.hand.weapon
    }

    /// IW4 weapon state (0 ready, 1 raising, 5 firing, 8.. reloading; see `weapon_iw4::WeaponState`).
    pub fn state(&self) -> i32 {
        self.hand.weaponstate
    }

    /// Run the weapon for `msec`; a fired bullet is traced from `eye` along `dir` (unit vector).
    pub fn update(
        &mut self,
        clip: &MapClip<'_>,
        msec: i32,
        input: &GunInput,
        eye: [f32; 3],
        dir: [f32; 3],
    ) -> (Option<WeaponTickEvent>, Option<Shot>) {
        let mut btn = 0;
        if input.fire {
            btn |= buttons::ATTACK;
        }
        if input.reload {
            btn |= buttons::RELOAD;
        }
        self.server_time += msec;
        let mut cmd = WeaponCmd {
            msec,
            server_time: self.server_time,
            buttons: btn,
            old_buttons: self.old_buttons,
            cmd_weapon: self.hand.weapon as u16,
            cmd_weapon_owned: true,
            pm_flags: input.pm_flags,
            ..WeaponCmd::default()
        };
        self.old_buttons = btn;
        let event = weapon_ordinary(&mut self.hand, &self.facts, &mut cmd);
        let shot = match event {
            Some(WeaponTickEvent::ShotAccepted { .. }) => {
                self.shots += 1;
                Some(self.trace_bullet(clip, eye, dir))
            }
            _ => None,
        };
        (event, shot)
    }

    fn trace_bullet(&self, clip: &MapClip<'_>, eye: [f32; 3], dir: [f32; 3]) -> Shot {
        let range = self.facts.bullet_range();
        let end = [
            eye[0] + dir[0] * range,
            eye[1] + dir[1] * range,
            eye[2] + dir[2] * range,
        ];
        let t: Trace = clip.trace(eye, end, [0.0; 3], [0.0; 3], MASK_SHOT);
        let f = t.fraction.clamp(0.0, 1.0);
        Shot {
            start: eye,
            end: [
                eye[0] + (end[0] - eye[0]) * f,
                eye[1] + (end[1] - eye[1]) * f,
                eye[2] + (end[2] - eye[2]) * f,
            ],
            hit: t.fraction < 1.0,
            normal: t.normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use weapon_iw4::{CapturedCombatInput, EmptyReloadPolicy};
    use wpk::clip::Clip;

    /// A full-auto rifle: 600 rpm (100 ms), 30-round magazine, 2 s reload.
    fn rifle() -> WeaponRecord {
        WeaponRecord::new(
            "test_rifle_mp",
            CapturedCombatInput {
                fire_time_ms: 100,
                raise_time_ms: 500,
                first_raise_time_ms: 500,
                drop_time_ms: 300,
                reload_time_ms: 2000,
                reload_empty_time_ms: 2500,
                empty_reload: EmptyReloadPolicy::Authored,
                clip_size: 30,
                start_ammo: 120,
                max_ammo: 210,
                fire_type: 0,
                weap_type: 0,
                weap_class: 0,
                shots_per_fire: 1,
                damage: 30,
                min_damage: 20,
                max_damage_range: 1000.0,
                min_damage_range: 2000.0,
                ..CapturedCombatInput::default()
            },
        )
    }

    fn wall() -> Clip {
        // A wall at x in [500, 520].
        let (min, max) = ([500.0, -1000.0, -1000.0], [520.0, 1000.0, 1000.0]);
        let mut c = Clip::default();
        c.push_brush(
            &[
                [1.0, 0.0, 0.0, max[0]],
                [-1.0, 0.0, 0.0, -min[0]],
                [0.0, 1.0, 0.0, max[1]],
                [0.0, -1.0, 0.0, -min[1]],
                [0.0, 0.0, 1.0, max[2]],
                [0.0, 0.0, -1.0, -min[2]],
            ],
            &[0; 6],
            1,
            0,
        );
        c
    }

    /// The M4A1 from a locally converted `out/packs/common.wpk` (never in git; skipped when absent).
    #[test]
    fn real_m4_fires_at_its_authored_rate() {
        extern crate std;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../out/packs/common.wpk");
        let Ok(bytes) = std::fs::read(path) else {
            std::println!("no local common pack, skipped");
            return;
        };
        let pack = wpk::Pack::parse(&bytes, true).unwrap();
        let weap = pack.section(wpk::tag::WEAPONS).unwrap().data;
        let rec = wpk::weapon::find(weap, "m4_mp").expect("m4_mp in common.wpk");
        let c = rec.combat;
        std::println!(
            "m4_mp: fire {} ms, clip {}, start ammo {}, reload {} / {} ms, damage {}-{}, raise {} ms",
            c.fire_time_ms,
            c.clip_size,
            c.start_ammo,
            c.reload_time_ms,
            c.reload_empty_time_ms,
            c.min_damage,
            c.damage,
            c.first_raise_time_ms
        );
        let w = wall();
        let clip = MapClip::new(&w);
        let mut g = Gun::new(&rec).expect("usable");
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        for _ in 0..200 {
            g.update(&clip, 16, &GunInput::default(), [0.0; 3], [1.0, 0.0, 0.0]);
        }
        let mut shots = 0;
        for _ in 0..125 {
            shots += usize::from(
                g.update(&clip, 16, &fire, [0.0; 3], [1.0, 0.0, 0.0])
                    .1
                    .is_some(),
            );
        }
        // IW engines restart the weapon timer each shot (no carry-over), so the interval rounds up to whole frames:
        // 70 ms at 16 ms frames fires every 80 ms (frame-rate dependent, like the PC and 360 games).
        let frames_per_shot = (c.fire_time_ms.max(1) as usize).div_ceil(16);
        let expected = 125 / frames_per_shot;
        std::println!("{shots} shots in 2 s ({expected} expected at 16 ms frames)");
        assert!(shots.abs_diff(expected.min(c.clip_size as usize)) <= 1);
    }

    #[test]
    fn raises_fires_at_its_rate_hits_the_wall_and_reloads() {
        let c = wall();
        let clip = MapClip::new(&c);
        let mut g = Gun::new(&rifle()).expect("usable");
        let fire = GunInput {
            fire: true,
            ..GunInput::default()
        };
        let eye = [0.0, 0.0, 60.0];
        let x = [1.0, 0.0, 0.0];
        // Raising: no shots for the first 500 ms.
        for _ in 0..30 {
            let (_, s) = g.update(&clip, 16, &GunInput::default(), eye, x);
            assert!(s.is_none());
        }
        // One second of fire at 100 ms per round: about 10 shots, all into the wall at x = 500.
        let mut hits = 0;
        for _ in 0..63 {
            if let (_, Some(s)) = g.update(&clip, 16, &fire, eye, x) {
                assert!(s.hit && (s.end[0] - 500.0).abs() < 1.0, "{s:?}");
                hits += 1;
            }
        }
        assert!((9..=11).contains(&hits), "{hits} shots in a second");
        assert_eq!(g.ammo().0, 30 - hits);
        // Reload: the magazine refills from the reserve after the reload time.
        let (clip_before, stock_before) = g.ammo();
        g.update(
            &clip,
            16,
            &GunInput {
                reload: true,
                ..GunInput::default()
            },
            eye,
            x,
        );
        for _ in 0..200 {
            g.update(&clip, 16, &GunInput::default(), eye, x);
        }
        let (clip_after, stock_after) = g.ammo();
        assert_eq!(clip_after, 30);
        assert_eq!(stock_after, stock_before - (30 - clip_before));
    }
}

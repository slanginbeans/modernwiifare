//! Pads to game actions: the GameCube controller (the main target, ADR 0005) and the Xbox 360 wired pad over USB, each
//! with MW2's console button layouts (Options > Controls > BUTTON LAYOUT: Default, Tactical).
//!
//! The Xbox 360 pad gets MW2's own 360 layouts:
//!
//! | Button | Default | Tactical |
//! |---|---|---|
//! | A | jump / stand | jump / stand |
//! | B | crouch, hold for prone | melee |
//! | X | reload, hold to use | reload, hold to use |
//! | Y | switch weapon | switch weapon |
//! | LB | tactical grenade | tactical grenade |
//! | RB | lethal grenade | lethal grenade |
//! | LT | aim down sights | aim down sights |
//! | RT | fire | fire |
//! | left stick click | sprint / hold breath | sprint / hold breath |
//! | right stick click | melee | crouch, hold for prone |
//! | D-pad right | killstreak reward (action slot 4) | the same |
//! | START | pause | pause |
//!
//! The GameCube pad has fewer buttons, so it keeps this project's mapping: A jump, B stance, X reload / use, Y switch,
//! Z sprint / hold breath, L aim, R fire, D-pad up / down lethal / tactical, D-pad right melee, D-pad left killstreak; Tactical swaps
//! B and D-pad right. Developer combinations: Z (GameCube) or BACK (360) held with D-pad up toggles free fly, with
//! D-pad down the performance overlay.

/// Game actions (bits of [`Controls::held`] / [`Controls::down`]).
pub mod act {
    pub const JUMP: u32 = 1 << 0;
    pub const STANCE: u32 = 1 << 1;
    pub const SPRINT: u32 = 1 << 2;
    pub const ADS: u32 = 1 << 3;
    pub const FIRE: u32 = 1 << 4;
    /// Reload; held: use (pick up), double tap: detonate C4.
    pub const USE: u32 = 1 << 5;
    pub const SWITCH: u32 = 1 << 6;
    pub const MELEE: u32 = 1 << 7;
    pub const FRAG: u32 = 1 << 8;
    pub const TACTICAL: u32 = 1 << 9;
    pub const KILLSTREAK: u32 = 1 << 10;
    pub const PAUSE: u32 = 1 << 11;
    /// Developer: free fly on / off.
    pub const FLY: u32 = 1 << 12;
    /// Developer: performance overlay on / off.
    pub const OVERLAY: u32 = 1 << 13;
    /// Free fly: move faster (down / up are the analog triggers, [`super::Controls::triggers`]).
    pub const FLY_FAST: u32 = 1 << 14;
    /// Hold breath (a scope steadied while fully aimed). MW2's console binds `+breath_sprint`: one button sprints
    /// and holds breath (IW4L `input_iw4` command 9 / 10), so BREATH is held with SPRINT in every layout.
    pub const BREATH: u32 = 1 << 15;
}

/// GameCube button bits (the shim's `MWF_BTN_*`).
pub mod gc {
    pub const LEFT: u16 = 0x0001;
    pub const RIGHT: u16 = 0x0002;
    pub const DOWN: u16 = 0x0004;
    pub const UP: u16 = 0x0008;
    pub const Z: u16 = 0x0010;
    pub const R: u16 = 0x0020;
    pub const L: u16 = 0x0040;
    pub const A: u16 = 0x0100;
    pub const B: u16 = 0x0200;
    pub const X: u16 = 0x0400;
    pub const Y: u16 = 0x0800;
    pub const START: u16 = 0x1000;
}

/// Xbox 360 button bits (the shim's `MWF_X360_*`: the USB report's bytes 2 and 3).
pub mod x360 {
    pub const UP: u16 = 0x0001;
    pub const DOWN: u16 = 0x0002;
    pub const LEFT: u16 = 0x0004;
    pub const RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LS: u16 = 0x0040;
    pub const RS: u16 = 0x0080;
    pub const LB: u16 = 0x0100;
    pub const RB: u16 = 0x0200;
    pub const GUIDE: u16 = 0x0400;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// A GameCube controller this frame (`held` / `down` as the shim reports them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcPad {
    pub held: u16,
    pub down: u16,
    pub sx: i8,
    pub sy: i8,
    pub cx: i8,
    pub cy: i8,
    pub l: u8,
    pub r: u8,
    pub connected: bool,
}

/// An Xbox 360 pad this frame (held buttons only; [`Mapper`] finds the presses).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct X360Pad {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub connected: bool,
}

/// How far a 360 trigger counts as pulled (XInput's `XINPUT_GAMEPAD_TRIGGER_THRESHOLD`, 30 of 255).
pub const X360_TRIGGER: u8 = 30;

/// The actions this frame and the sticks, raw (the look code learns their range and applies the dead zone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub held: u32,
    /// Actions that started this frame.
    pub down: u32,
    /// Left (move) and right (look) sticks, -127..127, up and right positive.
    pub stick_l: (i8, i8),
    pub stick_r: (i8, i8),
    /// Analog triggers 0..255 (left, right), for the calibration log and free fly.
    pub triggers: (u8, u8),
    /// A 360 pad gave input this frame.
    pub from_x360: bool,
}

impl Controls {
    pub fn held(&self, a: u32) -> bool {
        self.held & a != 0
    }

    pub fn pressed(&self, a: u32) -> bool {
        self.down & a != 0
    }
}

/// The GameCube pad's actions: (held, started this frame).
pub fn gc_actions(p: &GcPad, tactical: bool) -> (u32, u32) {
    let (stance, melee) = if tactical {
        (gc::RIGHT, gc::B)
    } else {
        (gc::B, gc::RIGHT)
    };
    let z = p.held & gc::Z != 0;
    let on = |b: u16, a: u32| if p.held & b != 0 { a } else { 0 };
    let mut held = on(gc::A, act::JUMP)
        | on(stance, act::STANCE)
        | on(gc::Z, act::SPRINT | act::BREATH)
        | on(gc::X, act::USE)
        | on(gc::Y, act::SWITCH)
        | on(melee, act::MELEE)
        | on(gc::LEFT, act::KILLSTREAK)
        | on(gc::START, act::PAUSE)
        | on(gc::B, act::FLY_FAST);
    if p.held & gc::L != 0 || p.l > 128 {
        held |= act::ADS;
    }
    if p.held & gc::R != 0 || p.r > 100 {
        held |= act::FIRE;
    }
    if !z {
        held |= on(gc::UP, act::FRAG) | on(gc::DOWN, act::TACTICAL);
    }
    let pressed = |b: u16, a: u32| if p.down & b != 0 { a } else { 0 };
    let mut down = pressed(gc::A, act::JUMP)
        | pressed(stance, act::STANCE)
        | pressed(gc::X, act::USE)
        | pressed(gc::Y, act::SWITCH)
        | pressed(melee, act::MELEE)
        | pressed(gc::LEFT, act::KILLSTREAK)
        | pressed(gc::START, act::PAUSE);
    if z {
        down |= pressed(gc::UP, act::FLY) | pressed(gc::DOWN, act::OVERLAY);
    } else {
        down |= pressed(gc::UP, act::FRAG) | pressed(gc::DOWN, act::TACTICAL);
    }
    (held, down)
}

/// The 360 pad's actions with MW2's console layout: (held, started this frame); `prev` is last frame's buttons.
pub fn x360_actions(p: &X360Pad, prev: u16, tactical: bool) -> (u32, u32) {
    use x360::*;
    let (stance, melee) = if tactical { (RS, B) } else { (B, RS) };
    let back = p.buttons & BACK != 0;
    let map = |bits: u16| -> u32 {
        let on = |b: u16, a: u32| if bits & b != 0 { a } else { 0 };
        let mut a = on(A, act::JUMP)
            | on(stance, act::STANCE)
            | on(LS, act::SPRINT | act::BREATH)
            | on(X, act::USE)
            | on(Y, act::SWITCH)
            | on(melee, act::MELEE)
            | on(RB, act::FRAG)
            | on(LB, act::TACTICAL)
            | on(START, act::PAUSE)
            | on(B, act::FLY_FAST);
        if back {
            a |= on(UP, act::FLY) | on(DOWN, act::OVERLAY);
        } else {
            a |= on(RIGHT, act::KILLSTREAK);
        }
        a
    };
    let mut held = map(p.buttons);
    // FLY / OVERLAY are presses only (a combination held over frames is not an action).
    held &= !(act::FLY | act::OVERLAY);
    if p.lt > X360_TRIGGER {
        held |= act::ADS;
    }
    if p.rt > X360_TRIGGER {
        held |= act::FIRE;
    }
    let down = map(p.buttons & !prev);
    (held, down)
}

/// A 360 stick axis (-32768..32767) as the GameCube-sized -127..127 the look code takes.
pub fn x360_axis(v: i16) -> i8 {
    (i32::from(v) >> 8).clamp(-127, 127) as i8
}

/// Merges the pads into one set of actions each frame (either pad plays; both at once add up).
#[derive(Clone, Copy, Debug, Default)]
pub struct Mapper {
    prev_x360: u16,
}

impl Mapper {
    pub fn map(&mut self, gc: &GcPad, x: &X360Pad, tactical: bool) -> Controls {
        let mut c = Controls::default();
        let deflection = |s: (i8, i8)| i32::from(s.0).pow(2) + i32::from(s.1).pow(2);
        if gc.connected {
            let (held, down) = gc_actions(gc, tactical);
            c.held |= held;
            c.down |= down;
            c.stick_l = (gc.sx, gc.sy);
            c.stick_r = (gc.cx, gc.cy);
            c.triggers = (gc.l, gc.r);
        }
        if x.connected {
            let (held, down) = x360_actions(x, self.prev_x360, tactical);
            c.held |= held;
            c.down |= down;
            let (l, r) = (
                (x360_axis(x.lx), x360_axis(x.ly)),
                (x360_axis(x.rx), x360_axis(x.ry)),
            );
            // Whichever pad's stick is pushed further.
            if deflection(l) > deflection(c.stick_l) {
                c.stick_l = l;
            }
            if deflection(r) > deflection(c.stick_r) {
                c.stick_r = r;
            }
            c.triggers = (c.triggers.0.max(x.lt), c.triggers.1.max(x.rt));
            c.from_x360 = held != 0 || l != (0, 0) || r != (0, 0);
            self.prev_x360 = x.buttons;
        } else {
            self.prev_x360 = 0;
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(buttons: u16) -> X360Pad {
        X360Pad {
            buttons,
            connected: true,
            ..X360Pad::default()
        }
    }

    #[test]
    fn x360_default_and_tactical_match_mw2() {
        let mut m = Mapper::default();
        let none = GcPad::default();
        let c = m.map(&none, &x(x360::B), false);
        assert!(c.held(act::STANCE) && c.pressed(act::STANCE) && !c.held(act::MELEE));
        let c = m.map(&none, &x(x360::B), false);
        // Still held, no new press.
        assert!(c.held(act::STANCE) && !c.pressed(act::STANCE));
        let c = m.map(&none, &x(x360::RS), false);
        assert!(c.held(act::MELEE) && c.pressed(act::MELEE));
        // Tactical swaps them.
        let c = m.map(&none, &x(x360::RS), true);
        assert!(c.held(act::STANCE) && !c.held(act::MELEE));
        let c = m.map(&none, &x(x360::B), true);
        assert!(c.held(act::MELEE) && c.pressed(act::MELEE));
        // The rest of the layout.
        for (b, a) in [
            (x360::A, act::JUMP),
            (x360::X, act::USE),
            (x360::Y, act::SWITCH),
            (x360::LS, act::SPRINT),
            (x360::LB, act::TACTICAL),
            (x360::RB, act::FRAG),
            (x360::RIGHT, act::KILLSTREAK),
            (x360::START, act::PAUSE),
        ] {
            m.map(&none, &x(0), false);
            let c = m.map(&none, &x(b), false);
            assert!(c.held(a) && c.pressed(a), "{b:#06x}");
        }
        let c = m.map(
            &none,
            &X360Pad {
                lt: 200,
                rt: 31,
                ..x(0)
            },
            false,
        );
        assert!(c.held(act::ADS) && c.held(act::FIRE));
        let c = m.map(&none, &X360Pad { rt: 30, ..x(0) }, false);
        assert!(!c.held(act::FIRE));
        // Sprinting while throwing a grenade throws it (no developer combination on the 360's game buttons).
        let c = m.map(&none, &x(x360::LS | x360::RB), false);
        assert!(c.held(act::SPRINT) && c.pressed(act::FRAG) && !c.pressed(act::FLY));
        // BACK + D-pad: the developer toggles, presses only.
        m.map(&none, &x(x360::BACK), false);
        let c = m.map(&none, &x(x360::BACK | x360::UP), false);
        assert!(c.pressed(act::FLY) && !c.held(act::FLY));
        let c = m.map(&none, &x(x360::BACK | x360::RIGHT), false);
        assert!(!c.held(act::KILLSTREAK));
    }

    #[test]
    fn gamecube_mapping_is_unchanged() {
        let p = |held: u16, down: u16| GcPad {
            held,
            down,
            connected: true,
            ..GcPad::default()
        };
        let (h, d) = gc_actions(&p(gc::B, gc::B), false);
        assert!(h & act::STANCE != 0 && d & act::STANCE != 0 && h & act::MELEE == 0);
        let (h, _) = gc_actions(&p(gc::RIGHT, 0), false);
        assert!(h & act::MELEE != 0);
        let (h, _) = gc_actions(&p(gc::RIGHT, 0), true);
        assert!(h & act::STANCE != 0 && h & act::MELEE == 0);
        // D-pad up throws, Z + D-pad up flies instead.
        let (h, d) = gc_actions(&p(gc::UP, gc::UP), false);
        assert!(h & act::FRAG != 0 && d & act::FRAG != 0);
        let (h, d) = gc_actions(&p(gc::UP | gc::Z, gc::UP), false);
        assert!(h & act::FRAG == 0 && d & act::FLY != 0 && h & act::SPRINT != 0);
        let (_, d) = gc_actions(&p(gc::DOWN | gc::Z, gc::DOWN), false);
        assert!(d & act::OVERLAY != 0);
        // Analog triggers: L past 128 aims, R past 100 fires.
        let (h, _) = gc_actions(
            &GcPad {
                l: 129,
                r: 101,
                ..p(0, 0)
            },
            false,
        );
        assert!(h & act::ADS != 0 && h & act::FIRE != 0);
        let (h, _) = gc_actions(&p(gc::LEFT, 0), false);
        assert!(h & act::KILLSTREAK != 0);
    }

    #[test]
    fn pads_merge_and_sticks_take_the_larger_push() {
        let mut m = Mapper::default();
        let g = GcPad {
            held: gc::A,
            sx: 20,
            cx: -100,
            connected: true,
            ..GcPad::default()
        };
        let c = m.map(
            &g,
            &X360Pad {
                buttons: x360::X,
                lx: 32767,
                rx: 1000,
                ..x(0)
            },
            false,
        );
        assert!(c.held(act::JUMP) && c.held(act::USE) && c.from_x360);
        assert_eq!(c.stick_l, (127, 0));
        assert_eq!(c.stick_r, (-100, 0));
        assert_eq!(x360_axis(-32768), -127);
        // A 360 pad unplugged: its buttons stop, no stale press when it comes back.
        let c = m.map(&g, &X360Pad::default(), false);
        assert!(!c.held(act::USE) && !c.from_x360);
        let c = m.map(&g, &x(x360::X), false);
        assert!(c.pressed(act::USE));
    }

    #[test]
    fn the_sprint_button_holds_breath_in_every_layout() {
        // MW2's console `+breath_sprint`: one button for both (GameCube Z, 360 left stick click).
        for tactical in [false, true] {
            let (held, _) = gc_actions(
                &GcPad {
                    held: gc::Z,
                    connected: true,
                    ..GcPad::default()
                },
                tactical,
            );
            assert_eq!(
                held & (act::SPRINT | act::BREATH),
                act::SPRINT | act::BREATH
            );
            let (held, _) = x360_actions(&x(x360::LS), 0, tactical);
            assert_eq!(
                held & (act::SPRINT | act::BREATH),
                act::SPRINT | act::BREATH
            );
            // Nothing else holds breath.
            let (held, _) = x360_actions(&x(!x360::LS), 0, tactical);
            assert_eq!(held & act::BREATH, 0);
        }
    }
}

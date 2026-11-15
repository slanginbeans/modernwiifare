//! Menu lists driven by the pad: one selection moving through lines, with the window of lines that fits on screen.
//! Holding a direction repeats it (as MW2's menus do), and lines with a value change it with left / right.

use core::ops::Range;

/// One frame of menu input (edge-triggered, with the held direction's repeats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub accept: bool,
    pub back: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing chosen yet.
    Stay,
    Accept(usize),
    /// Left (-1) or right (+1) on a line: change its value.
    Adjust(usize, i8),
    Back,
}

/// Move `selected` through `len` lines (wrapping) and report a choice.
pub fn step(selected: &mut usize, len: usize, nav: Nav) -> Step {
    if nav.back {
        return Step::Back;
    }
    if len == 0 {
        return Step::Stay;
    }
    *selected = (*selected).min(len - 1);
    if nav.up {
        *selected = (*selected + len - 1) % len;
    }
    if nav.down {
        *selected = (*selected + 1) % len;
    }
    if nav.accept {
        return Step::Accept(*selected);
    }
    if nav.left != nav.right {
        return Step::Adjust(*selected, if nav.right { 1 } else { -1 });
    }
    Step::Stay
}

/// The lines to show, `rows` at most, keeping `selected` in view (scrolled so it sits on the last row when below).
pub fn window(selected: usize, len: usize, rows: usize) -> Range<usize> {
    let first = selected
        .saturating_sub(rows.saturating_sub(1))
        .min(len.saturating_sub(rows));
    first..(first + rows).min(len)
}

/// Repeats a held direction: once when pushed, again after [`Repeat::DELAY_MS`], then every
/// [`Repeat::RATE_MS`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Repeat {
    dir: i8,
    next_ms: u32,
}

impl Repeat {
    pub const DELAY_MS: u32 = 400;
    pub const RATE_MS: u32 = 90;

    /// `dir`: the direction held now (-1, 0, 1). True when it fires this frame.
    pub fn update(&mut self, dir: i8, now_ms: u32) -> bool {
        if dir == 0 {
            self.dir = 0;
            return false;
        }
        if dir != self.dir {
            self.dir = dir;
            self.next_ms = now_ms.wrapping_add(Self::DELAY_MS);
            return true;
        }
        if now_ms.wrapping_sub(self.next_ms) < u32::MAX / 2 {
            self.next_ms = now_ms.wrapping_add(Self::RATE_MS);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_accepts() {
        let mut s = 0;
        let up = Nav {
            up: true,
            ..Nav::default()
        };
        assert_eq!(step(&mut s, 3, up), Step::Stay);
        assert_eq!(s, 2);
        let down = Nav {
            down: true,
            ..Nav::default()
        };
        step(&mut s, 3, down);
        assert_eq!(s, 0);
        assert_eq!(
            step(
                &mut s,
                3,
                Nav {
                    accept: true,
                    ..Nav::default()
                }
            ),
            Step::Accept(0)
        );
        assert_eq!(
            step(
                &mut s,
                3,
                Nav {
                    left: true,
                    ..Nav::default()
                }
            ),
            Step::Adjust(0, -1)
        );
        assert_eq!(
            step(
                &mut s,
                0,
                Nav {
                    back: true,
                    ..Nav::default()
                }
            ),
            Step::Back
        );
    }

    #[test]
    fn window_follows_the_selection() {
        assert_eq!(window(0, 30, 10), 0..10);
        assert_eq!(window(12, 30, 10), 3..13);
        assert_eq!(window(29, 30, 10), 20..30);
        assert_eq!(window(2, 4, 10), 0..4);
    }

    #[test]
    fn held_directions_repeat_after_a_delay() {
        let mut r = Repeat::default();
        assert!(r.update(1, 1000));
        assert!(!r.update(1, 1000 + Repeat::DELAY_MS - 1));
        assert!(r.update(1, 1000 + Repeat::DELAY_MS));
        assert!(!r.update(1, 1000 + Repeat::DELAY_MS + 10));
        assert!(r.update(1, 1000 + Repeat::DELAY_MS + Repeat::RATE_MS));
        assert!(!r.update(0, 2000));
        assert!(r.update(-1, 2001), "a new push fires at once");
    }
}

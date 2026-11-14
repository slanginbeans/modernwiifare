//! Menu lists driven by the pad: one selection moving through lines, with the window of lines that fits on screen.

use core::ops::Range;

/// One frame of menu input (edge-triggered).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub accept: bool,
    pub back: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing chosen yet.
    Stay,
    Accept(usize),
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
    Step::Stay
}

/// The lines to show, `rows` at most, keeping `selected` in view (scrolled so it sits on the last row when below).
pub fn window(selected: usize, len: usize, rows: usize) -> Range<usize> {
    let first = selected
        .saturating_sub(rows.saturating_sub(1))
        .min(len.saturating_sub(rows));
    first..(first + rows).min(len)
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
}

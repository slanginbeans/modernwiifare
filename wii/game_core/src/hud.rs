//! Screen-space HUD built from solid rectangles: a 3×5 pixel font for the debug overlay, and later the crosshair and
//! counters. Coordinates are 640×480 screen pixels, origin top left. The GPU side only draws rectangles.

use alloc::vec::Vec;

/// A rectangle `[x, y, w, h]` in screen pixels.
pub type Rect = [i16; 4];

/// 3×5 glyphs, rows top to bottom, 3 bits each (bit 2 = left column), 15 bits per glyph.
fn glyph(c: char) -> u16 {
    let rows: [u8; 5] = match c.to_ascii_uppercase() {
        '0' | 'O' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' | 'S' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [7, 4, 4, 4, 7],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [7, 4, 5, 5, 7],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 7],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'P' => [7, 5, 7, 4, 4],
        'Q' => [7, 5, 5, 7, 1],
        'R' => [6, 5, 6, 5, 5],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '.' => [0, 0, 0, 0, 2],
        ':' => [0, 2, 0, 2, 0],
        '/' => [1, 1, 2, 4, 4],
        '-' => [0, 0, 7, 0, 0],
        '+' => [0, 2, 7, 2, 0],
        '%' => [5, 1, 2, 4, 5],
        '_' => [0, 0, 0, 0, 7],
        '>' => [4, 2, 1, 2, 4],
        '<' => [1, 2, 4, 2, 1],
        '!' => [2, 2, 2, 0, 2],
        '?' => [7, 1, 2, 0, 2],
        '=' => [0, 7, 0, 7, 0],
        '(' => [1, 2, 2, 2, 1],
        ')' => [4, 2, 2, 2, 4],
        _ => [0; 5],
    };
    rows.iter().fold(0, |g, &r| (g << 3) | u16::from(r))
}

/// Append the rectangles that draw `text` at (`x`, `y`) with `scale`-pixel font pixels. Each lit run in a glyph row
/// is one rectangle. `\n` starts a new line. Returns the width of the longest line in pixels.
pub fn text(out: &mut Vec<Rect>, x: i16, y: i16, scale: i16, text: &str) -> i16 {
    let (mut cx, mut cy, mut widest) = (x, y, 0);
    for c in text.chars() {
        if c == '\n' {
            cy += 6 * scale;
            cx = x;
            continue;
        }
        let g = glyph(c);
        for row in 0..5 {
            let bits = (g >> (3 * (4 - row))) & 7;
            let mut col = 0;
            while col < 3 {
                if bits & (4 >> col) != 0 {
                    let start = col;
                    while col < 3 && bits & (4 >> col) != 0 {
                        col += 1;
                    }
                    out.push([
                        cx + start * scale,
                        cy + row * scale,
                        (col - start) * scale,
                        scale,
                    ]);
                } else {
                    col += 1;
                }
            }
        }
        cx += 4 * scale;
        widest = widest.max(cx - x);
    }
    widest
}

/// Format `v` with one decimal into `buf` (no_std, no float formatting machinery).
pub fn fixed1(buf: &mut heapless_str::Str, v: f32) {
    let t = (v * 10.0 + 0.5).max(0.0) as u32;
    buf.num(t / 10);
    buf.push('.');
    buf.num(t % 10);
}

/// A small fixed-capacity string for building overlay lines without allocation.
pub mod heapless_str {
    pub struct Str {
        buf: [u8; 256],
        len: usize,
    }

    impl Default for Str {
        fn default() -> Self {
            Str {
                buf: [0; 256],
                len: 0,
            }
        }
    }

    impl Str {
        pub fn push(&mut self, c: char) {
            if c.is_ascii() && self.len < self.buf.len() {
                self.buf[self.len] = c as u8;
                self.len += 1;
            }
        }
        pub fn push_str(&mut self, s: &str) {
            s.chars().for_each(|c| self.push(c));
        }
        pub fn num(&mut self, mut v: u32) {
            let mut digits = [0u8; 10];
            let mut n = 0;
            loop {
                digits[n] = b'0' + (v % 10) as u8;
                n += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            for i in (0..n).rev() {
                self.push(digits[i] as char);
            }
        }
        pub fn as_str(&self) -> &str {
            // Only ASCII is ever pushed.
            core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
        }
        pub fn clear(&mut self) {
            self.len = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn glyph_runs_become_rectangles() {
        let mut r = Vec::new();
        // '1': .X. / XX. / .X. / .X. / XXX -> one rect per run.
        let w = text(&mut r, 10, 20, 2, "1");
        assert_eq!(w, 8);
        assert_eq!(
            r,
            vec![
                [12, 20, 2, 2],
                [10, 22, 4, 2],
                [12, 24, 2, 2],
                [12, 26, 2, 2],
                [10, 28, 6, 2]
            ]
        );
        r.clear();
        text(&mut r, 0, 0, 1, "A\nB");
        assert!(r.iter().any(|q| q[1] >= 6), "second line below the first");
    }

    #[test]
    fn number_formatting() {
        let mut s = heapless_str::Str::default();
        s.push_str("FPS ");
        s.num(60);
        s.push(' ');
        fixed1(&mut s, 16.66);
        assert_eq!(s.as_str(), "FPS 60 16.7");
    }
}

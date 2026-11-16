//! Scripted pad input for testing on Dolphin or a Wii: `<app folder>/autoplay.txt`, when it is there, drives the
//! GameCube pad (port 1) one frame at a time and asks for screenshots. One command a line, `#` starts a comment:
//!
//! - `wait N`: N frames with the pad as it is.
//! - `press A+UP [N]`: hold the buttons N frames (default 2), then let go for 4.
//! - `hold A+UP`, `release A+UP`: change what is held.
//! - `stick X Y`, `cstick X Y`, `triggers L R`: the sticks (-128..127) and triggers (0..255), until changed.
//! - `times N <command>`: the command N times.
//! - `shot NAME`: a screenshot of the frame drawn next, to `<app folder>/shots/NAME.bmp`.
//! - `log TEXT`: a line in the match log.
//! - `warp X Y Z YAW`: in a match, put the player at map position X Y Z (dropped onto the floor below) facing YAW
//!   degrees, to test a spot (a mantle, a drop).
//! - `pace N`: from now on a new frame every N retraces (1: as fast as frames are ready, the default for scripts
//!   whatever the options say, since a script counts frames; 2: the steady 30 fps of the FRAME RATE option).
//! - `quit`: switch the console off (Dolphin stops and writes the SD folder back).
//!
//! Buttons: A B X Y Z L R START UP DOWN LEFT RIGHT. Frames are pad reads: one a frame in the menus and the match,
//! none while a map loads.

use alloc::string::String;
use alloc::vec::Vec;

use crate::controls::gc;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cmd {
    Wait(u32),
    Hold(u16),
    Release(u16),
    Stick(i8, i8),
    CStick(i8, i8),
    Triggers(u8, u8),
    Shot(String),
    Log(String),
    Warp([i32; 4]),
    Pace(u8),
    Quit,
}

/// The pad and requests for one frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub held: u16,
    pub sx: i8,
    pub sy: i8,
    pub cx: i8,
    pub cy: i8,
    pub l: u8,
    pub r: u8,
    /// Take a screenshot after this frame is drawn.
    pub shot: Option<String>,
    pub log: Option<String>,
    /// Put the player at (x, y, z) facing yaw degrees.
    pub warp: Option<[i32; 4]>,
    /// Frame pacing from now on (retraces per frame).
    pub pace: Option<u8>,
    pub quit: bool,
}

#[derive(Default)]
pub struct Script {
    cmds: Vec<Cmd>,
    pc: usize,
    wait: u32,
    pad: Frame,
}

fn buttons(s: &str) -> Result<u16, String> {
    let mut b = 0;
    for name in s.split('+') {
        b |= match name.to_ascii_uppercase().as_str() {
            "A" => gc::A,
            "B" => gc::B,
            "X" => gc::X,
            "Y" => gc::Y,
            "Z" => gc::Z,
            "L" => gc::L,
            "R" => gc::R,
            "START" => gc::START,
            "UP" => gc::UP,
            "DOWN" => gc::DOWN,
            "LEFT" => gc::LEFT,
            "RIGHT" => gc::RIGHT,
            other => return Err(alloc::format!("unknown button {other}")),
        };
    }
    Ok(b)
}

fn num<T: core::str::FromStr>(s: Option<&str>) -> Result<T, String> {
    s.and_then(|s| s.parse().ok())
        .ok_or_else(|| String::from("expected a number"))
}

fn parse_line(line: &str, out: &mut Vec<Cmd>) -> Result<(), String> {
    let line = line.split('#').next().unwrap_or("").trim();
    if line.is_empty() {
        return Ok(());
    }
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    let mut args = rest.split_whitespace();
    match word.to_ascii_lowercase().as_str() {
        "wait" => out.push(Cmd::Wait(num(args.next())?)),
        "press" => {
            let b = buttons(args.next().ok_or("press what?")?)?;
            let n = args.next().map_or(Ok(2), |s| num(Some(s)))?;
            out.extend([Cmd::Hold(b), Cmd::Wait(n), Cmd::Release(b), Cmd::Wait(4)]);
        }
        "hold" => out.push(Cmd::Hold(buttons(args.next().ok_or("hold what?")?)?)),
        "release" => out.push(Cmd::Release(buttons(args.next().ok_or("release what?")?)?)),
        "stick" => out.push(Cmd::Stick(num(args.next())?, num(args.next())?)),
        "cstick" => out.push(Cmd::CStick(num(args.next())?, num(args.next())?)),
        "triggers" => out.push(Cmd::Triggers(num(args.next())?, num(args.next())?)),
        "times" => {
            let n: u32 = num(args.next())?;
            let inner = rest
                .split_once(char::is_whitespace)
                .map(|(_, c)| c)
                .ok_or("times N what?")?;
            let mut one = Vec::new();
            parse_line(inner, &mut one)?;
            for _ in 0..n {
                out.extend(one.iter().cloned());
            }
        }
        "shot" => out.push(Cmd::Shot(String::from(rest))),
        "log" => out.push(Cmd::Log(String::from(rest))),
        "warp" => out.push(Cmd::Warp([
            num(args.next())?,
            num(args.next())?,
            num(args.next())?,
            num(args.next())?,
        ])),
        "pace" => {
            let n: u8 = num(args.next())?;
            if !(1..=4).contains(&n) {
                return Err(String::from("pace 1..4"));
            }
            out.push(Cmd::Pace(n));
        }
        "quit" => out.push(Cmd::Quit),
        other => return Err(alloc::format!("unknown command {other}")),
    }
    Ok(())
}

impl Script {
    /// Parse a script; the error names the first bad line.
    pub fn parse(text: &str) -> Result<Script, String> {
        let mut cmds = Vec::new();
        for (n, line) in text.lines().enumerate() {
            parse_line(line, &mut cmds).map_err(|e| alloc::format!("line {}: {e}", n + 1))?;
        }
        Ok(Script {
            cmds,
            ..Script::default()
        })
    }

    /// True once every command has run.
    pub fn done(&self) -> bool {
        self.pc >= self.cmds.len() && self.wait == 0
    }

    /// The next frame: commands run until one waits.
    pub fn frame(&mut self) -> Frame {
        let mut f = Frame {
            shot: None,
            log: None,
            warp: None,
            pace: None,
            quit: false,
            ..self.pad.clone()
        };
        if self.wait > 0 {
            self.wait -= 1;
            return f;
        }
        while let Some(c) = self.cmds.get(self.pc).cloned() {
            self.pc += 1;
            match c {
                Cmd::Wait(n) => {
                    if n > 0 {
                        self.wait = n - 1;
                        break;
                    }
                }
                Cmd::Hold(b) => self.pad.held |= b,
                Cmd::Release(b) => self.pad.held &= !b,
                Cmd::Stick(x, y) => (self.pad.sx, self.pad.sy) = (x, y),
                Cmd::CStick(x, y) => (self.pad.cx, self.pad.cy) = (x, y),
                Cmd::Triggers(l, r) => (self.pad.l, self.pad.r) = (l, r),
                // One request a frame: the next waits for the following frame.
                Cmd::Shot(name) => {
                    f.shot = Some(name);
                    break;
                }
                Cmd::Log(s) => {
                    f.log = Some(s);
                    break;
                }
                Cmd::Warp(w) => {
                    f.warp = Some(w);
                    break;
                }
                // Takes effect with this frame; the script goes on.
                Cmd::Pace(n) => f.pace = Some(n),
                Cmd::Quit => {
                    f.quit = true;
                    break;
                }
            }
        }
        Frame {
            shot: f.shot,
            log: f.log,
            warp: f.warp,
            pace: f.pace,
            quit: f.quit,
            ..self.pad.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pace_is_requested_without_using_a_frame() {
        let mut s = Script::parse(
            "pace 2
press A 1",
        )
        .unwrap();
        let f = s.frame();
        assert_eq!(f.pace, Some(2));
        assert_eq!(f.held, gc::A);
        assert_eq!(s.frame().pace, None);
        assert!(Script::parse("pace 0").is_err() && Script::parse("pace 5").is_err());
    }

    #[test]
    fn press_holds_then_releases() {
        let mut s = Script::parse("press A 2\nshot main\nquit").unwrap();
        let held: Vec<u16> = (0..6).map(|_| s.frame().held).collect();
        assert_eq!(held, [gc::A, gc::A, 0, 0, 0, 0]);
        let f = s.frame();
        assert_eq!(f.shot.as_deref(), Some("main"));
        assert!(s.frame().quit);
        assert!(s.done());
    }

    #[test]
    fn times_repeats_and_sticks_persist() {
        let mut s = Script::parse("stick 0 100\ntimes 2 press DOWN+Z 1  # two\n").unwrap();
        let f = s.frame();
        assert_eq!((f.held, f.sy), (gc::DOWN | gc::Z, 100));
        let frames: Vec<u16> = (0..9).map(|_| s.frame().held).collect();
        assert_eq!(frames.iter().filter(|&&h| h != 0).count(), 1);
        assert!(s.done());
    }

    #[test]
    fn warp_takes_a_spot() {
        let mut s = Script::parse(
            "warp 1189 420 -239 90
wait 1",
        )
        .unwrap();
        assert_eq!(s.frame().warp, Some([1189, 420, -239, 90]));
        assert_eq!(s.frame().warp, None);
        assert!(Script::parse("warp 1 2").is_err());
    }

    #[test]
    fn bad_lines_are_named() {
        let e = Script::parse("wait 1\npress Q").err().unwrap();
        assert!(e.starts_with("line 2"), "{e}");
    }
}

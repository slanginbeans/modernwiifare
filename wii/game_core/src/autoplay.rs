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
//! - `match MAP MODE [bots=N] [class=N] [primary=WEAPON] [kills=N] [passive] [frozen] [god] [ammo] [streaks]`
//!   (the first command, or after `endmatch`): skip the menus and start a private match on map MAP (`mp_rust`) in
//!   MODE (`ffa`, `tdm`, `dom`) with the debug settings at their defaults, N bots (default 5), the Nth class of the
//!   class menu (custom classes first, default 0) without asking for one, the score limit at N kills, and the debug
//!   page's match settings named: the primary weapon replaced by catalog weapon WEAPON (its name or catalog number),
//!   bots don't attack, bots frozen, god mode, infinite ammo, all killstreaks. The script's next command is the
//!   match's first frame; when the match ends the game goes on to the main menu.
//! - `endmatch`: leave the match (as the pause menu's End Game) for the main menu; a `match` may follow.
//! - `script NAME`: the test script running from here on (a suite of scripts in one file, one console boot): its
//!   screenshots go to `shots/NAME__SHOT.bmp`, its match logs carry `script=NAME`, and pacing goes back to a frame
//!   every retrace.
//! - `quit`: switch the console off (Dolphin stops and writes the SD folder back).
//!
//! Buttons: A B X Y Z L R START UP DOWN LEFT RIGHT. Frames are pad reads: one a frame in the menus and the match,
//! none while a map loads.

use alloc::string::String;
use alloc::vec::Vec;

use crate::controls::gc;

/// A `match` command: the private match to start straight away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickMatch {
    pub map: String,
    pub mode: crate::arena::Mode,
    pub bots: u8,
    pub class: u8,
    /// A catalog weapon's name or number for the debug page's primary weapon.
    pub primary: Option<String>,
    /// The score limit in kills (the mode's default without it).
    pub kills: Option<u32>,
    pub passive: bool,
    pub frozen: bool,
    pub god: bool,
    pub ammo: bool,
    pub streaks: bool,
}

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
    Match(QuickMatch),
    EndMatch,
    Script(String),
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
    /// Leave the match for the main menu.
    pub end_match: bool,
    /// A new script section starts (its name).
    pub script: Option<String>,
    pub quit: bool,
}

#[derive(Default)]
pub struct Script {
    cmds: Vec<Cmd>,
    pc: usize,
    wait: u32,
    pad: Frame,
    pending_script: Option<String>,
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
        "match" => {
            let map = String::from(args.next().ok_or("match which map?")?);
            let mode = match args.next().map(str::to_ascii_lowercase).as_deref() {
                Some("ffa") => crate::arena::Mode::Ffa,
                Some("tdm") => crate::arena::Mode::Tdm,
                Some("dom") => crate::arena::Mode::Dom,
                _ => return Err(String::from("match MAP ffa|tdm|dom")),
            };
            let mut m = QuickMatch {
                map,
                mode,
                bots: 5,
                class: 0,
                primary: None,
                kills: None,
                passive: false,
                frozen: false,
                god: false,
                ammo: false,
                streaks: false,
            };
            for a in args {
                match (a.split_once('='), a) {
                    (Some(("bots", n)), _) => {
                        m.bots = num::<u8>(Some(n))?.min(crate::debug::MAX_BOTS);
                    }
                    (Some(("class", n)), _) => m.class = num(Some(n))?,
                    (Some(("primary", w)), _) => m.primary = Some(String::from(w)),
                    (Some(("kills", n)), _) => m.kills = Some(num(Some(n))?),
                    (None, "passive") => m.passive = true,
                    (None, "frozen") => m.frozen = true,
                    (None, "god") => m.god = true,
                    (None, "ammo") => m.ammo = true,
                    (None, "streaks") => m.streaks = true,
                    _ => return Err(alloc::format!("match: unknown {a}")),
                }
            }
            // Only where the game is at the main menu: the script's start (after its name), or after `endmatch`.
            let at_menu = out
                .iter()
                .rev()
                .find(|c| !matches!(c, Cmd::Script(_)))
                .is_none_or(|c| matches!(c, Cmd::EndMatch));
            if !at_menu {
                return Err(String::from(
                    "match must be the first command or follow endmatch",
                ));
            }
            out.push(Cmd::Match(m));
        }
        "endmatch" => out.push(Cmd::EndMatch),
        "script" => {
            if rest.is_empty() || rest.contains(char::is_whitespace) {
                return Err(String::from("script NAME (one word)"));
            }
            out.push(Cmd::Script(String::from(rest)));
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

    /// The private match to start straight away (a first `match` command), taken once.
    pub fn take_match(&mut self) -> Option<QuickMatch> {
        // Script names before it are taken with it (the game reads them back with [`Script::take_script`]).
        let mut at = self.pc;
        while let Some(Cmd::Script(_)) = self.cmds.get(at) {
            at += 1;
        }
        match self.cmds.get(at) {
            Some(Cmd::Match(m)) if self.wait == 0 => {
                let m = m.clone();
                for c in &self.cmds[self.pc..at] {
                    if let Cmd::Script(n) = c {
                        self.pending_script = Some(n.clone());
                    }
                }
                self.pc = at + 1;
                Some(m)
            }
            _ => None,
        }
    }

    /// A `match` is next (after any `script` names): the game should start it from the main menu.
    pub fn match_ready(&self) -> bool {
        self.wait == 0
            && self.cmds[self.pc.min(self.cmds.len())..]
                .iter()
                .find(|c| !matches!(c, Cmd::Script(_)))
                .is_some_and(|c| matches!(c, Cmd::Match(_)))
    }

    /// A script name taken by [`Script::take_match`], once.
    pub fn take_script(&mut self) -> Option<String> {
        self.pending_script.take()
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
            end_match: false,
            script: None,
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
                // Started by the game from the main menu ([`Script::take_match`]): wait for it there.
                Cmd::Match(_) => {
                    self.pc -= 1;
                    break;
                }
                Cmd::EndMatch => {
                    f.end_match = true;
                    break;
                }
                // A section without a `match` (it starts in the menus).
                Cmd::Script(n) => f.script = Some(n),
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
            end_match: f.end_match,
            script: f.script,
            quit: f.quit,
            ..self.pad.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_is_taken_first_then_the_script_runs() {
        let mut s =
            Script::parse("match mp_rust tdm bots=0 class=2 primary=rpg frozen god\npress A 1")
                .unwrap();
        assert_eq!(
            s.take_match(),
            Some(QuickMatch {
                map: String::from("mp_rust"),
                mode: crate::arena::Mode::Tdm,
                bots: 0,
                class: 2,
                primary: Some(String::from("rpg")),
                kills: None,
                passive: false,
                frozen: true,
                god: true,
                ammo: false,
                streaks: false,
            })
        );
        assert_eq!(s.take_match(), None);
        assert_eq!(s.frame().held, gc::A);
        // Defaults, and the errors.
        let mut d = Script::parse("match mp_rust ffa").unwrap();
        assert!(d.take_match().is_some_and(|m| m.bots == 5 && m.class == 0));
        assert!(Script::parse("wait 1\nmatch mp_rust tdm").is_err());
        // A suite: named sections, each with its match after the previous one's endmatch.
        let mut s = Script::parse(
            "script a\nmatch mp_rust tdm\nwait 2\nendmatch\nscript b\nmatch mp_rust ffa bots=0\nquit",
        )
        .unwrap();
        assert!(s.take_match().is_some());
        assert_eq!(s.take_script().as_deref(), Some("a"));
        assert!(s.take_match().is_none());
        s.frame();
        s.frame();
        assert!(s.frame().end_match);
        // The next frame (at the menu) names section b and waits there for its match, which the game then takes.
        let f = s.frame();
        assert!(!f.quit && f.script.as_deref() == Some("b"));
        let m = s.take_match().unwrap();
        assert_eq!((m.mode, m.bots), (crate::arena::Mode::Ffa, 0));
        assert_eq!(s.take_script(), None);
        assert!(s.frame().quit);
        assert!(Script::parse("match mp_rust tdm\nwait 1\nmatch mp_rust tdm").is_err());
        assert!(Script::parse("match mp_rust ctf").is_err());
        assert!(Script::parse("match mp_rust tdm skill=3").is_err());
        // A script without one starts in the menus.
        assert_eq!(Script::parse("wait 1").unwrap().take_match(), None);
    }

    #[test]
    fn pace_is_requested_without_using_a_frame() {
        let mut s = Script::parse("pace 2\npress A 1").unwrap();
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

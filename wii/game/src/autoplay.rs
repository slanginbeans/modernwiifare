//! Scripted pad input and screenshots for testing (`game_core::autoplay`): `<app folder>/autoplay.txt`, read once at
//! start. Without the file nothing changes. Screenshots go to `<app folder>/shots/NAME.bmp` (in a suite,
//! `shots/SCRIPT__NAME.bmp`, SCRIPT the section's `script` name).

use alloc::string::String;

use game_core::autoplay::{Frame, Script};

use crate::{AppPath, devlog, ffi};

struct State {
    script: Script,
    last_held: u16,
    shot: Option<String>,
    warp: Option<[i32; 4]>,
    /// The script's `pace`, once it gives one.
    pace: Option<u8>,
    /// The running section's `script` name (a suite in one boot).
    name: Option<String>,
    /// The script said `endmatch`: the match leaves for the main menu.
    end_match: bool,
    quit: bool,
}

static mut STATE: Option<State> = None;

#[allow(static_mut_refs)]
fn state() -> Option<&'static mut State> {
    // Single-threaded: the game loop only.
    unsafe { STATE.as_mut() }
}

/// Read `autoplay.txt` when it is there. A bad script shows its error and stops.
pub fn init() {
    let Some(path) = AppPath::new("autoplay.txt") else {
        return;
    };
    // A whole suite fits (the buffer is freed once parsed).
    let mut buf = alloc::vec![0u8; 512 * 1024];
    let n = unsafe { ffi::mwf_file_read(path.as_ptr(), 0, buf.len() as u32, buf.as_mut_ptr()) };
    if n <= 0 {
        return;
    }
    let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
    match Script::parse(text) {
        Ok(script) => {
            if let Some(dir) = AppPath::new("shots") {
                unsafe { ffi::mwf_mkdir(dir.as_ptr()) };
            }
            unsafe {
                STATE = Some(State {
                    script,
                    last_held: 0,
                    shot: None,
                    warp: None,
                    pace: None,
                    name: None,
                    end_match: false,
                    quit: false,
                })
            };
        }
        Err(e) => panic!("autoplay.txt: {e}"),
    }
}

/// This frame's scripted pad, when a script runs (its requests are kept for the frame's end).
pub fn frame() -> Option<Frame> {
    let s = state()?;
    if s.quit {
        return Some(Frame::default());
    }
    let f = s.script.frame();
    if let Some(name) = &f.shot {
        s.shot = Some(name.clone());
    }
    if let Some(line) = &f.log {
        devlog::event(ffi::ms(), "autoplay", format_args!("{line}"));
    }
    if f.warp.is_some() {
        s.warp = f.warp;
    }
    if let Some(n) = &f.script {
        begin_section(s, n.clone());
    }
    s.end_match |= f.end_match;
    if let Some(p) = f.pace {
        s.pace = Some(p);
        unsafe { ffi::mwf_set_frame_interval(u32::from(p)) };
        crate::PACE.store(u32::from(p), core::sync::atomic::Ordering::Relaxed);
        devlog::set_pace(u32::from(p));
        devlog::event(ffi::ms(), "pace", format_args!("retraces={p}"));
    }
    s.quit |= f.quit;
    Some(f)
}

/// The frame pacing a script runs with: its `pace`, else a frame every retrace (scripts count frames, so the
/// FRAME RATE option doesn't apply to them). None without a script.
pub fn pace() -> Option<u8> {
    state().map(|s| s.pace.unwrap_or(1))
}

/// The script's first-command `match`, once (the game starts it instead of showing the menus).
pub fn quick_match() -> Option<game_core::autoplay::QuickMatch> {
    let s = state()?;
    let m = s.script.take_match()?;
    if let Some(n) = s.script.take_script() {
        begin_section(s, n);
    }
    s.end_match = false;
    devlog::event(
        ffi::ms(),
        "autoplay",
        format_args!("match {} bots={} class={}", m.map, m.bots, m.class),
    );
    Some(m)
}

/// A new suite section: its name for shots and logs, and pacing back to a frame every retrace.
fn begin_section(s: &mut State, name: String) {
    devlog::event(ffi::ms(), "autoplay", format_args!("script {name}"));
    s.name = Some(name);
    s.pace = None;
    unsafe { ffi::mwf_set_frame_interval(1) };
    crate::PACE.store(1, core::sync::atomic::Ordering::Relaxed);
    devlog::set_pace(1);
}

/// A script's `match` waits to be started (the main menu hands back to the game).
pub fn match_ready() -> bool {
    state().is_some_and(|s| s.script.match_ready())
}

/// The running suite section's name (for the match log's header).
pub fn script_name() -> Option<String> {
    state()?.name.clone()
}

/// The script said `endmatch`, once: the match leaves for the main menu.
pub fn take_end_match() -> bool {
    state().is_some_and(|s| core::mem::take(&mut s.end_match))
}

/// A `warp` the script asked for, once (the match puts the player there).
pub fn take_warp() -> Option<[i32; 4]> {
    state()?.warp.take()
}

pub fn last_held() -> u16 {
    state().map_or(0, |s| s.last_held)
}

pub fn set_last_held(h: u16) {
    if let Some(s) = state() {
        s.last_held = h;
    }
}

/// After a frame is queued: its screenshot, and switching off once the script says `quit`.
pub fn after_frame() {
    let Some(s) = state() else {
        return;
    };
    if let Some(name) = s.shot.take() {
        let mut file = String::from("shots/");
        if let Some(n) = &s.name {
            file.push_str(n);
            file.push_str("__");
        }
        file.push_str(&name);
        file.push_str(".bmp");
        if let Some(p) = AppPath::new(&file) {
            unsafe { ffi::mwf_screenshot(p.as_ptr()) };
        }
    }
    if s.quit {
        devlog::end(ffi::ms());
        unsafe { ffi::mwf_poweroff() };
    }
}

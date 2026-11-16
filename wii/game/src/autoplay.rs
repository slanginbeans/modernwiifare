//! Scripted pad input and screenshots for testing (`game_core::autoplay`): `<app folder>/autoplay.txt`, read once at
//! start. Without the file nothing changes. Screenshots go to `<app folder>/shots/NAME.bmp`.

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
    let mut buf = alloc::vec![0u8; 64 * 1024];
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

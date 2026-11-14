//! The test log on the SD card: `<app folder>/logs/NNNN.log`, one file per match (numbered by `logs/next.txt`),
//! written by [`game_core::telemetry`]. Written to the card only at natural breaks, never in the middle of play (an
//! SD write takes tens of milliseconds): when the game is paused, when you die, at the end of a match and on a panic.
//! Each write's own time is logged. `report.bat` collects and summarises them.

use alloc::vec::Vec;

use game_core::telemetry::{Frame, Telemetry};

use crate::{AppPath, ffi};

struct Session {
    t: Telemetry,
    path: AppPath,
}

static mut LOG: Option<Session> = None;

#[allow(static_mut_refs)]
fn session() -> Option<&'static mut Session> {
    // Single-threaded: the game loop and the panic handler only.
    unsafe { LOG.as_mut() }
}

/// The next log number (from `logs/next.txt`, then advanced).
fn next_number() -> u32 {
    let mut n = 1u32;
    if let Some(p) = AppPath::new("logs/next.txt") {
        let mut b = [0u8; 16];
        let got = unsafe { ffi::mwf_file_read(p.as_ptr(), 0, b.len() as u32, b.as_mut_ptr()) };
        if got > 0
            && let Ok(v) = core::str::from_utf8(&b[..got as usize])
                .unwrap_or("")
                .trim()
                .parse::<u32>()
        {
            n = v.max(1);
        }
        let mut s = game_core::hud::heapless_str::Str::default();
        s.num(n + 1);
        unsafe { ffi::mwf_file_write(p.as_ptr(), s.as_str().as_ptr(), s.as_str().len() as u32) };
    }
    n
}

/// Start a log for a match (ends any open one). `header`: `key=value` pairs.
pub fn start(now_ms: u32, header: core::fmt::Arguments<'_>) {
    end(now_ms);
    if let Some(dir) = AppPath::new("logs") {
        unsafe { ffi::mwf_mkdir(dir.as_ptr()) };
    }
    let n = next_number();
    let mut name = game_core::hud::heapless_str::Str::default();
    name.push_str("logs/");
    for d in [n / 1000 % 10, n / 100 % 10, n / 10 % 10, n % 10] {
        name.push(char::from(b'0' + d as u8));
    }
    name.push_str(".log");
    let Some(path) = AppPath::new(name.as_str()) else {
        return;
    };
    let mut h = alloc::string::String::new();
    let _ = core::fmt::write(&mut h, header);
    let session = Session {
        t: Telemetry::new(now_ms, &h),
        path,
    };
    unsafe { LOG = Some(session) };
    flush(now_ms);
}

/// Record a frame.
pub fn frame(now_ms: u32, f: &Frame) {
    if let Some(s) = session() {
        s.t.frame(now_ms, f);
    }
}

pub fn event(now_ms: u32, what: &'static str, detail: core::fmt::Arguments<'_>) {
    if let Some(s) = session() {
        s.t.event(now_ms, what, detail);
    }
}

/// Append what's buffered to the file; logs the write's time as an event (written with the next flush).
pub fn flush(now_ms: u32) {
    let Some(s) = session() else {
        return;
    };
    let bytes: Vec<u8> = s.t.take();
    if bytes.is_empty() {
        return;
    }
    let t0 = ffi::us();
    let wrote =
        unsafe { ffi::mwf_file_append(s.path.as_ptr(), bytes.as_ptr(), bytes.len() as u32) };
    let us = ffi::us().wrapping_sub(t0);
    s.t.event(
        now_ms,
        "log_write",
        format_args!(
            "bytes={} us={us} ok={}",
            bytes.len(),
            wrote == bytes.len() as i32
        ),
    );
}

/// End the match's log: summary, final write.
pub fn end(now_ms: u32) {
    if let Some(s) = session() {
        s.t.finish(now_ms);
        flush(now_ms);
    }
    unsafe { LOG = None };
}

/// From the panic handler: record the message and write everything out.
pub fn panic(msg: &str) {
    let now = ffi::ms();
    event(now, "panic", format_args!("msg=\"{msg}\""));
    end(now);
}

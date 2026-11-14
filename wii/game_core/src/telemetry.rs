//! Test telemetry: what a play session did, recorded on the Wii and written to the SD card as a small text log that
//! `report.bat` (`mwf-convert report`) turns into a summary to paste back.
//!
//! The log is plain text, one record per line, the first word saying what it is:
//! - `H key=value ...` header (build, map, class, bots, memory at start);
//! - `S t=<s> fps= fr=avg/max cpu= gpu= <section>=avg ... tris= draws= mem1= mem2=` one row per second (averages in
//!   ms with 0.01 precision, max frame);
//! - `X t= fr= <section>= ...` a spike: one frame over 33 ms, with its own breakdown (at most 20 per minute);
//! - `E t= <what> key=value ...` events (kills, deaths, hits, load errors, sound voices exhausted, bot stuck ...);
//! - `P` summary at the end (percentiles from a frame-time histogram, share of frames over budget, section
//!   averages over the whole session, event counts).
//!
//! Nothing here touches the SD card: the caller flushes [`Telemetry::take`] when it is cheap to (pause, match end,
//! every 30 s in a match), so writes never land in the middle of gameplay frames unannounced.

use alloc::vec::Vec;
use core::fmt::Write;

/// CPU sections timed each frame (µs). Keep in sync with [`SECTION_NAMES`].
pub mod section {
    pub const INPUT: usize = 0;
    pub const PLAYER: usize = 1;
    pub const GUN: usize = 2;
    pub const BOTS: usize = 3;
    pub const WORLD: usize = 4;
    pub const BODIES: usize = 5;
    pub const VIEW: usize = 6;
    pub const HUD: usize = 7;
    pub const COUNT: usize = 8;
}

pub const SECTION_NAMES: [&str; section::COUNT] =
    ["in", "mv", "gun", "bot", "wld", "bod", "vm", "hud"];

/// One frame's numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    /// Whole frame, start to start (µs).
    pub frame_us: u32,
    /// CPU work before waiting for the GPU (µs).
    pub cpu_us: u32,
    /// Time waiting for the GPU at the end of the frame (µs).
    pub gpu_us: u32,
    pub sections: [u32; section::COUNT],
    pub triangles: u32,
    pub draws: u32,
    /// Free MEM1 heap and MEM2 (bytes; 0 = unknown).
    pub mem1_free: u32,
    pub mem2_free: u32,
}

/// Frame-time histogram: 0.25 ms bins up to 64 ms (last bin: everything above).
const BIN_US: u32 = 250;
const BINS: usize = 256;
/// The 60 fps budget.
pub const BUDGET_US: u32 = 16_667;
const SPIKE_US: u32 = 33_334;
const MAX_SPIKES_PER_MINUTE: u32 = 20;

#[derive(Default)]
struct Second {
    frames: u32,
    frame: u64,
    frame_max: u32,
    cpu: u64,
    gpu: u64,
    sections: [u64; section::COUNT],
    triangles: u64,
    draws: u64,
}

pub struct Telemetry {
    buf: Vec<u8>,
    start_ms: u32,
    second_start_ms: u32,
    sec: Second,
    hist: [u32; BINS],
    total_frames: u64,
    over_budget: u64,
    over_spike: u64,
    total_sections: [u64; section::COUNT],
    total_cpu: u64,
    total_gpu: u64,
    spikes_this_minute: u32,
    minute: u32,
    events: Vec<(&'static str, u32)>,
    last_mem: (u32, u32),
}

fn ms2(out: &mut Vec<u8>, us: u64) {
    // Microseconds as milliseconds with two decimals, no floats.
    let _ = write!(Buf(out), "{}.{:02}", us / 1000, (us % 1000) / 10);
}

struct Buf<'a>(&'a mut Vec<u8>);
impl Write for Buf<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

impl Telemetry {
    /// A session starting at `now_ms`; `header` is `key=value` pairs (no newlines).
    pub fn new(now_ms: u32, header: &str) -> Telemetry {
        let mut t = Telemetry {
            buf: Vec::with_capacity(64 * 1024),
            start_ms: now_ms,
            second_start_ms: now_ms,
            sec: Second::default(),
            hist: [0; BINS],
            total_frames: 0,
            over_budget: 0,
            over_spike: 0,
            total_sections: [0; section::COUNT],
            total_cpu: 0,
            total_gpu: 0,
            spikes_this_minute: 0,
            minute: 0,
            events: Vec::new(),
            last_mem: (0, 0),
        };
        t.line(format_args!("H {header}"));
        t
    }

    fn line(&mut self, args: core::fmt::Arguments<'_>) {
        let _ = Buf(&mut self.buf).write_fmt(args);
        self.buf.push(b'\n');
    }

    fn t(&self, now_ms: u32) -> (u32, u32) {
        let d = now_ms.wrapping_sub(self.start_ms);
        (d / 1000, d % 1000)
    }

    /// Record an event (`what` is one word; `detail` is `key=value` pairs).
    pub fn event(&mut self, now_ms: u32, what: &'static str, detail: core::fmt::Arguments<'_>) {
        let (s, m) = self.t(now_ms);
        self.line(format_args!("E t={s}.{m:03} {what} {detail}"));
        match self.events.iter_mut().find(|e| e.0 == what) {
            Some(e) => e.1 += 1,
            None => self.events.push((what, 1)),
        }
    }

    /// Record one frame that ended at `now_ms`.
    pub fn frame(&mut self, now_ms: u32, f: &Frame) {
        let s = &mut self.sec;
        s.frames += 1;
        s.frame += u64::from(f.frame_us);
        s.frame_max = s.frame_max.max(f.frame_us);
        s.cpu += u64::from(f.cpu_us);
        s.gpu += u64::from(f.gpu_us);
        for (a, b) in s.sections.iter_mut().zip(f.sections) {
            *a += u64::from(b);
        }
        s.triangles += u64::from(f.triangles);
        s.draws += u64::from(f.draws);
        self.total_frames += 1;
        self.total_cpu += u64::from(f.cpu_us);
        self.total_gpu += u64::from(f.gpu_us);
        for (a, b) in self.total_sections.iter_mut().zip(f.sections) {
            *a += u64::from(b);
        }
        self.hist[((f.frame_us / BIN_US) as usize).min(BINS - 1)] += 1;
        if f.frame_us > BUDGET_US {
            self.over_budget += 1;
        }
        if f.mem1_free != 0 || f.mem2_free != 0 {
            self.last_mem = (f.mem1_free, f.mem2_free);
        }
        let (secs, msr) = self.t(now_ms);
        if secs / 60 != self.minute {
            self.minute = secs / 60;
            self.spikes_this_minute = 0;
        }
        if f.frame_us > SPIKE_US {
            self.over_spike += 1;
            if self.spikes_this_minute < MAX_SPIKES_PER_MINUTE {
                self.spikes_this_minute += 1;
                let mut l = Vec::with_capacity(160);
                let _ = write!(Buf(&mut l), "X t={secs}.{msr:03} fr=");
                ms2(&mut l, u64::from(f.frame_us));
                l.extend_from_slice(b" cpu=");
                ms2(&mut l, u64::from(f.cpu_us));
                l.extend_from_slice(b" gpu=");
                ms2(&mut l, u64::from(f.gpu_us));
                for (n, v) in SECTION_NAMES.iter().zip(f.sections) {
                    let _ = write!(Buf(&mut l), " {n}=");
                    ms2(&mut l, u64::from(v));
                }
                let _ = write!(Buf(&mut l), " tris={} draws={}", f.triangles, f.draws);
                self.buf.extend_from_slice(&l);
                self.buf.push(b'\n');
            }
        }
        if now_ms.wrapping_sub(self.second_start_ms) >= 1000 {
            self.flush_second(now_ms);
        }
    }

    fn flush_second(&mut self, now_ms: u32) {
        let s = core::mem::take(&mut self.sec);
        if s.frames == 0 {
            return;
        }
        let span = now_ms.wrapping_sub(self.second_start_ms).max(1);
        self.second_start_ms = now_ms;
        let n = u64::from(s.frames);
        let (secs, _) = self.t(now_ms);
        let mut l = Vec::with_capacity(200);
        let _ = write!(
            Buf(&mut l),
            "S t={secs} fps={} fr=",
            s.frames as u64 * 1000 / u64::from(span)
        );
        ms2(&mut l, s.frame / n);
        l.push(b'/');
        ms2(&mut l, u64::from(s.frame_max));
        l.extend_from_slice(b" cpu=");
        ms2(&mut l, s.cpu / n);
        l.extend_from_slice(b" gpu=");
        ms2(&mut l, s.gpu / n);
        for (name, v) in SECTION_NAMES.iter().zip(s.sections) {
            let _ = write!(Buf(&mut l), " {name}=");
            ms2(&mut l, v / n);
        }
        let _ = write!(
            Buf(&mut l),
            " tris={} draws={} mem1={}k mem2={}k",
            s.triangles / n,
            s.draws / n,
            self.last_mem.0 / 1024,
            self.last_mem.1 / 1024
        );
        self.buf.extend_from_slice(&l);
        self.buf.push(b'\n');
    }

    /// Frame time (µs) at percentile `p` (0..100) of the session so far, from the histogram (bin upper edge).
    pub fn percentile(&self, p: u32) -> u32 {
        let target = (self.total_frames * u64::from(p)).div_ceil(100).max(1);
        let mut seen = 0u64;
        for (i, &c) in self.hist.iter().enumerate() {
            seen += u64::from(c);
            if seen >= target {
                return (i as u32 + 1) * BIN_US;
            }
        }
        BINS as u32 * BIN_US
    }

    /// End of session: the summary line(s).
    pub fn finish(&mut self, now_ms: u32) {
        self.flush_second(now_ms);
        let n = self.total_frames.max(1);
        let mut l = Vec::with_capacity(400);
        let (secs, _) = self.t(now_ms);
        let _ = write!(Buf(&mut l), "P t={secs} frames={} p50=", self.total_frames);
        for (i, p) in [50u32, 90, 95, 99].iter().enumerate() {
            if i > 0 {
                let _ = write!(Buf(&mut l), " p{p}=");
            }
            ms2(&mut l, u64::from(self.percentile(*p)));
        }
        let _ = write!(
            Buf(&mut l),
            " over16={}.{}% over33={}.{}% cpu=",
            self.over_budget * 100 / n,
            self.over_budget * 1000 / n % 10,
            self.over_spike * 100 / n,
            self.over_spike * 1000 / n % 10
        );
        ms2(&mut l, self.total_cpu / n);
        l.extend_from_slice(b" gpu=");
        ms2(&mut l, self.total_gpu / n);
        for (name, v) in SECTION_NAMES.iter().zip(self.total_sections) {
            let _ = write!(Buf(&mut l), " {name}=");
            ms2(&mut l, v / n);
        }
        for (what, c) in &self.events {
            let _ = write!(Buf(&mut l), " ev.{what}={c}");
        }
        self.buf.extend_from_slice(&l);
        self.buf.push(b'\n');
    }

    /// The text recorded since the last call (the caller appends it to the log file).
    pub fn take(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.buf)
    }

    /// Bytes waiting to be written.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(us: u32) -> Frame {
        Frame {
            frame_us: us,
            cpu_us: us / 2,
            gpu_us: us / 4,
            sections: [100, 2000, 300, 1500, 3000, 50, 900, 200],
            triangles: 120_000,
            draws: 400,
            mem1_free: 4 << 20,
            mem2_free: 10 << 20,
        }
    }

    #[test]
    fn records_rows_spikes_events_and_a_summary() {
        let mut t = Telemetry::new(1000, "build=test map=mp_x");
        let mut now = 1000;
        for i in 0..180 {
            let us = if i == 100 { 40_000 } else { 16_000 };
            now += us / 1000;
            t.frame(now, &frame(us));
        }
        t.event(
            now,
            "kill",
            format_args!("attacker=0 victim=2 loc=head dist=812"),
        );
        t.finish(now);
        let text = alloc::string::String::from_utf8(t.take()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "H build=test map=mp_x");
        assert!(lines.iter().any(|l| l.starts_with("S t=1 fps=")), "{text}");
        let spike = lines.iter().find(|l| l.starts_with("X ")).expect("spike");
        assert!(
            spike.contains("fr=40.00") && spike.contains("mv=2.00"),
            "{spike}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("E t=") && l.contains("kill attacker=0"))
        );
        let p = lines.last().unwrap();
        assert!(
            p.starts_with("P ") && p.contains("p50=16.25") && p.contains("ev.kill=1"),
            "{p}"
        );
        assert!(p.contains("over33=0.5%"), "{p}");
        assert_eq!(t.pending(), 0);
    }
}

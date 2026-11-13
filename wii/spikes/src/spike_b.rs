//! Spike B: can Wiis find each other and talk over the LAN?
//!
//! Once per second each Wii sends the same beacon twice: to 255.255.255.255 (limited broadcast)
//! and to its subnet broadcast (e.g. 192.168.1.255). Every Wii that hears a beacon answers with a
//! unicast reply, which gives the sender a round-trip time. The screen shows, per peer, what
//! percentage of its beacons arrived over each broadcast style, plus RTT min/avg/max.
//!
//! Controls: B resets stats, START or RESET exits.

use core::ffi::CStr;

use syslink::addr::{Ip, LIMITED_BROADCAST, directed_broadcast, parse_ipv4};
use syslink::discovery::{
    DISCOVERY_PORT, Kind, NAME_LEN, PACKET_LEN, Packet, Route, make_name, name_str,
};
use syslink::fnv1a32;
use syslink::peers::{Event, PeerTable};

use crate::ffi;
use crate::rt::{BUILD_ID, FixedBuf};
use crate::say;

const BEACON_INTERVAL_MS: u32 = 1_000;
const REDRAW_INTERVAL_MS: u32 = 250;
/// Console lines are padded to this width so redraws overwrite old text without clearing.
const LINE: usize = 72;

#[derive(Default)]
struct SendStats {
    ok: u32,
    err: u32,
    last_err: i32,
}

impl SendStats {
    fn record(&mut self, r: i32) {
        if r >= 0 {
            self.ok += 1;
        } else {
            self.err += 1;
            self.last_err = r;
        }
    }
}

#[derive(Default)]
struct RxStats {
    packets: u32,
    bad: u32,
    recv_err: u32,
    last_recv_err: i32,
}

/// Print one fixed-width console line.
fn line(args: core::fmt::Arguments) {
    let mut b = FixedBuf::<{ LINE + 1 }>::new();
    let _ = core::fmt::write(&mut b, args);
    let s = core::str::from_utf8(b.as_bytes()).unwrap_or("");
    say!("{:<width$.width$}\n", s, width = LINE);
}

fn cstr_ip(buf: &[u8; 16]) -> Option<u32> {
    CStr::from_bytes_until_nul(buf)
        .ok()?
        .to_str()
        .ok()
        .and_then(parse_ipv4)
}

pub fn run() -> i32 {
    let build = fnv1a32(BUILD_ID.as_bytes());
    // Unique enough for a LAN: boot-time tick count mixed through FNV.
    let my_id = fnv1a32(&ffi::ms().to_be_bytes()) ^ build.rotate_left(7);
    let mut name_text = FixedBuf::<NAME_LEN>::new();
    let _ = core::fmt::write(&mut name_text, format_args!("Wii-{:04X}", my_id & 0xFFFF));
    let my_name = make_name(core::str::from_utf8(name_text.as_bytes()).unwrap_or("Wii"));

    say!("\x1b[2;0HModern Wiifare - Spike B: system link discovery\n\n");
    say!("Bringing up network (DHCP)...\n");
    ffi::vsync();

    let (mut ip_s, mut mask_s, mut gw_s) = ([0u8; 16], [0u8; 16], [0u8; 16]);
    let net_r =
        unsafe { ffi::mwf_net_init(ip_s.as_mut_ptr(), mask_s.as_mut_ptr(), gw_s.as_mut_ptr()) };
    let (ip, mask, gw) = (cstr_ip(&ip_s), cstr_ip(&mask_s), cstr_ip(&gw_s));
    let (Some(ip), Some(mask)) = (ip, mask) else {
        say!("\nNetwork init FAILED: if_config returned {net_r}.\n");
        say!(
            "Check the USB LAN adapter / Wii network settings, then retry.\n\nSTART or RESET exits.\n"
        );
        loop {
            ffi::vsync();
            if ffi::pad0().down & ffi::BTN_START != 0 || ffi::should_exit() {
                return 1;
            }
        }
    };
    let bcast = directed_broadcast(ip, mask);

    let (mut bopt, mut bind_r) = (0i32, 0i32);
    let sock = unsafe { ffi::mwf_udp_open(DISCOVERY_PORT, &mut bopt, &mut bind_r) };

    let mut table = PeerTable::new(my_id);
    let mut tx_lim = SendStats::default();
    let mut tx_dir = SendStats::default();
    let mut tx_reply = SendStats::default();
    let mut rx = RxStats::default();
    let mut seq: u32 = 0;
    let started = ffi::ms();
    let mut next_beacon = started;
    let mut next_redraw = started;
    let mut buf = [0u8; 512];
    let mem = ffi::mem_info();

    say!("\x1b[2J");
    loop {
        let pad = ffi::pad0();
        if pad.down & ffi::BTN_START != 0 || ffi::should_exit() {
            return 0;
        }
        if pad.down & ffi::BTN_B != 0 {
            table.reset();
            tx_lim = SendStats::default();
            tx_dir = SendStats::default();
            tx_reply = SendStats::default();
            rx = RxStats::default();
        }
        let now = ffi::ms();

        // Beacon: same seq over both broadcast styles.
        if sock >= 0 && now.wrapping_sub(next_beacon) as i32 >= 0 {
            next_beacon = next_beacon.wrapping_add(BEACON_INTERVAL_MS);
            seq = seq.wrapping_add(1);
            for (route, dest, stats) in [
                (Route::Limited, LIMITED_BROADCAST, &mut tx_lim),
                (Route::Directed, bcast, &mut tx_dir),
            ] {
                let p = Packet {
                    kind: Kind::Beacon,
                    route,
                    console_id: my_id,
                    seq,
                    t_ms: now,
                    echo_seq: 0,
                    echo_t_ms: 0,
                    build,
                    name: my_name,
                };
                let bytes = p.encode();
                let r = unsafe {
                    ffi::mwf_udp_send(
                        sock,
                        dest,
                        DISCOVERY_PORT,
                        bytes.as_ptr(),
                        PACKET_LEN as u32,
                    )
                };
                stats.record(r);
            }
        }

        // Drain the socket (bounded so a flood can't stall the frame).
        if sock >= 0 {
            for _ in 0..32 {
                let (mut from_ip, mut from_port) = (0u32, 0u16);
                let n = unsafe {
                    ffi::mwf_udp_recv(
                        sock,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut from_ip,
                        &mut from_port,
                    )
                };
                if n == 0 {
                    break;
                }
                if n < 0 {
                    rx.recv_err += 1;
                    rx.last_recv_err = n;
                    break;
                }
                rx.packets += 1;
                let Ok(p) = Packet::decode(&buf[..n as usize]) else {
                    rx.bad += 1;
                    continue;
                };
                let now = ffi::ms();
                if let Event::BeaconFrom { ip: peer_ip } = table.on_packet(&p, from_ip, now) {
                    let reply = p.reply_to(my_id, build, my_name, now).encode();
                    let r = unsafe {
                        ffi::mwf_udp_send(
                            sock,
                            peer_ip,
                            DISCOVERY_PORT,
                            reply.as_ptr(),
                            PACKET_LEN as u32,
                        )
                    };
                    tx_reply.record(r);
                }
            }
        }

        if now.wrapping_sub(next_redraw) as i32 >= 0 {
            next_redraw = now.wrapping_add(REDRAW_INTERVAL_MS);
            say!("\x1b[1;0H");
            line(format_args!(
                "Modern Wiifare - Spike B: system link discovery    build {BUILD_ID}"
            ));
            line(format_args!(""));
            line(format_args!(
                "This Wii {}  IP {}  mask {}",
                name_str(&my_name),
                Ip(ip),
                Ip(mask)
            ));
            line(format_args!(
                "gateway {}  subnet broadcast {}  port {}",
                gw.map(Ip).unwrap_or(Ip(0)),
                Ip(bcast),
                DISCOVERY_PORT
            ));
            line(format_args!(
                "socket {sock}  SO_BROADCAST {bopt}  bind {bind_r}  (0 = ok)"
            ));
            line(format_args!(
                "sent seq {seq}: limited ok {} err {} ({})  directed ok {} err {} ({})",
                tx_lim.ok, tx_lim.err, tx_lim.last_err, tx_dir.ok, tx_dir.err, tx_dir.last_err
            ));
            line(format_args!(
                "recv {} pkts, {} bad, {} errors ({})  replies sent {} err {}",
                rx.packets, rx.bad, rx.recv_err, rx.last_recv_err, tx_reply.ok, tx_reply.err
            ));
            line(format_args!(
                "MEM1 free {} KB  MEM2 free {} KB  uptime {} s",
                mem.arena1_hi.wrapping_sub(mem.arena1_lo) / 1024,
                mem.arena2_hi.wrapping_sub(mem.arena2_lo) / 1024,
                now.wrapping_sub(started) / 1000
            ));
            line(format_args!(""));
            line(format_args!("Peers: {}", table.len()));
            line(format_args!(
                "  NAME      IP               LIM%  DIR%  RTT ms min/avg/max   BUILD  SEEN"
            ));
            let mut shown = 0;
            for p in table.iter() {
                let avg = p.rtt.avg();
                let mut rtt = FixedBuf::<24>::new();
                let _ = match avg {
                    Some(a) => core::fmt::write(
                        &mut rtt,
                        format_args!("{}/{}/{}", p.rtt.min, a, p.rtt.max),
                    ),
                    None => core::fmt::write(&mut rtt, format_args!("-")),
                };
                line(format_args!(
                    "  {:<9} {:<15}  {:>4}  {:>4}  {:<18}  {:<5}  {}",
                    name_str(&p.name),
                    Ip(p.ip),
                    p.delivery_pct(Route::Limited),
                    p.delivery_pct(Route::Directed),
                    core::str::from_utf8(rtt.as_bytes()).unwrap_or("?"),
                    if p.build == build { "same" } else { "DIFF" },
                    if p.is_stale(now) { "stale" } else { "live" }
                ));
                shown += 1;
            }
            for _ in shown..syslink::peers::MAX_PEERS {
                line(format_args!(""));
            }
            line(format_args!(""));
            line(format_args!("B: reset stats    START/RESET: exit"));
        }

        ffi::vsync();
    }
}

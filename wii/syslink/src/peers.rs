//! Fixed-capacity table of consoles heard on the LAN, with the per-route delivery and RTT
//! statistics Spike B reports. No allocation.

use crate::discovery::{Kind, NAME_LEN, Packet, Route};

pub const MAX_PEERS: usize = 8;
/// A peer not heard from for this long is shown as stale.
pub const STALE_MS: u32 = 3_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RttStats {
    pub last: u32,
    pub min: u32,
    pub max: u32,
    pub sum: u64,
    pub count: u32,
}

impl RttStats {
    fn add(&mut self, ms: u32) {
        if self.count == 0 || ms < self.min {
            self.min = ms;
        }
        self.max = self.max.max(ms);
        self.last = ms;
        self.sum += ms as u64;
        self.count += 1;
    }
    pub fn avg(&self) -> Option<u32> {
        (self.count > 0).then(|| (self.sum / self.count as u64) as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub console_id: u32,
    pub ip: u32,
    pub name: [u8; NAME_LEN],
    pub build: u32,
    pub first_seq: u32,
    pub last_seq: u32,
    pub rx_limited: u32,
    pub rx_directed: u32,
    pub replies: u32,
    pub rtt: RttStats,
    pub last_seen_ms: u32,
}

impl Peer {
    fn new(p: &Packet, ip: u32, now_ms: u32) -> Peer {
        Peer {
            console_id: p.console_id,
            ip,
            name: p.name,
            build: p.build,
            first_seq: p.seq,
            last_seq: p.seq,
            rx_limited: 0,
            rx_directed: 0,
            replies: 0,
            rtt: RttStats::default(),
            last_seen_ms: now_ms,
        }
    }

    /// Beacons this peer has sent per route since we first heard it (one per seq per route).
    pub fn beacons_expected(&self) -> u32 {
        self.last_seq.wrapping_sub(self.first_seq).wrapping_add(1)
    }

    /// Percentage (0-100) of that peer's beacons we received over `route`.
    pub fn delivery_pct(&self, route: Route) -> u32 {
        let got = match route {
            Route::Limited => self.rx_limited,
            Route::Directed => self.rx_directed,
            Route::Unicast => return 100,
        };
        let exp = self.beacons_expected().max(1);
        (got.min(exp) as u64 * 100 / exp as u64) as u32
    }

    pub fn is_stale(&self, now_ms: u32) -> bool {
        now_ms.wrapping_sub(self.last_seen_ms) > STALE_MS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A beacon from another console; the caller should send it `Packet::reply_to`.
    BeaconFrom { ip: u32 },
    /// A reply to one of our beacons; RTT recorded.
    Reply { rtt_ms: u32 },
    /// Our own broadcast looped back, or a reply to a console that is not us.
    Ignored,
    /// The table is full; packet dropped.
    Full,
}

pub struct PeerTable {
    my_id: u32,
    peers: [Option<Peer>; MAX_PEERS],
}

impl PeerTable {
    pub const fn new(my_id: u32) -> PeerTable {
        PeerTable {
            my_id,
            peers: [None; MAX_PEERS],
        }
    }

    pub fn reset(&mut self) {
        self.peers = [None; MAX_PEERS];
    }

    pub fn iter(&self) -> impl Iterator<Item = &Peer> {
        self.peers.iter().flatten()
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn slot(&mut self, p: &Packet, ip: u32, now_ms: u32) -> Option<&mut Peer> {
        let idx = match self
            .peers
            .iter()
            .position(|s| s.is_some_and(|q| q.console_id == p.console_id))
        {
            Some(i) => i,
            None => {
                let free = self.peers.iter().position(Option::is_none)?;
                self.peers[free] = Some(Peer::new(p, ip, now_ms));
                free
            }
        };
        self.peers[idx].as_mut()
    }

    /// Feed one received packet. `from_ip` is the datagram's source address.
    pub fn on_packet(&mut self, p: &Packet, from_ip: u32, now_ms: u32) -> Event {
        if p.console_id == self.my_id {
            return Event::Ignored;
        }
        let Some(peer) = self.slot(p, from_ip, now_ms) else {
            return Event::Full;
        };
        peer.ip = from_ip;
        peer.name = p.name;
        peer.build = p.build;
        peer.last_seen_ms = now_ms;
        match p.kind {
            Kind::Beacon => {
                // Restarted console: its seq went backwards. Start counting again.
                if p.seq < peer.first_seq {
                    peer.first_seq = p.seq;
                    peer.rx_limited = 0;
                    peer.rx_directed = 0;
                }
                if p.seq > peer.last_seq {
                    peer.last_seq = p.seq;
                }
                match p.route {
                    Route::Limited => peer.rx_limited += 1,
                    Route::Directed => peer.rx_directed += 1,
                    Route::Unicast => {}
                }
                Event::BeaconFrom { ip: from_ip }
            }
            Kind::Reply => {
                let rtt = now_ms.wrapping_sub(p.echo_t_ms);
                peer.replies += 1;
                peer.rtt.add(rtt);
                Event::Reply { rtt_ms: rtt }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::make_name;

    fn beacon(id: u32, seq: u32, route: Route, t: u32) -> Packet {
        Packet {
            kind: Kind::Beacon,
            route,
            console_id: id,
            seq,
            t_ms: t,
            echo_seq: 0,
            echo_t_ms: 0,
            build: 1,
            name: make_name("peer"),
        }
    }

    #[test]
    fn ignores_own_packets() {
        let mut t = PeerTable::new(5);
        assert_eq!(
            t.on_packet(&beacon(5, 1, Route::Limited, 0), 1, 0),
            Event::Ignored
        );
        assert!(t.is_empty());
    }

    #[test]
    fn counts_delivery_per_route() {
        let mut t = PeerTable::new(1);
        for seq in 10..20 {
            t.on_packet(&beacon(2, seq, Route::Directed, 0), 0x0A00_0002, seq);
            if seq % 2 == 0 {
                t.on_packet(&beacon(2, seq, Route::Limited, 0), 0x0A00_0002, seq);
            }
        }
        let p = t.iter().next().unwrap();
        assert_eq!(p.beacons_expected(), 10);
        assert_eq!(p.delivery_pct(Route::Directed), 100);
        assert_eq!(p.delivery_pct(Route::Limited), 50);
    }

    #[test]
    fn records_rtt_from_replies() {
        let mut t = PeerTable::new(1);
        let mut reply = beacon(3, 0, Route::Unicast, 0);
        reply.kind = Kind::Reply;
        for (sent, now) in [(1000, 1004), (2000, 2002), (3000, 3009)] {
            reply.echo_t_ms = sent;
            t.on_packet(&reply, 7, now);
        }
        let p = t.iter().next().unwrap();
        assert_eq!(
            (p.replies, p.rtt.min, p.rtt.max, p.rtt.last, p.rtt.avg()),
            (3, 2, 9, 9, Some(5))
        );
    }

    #[test]
    fn rtt_survives_clock_wrap() {
        let mut t = PeerTable::new(1);
        let mut reply = beacon(3, 0, Route::Unicast, 0);
        reply.kind = Kind::Reply;
        reply.echo_t_ms = u32::MAX - 1;
        assert_eq!(t.on_packet(&reply, 7, 3), Event::Reply { rtt_ms: 5 });
    }

    #[test]
    fn restart_resets_counts() {
        let mut t = PeerTable::new(1);
        t.on_packet(&beacon(2, 100, Route::Limited, 0), 9, 0);
        t.on_packet(&beacon(2, 1, Route::Limited, 0), 9, 1);
        let p = t.iter().next().unwrap();
        assert_eq!((p.first_seq, p.rx_limited), (1, 1));
    }

    #[test]
    fn table_fills_then_reports_full() {
        let mut t = PeerTable::new(0);
        for id in 1..=MAX_PEERS as u32 {
            assert!(matches!(
                t.on_packet(&beacon(id, 1, Route::Limited, 0), id, 0),
                Event::BeaconFrom { .. }
            ));
        }
        assert_eq!(
            t.on_packet(&beacon(99, 1, Route::Limited, 0), 99, 0),
            Event::Full
        );
        assert_eq!(t.len(), MAX_PEERS);
    }

    #[test]
    fn staleness() {
        let mut t = PeerTable::new(0);
        t.on_packet(&beacon(2, 1, Route::Limited, 0), 2, 1_000);
        let p = *t.iter().next().unwrap();
        assert!(!p.is_stale(1_000 + STALE_MS));
        assert!(p.is_stale(1_001 + STALE_MS));
    }
}

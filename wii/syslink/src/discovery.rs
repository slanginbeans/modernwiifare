//! Discovery packets: every Wii broadcasts a `Beacon` once per second on [`DISCOVERY_PORT`];
//! every Wii that hears one answers with a unicast `Reply` echoing the beacon's seq and timestamp,
//! which gives the sender a round-trip time.
//!
//! Wire format (48 bytes, big-endian):
//! ```text
//!  0  magic "MWF"      3   version u8      4  kind u8        5  route u8   6 reserved u16
//!  8  console_id u32  12   seq u32        16  t_ms u32      20  echo_seq u32
//! 24  echo_t_ms u32   28   build u32      32  name [u8;16] (UTF-8, zero padded)
//! ```

pub const MAGIC: [u8; 3] = *b"MWF";
pub const PROTO_VERSION: u8 = 1;
/// IW4's default port; system link only ever runs on a LAN.
pub const DISCOVERY_PORT: u16 = 28960;
pub const NAME_LEN: usize = 16;
pub const PACKET_LEN: usize = 48;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Beacon = 1,
    Reply = 2,
}

/// How the packet was addressed by its sender. Lets the receiver report which broadcast style works.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// 255.255.255.255
    Limited = 0,
    /// subnet-directed, e.g. 192.168.1.255
    Directed = 1,
    Unicast = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    pub kind: Kind,
    pub route: Route,
    pub console_id: u32,
    pub seq: u32,
    pub t_ms: u32,
    pub echo_seq: u32,
    pub echo_t_ms: u32,
    pub build: u32,
    pub name: [u8; NAME_LEN],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    TooShort(usize),
    BadMagic,
    Version(u8),
    BadKind(u8),
    BadRoute(u8),
}

/// Build a zero-padded name field, truncating on a UTF-8 boundary.
pub fn make_name(s: &str) -> [u8; NAME_LEN] {
    let mut out = [0u8; NAME_LEN];
    let mut end = s.len().min(NAME_LEN);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&s.as_bytes()[..end]);
    out
}

/// Name field as text (up to the first NUL; invalid UTF-8 shows as `?`).
pub fn name_str(name: &[u8; NAME_LEN]) -> &str {
    let end = name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    core::str::from_utf8(&name[..end]).unwrap_or("?")
}

impl Packet {
    pub fn encode(&self) -> [u8; PACKET_LEN] {
        let mut b = [0u8; PACKET_LEN];
        b[0..3].copy_from_slice(&MAGIC);
        b[3] = PROTO_VERSION;
        b[4] = self.kind as u8;
        b[5] = self.route as u8;
        // b[6..8] reserved = 0
        b[8..12].copy_from_slice(&self.console_id.to_be_bytes());
        b[12..16].copy_from_slice(&self.seq.to_be_bytes());
        b[16..20].copy_from_slice(&self.t_ms.to_be_bytes());
        b[20..24].copy_from_slice(&self.echo_seq.to_be_bytes());
        b[24..28].copy_from_slice(&self.echo_t_ms.to_be_bytes());
        b[28..32].copy_from_slice(&self.build.to_be_bytes());
        b[32..48].copy_from_slice(&self.name);
        b
    }

    /// Decode a datagram. Longer datagrams are accepted (trailing bytes ignored) so later
    /// protocol versions can append fields.
    pub fn decode(b: &[u8]) -> Result<Packet, DecodeError> {
        if b.len() < PACKET_LEN {
            return Err(DecodeError::TooShort(b.len()));
        }
        if b[0..3] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if b[3] != PROTO_VERSION {
            return Err(DecodeError::Version(b[3]));
        }
        let kind = match b[4] {
            1 => Kind::Beacon,
            2 => Kind::Reply,
            k => return Err(DecodeError::BadKind(k)),
        };
        let route = match b[5] {
            0 => Route::Limited,
            1 => Route::Directed,
            2 => Route::Unicast,
            r => return Err(DecodeError::BadRoute(r)),
        };
        let u = |o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&b[32..48]);
        Ok(Packet {
            kind,
            route,
            console_id: u(8),
            seq: u(12),
            t_ms: u(16),
            echo_seq: u(20),
            echo_t_ms: u(24),
            build: u(28),
            name,
        })
    }

    /// The unicast reply this console should send back for a received beacon.
    pub fn reply_to(
        &self,
        my_id: u32,
        my_build: u32,
        my_name: [u8; NAME_LEN],
        now_ms: u32,
    ) -> Packet {
        Packet {
            kind: Kind::Reply,
            route: Route::Unicast,
            console_id: my_id,
            seq: 0,
            t_ms: now_ms,
            echo_seq: self.seq,
            echo_t_ms: self.t_ms,
            build: my_build,
            name: my_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Packet {
        Packet {
            kind: Kind::Beacon,
            route: Route::Directed,
            console_id: 0xDEAD_BEEF,
            seq: 7,
            t_ms: 123_456,
            echo_seq: 0,
            echo_t_ms: 0,
            build: 0x0102_0304,
            name: make_name("Wii-BEEF"),
        }
    }

    #[test]
    fn roundtrip() {
        let p = sample();
        assert_eq!(Packet::decode(&p.encode()), Ok(p));
    }

    #[test]
    fn wire_layout_is_big_endian() {
        let b = sample().encode();
        assert_eq!(&b[0..6], &[b'M', b'W', b'F', 1, 1, 1]);
        assert_eq!(&b[8..12], &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(&b[12..16], &[0, 0, 0, 7]);
        assert_eq!(&b[28..32], &[1, 2, 3, 4]);
        assert_eq!(&b[32..40], b"Wii-BEEF");
    }

    #[test]
    fn rejects_bad_input() {
        let good = sample().encode();
        assert_eq!(Packet::decode(&good[..47]), Err(DecodeError::TooShort(47)));
        let mut b = good;
        b[0] = b'X';
        assert_eq!(Packet::decode(&b), Err(DecodeError::BadMagic));
        let mut b = good;
        b[3] = 9;
        assert_eq!(Packet::decode(&b), Err(DecodeError::Version(9)));
        let mut b = good;
        b[4] = 0;
        assert_eq!(Packet::decode(&b), Err(DecodeError::BadKind(0)));
        let mut b = good;
        b[5] = 3;
        assert_eq!(Packet::decode(&b), Err(DecodeError::BadRoute(3)));
    }

    #[test]
    fn accepts_longer_datagrams() {
        let mut long = [0u8; 64];
        long[..PACKET_LEN].copy_from_slice(&sample().encode());
        assert_eq!(Packet::decode(&long), Ok(sample()));
    }

    #[test]
    fn reply_echoes_beacon() {
        let r = sample().reply_to(42, 9, make_name("other"), 999);
        assert_eq!(
            (r.kind, r.route, r.console_id),
            (Kind::Reply, Route::Unicast, 42)
        );
        assert_eq!((r.echo_seq, r.echo_t_ms, r.t_ms), (7, 123_456, 999));
    }

    #[test]
    fn names() {
        assert_eq!(name_str(&make_name("Wii-1234")), "Wii-1234");
        assert_eq!(
            name_str(&make_name("a-very-long-console-name")),
            "a-very-long-cons"
        );
        // "é" is 2 bytes; truncation must not split it.
        assert_eq!(name_str(&make_name("abcdefghijklmnoé")), "abcdefghijklmno");
    }
}

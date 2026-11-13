//! IPv4 helpers. An address is a `u32` whose numeric value is `a.b.c.d` = `a<<24 | b<<16 | c<<8 | d`.
//! On the Wii (big-endian) this is also the in-memory network byte order, so it crosses the C shim as-is.

use core::fmt;

pub const LIMITED_BROADCAST: u32 = 0xFFFF_FFFF;

/// Parse dotted-quad text such as `"192.168.1.20"`. Rejects anything else.
pub fn parse_ipv4(s: &str) -> Option<u32> {
    let mut out: u32 = 0;
    let mut parts = 0;
    for part in s.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let v: u32 = part.parse().ok()?;
        if v > 255 {
            return None;
        }
        out = (out << 8) | v;
        parts += 1;
    }
    (parts == 4).then_some(out)
}

/// Subnet-directed broadcast, e.g. 192.168.1.20/24 -> 192.168.1.255.
pub const fn directed_broadcast(ip: u32, mask: u32) -> u32 {
    ip | !mask
}

/// Display wrapper: `Ip(addr)` prints as dotted quad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ip(pub u32);

impl fmt::Display for Ip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0.to_be_bytes();
        // Pad as a whole so `{:<15}` works on the dotted quad.
        let mut buf = [0u8; 15];
        let mut n = 0;
        for (i, octet) in [a, b, c, d].into_iter().enumerate() {
            if i > 0 {
                buf[n] = b'.';
                n += 1;
            }
            let mut digits = [0u8; 3];
            let mut k = 0;
            let mut v = octet;
            loop {
                digits[k] = b'0' + v % 10;
                k += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            while k > 0 {
                k -= 1;
                buf[n] = digits[k];
                n += 1;
            }
        }
        // Only ASCII digits and dots were written.
        f.pad(core::str::from_utf8(&buf[..n]).unwrap_or("?"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;

    #[test]
    fn parse_valid() {
        assert_eq!(parse_ipv4("192.168.1.20"), Some(0xC0A8_0114));
        assert_eq!(parse_ipv4("0.0.0.0"), Some(0));
        assert_eq!(parse_ipv4("255.255.255.255"), Some(LIMITED_BROADCAST));
    }

    #[test]
    fn parse_invalid() {
        for s in [
            "",
            "1.2.3",
            "1.2.3.4.5",
            "256.1.1.1",
            "1..2.3",
            "a.b.c.d",
            "1.2.3.-4",
            "1234.1.1.1",
            " 1.2.3.4",
        ] {
            assert_eq!(parse_ipv4(s), None, "{s:?}");
        }
    }

    #[test]
    fn broadcast() {
        let ip = parse_ipv4("192.168.1.20").unwrap();
        let mask = parse_ipv4("255.255.255.0").unwrap();
        assert_eq!(
            Ip(directed_broadcast(ip, mask)),
            Ip(parse_ipv4("192.168.1.255").unwrap())
        );
        let mask16 = parse_ipv4("255.255.0.0").unwrap();
        assert_eq!(
            directed_broadcast(ip, mask16),
            parse_ipv4("192.168.255.255").unwrap()
        );
    }

    #[test]
    fn display_and_pad() {
        assert_eq!(format!("{}", Ip(0xC0A8_0114)), "192.168.1.20");
        assert_eq!(format!("{}", Ip(0)), "0.0.0.0");
        assert_eq!(format!("[{:<15}]", Ip(0x0A00_0001)), "[10.0.0.1       ]");
    }
}

//! Modern Wiifare system-link protocol.
//!
//! Platform-independent and `no_std`, so it can be unit-tested on the host and linked into the
//! Wii runtime unchanged. All multi-byte fields go on the wire as big-endian, regardless of host.
#![no_std]

#[cfg(test)]
extern crate std;

pub mod addr;
pub mod discovery;
pub mod peers;

/// FNV-1a 32-bit. Used to turn build ids into a compact value carried in packets.
pub const fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u32;
        h = h.wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn fnv_known_vectors() {
        assert_eq!(super::fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(super::fnv1a32(b"a"), 0xe40c_292c);
        assert_eq!(super::fnv1a32(b"foobar"), 0xbf9c_f968);
    }
}

//! Big-endian field access. Readers panic on out-of-range offsets; callers check lengths first (records do).

use alloc::vec::Vec;

pub fn u16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
pub fn u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
pub fn u64(b: &[u8], at: usize) -> u64 {
    (u64::from(u32(b, at)) << 32) | u64::from(u32(b, at + 4))
}
pub fn f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32(b, at))
}

pub fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_be_bytes());
}
pub fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_be_bytes());
}
pub fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_be_bytes());
}

/// Appends big-endian values.
pub trait Push {
    fn u8(&mut self, v: u8);
    fn u16(&mut self, v: u16);
    fn u32(&mut self, v: u32);
    fn f32(&mut self, v: f32);
    fn zeros(&mut self, n: usize);
}

impl Push for Vec<u8> {
    fn u8(&mut self, v: u8) {
        self.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    fn zeros(&mut self, n: usize) {
        self.resize(self.len() + n, 0);
    }
}

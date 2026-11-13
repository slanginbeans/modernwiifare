//! Minimal Rust runtime for the Wii: heap via newlib, panic screen, text console output.

use core::alloc::{GlobalAlloc, Layout};
use core::fmt::{self, Write};

use crate::ffi;

unsafe extern "C" {
    fn memalign(align: usize, size: usize) -> *mut u8;
    fn free(p: *mut u8);
}

/// Heap backed by newlib's malloc (MEM1 arena). Later phases replace this with budgeted pools.
pub struct NewlibAlloc;

unsafe impl GlobalAlloc for NewlibAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { memalign(layout.align().max(8), layout.size()) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { free(ptr) }
    }
}

#[global_allocator]
static ALLOC: NewlibAlloc = NewlibAlloc;

/// Writes to the libogc text console.
pub struct Con;

impl Write for Con {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        unsafe { ffi::mwf_print(s.as_ptr(), s.len()) };
        Ok(())
    }
}

#[macro_export]
macro_rules! say {
    ($($t:tt)*) => {{
        use core::fmt::Write as _;
        let _ = write!($crate::rt::Con, $($t)*);
    }};
}

/// Fixed-size text buffer; formatting past the end is truncated, never panics.
pub struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    pub const fn new() -> Self {
        FixedBuf {
            buf: [0; N],
            len: 0,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl<const N: usize> Default for FixedBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(N - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut msg = FixedBuf::<512>::new();
    let _ = write!(msg, "{info}");
    let b = msg.as_bytes();
    unsafe { ffi::mwf_panic(b.as_ptr(), b.len()) }
}

/// Build id baked in at compile time (the Makefile passes the git hash); both Wiis must match.
pub const BUILD_ID: &str = match option_env!("MWF_BUILD_ID") {
    Some(s) => s,
    None => "dev",
};

//! Minimal runtime: heap via newlib (MEM1 arena), panic screen. Big data (the pack) lives in MEM2 via the C shim.

use core::alloc::{GlobalAlloc, Layout};
use core::fmt::{self, Write};

use crate::ffi;

unsafe extern "C" {
    fn memalign(align: usize, size: usize) -> *mut u8;
    fn free(p: *mut u8);
}

struct NewlibAlloc;

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

struct Msg {
    buf: [u8; 512],
    len: usize,
}

impl Write for Msg {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut m = Msg {
        buf: [0; 512],
        len: 0,
    };
    let _ = write!(m, "{info}");
    unsafe { ffi::mwf_panic(m.buf.as_ptr(), m.len) }
}

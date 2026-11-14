//! Minimal runtime: heap via newlib (MEM1 arena), panic screen. Big data (the pack) lives in MEM2 via the C shim.

use core::alloc::{GlobalAlloc, Layout};
use core::fmt::{self, Write};

use crate::ffi;

unsafe extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn memalign(align: usize, size: usize) -> *mut u8;
    fn realloc(p: *mut u8, size: usize) -> *mut u8;
    fn free(p: *mut u8);
}

/// newlib's `malloc` returns 8-byte aligned blocks.
const MALLOC_ALIGN: usize = 8;

struct NewlibAlloc;

unsafe impl GlobalAlloc for NewlibAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGN {
            unsafe { malloc(layout.size()) }
        } else {
            unsafe { memalign(layout.align(), layout.size()) }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { free(ptr) }
    }
    /// Grow or shrink in place when newlib can (a Vec growing no longer always copies).
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGN {
            return unsafe { realloc(ptr, new_size) };
        }
        let new = unsafe { memalign(layout.align(), new_size) };
        if !new.is_null() {
            unsafe { core::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size)) };
            unsafe { free(ptr) };
        }
        new
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
    // Into the test log first (the panic screen never returns).
    crate::devlog::panic(core::str::from_utf8(&m.buf[..m.len]).unwrap_or("?"));
    unsafe { ffi::mwf_panic(m.buf.as_ptr(), m.len) }
}

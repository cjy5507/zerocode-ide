//! The counting allocator the memory tests read.
//!
//! One per test binary that declares `mod support;`: each binary is its own
//! process with its own global allocator, so the counts one binary reads are
//! never moved by another. Within a binary every test shares them, which is
//! why a binary that reads them keeps its measurements in one test, or behind
//! one lock.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAllocator;

/// Allocations made since the last reset.
pub static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
/// Bytes asked for and not yet given back — the size each caller requested,
/// before the allocator rounds it up to its own size class.
pub static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

/// What is held right now.
pub fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

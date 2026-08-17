//! A counting global allocator, for proving invariant I-9.
//!
//! I-9 says a steady-state frame allocates nothing: `Scene::reset` keeps its
//! buffers, so the second frame reuses the first frame's memory. That is not
//! something a benchmark can show — a frame that allocates once is still fast.
//! It needs a counter on the allocator itself.
//!
//! # Why the counters are thread-local
//!
//! `cargo test` runs test functions on parallel threads. Global counters would
//! mean one test's allocations landing in another's measurement, which shows up
//! as a flaky "expected 0 allocations, got 3". Per-thread counters make each
//! measurement independent of what else is running.
//!
//! The counters are `Cell<u64>` in a `const`-initialised `thread_local!`, which
//! matters: a lazily initialised or destructor-carrying thread local allocates
//! on first access, and allocating inside the allocator recurses forever.
#![allow(
    unsafe_code,
    reason = "a GlobalAlloc implementation cannot be written without it"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static REALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

/// A snapshot of one thread's allocator activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// Calls to `alloc` and `alloc_zeroed`.
    pub allocations: u64,
    /// Calls to `realloc`. A `Vec` growing past its capacity shows up here,
    /// which is exactly the I-9 violation being watched for.
    pub reallocations: u64,
    /// Calls to `dealloc`.
    pub deallocations: u64,
    /// Bytes requested by `alloc`, `alloc_zeroed` and `realloc` growth.
    pub bytes: u64,
}

impl Counters {
    /// This thread's counters right now.
    pub fn snapshot() -> Counters {
        Counters {
            allocations: ALLOCATIONS.get(),
            reallocations: REALLOCATIONS.get(),
            deallocations: DEALLOCATIONS.get(),
            bytes: BYTES.get(),
        }
    }

    /// Activity since `earlier`.
    pub fn since(self, earlier: Counters) -> Counters {
        Counters {
            allocations: self.allocations - earlier.allocations,
            reallocations: self.reallocations - earlier.reallocations,
            deallocations: self.deallocations - earlier.deallocations,
            bytes: self.bytes - earlier.bytes,
        }
    }

    /// True when nothing was allocated, reallocated or freed.
    pub fn is_quiet(&self) -> bool {
        self.allocations == 0 && self.reallocations == 0 && self.deallocations == 0
    }

    /// Allocations plus reallocations — the count that I-9 requires to be zero.
    pub fn acquisitions(&self) -> u64 {
        self.allocations + self.reallocations
    }
}

/// Runs `body` and reports what it allocated on this thread.
///
/// ```ignore
/// let (scene, counters) = measure(|| build_frame());
/// assert_eq!(counters.acquisitions(), 0, "I-9: steady-state frame allocated");
/// ```
pub fn measure<T>(body: impl FnOnce() -> T) -> (T, Counters) {
    let before = Counters::snapshot();
    let value = body();
    let after = Counters::snapshot();
    (value, after.since(before))
}

/// Wraps a global allocator and counts what passes through it.
///
/// Install it in a test binary:
///
/// ```ignore
/// #[global_allocator]
/// static ALLOC: CountingAllocator = CountingAllocator::new();
/// ```
#[derive(Debug, Default)]
pub struct CountingAllocator {
    inner: System,
}

impl CountingAllocator {
    /// A counting allocator over the system allocator.
    pub const fn new() -> CountingAllocator {
        CountingAllocator { inner: System }
    }
}

/// Bumps a counter without ever allocating.
///
/// `try_with` rather than `with`: during thread teardown the local is gone, and
/// `with` would panic from inside the allocator.
#[inline]
fn bump(cell: &'static std::thread::LocalKey<Cell<u64>>, by: u64) {
    let _ = cell.try_with(|c| c.set(c.get().wrapping_add(by)));
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCATIONS, 1);
        bump(&BYTES, layout.size() as u64);
        unsafe { self.inner.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCATIONS, 1);
        bump(&BYTES, layout.size() as u64);
        unsafe { self.inner.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump(&DEALLOCATIONS, 1);
        unsafe { self.inner.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(&REALLOCATIONS, 1);
        bump(&BYTES, new_size.saturating_sub(layout.size()) as u64);
        unsafe { self.inner.realloc(ptr, layout, new_size) }
    }
}

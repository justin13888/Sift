//! D-24 — the subsystem tag travels in the allocation.
//!
//! # Why explicit cache accounting is the primary mechanism and this is not
//!
//! Every cache declares a byte budget, an eviction policy, and reports its live size.
//! That is where the numbers come from. **This allocator exists to catch memory that is
//! not in a declared cache** — the *residual*, total footprint minus the sum of declared
//! caches, which is what a maintainer chases when NFR-45's slope gate fires and NFR-12
//! says the footprint is ratcheting.
//!
//! # Why the tag has to travel with the allocation
//!
//! Two ways of doing this look equivalent and are not, and both failures are silent.
//!
//! **Asymmetry.** The pipeline hands one buffer from the MIME parser to the sanitizer to
//! the blocker to the broker. Tagging only on allocation means the subsystem that
//! allocated is charged and the subsystem that frees is credited — so every hand-off leaks
//! a counter, and over NFR-12's fourteen days the divergence is unbounded. Freeing has to
//! decrement whoever allocated, which means the tag has to be *in the allocation*.
//!
//! **Task migration.** D-19 runs one multi-threaded work-stealing runtime. A bare
//! thread-local set once per operation is wrong at every await point, because the task
//! resumes on another thread. **The tag is task-scoped and re-established at every poll**,
//! which is sound precisely because a task is not stolen mid-poll.
//!
//! # What it costs
//!
//! A header word on every allocation, alignment care, and per-CPU sharded counters rather
//! than one contended atomic. NFR-44 bounds the whole mechanism at **2% in release**, in
//! counters-only mode — which is what makes it acceptable to leave on in release, and
//! leaving it on is what makes the residual observable on a user's machine rather than
//! only on a developer's.
//!
//! Per-allocation stack capture is a different thing entirely and is **development-only,
//! behind a feature flag**, because it does not fit inside NFR-44.
//!
//! # The alternative D-24 records
//!
//! Per-subsystem heaps would give symmetry for free — but a buffer would then have to be
//! freed to the heap it came from, which is the same problem wearing different clothes. If
//! the pipeline ends up copying at stage boundaries anyway, heaps become the better answer.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use core::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use sift_subsystem::Subsystem;

/// How many counter shards. One contended atomic per subsystem would make the allocator
/// its own scalability problem on exactly the workload D-19's runtime is for.
const SHARDS: usize = 16;

#[expect(
    clippy::declare_interior_mutable_const,
    reason = "the array initialiser needs it"
)]
const ZERO: AtomicI64 = AtomicI64::new(0);
#[expect(
    clippy::declare_interior_mutable_const,
    reason = "the array initialiser needs it"
)]
const ZERO_ROW: [AtomicI64; Subsystem::COUNT] = [ZERO; Subsystem::COUNT];

/// Live bytes, per shard per subsystem. Signed, because a shard can go negative when a
/// buffer allocated on one thread is freed on another — only the sum across shards is
/// meaningful.
static LIVE: [[AtomicI64; Subsystem::COUNT]; SHARDS] = [ZERO_ROW; SHARDS];

static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The current subsystem. `const`-initialised so that reading it never allocates —
    /// a lazily initialised thread-local would recurse into the allocator on first use.
    static TAG: Cell<u8> = const { Cell::new(0) };
    /// This thread's counter shard, assigned on first use.
    static SHARD: Cell<u16> = const { Cell::new(u16::MAX) };
}

/// The tag currently in force on this thread.
#[must_use]
pub fn current() -> Subsystem {
    let i = TAG.with(Cell::get) as usize;
    Subsystem::ALL.get(i).copied().unwrap_or(Subsystem::Runtime)
}

/// Establish the tag for the duration of `f`, restoring the previous one afterwards.
///
/// **This is what a task calls at every poll**, not once when it is spawned. Restoring
/// rather than clearing is what makes it composable: the pipeline changes tag at a stage
/// boundary and changes back, inside whatever tag the caller had.
pub fn tagged<R>(subsystem: Subsystem, f: impl FnOnce() -> R) -> R {
    let previous = TAG.with(Cell::get);
    TAG.with(|t| t.set(u8::try_from(subsystem.index()).unwrap_or(0)));
    // A panic here would leave the tag set for whatever runs next on this thread, which
    // would silently misattribute it. D-47 catches panics at stage boundaries — which is
    // exactly where this is called — so the guard restores on unwind.
    let guard = Restore(previous);
    let r = f();
    drop(guard);
    r
}

struct Restore(u8);
impl Drop for Restore {
    fn drop(&mut self) {
        TAG.with(|t| t.set(self.0));
    }
}

fn shard() -> usize {
    let s = SHARD.with(Cell::get);
    if s != u16::MAX {
        return s as usize;
    }
    let assigned = NEXT_SHARD.fetch_add(1, Ordering::Relaxed) % SHARDS;
    SHARD.with(|c| c.set(u16::try_from(assigned).unwrap_or(0)));
    assigned
}

/// Live bytes attributed to a subsystem, summed across shards.
#[must_use]
pub fn live_bytes(subsystem: Subsystem) -> i64 {
    (0..SHARDS)
        .map(|s| LIVE[s][subsystem.index()].load(Ordering::Relaxed))
        .sum()
}

/// Live bytes for every subsystem, in [`Subsystem::ALL`] order.
#[must_use]
pub fn snapshot() -> [i64; Subsystem::COUNT] {
    let mut out = [0i64; Subsystem::COUNT];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (0..SHARDS)
            .map(|s| LIVE[s][i].load(Ordering::Relaxed))
            .sum();
    }
    out
}

/// The total this allocator accounts for. The *residual* is the process footprint minus
/// this, and it is the number that matters when the slope gate fires.
#[must_use]
pub fn total_attributed() -> i64 {
    snapshot().iter().sum()
}

/// The header word stored immediately before every returned pointer.
const HEADER: usize = size_of::<usize>();

/// How far the user pointer sits past the block's real start.
///
/// At least a header word, and at least the requested alignment, so that the returned
/// pointer is aligned and the header is entirely within the block.
const fn offset(align: usize) -> usize {
    if align > HEADER { align } else { HEADER }
}

fn inner_layout(layout: Layout) -> Option<(Layout, usize)> {
    let off = offset(layout.align());
    let size = layout.size().checked_add(off)?;
    Layout::from_size_align(size, layout.align())
        .ok()
        .map(|l| (l, off))
}

/// A global allocator that records which subsystem owns each allocation.
///
/// Wraps another allocator rather than implementing one: D-20 chooses mimalloc for the
/// purge control NFR-12 needs, and this is orthogonal to that choice.
#[derive(Debug)]
pub struct Tagging<A>(pub A);

// SAFETY: every returned pointer is the base pointer advanced by `offset(align)`, which is
// a multiple of the requested alignment, so it is correctly aligned. `dealloc` recomputes
// the same offset from the same layout, so it always frees the block it was given.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Tagging<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((inner, off)) = inner_layout(layout) else {
            return core::ptr::null_mut();
        };
        // SAFETY: `inner` is a valid layout with non-zero size, since `off >= HEADER > 0`.
        let base = unsafe { self.0.alloc(inner) };
        if base.is_null() {
            return base;
        }
        let tag = TAG.with(Cell::get);
        // SAFETY: the block is at least `off` bytes, and the header sits in the word
        // immediately before the user pointer, which is inside it.
        unsafe {
            base.add(off - HEADER)
                .cast::<usize>()
                .write_unaligned(tag as usize);
        }
        record(tag, layout.size() as i64);
        // SAFETY: `off <= inner.size()`, so this stays within the allocation.
        unsafe { base.add(off) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let Some((inner, off)) = inner_layout(layout) else {
            return;
        };
        // SAFETY: `ptr` came from `alloc` with this same layout, so `ptr - off` is the base.
        let base = unsafe { ptr.sub(off) };
        // SAFETY: the header was written here by `alloc`.
        let tag = unsafe { base.add(off - HEADER).cast::<usize>().read_unaligned() } as u8;
        // Decrement the subsystem that *allocated*, which is the whole point: the buffer
        // may have been handed across three stages since.
        record(tag, -(layout.size() as i64));
        // SAFETY: `inner` is the layout the block was allocated with.
        unsafe { self.0.dealloc(base, inner) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Deliberately not delegating to the inner allocator's realloc: doing so would
        // move the block without moving the header, and the tag would be read from
        // whatever followed. Allocate, copy, free — correctness over one memcpy.
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return core::ptr::null_mut();
        };
        // SAFETY: delegating to this allocator's own alloc.
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            // SAFETY: both are valid for the smaller of the two sizes.
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
    }
}

fn record(tag: u8, delta: i64) {
    let i = tag as usize;
    if i < Subsystem::COUNT {
        LIVE[shard()][i].fetch_add(delta, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::System;

    /// The counters are process-global, and tests run in parallel. Each test therefore
    /// measures a *delta* against its own subsystem rather than an absolute, and no two
    /// tests share one — which is why there are eighteen to choose from.
    const A: Tagging<System> = Tagging(System);

    fn alloc(layout: Layout) -> *mut u8 {
        unsafe { A.alloc(layout) }
    }
    fn dealloc(p: *mut u8, layout: Layout) {
        unsafe { A.dealloc(p, layout) };
    }

    #[test]
    fn a_pointer_is_aligned_for_every_alignment_a_layout_can_ask_for() {
        // The header is stored before the user pointer, so the offset has to be a multiple
        // of the alignment as well as at least a word. Getting this wrong produces
        // misaligned pointers that work on x86 and fault elsewhere.
        for align_shift in 0..12 {
            let align = 1usize << align_shift;
            let layout = Layout::from_size_align(64, align).unwrap();
            let p = alloc(layout);
            assert!(!p.is_null(), "allocation failed at align {align}");
            assert_eq!(p as usize % align, 0, "misaligned at align {align}");
            dealloc(p, layout);
        }
    }

    #[test]
    fn bytes_are_attributed_to_the_tag_in_force() {
        let before = live_bytes(Subsystem::Index);
        let layout = Layout::from_size_align(1024, 8).unwrap();
        let p = tagged(Subsystem::Index, || alloc(layout));
        assert_eq!(live_bytes(Subsystem::Index) - before, 1024);
        dealloc(p, layout);
        assert_eq!(live_bytes(Subsystem::Index) - before, 0);
    }

    #[test]
    fn freeing_credits_whoever_allocated_not_whoever_freed() {
        // The asymmetry that makes the tag travel *in* the allocation. The pipeline hands
        // one buffer from Parse to Sanitize to Broker; if the freeing subsystem were
        // credited, every hand-off would leak a counter and NFR-12's fourteen days would
        // diverge without bound.
        let parse_before = live_bytes(Subsystem::Parse);
        let broker_before = live_bytes(Subsystem::Broker);

        let layout = Layout::from_size_align(4096, 16).unwrap();
        let buffer = tagged(Subsystem::Parse, || alloc(layout));
        assert_eq!(live_bytes(Subsystem::Parse) - parse_before, 4096);

        // Handed downstream and freed there.
        tagged(Subsystem::Broker, || dealloc(buffer, layout));

        assert_eq!(
            live_bytes(Subsystem::Parse) - parse_before,
            0,
            "the allocating subsystem was not credited back"
        );
        assert_eq!(
            live_bytes(Subsystem::Broker) - broker_before,
            0,
            "the freeing subsystem was charged for somebody else's buffer"
        );
    }

    #[test]
    fn the_tag_is_restored_rather_than_cleared() {
        // Composability: the pipeline changes tag at a stage boundary and changes back,
        // inside whatever tag the caller already had.
        tagged(Subsystem::Sync, || {
            assert_eq!(current(), Subsystem::Sync);
            tagged(Subsystem::Store, || assert_eq!(current(), Subsystem::Store));
            assert_eq!(current(), Subsystem::Sync);
        });
    }

    #[test]
    fn a_panic_does_not_leave_the_tag_set() {
        // D-47 catches panics at exactly the stage boundaries this is called from. A tag
        // left set would silently misattribute everything that ran next on this thread.
        let outer = current();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(|| tagged(Subsystem::Sanitize, || panic!("stage failed")));
        std::panic::set_hook(hook);
        assert!(r.is_err());
        assert_eq!(current(), outer, "the tag survived a caught panic");
    }

    #[test]
    fn reallocating_keeps_the_accounting_straight() {
        // realloc deliberately does not delegate: the inner allocator would move the block
        // without moving the header, and the tag would then be read from whatever followed.
        let before = live_bytes(Subsystem::Blobs);
        let small = Layout::from_size_align(64, 8).unwrap();
        let p = tagged(Subsystem::Blobs, || alloc(small));
        unsafe { core::ptr::write_bytes(p, 0xAB, 64) };

        let p2 = tagged(Subsystem::Blobs, || unsafe { A.realloc(p, small, 4096) });
        assert!(!p2.is_null());
        assert_eq!(live_bytes(Subsystem::Blobs) - before, 4096);

        // The contents survived the move.
        let copied = unsafe { core::slice::from_raw_parts(p2, 64) };
        assert!(copied.iter().all(|b| *b == 0xAB), "realloc lost the data");

        dealloc(p2, Layout::from_size_align(4096, 8).unwrap());
        assert_eq!(live_bytes(Subsystem::Blobs) - before, 0);
    }

    #[test]
    fn allocations_balance_across_threads() {
        // Counters are sharded per thread and a shard can go negative when a buffer
        // crosses threads. Only the sum is meaningful, and the sum must return to zero.
        let before = live_bytes(Subsystem::Adapters);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    let layout = Layout::from_size_align(512, 8).unwrap();
                    for _ in 0..500 {
                        let p = tagged(Subsystem::Adapters, || alloc(layout));
                        dealloc(p, layout);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(live_bytes(Subsystem::Adapters) - before, 0);
    }

    #[test]
    fn a_buffer_allocated_on_one_thread_and_freed_on_another_still_balances() {
        let before = live_bytes(Subsystem::Mutations);
        let layout = Layout::from_size_align(2048, 8).unwrap();
        let p = tagged(Subsystem::Mutations, || alloc(layout)) as usize;
        std::thread::spawn(move || dealloc(p as *mut u8, layout))
            .join()
            .unwrap();
        assert_eq!(live_bytes(Subsystem::Mutations) - before, 0);
    }

    #[test]
    fn the_default_tag_is_a_real_subsystem() {
        // Anything allocated before a tag is established — process startup, the runtime's
        // own structures — has to land somewhere in an exhaustive partition rather than
        // nowhere.
        std::thread::spawn(|| {
            assert_eq!(current(), Subsystem::Shell);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_zero_sized_request_does_not_return_null() {
        // Rust never asks for a zero-size allocation through GlobalAlloc, but the header
        // arithmetic must not overflow or return null if one arrives.
        let layout = Layout::from_size_align(0, 1).unwrap();
        let p = alloc(layout);
        assert!(!p.is_null());
        dealloc(p, layout);
    }

    #[test]
    fn a_layout_that_overflows_once_the_header_is_added_fails_rather_than_wrapping() {
        // The largest layout that is constructible on its own. Adding the header offset
        // pushes the inner layout past what `Layout` permits, and the allocator must return
        // null rather than wrapping into a small allocation — which would be a heap
        // overflow presented as a successful allocation.
        let largest = (isize::MAX as usize) - 7;
        let layout = Layout::from_size_align(largest, 8).expect("constructible on its own");
        assert!(
            Layout::from_size_align(largest + HEADER, 8).is_err(),
            "the premise of this test no longer holds"
        );
        assert!(alloc(layout).is_null());
    }

    #[test]
    fn the_snapshot_covers_every_subsystem() {
        assert_eq!(snapshot().len(), Subsystem::COUNT);
    }
}

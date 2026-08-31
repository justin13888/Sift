//! The allocator installed as *the* global allocator, which is the only configuration it
//! will ever actually run in.
//!
//! The unit tests exercise it as an ordinary object. That misses the constraints that make
//! a global allocator different and that fail in production rather than in a test harness:
//! it must not recurse into itself, it must survive being called before `main`, and it must
//! survive being called during thread teardown after thread-local destructors have run.
//!
//! This file is a separate integration test because `#[global_allocator]` is per-binary.

use sift_alloc::{Tagging, current, live_bytes, tagged, total_attributed};
use sift_subsystem::Subsystem;

#[global_allocator]
static ALLOC: Tagging<std::alloc::System> = Tagging(std::alloc::System);

#[test]
fn ordinary_allocation_works_at_all() {
    // If the header arithmetic were wrong this test binary would not have started, so
    // reaching here is already the strongest signal. Assert something anyway.
    let v: Vec<u64> = (0..10_000).collect();
    assert_eq!(v.iter().sum::<u64>(), 49_995_000);
    let s: String = std::iter::repeat_n('x', 100_000).collect();
    assert_eq!(s.len(), 100_000);
}

#[test]
fn work_done_under_a_tag_is_attributed_to_it() {
    let before = live_bytes(Subsystem::Index);
    let held = tagged(Subsystem::Index, || vec![0u8; 1 << 20]);
    let during = live_bytes(Subsystem::Index) - before;
    assert!(
        during >= (1 << 20),
        "a megabyte under the Index tag reported {during}"
    );
    drop(held);
    assert!(
        live_bytes(Subsystem::Index) - before < (1 << 20),
        "the allocation was not credited back on drop"
    );
}

#[test]
fn nested_collections_do_not_recurse_into_the_allocator() {
    // The failure mode this catches is an allocator that allocates on its hot path — a map,
    // a lock, a lazily initialised thread-local. Any of those recurses infinitely the first
    // time it is touched, and the symptom is a stack overflow at startup rather than a
    // failing assertion.
    let nested: Vec<Vec<String>> = (0..200)
        .map(|i| (0..50).map(|j| format!("{i}-{j}")).collect())
        .collect();
    assert_eq!(nested.len(), 200);
    assert_eq!(nested[199][49], "199-49");
}

#[test]
fn a_thread_that_exits_while_holding_memory_does_not_fault() {
    // Thread teardown runs after thread-local destructors. An allocator whose counters live
    // behind a thread-local *with* a destructor panics here with "cannot access a TLS value
    // during or after destruction". The shard and tag cells are `const`-initialised
    // precisely so they have no destructor.
    for _ in 0..32 {
        std::thread::spawn(|| {
            let _held: Vec<Vec<u8>> = (0..100).map(|n| vec![0u8; n * 64]).collect();
            tagged(Subsystem::Sync, || {
                let _more = vec![0u8; 4096];
            });
        })
        .join()
        .expect("a thread faulted during teardown");
    }
}

#[test]
fn buffers_handed_between_subsystems_balance() {
    // The pipeline's shape: allocate under Parse, hand downstream, free under Broker.
    let parse_before = live_bytes(Subsystem::Parse);
    let broker_before = live_bytes(Subsystem::Broker);

    for _ in 0..1_000 {
        let buffer = tagged(Subsystem::Parse, || vec![0u8; 8192]);
        tagged(Subsystem::Broker, || drop(buffer));
    }

    assert_eq!(live_bytes(Subsystem::Parse) - parse_before, 0);
    assert_eq!(live_bytes(Subsystem::Broker) - broker_before, 0);
}

#[test]
fn the_total_is_positive_and_finite() {
    // The residual — footprint minus this total — is the number NFR-45's slope gate sends a
    // maintainer to look at. A total that is negative or absurd means the accounting is
    // broken and the residual is meaningless.
    let total = total_attributed();
    assert!(total > 0, "nothing is attributed at all: {total}");
    assert!(total < 1 << 40, "attributed a terabyte: {total}");
}

#[test]
fn the_tag_survives_a_realloc_heavy_workload() {
    // Vec growth is the realloc path, which deliberately does not delegate to the inner
    // allocator. A leak here would show as a counter that never returns.
    let before = live_bytes(Subsystem::Store);
    tagged(Subsystem::Store, || {
        let mut v: Vec<u64> = Vec::new();
        for i in 0..100_000u64 {
            v.push(i);
        }
        assert_eq!(v.len(), 100_000);
    });
    assert_eq!(
        live_bytes(Subsystem::Store) - before,
        0,
        "realloc leaked attribution"
    );
}

#[test]
fn the_default_tag_is_stable_on_the_main_thread() {
    assert_eq!(current(), Subsystem::Shell);
}

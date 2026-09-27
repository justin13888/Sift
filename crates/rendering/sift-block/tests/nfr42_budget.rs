//! NFR-42 — the filter engine at or under 40 MB with the three standard lists loaded.
//!
//! "In every build, however those lists arrive": a build without the `public-lists` feature
//! still owes the budget, because its user can import the same lists as custom rules. So
//! this reads the vendored text directly rather than through the feature, and holds in every
//! build this workspace produces.
//!
//! The figure is the engine's **live heap after construction**, counted by D-24's allocator
//! under the Broker tag the filter engine is attributed to — every byte the `Blocker` keeps,
//! both layers and the compiled rules, and nothing it only used while parsing. It is a heap
//! count on the machine the test runs on, not a footprint on the reference environment, so a
//! pass here is evidence for the hypothesis rather than its validation.
//!
//! It is also not the peak. Building the engine passes through the parsed content rules,
//! several times the size of what is kept, before they are serialized and dropped; that
//! transient is outside what this counts.
//!
//! A separate integration test because `#[global_allocator]` is per-binary.

use sift_alloc::{Tagging, live_bytes, tagged};
use sift_block::engine::{BUNDLED_EMAIL_LIST, Blocker};
use sift_subsystem::Subsystem;

#[global_allocator]
static ALLOC: Tagging<std::alloc::System> = Tagging(std::alloc::System);

const EASYLIST: &str = include_str!("../lists/easylist.txt");
const EASYPRIVACY: &str = include_str!("../lists/easyprivacy.txt");

/// NFR-42's 40 MB, in bytes.
const BUDGET: i64 = 40 * 1000 * 1000;

#[test]
fn the_engine_with_all_three_lists_stays_inside_nfr42() {
    let before = live_bytes(Subsystem::Broker);
    let blocker = tagged(Subsystem::Broker, || {
        Blocker::from_lists(&[BUNDLED_EMAIL_LIST, EASYLIST, EASYPRIVACY])
    });
    let held = live_bytes(Subsystem::Broker) - before;

    assert!(
        held <= BUDGET,
        "the engine holds {held} bytes with the three lists loaded, over NFR-42's {BUDGET}"
    );
    // Not a vacuous pass: the lists were parsed, and the count covers what they became.
    assert!(
        held > 1_000_000,
        "only {held} bytes were attributed; the engine was not counted"
    );
    assert!(blocker.compiled_rule_count() > 10_000, "{blocker:?}");

    drop(blocker);
}

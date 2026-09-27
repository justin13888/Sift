//! NFR-40 method 4 — the sanitizer under fuzzing, seeded with the fidelity corpus.
//!
//! Per input, one of two outcomes is acceptable and nothing else is:
//!
//! - **A bound refuses it.** [`sanitize`] returns an error from the limits register (L-6,
//!   L-7, L-8), and the message falls back to FR-9's raw view.
//! - **It is accepted, and the output is clean.** [`audit`] of the output finds nothing an
//!   invariant forbids in the tree **as the engine will build it**, and the output is
//!   parse-stable (I8): what `check_parse_stability` asserts holds — sanitizing the output
//!   again changes nothing — unless a bound refuses the output.
//!
//! A panic anywhere is a finding in its own right (NFR-19): libFuzzer reports it as a crash.
//! Every finding is minimized and admitted to `fixtures/mxss/vectors.txt` as a permanent
//! vector (NFR-40 method 5), with the fix, before the fuzzer is run again.
//!
//! The input is bytes and the sanitizer takes text. Invalid UTF-8 is replaced rather than
//! skipped, so no input the fuzzer spends time on is wasted — and the tree builder sees what
//! the pipeline's decode stage would give it (I10).

#![no_main]

use libfuzzer_sys::fuzz_target;
use sift_sanitize::audit::audit;
use sift_sanitize::sanitize::sanitize;

fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);

    let Ok(once) = sanitize(&html) else {
        // Refused by a bound: the raw view, which is a correct outcome rather than a finding.
        return;
    };

    let violations = audit(&once.html);
    assert!(
        violations.is_empty(),
        "the sanitized output violates an invariant as the engine builds it: {violations:?}\n\
         output: {:?}",
        once.html,
    );

    // I8, as `check_parse_stability` states it — without sanitizing the input a second time,
    // which would halve the executions a bounded run reaches. A bound refusing the output is
    // the same raw-view outcome as refusing the input.
    if let Ok(twice) = sanitize(&once.html) {
        assert!(
            twice.html == once.html,
            "I8: the sanitized output reparses into a different document\n\
             output: {:?}\nagain:  {:?}",
            once.html,
            twice.html,
        );
    }
});

//! NFR-40 method 4 — the sanitizer under fuzzing, seeded with the fidelity corpus.
//!
//! Per input, one of two outcomes is acceptable and nothing else is:
//!
//! - **A size bound refuses it.** [`sanitize`] returns L-6, L-7 or L-8's error, and the
//!   message falls back to FR-9's raw view.
//! - **It is accepted, and the output is clean.** [`audit`] of the output finds nothing an
//!   invariant forbids in the tree **as the engine will build it**.
//!
//! I8 needs no assertion of its own here: under D-121, [`sanitize`] returns only a fixed
//! point of its policy pass, so an accepted output is parse-stable by the function's own
//! return condition, and sanitizing it again could only repeat that comparison. What I8
//! failure looks like now is [`SanitizeError::Unstable`] — an input whose output did not
//! settle within L-35's passes. The pipeline sends that message to the raw view, which is
//! safe, but here it is a **finding**: the pass count is a hypothesis D-121 says one re-pass
//! has so far justified, and an input that exhausts the margin is the evidence against it.
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
use sift_sanitize::sanitize::{SanitizeError, sanitize};

fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);

    let once = match sanitize(&html) {
        Ok(once) => once,
        Err(SanitizeError::Unstable) => {
            panic!("I8: the output did not settle within L-35's passes (D-121)\ninput: {html:?}")
        }
        // Refused by a size bound: the raw view, which is a correct outcome rather than a
        // finding.
        Err(
            SanitizeError::TooDeep | SanitizeError::TooManyNodes | SanitizeError::TooManyAttributes,
        ) => {
            return;
        }
    };

    let violations = audit(&once.html);
    assert!(
        violations.is_empty(),
        "the sanitized output violates an invariant as the engine builds it: {violations:?}\n\
         output: {:?}",
        once.html,
    );
});

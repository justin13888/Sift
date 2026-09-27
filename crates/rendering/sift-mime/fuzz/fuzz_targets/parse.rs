//! NFR-40 method 4 — the MIME parser under fuzzing, seeded with the fidelity corpus.
//!
//! Per input, the parser either rejects it with a [`ParseError`] — FR-9's raw view — or
//! returns a structure that honours every bound the limits register sets on it. Nothing it
//! returns may point outside the bytes it was given, because stage 2 slices the selected
//! part's body out of those bytes by that range. A panic anywhere is a finding (NFR-19).
//!
//! Selection runs over every accepted structure too: it is the stage that consumes the tree,
//! and the one whose choice the debug view reports.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sift_foundation::limits::{L2_MIME_PARTS, L3_MIME_DEPTH, L5_HEADER_FIELDS};
use sift_mime::parse::{Part, parse};
use sift_mime::select::select;

fuzz_target!(|data: &[u8]| {
    let Ok(message) = parse(data) else {
        // Rejected to the raw view: a correct outcome rather than a finding.
        return;
    };

    assert!(
        message.headers.len() as u64 <= L5_HEADER_FIELDS,
        "L-5: {} top-level header fields accepted",
        message.headers.len(),
    );

    let parts = message.root.walk();
    assert!(
        parts.len() as u64 <= L2_MIME_PARTS,
        "L-2: {} parts accepted",
        parts.len(),
    );
    assert!(
        depth(&message.root) <= L3_MIME_DEPTH,
        "L-3: a tree {} levels deep accepted",
        depth(&message.root),
    );

    for part in parts {
        assert!(
            part.body.start <= part.body.end && part.body.end <= data.len(),
            "a {} part's body {:?} is not a range within the {} bytes parsed",
            part.content_type(),
            part.body,
            data.len(),
        );
        assert!(
            part.headers.len() as u64 <= L5_HEADER_FIELDS,
            "L-5: a part with {} header fields accepted",
            part.headers.len(),
        );
    }

    let selection = select(&message);
    if let Some(chosen) = selection.chosen {
        // What stage 2 hands on is a slice of the input; it must be one.
        let _ = &data[chosen.body.clone()];
    }
});

/// Levels below the root, counted the way L-3 counts them: the root is level zero.
fn depth(part: &Part) -> u64 {
    part.children
        .iter()
        .map(|c| 1 + depth(c))
        .max()
        .unwrap_or(0)
}

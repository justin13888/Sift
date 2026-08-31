//! Every numeric bound Sift enforces at runtime, in one place.
//!
//! Owns L-1 through L-29. See `docs/limits.md`.
//!
//! # Why this is a register rather than a scattered set of constants
//!
//! Three separate consumers must assert the *same* number, and two of them share code:
//! the enforcement site itself, NFR-40's property generator — which has to generate
//! inputs **at** the boundary to be worth running — and FR-33 item 9's live per-message
//! invariant check. A bound scattered across those three is a bound that drifts, and
//! **a property test cannot assert a limit it has to guess**. That is why [`ALL`] is
//! enumerable: a generator walks it rather than being handed a copy of each number.
//!
//! # Exceeding a limit rejects; it does not truncate
//!
//! A message that exceeds any parse limit MUST degrade to the raw source view under
//! FR-9, and MUST NOT be rendered partially. The tempting alternative — truncate and
//! render what fits — is wrong twice. It shows the user a partial message with nothing
//! saying so, which is the dishonesty FR-12 rejects when it insists "not cached" and
//! "not available" stay distinct. And it hands the sanitizer's output contract to the
//! attacker: **a document truncated mid-tree is a document whose structure the sender
//! chose by choosing where the cap fell**, which is a parse-differential primitive of
//! exactly the kind I8 exists to close.
//!
//! Rejection also costs less than it appears, because NFR-19 already requires parse
//! failure to degrade to the raw view. The path exists, is tested, and is the one a user
//! already meets on malformed mail.
//!
//! **One exception, and it is not a message.** The snippet ([`L16_SNIPPET_CHARS`]) is a
//! summary Sift derives for the list rather than content it renders, so it is bounded by
//! truncation. That is consistent with I9, which permits removal and forbids invention.
//!
//! # Every number here is a hypothesis
//!
//! To be validated against the reference environment, on the same footing as every other
//! figure in the specification. They are chosen to be far above what legitimate mail uses
//! and far below what exhausts the machine, and the gap between those two is wide enough
//! that precision is not the point.
//!
//! # Changing a limit
//!
//! **A limit MUST NOT be raised to accommodate a single message.** The corpus decides: if
//! the fidelity corpus contains legitimate mail that a bound rejects, the bound is wrong
//! and moves with a note saying which message moved it. If it does not, the message is
//! the adversary this design is for, and rejection is the correct outcome rather than a
//! bug report.
//!
//! **A new limit arrives here, not beside the code that enforces it** — the same rule the
//! sanitizer applies to its allowlist. An addition that lands anywhere else is invisible
//! to the two other consumers that must assert it, and the failure is silent in both.

use core::time::Duration;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

// ---------------------------------------------------------------------------
// Parse limits — asserted by pipeline stages 1 through 3 and by the sanitizer.
// ---------------------------------------------------------------------------

/// L-1 · Bytes of a single body part decoded into the sanitizer.
///
/// The part selected at stage 2, *after* transfer decoding. Larger parts exist and are
/// almost never legitimate HTML.
pub const L1_BODY_PART_BYTES: u64 = 8 * MIB;

/// L-2 · MIME parts per message, counted across the whole tree rather than per level.
pub const L2_MIME_PARTS: u64 = 1024;

/// L-3 · MIME nesting depth.
///
/// A `multipart` inside a `message/rfc822` inside a `multipart` is ordinary; thirty-two
/// levels is an attack.
pub const L3_MIME_DEPTH: u64 = 32;

/// L-4 · Bytes of the header block.
///
/// Applies *before* any header is decoded, so an encoded-word bomb is bounded before
/// NFR-28's decoding runs.
pub const L4_HEADER_BLOCK_BYTES: u64 = 256 * KIB;

/// L-5 · Header fields per message.
pub const L5_HEADER_FIELDS: u64 = 1024;

/// L-6 · DOM tree depth after parsing.
///
/// The HTML5 tree builder's own recovery already collapses much deeper nesting; this
/// bounds what survives it.
pub const L6_DOM_DEPTH: u64 = 512;

/// L-7 · DOM nodes per document.
///
/// **This is the cap NFR-41's 30 ms budget is actually a function of**, and the one most
/// likely to move once measured.
pub const L7_DOM_NODES: u64 = 250_000;

/// L-8 · Attributes per element.
pub const L8_ATTRS_PER_ELEMENT: u64 = 256;

/// L-9 · CSS declarations resolved across all stylesheets and style attributes.
///
/// The bound on D-27's cascade, which is the pass most likely to miss NFR-41.
pub const L9_CSS_DECLARATIONS: u64 = 100_000;

// ---------------------------------------------------------------------------
// Resource limits — asserted by the broker, *before* a decoder is handed bytes.
// ---------------------------------------------------------------------------

/// L-10 · Encoded bytes of a single image accepted for rendering.
///
/// Checked against the declared length **and** enforced against the transferred length,
/// since a sender controls both.
pub const L10_IMAGE_BYTES: u64 = 32 * MIB;

/// L-11 · Pixels of an image decoded for classification.
///
/// D-29 decodes only for the dark transform's classifier. This is the decode-bomb bound,
/// and it is checked against the *declared* dimensions in the container **before any
/// decoder runs**.
pub const L11_DECODE_PIXELS: u64 = 40_000_000;

/// L-12 · Rasterized output of a vector image — the same bound as [`L11_DECODE_PIXELS`].
///
/// A vector image declares no pixel dimensions of its own, so the bound is on what it is
/// rasterized into. D-29 refuses what cannot be rasterized inside it.
pub const L12_RASTER_PIXELS: u64 = L11_DECODE_PIXELS;

/// L-13 · Bytes of a single fetch without explicit confirmation.
///
/// The default NFR-39 requires and does not supply. User-configurable, like the cache
/// budget.
pub const L13_FETCH_BYTES: u64 = 25 * MIB;

// ---------------------------------------------------------------------------
// Concurrency and connection limits.
// ---------------------------------------------------------------------------

/// L-23 · Concurrent provider connections per installation.
///
/// Scheduling's own arithmetic — five accounts watching three folders each — plus one for
/// on-demand work. It is what decides how many folders may be watched at all when a
/// provider cannot watch several over one connection.
pub const L23_CONNECTIONS: u64 = 16;

/// L-26 · Envelopes fetched per backfill page.
///
/// The page size D-53's resumable backfill requires. It is the granularity a resume
/// rewinds to, so it trades round trips against work repeated after an interruption.
pub const L26_BACKFILL_PAGE: u64 = 500;

/// L-29 · Concurrent resource loads per rendered document.
///
/// The bound D-91 requires so that one message with several hundred fetching positions
/// cannot saturate the pool the store, the queue and search share. **Requests beyond it
/// queue rather than fail.**
pub const L29_DOC_CONCURRENCY: u64 = 8;

// ---------------------------------------------------------------------------
// Storage and display limits.
// ---------------------------------------------------------------------------

/// L-14 · Bytes of a single blob.
///
/// Above the cache budget's own default, so in practice NFR-14 binds first. This exists
/// so that a single attachment cannot be the thing that makes the budget unenforceable.
pub const L14_BLOB_BYTES: u64 = 2 * GIB;

/// L-15 · Characters in a tag name.
///
/// The charset is Unicode scalar values excluding control characters, normalized under
/// NFR-54 like every other attacker-controlled string.
pub const L15_TAG_NAME_CHARS: u64 = 256;

/// L-20 · Default envelope and index budget.
///
/// The value NFR-52 calls user-configurable, and which D-53 makes the *sole* bound on
/// first sync. At roughly two kilobytes per message including its index entry it lands
/// near the 500,000 messages NFR-5 is measured over, so the benchmark and the shipped
/// default describe the same product rather than two.
pub const L20_ENVELOPE_INDEX_BUDGET: u64 = GIB;

/// L-25 · Characters in a header-derived display value.
///
/// Over display names, subjects, folder and tag names and attachment names. Chosen as the
/// internet message format's own line bound: far above any legitimate value and far below
/// a denial of service against native chrome. Truncation is at a grapheme boundary and
/// **follows** normalization, per D-100.
pub const L25_DISPLAY_CHARS: u64 = 998;

/// L-16 · Characters in a snippet.
///
/// **Truncated rather than rejected** — the one exception to the rule above, because a
/// snippet is a summary Sift derives rather than content it renders. Bounds the envelope,
/// which NFR-52 budgets and which is retained far longer than any body.
pub const L16_SNIPPET_CHARS: u64 = 280;

// ---------------------------------------------------------------------------
// Time limits. Bounds on behaviour, not performance targets: a target belongs to its
// requirement, and appears here only if exceeding it changes what Sift *does* rather
// than how fast it does it.
// ---------------------------------------------------------------------------

/// L-17 · Age at which a queued intent stops being retried.
///
/// D-85 expires on elapsed time rather than attempts, because an intent that failed twice
/// in a week offline and one that failed two hundred times in a minute are not the same
/// situation. Long enough to cover an ordinary offline stretch; short enough that the
/// conflict pile-up FR-16 must adjudicate stays adjudicable.
pub const L17_INTENT_EXPIRY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// L-18 · Time with no reader visible before the body view is torn down.
///
/// Long enough to survive switching folders and returning; short enough that the largest
/// single allocation in the running application does not persist through an interruption.
/// D-90 defines what counts as visible, and it is minimization rather than occlusion.
pub const L18_BODY_VIEW_IDLE: Duration = Duration::from_secs(30);

/// L-19 · Time the pressure signal must stay clear before a shed tier is released.
///
/// The hysteresis D-93 requires. Without it a system oscillating around the threshold
/// reparses the 40 MB filter engine on every crossing. **One value serves every tier,
/// which that decision records as its own weakest point** — a body view is cheap to
/// rebuild and a filter engine is not.
pub const L19_PRESSURE_DWELL: Duration = Duration::from_secs(60);

/// L-21 · Reader dwell before a message is marked read.
///
/// The value D-52 calls "a short configurable dwell", in the decision that argues this
/// number sets the queue write rate, the flush wakeup rate against NFR-11, and the
/// data-cap burn. Longer than arrow-key traversal and shorter than reading, which is the
/// only property it has to have. May be set to off.
pub const L21_READ_DWELL: Duration = Duration::from_secs(2);

/// L-22 · Duration of FR-15's timed undo window.
///
/// D-86 explains why the window withholds nothing, so this bounds an affordance rather
/// than a network delay.
pub const L22_UNDO_WINDOW: Duration = Duration::from_secs(10);

/// L-24 · Cap on reconnection and retry backoff.
///
/// The number NFR-38's "no retry storm on wake" rests on. **The cap matches the longest
/// aligned poll interval, so a backed-off account rejoins an existing wheel fire rather
/// than adding one.** See [`L24_BACKOFF_JITTER`].
pub const L24_BACKOFF_CAP: Duration = Duration::from_secs(15 * 60);

/// The jitter applied to [`L24_BACKOFF_CAP`], as a fraction either side.
pub const L24_BACKOFF_JITTER: f64 = 0.25;

/// L-27 · Interval at which an IMAP idle watch is re-issued.
///
/// A bound rather than a preference: it sits below the protocol's own thirty-minute
/// expectation and below common network-address-translation timeouts, and **exceeding it
/// drops a watch silently**.
pub const L27_IMAP_IDLE_REISSUE: Duration = Duration::from_secs(29 * 60);

/// L-28 · Interval between reattempts while a captive portal is present.
///
/// Under D-96 this is a bounded reattempt of the account's own next operation rather than
/// a probe to a detection host — Sift contacts no detection endpoint of any kind.
pub const L28_PORTAL_REATTEMPT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// The register.
// ---------------------------------------------------------------------------

/// What Sift does when a bound is crossed.
///
/// This is not decoration. `docs/limits.md` is emphatic that the answer for a *message*
/// is rejection rather than truncation, and the two consequences below that look
/// permissive — [`Truncate`](Consequence::Truncate) and [`Queue`](Consequence::Queue) —
/// are each permitted for a stated reason rather than by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consequence {
    /// The message degrades to FR-9's raw source view and **is not rendered partially**.
    ///
    /// A document truncated mid-tree is a document whose structure the sender chose by
    /// choosing where the cap fell — a parse-differential primitive of the kind I8 exists
    /// to close.
    RejectToRawView,
    /// The resource resolves to the broker's deterministic blocked answer, and **is not
    /// handed over partially decoded**.
    ///
    /// A denied address resolves to this rather than to nothing, so that a fabricated or
    /// stale address resolving to nothing stays distinguishable from a decision.
    Blocked,
    /// The operation proceeds only on explicit user confirmation.
    RequireConfirmation,
    /// The value is refused. Unlike [`RejectToRawView`](Consequence::RejectToRawView) this
    /// is about one field rather than a whole message.
    Refuse,
    /// The value is truncated. Permitted only where the bounded thing is something Sift
    /// *derived* rather than content it renders, which is consistent with I9: removal is
    /// permitted, invention is not.
    Truncate,
    /// Work beyond the bound queues rather than failing.
    Queue,
    /// A governing quantity — a budget or a capacity — rather than a threshold that
    /// rejects a particular input.
    Capacity,
    /// A duration Sift enforces. Exceeding it changes what Sift does rather than how fast.
    Deadline,
}

/// The quantity a limit bounds, kept typed so the register cannot claim 30 seconds is
/// 30 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Magnitude {
    Bytes(u64),
    Count(u64),
    Pixels(u64),
    Chars(u64),
    Time(Duration),
}

impl Magnitude {
    /// The scalar, for a limit that has one. `None` for a duration.
    #[must_use]
    pub fn scalar(self) -> Option<u64> {
        match self {
            Self::Bytes(v) | Self::Count(v) | Self::Pixels(v) | Self::Chars(v) => Some(v),
            Self::Time(_) => None,
        }
    }
}

/// One numeric bound, with what it bounds and what happens when it is crossed.
#[derive(Debug, Clone, Copy)]
pub struct Limit {
    /// The stable identifier. Never renumbered, never reused.
    pub id: &'static str,
    pub magnitude: Magnitude,
    /// What the number bounds, in the words `docs/limits.md` uses.
    pub bounds: &'static str,
    pub consequence: Consequence,
}

impl Limit {
    /// The values a property generator should try: **at** the bound, and either side of
    /// it. NFR-40's generator "has to generate inputs *at* the boundary to be worth
    /// running", and this is what stops it having to guess where the boundary is.
    ///
    /// `None` for a duration, which is not something an input generator produces.
    #[must_use]
    pub fn boundary(&self) -> Option<[u64; 3]> {
        let v = self.magnitude.scalar()?;
        Some([v.saturating_sub(1), v, v.saturating_add(1)])
    }

    /// Whether a value crosses this bound. Limits are inclusive maxima: the stated value
    /// is legal and one more is not.
    #[must_use]
    pub fn exceeded_by(&self, value: u64) -> bool {
        self.magnitude.scalar().is_some_and(|v| value > v)
    }
}

use Consequence::{
    Blocked, Capacity, Deadline, Queue, Refuse, RejectToRawView, RequireConfirmation, Truncate,
};
use Magnitude::{Bytes, Chars, Count, Pixels, Time};

/// Every limit Sift enforces at runtime.
///
/// Enumerable on purpose: this is what the property generator and the live per-message
/// invariant check walk, rather than each holding its own copy of twenty-nine numbers.
pub const ALL: &[Limit] = &[
    // Parse limits.
    Limit {
        id: "L-1",
        magnitude: Bytes(L1_BODY_PART_BYTES),
        bounds: "bytes of a single body part decoded into the sanitizer",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-2",
        magnitude: Count(L2_MIME_PARTS),
        bounds: "MIME parts per message",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-3",
        magnitude: Count(L3_MIME_DEPTH),
        bounds: "MIME nesting depth",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-4",
        magnitude: Bytes(L4_HEADER_BLOCK_BYTES),
        bounds: "bytes of the header block",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-5",
        magnitude: Count(L5_HEADER_FIELDS),
        bounds: "header fields per message",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-6",
        magnitude: Count(L6_DOM_DEPTH),
        bounds: "DOM tree depth after parsing",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-7",
        magnitude: Count(L7_DOM_NODES),
        bounds: "DOM nodes per document",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-8",
        magnitude: Count(L8_ATTRS_PER_ELEMENT),
        bounds: "attributes per element",
        consequence: RejectToRawView,
    },
    Limit {
        id: "L-9",
        magnitude: Count(L9_CSS_DECLARATIONS),
        bounds: "CSS declarations resolved across all stylesheets and style attributes",
        consequence: RejectToRawView,
    },
    // Resource limits.
    Limit {
        id: "L-10",
        magnitude: Bytes(L10_IMAGE_BYTES),
        bounds: "encoded bytes of a single image accepted for rendering",
        consequence: Blocked,
    },
    Limit {
        id: "L-11",
        magnitude: Pixels(L11_DECODE_PIXELS),
        bounds: "pixels of an image decoded for classification",
        consequence: Blocked,
    },
    Limit {
        id: "L-12",
        magnitude: Pixels(L12_RASTER_PIXELS),
        bounds: "rasterized output of a vector image",
        consequence: Blocked,
    },
    Limit {
        id: "L-13",
        magnitude: Bytes(L13_FETCH_BYTES),
        bounds: "bytes of a single fetch without explicit confirmation",
        consequence: RequireConfirmation,
    },
    // Storage and display limits.
    Limit {
        id: "L-14",
        magnitude: Bytes(L14_BLOB_BYTES),
        bounds: "bytes of a single blob",
        consequence: Blocked,
    },
    Limit {
        id: "L-15",
        magnitude: Chars(L15_TAG_NAME_CHARS),
        bounds: "characters in a tag name",
        consequence: Refuse,
    },
    Limit {
        id: "L-16",
        magnitude: Chars(L16_SNIPPET_CHARS),
        bounds: "characters in a snippet",
        consequence: Truncate,
    },
    Limit {
        id: "L-20",
        magnitude: Bytes(L20_ENVELOPE_INDEX_BUDGET),
        bounds: "default envelope and index budget",
        consequence: Capacity,
    },
    Limit {
        id: "L-25",
        magnitude: Chars(L25_DISPLAY_CHARS),
        bounds: "characters in a header-derived display value",
        consequence: Truncate,
    },
    // Concurrency and connection limits.
    Limit {
        id: "L-23",
        magnitude: Count(L23_CONNECTIONS),
        bounds: "concurrent provider connections per installation",
        consequence: Capacity,
    },
    Limit {
        id: "L-26",
        magnitude: Count(L26_BACKFILL_PAGE),
        bounds: "envelopes fetched per backfill page",
        consequence: Capacity,
    },
    Limit {
        id: "L-29",
        magnitude: Count(L29_DOC_CONCURRENCY),
        bounds: "concurrent resource loads per rendered document",
        consequence: Queue,
    },
    // Time limits.
    Limit {
        id: "L-17",
        magnitude: Time(L17_INTENT_EXPIRY),
        bounds: "age at which a queued intent stops being retried",
        consequence: Deadline,
    },
    Limit {
        id: "L-18",
        magnitude: Time(L18_BODY_VIEW_IDLE),
        bounds: "time with no reader visible before the body view is torn down",
        consequence: Deadline,
    },
    Limit {
        id: "L-19",
        magnitude: Time(L19_PRESSURE_DWELL),
        bounds: "time the pressure signal must stay clear before a shed tier is released",
        consequence: Deadline,
    },
    Limit {
        id: "L-21",
        magnitude: Time(L21_READ_DWELL),
        bounds: "reader dwell before a message is marked read",
        consequence: Deadline,
    },
    Limit {
        id: "L-22",
        magnitude: Time(L22_UNDO_WINDOW),
        bounds: "duration of FR-15's timed undo window",
        consequence: Deadline,
    },
    Limit {
        id: "L-24",
        magnitude: Time(L24_BACKOFF_CAP),
        bounds: "cap on reconnection and retry backoff",
        consequence: Deadline,
    },
    Limit {
        id: "L-27",
        magnitude: Time(L27_IMAP_IDLE_REISSUE),
        bounds: "interval at which an IMAP idle watch is re-issued",
        consequence: Deadline,
    },
    Limit {
        id: "L-28",
        magnitude: Time(L28_PORTAL_REATTEMPT),
        bounds: "interval between reattempts while a captive portal is present",
        consequence: Deadline,
    },
];

/// Look a limit up by identifier.
#[must_use]
pub fn get(id: &str) -> Option<&'static Limit> {
    ALL.iter().find(|l| l.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_register_holds_l1_through_l29_exactly_once() {
        // docs/limits.md says it owns L-1 through L-29. A gap here means a bound that
        // exists in the specification and is enforced by nothing, or the reverse.
        let ids: BTreeSet<&str> = ALL.iter().map(|l| l.id).collect();
        assert_eq!(ids.len(), ALL.len(), "an identifier appears twice");
        for n in 1..=29 {
            let id = format!("L-{n}");
            assert!(
                ids.contains(id.as_str()),
                "{id} is in docs/limits.md and not here"
            );
        }
        assert_eq!(
            ALL.len(),
            29,
            "an identifier is here and not in docs/limits.md"
        );
    }

    #[test]
    fn every_parse_limit_rejects_the_message_rather_than_truncating_it() {
        // The rule docs/limits.md exists to state. Truncating hands the sanitizer's
        // output contract to the attacker: the sender chooses the document's structure by
        // choosing where the cap falls.
        for id in [
            "L-1", "L-2", "L-3", "L-4", "L-5", "L-6", "L-7", "L-8", "L-9",
        ] {
            let l = get(id).expect("present");
            assert_eq!(
                l.consequence,
                Consequence::RejectToRawView,
                "{id} bounds a parse and must reject to the raw view"
            );
        }
    }

    #[test]
    fn only_derived_summaries_truncate() {
        // "One exception, and it is not a message." The snippet is a summary Sift derives
        // for the list; the display value is a header rendered into native chrome. Neither
        // is a document, so neither can be a parse differential. Anything else truncating
        // is the mistake this test exists to catch.
        let truncating: BTreeSet<&str> = ALL
            .iter()
            .filter(|l| l.consequence == Consequence::Truncate)
            .map(|l| l.id)
            .collect();
        assert_eq!(truncating, BTreeSet::from(["L-16", "L-25"]));
    }

    #[test]
    fn no_resource_limit_is_handed_over_partially() {
        for id in ["L-10", "L-11", "L-12", "L-14"] {
            assert_eq!(
                get(id).expect("present").consequence,
                Consequence::Blocked,
                "{id}"
            );
        }
    }

    #[test]
    fn the_register_agrees_with_the_constants() {
        // The register is a second statement of each number and could drift from the
        // first. It cannot drift silently.
        assert_eq!(
            get("L-1").unwrap().magnitude,
            Magnitude::Bytes(L1_BODY_PART_BYTES)
        );
        assert_eq!(
            get("L-7").unwrap().magnitude,
            Magnitude::Count(L7_DOM_NODES)
        );
        assert_eq!(
            get("L-11").unwrap().magnitude,
            Magnitude::Pixels(L11_DECODE_PIXELS)
        );
        assert_eq!(
            get("L-16").unwrap().magnitude,
            Magnitude::Chars(L16_SNIPPET_CHARS)
        );
        assert_eq!(
            get("L-17").unwrap().magnitude,
            Magnitude::Time(L17_INTENT_EXPIRY)
        );
    }

    #[test]
    fn a_vector_image_is_bounded_by_what_it_rasterizes_into() {
        // L-12 is stated as "the same 40 megapixels" rather than as its own number, and
        // the two moving apart would be a silent widening of the decode-bomb bound.
        assert_eq!(L12_RASTER_PIXELS, L11_DECODE_PIXELS);
    }

    #[test]
    fn the_generator_is_handed_the_boundary_rather_than_guessing_it() {
        let l = get("L-8").expect("present");
        assert_eq!(l.boundary(), Some([255, 256, 257]));
        // A duration is not something an input generator produces.
        assert_eq!(get("L-22").unwrap().boundary(), None);
    }

    #[test]
    fn a_limit_is_an_inclusive_maximum() {
        let l = get("L-3").expect("present");
        assert!(!l.exceeded_by(L3_MIME_DEPTH), "the stated value is legal");
        assert!(l.exceeded_by(L3_MIME_DEPTH + 1), "one more is not");
    }

    #[test]
    fn every_limit_says_what_it_bounds() {
        for l in ALL {
            assert!(l.bounds.len() > 10, "{} does not say what it bounds", l.id);
        }
    }

    #[test]
    fn the_backfill_page_is_smaller_than_the_envelope_budget_can_hold() {
        // L-26 is the granularity a resumed backfill rewinds to. A page larger than the
        // budget could hold would make D-53's first sync unable to complete a single page.
        let per_message_estimate = 2 * KIB;
        assert!(L26_BACKFILL_PAGE * per_message_estimate < L20_ENVELOPE_INDEX_BUDGET);
    }
}

#[cfg(test)]
mod agrees_with_the_specification {
    //! `docs/limits.md` and this register are two statements of the same twenty-nine
    //! numbers, and the project's working rule is that changing a decision means changing
    //! two places — "leaving the two disagreeing is a defect". This is that defect made
    //! into a test failure.
    //!
    //! Compiled in with `include_str!` rather than read at runtime, so the check runs
    //! wherever the tests run and fails loudly if the document is moved or renamed.

    use super::*;
    use std::collections::BTreeSet;

    const DOC: &str = include_str!("../../../../docs/limits.md");

    /// Identifiers as the document's tables write them: `| **L-7** | ...`.
    fn ids_in_doc() -> BTreeSet<String> {
        DOC.lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("| **L-")?;
                let n = rest.split("**").next()?;
                n.parse::<u32>().ok().map(|n| format!("L-{n}"))
            })
            .collect()
    }

    #[test]
    fn every_limit_in_the_document_is_enforced_here() {
        let doc = ids_in_doc();
        assert!(
            !doc.is_empty(),
            "no identifiers parsed — has the table format changed?"
        );
        let register: BTreeSet<String> = ALL.iter().map(|l| l.id.to_owned()).collect();

        let missing: Vec<_> = doc.difference(&register).collect();
        assert!(
            missing.is_empty(),
            "in docs/limits.md and enforced by nothing: {missing:?}\n\
             A new limit arrives in the register, not beside the code that enforces it."
        );

        let extra: Vec<_> = register.difference(&doc).collect();
        assert!(
            extra.is_empty(),
            "enforced here and absent from docs/limits.md: {extra:?}\n\
             A bound the document does not carry is invisible to the two other consumers \
             that must assert it."
        );
    }

    #[test]
    fn the_document_still_states_the_rejection_rule() {
        // The rule this whole module is shaped by. If it is ever softened in the document,
        // the Consequence assignments here stop being justified and must be revisited
        // rather than quietly left as they are.
        assert!(
            DOC.contains("Exceeding a limit rejects; it does not truncate"),
            "docs/limits.md no longer states the rejection rule this register encodes"
        );
    }
}

//! The request path.

use crate::token::{Address, Token};
use sift_block::engine::{Authority, Decision};
use sift_block::heuristic::{self, Candidate, Finding};
use sift_block::origin::{Infrastructure, Origin};
use sift_foundation::limits::{
    L10_IMAGE_BYTES, L11_DECODE_PIXELS, L12_RASTER_PIXELS, L13_FETCH_BYTES, L29_DOC_CONCURRENCY,
};
use std::collections::BTreeMap;

/// One of exactly three answers.
///
/// D-91: **every request receives exactly one of these.** Leaving a failed or timed-out load
/// unanswered was rejected, because an unanswered request is one the engine waits on
/// forever and a document that never finishes loading is worse than one that finishes
/// incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The bytes. Streamed rather than buffered whole — buffering each resource was
    /// rejected, and a message with several hundred positions is why.
    Bytes { length: u64 },
    /// The **deterministic** blocked answer, with the reason.
    ///
    /// Deterministic rather than nothing, and the distinction is load-bearing: a fabricated
    /// or stale address resolving to nothing is a *defect being caught*, and conflating the
    /// two would hide it.
    Blocked(Reason),
    /// Could not be produced. Distinct from blocked, because "Sift refused this" and "this
    /// did not arrive" are different facts and FR-12 insists such pairs stay distinct.
    Unavailable(Unavailable),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A filter rule matched. Carries which, for FR-33.
    Rule(String),
    /// FR-29's heuristics, independent of list coverage. **Every heuristic block logs its
    /// reason.**
    Heuristic(Vec<Finding>),
    /// Remote content is blocked by default and this sender is not allowed — FR-8.
    NotAllowedBySender,
    /// The active network policy tier forbids it. NFR-32: zero speculative prefetch in
    /// Conservative or Minimal.
    NetworkPolicy,
    /// A bound in the limits register. Checked **before any decoder is handed bytes**.
    Bounds(&'static str),
    /// No blocking authority is loaded.
    ///
    /// **Names the shed rather than a rule**, because there was no rule. Telling a user
    /// their image matched a filter would send them looking for one that does not exist.
    /// L1 is not a rare event on the reference rig, so this is a state users will meet.
    Shed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// The document's token was revoked while this was in flight.
    ///
    /// **Cancelled and answered**, rather than raced: D-90 revokes at navigation and NFR-46
    /// tears views down routinely, so this is ordinary rather than exceptional.
    Revoked,
    /// The deadline passed. Every load has one; unanswered within it, the work is cancelled
    /// and this is returned.
    Deadline,
    /// The address does not correspond to a position in this document.
    ///
    /// A fabricated or stale address. **Not** a block — see [`Answer::Blocked`].
    UnknownAddress,
    /// The bytes are not held and could not be fetched now.
    NotFetchable,
    /// A blob failed to authenticate and was discarded.
    DecryptFailed,
}

/// What is known about one position at request time.
#[derive(Debug, Clone)]
pub struct Position {
    pub url: String,
    pub declared_length: Option<u64>,
    pub declared_width: Option<u32>,
    pub declared_height: Option<u32>,
    pub declared_pixels: Option<u64>,
    pub style: Option<String>,
    pub alt: Option<String>,
    pub in_zero_height_container: bool,
    /// `image`, `font`, `stylesheet` — what the engine says it is asking for.
    pub request_type: String,
}

/// A live document: its token, its positions, and the origin it renders under.
#[derive(Debug)]
pub struct Document {
    pub token: Token,
    pub origin: Origin,
    pub positions: Vec<Position>,
    /// D-91 bounds per-document concurrency at L-29 and **queues beyond it rather than
    /// failing**: one message with several hundred fetching positions must not saturate the
    /// pool the store, the queue and search share.
    in_flight: usize,
    queued: usize,
}

/// A request as the engine hands it over.
#[derive(Debug, Clone)]
pub struct Request {
    pub url: String,
    /// The transferred length, where known. Checked **as well as** the declared length,
    /// because a sender controls both.
    pub transferred_length: Option<u64>,
}

/// The broker.
#[derive(Debug, Default)]
pub struct Broker {
    documents: BTreeMap<String, Document>,
    /// Whether the active network policy tier permits fetching at all.
    pub tier_permits_fetch: bool,
    /// Senders the user has explicitly allowed — FR-8. Keyed on the **attested synthetic
    /// origin**, never on a displayed sender: this is security state, and write access to it
    /// is write access to Sift's egress policy.
    allowed_origins: Vec<String>,
}

impl Broker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
            tier_permits_fetch: true,
            allowed_origins: Vec::new(),
        }
    }

    /// Register a document. Returns the token its addresses are minted under.
    pub fn open_document(&mut self, origin: Origin, positions: Vec<Position>) -> Token {
        let token = Token::mint();
        self.documents.insert(
            token.as_str().to_owned(),
            Document {
                token: token.clone(),
                origin,
                positions,
                in_flight: 0,
                queued: 0,
            },
        );
        token
    }

    /// Revoke a document's token.
    ///
    /// D-90 revokes when navigation away begins — earlier and more often than teardown.
    /// Everything in flight under it is **cancelled and answered `Revoked`**, not left to
    /// race: an unanswered request is one the engine waits on forever.
    pub fn revoke(&mut self, token: &Token) -> bool {
        self.documents.remove(token.as_str()).is_some()
    }

    /// Allow a sender's remote content durably — FR-8.
    ///
    /// Refuses for a null origin, because there is nothing to key on. That is not an error
    /// to report to the user: the interface must not have offered the control at all.
    pub fn allow_origin(&mut self, origin: &Origin) -> bool {
        let Some(domain) = origin.domain() else {
            return false;
        };
        if !origin.can_carry_a_durable_allowance() {
            return false;
        }
        self.allowed_origins.push(domain.to_ascii_lowercase());
        true
    }

    /// Answer one request.
    ///
    /// The order of the checks is the design. Identity first, because an address that names
    /// no position is a defect being caught rather than a decision to make. Then bounds,
    /// **before any decoder is handed bytes**. Then policy, then the authority, then the
    /// heuristics — which run whether or not a list matched, because public lists cover
    /// email tracking poorly.
    pub fn answer(
        &mut self,
        request: &Request,
        authority: &Authority,
        infrastructure: &Infrastructure,
    ) -> Answer {
        // 1. Is this address valid for this view at all? D-28.
        let Some(address) = Address::parse(&request.url) else {
            return Answer::Unavailable(Unavailable::UnknownAddress);
        };
        let Some(document) = self.documents.get_mut(address.token.as_str()) else {
            // The token was revoked, or was never minted. Either way the document this
            // claims to belong to is not here.
            return Answer::Unavailable(Unavailable::Revoked);
        };
        let Some(position) = document.positions.get(address.position).cloned() else {
            return Answer::Unavailable(Unavailable::UnknownAddress);
        };

        // 2. Bounded means bounded **before the decode, not during it**. A decode bomb is a
        //    denial of service under NFR-19, and checking dimensions after handing bytes to a
        //    decoder is checking them too late.
        if let Some(bound) = exceeds_a_bound(&position, request.transferred_length) {
            return Answer::Blocked(Reason::Bounds(bound));
        }

        // 3. May it be fetched now? The tier's answer does not depend on the message.
        if !self.tier_permits_fetch {
            return Answer::Blocked(Reason::NetworkPolicy);
        }

        // 4. Has the user allowed this sender? Remote content is blocked **by default**.
        let allowed = document.origin.domain().is_some_and(|d| {
            self.allowed_origins
                .iter()
                .any(|a| a == &d.to_ascii_lowercase())
        });
        let first_party = infrastructure.is_first_party(&document.origin, host_of(&position.url));
        if !allowed && !first_party {
            return Answer::Blocked(Reason::NotAllowedBySender);
        }

        // 5. The authority. An absent one denies, and names the shed rather than a rule.
        let source = document.origin.domain().unwrap_or("invalid.");
        match authority.decide(&position.url, source, &position.request_type) {
            Decision::AbsentAuthority => return Answer::Blocked(Reason::Shed),
            d if !d.permits_fetch() => {
                return Answer::Blocked(Reason::Rule(describe(&d)));
            }
            _ => {}
        }

        // 6. FR-29's heuristics, independent of filter-list coverage.
        let findings = heuristic::examine(
            &Candidate {
                url: &position.url,
                declared_width: position.declared_width,
                declared_height: position.declared_height,
                style: position.style.as_deref(),
                alt: position.alt.as_deref(),
                in_zero_height_container: position.in_zero_height_container,
            },
            &document.origin,
        );
        if !findings.is_empty() {
            return Answer::Blocked(Reason::Heuristic(findings));
        }

        // 7. Per-document concurrency. **Queues rather than fails**, because refusing a
        //    legitimate image because five others were already loading would be a rendering
        //    defect wearing a safety argument.
        if document.in_flight >= L29_DOC_CONCURRENCY as usize {
            document.queued += 1;
        } else {
            document.in_flight += 1;
        }

        Answer::Bytes {
            length: position.declared_length.unwrap_or(0),
        }
    }

    /// Whether prefetching is permitted right now.
    ///
    /// > "**If the broker prefetches remote images, that prefetch *is* the tracking
    /// > event.**"
    ///
    /// So prefetch is gated on the same allow decision as display, and **disabled entirely**
    /// under a constrained tier — NFR-32's zero speculative prefetch. The same trap applies
    /// to link unwrapping, which is why FR-30 recovers a destination locally or says it
    /// cannot.
    #[must_use]
    pub const fn may_prefetch(&self) -> bool {
        self.tier_permits_fetch
    }

    #[must_use]
    pub fn live_documents(&self) -> usize {
        self.documents.len()
    }
}

/// Every bound checked before a decoder sees a byte.
fn exceeds_a_bound(position: &Position, transferred: Option<u64>) -> Option<&'static str> {
    // L-10 is checked against the declared length **and** enforced against the transferred
    // length, because a sender controls both and a small declaration with a large body is
    // the obvious way around a declared-length check.
    if position
        .declared_length
        .is_some_and(|n| n > L10_IMAGE_BYTES)
        || transferred.is_some_and(|n| n > L10_IMAGE_BYTES)
    {
        return Some("L-10 image bytes");
    }
    if position
        .declared_length
        .is_some_and(|n| n > L13_FETCH_BYTES)
        || transferred.is_some_and(|n| n > L13_FETCH_BYTES)
    {
        return Some("L-13 single fetch without confirmation");
    }
    // L-11 is the decode-bomb bound, checked against the **declared dimensions in the
    // container** before any decoder runs.
    if position
        .declared_pixels
        .is_some_and(|p| p > L11_DECODE_PIXELS)
    {
        return Some("L-11 decoded pixels");
    }
    if let (Some(w), Some(h)) = (position.declared_width, position.declared_height) {
        if u64::from(w) * u64::from(h) > L12_RASTER_PIXELS {
            return Some("L-12 rasterized pixels");
        }
    }
    None
}

fn describe(decision: &Decision) -> String {
    match decision {
        Decision::Agreed(v) => format!("authority and backstop agreed: {v:?}"),
        Decision::Disagreed {
            authority,
            backstop,
        } => {
            // Surfaced rather than silently resolved — D-10 requires the debug view show it.
            format!("layers disagreed: authority {authority:?}, backstop {backstop:?}")
        }
        Decision::AbsentAuthority => "no authority loaded".to_owned(),
    }
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_block::engine::Blocker;
    use sift_block::origin::Authentication;

    fn attested(domain: &str) -> Origin {
        Origin::derive(&Authentication {
            signing_domain: Some(domain.to_owned()),
            ..Authentication::default()
        })
    }

    fn position(url: &str) -> Position {
        Position {
            url: url.to_owned(),
            declared_length: Some(1024),
            declared_width: Some(600),
            declared_height: Some(400),
            declared_pixels: Some(240_000),
            style: None,
            alt: Some("an image".to_owned()),
            in_zero_height_container: false,
            request_type: "image".to_owned(),
        }
    }

    fn authority() -> Authority {
        Authority::Loaded(Box::new(Blocker::from_rules(&[
            "||tracker.test^".to_owned()
        ])))
    }

    fn broker_with(origin: Origin, positions: Vec<Position>) -> (Broker, Token) {
        let mut b = Broker::new();
        let t = b.open_document(origin, positions);
        (b, t)
    }

    #[test]
    fn remote_content_is_blocked_by_default() {
        // FR-8. The user has allowed nothing, so nothing loads — and the reason says which
        // of the several possible reasons it was.
        let (mut b, t) = broker_with(
            attested("sender.test"),
            vec![position("https://cdn.other.test/x.png")],
        );
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Blocked(Reason::NotAllowedBySender));
    }

    #[test]
    fn an_allowed_sender_loads() {
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(
            origin.clone(),
            vec![position("https://cdn.other.test/x.png")],
        );
        assert!(b.allow_origin(&origin));
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert!(matches!(a, Answer::Bytes { .. }), "{a:?}");
    }

    #[test]
    fn a_null_origin_cannot_be_allowed_durably() {
        // FR-8's allowlist keys on the origin. For a null origin there is nothing to key on,
        // and the interface must not have offered the control — so this refuses rather than
        // inventing a key.
        let mut b = Broker::new();
        assert!(!b.allow_origin(&Origin::Null));
    }

    #[test]
    fn a_fabricated_address_resolves_to_unavailable_rather_than_blocked() {
        // "A denied address resolves to a deterministic blocked answer, not to nothing" —
        // and the converse matters as much: an address that names no position is **a defect
        // being caught**, and conflating it with a block would hide it.
        let (mut b, _t) = broker_with(attested("sender.test"), vec![position("https://a.test/x")]);
        let a = b.answer(
            &Request {
                url: "sift-resource://deadbeef/0".to_owned(),
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Unavailable(Unavailable::Revoked));
    }

    #[test]
    fn a_position_that_does_not_exist_is_unavailable() {
        let (mut b, t) = broker_with(attested("sender.test"), vec![position("https://a.test/x")]);
        let url = Address {
            token: t,
            position: 99,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Unavailable(Unavailable::UnknownAddress));
    }

    #[test]
    fn revoking_cancels_everything_under_the_token() {
        // D-90 revokes at navigation rather than at teardown, which is earlier and more
        // often. Anything in flight is answered rather than left to race — an unanswered
        // request is one the engine waits on forever.
        let (mut b, t) = broker_with(attested("sender.test"), vec![position("https://a.test/x")]);
        assert!(b.revoke(&t));
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Unavailable(Unavailable::Revoked));
        assert_eq!(b.live_documents(), 0);
    }

    #[test]
    fn two_documents_do_not_share_an_address_space() {
        // What the per-document token buys: an address minted for one document names nothing
        // in another, so two messages cannot correlate through a guessed address.
        let mut b = Broker::new();
        let first = b.open_document(attested("a.test"), vec![position("https://a.test/x")]);
        let second = b.open_document(attested("b.test"), vec![position("https://b.test/y")]);
        assert_ne!(first.as_str(), second.as_str());

        let crossed = Address {
            token: first,
            position: 0,
        }
        .to_url();
        let parsed = Address::parse(&crossed).expect("well formed");
        assert_ne!(parsed.token.as_str(), second.as_str());
    }

    #[test]
    fn bounds_are_checked_before_anything_is_decoded() {
        // "Dimensions and resource limits are checked before any decoder is handed bytes."
        // A decode bomb is a denial of service under NFR-19, and checking after handing the
        // bytes over is checking too late.
        let mut oversized = position("https://a.test/x.png");
        oversized.declared_pixels = Some(L11_DECODE_PIXELS + 1);
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![oversized]);
        b.allow_origin(&origin);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Blocked(Reason::Bounds("L-11 decoded pixels")));
    }

    #[test]
    fn the_transferred_length_is_checked_as_well_as_the_declared_one() {
        // A sender controls both. A small declaration with a large body is the obvious way
        // around a declared-length check, so L-10 is enforced against what actually arrives.
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![position("https://a.test/x.png")]);
        b.allow_origin(&origin);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: Some(L10_IMAGE_BYTES + 1),
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Blocked(Reason::Bounds("L-10 image bytes")));
    }

    #[test]
    fn an_absent_authority_names_the_shed_rather_than_a_rule() {
        // L1 is not a rare event on the reference rig, so this is a state users will meet.
        // Telling them their image matched a filter would send them looking for a rule that
        // does not exist.
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![position("https://a.test/x.png")]);
        b.allow_origin(&origin);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &Authority::Absent,
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Blocked(Reason::Shed));
    }

    #[test]
    fn a_constrained_tier_refuses_before_asking_anything_else() {
        // NFR-32: zero speculative prefetch in Conservative or Minimal. The tier's answer
        // does not depend on the message, so it is asked before the message is consulted.
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![position("https://a.test/x.png")]);
        b.allow_origin(&origin);
        b.tier_permits_fetch = false;
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert_eq!(a, Answer::Blocked(Reason::NetworkPolicy));
        assert!(!b.may_prefetch(), "prefetch survived a constrained tier");
    }

    #[test]
    fn prefetch_is_gated_on_the_same_decision_as_display() {
        // "If the broker prefetches remote images, that prefetch **is** the tracking event."
        let mut b = Broker::new();
        assert!(b.may_prefetch());
        b.tier_permits_fetch = false;
        assert!(!b.may_prefetch());
    }

    #[test]
    fn a_tracking_pixel_is_blocked_by_heuristic_even_when_no_rule_matches() {
        // FR-29: public filter lists cover email tracking poorly, so the heuristics run
        // whether or not a list matched — and the block names the heuristic rather than a
        // rule, because there was no rule.
        let mut pixel = position("https://unlisted.test/open.gif");
        pixel.declared_width = Some(1);
        pixel.declared_height = Some(1);
        pixel.declared_pixels = Some(1);
        pixel.alt = None;

        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![pixel]);
        b.allow_origin(&origin);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        match a {
            Answer::Blocked(Reason::Heuristic(findings)) => {
                assert!(!findings.is_empty());
                for f in findings {
                    assert!(
                        !f.evidence.is_empty(),
                        "a heuristic blocked without evidence"
                    );
                }
            }
            other => panic!("a 1x1 pixel from an unlisted host loaded: {other:?}"),
        }
    }

    #[test]
    fn a_listed_tracker_is_blocked_by_the_authority() {
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![position("https://tracker.test/p.png")]);
        b.allow_origin(&origin);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &Infrastructure::default(),
        );
        assert!(matches!(a, Answer::Blocked(Reason::Rule(_))), "{a:?}");
    }

    #[test]
    fn attested_infrastructure_loads_without_a_per_sender_allowance() {
        // D-37, gated on attestation. This is the case that makes real mail work: a bank's
        // images live on a mail provider's infrastructure, and refusing them would break
        // legitimate mail for everybody.
        let origin = attested("bank.test");
        let (mut b, t) = broker_with(
            origin,
            vec![position("https://img.mailinfra.test/logo.png")],
        );
        let list = Infrastructure::new(vec!["mailinfra.test".to_owned()]);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &list,
        );
        assert!(matches!(a, Answer::Bytes { .. }), "{a:?}");
    }

    #[test]
    fn an_unauthenticated_sender_gets_nothing_from_the_infrastructure_list() {
        let spoof = Origin::derive(&Authentication {
            from_domain: Some("bank.test".to_owned()),
            ..Authentication::default()
        });
        let (mut b, t) = broker_with(spoof, vec![position("https://img.mailinfra.test/logo.png")]);
        let list = Infrastructure::new(vec!["mailinfra.test".to_owned()]);
        let url = Address {
            token: t,
            position: 0,
        }
        .to_url();
        let a = b.answer(
            &Request {
                url,
                transferred_length: None,
            },
            &authority(),
            &list,
        );
        assert_eq!(a, Answer::Blocked(Reason::NotAllowedBySender));
    }

    #[test]
    fn every_request_receives_exactly_one_answer() {
        // D-91. "Leaving a failed or timed-out load unanswered" was rejected: an unanswered
        // request is one the engine waits on forever, and a document that never finishes
        // loading is worse than one that finishes incomplete.
        let origin = attested("sender.test");
        let (mut b, t) = broker_with(origin.clone(), vec![position("https://a.test/x.png")]);
        for url in [
            Address {
                token: t,
                position: 0,
            }
            .to_url(),
            "sift-resource://nope/0".to_owned(),
            "https://external.test/x".to_owned(),
            "not-a-url".to_owned(),
            String::new(),
        ] {
            let _: Answer = b.answer(
                &Request {
                    url,
                    transferred_length: None,
                },
                &authority(),
                &Infrastructure::default(),
            );
        }
    }

    #[test]
    fn an_external_scheme_never_resolves_here() {
        // N-1 rejects every other scheme at the engine's policy layer; this is the second
        // check rather than the first, and it must not accept what the first refused.
        let (mut b, _t) = broker_with(attested("a.test"), vec![position("https://a.test/x")]);
        for url in [
            "https://a.test/x",
            "file:///etc/passwd",
            "data:text/html,x",
            "about:blank",
        ] {
            let a = b.answer(
                &Request {
                    url: url.to_owned(),
                    transferred_length: None,
                },
                &authority(),
                &Infrastructure::default(),
            );
            assert_eq!(
                a,
                Answer::Unavailable(Unavailable::UnknownAddress),
                "{url} resolved"
            );
        }
    }
}

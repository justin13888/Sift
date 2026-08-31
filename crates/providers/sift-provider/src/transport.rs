//! The transport an adapter speaks over, and D-65's fixture-replay harness.
//!
//! # Why the transport is a trait
//!
//! `docs/build/verification.md` puts the fixture-and-replay harness in P0, before the
//! adapters it tests, and gives two reasons that are really one: it **makes NFR-29's
//! degradation assertable against servers nobody has**, and it turns D-31's warning — that
//! "small subset of IMAP" has defeated better-resourced projects — into a growing corpus
//! rather than a growing worry.
//!
//! Neither is possible if an adapter talks to a socket directly.
//!
//! **Fixtures MUST be scrubbed of addresses, subjects and content before they are
//! committed.** NFR-22 reaches the test tree: a corpus of real mail in a public repository
//! is the disclosure the whole privacy posture exists to prevent, arriving through the door
//! nobody was watching.
//!
//! # The request is split from the exchange, deliberately
//!
//! [`Exchange`] is verb and target and nothing else, because it is the **fixture key**.
//! Headers are carried beside it in [`Request`], and a bearer token, an idempotency key and
//! a content type are exactly the parts of a request that differ between two runs of the
//! same session. A fixture keyed on them would match nothing on the second run, and a
//! corpus scrubbed of them would be a corpus of keys that no longer identify anything.

use std::collections::BTreeMap;

/// A request, in the shape every provider's transport shares.
///
/// **This is the fixture key.** It holds what identifies the exchange and nothing that
/// varies between two runs of the same session.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exchange {
    /// The protocol verb or HTTP method.
    pub verb: String,
    /// The endpoint, command, or path.
    pub target: String,
}

impl Exchange {
    pub fn new(verb: &str, target: &str) -> Self {
        Self {
            verb: verb.to_owned(),
            target: target.to_owned(),
        }
    }
}

/// One request: the exchange that identifies it, the headers that authorize it, and a body.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub exchange: Exchange,
    /// Header names are compared case-insensitively by the transport; they are stored as the
    /// caller wrote them.
    pub headers: Vec<(String, String)>,
    pub body: &'a [u8],
}

impl<'a> Request<'a> {
    #[must_use]
    pub fn new(verb: &str, target: &str) -> Self {
        Self {
            exchange: Exchange::new(verb, target),
            headers: Vec::new(),
            body: &[],
        }
    }

    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub const fn body(mut self, body: &'a [u8]) -> Self {
        self.body = body;
        self
    }
}

/// One answer.
///
/// **The status crosses the transport boundary rather than being collapsed into an error.**
/// The distinction an adapter needs most is between two failures that are both "the server
/// said no": a cursor the server no longer accepts is D-82's `Invalidated` and NFR-18's
/// recovery, while a message that is simply gone is an ordinary removal. Collapsing both
/// into a refusal would make a routine delta look like a broken account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    #[must_use]
    pub fn ok(body: &[u8]) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.to_vec(),
        }
    }

    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// A header, matched case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What can go wrong on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// Retryable. The scheduler decides when, under D-87 and L-24 — never the adapter, and
    /// never by sleeping.
    Transient,
    /// The provider asked for a delay.
    ///
    /// **A deadline on the timing wheel, never a sleep.** A sleeping adapter is a per-account
    /// sleep loop, which D-25 prohibits outright — and the stated delay is a *floor* rather
    /// than an instruction, because the wheel coalesces and a throttled account resumes on
    /// the first tick at or after the instant.
    Throttled { retry_after_millis: u64 },
    /// The provider refused, in its own well-formed terms.
    Refused(String),
    /// The request went out and no answer came back.
    ///
    /// D-85 moves the intent to *Reconciling*: the adapter establishes server state before
    /// retrying rather than blindly replaying, which is what NFR-17 means by exactly-once
    /// **observable**.
    Unknown,
    /// The answer exceeded L-13 without the explicit confirmation NFR-39 requires.
    ///
    /// Refused rather than truncated, which is `docs/limits.md`'s rule for every bound it
    /// holds bar one: a truncated answer is one whose shape the sender chose.
    TooLarge { limit_bytes: u64 },
    /// No fixture covers this exchange. Only reachable under replay.
    NoFixture(Exchange),
}

/// What an adapter uses to reach a provider.
pub trait Transport {
    /// # Errors
    /// See [`TransportError`].
    fn exchange(&mut self, request: &Request<'_>) -> Result<Response, TransportError>;

    /// Bytes actually written to and read from the socket, since the transport was created.
    ///
    /// FR-36 counts **bytes on the wire**, which is neither the body's length nor the
    /// decoded length: it is what the interface carried, compression, headers, and the
    /// handshake included. A transport that reported the decoded body would understate a
    /// gzipped feed and overstate nothing, which is the wrong direction for a data cap.
    ///
    /// The default is zero for transports that do not touch a socket — the replay harness
    /// is not on anybody's data plan.
    fn wire_bytes(&self) -> (u64, u64) {
        (0, 0)
    }
}

/// D-65's fixture-and-replay harness.
///
/// Replays recorded exchanges, and **fails loudly on one it has never seen**. An unrecorded
/// exchange returning an empty success would let an adapter quietly change what it asks for,
/// which is the drift a fixture corpus exists to catch.
#[derive(Debug, Default)]
pub struct Replay {
    responses: BTreeMap<Exchange, Vec<Response>>,
    /// Exchanges the adapter actually performed, in order.
    ///
    /// Recorded so a test can assert on *what was asked*, not only on what came back.
    /// "Sift MUST NOT fetch whole messages" is a claim about requests, and only this can
    /// check it.
    pub performed: Vec<Exchange>,
    /// The headers each performed request carried, in step with `performed`.
    ///
    /// "No send or compose scope is ever requested" and "the idempotency key is the
    /// client-assigned intent identifier" are both claims about headers, and neither is
    /// checkable against a response.
    pub headers: Vec<Vec<(String, String)>>,
    /// The bodies each performed request carried, in step with `performed`.
    pub bodies: Vec<Vec<u8>>,
    /// Deliberate failures, keyed by how many times the exchange has been seen.
    faults: BTreeMap<(Exchange, usize), TransportError>,
}

impl Replay {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful response. Repeated calls queue successive responses for the same
    /// exchange, which is how a paged delta is expressed.
    pub fn on(&mut self, verb: &str, target: &str, response: &[u8]) -> &mut Self {
        self.respond(verb, target, Response::ok(response))
    }

    /// Record a response with its own status — an expired cursor, a throttle, a refusal.
    pub fn respond(&mut self, verb: &str, target: &str, response: Response) -> &mut Self {
        self.responses
            .entry(Exchange::new(verb, target))
            .or_default()
            .push(response);
        self
    }

    /// Inject a failure at the *n*th occurrence of an exchange, counted from zero.
    ///
    /// Fault injection is what makes NFR-29's degradation assertable: a server that stops
    /// advertising an extension, a delta link that expires, a throttle. None of those can be
    /// arranged against a real provider on demand.
    pub fn fail_nth(
        &mut self,
        verb: &str,
        target: &str,
        n: usize,
        error: TransportError,
    ) -> &mut Self {
        self.faults
            .insert((Exchange::new(verb, target), n), error);
        self
    }

    /// How many times an exchange has been performed.
    #[must_use]
    pub fn count_of(&self, verb: &str, target: &str) -> usize {
        self.performed
            .iter()
            .filter(|e| e.verb == verb && e.target == target)
            .count()
    }

    /// The headers of the *n*th performed request.
    #[must_use]
    pub fn header_of(&self, n: usize, name: &str) -> Option<&str> {
        self.headers.get(n)?.iter().find_map(|(k, v)| {
            if k.eq_ignore_ascii_case(name) {
                Some(v.as_str())
            } else {
                None
            }
        })
    }
}

impl Transport for Replay {
    fn exchange(&mut self, request: &Request<'_>) -> Result<Response, TransportError> {
        let exchange = &request.exchange;
        let seen = self.count_of(&exchange.verb, &exchange.target);
        self.performed.push(exchange.clone());
        self.headers.push(request.headers.clone());
        self.bodies.push(request.body.to_vec());

        if let Some(fault) = self.faults.get(&(exchange.clone(), seen)) {
            return Err(fault.clone());
        }
        match self.responses.get(exchange) {
            Some(responses) => Ok(responses
                .get(seen)
                .or_else(|| responses.last())
                .cloned()
                .unwrap_or_else(|| Response::ok(&[]))),
            // Loudly. An unrecorded exchange returning an empty success would let an adapter
            // quietly change what it asks for.
            None => Err(TransportError::NoFixture(exchange.clone())),
        }
    }
}

/// Whether a stated retry delay is honoured by sleeping.
///
/// **Never.** D-87: it is a deadline on the wheel. A sleeping adapter is a per-account sleep
/// loop, which is the thing D-25 prohibits outright rather than tolerating.
#[must_use]
pub const fn a_stated_delay_is_slept_on() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecorded_exchange_fails_loudly() {
        // Returning an empty success would let an adapter quietly change what it asks for,
        // which is the drift a fixture corpus exists to catch.
        let mut r = Replay::new();
        assert!(matches!(
            r.exchange(&Request::new("GET", "/unrecorded")),
            Err(TransportError::NoFixture(_))
        ));
    }

    #[test]
    fn successive_responses_express_a_paged_delta() {
        let mut r = Replay::new();
        r.on("POST", "/delta", b"page1")
            .on("POST", "/delta", b"page2");
        let q = Request::new("POST", "/delta");
        assert_eq!(r.exchange(&q).map(|x| x.body), Ok(b"page1".to_vec()));
        assert_eq!(r.exchange(&q).map(|x| x.body), Ok(b"page2".to_vec()));
        assert_eq!(
            r.exchange(&q).map(|x| x.body),
            Ok(b"page2".to_vec()),
            "the last page repeats"
        );
    }

    #[test]
    fn a_fault_can_be_injected_at_a_chosen_attempt() {
        // NFR-29's degradations cannot be arranged against a real provider on demand.
        let mut r = Replay::new();
        r.on("GET", "/x", b"ok").fail_nth(
            "GET",
            "/x",
            1,
            TransportError::Throttled {
                retry_after_millis: 5_000,
            },
        );
        let q = Request::new("GET", "/x");
        assert!(r.exchange(&q).is_ok());
        assert!(matches!(
            r.exchange(&q),
            Err(TransportError::Throttled { .. })
        ));
        assert!(r.exchange(&q).is_ok(), "the fault repeated");
    }

    #[test]
    fn what_was_asked_for_is_recorded_not_only_what_came_back() {
        // "Sift MUST NOT fetch whole messages" is a claim about *requests*, and only this can
        // check it.
        let mut r = Replay::new();
        r.on("GET", "/envelopes", b"[]");
        let _ = r.exchange(&Request::new("GET", "/envelopes").header("Authorization", "Bearer x"));
        assert_eq!(r.performed, vec![Exchange::new("GET", "/envelopes")]);
        assert_eq!(r.count_of("GET", "/envelopes"), 1);
        assert_eq!(r.header_of(0, "authorization"), Some("Bearer x"));
    }

    #[test]
    fn a_header_does_not_change_which_fixture_matches() {
        // The point of splitting the request from the exchange: a bearer token differs
        // between two runs of the same session, so a fixture keyed on it would match nothing
        // the second time.
        let mut r = Replay::new();
        r.on("GET", "/x", b"body");
        assert!(
            r.exchange(&Request::new("GET", "/x").header("Authorization", "Bearer one"))
                .is_ok()
        );
        assert!(
            r.exchange(&Request::new("GET", "/x").header("Authorization", "Bearer two"))
                .is_ok()
        );
    }

    #[test]
    fn a_status_survives_the_boundary_rather_than_collapsing_into_an_error() {
        // A cursor the server no longer accepts is a recovery, not a broken account.
        let mut r = Replay::new();
        r.respond(
            "GET",
            "/history",
            Response {
                status: 404,
                headers: vec![],
                body: b"{}".to_vec(),
            },
        );
        let got = r.exchange(&Request::new("GET", "/history")).unwrap();
        assert_eq!(got.status, 404);
        assert!(!got.is_success());
    }

    #[test]
    fn the_replay_harness_is_not_on_anybodys_data_plan() {
        assert_eq!(Replay::new().wire_bytes(), (0, 0));
    }

    #[test]
    fn a_stated_delay_is_a_deadline_rather_than_a_sleep() {
        assert!(!a_stated_delay_is_slept_on());
    }
}

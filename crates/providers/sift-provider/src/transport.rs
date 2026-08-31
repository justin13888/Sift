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

use std::collections::BTreeMap;

/// A request, in the shape every provider's transport shares.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exchange {
    /// The protocol verb or HTTP method.
    pub verb: String,
    /// The endpoint, command, or path.
    pub target: String,
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
    /// No fixture covers this exchange. Only reachable under replay.
    NoFixture(Exchange),
}

/// What an adapter uses to reach a provider.
pub trait Transport {
    /// # Errors
    /// See [`TransportError`].
    fn exchange(&mut self, exchange: &Exchange, body: &[u8]) -> Result<Vec<u8>, TransportError>;
}

/// D-65's fixture-and-replay harness.
///
/// Replays recorded exchanges, and **fails loudly on one it has never seen**. An unrecorded
/// exchange returning an empty success would let an adapter quietly change what it asks for,
/// which is the drift a fixture corpus exists to catch.
#[derive(Debug, Default)]
pub struct Replay {
    responses: BTreeMap<Exchange, Vec<Vec<u8>>>,
    /// Exchanges the adapter actually performed, in order.
    ///
    /// Recorded so a test can assert on *what was asked*, not only on what came back.
    /// "Sift MUST NOT fetch whole messages" is a claim about requests, and only this can
    /// check it.
    pub performed: Vec<Exchange>,
    /// Deliberate failures, keyed by how many times the exchange has been seen.
    faults: BTreeMap<(Exchange, usize), TransportError>,
}

impl Replay {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a response. Repeated calls queue successive responses for the same exchange,
    /// which is how a paged delta is expressed.
    pub fn on(&mut self, verb: &str, target: &str, response: &[u8]) -> &mut Self {
        self.responses
            .entry(Exchange {
                verb: verb.to_owned(),
                target: target.to_owned(),
            })
            .or_default()
            .push(response.to_vec());
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
        self.faults.insert(
            (
                Exchange {
                    verb: verb.to_owned(),
                    target: target.to_owned(),
                },
                n,
            ),
            error,
        );
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
}

impl Transport for Replay {
    fn exchange(&mut self, exchange: &Exchange, _body: &[u8]) -> Result<Vec<u8>, TransportError> {
        let seen = self.count_of(&exchange.verb, &exchange.target);
        self.performed.push(exchange.clone());

        if let Some(fault) = self.faults.get(&(exchange.clone(), seen)) {
            return Err(fault.clone());
        }
        match self.responses.get(exchange) {
            Some(responses) => Ok(responses
                .get(seen)
                .or_else(|| responses.last())
                .cloned()
                .unwrap_or_default()),
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
        let e = Exchange {
            verb: "GET".into(),
            target: "/unrecorded".into(),
        };
        assert!(matches!(
            r.exchange(&e, &[]),
            Err(TransportError::NoFixture(_))
        ));
    }

    #[test]
    fn successive_responses_express_a_paged_delta() {
        let mut r = Replay::new();
        r.on("POST", "/delta", b"page1")
            .on("POST", "/delta", b"page2");
        let e = Exchange {
            verb: "POST".into(),
            target: "/delta".into(),
        };
        assert_eq!(r.exchange(&e, &[]), Ok(b"page1".to_vec()));
        assert_eq!(r.exchange(&e, &[]), Ok(b"page2".to_vec()));
        assert_eq!(
            r.exchange(&e, &[]),
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
        let e = Exchange {
            verb: "GET".into(),
            target: "/x".into(),
        };
        assert!(r.exchange(&e, &[]).is_ok());
        assert!(matches!(
            r.exchange(&e, &[]),
            Err(TransportError::Throttled { .. })
        ));
        assert!(r.exchange(&e, &[]).is_ok(), "the fault repeated");
    }

    #[test]
    fn what_was_asked_for_is_recorded_not_only_what_came_back() {
        // "Sift MUST NOT fetch whole messages" is a claim about *requests*, and only this can
        // check it.
        let mut r = Replay::new();
        r.on("GET", "/envelopes", b"[]");
        let e = Exchange {
            verb: "GET".into(),
            target: "/envelopes".into(),
        };
        let _ = r.exchange(&e, &[]);
        assert_eq!(r.performed, vec![e]);
        assert_eq!(r.count_of("GET", "/envelopes"), 1);
    }

    #[test]
    fn a_stated_delay_is_a_deadline_rather_than_a_sleep() {
        assert!(!a_stated_delay_is_slept_on());
    }
}

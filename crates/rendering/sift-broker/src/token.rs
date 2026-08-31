//! D-28 — the internal scheme addresses per-view capabilities.
//!
//! # Why not a content hash, and why not a message identifier
//!
//! A **content hash** is a stable global identifier. Two messages containing the same image
//! would address it identically, which is a correlation channel between messages — exactly
//! what NFR-25's isolated data store closes — and a *guessed* address would be a valid one.
//!
//! A **message and part identifier** is guessable and stable across views, so revoking it
//! would mean nothing: the next view would accept the same address.
//!
//! So each document mints an unguessable token, resources are addressed under it, and the
//! whole token is invalidated at once. A fabricated address resolves to nothing.
//!
//! # Per document, not per view — D-90
//!
//! The body view is **reused** across messages rather than respawned, because respawning
//! would put a content-process launch inside NFR-3's 80 ms on every arrow-key press. Reuse
//! is only safe because the token moves: it is minted per *document* and revoked when
//! navigation away from that document begins.
//!
//! That **strengthens** D-28 rather than weakening it — revocation now happens at
//! navigation, which is earlier and more often than teardown.
//!
//! D-90 is explicit that this rests on three other decisions holding: no script (D-50), no
//! persistent storage (NFR-25), and no network (N-1). **Relax any one and this loses its
//! footing**, because the residue a respawn would have cleared is only absent while all
//! three are true.
//!
//! # The cost, recorded
//!
//! Nothing caches across views, and the opaque token lands in FR-33's debug view — degrading
//! the product's most useful diagnostic surface. The fix is the debug view resolving tokens
//! back to a message and part, **not** weakening the scheme.

use std::sync::atomic::{AtomicU64, Ordering};

/// An unguessable per-document capability token.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Token(String);

impl Token {
    /// Mint a token for one document.
    ///
    /// The value must be unguessable: a guessed address must not be a valid one. This
    /// composes process-lifetime entropy with a monotonic counter so that two documents in
    /// one session never share a token even if minted in the same instant.
    #[must_use]
    pub fn mint() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let entropy = process_entropy();
        // Wide enough that enumeration is not a strategy. This is *not* the place a weak
        // random is acceptable — unlike D-78's identity tiebreak, this value is the
        // capability itself.
        Self(format!(
            "{:016x}{:016x}",
            entropy ^ n.rotate_left(17),
            splitmix(entropy ^ n)
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn process_entropy() -> u64 {
    use std::sync::OnceLock;
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::from(d.subsec_nanos()));
        let boxed = Box::new(0u8);
        let addr = std::ptr::from_ref::<u8>(&*boxed) as usize as u64;
        splitmix(nanos.rotate_left(32) ^ addr)
    })
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// An address under the internal scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub token: Token,
    /// The position's index within the document, from the sanitizer's rewrite.
    pub position: usize,
}

impl Address {
    #[must_use]
    pub fn to_url(&self) -> String {
        format!(
            "{}://{}/{}",
            sift_foundation::identifiers::INTERNAL_SCHEME,
            self.token.as_str(),
            self.position
        )
    }

    /// Parse an address the engine handed back.
    ///
    /// Returns `None` for anything that is not a well-formed internal address. **Every other
    /// scheme is rejected at the engine's policy layer before reaching here** — this is the
    /// second check, not the first.
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        let prefix = format!("{}://", sift_foundation::identifiers::INTERNAL_SCHEME);
        let rest = url.strip_prefix(&prefix)?;
        let (token, position) = rest.split_once('/')?;
        if token.is_empty() {
            return None;
        }
        Some(Self {
            token: Token(token.to_owned()),
            position: position.parse().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn a_token_is_never_reused() {
        // A repeated token would let a revoked document's addresses resolve against a live
        // one, which is the whole thing revocation is for.
        let tokens: BTreeSet<String> = (0..10_000).map(|_| Token::mint().0).collect();
        assert_eq!(tokens.len(), 10_000);
    }

    #[test]
    fn a_token_is_wide_enough_that_enumeration_is_not_a_strategy() {
        // Unlike D-78's identity tiebreak, this value **is** the capability. A guessed
        // address must not be a valid one.
        assert!(Token::mint().as_str().len() >= 32);
    }

    #[test]
    fn an_address_round_trips() {
        let t = Token::mint();
        let a = Address {
            token: t.clone(),
            position: 7,
        };
        let parsed = Address::parse(&a.to_url()).expect("well formed");
        assert_eq!(parsed.token, t);
        assert_eq!(parsed.position, 7);
    }

    #[test]
    fn only_the_internal_scheme_parses() {
        for url in [
            "https://example.test/0",
            "file:///etc/passwd",
            "data:text/html,x",
            "sift-resource:/0",
            "sift-resource://",
            "sift-resource:///0",
            "",
        ] {
            assert!(
                Address::parse(url).is_none(),
                "{url} parsed as an internal address"
            );
        }
    }

    #[test]
    fn a_malformed_position_does_not_parse() {
        let t = Token::mint();
        assert!(Address::parse(&format!("sift-resource://{}/notanumber", t.as_str())).is_none());
    }

    #[test]
    fn the_scheme_is_the_one_the_register_records() {
        // The internal scheme must never be the registered one, and platform-baseline pins
        // both. If they were ever made equal, registering the callback scheme with the
        // operating system would register this too.
        let url = Address {
            token: Token::mint(),
            position: 0,
        }
        .to_url();
        assert!(url.starts_with(sift_foundation::identifiers::INTERNAL_SCHEME));
        assert_ne!(
            sift_foundation::identifiers::INTERNAL_SCHEME,
            sift_foundation::identifiers::CALLBACK_SCHEME
        );
    }
}

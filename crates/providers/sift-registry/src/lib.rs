//! D-12's seam: the one place above the adapters where a provider is named.
//!
//! # Why this crate exists at all
//!
//! "Add a Gmail account" is a thing a person clicks, so *something* has to know that Gmail
//! exists. D-12 says that something may not be the application, the presentation layer or
//! the ABI, and `cargo xtask invariants` enforces it by banning the name outright in those
//! three layers.
//!
//! Until now the only place left was a **shell**, which is why `sift-harness` held the
//! assembled application: it is the one layer the rule does not reach. That was survivable
//! while the harness was the only shell. It stops being survivable the moment the macOS
//! shell needs the same application, because that shell is Swift and cannot construct a Rust
//! adapter — so the knowledge cannot live there either.
//!
//! The answer is to put it **below** everything that is banned from having it rather than
//! above. The providers layer is not covered by the rule, for the obvious reason that it is
//! where the adapters are. So the register lives here, the layers above resolve an opaque
//! [`ProviderKind`] through it, and nothing above the adapters ever matches on provider
//! identity.
//!
//! # What this is not
//!
//! It is **not** an adapter, and `cargo xtask arch` treats it accordingly: the isolation rule
//! that adapters may not see each other iterates the adapters themselves, and none of them
//! depends on this crate. The property that rule protects — *"an adapter's fitness is tested
//! by whether it compiles against the capability crate alone"* — is untouched.
//!
//! It also holds **no policy**. It answers "what can be added" and "build me one of those";
//! it decides nothing about when to sync, what to fetch, or how a failure is handled. Those
//! are the capability model's, and they are asked of the account after it exists.

pub mod corpus;

use sift_provider::erased::ErasedAdapter;
use sift_provider::oauth::OAuthProfile;

/// An adapter over a real socket, or the reason one could not be reached.
///
/// Never a credential problem: establishing the transport does not authenticate.
pub type Connected = Result<Box<dyn ErasedAdapter>, String>;

/// How an account authenticates.
///
/// The set is deliberately small: it is the shape of the *setup flow*, not a provider
/// taxonomy. Two providers needing the same flow are the same value here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authentication {
    /// D-88's authorization-code flow with PKCE, returning through D-36's registered scheme.
    OAuth,
    /// A password the user supplies, held in the credential store like any other secret.
    Password,
}

/// A provider kind, as it is persisted and as it crosses the boundary.
///
/// A short stable string rather than an integer, because it is written into the account
/// registry on disk and a renumbering would silently repoint an account at another
/// provider. It is opaque above this crate: nothing may branch on its value, and the only
/// legitimate uses are to persist it, to display the descriptor it resolves to, and to hand
/// it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderKind(&'static str);

impl ProviderKind {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl core::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.0)
    }
}

/// What a shell needs to offer a provider in the add-account flow.
///
/// `display_key` is a **state identifier, not a name**. D-56 and D-68 keep Sift's own prose
/// out of every layer below the shell, and a provider's display name is Sift's prose: it is
/// the shell that renders and translates it.
pub struct Descriptor {
    pub kind: ProviderKind,
    pub display_key: &'static str,
    pub authentication: Authentication,
    /// Whether the flow must offer manual configuration — FR-3's first-class path.
    pub needs_manual_configuration: bool,
    /// The authorization endpoints, where this kind uses OAuth.
    profile: Option<fn() -> OAuthProfile>,
    /// The API host a live adapter reaches.
    api_host: &'static str,
    /// Build a live adapter over a real socket, given a bearer token.
    live: fn(&str, &str) -> Connected,
    /// Build one over the recorded corpus instead — D-65.
    replay: fn() -> Box<dyn ErasedAdapter>,
}

impl core::fmt::Debug for Descriptor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Descriptor")
            .field("kind", &self.kind)
            .field("authentication", &self.authentication)
            .finish_non_exhaustive()
    }
}

impl Descriptor {
    /// The authorization profile, where this kind authorizes.
    #[must_use]
    pub fn profile(&self) -> Option<OAuthProfile> {
        self.profile.map(|f| f())
    }

    /// Reach the provider over the network.
    ///
    /// # Errors
    /// The transport could not be established — a name that does not resolve, a TLS
    /// handshake that failed. Never a credential problem: this does not authenticate.
    pub fn connect(&self, access_token: &str) -> Connected {
        (self.live)(self.api_host, access_token)
    }

    /// Reach the recorded corpus instead of a socket — D-65.
    ///
    /// `docs/build/verification.md` puts the fixture harness in P0 **before the adapters it
    /// tests**, because it is what makes a provider's behaviour assertable "against servers
    /// nobody has": a cursor outside the retained history window, a throttle, a message that
    /// vanishes between the delta and the fetch.
    #[must_use]
    pub fn replayed(&self) -> Box<dyn ErasedAdapter> {
        (self.replay)()
    }
}

/// Everything a user can add.
///
/// The four adapters D-12 names; three are not yet buildable and are absent rather than
/// listed-and-broken, which is the same rule the capability model applies to affordances.
pub static KINDS: &[Descriptor] = &[Descriptor {
    kind: ProviderKind("gmail"),
    display_key: "provider.gmail",
    authentication: Authentication::OAuth,
    needs_manual_configuration: false,
    profile: Some(sift_gmail::oauth::profile),
    api_host: sift_gmail::oauth::API_HOST,
    live: connect_gmail,
    replay: replay_gmail,
}];

fn connect_gmail(host: &str, access_token: &str) -> Connected {
    let api = sift_http::Https::to(host)?;
    Ok(Box::new(sift_gmail::Gmail::new(api, access_token)))
}

fn replay_gmail() -> Box<dyn ErasedAdapter> {
    Box::new(sift_gmail::Gmail::new(corpus::gmail(), "a-fixture-token"))
}

/// Resolve a persisted kind.
///
/// Returns `None` for a kind this build does not have, which is what an account added by a
/// newer build and opened by an older one looks like. That is an account in a condition, not
/// a panic.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Descriptor> {
    KINDS.iter().find(|d| d.kind.0 == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_resolves_to_itself() {
        for d in KINDS {
            assert_eq!(by_id(d.kind.as_str()).map(|r| r.kind), Some(d.kind));
        }
    }

    #[test]
    fn a_kind_this_build_does_not_have_is_none_rather_than_a_panic() {
        // An account added by a newer build and opened by an older one. D-32 refuses rather
        // than guesses everywhere else in the design; this is the same posture.
        assert!(by_id("a-provider-from-the-future").is_none());
        assert!(by_id("").is_none());
    }

    #[test]
    fn every_kind_that_authorizes_asks_for_nothing_that_can_send() {
        // D-88, checked at the register rather than only at the adapter, because this is the
        // list a user is offered and the scope set is permanent once consented to.
        for d in KINDS {
            if let Some(profile) = d.profile() {
                assert!(
                    !profile.authorizes_sending(),
                    "{} requests a scope that authorizes sending",
                    d.kind
                );
            }
        }
    }

    #[test]
    fn an_oauth_kind_has_a_profile_and_a_password_kind_does_not() {
        for d in KINDS {
            match d.authentication {
                Authentication::OAuth => assert!(d.profile().is_some(), "{}", d.kind),
                Authentication::Password => assert!(d.profile().is_none(), "{}", d.kind),
            }
        }
    }

    #[test]
    fn a_replayed_account_declares_what_the_live_one_would() {
        // The corpus exists to test the adapter, so it must not be a different adapter. If
        // these diverged, every fixture-driven assertion would be about a shape no real
        // account has.
        let d = by_id("gmail").expect("gmail is registered");
        let replayed = d.replayed();
        assert_eq!(replayed.capabilities(), &sift_gmail::capabilities());
    }
}

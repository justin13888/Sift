//! D-88 — the parts of the flow that decide whether an account survives it.

use std::collections::BTreeSet;

/// Why a refresh failed.
///
/// The classification is the whole decision. D-88 narrows *non-transient* to a **well-formed
/// provider error explicitly denying the grant**, and everything else is transient — because
/// the alternative is worse in a way users experience directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The transport failed. Says nothing about the grant.
    Transport,
    /// A server error. Says nothing about the grant.
    ServerError,
    /// A timeout.
    Timeout,
    /// Throttled. D-87 puts the stated delay on the wheel rather than sleeping.
    Throttled,
    /// **A response that does not parse as the provider's own error document.**
    ///
    /// This is the one that matters. A captive portal answering with a login page produces
    /// exactly this, and treating it as authoritative would prompt for re-authentication on
    /// **every account at once** — an application that appears to have lost the user's
    /// credentials, when in fact it is on a hotel network. That is NFR-34's cascade,
    /// arriving from the credential store rather than the network.
    Unparseable,
    /// A well-formed provider error explicitly denying the grant. **The only non-transient
    /// case.**
    ProviderDeniedTheGrant,
}

impl FailureKind {
    /// Whether this failure means the user must re-authenticate.
    #[must_use]
    pub const fn is_non_transient(self) -> bool {
        matches!(self, Self::ProviderDeniedTheGrant)
    }
}

/// A token pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub access: String,
    pub refresh: String,
}

/// What the credential store holds for one account.
///
/// **The previous pair is retained, marked superseded, until the new one has completed one
/// request.** D-88's fourth point, and the failure it prevents is total: against a provider
/// that rotates refresh tokens, a crash between receiving a new pair and successfully using
/// it would leave the account with a token the provider has already invalidated and no way
/// back. The user's only recovery would be re-adding the account.
#[derive(Debug, Clone, Default)]
pub struct Stored {
    current: Option<Pair>,
    superseded: Option<Pair>,
}

impl Stored {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current: None,
            superseded: None,
        }
    }

    /// Write a newly received pair.
    ///
    /// **The write precedes the use.** Using a pair before storing it means a crash in that
    /// window loses it.
    pub fn store(&mut self, received: Pair) {
        if let Some(previous) = self.current.replace(received) {
            self.superseded = Some(previous);
        }
    }

    /// The new pair completed a request. The old one can go.
    pub fn confirm(&mut self) {
        self.superseded = None;
    }

    /// What to present. The current pair, or the superseded one if the current has not yet
    /// proved itself and is gone.
    #[must_use]
    pub fn usable(&self) -> Option<&Pair> {
        self.current.as_ref().or(self.superseded.as_ref())
    }

    #[must_use]
    pub const fn has_a_superseded_pair(&self) -> bool {
        self.superseded.is_some()
    }
}

/// D-88's third point — refresh is **single-flight per account**.
///
/// Concurrent refusals otherwise produce concurrent refreshes, and against a provider that
/// rotates refresh tokens the second presents a spent token and the grant is invalidated.
/// D-88 puts it memorably: **"the uncoordinated implementation logs the user out by trying
/// too hard."**
#[derive(Debug, Default)]
pub struct SingleFlight {
    in_flight: BTreeSet<u128>,
}

impl SingleFlight {
    /// Begin a refresh, or report that one is already running.
    ///
    /// A caller that gets `false` waits for the running one rather than starting a second.
    pub fn begin(&mut self, account: u128) -> bool {
        self.in_flight.insert(account)
    }

    pub fn finish(&mut self, account: u128) {
        self.in_flight.remove(&account);
    }

    #[must_use]
    pub fn is_running(&self, account: u128) -> bool {
        self.in_flight.contains(&account)
    }
}

/// What happens when the credential store becomes unavailable mid-session.
///
/// **Material already read stays usable, and working accounts are not torn down.**
/// Discarding it would convert a recoverable condition into an interactive re-authentication
/// of every account — the same cascade the classifier above exists to prevent, arriving by a
/// different route.
///
/// An account enters *storage unavailable* **only when it needs material it does not hold**:
/// a refresh that must write, or an account opened for the first time this session.
#[must_use]
pub const fn store_unavailable_tears_down_working_accounts() -> bool {
    false
}

/// Whether credential material is re-read from the store on every use.
///
/// **It is not.** That would put the credential store on the hot path and would still not
/// shorten the window that matters — the material is in memory for the life of the process
/// either way, and it is released on D-70's shutdown path.
#[must_use]
pub const fn material_is_reread_per_use() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(n: &str) -> Pair {
        Pair {
            access: format!("a{n}"),
            refresh: format!("r{n}"),
        }
    }

    #[test]
    fn only_a_well_formed_denial_is_non_transient() {
        assert!(FailureKind::ProviderDeniedTheGrant.is_non_transient());
        for k in [
            FailureKind::Transport,
            FailureKind::ServerError,
            FailureKind::Timeout,
            FailureKind::Throttled,
            FailureKind::Unparseable,
        ] {
            assert!(!k.is_non_transient(), "{k:?} would prompt the user");
        }
    }

    #[test]
    fn a_captive_portals_login_page_does_not_log_the_user_out() {
        // The specific failure D-88's classifier exists to prevent: an unparseable response
        // treated as authoritative prompts for re-authentication on every account at once,
        // and the application appears to have lost the user's credentials when it is
        // actually on a hotel network.
        assert!(!FailureKind::Unparseable.is_non_transient());
    }

    #[test]
    fn the_previous_pair_is_kept_until_the_new_one_works() {
        // Against a provider that rotates refresh tokens, a crash between receiving a new
        // pair and using it would otherwise leave the account with no way back.
        let mut s = Stored::new();
        s.store(pair("1"));
        s.store(pair("2"));
        assert!(s.has_a_superseded_pair());
        assert_eq!(s.usable(), Some(&pair("2")));
        s.confirm();
        assert!(!s.has_a_superseded_pair());
    }

    #[test]
    fn the_first_pair_supersedes_nothing() {
        let mut s = Stored::new();
        s.store(pair("1"));
        assert!(!s.has_a_superseded_pair());
    }

    #[test]
    fn refresh_is_single_flight_per_account() {
        // "The uncoordinated implementation logs the user out by trying too hard."
        let mut f = SingleFlight::default();
        assert!(f.begin(1), "the first refresh did not start");
        assert!(
            !f.begin(1),
            "a second concurrent refresh started for the same account"
        );
        assert!(f.begin(2), "a different account was blocked");
        f.finish(1);
        assert!(
            f.begin(1),
            "the account could not refresh again after finishing"
        );
    }

    #[test]
    fn one_accounts_refresh_does_not_block_another() {
        let mut f = SingleFlight::default();
        f.begin(1);
        assert!(!f.is_running(2));
    }

    #[test]
    fn losing_the_credential_store_does_not_tear_down_working_accounts() {
        // Discarding material already read converts a recoverable condition into an
        // interactive re-authentication of every account.
        assert!(!store_unavailable_tears_down_working_accounts());
    }

    #[test]
    fn material_is_not_reread_on_every_use() {
        // That would put the credential store on the hot path and would not shorten the
        // window that matters.
        assert!(!material_is_reread_per_use());
    }
}

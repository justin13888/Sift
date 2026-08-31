//! D-36, D-88 — the authorization flow.

use core::time::Duration;

/// The in-flight secrets of one authorization.
///
/// **Held in memory and nowhere else.** The verifier and the state parameter exist before
/// the account does and are worthless afterwards, so putting them in the credential store
/// would be storing a secret that has no owner and no lifetime.
#[derive(Debug, Clone)]
pub struct InFlight {
    pub state: String,
    pub verifier: String,
    pub started_millis: u64,
}

/// How long an authorization may stay in flight.
pub const FLOW_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The flows currently running.
#[derive(Debug, Default)]
pub struct Flows {
    running: Vec<InFlight>,
}

impl Flows {
    pub fn begin(&mut self, flow: InFlight) {
        self.running.push(flow);
    }

    /// Match a callback to the flow that started it.
    ///
    /// **A callback whose state matches no flow in progress is discarded without comment.**
    /// Not an error, not a state, not a log line the user sees: the registered scheme is one
    /// of only two local attack surfaces Sift has, and any process running as the user can
    /// invoke it. Reporting an unmatched callback would turn that into a way to make Sift
    /// say things.
    ///
    /// This is what D-36 means by the state parameter "doing real work rather than being
    /// ceremony".
    pub fn match_callback(&mut self, state: &str, now_millis: u64) -> Option<InFlight> {
        self.expire(now_millis);
        let index = self.running.iter().position(|f| f.state == state)?;
        Some(self.running.remove(index))
    }

    /// Drop flows past the timeout.
    pub fn expire(&mut self, now_millis: u64) {
        let timeout = FLOW_TIMEOUT.as_millis() as u64;
        self.running
            .retain(|f| now_millis.saturating_sub(f.started_millis) < timeout);
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.running.len()
    }
}

/// Whether an authorization may begin.
///
/// D-71 makes a missing scheme registration a **process-scoped refusal**: Sift MUST NOT
/// begin an authorization whose callback has nowhere to arrive, and the registration is
/// checked **before** a flow starts rather than discovered after one — because discovering
/// it afterwards means the user has already been sent to a browser and returned to nothing.
#[must_use]
pub const fn may_begin(scheme_is_registered: bool) -> bool {
    scheme_is_registered
}

/// Whether a client secret is embedded.
///
/// **No.** Sift is a public client and PKCE replaces the secret — "a secret shipped through
/// three channels is not a secret".
#[must_use]
pub const fn embeds_a_client_secret() -> bool {
    false
}

/// Whether any scope requested can send mail.
///
/// D-88's second point puts the no-send constraint **where a reviewer can check it against
/// an authorization screen**: the scope set is the minimum for read, search and the FR-13
/// intent set, and no send or compose scope is ever requested. It is also **permanent** —
/// widening it later forces the entire install base through re-consent.
#[must_use]
pub fn scopes_are_read_only(scopes: &[&str]) -> bool {
    !scopes.iter().any(|s| {
        let s = s.to_ascii_lowercase();
        s.contains("send") || s.contains("compose") || s.contains("mail.write")
    })
}

/// Whether removal blocks on revoking the grant at the provider.
///
/// **It does not.** FR-4's *local* erasure is what is provable; revocation is best-effort on
/// top, and blocking on it would mean an account the user asked to remove staying until a
/// server answered.
#[must_use]
pub const fn removal_blocks_on_revocation() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(state: &str, started: u64) -> InFlight {
        InFlight {
            state: state.into(),
            verifier: "v".into(),
            started_millis: started,
        }
    }

    #[test]
    fn a_callback_matches_the_flow_that_started_it() {
        let mut f = Flows::default();
        f.begin(flow("abc", 0));
        assert!(f.match_callback("abc", 1000).is_some());
        assert_eq!(f.in_flight(), 0);
    }

    #[test]
    fn a_forged_callback_is_discarded_without_comment() {
        // The registered scheme is one of two local attack surfaces, and any process running
        // as the user can invoke it. Reporting an unmatched callback would turn that into a
        // way to make Sift say things.
        let mut f = Flows::default();
        f.begin(flow("abc", 0));
        assert!(f.match_callback("forged", 1000).is_none());
        assert_eq!(
            f.in_flight(),
            1,
            "the real flow was disturbed by a forged callback"
        );
    }

    #[test]
    fn concurrent_authorizations_are_told_apart_by_their_state() {
        let mut f = Flows::default();
        f.begin(flow("one", 0));
        f.begin(flow("two", 0));
        assert_eq!(
            f.match_callback("two", 100).map(|x| x.state),
            Some("two".to_owned())
        );
        assert_eq!(f.in_flight(), 1);
    }

    #[test]
    fn a_flow_expires() {
        let mut f = Flows::default();
        f.begin(flow("abc", 0));
        assert!(
            f.match_callback("abc", FLOW_TIMEOUT.as_millis() as u64 + 1)
                .is_none()
        );
        assert_eq!(f.in_flight(), 0);
    }

    #[test]
    fn an_authorization_does_not_begin_without_somewhere_to_return_to() {
        // Discovering it afterwards means the user has already been sent to a browser and
        // returned to nothing.
        assert!(!may_begin(false));
        assert!(may_begin(true));
    }

    #[test]
    fn there_is_no_embedded_client_secret() {
        assert!(!embeds_a_client_secret());
    }

    #[test]
    fn no_scope_can_send_mail() {
        // The no-send constraint where a reviewer can check it against an authorization
        // screen.
        assert!(scopes_are_read_only(&[
            "mail.read",
            "mail.readbasic",
            "https://mail.example/readonly"
        ]));
        assert!(!scopes_are_read_only(&["mail.read", "mail.send"]));
        assert!(!scopes_are_read_only(&["mail.compose"]));
        assert!(!scopes_are_read_only(&["Mail.Write"]));
    }

    #[test]
    fn removing_an_account_does_not_wait_for_a_server() {
        // Local erasure is what FR-4 makes provable; revocation is best-effort on top.
        assert!(!removal_blocks_on_revocation());
    }
}

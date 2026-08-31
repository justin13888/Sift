//! The authorization flow, driven — D-88 and D-36 in one place.
//!
//! # Why this is here and not in the adapter
//!
//! The adapter declares its endpoints and its scope set as data
//! ([`OAuthProfile`](sift_provider::oauth::OAuthProfile)); this runs the flow against them.
//! Four rules would otherwise exist four times and differ four ways: single-flight refresh,
//! write-before-use, the failure classifier, and the refusal to begin a flow whose callback
//! has nowhere to arrive.
//!
//! # The five points of D-88, and where each one is
//!
//! 1. **A public client with PKCE and no embedded secret.** In
//!    [`sift_provider::oauth`], because the request bodies are built there and a reviewer
//!    checking for a client secret should have one place to look.
//! 2. **A permanent minimum scope set with no send or compose scope ever requested.**
//!    Declared by the adapter, and checked here by [`begin`] before the user is sent
//!    anywhere.
//! 3. **Single-flight refresh per account.** [`Broker::refresh`], over
//!    [`SingleFlight`](crate::refresh::SingleFlight).
//! 4. **Write before use, retaining the previous pair.** [`Broker::store_pair`], and the
//!    credential store's four-item set is what makes it survive a crash.
//! 5. **A refresh failure is non-transient only on a well-formed provider denial.**
//!    [`classify`], over the answer parser in [`sift_provider::oauth`].

use crate::flow::{Flows, InFlight, may_begin};
use crate::refresh::{FailureKind, Pair, SingleFlight};
use crate::store::{CredentialStore, Item, StoreError};
use sift_foundation::identity::AccountId;
use sift_provider::oauth::{
    self, OAuthProfile, Pkce, TokenAnswer, authorization_url, exchange_body, granted_covers,
    refresh_body,
};
use sift_provider::transport::{Request, Transport, TransportError};

/// Why an authorization or a refresh did not produce a usable pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The callback scheme is not registered, so the flow has nowhere to return to.
    ///
    /// Checked **before** a flow starts, per D-36 — discovering it afterwards means the user
    /// has already been sent to a browser and returned to nothing.
    NoCallbackRegistration,
    /// The profile asks for a scope that would authorize sending.
    ///
    /// The no-send constraint, where a reviewer can check it against an authorization
    /// screen. This refuses to *begin*: a granted submission capability is an outbound
    /// message path whether or not any code calls it.
    ScopeWouldAuthorizeSending,
    /// The callback matched no flow in progress.
    ///
    /// **Discarded without comment** rather than reported: the registered scheme is one of
    /// only two local attack surfaces Sift has, and any process running as the user can
    /// invoke it. Reporting an unmatched callback would turn that into a way to make Sift
    /// say things.
    NoSuchFlow,
    /// The user declined, or the provider did.
    Declined(String),
    /// The provider granted less than was asked for.
    NarrowerThanAsked { granted: String },
    /// It did not work, and the classification says what that means.
    Failed(FailureKind),
    /// The credential store refused. D-71: a security absence refuses.
    Store(StoreError),
    /// Randomness was unavailable, so PKCE would prove nothing.
    NoEntropy,
}

impl core::fmt::Display for AuthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoCallbackRegistration => {
                write!(f, "the callback scheme is not registered with the system")
            }
            Self::ScopeWouldAuthorizeSending => write!(
                f,
                "the scope set would authorize sending, which this product does not do"
            ),
            Self::NoSuchFlow => write!(f, "no authorization is in progress"),
            Self::Declined(why) => write!(f, "authorization was declined: {why}"),
            Self::NarrowerThanAsked { granted } => {
                write!(f, "the provider granted only `{granted}`")
            }
            Self::Failed(kind) => write!(f, "the token exchange failed: {kind:?}"),
            Self::Store(e) => write!(f, "{e}"),
            Self::NoEntropy => write!(f, "the platform's randomness was unavailable"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<StoreError> for AuthError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Classify what came back from the token endpoint — D-88's fifth point.
#[must_use]
pub fn classify(answer: &TokenAnswer) -> FailureKind {
    match answer {
        // Only this. Everything else is transient, because the alternative is worse in a way
        // users experience directly.
        TokenAnswer::GrantDenied { .. } => FailureKind::ProviderDeniedTheGrant,
        TokenAnswer::Refused { .. } => FailureKind::ServerError,
        TokenAnswer::Unparseable => FailureKind::Unparseable,
        TokenAnswer::Granted(_) => FailureKind::ServerError,
    }
}

fn classify_transport(error: &TransportError) -> FailureKind {
    match error {
        TransportError::Throttled { .. } => FailureKind::Throttled,
        TransportError::Unknown => FailureKind::Timeout,
        _ => FailureKind::Transport,
    }
}

/// What one account's authorization is against.
///
/// The three travel together because they are meaningless apart: the redirect must be one
/// the client is registered for, and the client must be one the endpoints know. Passing them
/// separately let a caller mix a profile from one account with a client identifier from
/// another, which fails at the provider with a message nobody can act on.
#[derive(Debug, Clone)]
pub struct Registration {
    pub profile: OAuthProfile,
    /// **No client secret.** D-88's first point: a public client with PKCE, because "a
    /// secret shipped through three channels is not a secret".
    pub client_id: String,
    /// D-36's registered URI scheme. Never a loopback address — NFR-24 admits no listening
    /// socket for any purpose.
    pub redirect_uri: String,
}

/// NFR-23's single place. Everything that touches credential material goes through here.
#[derive(Debug)]
pub struct Broker<S: CredentialStore> {
    store: S,
    flows: Flows,
    in_flight: SingleFlight,
}

impl<S: CredentialStore> Broker<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            flows: Flows::default(),
            in_flight: SingleFlight::default(),
        }
    }

    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }

    #[must_use]
    pub fn flows_in_progress(&self) -> usize {
        self.flows.in_flight()
    }

    /// Begin an authorization. Returns the address to open in the user's own browser.
    ///
    /// Two refusals happen **before** the user goes anywhere, and both would be discovered
    /// too late otherwise.
    ///
    /// # Errors
    /// See [`AuthError`].
    pub fn begin(
        &mut self,
        registration: &Registration,
        scheme_is_registered: bool,
        now_millis: u64,
    ) -> Result<String, AuthError> {
        if !may_begin(scheme_is_registered) {
            return Err(AuthError::NoCallbackRegistration);
        }
        if registration.profile.authorizes_sending() {
            return Err(AuthError::ScopeWouldAuthorizeSending);
        }
        let pkce = Pkce::generate().map_err(|_| AuthError::NoEntropy)?;
        let state = oauth::state().map_err(|_| AuthError::NoEntropy)?;
        let url = authorization_url(
            &registration.profile,
            &registration.client_id,
            &registration.redirect_uri,
            &state,
            &pkce,
        );
        self.flows.begin(InFlight {
            state,
            verifier: pkce.verifier,
            started_millis: now_millis,
        });
        Ok(url)
    }

    /// Complete an authorization from the address the system handed back.
    ///
    /// # Errors
    /// See [`AuthError`].
    pub fn complete<T: Transport>(
        &mut self,
        transport: &mut T,
        registration: &Registration,
        account: AccountId,
        callback: &str,
        now_millis: u64,
    ) -> Result<Pair, AuthError> {
        let profile = &registration.profile;
        let state = oauth::callback_parameter(callback, "state").ok_or(AuthError::NoSuchFlow)?;
        let flow = self
            .flows
            .match_callback(&state, now_millis)
            .ok_or(AuthError::NoSuchFlow)?;

        // The user said no, or the provider did. Either way it is a decision rather than a
        // failure, and it must not present as one.
        if let Some(declined) = oauth::callback_parameter(callback, "error") {
            return Err(AuthError::Declined(declined));
        }
        let code = oauth::callback_parameter(callback, "code").ok_or(AuthError::NoSuchFlow)?;

        let body = exchange_body(
            &registration.client_id,
            &registration.redirect_uri,
            &code,
            &flow.verifier,
        );
        let answer = self.post_token(transport, profile, body.as_bytes())?;
        let TokenAnswer::Granted(grant) = answer else {
            return Err(AuthError::Failed(classify(&answer)));
        };
        if !granted_covers(&profile.scopes, grant.scope.as_deref()) {
            return Err(AuthError::NarrowerThanAsked {
                granted: grant.scope.unwrap_or_default(),
            });
        }
        let pair = Pair {
            access: grant.access,
            refresh: grant.refresh.unwrap_or_default(),
        };
        self.store_pair(account, &pair)?;
        Ok(pair)
    }

    /// The pair to present, read from the store.
    ///
    /// # Errors
    /// See [`AuthError`].
    pub fn usable(&self, account: AccountId) -> Result<Pair, AuthError> {
        let access = self
            .store
            .read(account, Item::Access)
            .or_else(|_| self.store.read(account, Item::SupersededAccess))?;
        let refresh = self
            .store
            .read(account, Item::Refresh)
            .or_else(|_| self.store.read(account, Item::SupersededRefresh))?;
        Ok(Pair { access, refresh })
    }

    /// Refresh an account's pair — D-88's third and fourth points together.
    ///
    /// # Errors
    /// See [`AuthError`].
    pub fn refresh<T: Transport>(
        &mut self,
        transport: &mut T,
        registration: &Registration,
        account: AccountId,
    ) -> Result<Pair, AuthError> {
        // Concurrent refusals otherwise produce concurrent refreshes, and against a provider
        // that rotates refresh tokens the second presents a spent token and the grant is
        // invalidated. "The uncoordinated implementation logs the user out by trying too
        // hard."
        if !self.in_flight.begin(account.as_u128()) {
            return Err(AuthError::Failed(FailureKind::Transport));
        }
        let outcome = self.refresh_inner(transport, registration, account);
        self.in_flight.finish(account.as_u128());
        outcome
    }

    fn refresh_inner<T: Transport>(
        &mut self,
        transport: &mut T,
        registration: &Registration,
        account: AccountId,
    ) -> Result<Pair, AuthError> {
        let held = self.usable(account)?;
        let body = refresh_body(&registration.client_id, &held.refresh);
        let answer = self.post_token(transport, &registration.profile, body.as_bytes())?;
        let TokenAnswer::Granted(grant) = answer else {
            return Err(AuthError::Failed(classify(&answer)));
        };
        let pair = Pair {
            access: grant.access,
            // A provider that does not rotate returns no new refresh token, and the one
            // held is still good. Treating the absence as a loss would break the account.
            refresh: grant.refresh.unwrap_or(held.refresh),
        };
        self.store_pair(account, &pair)?;
        Ok(pair)
    }

    /// D-88's fourth point: **the write precedes the use, and the previous pair is kept.**
    ///
    /// The failure it prevents is total. Against a provider that rotates refresh tokens, a
    /// crash between receiving a new pair and successfully using it would leave the account
    /// with a token the provider has already invalidated and no way back — the user's only
    /// recovery being to re-add the account.
    ///
    /// # Errors
    /// [`AuthError::Store`] where the credential store refused.
    pub fn store_pair(&self, account: AccountId, pair: &Pair) -> Result<(), AuthError> {
        if let Ok(previous) = self.store.read(account, Item::Access) {
            self.store
                .write(account, Item::SupersededAccess, &previous)?;
        }
        if let Ok(previous) = self.store.read(account, Item::Refresh) {
            self.store
                .write(account, Item::SupersededRefresh, &previous)?;
        }
        self.store.write(account, Item::Access, &pair.access)?;
        self.store.write(account, Item::Refresh, &pair.refresh)?;
        Ok(())
    }

    /// The new pair completed a request. The old one can go.
    ///
    /// # Errors
    /// [`AuthError::Store`] where the credential store refused.
    pub fn confirm(&self, account: AccountId) -> Result<(), AuthError> {
        self.store.delete(account, Item::SupersededAccess)?;
        self.store.delete(account, Item::SupersededRefresh)?;
        Ok(())
    }

    /// FR-4: erase every credential this account has, and prove it by enumeration.
    ///
    /// # Errors
    /// [`AuthError::Store`] where the credential store refused.
    pub fn erase(&self, account: AccountId) -> Result<(), AuthError> {
        self.store.erase(account)?;
        Ok(())
    }

    fn post_token<T: Transport>(
        &self,
        transport: &mut T,
        profile: &OAuthProfile,
        body: &[u8],
    ) -> Result<TokenAnswer, AuthError> {
        let request = Request::new("POST", &profile.token.path)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body);
        match transport.exchange(&request) {
            Ok(response) => Ok(oauth::read_token_answer(response.status, &response.body)),
            Err(e) => Err(AuthError::Failed(classify_transport(&e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_provider::oauth::Endpoint;
    use sift_provider::transport::{Replay, Response};
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// A store that keeps items in memory, so the flow can be tested without a keychain.
    ///
    /// **Not a substitute for the platform store** — it exists only here, and the real one
    /// has its own round-trip test against the platform.
    #[derive(Debug, Default)]
    struct InMemory(RefCell<BTreeMap<String, String>>);

    impl CredentialStore for InMemory {
        fn write(&self, a: AccountId, i: Item, secret: &str) -> Result<(), StoreError> {
            self.0
                .borrow_mut()
                .insert(crate::store::key_for(a, i), secret.to_owned());
            Ok(())
        }
        fn read(&self, a: AccountId, i: Item) -> Result<String, StoreError> {
            self.0
                .borrow()
                .get(&crate::store::key_for(a, i))
                .cloned()
                .ok_or(StoreError::NotFound)
        }
        fn delete(&self, a: AccountId, i: Item) -> Result<(), StoreError> {
            self.0.borrow_mut().remove(&crate::store::key_for(a, i));
            Ok(())
        }
    }

    fn profile() -> OAuthProfile {
        OAuthProfile {
            authorize: Endpoint::new("auth.example.test", "/authorize"),
            token: Endpoint::new("token.example.test", "/token"),
            revoke: None,
            scopes: vec!["read".into(), "modify".into()],
            authorize_parameters: vec![],
            sending_scopes: vec!["https://mail.example.test/".into()],
        }
    }

    fn broker() -> Broker<InMemory> {
        Broker::new(InMemory::default())
    }

    fn registration() -> Registration {
        Registration {
            profile: profile(),
            client_id: "c".into(),
            redirect_uri: "net.example:/cb".into(),
        }
    }

    fn account() -> AccountId {
        AccountId::from_u128(1)
    }

    fn granting(body: &str) -> Replay {
        let mut r = Replay::new();
        r.on("POST", "/token", body.as_bytes());
        r
    }

    fn state_of(url: &str) -> String {
        oauth::callback_parameter(url, "state").expect("no state in the authorization url")
    }

    #[test]
    fn an_authorization_does_not_begin_without_somewhere_to_return_to() {
        // D-36, checked before the user is sent to a browser rather than after they come
        // back to nothing.
        let mut b = broker();
        assert_eq!(
            b.begin(&registration(), false, 0),
            Err(AuthError::NoCallbackRegistration)
        );
        assert_eq!(b.flows_in_progress(), 0);
    }

    #[test]
    fn an_authorization_that_would_ask_for_a_sending_scope_never_starts() {
        // The no-send constraint at the point a reviewer can check it: the consent screen.
        let mut sending = registration();
        sending
            .profile
            .scopes
            .push("https://mail.example.test/".into());
        let mut b = broker();
        assert_eq!(
            b.begin(&sending, true, 0),
            Err(AuthError::ScopeWouldAuthorizeSending)
        );
    }

    #[test]
    fn a_completed_flow_stores_the_pair_before_anything_uses_it() {
        // D-88's fourth point. The write precedes the use.
        let mut b = broker();
        let url = b.begin(&registration(), true, 0).unwrap();
        let mut t = granting(r#"{"access_token":"at","refresh_token":"rt","scope":"read modify"}"#);
        let pair = b
            .complete(
                &mut t,
                &registration(),
                account(),
                &format!("net.example:/cb?state={}&code=abc", state_of(&url)),
                0,
            )
            .unwrap();
        assert_eq!(pair.access, "at");
        assert_eq!(b.store().read(account(), Item::Access).unwrap(), "at");
        assert_eq!(b.store().read(account(), Item::Refresh).unwrap(), "rt");
    }

    #[test]
    fn the_exchange_sends_the_verifier_that_matches_the_challenge_and_no_secret() {
        let mut b = broker();
        let url = b.begin(&registration(), true, 0).unwrap();
        let challenge = oauth::callback_parameter(&url, "code_challenge").unwrap();
        let mut t = granting(r#"{"access_token":"at","refresh_token":"rt"}"#);
        b.complete(
            &mut t,
            &registration(),
            account(),
            &format!("net.example:/cb?state={}&code=abc", state_of(&url)),
            0,
        )
        .unwrap();
        let sent = String::from_utf8(t.bodies[0].clone()).unwrap();
        let verifier = sent
            .split('&')
            .find_map(|f| f.strip_prefix("code_verifier="))
            .expect("no verifier was sent");
        assert!(
            !sent.contains("client_secret"),
            "a public client sent a secret"
        );
        // The verifier's own challenge is the one the browser was given. Without this the
        // exchange could send any verifier at all and the test would still pass.
        assert_eq!(
            oauth::base64url(&sha256(verifier.as_bytes())),
            challenge,
            "the verifier does not match the challenge the user authorized against"
        );
    }

    fn sha256(bytes: &[u8]) -> [u8; 32] {
        use sha2::Digest as _;
        sha2::Sha256::digest(bytes).into()
    }

    #[test]
    fn a_forged_callback_is_discarded_and_does_not_disturb_the_real_flow() {
        // Any process running as the user can invoke the registered scheme.
        let mut b = broker();
        let _ = b.begin(&registration(), true, 0).unwrap();
        let mut t = granting(r#"{"access_token":"at"}"#);
        assert_eq!(
            b.complete(
                &mut t,
                &registration(),
                account(),
                "net.example:/cb?state=forged&code=abc",
                0,
            ),
            Err(AuthError::NoSuchFlow)
        );
        assert_eq!(b.flows_in_progress(), 1, "the real flow was disturbed");
        assert!(
            t.performed.is_empty(),
            "a forged callback reached the network"
        );
    }

    #[test]
    fn a_user_who_declines_is_told_that_rather_than_shown_a_failure() {
        let mut b = broker();
        let url = b.begin(&registration(), true, 0).unwrap();
        let mut t = granting(r#"{"access_token":"at"}"#);
        assert_eq!(
            b.complete(
                &mut t,
                &registration(),
                account(),
                &format!(
                    "net.example:/cb?error=access_denied&state={}",
                    state_of(&url)
                ),
                0,
            ),
            Err(AuthError::Declined("access_denied".into()))
        );
    }

    #[test]
    fn a_narrower_grant_than_was_asked_for_stops_the_account_being_added() {
        // The scope set is already the minimum. A narrower grant is an account that will
        // fail later in a way nothing connects back to the consent screen.
        let mut b = broker();
        let url = b.begin(&registration(), true, 0).unwrap();
        let mut t = granting(r#"{"access_token":"at","refresh_token":"rt","scope":"read"}"#);
        assert_eq!(
            b.complete(
                &mut t,
                &registration(),
                account(),
                &format!("net.example:/cb?state={}&code=abc", state_of(&url)),
                0,
            ),
            Err(AuthError::NarrowerThanAsked {
                granted: "read".into()
            })
        );
        assert!(
            b.store().read(account(), Item::Access).is_err(),
            "a pair that does not cover the scope set was stored"
        );
    }

    #[test]
    fn a_refresh_keeps_the_previous_pair_until_the_new_one_is_confirmed() {
        let mut b = broker();
        b.store_pair(
            account(),
            &Pair {
                access: "old-at".into(),
                refresh: "old-rt".into(),
            },
        )
        .unwrap();
        let mut t = granting(r#"{"access_token":"new-at","refresh_token":"new-rt"}"#);
        b.refresh(&mut t, &registration(), account()).unwrap();

        assert_eq!(
            b.store().read(account(), Item::SupersededRefresh).unwrap(),
            "old-rt",
            "the previous pair was discarded before the new one proved itself"
        );
        b.confirm(account()).unwrap();
        assert!(b.store().read(account(), Item::SupersededRefresh).is_err());
    }

    #[test]
    fn a_provider_that_does_not_rotate_does_not_lose_the_refresh_token() {
        let mut b = broker();
        b.store_pair(
            account(),
            &Pair {
                access: "old-at".into(),
                refresh: "kept-rt".into(),
            },
        )
        .unwrap();
        let mut t = granting(r#"{"access_token":"new-at"}"#);
        let pair = b.refresh(&mut t, &registration(), account()).unwrap();
        assert_eq!(pair.refresh, "kept-rt");
    }

    #[test]
    fn a_captive_portal_does_not_log_the_user_out() {
        // NFR-34's cascade, arriving from the credential store rather than the network. An
        // unparseable answer treated as authoritative prompts for re-authentication on
        // every account at once.
        let mut b = broker();
        b.store_pair(
            account(),
            &Pair {
                access: "at".into(),
                refresh: "rt".into(),
            },
        )
        .unwrap();
        let mut t = Replay::new();
        t.respond(
            "POST",
            "/token",
            Response {
                status: 200,
                headers: vec![],
                body: b"<html>Sign in to the network</html>".to_vec(),
            },
        );
        let outcome = b.refresh(&mut t, &registration(), account());
        assert_eq!(outcome, Err(AuthError::Failed(FailureKind::Unparseable)));
        assert!(!FailureKind::Unparseable.is_non_transient());
        assert_eq!(
            b.store().read(account(), Item::Refresh).unwrap(),
            "rt",
            "a hotel network discarded the account's credentials"
        );
    }

    #[test]
    fn only_a_well_formed_denial_asks_the_user_to_sign_in_again() {
        let mut b = broker();
        b.store_pair(
            account(),
            &Pair {
                access: "at".into(),
                refresh: "rt".into(),
            },
        )
        .unwrap();
        let mut t = Replay::new();
        t.respond(
            "POST",
            "/token",
            Response {
                status: 400,
                headers: vec![],
                body: br#"{"error":"invalid_grant"}"#.to_vec(),
            },
        );
        let Err(AuthError::Failed(kind)) = b.refresh(&mut t, &registration(), account()) else {
            panic!("the refresh did not fail");
        };
        assert!(kind.is_non_transient());
    }

    #[test]
    fn a_throttled_refresh_goes_to_the_scheduler_rather_than_to_the_user() {
        let mut b = broker();
        b.store_pair(
            account(),
            &Pair {
                access: "at".into(),
                refresh: "rt".into(),
            },
        )
        .unwrap();
        let mut t = Replay::new();
        t.on("POST", "/token", b"{}").fail_nth(
            "POST",
            "/token",
            0,
            TransportError::Throttled {
                retry_after_millis: 1000,
            },
        );
        assert_eq!(
            b.refresh(&mut t, &registration(), account()),
            Err(AuthError::Failed(FailureKind::Throttled))
        );
        assert!(!FailureKind::Throttled.is_non_transient());
    }

    #[test]
    fn erasure_leaves_nothing_and_says_so_by_enumeration() {
        // FR-4, and the enumeration is what makes the claim checkable rather than hopeful.
        let b = broker();
        b.store_pair(
            account(),
            &Pair {
                access: "at".into(),
                refresh: "rt".into(),
            },
        )
        .unwrap();
        b.store_pair(
            account(),
            &Pair {
                access: "at2".into(),
                refresh: "rt2".into(),
            },
        )
        .unwrap();
        assert_eq!(b.store().remaining(account()).len(), 4);
        b.erase(account()).unwrap();
        assert!(b.store().remaining(account()).is_empty());
    }

    #[test]
    fn erasing_one_account_leaves_another_alone() {
        let b = broker();
        let other = AccountId::from_u128(2);
        for id in [account(), other] {
            b.store_pair(
                id,
                &Pair {
                    access: "at".into(),
                    refresh: "rt".into(),
                },
            )
            .unwrap();
        }
        b.erase(account()).unwrap();
        assert_eq!(b.store().remaining(other).len(), 2);
    }
}

//! Adding an account: the authorization flow, and the adapter it produces.
//!
//! # Why this is not in a shell
//!
//! It was, and then there were two shells. Driving an OAuth flow is not shell work — it is
//! four steps with an ordering that matters, a credential store at the end of it, and one
//! check (is the callback scheme registered?) that must happen *before* the user is sent to a
//! browser rather than after they come back to nothing. A second copy of that in AppKit would
//! be a second answer to every one of those questions.
//!
//! # Naming a provider here is legal, and that is the point of the register
//!
//! Everywhere above the adapter layer plans against declared capabilities, and
//! `cargo xtask invariants` enforces it in the application, presentation and ABI layers. This
//! file is in the application layer and names no provider: it resolves an opaque
//! [`ProviderKind`] through the register in `sift-registry`, which sits *below* every layer
//! the rule covers. "Add a Gmail account" is a thing a person clicks, and the register is
//! where that sentence is allowed to exist.

use sift_credentials::oauth::{AuthError, Broker, Registration};
use sift_credentials::store::CredentialStore;
use sift_foundation::identity::AccountId;
use sift_provider::erased::ErasedAdapter;
use sift_registry::ProviderKind;

/// The adapter an account is reached through.
///
/// `dyn` with its error erased, which is the capability model working: this shell asks which
/// provider an account is exactly **once**, when the user adds it. After that the type is
/// gone and every command plans against what the account declares.
pub type Live = Box<dyn ErasedAdapter>;

/// The kind the harness adds. A register of one, until the other three adapters are live.
#[must_use]
pub fn default_kind() -> ProviderKind {
    sift_registry::KINDS[0].kind
}

/// Where the callback comes back to — D-36's registered URI scheme, as the client requires it.
///
/// **Not a loopback address.** NFR-24 admits no listening socket for any purpose, and D-36
/// removes the loopback redirect rather than excusing it — which is what forces the scheme to
/// be derived from the client rather than fixed, because the provider client type that permits
/// a scheme redirect at all accepts exactly one: the client identifier, reversed.
#[must_use]
pub fn redirect_uri(client_id: &str) -> String {
    sift_foundation::identifiers::redirect_uri_for(client_id)
}

/// The authorization this shell can offer for a kind.
///
/// # Errors
/// The kind is not one this build has, or it does not authorize at all.
pub fn registration(kind: ProviderKind, client_id: &str) -> Result<Registration, String> {
    let descriptor =
        sift_registry::by_id(kind.as_str()).ok_or_else(|| format!("no such provider: {kind}"))?;
    let profile = descriptor
        .profile()
        .ok_or_else(|| format!("{kind} does not authorize"))?;
    Ok(Registration {
        profile,
        client_id: client_id.to_owned(),
        redirect_uri: redirect_uri(client_id),
    })
}

/// Begin an authorization and hand back the address to open.
///
/// # Errors
/// See [`AuthError`]. In particular it refuses to begin where the callback scheme is not
/// registered with the system — checked before the user goes anywhere, because discovering
/// it afterwards means they have already been sent to a browser and returned to nothing.
pub fn begin<S: CredentialStore>(
    broker: &mut Broker<S>,
    client_id: &str,
    scheme_is_registered: bool,
    now_millis: u64,
) -> Result<String, AuthError> {
    let registration = registration(default_kind(), client_id)
        .map_err(|why| AuthError::Store(sift_credentials::store::StoreError::Unavailable(why)))?;
    broker.begin(&registration, scheme_is_registered, now_millis)
}

/// Complete an authorization from the address the system handed back, and build the adapter.
///
/// # Errors
/// See [`AuthError`].
pub fn complete<S: CredentialStore>(
    broker: &mut Broker<S>,
    client_id: &str,
    account: AccountId,
    callback: &str,
    now_millis: u64,
) -> Result<Live, AuthError> {
    let unavailable =
        |why: String| AuthError::Store(sift_credentials::store::StoreError::Unavailable(why));
    let kind = default_kind();
    let descriptor = sift_registry::by_id(kind.as_str())
        .ok_or_else(|| unavailable(format!("no such provider: {kind}")))?;
    let registration = registration(kind, client_id).map_err(unavailable)?;
    let mut token_transport =
        sift_http::Https::to(&registration.profile.token.host).map_err(unavailable)?;
    let pair = broker.complete(
        &mut token_transport,
        &registration,
        account,
        callback,
        now_millis,
    )?;
    descriptor.connect(&pair.access).map_err(unavailable)
}

/// An account backed by the fixture corpus rather than by a socket — D-65.
#[must_use]
pub fn replayed() -> Live {
    sift_registry::KINDS[0].replayed()
}

/// Whether the callback returns through a socket.
///
/// **No.** NFR-24 admits no listening socket for any purpose, and this shell is where a
/// loopback redirect would be easiest to reach for.
#[must_use]
pub const fn callback_arrives_on_a_socket() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_callback_returns_through_a_registered_scheme_and_not_a_socket() {
        assert!(!callback_arrives_on_a_socket());
        let uri = redirect_uri("not-a-google-client");
        assert!(
            uri.starts_with(sift_foundation::identifiers::CALLBACK_SCHEME),
            "{uri}"
        );
        assert!(
            !uri.contains("localhost") && !uri.contains("127.0.0.1"),
            "{uri}"
        );
    }

    #[test]
    fn the_registration_asks_for_nothing_that_can_send() {
        let r = registration(default_kind(), "c").expect("the default kind authorizes");
        assert!(!r.profile.authorizes_sending());
    }
}

//! Adding an account, and everything a live one needs to work.
//!
//! # A shell asks which provider exactly once, and now it does not even do that
//!
//! Everywhere above the adapter layer plans against declared capabilities, and
//! `cargo xtask invariants` enforces it — but the rule stops below the shells, because "add a
//! Gmail account" is a thing a person clicks.
//!
//! This shell used to be where that name lived. It no longer is: the register in
//! `sift-registry` sits *below* every layer the rule covers, so the name lives there and this
//! module resolves an opaque [`ProviderKind`] through it. What is left here is the part that
//! is genuinely the shell's — driving the authorization flow — and it is written against the
//! descriptor rather than against a provider.

use sift_credentials::oauth::{AuthError, Broker, Registration};
use sift_credentials::store::CredentialStore;
use sift_foundation::identifiers::CALLBACK_SCHEME;
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

/// Where the callback comes back to — D-36's registered URI scheme.
///
/// **Not a loopback address.** NFR-24 admits no listening socket for any purpose, and D-36
/// removes the loopback redirect rather than excusing it.
#[must_use]
pub fn redirect_uri() -> String {
    format!("{CALLBACK_SCHEME}:/oauth2/callback")
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
        redirect_uri: redirect_uri(),
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
        let uri = redirect_uri();
        assert!(uri.starts_with(CALLBACK_SCHEME), "{uri}");
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

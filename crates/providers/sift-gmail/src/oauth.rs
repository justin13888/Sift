//! What this account declares about how it is authorized — and the scope finding that
//! changed two rows of its capability table.
//!
//! # The finding: D-7's hybrid cannot be built without authorizing sending
//!
//! D-7 chose "IMAP IDLE purely as a wake signal, the delta over the API", and recorded its
//! own contestability: a coalesced poll "may be indistinguishable to the user, at half the
//! code and one connection". Building the wire protocol turned that from a preference into a
//! constraint, because of a fact about the provider's authorization model rather than about
//! its protocols:
//!
//! **IMAP access over OAuth requires the provider's full-mailbox scope, and that same scope
//! authorizes SMTP submission.** There is no read-only or modify-only scope that admits
//! IMAP. So the hybrid is reachable only by asking the user, on the consent screen, to grant
//! Sift the ability to send mail as them.
//!
//! D-88's second point forbids exactly that, in terms that leave no room: "a permanent
//! minimum scope set with **no send or compose scope ever requested** — the no-send
//! constraint expressed where a reviewer can check it against an authorization screen". A
//! granted submission capability is an outbound message path whether or not any code calls
//! it; `docs/product/scope.md` makes the constraint structural rather than a preference
//! about which functions exist.
//!
//! So the doorbell goes, and the polling path D-7 already required to "exist and be good"
//! is the only path. Two consequences are recorded in the capability table rather than
//! discovered later:
//!
//! - `push` is `PollOnly`, not `IdleOnePerConnection`.
//! - `permanent_delete` is **false**. Immediate permanent deletion is in the same
//!   full-mailbox scope for the same reason, so FR-13's ninth intent is unavailable on this
//!   provider. That is the capability model working: the affordance is *absent* rather than
//!   approximated, which is the rule `docs/mail/mutations.md` already applies to junk
//!   reporting on an account that does not support it.
//!
//! # What is not resolved here
//!
//! Issue #16 — the restricted-scope verification — is untouched by this. The scope Sift asks
//! for is still a restricted one, so the assessment is still the gate, and it is still a
//! budget question rather than a technical one.

use sift_provider::oauth::{Endpoint, OAuthProfile};

/// The API host. Every request in `wire` is against this one origin.
pub const API_HOST: &str = "gmail.googleapis.com";

/// The scope Sift asks for, and the whole of it.
///
/// Read, search, label, trash, untrash, and report junk in both directions. It cannot send,
/// cannot compose, and cannot permanently delete.
pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

/// The scope Sift will not ask for, named so the refusal is checkable rather than implied.
///
/// It grants IMAP and SMTP, which is why D-7's doorbell is unreachable. Declared to
/// [`OAuthProfile::sending_scopes`] so that a future edit adding it fails a test rather than
/// a review.
pub const FULL_MAILBOX_SCOPE: &str = "https://mail.google.com/";

/// The authorization profile the credential broker plans against.
#[must_use]
pub fn profile() -> OAuthProfile {
    OAuthProfile {
        authorize: Endpoint::new("accounts.google.com", "/o/oauth2/v2/auth"),
        token: Endpoint::new("oauth2.googleapis.com", "/token"),
        revoke: Some(Endpoint::new("oauth2.googleapis.com", "/revoke")),
        scopes: vec![SCOPE.to_owned()],
        authorize_parameters: vec![
            // Without this the provider returns no refresh token, and the account would
            // stop working an hour after it was added.
            ("access_type".to_owned(), "offline".to_owned()),
            // And without this, it returns one only on the *first* consent — so a user
            // re-adding an account after FR-4's erasure would get an account that dies in
            // an hour, with nothing saying why.
            ("prompt".to_owned(), "consent".to_owned()),
        ],
        sending_scopes: vec![FULL_MAILBOX_SCOPE.to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_set_is_one_scope_and_it_cannot_send() {
        let p = profile();
        assert_eq!(p.scopes, vec![SCOPE.to_owned()]);
        assert!(!p.authorizes_sending());
    }

    #[test]
    fn the_full_mailbox_scope_is_recognised_as_a_sending_scope() {
        // The whole point of the finding: this scope's name says nothing about submission,
        // and it grants it. If a later edit adds it — for the doorbell, or for permanent
        // delete — this is the test that fails.
        let mut p = profile();
        p.scopes.push(FULL_MAILBOX_SCOPE.to_owned());
        assert!(p.authorizes_sending());
    }

    #[test]
    fn a_refresh_token_is_asked_for_every_time_consent_is_given() {
        // Both parameters, and each has a failure that is silent without it: no refresh
        // token at all, and no refresh token on a re-add.
        let p = profile();
        assert!(p.authorize_parameters.contains(&("access_type".into(), "offline".into())));
        assert!(p.authorize_parameters.contains(&("prompt".into(), "consent".into())));
    }

    #[test]
    fn revocation_is_offered_but_nothing_depends_on_it() {
        // FR-4's erasure is local and provable; revocation is best-effort on top.
        assert!(profile().revoke.is_some());
    }
}

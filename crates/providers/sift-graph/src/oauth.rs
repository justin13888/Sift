//! What a Graph account declares about how it is authorized.
//!
//! # One tenant, for both kinds of account
//!
//! The identity platform serves personal accounts and work or school accounts from different
//! directories, and each has its own authority. The **`common`** authority admits both, and
//! it is the only one this profile uses: the capability doc comment in the crate root already
//! says consumer and organizational accounts share one code path, and D-12 forbids the branch
//! that choosing an authority per account type would be. A person signs in with whichever
//! account they have, and the platform decides which directory holds it.
//!
//! # D-88's permanent scope set
//!
//! Two scopes, and the whole of it:
//!
//! - **`Mail.ReadWrite`** on the Graph resource — read, search, and every FR-13 intent this
//!   adapter declares: move, archive, trash, flag, read state, categories, and permanent
//!   delete. It carries no submission right; the provider puts sending behind a scope of its
//!   own, which is named in [`SENDING_SCOPES`] so that asking for it fails a test.
//! - **`offline_access`** — FR-2's silent refresh. Without it the platform issues no refresh
//!   token and the account stops working an hour after it is added.
//!
//! Fully qualified rather than bare, so the token is minted for the one resource the adapter
//! reaches and the consent screen names it. The set is written into
//! `docs/mail/providers/microsoft-graph.md` because widening it later forces the whole install
//! base through re-consent.
//!
//! # Revocation
//!
//! The platform offers a public client no endpoint to revoke its own grant, so the profile
//! declares none. FR-4's erasure is local and provable without it; revocation was only ever
//! best-effort on top.

use sift_provider::oauth::{Endpoint, OAuthProfile};

pub use crate::wire::API_HOST;

/// The identity platform's host.
pub const AUTHORITY_HOST: &str = "login.microsoftonline.com";

/// The authority that admits personal and work or school accounts alike.
pub const TENANT: &str = "common";

/// The scope Sift asks for to reach mail.
pub const MAIL_SCOPE: &str = "https://graph.microsoft.com/Mail.ReadWrite";

/// The scope that makes the platform issue a refresh token.
pub const OFFLINE_SCOPE: &str = "offline_access";

/// Scopes Sift will not ask for, named so the refusal is checkable rather than implied.
///
/// The ones spelled with "send" are caught by [`OAuthProfile::authorizes_sending`] anyway and
/// are listed for the reader. The rest are the ones whose names say nothing about submission
/// and grant it: a full-mailbox legacy protocol scope that can send, and a resource's
/// `.default`, which grants whatever the registration statically lists — including a send
/// permission somebody later adds to it in a portal nobody reviews.
pub const SENDING_SCOPES: &[&str] = &[
    "https://graph.microsoft.com/Mail.Send",
    "https://graph.microsoft.com/Mail.Send.Shared",
    "https://graph.microsoft.com/.default",
    "https://outlook.office.com/SMTP.Send",
    "https://outlook.office.com/EWS.AccessAsUser.All",
    "https://outlook.office.com/.default",
];

/// The authorization profile the credential broker plans against.
#[must_use]
pub fn profile() -> OAuthProfile {
    OAuthProfile {
        authorize: Endpoint::new(AUTHORITY_HOST, &format!("/{TENANT}/oauth2/v2.0/authorize")),
        token: Endpoint::new(AUTHORITY_HOST, &format!("/{TENANT}/oauth2/v2.0/token")),
        revoke: None,
        scopes: vec![MAIL_SCOPE.to_owned(), OFFLINE_SCOPE.to_owned()],
        authorize_parameters: vec![
            // The browser session is shared with the system browser, so a person already
            // signed in to one account would otherwise be handed straight back as that
            // account — and adding a second mailbox would silently re-add the first.
            ("prompt".to_owned(), "select_account".to_owned()),
        ],
        sending_scopes: SENDING_SCOPES.iter().map(|s| (*s).to_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_set_is_mail_and_a_refresh_token_and_it_cannot_send() {
        let p = profile();
        assert_eq!(
            p.scopes,
            vec![MAIL_SCOPE.to_owned(), OFFLINE_SCOPE.to_owned()]
        );
        assert!(!p.authorizes_sending());
    }

    #[test]
    fn every_declared_sending_scope_is_recognised_as_one() {
        // If a later edit adds any of these — for a send button, for the legacy protocol, or
        // for the convenience of `.default` — this is the test that fails.
        for scope in SENDING_SCOPES {
            let mut p = profile();
            p.scopes.push((*scope).to_owned());
            assert!(p.authorizes_sending(), "{scope}");
        }
    }

    #[test]
    fn one_authority_serves_both_kinds_of_account() {
        // D-12: no branch on tenant type. `common` is the authority that admits both.
        let p = profile();
        assert_eq!(p.authorize.host, AUTHORITY_HOST);
        assert_eq!(p.token.host, AUTHORITY_HOST);
        assert!(
            p.authorize.path.starts_with("/common/"),
            "{}",
            p.authorize.path
        );
        assert!(p.token.path.starts_with("/common/"), "{}", p.token.path);
    }

    #[test]
    fn the_account_chooser_is_asked_for() {
        assert!(
            profile()
                .authorize_parameters
                .contains(&("prompt".into(), "select_account".into()))
        );
    }

    #[test]
    fn nothing_depends_on_revocation() {
        // No public-client revoke endpoint exists; FR-4's erasure is local.
        assert!(profile().revoke.is_none());
    }

    #[test]
    fn the_scopes_are_for_the_host_the_adapter_reaches() {
        assert!(MAIL_SCOPE.contains(API_HOST));
    }
}

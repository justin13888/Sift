//! The identifiers this project consumes permanently.
//!
//! `docs/product/platform-baseline.md` owns the register; this is the same values where
//! code can reach them. They are here rather than beside each consumer because several
//! independent things must agree on each one and none of them can be changed afterwards:
//! the bundle identifier binds the App Store record, the Keychain names are published in
//! the Cask uninstall stanza, and the Flatpak application id is simultaneously the D-Bus
//! well-known name, the desktop-file name, the portal identity and the data root.
//!
//! **Each is consumed once, for the life of the project.** A test cross-checks every value
//! here against the document that owns it, so the two cannot drift.

/// The macOS bundle identifier. One value for **both** channels, per D-45.
///
/// Two identifiers would mean two containers, two per-installation secrets, and under
/// D-43 a channel switch that discards the blob cache and re-authenticates every account.
///
/// Reverse-DNS under `justinchung.net`, a domain the copyright holder controls. The
/// abandoned Tauri scaffold left `com.justin13888.sift` behind and it was rejected rather
/// than inherited: a code-hosting account name is not a domain, and reverse-DNS under a
/// namespace somebody else may register later is the collision the convention prevents.
pub const BUNDLE_IDENTIFIER: &str = "net.justinchung.sift";

/// The URI scheme the OAuth authorization callback returns through — D-36.
///
/// Registered in the bundle's URL types. **This is not a socket**: NFR-24 admits no
/// listening socket for any purpose, and D-36 removes the loopback redirect rather than
/// excusing it.
///
/// It is also one of only two local attack surfaces Sift has, since any process running as
/// the user can invoke it. That is why D-88's state parameter is doing real work rather
/// than being ceremony, and why a callback whose state matches no flow in progress is
/// discarded without comment.
pub const CALLBACK_SCHEME: &str = "net.justinchung.sift";

/// The internal scheme body-view resources are addressed under — D-28.
///
/// **MUST NOT appear in the bundle's registered URL types, on either platform.** It is
/// registered with the web engine and nowhere else. Registering it with the operating
/// system would let any local process hand a capability-token address to Sift, which is
/// the one thing D-28's unguessability is for.
///
/// Deliberately not derived from [`BUNDLE_IDENTIFIER`]: the two must be impossible to
/// confuse at a glance, because one is reachable from outside the process and the other
/// must not be.
pub const INTERNAL_SCHEME: &str = "sift-resource";

/// The Flatpak application id.
///
/// Also the D-Bus well-known name — which is how the single-instance rule is honoured on
/// Linux **without a socket**, since NFR-24 forbids one and the platform's own name
/// ownership does the job.
pub const FLATPAK_APPLICATION_ID: &str = "net.justinchung.Sift";

/// The Keychain service name credential items are stored under — NFR-23.
///
/// Published in the Cask uninstall stanza, so changing it breaks uninstall for existing
/// users. Individual items are not permanent; **the naming scheme is**.
pub const KEYCHAIN_SERVICE: &str = "net.justinchung.sift";

/// The Keychain access group, once the team identifier is known.
///
/// The team identifier prefixes it, which is why D-45 requires **both** macOS channels be
/// sandboxed under one team: Keychain item access binds to the creating code's designated
/// requirement, and the ACLs only match if both builds present the same one.
#[must_use]
pub fn keychain_access_group(team_identifier: &str) -> String {
    format!("{team_identifier}.{BUNDLE_IDENTIFIER}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = include_str!("../../../../docs/product/platform-baseline.md");

    #[test]
    fn every_identifier_is_recorded_in_the_document_that_owns_it() {
        // "Each MUST be recorded here before first submission to any channel." A value in
        // code and not in the register is one nobody reviewed.
        for (name, value) in [
            ("bundle identifier", BUNDLE_IDENTIFIER),
            ("callback scheme", CALLBACK_SCHEME),
            ("internal scheme", INTERNAL_SCHEME),
            ("Flatpak application id", FLATPAK_APPLICATION_ID),
            ("Keychain service", KEYCHAIN_SERVICE),
        ] {
            assert!(
                DOC.contains(value),
                "the {name} `{value}` is not in docs/product/platform-baseline.md"
            );
        }
    }

    #[test]
    fn the_internal_scheme_is_not_the_registered_one() {
        // The rule the whole of "Two schemes, and only one of them is registered" exists
        // for. If these were ever made equal, registering the callback scheme with the
        // operating system would register the internal one too, and any local process
        // could hand Sift a capability-token address.
        assert_ne!(INTERNAL_SCHEME, CALLBACK_SCHEME);
        assert!(
            !INTERNAL_SCHEME.starts_with("net.justinchung"),
            "the internal scheme must not be confusable with the registered one"
        );
    }

    #[test]
    fn the_callback_scheme_matches_the_bundle_identifier() {
        // Providers requiring a custom-scheme redirect expect it to be the bundle
        // identifier, so this is derived rather than chosen — and if it drifted, the OAuth
        // client registration would stop matching the app.
        assert_eq!(CALLBACK_SCHEME, BUNDLE_IDENTIFIER);
    }

    #[test]
    fn nothing_inherited_the_scaffolds_identifier() {
        // The scaffold left `com.justin13888.sift`, under a domain the copyright holder
        // does not control. This is the check that it did not creep back in.
        for v in [
            BUNDLE_IDENTIFIER,
            CALLBACK_SCHEME,
            FLATPAK_APPLICATION_ID,
            KEYCHAIN_SERVICE,
        ] {
            assert!(
                !v.contains("justin13888"),
                "{v} is under a domain nobody controls"
            );
        }
    }

    #[test]
    fn identifiers_are_reverse_dns_under_the_controlled_domain() {
        for v in [BUNDLE_IDENTIFIER, FLATPAK_APPLICATION_ID, KEYCHAIN_SERVICE] {
            assert!(
                v.starts_with("net.justinchung."),
                "{v} is not under justinchung.net"
            );
        }
    }

    #[test]
    fn the_access_group_is_the_team_identifier_prefixed() {
        assert_eq!(
            keychain_access_group("ABCDE12345"),
            "ABCDE12345.net.justinchung.sift"
        );
    }
}

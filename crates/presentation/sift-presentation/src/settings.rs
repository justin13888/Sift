//! D-101 — the settings surface, enumerated with the defaults it ships.
//!
//! # Organized by scope, which is a storage-shaped organization shown to users
//!
//! D-101 organizes by **installation versus account** rather than by topic, because that is
//! the split storage already uses and two organizations would drift. The decision records its
//! own weakness: **scope is a storage concept shown to users, who do not know what an
//! installation is.**
//!
//! # Every default here is a decision that ships
//!
//! A shell that invents a setting, or ships a different default, is the failure this
//! enumeration exists to prevent — and it is a failure that would be invisible until the two
//! shells were compared side by side.

use core::time::Duration;
use sift_foundation::limits::{L13_FETCH_BYTES, L20_ENVELOPE_INDEX_BUDGET, L21_READ_DWELL};

/// Which store a setting lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Erased by uninstallation only.
    Installation,
    /// Erased by FR-4's account removal, as part of the database.
    Account,
}

/// One setting.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub scope: Scope,
    pub default: Default_,
    /// Whether this is **security state rather than a preference**.
    ///
    /// The per-sender allowlist is the case: write access to it is write access to Sift's
    /// egress policy. D-101 requires the two per-sender lists be *shown but not edited like
    /// preferences* — reviewable and revocable, not bulk-editable — because a bulk edit of an
    /// egress policy is a single gesture that turns every sender's images on.
    pub security_state: bool,
}

/// A default value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Default_ {
    Off,
    On,
    Bytes(u64),
    Days(u32),
    Time(Duration),
    /// No value; the list starts empty.
    Empty,
    /// Detection decides.
    Detected,
}

/// Every setting Sift ships, with its default.
pub const SETTINGS: &[Setting] = &[
    // --- Installation ---
    Setting {
        key: "cache.budget-days",
        scope: Scope::Installation,
        default: Default_::Days(90),
        security_state: false,
    },
    Setting {
        key: "cache.budget-bytes",
        scope: Scope::Installation,
        default: Default_::Bytes(2 * 1024 * 1024 * 1024),
        security_state: false,
    },
    Setting {
        key: "envelope-index.budget-bytes",
        scope: Scope::Installation,
        default: Default_::Bytes(L20_ENVELOPE_INDEX_BUDGET),
        security_state: false,
    },
    Setting {
        key: "fetch.confirm-above-bytes",
        scope: Scope::Installation,
        default: Default_::Bytes(L13_FETCH_BYTES),
        security_state: false,
    },
    // FR-27's lists, all enabled. A blocker shipped switched off is a blocker that protects
    // whoever reads the settings screen.
    Setting {
        key: "blocking.standard-lists",
        scope: Scope::Installation,
        default: Default_::On,
        security_state: false,
    },
    Setting {
        key: "blocking.bundled-email-list",
        scope: Scope::Installation,
        default: Default_::On,
        security_state: false,
    },
    // FR-31 — **off**. A mangled brand header is a visible defect; a light message in a dark
    // window is merely unpleasant.
    Setting {
        key: "dark-transform",
        scope: Scope::Installation,
        default: Default_::Off,
        security_state: false,
    },
    Setting {
        key: "read.dwell",
        scope: Scope::Installation,
        default: Default_::Time(L21_READ_DWELL),
        security_state: false,
    },
    Setting {
        key: "data.cap",
        scope: Scope::Installation,
        default: Default_::Empty,
        security_state: false,
    },
    Setting {
        key: "data.accounting-window-days",
        scope: Scope::Installation,
        default: Default_::Days(30),
        security_state: false,
    },
    Setting {
        key: "network.per-network-overrides",
        scope: Scope::Installation,
        default: Default_::Detected,
        security_state: false,
    },
    Setting {
        key: "notifications.quiet-mode",
        scope: Scope::Installation,
        default: Default_::Off,
        security_state: false,
    },
    Setting {
        key: "notifications.default-inbox-only",
        scope: Scope::Installation,
        default: Default_::On,
        security_state: false,
    },
    // FR-33 and FR-34 ship in release behind a preference, default off. A surface that only
    // existed in a debug build would be the untested one.
    Setting {
        key: "debug.message-view",
        scope: Scope::Installation,
        default: Default_::Off,
        security_state: false,
    },
    Setting {
        key: "debug.runtime-panel",
        scope: Scope::Installation,
        default: Default_::Off,
        security_state: false,
    },
    // --- Account ---
    Setting {
        key: "account.watched-folders",
        scope: Scope::Account,
        default: Default_::Detected,
        security_state: false,
    },
    Setting {
        key: "account.notification-rules",
        scope: Scope::Account,
        default: Default_::Detected,
        security_state: false,
    },
    // D-95: **a newly added account is not paused**, even while others are.
    Setting {
        key: "account.paused",
        scope: Scope::Account,
        default: Default_::Off,
        security_state: false,
    },
    Setting {
        key: "account.sender-allowlist",
        scope: Scope::Account,
        default: Default_::Empty,
        security_state: true,
    },
    Setting {
        key: "account.sender-dark-mode",
        scope: Scope::Account,
        default: Default_::Empty,
        security_state: false,
    },
];

#[must_use]
pub fn get(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Whether a shell may add a setting of its own.
///
/// **No.** Two shells with different settings is the drift D-101 exists to prevent, and it
/// would be invisible until somebody compared them.
#[must_use]
pub const fn shells_may_invent_settings() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn keys_are_unique() {
        let keys: BTreeSet<&str> = SETTINGS.iter().map(|s| s.key).collect();
        assert_eq!(keys.len(), SETTINGS.len());
    }

    #[test]
    fn the_dark_transform_ships_off() {
        // FR-31's default, and the reasoning: a mangled brand header is a visible defect.
        assert_eq!(get("dark-transform").unwrap().default, Default_::Off);
    }

    #[test]
    fn blocking_ships_on() {
        // A blocker shipped switched off protects whoever reads the settings screen.
        assert_eq!(
            get("blocking.standard-lists").unwrap().default,
            Default_::On
        );
        assert_eq!(
            get("blocking.bundled-email-list").unwrap().default,
            Default_::On
        );
    }

    #[test]
    fn the_debug_surfaces_ship_in_release_but_switched_off() {
        // A surface that only existed in a debug build would be the untested one.
        for key in ["debug.message-view", "debug.runtime-panel"] {
            assert_eq!(get(key).unwrap().default, Default_::Off, "{key}");
        }
    }

    #[test]
    fn a_new_account_is_not_paused_even_while_others_are() {
        assert_eq!(get("account.paused").unwrap().default, Default_::Off);
        assert_eq!(get("account.paused").unwrap().scope, Scope::Account);
    }

    #[test]
    fn the_sender_allowlist_is_security_state_rather_than_a_preference() {
        // Write access to it is write access to Sift's egress policy, which is why D-101
        // requires it be reviewable and revocable rather than bulk-editable.
        let s = get("account.sender-allowlist").unwrap();
        assert!(s.security_state);
        assert_eq!(s.default, Default_::Empty);
    }

    #[test]
    fn exactly_one_setting_is_security_state() {
        // If a second appears it should be a deliberate addition rather than a drift.
        let count = SETTINGS.iter().filter(|s| s.security_state).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn account_scoped_settings_are_the_ones_fr4_erases() {
        // Everything account-scoped goes with the database; everything installation-scoped
        // survives until uninstallation.
        let account: Vec<&str> = SETTINGS
            .iter()
            .filter(|s| s.scope == Scope::Account)
            .map(|s| s.key)
            .collect();
        assert!(
            account.iter().all(|k| k.starts_with("account.")),
            "{account:?}"
        );
    }

    #[test]
    fn a_shell_may_not_invent_a_setting() {
        assert!(!shells_may_invent_settings());
    }
}

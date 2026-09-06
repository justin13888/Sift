//! D-101's settings, enumerated once with their defaults.
//!
//! # Why the defaults are here and not in a shell
//!
//! **Every default is a decision that ships.** Under D-56 the settings surface is written
//! twice — once in Swift and once in GTK — and a default chosen independently by two shells is
//! two products. The ones that matter most are the ones that look least like decisions: the
//! dark transform is off, the debug surfaces are off, there is no data cap, and the per-sender
//! allowlist is empty.
//!
//! # Organised by scope, not by topic
//!
//! Scope is the split storage already makes, and the split that decides behaviour: an account
//! setting disappears with FR-4's removal and an installation setting does not. Organising by
//! topic would put the cache budget beside the per-sender allowlist, which look related and
//! have opposite lifetimes.
//!
//! # Two of these are not preferences
//!
//! The per-sender remote-content allowlist is **security state**, and the settings surface's
//! job is to make it reviewable and revocable rather than to invite bulk editing of it in a
//! screen away from any message. It is enumerated here so that a shell knows to show it, and
//! it is deliberately not writable through this module.

/// What a setting holds. Deliberately small: a settings model that can express anything is one
/// that ends up shaped differently in each shell, which is the failure D-101 is avoiding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Flag(bool),
    /// A count, a byte budget, or a duration in milliseconds. The unit is the setting's.
    Number(i64),
    Text(String),
}

impl Value {
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Flag(v) => (*v).to_string(),
            Self::Number(v) => v.to_string(),
            Self::Text(v) => v.clone(),
        }
    }

    #[must_use]
    pub fn parse_like(&self, text: &str) -> Option<Self> {
        Some(match self {
            Self::Flag(_) => Self::Flag(matches!(text, "true" | "1" | "on" | "yes")),
            Self::Number(_) => Self::Number(text.parse().ok()?),
            Self::Text(_) => Self::Text(text.to_owned()),
        })
    }
}

/// Which lifetime a setting has, which is the only organising principle D-101 accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Survives the removal of every account.
    Installation,
    /// Goes with the account, under FR-4.
    Account,
}

/// One setting.
#[derive(Debug, Clone)]
pub struct Setting {
    /// Stable, and the key it is stored under. Never renumbered.
    pub key: &'static str,
    pub scope: Scope,
    pub default: Value,
    /// Which requirement or decision owns it — so a settings screen can say *why* a thing is
    /// there, and so a reviewer can find the argument rather than the value.
    pub owner: &'static str,
    /// True where the value is a record of decisions made in context rather than a preference.
    /// A shell shows these and does not offer bulk editing of them.
    pub is_security_state: bool,
}

const fn installation(key: &'static str, default: Value, owner: &'static str) -> Setting {
    Setting {
        key,
        scope: Scope::Installation,
        default,
        owner,
        is_security_state: false,
    }
}

const fn account(key: &'static str, default: Value, owner: &'static str) -> Setting {
    Setting {
        key,
        scope: Scope::Account,
        default,
        owner,
        is_security_state: false,
    }
}

/// D-101's two tables, in one list.
///
/// The order is the order the tables are written in, because a settings screen that reordered
/// them would be a third opinion about what belongs beside what.
pub static SETTINGS: &[Setting] = &[
    // Installation.
    installation(
        "cache.budget-bytes",
        Value::Number(2 * 1024 * 1024 * 1024),
        "NFR-14",
    ),
    installation("cache.budget-days", Value::Number(90), "NFR-14"),
    installation("index.envelope-budget", Value::Number(50_000), "NFR-52"),
    installation(
        "network.single-fetch-ceiling-bytes",
        Value::Number(25 * 1024 * 1024),
        "NFR-39",
    ),
    installation("blocking.standard-lists", Value::Flag(true), "FR-27"),
    installation("blocking.email-list", Value::Flag(true), "FR-27"),
    // **Off.** FR-31 makes the dark transform opt-in by name, and a transform applied over a
    // message a sender already styled is how a readable message becomes unreadable.
    installation("render.dark-transform", Value::Flag(false), "FR-31"),
    installation("read.mark-read-dwell-millis", Value::Number(2_000), "D-52"),
    // No cap. A cap nobody set that stops mail arriving is worse than no cap.
    installation("network.data-cap-bytes", Value::Number(0), "FR-36"),
    installation("network.accounting-window-days", Value::Number(30), "FR-36"),
    installation("notify.quiet-mode", Value::Flag(false), "FR-23"),
    installation("notify.new-mail-in-inbox", Value::Flag(true), "FR-23"),
    // **Off**, and preference-gated by their own decision. A debug surface that is on by
    // default is a debug surface whose cost nobody measured.
    installation("debug.message-view", Value::Flag(false), "FR-33"),
    installation("debug.runtime-panel", Value::Flag(false), "FR-34"),
    // Account.
    account("sync.watched-folders", Value::Text(String::new()), "FR-43"),
    account(
        "notify.per-folder-rules",
        Value::Text(String::new()),
        "FR-23",
    ),
    account("sync.paused", Value::Flag(false), "D-95"),
    Setting {
        key: "render.sender-allowlist",
        scope: Scope::Account,
        default: Value::Text(String::new()),
        owner: "FR-8",
        // Security state, not a preference: a record of decisions the user made in context,
        // to be reviewable and revocable rather than bulk-edited in a screen away from any
        // message.
        is_security_state: true,
    },
    Setting {
        key: "render.sender-dark-choices",
        scope: Scope::Account,
        default: Value::Text(String::new()),
        owner: "FR-31",
        is_security_state: true,
    },
];

/// Look one up by key.
#[must_use]
pub fn by_key(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults that matter most are the ones that look least like decisions. Each of
    /// these being wrong is a product that quietly does something the specification says it
    /// must not do by default.
    #[test]
    fn the_defaults_that_ship_are_the_ones_stated() {
        let flag = |key: &str| match by_key(key).map(|s| s.default.clone()) {
            Some(Value::Flag(v)) => v,
            other => panic!("`{key}` is {other:?}"),
        };
        assert!(
            !flag("render.dark-transform"),
            "FR-31 makes it opt-in by name"
        );
        assert!(!flag("debug.message-view"), "FR-33 is off");
        assert!(!flag("debug.runtime-panel"), "FR-34 is off");
        assert!(
            !flag("sync.paused"),
            "an account is not added paused — D-95"
        );

        assert_eq!(
            by_key("network.data-cap-bytes").map(|s| s.default.clone()),
            Some(Value::Number(0)),
            "a cap nobody set that stops mail arriving is worse than no cap"
        );
    }

    /// The two per-sender lists are records of decisions rather than preferences, and a shell
    /// has to be able to tell which is which without knowing the keys.
    #[test]
    fn security_state_is_marked_as_such_rather_than_left_to_a_shell_to_notice() {
        let marked: Vec<&str> = SETTINGS
            .iter()
            .filter(|s| s.is_security_state)
            .map(|s| s.key)
            .collect();
        assert_eq!(
            marked,
            vec!["render.sender-allowlist", "render.sender-dark-choices"]
        );
    }

    #[test]
    fn every_key_is_unique_and_scoped() {
        let mut keys: Vec<&str> = SETTINGS.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two settings share a key");
        assert!(SETTINGS.iter().any(|s| s.scope == Scope::Installation));
        assert!(SETTINGS.iter().any(|s| s.scope == Scope::Account));
    }

    /// A value read back from storage is text, so every kind has to survive the round trip.
    #[test]
    fn a_value_round_trips_through_the_text_it_is_stored_as() {
        for value in [
            Value::Flag(true),
            Value::Flag(false),
            Value::Number(-1),
            Value::Number(2 * 1024 * 1024 * 1024),
            Value::Text("inbox,sent".to_owned()),
        ] {
            let text = value.as_text();
            assert_eq!(value.parse_like(&text), Some(value.clone()), "{text}");
        }
    }
}

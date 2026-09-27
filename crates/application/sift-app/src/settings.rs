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

/// The key FR-23's per-folder rules are stored under.
pub const FOLDER_RULES: &str = "notify.per-folder-rules";

/// FR-23's per-folder notification rule for one account — what `notify.per-folder-rules`
/// holds.
///
/// # The grammar
///
/// Comma-separated, case-insensitive, surrounding space ignored:
///
/// - `none` — nothing this account receives is announced;
/// - `all` — anything that arrives in any watched folder is;
/// - otherwise a list of folders, each either a special use (`inbox`, `archive`, `sent`,
///   `trash`, `spam`, `drafts`) or `folder:N`, a folder's local identity under D-83.
///
/// Special uses rather than display names, for FR-5's reason: a display name is the provider's
/// and the locale's, and a rule keyed on one would stop matching when either changed. A
/// folder's local identity is Sift's own and survives a rename, which is what makes it the
/// key a rule for one particular folder is written against.
///
/// **Empty text is no rule at all**, and the account takes the installation default — which is
/// what an account that existed before rules were recorded holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderRule {
    None,
    All,
    /// A non-empty list.
    Only(Vec<FolderSelector>),
}

/// One folder a [`FolderRule`] names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderSelector {
    /// A special use, as the store records it — `Inbox`, `Archive`, and so on.
    SpecialUse(&'static str),
    /// A folder's local identity.
    Folder(i64),
}

/// The special uses a rule may name, as the store records them — derived from the adapter
/// layer's one list and the text D-83's folder reconciliation writes for each, so a special use
/// added there is one a rule can name without an edit here.
fn special_use_names() -> impl Iterator<Item = &'static str> {
    sift_provider::adapter::SpecialUse::ALL
        .into_iter()
        .map(sift_sync::ingest::special_use_name)
}

impl FolderRule {
    /// Read a stored rule. `Ok(None)` is empty text: no rule, so the installation default.
    ///
    /// # Errors
    /// A token that is none of the grammar's, or `none` or `all` beside anything else — a rule
    /// that says both "nothing" and "the inbox" says neither.
    pub fn parse(text: &str) -> Result<Option<Self>, String> {
        let tokens: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        match tokens.as_slice() {
            [] => return Ok(None),
            [only] if only.eq_ignore_ascii_case("none") => return Ok(Some(Self::None)),
            [only] if only.eq_ignore_ascii_case("all") => return Ok(Some(Self::All)),
            _ => {}
        }
        let mut selected = Vec::with_capacity(tokens.len());
        for token in tokens {
            let lower = token.to_ascii_lowercase();
            let selector = if let Some(id) = lower.strip_prefix("folder:") {
                FolderSelector::Folder(
                    id.trim()
                        .parse()
                        .map_err(|_| format!("`{token}` does not name a folder by its identity"))?,
                )
            } else if let Some(name) =
                special_use_names().find(|name| name.eq_ignore_ascii_case(token))
            {
                FolderSelector::SpecialUse(name)
            } else if lower == "none" || lower == "all" {
                return Err(format!("`{token}` cannot be combined with anything else"));
            } else {
                return Err(format!("`{token}` is not a folder a rule can name"));
            };
            if !selected.contains(&selector) {
                selected.push(selector);
            }
        }
        Ok(Some(Self::Only(selected)))
    }

    /// The rule a new account inherits, from the installation's `notify.new-mail-in-inbox` —
    /// D-101's *notify on new mail in the inbox only*, or, turned off, nothing.
    #[must_use]
    pub fn installation_default(new_mail_in_inbox: bool) -> Self {
        if new_mail_in_inbox {
            Self::Only(vec![FolderSelector::SpecialUse("Inbox")])
        } else {
            Self::None
        }
    }

    /// The text [`Self::parse`] reads back as this rule.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::All => "all".to_owned(),
            Self::Only(selected) => selected
                .iter()
                .map(|s| match s {
                    FolderSelector::SpecialUse(name) => name.to_ascii_lowercase(),
                    FolderSelector::Folder(id) => format!("folder:{id}"),
                })
                .collect::<Vec<_>>()
                .join(","),
        }
    }

    /// Whether mail arriving in this folder is announced.
    #[must_use]
    pub fn admits(&self, folder: i64, special_use: Option<&str>) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Only(selected) => selected.iter().any(|s| match s {
                FolderSelector::Folder(id) => *id == folder,
                FolderSelector::SpecialUse(name) => {
                    special_use.is_some_and(|u| u.eq_ignore_ascii_case(name))
                }
            }),
        }
    }
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

    /// FR-23: D-101's default is "quiet off; notify on new mail in the inbox only", and an
    /// account with no rule of its own takes that.
    #[test]
    fn the_notification_defaults_are_quiet_off_and_the_inbox_only() {
        assert_eq!(
            by_key("notify.quiet-mode").map(|s| s.default.clone()),
            Some(Value::Flag(false))
        );
        assert_eq!(
            by_key("notify.new-mail-in-inbox").map(|s| s.default.clone()),
            Some(Value::Flag(true))
        );
        assert_eq!(
            by_key(FOLDER_RULES).map(|s| (s.scope, s.default.clone())),
            Some((Scope::Account, Value::Text(String::new())))
        );
        assert_eq!(
            FolderRule::parse(""),
            Ok(None),
            "empty is no rule, not `none`"
        );

        let inbox_only = FolderRule::installation_default(true);
        assert!(inbox_only.admits(7, Some("Inbox")));
        assert!(!inbox_only.admits(8, Some("Archive")));
        assert!(!inbox_only.admits(9, None), "a folder with no special use");
        assert_eq!(FolderRule::installation_default(false), FolderRule::None);
    }

    #[test]
    fn a_rule_names_folders_by_special_use_or_by_identity_and_nothing_else() {
        let rule = FolderRule::parse(" Inbox , folder:12,SPAM,inbox ")
            .expect("valid")
            .expect("a rule");
        assert_eq!(
            rule,
            FolderRule::Only(vec![
                FolderSelector::SpecialUse("Inbox"),
                FolderSelector::Folder(12),
                FolderSelector::SpecialUse("Spam"),
            ])
        );
        assert!(rule.admits(1, Some("Inbox")));
        assert!(rule.admits(12, None));
        assert!(rule.admits(3, Some("Spam")));
        assert!(!rule.admits(4, Some("Sent")));

        assert_eq!(FolderRule::parse("none"), Ok(Some(FolderRule::None)));
        assert_eq!(FolderRule::parse("ALL"), Ok(Some(FolderRule::All)));
        assert!(FolderRule::All.admits(99, None));
        assert!(!FolderRule::None.admits(1, Some("Inbox")));

        for refused in [
            "Posteingang",
            "folder:x",
            "none,inbox",
            "inbox,all",
            "folder:",
        ] {
            assert!(
                FolderRule::parse(refused).is_err(),
                "`{refused}` was accepted"
            );
        }
    }

    /// Every special use folder reconciliation writes is one a rule can name, and the rule
    /// admits exactly the folder the store labelled with it.
    #[test]
    fn every_special_use_the_store_writes_is_one_a_rule_can_name() {
        for use_ in sift_provider::adapter::SpecialUse::ALL {
            let stored = sift_sync::ingest::special_use_name(use_);
            let rule = FolderRule::parse(&stored.to_ascii_lowercase())
                .unwrap_or_else(|e| panic!("`{stored}` is not nameable: {e}"))
                .expect("a rule");
            assert_eq!(
                rule,
                FolderRule::Only(vec![FolderSelector::SpecialUse(stored)])
            );
            assert!(rule.admits(1, Some(stored)), "`{stored}` is not admitted");
            assert_eq!(FolderRule::parse(&rule.as_text()), Ok(Some(rule)));
        }
    }

    #[test]
    fn a_rule_round_trips_through_the_text_it_is_stored_as() {
        for rule in [
            FolderRule::None,
            FolderRule::All,
            FolderRule::installation_default(true),
            FolderRule::Only(vec![
                FolderSelector::Folder(3),
                FolderSelector::SpecialUse("Archive"),
            ]),
        ] {
            assert_eq!(FolderRule::parse(&rule.as_text()), Ok(Some(rule.clone())));
        }
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

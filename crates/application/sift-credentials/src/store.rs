//! Where credentials live, which is the operating system's own store and nowhere else.
//!
//! # The rule, and why it reads as mundane
//!
//! `docs/security/credentials.md`: credentials exist **only** in the OS credential store —
//! never in a database, never in a log, never in a crash dump. The reasons are ordinary and
//! that is exactly the point: databases get backed up, synced, copied for debugging, and
//! attached to bug reports. A token in an account database is a token in whatever the user's
//! backup software does with that file.
//!
//! # NFR-23: one place, so "which code can read a token" has one answer
//!
//! Everything that touches credential material goes through this module. No shell reaches
//! it; `cargo xtask arch` enforces that the ABI cannot see this crate and the shells cannot
//! see past the ABI.
//!
//! # FR-4's erasure is assertable by enumeration
//!
//! Items are keyed on D-89's account identity and the closed [`Item`] set, so "has every
//! credential for this account been erased?" is a question with a finite answer rather than
//! a search. [`CredentialStore::erase`] answers it, and the test beside it checks the
//! enumeration rather than trusting the deletion.

// Gated, because its only consumers are. `mod macos` below is the sole user and is itself
// `cfg(target_os = "macos")`, so on every other platform this import has nothing to serve and
// `-D warnings` rejects the crate — which is what failed `linux` and the floor build while
// passing on the machine the change was written on.
#[cfg(target_os = "macos")]
use sift_foundation::identifiers::{KEYCHAIN_SERVICE, TEAM_IDENTIFIER, keychain_access_group};
use sift_foundation::identity::AccountId;

/// The credential items one account can have. **Closed**, so erasure is an enumeration.
///
/// Four rather than two, because D-88 retains the previous pair marked superseded until the
/// new one has completed one request — and a pair that exists only in memory would not
/// survive the crash it exists to protect against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Access,
    Refresh,
    SupersededAccess,
    SupersededRefresh,
    /// The key an account's databases are sealed under — or, under the reserved installation
    /// identity, the installation's own secret.
    ///
    /// **This is what makes FR-4's erasure a proof rather than a promise.** Removing an
    /// account's files is a deletion somebody has to trust; destroying this makes what is left
    /// unreadable, which is checkable. It is in `ALL`, so the enumeration erasure walks
    /// already covers it.
    DatabaseKey,
}

impl Item {
    /// Every item an account can hold. FR-4 enumerates this.
    pub const ALL: &'static [Self] = &[
        Self::Access,
        Self::Refresh,
        Self::SupersededAccess,
        Self::SupersededRefresh,
        Self::DatabaseKey,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Refresh => "refresh",
            Self::SupersededAccess => "superseded-access",
            Self::SupersededRefresh => "superseded-refresh",
            Self::DatabaseKey => "database-key",
        }
    }
}

/// The key one item is stored under.
///
/// **The naming scheme is permanent**, not the individual items: it is published in the
/// Cask zap stanza, so a change to it breaks zap cleanup for existing users.
#[must_use]
pub fn key_for(account: AccountId, item: Item) -> String {
    format!("{account}/{}", item.name())
}

/// What can go wrong reaching the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The item is not there. Not an error at every call site, which is why it is its own
    /// variant rather than an absence folded into a failure.
    NotFound,
    /// The store refused, was locked, or is not present on this platform.
    ///
    /// D-71 makes this a **process-scoped** condition: a security guarantee that is absent
    /// refuses rather than degrading, so an account whose credentials cannot be stored is
    /// not added with the material held in memory and hoped for.
    Unavailable(String),
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such credential item"),
            Self::Unavailable(why) => write!(f, "the credential store is unavailable: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The platform's credential store.
pub trait CredentialStore {
    /// # Errors
    /// [`StoreError::Unavailable`] where the store refused.
    fn write(&self, account: AccountId, item: Item, secret: &str) -> Result<(), StoreError>;

    /// # Errors
    /// [`StoreError::NotFound`] where the item is absent.
    fn read(&self, account: AccountId, item: Item) -> Result<String, StoreError>;

    /// # Errors
    /// [`StoreError::Unavailable`] where the store refused. Deleting what is not there
    /// succeeds, because the caller's intent — "this must not exist" — is satisfied.
    fn delete(&self, account: AccountId, item: Item) -> Result<(), StoreError>;

    /// FR-4: erase every credential for an account, by enumeration.
    ///
    /// **Does not block on revoking the grant at the provider.** Local erasure is what is
    /// provable; revocation is best-effort on top, and blocking on it would mean an account
    /// the user asked to remove staying until a server answered.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] where the store refused.
    fn erase(&self, account: AccountId) -> Result<(), StoreError> {
        for item in Item::ALL {
            self.delete(account, *item)?;
        }
        Ok(())
    }

    /// Which items this account still has. The check FR-4's claim rests on.
    fn remaining(&self, account: AccountId) -> Vec<Item> {
        Item::ALL
            .iter()
            .filter(|item| self.read(account, **item).is_ok())
            .copied()
            .collect()
    }
}

/// The platform's own store, where this build has one.
///
/// # Errors
/// [`StoreError::Unavailable`] on a platform whose backend is not built yet. **Refused
/// rather than substituted** — D-71 makes an absent security guarantee a refusal, and the
/// substitute here would be a file, which is the one thing `docs/security/credentials.md`
/// forbids by name.
pub fn platform() -> Result<Platform, StoreError> {
    Platform::open()
}

#[cfg(target_os = "macos")]
pub use macos::Platform;

#[cfg(not(target_os = "macos"))]
pub use unimplemented::Platform;

#[cfg(target_os = "macos")]
mod macos {
    use super::{
        CredentialStore, Item, KEYCHAIN_SERVICE, StoreError, TEAM_IDENTIFIER, key_for,
        keychain_access_group,
    };
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::base::Error;
    use security_framework::passwords::{self, PasswordOptions};
    use sift_foundation::identity::AccountId;

    /// `errSecItemNotFound`.
    const ITEM_NOT_FOUND: i32 = -25_300;
    /// `errSecMissingEntitlement`: this process carries no keychain access group, so the
    /// data-protection keychain will not serve it at all.
    const MISSING_ENTITLEMENT: i32 = -34_018;

    /// The platform keychain.
    ///
    /// # The bundle: the data-protection keychain, in the team's access group
    ///
    /// D-45 signs the bundle under the team and sandboxes it, with
    /// `854G577S2Y.net.justinchung.sift` as its one Keychain access group. Every item is
    /// written there, and read and deleted there, with the attributes
    /// `docs/product/platform-baseline.md` ("Credential storage attributes") requires:
    ///
    /// - **not synchronizable** — a synchronizing item would redefine "installation" as *per
    ///   account holder*, placing D-43's convergent secret on every machine the user owns;
    /// - **available after first unlock, and not device-only** — what a resident login item
    ///   needs, and what survives a migration to new hardware without D-43 discarding the
    ///   blob store wholesale.
    ///
    /// The group is named explicitly rather than left to the entitlement's default, so an
    /// item cannot land in some other group a later entitlement change puts first.
    ///
    /// # Everything else: the login keychain, as before
    ///
    /// A process with no access-group entitlement — the development tools, the corpus
    /// generator, a test binary — is refused by the data-protection keychain outright
    /// (`errSecMissingEntitlement`). It falls back to the file-based login keychain, which is
    /// still the operating system's credential store, so NFR-23 holds either way; what it
    /// loses is the team-scoped access group, which only a signed bundle can hold. The two
    /// stores never exchange items, so a signed bundle never reads what a tool wrote.
    ///
    /// **A bundle cannot take that fallback by accident**: `mise run macos` reads every
    /// signed bundle's entitlements back and fails a build that lost the access group.
    ///
    /// Items an ad-hoc build wrote to the login keychain are **not migrated**. Their access
    /// control names the ad-hoc code's designated requirement, which the signed bundle does
    /// not satisfy, so they could not be read without a prompt anyway; a development account
    /// is re-added instead.
    #[derive(Debug, Clone, Copy)]
    pub struct Platform;

    impl Platform {
        pub(super) const fn open() -> Result<Self, StoreError> {
            Ok(Self)
        }
    }

    /// The data-protection query for one item: the service, the item key, the team's access
    /// group, and the not-synchronizable store. Write, read and delete all start from it, so
    /// the three cannot address different items.
    fn protected(key: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, key);
        options.use_protected_keychain();
        options.set_access_group(&keychain_access_group(TEAM_IDENTIFIER));
        options.set_access_synchronized(Some(false));
        options
    }

    fn unavailable(e: &Error) -> StoreError {
        StoreError::Unavailable(e.to_string())
    }

    impl CredentialStore for Platform {
        fn write(&self, account: AccountId, item: Item, secret: &str) -> Result<(), StoreError> {
            let key = key_for(account, item);
            let mut options = protected(&key);
            // After first unlock, and *not* this-device-only. No flags: nothing about reading
            // a token should ask the user for presence.
            let access = SecAccessControl::create_with_protection(
                Some(ProtectionMode::AccessibleAfterFirstUnlock),
                0,
            )
            .map_err(|e| unavailable(&e))?;
            options.set_access_control(access);
            match passwords::set_generic_password_options(secret.as_bytes(), options) {
                Err(e) if e.code() == MISSING_ENTITLEMENT => {
                    passwords::set_generic_password(KEYCHAIN_SERVICE, &key, secret.as_bytes())
                        .map_err(|e| unavailable(&e))
                }
                other => other.map_err(|e| unavailable(&e)),
            }
        }

        fn read(&self, account: AccountId, item: Item) -> Result<String, StoreError> {
            let key = key_for(account, item);
            let bytes = match passwords::generic_password(protected(&key)) {
                Err(e) if e.code() == MISSING_ENTITLEMENT => {
                    passwords::get_generic_password(KEYCHAIN_SERVICE, &key)
                }
                other => other,
            }
            // **Absent is not the same as unreadable.** Before first unlock the store refuses
            // with a different code, and reporting that as "not there" would invite a caller
            // to mint a replacement for a secret that still exists — D-43's installation
            // secret first among them.
            .map_err(|e| {
                if e.code() == ITEM_NOT_FOUND {
                    StoreError::NotFound
                } else {
                    unavailable(&e)
                }
            })?;
            String::from_utf8(bytes)
                .map_err(|_| StoreError::Unavailable("the stored item was not text".into()))
        }

        fn delete(&self, account: AccountId, item: Item) -> Result<(), StoreError> {
            let key = key_for(account, item);
            let result = match passwords::delete_generic_password_options(protected(&key)) {
                Err(e) if e.code() == MISSING_ENTITLEMENT => {
                    passwords::delete_generic_password(KEYCHAIN_SERVICE, &key)
                }
                other => other,
            };
            match result {
                Ok(()) => Ok(()),
                // Deleting what is not there satisfies the caller's intent.
                Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
                // Anything else is the store refusing, and FR-4's erasure is a claim that the
                // item is gone — which a refusal does not establish.
                Err(e) => Err(unavailable(&e)),
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod unimplemented {
    use super::{CredentialStore, Item, StoreError};
    use sift_foundation::identity::AccountId;

    /// No backend on this platform yet.
    ///
    /// It **refuses** rather than falling back. D-71's rule is that a security absence
    /// refuses and a feature absence degrades visibly, and this is the first kind: the only
    /// fallback available is a file, which is precisely what the constraint forbids.
    #[derive(Debug, Clone, Copy)]
    pub struct Platform;

    impl Platform {
        pub(super) fn open() -> Result<Self, StoreError> {
            Err(StoreError::Unavailable(
                "no credential store backend is built for this platform".into(),
            ))
        }
    }

    impl CredentialStore for Platform {
        fn write(&self, _: AccountId, _: Item, _: &str) -> Result<(), StoreError> {
            Err(StoreError::Unavailable("no backend".into()))
        }
        fn read(&self, _: AccountId, _: Item) -> Result<String, StoreError> {
            Err(StoreError::Unavailable("no backend".into()))
        }
        fn delete(&self, _: AccountId, _: Item) -> Result<(), StoreError> {
            Err(StoreError::Unavailable("no backend".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_item_set_is_closed_so_erasure_is_an_enumeration() {
        // FR-4's claim rests on this: "has every credential for this account been erased?"
        // is a question with a finite answer rather than a search.
        //
        // **The `match` is the tripwire, not the count.** Rust cannot enumerate an enum's
        // variants without a derive, so nothing here can *prove* `ALL` is complete. What this
        // does instead is fail to compile the moment a variant is added — which puts the
        // author in this function, reading the sentence above, at the moment the omission
        // would otherwise become an unerased credential.
        for item in Item::ALL {
            let covered = match item {
                Item::Access
                | Item::Refresh
                | Item::SupersededAccess
                | Item::SupersededRefresh
                | Item::DatabaseKey => true,
            };
            assert!(covered);
        }
        assert_eq!(
            Item::ALL.len(),
            5,
            "a variant was added to the enum without joining the enumeration erasure walks"
        );
        let mut names: Vec<&str> = Item::ALL.iter().map(|i| i.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Item::ALL.len(), "two items share a name");
    }

    #[test]
    fn a_key_names_the_account_identity_and_not_an_address() {
        // D-89: the identity is Sift's own, independent of address and provider. Keying on
        // an address would mean two accounts at one address shared a token.
        let key = key_for(AccountId::from_u128(7), Item::Refresh);
        assert!(key.ends_with("/refresh"), "{key}");
        assert!(!key.contains('@'), "{key}");
    }

    #[test]
    fn two_accounts_do_not_share_a_key() {
        assert_ne!(
            key_for(AccountId::from_u128(1), Item::Access),
            key_for(AccountId::from_u128(2), Item::Access)
        );
    }

    #[test]
    fn the_superseded_pair_has_somewhere_durable_to_live() {
        // D-88's fourth point. A pair held only in memory would not survive the crash it
        // exists to protect against: against a provider that rotates refresh tokens, a
        // crash between receiving a new pair and using it would leave the account with a
        // token the provider has already invalidated and no way back.
        assert!(Item::ALL.contains(&Item::SupersededRefresh));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_secret_round_trips_through_the_platform_store_and_erases_completely() {
        // Writes to the real keychain under a scratch account identity, and removes it.
        let store = platform().expect("the platform store");
        let account = AccountId::from_u128(u128::from(std::process::id()) | (1 << 100));
        let _ = store.erase(account);

        store
            .write(account, Item::Refresh, "a-refresh-token")
            .unwrap();
        assert_eq!(
            store.read(account, Item::Refresh).unwrap(),
            "a-refresh-token"
        );
        assert_eq!(store.remaining(account), vec![Item::Refresh]);

        store.erase(account).unwrap();
        assert!(
            store.remaining(account).is_empty(),
            "FR-4's erasure left something behind"
        );
        assert_eq!(
            store.read(account, Item::Refresh),
            Err(StoreError::NotFound)
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_platform_with_no_backend_refuses_rather_than_writing_a_file() {
        assert!(matches!(platform(), Err(StoreError::Unavailable(_))));
    }
}

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
use sift_foundation::identifiers::KEYCHAIN_SERVICE;
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
    use super::{CredentialStore, Item, KEYCHAIN_SERVICE, StoreError, key_for};
    use security_framework::os::macos::keychain::SecKeychain;
    use sift_foundation::identity::AccountId;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// The platform keychain.
    ///
    /// The access group the items land in is decided by the bundle's entitlements rather
    /// than by this code — D-45 requires both macOS channels be sandboxed under one team,
    /// because Keychain access binds to the creating code's designated requirement and the
    /// ACLs only match if both builds present the same one.
    #[derive(Debug, Clone, Copy)]
    pub struct Platform;

    impl Platform {
        pub(super) const fn open() -> Result<Self, StoreError> {
            Ok(Self)
        }
    }

    /// `SIFT_KEYCHAIN`: a keychain file to use **instead of** the user's default one.
    ///
    /// It exists for Q-12's measurement protocol, which runs the shipping binary against a
    /// scratch container and must not read or write the credential items of the user's real
    /// installation. Reading them would also stop the run: an item's access list names the
    /// build that created it, so a fresh build asks for the login password, and nobody is
    /// there to type it.
    ///
    /// **Still the OS credential store, and never a fallback.** The named file is a keychain
    /// the platform opens and guards; nothing here writes a secret anywhere else. A name that
    /// cannot be opened is [`StoreError::Unavailable`] rather than a silent return to the
    /// default keychain, because that return would write into exactly the store the caller
    /// asked to keep out of. Read once per process: the store a process uses does not change
    /// under it.
    fn named() -> Result<Option<SecKeychain>, StoreError> {
        static NAMED: OnceLock<Option<PathBuf>> = OnceLock::new();
        let Some(path) = NAMED
            .get_or_init(|| {
                std::env::var_os("SIFT_KEYCHAIN")
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
            })
            .as_ref()
        else {
            return Ok(None);
        };
        open_named(path).map(Some)
    }

    /// Opens the keychain file at `path`, refusing one that does not exist.
    ///
    /// `SecKeychainOpen` does not check the file: it hands back a reference to a keychain
    /// that is not there, lookups through it then fail as not-found, and a delete through it
    /// succeeds having removed nothing. So a mistyped `SIFT_KEYCHAIN` would read as an
    /// account with no credentials rather than as the refusal the doc comment on `named`
    /// promises. The existence check is what makes that promise true.
    pub(super) fn open_named(path: &std::path::Path) -> Result<SecKeychain, StoreError> {
        if !path.is_file() {
            return Err(StoreError::Unavailable(format!(
                "SIFT_KEYCHAIN: no keychain file at {}",
                path.display()
            )));
        }
        SecKeychain::open(path).map_err(|e| StoreError::Unavailable(format!("SIFT_KEYCHAIN: {e}")))
    }

    impl CredentialStore for Platform {
        fn write(&self, account: AccountId, item: Item, secret: &str) -> Result<(), StoreError> {
            let key = key_for(account, item);
            match named()? {
                Some(keychain) => {
                    keychain.set_generic_password(KEYCHAIN_SERVICE, &key, secret.as_bytes())
                }
                None => security_framework::passwords::set_generic_password(
                    KEYCHAIN_SERVICE,
                    &key,
                    secret.as_bytes(),
                ),
            }
            .map_err(|e| StoreError::Unavailable(e.to_string()))
        }

        fn read(&self, account: AccountId, item: Item) -> Result<String, StoreError> {
            let key = key_for(account, item);
            let bytes = match named()? {
                Some(keychain) => keychain
                    .find_generic_password(KEYCHAIN_SERVICE, &key)
                    .map(|(password, _)| password.to_vec()),
                None => security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, &key),
            }
            .map_err(|_| StoreError::NotFound)?;
            String::from_utf8(bytes)
                .map_err(|_| StoreError::Unavailable("the stored item was not text".into()))
        }

        fn delete(&self, account: AccountId, item: Item) -> Result<(), StoreError> {
            let key = key_for(account, item);
            match named()? {
                Some(keychain) => {
                    if let Ok((_, found)) = keychain.find_generic_password(KEYCHAIN_SERVICE, &key) {
                        found.delete();
                    }
                }
                // Deleting what is not there satisfies the caller's intent.
                None => {
                    let _ = security_framework::passwords::delete_generic_password(
                        KEYCHAIN_SERVICE,
                        &key,
                    );
                }
            }
            Ok(())
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

    #[cfg(target_os = "macos")]
    #[test]
    fn a_named_keychain_that_does_not_exist_is_unavailable_rather_than_empty() {
        // A mistyped SIFT_KEYCHAIN must refuse, not read back as NotFound: the latter makes
        // an account look credential-less and makes a delete succeed having removed nothing.
        let missing = std::env::temp_dir().join(format!(
            "sift-no-such-keychain-{}.keychain-db",
            std::process::id()
        ));
        assert!(!missing.exists());
        assert!(matches!(
            macos::open_named(&missing),
            Err(StoreError::Unavailable(_))
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_platform_with_no_backend_refuses_rather_than_writing_a_file() {
        assert!(matches!(platform(), Err(StoreError::Unavailable(_))));
    }
}

//! The installation container: where accounts live, and what survives a restart.
//!
//! # What this fixes
//!
//! Until this existed the root was a temporary directory named after the process, wiped on
//! every run, and there was no registry at all — so **accounts did not survive a restart**, and
//! neither did anything in them. That was survivable only because nothing else was durable
//! either. It stops being survivable the moment a real account is added.
//!
//! # The registry is a sealed database, not a file of names
//!
//! It holds which accounts exist, what each declares, and whether writes are authorised for it.
//! That last one is the reason it is here rather than in the account's own store: an account
//! store is **discardable by construction** — D-73 says a file that fails to authenticate is
//! discarded and never repaired — and a security flag whose safe state can be flipped to unsafe
//! by deleting a file is not a flag.
//!
//! # Identity is random, and ordinals are never reused
//!
//! D-89 makes an account's identity Sift's own, and re-adding an account a *new* account. A
//! sequential identity breaks that: remove account 3 and add another, and the new one is
//! account 3 again — sharing the file names and the credential keys of a predecessor whose
//! erasure may not have finished. So identity is 128 random bits, and the ordinal, which is
//! only 16 bits and is what local identity is generated from, is monotonic and never reclaimed.

use rusqlite::Connection;
use sift_credentials::store::{CredentialStore, Item};
use sift_foundation::identity::{AccountId, AccountOrdinal};
use std::path::{Path, PathBuf};

/// The identity the installation's own secrets are filed under.
///
/// Zero is never issued to an account — identities are random 128-bit values and the odds are
/// not the argument; the argument is that this one is reserved, so nothing has to rely on them.
pub const INSTALLATION: AccountId = AccountId::from_u128(0);

/// One account, as the registry records it.
#[derive(Debug, Clone)]
pub struct Registered {
    pub id: AccountId,
    pub ordinal: AccountOrdinal,
    /// The provider kind, opaque above the adapters. A name, never a `match`.
    pub kind: String,
    pub display_name: String,
    /// Whether the user has authorised Sift to change this mailbox. **Default false.**
    pub writes_enabled: bool,
}

/// The installation's own directory and registry.
#[derive(Debug)]
pub struct Container {
    root: PathBuf,
    registry: Connection,
    /// The installation secret, from which the registry's own file key is derived.
    owner: [u8; 32],
}

impl Container {
    /// Open the container at `root`, creating it and its registry on first run.
    ///
    /// # Errors
    /// The directory cannot be made, the credential store is unavailable — which D-71 makes a
    /// refusal rather than a degradation, because the alternative is a key in a file — or the
    /// registry does not authenticate under the installation secret.
    pub fn open<S: CredentialStore>(root: &Path, credentials: &S) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let owner = installation_secret(credentials)?;
        let path = root.join("installation.registry");

        sift_store::vfs::register()?;
        sift_store::vfs::present_key(
            &path,
            sift_crypto::derive::file_key(&owner, sift_crypto::derive::Role::Policy),
            sift_crypto::derive::file_key_id(&owner, sift_crypto::derive::Role::Policy),
        );
        let opened = Self::open_registry(&path);
        sift_store::vfs::withdraw_key(&path);
        let registry = opened?;

        Ok(Self {
            root: root.to_path_buf(),
            registry,
            owner,
        })
    }

    fn open_registry(path: &Path) -> Result<Connection, String> {
        let conn = Connection::open_with_flags_and_vfs(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            sift_store::vfs::VFS_NAME,
        )
        .map_err(|e| format!("the registry did not open: {e}"))?;

        // Before anything writes: the page size cannot change once a database has pages.
        for (name, value) in [
            ("page_size", "4096"),
            ("temp_store", "MEMORY"),
            ("mmap_size", "0"),
        ] {
            conn.pragma_update(None, name, value)
                .map_err(|e| format!("{name}: {e}"))?;
        }
        let _: String = conn
            .pragma_update_and_check(None, "locking_mode", "EXCLUSIVE", |r| r.get(0))
            .map_err(|e| format!("locking_mode: {e}"))?;
        let _: String = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
            .map_err(|e| format!("journal_mode: {e}"))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| e.to_string())?;
        sift_store::vfs::assert_pragmas(&conn)?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS account (
                 -- D-89's identity: 128 random bits, and re-adding is a new account.
                 id             BLOB PRIMARY KEY NOT NULL,
                 -- What local identity is generated from. Monotonic, and never reclaimed.
                 ordinal        INTEGER NOT NULL UNIQUE,
                 kind           TEXT NOT NULL,
                 display_name   TEXT NOT NULL,
                 -- The account is watched until the user says otherwise. Here rather than in
                 -- the account's own store, which is discardable: a security flag whose safe
                 -- state can be flipped by deleting a file is not a flag.
                 writes_enabled INTEGER NOT NULL DEFAULT 0
             ) STRICT;
             CREATE TABLE IF NOT EXISTS ordinal_watermark (
                 -- One row. The highest ordinal ever issued, which is *not* the highest in
                 -- use: removing an account must not free its ordinal for the next one, or a
                 -- new account inherits the local identities of a removed one.
                 only_row       INTEGER PRIMARY KEY CHECK (only_row = 1),
                 next           INTEGER NOT NULL
             ) STRICT;
             INSERT OR IGNORE INTO ordinal_watermark (only_row, next) VALUES (1, 0);
             CREATE TABLE IF NOT EXISTS setting (
                 -- D-101's stable keys. A row exists only where the user has changed
                 -- something: the default is in the code, and a table pre-filled with
                 -- defaults would make a later change to one of them invisible to everyone
                 -- who had ever opened the settings screen.
                 key            TEXT PRIMARY KEY NOT NULL,
                 value          TEXT NOT NULL
             ) STRICT;
             CREATE TABLE IF NOT EXISTS account_setting (
                 -- D-101's other table. **The scope split is the storage split**, and until
                 -- this existed the account half had nowhere to live: every account setting
                 -- was refused on the way in, so the shells drew controls that were disabled
                 -- with a comment explaining that a value accepted here would be dropped.
                 --
                 -- Here rather than in the account's own store, for the reason `writes_enabled`
                 -- is: this is the table FR-4 erases by removing the account's row, and one
                 -- transaction removing both is one thing that can fail rather than two.
                 account        BLOB NOT NULL REFERENCES account(id),
                 key            TEXT NOT NULL,
                 value          TEXT NOT NULL,
                 PRIMARY KEY (account, key)
             ) STRICT;",
        )
        .map_err(|e| format!("the registry schema: {e}"))?;
        Ok(conn)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub const fn owner(&self) -> &[u8; 32] {
        &self.owner
    }

    /// Every account, oldest first.
    ///
    /// **FR-4's erasure is provable because this is the enumeration.** What is not listed here
    /// does not exist, so an account removed from this table with its files and credentials
    /// gone is gone — there is no second place to look and no residue to miss.
    ///
    /// # Errors
    /// The registry could not be read.
    pub fn accounts(&self) -> Result<Vec<Registered>, String> {
        let mut statement = self
            .registry
            .prepare(
                "SELECT id, ordinal, kind, display_name, writes_enabled
                 FROM account ORDER BY ordinal",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |r| {
                let id: Vec<u8> = r.get(0)?;
                let ordinal: i64 = r.get(1)?;
                Ok(Registered {
                    id: AccountId::from_u128(be_u128(&id)),
                    ordinal: AccountOrdinal::new(u16::try_from(ordinal).unwrap_or(u16::MAX)),
                    kind: r.get(2)?,
                    display_name: r.get(3)?,
                    writes_enabled: r.get::<_, i64>(4)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Record a new account and hand back its identity and ordinal.
    ///
    /// # Errors
    /// The registry refused, or every ordinal has been issued — which is 65,536 accounts over
    /// the life of one installation, and is a refusal rather than a wrap, because wrapping
    /// hands a new account the local identities of an old one.
    pub fn register(&mut self, kind: &str, display_name: &str) -> Result<Registered, String> {
        let next: i64 = self
            .registry
            .query_row(
                "SELECT next FROM ordinal_watermark WHERE only_row = 1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let ordinal = u16::try_from(next)
            .map_err(|_| "this installation has issued every ordinal it has".to_owned())?;

        let id = AccountId::from_u128(random_identity());
        self.registry
            .execute(
                "INSERT INTO account (id, ordinal, kind, display_name, writes_enabled)
                 VALUES (?1, ?2, ?3, ?4, 0)",
                rusqlite::params![
                    id.as_u128().to_be_bytes().to_vec(),
                    next,
                    kind,
                    display_name
                ],
            )
            .map_err(|e| e.to_string())?;
        // Bumped in the same transaction-free step but *after* the insert, so a failure leaves
        // the watermark where it was rather than skipping an ordinal nobody used.
        self.registry
            .execute(
                "UPDATE ordinal_watermark SET next = ?1 WHERE only_row = 1",
                [next + 1],
            )
            .map_err(|e| e.to_string())?;

        Ok(Registered {
            id,
            ordinal: AccountOrdinal::new(ordinal),
            kind: kind.to_owned(),
            display_name: display_name.to_owned(),
            writes_enabled: false,
        })
    }

    /// Authorise writes for an account, or withdraw the authorisation.
    ///
    /// # Errors
    /// The registry refused, or there is no such account.
    pub fn set_writes_enabled(&mut self, id: AccountId, enabled: bool) -> Result<(), String> {
        let changed = self
            .registry
            .execute(
                "UPDATE account SET writes_enabled = ?2 WHERE id = ?1",
                rusqlite::params![id.as_u128().to_be_bytes().to_vec(), i64::from(enabled)],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("no account {id} in the registry"));
        }
        Ok(())
    }

    /// One installation setting, or `None` where the user has not changed it.
    ///
    /// **Absent means "the default", not "false".** A row exists only where somebody set
    /// something, so a later change to a default reaches every installation that never touched
    /// it — which is the point of not pre-filling the table.
    ///
    /// # Errors
    /// The registry could not be read.
    pub fn setting(&self, key: &str) -> Result<Option<String>, String> {
        match self
            .registry
            .query_row("SELECT value FROM setting WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            }) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Record an installation setting.
    ///
    /// # Errors
    /// The registry refused, or the key is not one D-101 enumerates — an unknown key is a
    /// refusal rather than a row, because a settings store that accepts anything is one that
    /// accumulates keys nothing reads.
    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        let setting = crate::settings::by_key(key)
            .ok_or_else(|| format!("`{key}` is not a setting this build has"))?;
        if setting.scope != crate::settings::Scope::Installation {
            return Err(format!(
                "`{key}` is an account setting, not an installation one"
            ));
        }
        if setting.is_security_state {
            // The per-sender lists are records of decisions made in context. They are shown
            // and revoked from the surface where the decision was made, not bulk-edited here.
            return Err(format!(
                "`{key}` is security state and is not set through settings"
            ));
        }
        setting
            .default
            .parse_like(value)
            .ok_or_else(|| format!("`{value}` is not a value `{key}` can hold"))?;
        self.registry
            .execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// One account setting, or `None` where the user has not changed it.
    ///
    /// # Errors
    /// The registry could not be read.
    pub fn account_setting(&self, id: AccountId, key: &str) -> Result<Option<String>, String> {
        match self.registry.query_row(
            "SELECT value FROM account_setting WHERE account = ?1 AND key = ?2",
            rusqlite::params![id.as_u128().to_be_bytes().to_vec(), key],
            |r| r.get::<_, String>(0),
        ) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Record an account setting.
    ///
    /// # Errors
    /// The registry refused, the key is not one D-101 enumerates, it is an installation
    /// setting rather than an account one, it is security state, or the value is not one that
    /// setting can hold.
    pub fn set_account_setting(
        &mut self,
        id: AccountId,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let setting = crate::settings::by_key(key)
            .ok_or_else(|| format!("`{key}` is not a setting this build has"))?;
        if setting.scope != crate::settings::Scope::Account {
            return Err(format!(
                "`{key}` is an installation setting, not an account one"
            ));
        }
        if setting.is_security_state {
            // The per-sender lists are records of decisions made in context. They are shown
            // and revoked from the surface where the decision was made, not bulk-edited here.
            // **This refusal is the one that does not move**: giving the account half of the
            // table somewhere to live must not give these two a bulk editor by accident.
            return Err(format!(
                "`{key}` is security state and is not set through settings"
            ));
        }
        setting
            .default
            .parse_like(value)
            .ok_or_else(|| format!("`{value}` is not a value `{key}` can hold"))?;
        self.registry
            .execute(
                "INSERT INTO account_setting (account, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account, key) DO UPDATE SET value = excluded.value",
                rusqlite::params![id.as_u128().to_be_bytes().to_vec(), key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// FR-4 — erase an account, by enumeration.
    ///
    /// The registry row, both files, and every credential item. The ordinal is **not** freed:
    /// D-78 makes local identity never reused, and handing the next account a removed one's
    /// ordinal would give it that account's local identities.
    ///
    /// # Errors
    /// The registry refused. A file that is already gone is not an error — the caller's intent
    /// is that it must not exist, and it does not.
    pub fn forget<S: CredentialStore>(
        &mut self,
        id: AccountId,
        credentials: &S,
    ) -> Result<(), String> {
        // Credentials first. Destroying the key is what makes the files unreadable, so a
        // failure part-way through leaves ciphertext nobody can open rather than plaintext
        // nobody has removed.
        for item in Item::ALL {
            match credentials.delete(id, *item) {
                Ok(()) => {}
                Err(sift_credentials::store::StoreError::NotFound) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let paths = sift_store::account::AccountPaths::under(&self.root, id);
        for path in [&paths.store, &paths.journal] {
            for suffix in ["", "-wal", "-journal", "-shm"] {
                let mut p = path.clone().into_os_string();
                p.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(p));
            }
        }
        // **One transaction, which is what the schema comment above claims.** The settings
        // reference the row, so they go first — and both go or neither does, or a failure
        // between them leaves an account whose per-account decisions have been erased and
        // which is otherwise intact.
        let removal = self.registry.transaction().map_err(|e| e.to_string())?;
        removal
            .execute(
                "DELETE FROM account_setting WHERE account = ?1",
                [id.as_u128().to_be_bytes().to_vec()],
            )
            .map_err(|e| e.to_string())?;
        removal
            .execute(
                "DELETE FROM account WHERE id = ?1",
                [id.as_u128().to_be_bytes().to_vec()],
            )
            .map_err(|e| e.to_string())?;
        removal.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Anything in the container that no registry row claims.
    ///
    /// A crash between creating an account's files and recording it leaves exactly this. They
    /// are unreadable — the key is filed under an identity nothing lists — so they are not a
    /// disclosure; they are dead bytes, and NFR-14's budget is not a place to leave any.
    ///
    /// # Errors
    /// The container or the registry could not be read.
    pub fn orphans(&self) -> Result<Vec<PathBuf>, String> {
        let known: std::collections::BTreeSet<String> = self
            .accounts()?
            .iter()
            .map(|a| format!("{}", a.id))
            .collect();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("installation.registry") {
                continue;
            }
            let stem = name.split('.').next().unwrap_or(name);
            if !known.contains(stem) {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }
}

/// The installation's own 32-byte secret, from the credential store.
///
/// Created on first run and never anywhere but there. NFR-23 admits no fallback: a platform
/// with no credential store **refuses**, because the only fallback available is a file, which
/// is what the constraint forbids by name.
fn installation_secret<S: CredentialStore>(credentials: &S) -> Result<[u8; 32], String> {
    match credentials.read(INSTALLATION, Item::DatabaseKey) {
        Ok(existing) => decode_secret(&existing),
        Err(sift_credentials::store::StoreError::NotFound) => {
            let mut secret = [0u8; 32];
            getrandom::fill(&mut secret).map_err(|e| format!("no randomness: {e}"))?;
            credentials
                .write(INSTALLATION, Item::DatabaseKey, &encode_secret(&secret))
                .map_err(|e| e.to_string())?;
            Ok(secret)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The account key, from the credential store, created on first use.
///
/// # Errors
/// The credential store is unavailable, or the stored value is not a key.
pub fn account_secret<S: CredentialStore>(
    credentials: &S,
    id: AccountId,
) -> Result<[u8; 32], String> {
    match credentials.read(id, Item::DatabaseKey) {
        Ok(existing) => decode_secret(&existing),
        Err(sift_credentials::store::StoreError::NotFound) => {
            let mut secret = [0u8; 32];
            getrandom::fill(&mut secret).map_err(|e| format!("no randomness: {e}"))?;
            credentials
                .write(id, Item::DatabaseKey, &encode_secret(&secret))
                .map_err(|e| e.to_string())?;
            Ok(secret)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Hex, because the credential store holds strings and a key is bytes.
fn encode_secret(secret: &[u8; 32]) -> String {
    secret.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn decode_secret(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 {
        return Err("the stored key is not 32 bytes".to_owned());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|_| "the stored key is not hexadecimal".to_owned())?;
    }
    Ok(out)
}

/// 128 random bits.
///
/// Not a counter. D-89 makes re-adding an account a *new* account, and a counter breaks that:
/// remove account 3, add another, and the new one is account 3 again — sharing the file names
/// and credential keys of a predecessor whose erasure may not have finished.
fn random_identity() -> u128 {
    let mut bytes = [0u8; 16];
    // A failure here would mean the platform has no randomness at all. Falling back to a
    // counter would silently reintroduce exactly the collision this exists to prevent, so the
    // fallback is the clock — which is not a secret and does not need to be, because the
    // requirement is uniqueness rather than unpredictability.
    if getrandom::fill(&mut bytes).is_err() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes = now.to_be_bytes();
    }
    u128::from_be_bytes(bytes)
}

fn be_u128(bytes: &[u8]) -> u128 {
    let mut out = [0u8; 16];
    let n = bytes.len().min(16);
    out[16 - n..].copy_from_slice(&bytes[..n]);
    u128::from_be_bytes(out)
}

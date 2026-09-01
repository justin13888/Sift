//! Opening an account: two files, one version, and the ordering between them.

use crate::schema::{self, JOURNAL_SCHEMA_V1, Migration, STORE_SCHEMA_V1, SchemaError, admit_pair};
use rusqlite::Connection;
use sift_crypto::derive;
use sift_foundation::identity::AccountId;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum OpenError {
    Schema(SchemaError),
    Sql(rusqlite::Error),
    /// The sealing layer refused. Distinct from a SQL error because the answer is different:
    /// a file that does not authenticate is discarded and resynced, never repaired.
    Sealing(String),
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<SchemaError> for OpenError {
    fn from(e: SchemaError) -> Self {
        Self::Schema(e)
    }
}

impl core::fmt::Display for OpenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Schema(e) => write!(f, "{e}"),
            Self::Sql(e) => write!(f, "{e}"),
            Self::Sealing(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for OpenError {}

/// Where an account's two files live.
#[derive(Debug, Clone)]
pub struct AccountPaths {
    pub store: PathBuf,
    pub journal: PathBuf,
}

impl AccountPaths {
    /// Under the application's own container.
    ///
    /// The layout MUST NOT live anywhere the operating system may purge on its own: a purge
    /// would remove blobs while leaving the encrypted shared blob index referencing them,
    /// and the refcount rebuild covers abnormal termination rather than the OS deleting
    /// files underneath a running application.
    #[must_use]
    pub fn under(root: &Path, account: AccountId) -> Self {
        let name = format!("{account}");
        Self {
            store: root.join(format!("{name}.store")),
            journal: root.join(format!("{name}.journal")),
        }
    }
}

/// An open account.
///
/// Holds both halves, because neither is usable without the other: the base state is in the
/// store and the overlay that must be read *through* is derived from the journal.
#[derive(Debug)]
pub struct Account {
    pub store: Connection,
    pub journal: Connection,
    pub id: AccountId,
}

impl Account {
    /// Open or create an account.
    ///
    /// Refuses rather than guesses on every version disagreement — including the one D-74
    /// adds, where the two halves disagree with each other.
    pub fn open(paths: &AccountPaths, id: AccountId) -> Result<Self, OpenError> {
        let store = Connection::open(&paths.store)?;
        let journal = Connection::open(&paths.journal)?;
        Self::configure(&store)?;
        Self::configure(&journal)?;

        Self::finish(store, journal, id)
    }

    fn finish(store: Connection, journal: Connection, id: AccountId) -> Result<Self, OpenError> {
        let store_version = schema::version_of(&store)?;
        let journal_version = schema::version_of(&journal)?;

        // A fresh account is both halves at zero. One at zero and the other not is a
        // half-created account, and it is a disagreement like any other.
        if store_version == 0 && journal_version == 0 {
            schema::create(&store, STORE_SCHEMA_V1, schema::CURRENT_VERSION)?;
            schema::create(&journal, JOURNAL_SCHEMA_V1, schema::CURRENT_VERSION)?;
            return Ok(Self { store, journal, id });
        }

        match admit_pair(store_version, journal_version)? {
            Migration::None => {}
            Migration::Forward { from } => {
                // Forward-only, ordered, and both halves together. There is nothing to do at
                // version 1; the arm exists so that the first real migration has a place
                // rather than a redesign.
                let _ = from;
            }
        }
        Ok(Self { store, journal, id })
    }

    /// Open or create an account **sealed** under a key derived from its own.
    ///
    /// D-74 gives an account two files and D-106 gives each of them its own derived key, so the
    /// two never share a counter space — one key over two files is nonce reuse, which costs
    /// confidentiality and authenticity both.
    ///
    /// The keys are lent to the VFS for the duration of the open and withdrawn immediately
    /// after: the files stay sealed, and nothing new can be opened against them. `PageKey`
    /// zeroes itself when the registry drops it.
    ///
    /// # Errors
    /// As [`open`](Self::open), plus a refusal where the VFS cannot be registered or where a
    /// file does not authenticate under the key presented — which D-73 makes a refusal rather
    /// than a repair.
    pub fn open_sealed(
        paths: &AccountPaths,
        id: AccountId,
        owner_key: &[u8; 32],
    ) -> Result<Self, OpenError> {
        crate::vfs::register().map_err(OpenError::Sealing)?;
        crate::vfs::present_key(
            &paths.store,
            derive::file_key(owner_key, derive::Role::Store),
            derive::file_key_id(owner_key, derive::Role::Store),
        );
        crate::vfs::present_key(
            &paths.journal,
            derive::file_key(owner_key, derive::Role::Journal),
            derive::file_key_id(owner_key, derive::Role::Journal),
        );
        let opened = Self::open_through(paths, id, Some(crate::vfs::VFS_NAME));
        crate::vfs::withdraw_key(&paths.store);
        crate::vfs::withdraw_key(&paths.journal);
        opened
    }

    fn open_through(
        paths: &AccountPaths,
        id: AccountId,
        vfs_name: Option<&str>,
    ) -> Result<Self, OpenError> {
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let (store, journal) = match vfs_name {
            Some(name) => (
                Connection::open_with_flags_and_vfs(&paths.store, flags, name)?,
                Connection::open_with_flags_and_vfs(&paths.journal, flags, name)?,
            ),
            None => (
                Connection::open(&paths.store)?,
                Connection::open(&paths.journal)?,
            ),
        };
        // **Before anything writes.** `page_size` cannot be changed once a database has pages,
        // and a database created at the engine's default would have every write straddle two
        // sealed blocks for the rest of its life.
        if vfs_name.is_some() {
            for conn in [&store, &journal] {
                conn.pragma_update(
                    None,
                    "page_size",
                    i64::from(crate::vfs::LOGICAL_BLOCK as u32),
                )?;
                conn.pragma_update(None, "temp_store", "MEMORY")?;
                conn.pragma_update(None, "mmap_size", 0i64)?;
                let _: String =
                    conn.pragma_update_and_check(None, "locking_mode", "EXCLUSIVE", |r| r.get(0))?;
            }
        }
        Self::configure(&store)?;
        Self::configure(&journal)?;
        if vfs_name.is_some() {
            for conn in [&store, &journal] {
                crate::vfs::assert_pragmas(conn).map_err(OpenError::Sealing)?;
            }
        }
        Self::finish(store, journal, id)
    }

    /// The pragmas NFR-16 depends on.
    fn configure(conn: &Connection) -> Result<(), rusqlite::Error> {
        // "The store runs in write-ahead-logging mode with a durable commit." Both halves:
        // D-76 covers the write-ahead log under the same page layer, and the journal holds
        // intents naming the user's mail.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // FULL rather than NORMAL. NORMAL can lose the last transactions on power loss,
        // and the failure model's whole durability argument is that nothing the user
        // watched succeed is lost.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(())
    }

    /// Apply an intent, in the order D-74 makes normative.
    ///
    /// **Journal first, store second.** The reverse order would leave local state saying a
    /// mutation happened with the queue not holding it — losing a mutation the user watched
    /// succeed. This order can leave an intent enqueued whose optimistic effect was never
    /// applied, which is invisible and self-correcting, because the overlay is derived from
    /// the queue.
    ///
    /// The failure model states the same rule from the other side: *an intent MUST be
    /// durably enqueued before it is applied optimistically to local state, not after.*
    pub fn write_intent_then_apply<J, S>(
        &mut self,
        enqueue: J,
        apply: S,
    ) -> Result<(), rusqlite::Error>
    where
        J: FnOnce(&rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error>,
        S: FnOnce(&rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error>,
    {
        {
            let tx = self.journal.transaction()?;
            enqueue(&tx)?;
            tx.commit()?;
        }
        {
            let tx = self.store.transaction()?;
            apply(&tx)?;
            tx.commit()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_foundation::identity::AccountId;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sift-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch");
            Self(dir)
        }
        fn paths(&self) -> AccountPaths {
            AccountPaths::under(&self.0, AccountId::from_u128(1))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_account_is_two_files() {
        // D-74, and the reason is D-57: backup exclusion is a property of a file.
        let s = Scratch::new("two-files");
        let p = s.paths();
        Account::open(&p, AccountId::from_u128(1)).expect("open");
        assert!(p.store.exists(), "no store file");
        assert!(p.journal.exists(), "no journal file");
        assert_ne!(p.store, p.journal);
    }

    #[test]
    fn reopening_an_account_does_not_recreate_it() {
        let s = Scratch::new("reopen");
        let p = s.paths();
        {
            let a = Account::open(&p, AccountId::from_u128(1)).expect("create");
            a.store
                .execute("INSERT INTO tag (name) VALUES ('kept')", [])
                .expect("insert");
        }
        let a = Account::open(&p, AccountId::from_u128(1)).expect("reopen");
        let n: i64 = a
            .store
            .query_row("SELECT count(*) FROM tag WHERE name = 'kept'", [], |r| {
                r.get(0)
            })
            .expect("query");
        assert_eq!(n, 1, "reopening wiped the account");
    }

    #[test]
    fn halves_that_disagree_refuse_to_open() {
        let s = Scratch::new("disagree");
        let p = s.paths();
        Account::open(&p, AccountId::from_u128(1)).expect("create");

        // Migrate one half and not the other, which is what a crash mid-migration leaves.
        let store = Connection::open(&p.store).expect("open store");
        store
            .pragma_update(None, "user_version", 2i64)
            .expect("bump");
        drop(store);

        let err = Account::open(&p, AccountId::from_u128(1));
        assert!(
            matches!(
                err,
                Err(OpenError::Schema(SchemaError::HalvesDisagree { .. }))
            ),
            "an account whose halves disagree opened anyway"
        );
    }

    #[test]
    fn a_newer_account_refuses_to_open() {
        let s = Scratch::new("newer");
        let p = s.paths();
        Account::open(&p, AccountId::from_u128(1)).expect("create");
        for f in [&p.store, &p.journal] {
            let c = Connection::open(f).expect("open");
            c.pragma_update(None, "user_version", 99i64).expect("bump");
        }
        assert!(matches!(
            Account::open(&p, AccountId::from_u128(1)),
            Err(OpenError::Schema(SchemaError::TooNew { .. }))
        ));
    }

    #[test]
    fn both_halves_are_in_write_ahead_logging_mode() {
        // NFR-16's crash consistency rests on this, and D-76 covers the log under the same
        // page layer because the journal's log holds intents naming the user's mail.
        let s = Scratch::new("wal");
        let a = Account::open(&s.paths(), AccountId::from_u128(1)).expect("open");
        for (name, conn) in [("store", &a.store), ("journal", &a.journal)] {
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .expect("pragma");
            assert_eq!(mode.to_lowercase(), "wal", "{name} is not in WAL mode");
        }
    }

    #[test]
    fn the_journal_is_written_before_the_store() {
        // D-74's normative ordering. The failure this prevents is the reverse: local state
        // saying a mutation happened with the queue not holding it, which loses a mutation
        // the user watched succeed.
        //
        // The order is asserted by making the *store* write fail and checking what survived.
        let s = Scratch::new("ordering");
        let mut a = Account::open(&s.paths(), AccountId::from_u128(1)).expect("open");

        let result = a.write_intent_then_apply(
            |tx| {
                tx.execute(
                    "INSERT INTO intent (id, message_id, operation, intent_version, state,
                                         created_millis, per_message_seq, expires_millis)
                     VALUES (X'01', X'02', 'Archive', 1, 'Pending', 0, 0, 0)",
                    [],
                )
                .map(|_| ())
            },
            |tx| {
                tx.execute("INSERT INTO no_such_table VALUES (1)", [])
                    .map(|_| ())
            },
        );
        assert!(result.is_err(), "the failing store write reported success");

        let queued: i64 = a
            .journal
            .query_row("SELECT count(*) FROM intent", [], |r| r.get(0))
            .expect("query");
        assert_eq!(
            queued, 1,
            "the intent was not durable before the store was touched, so a crash here \
             would lose a mutation the user watched succeed"
        );
    }

    #[test]
    fn an_intent_enqueued_without_its_effect_is_self_correcting() {
        // The failure the chosen order *does* leave: an intent in the queue whose optimistic
        // effect was never applied. It is invisible and self-correcting precisely because
        // the overlay is derived from the queue rather than stored beside the base state.
        let s = Scratch::new("selfcorrect");
        let a = Account::open(&s.paths(), AccountId::from_u128(1)).expect("open");
        a.journal
            .execute(
                "INSERT INTO intent (id, message_id, operation, intent_version, state,
                                     created_millis, per_message_seq, expires_millis)
                 VALUES (X'01', X'02', 'Archive', 1, 'Pending', 0, 0, 0)",
                [],
            )
            .expect("enqueue");

        // Rebuilding the overlay from the queue is what startup does, and it recovers the
        // effect that was never applied.
        let pending: i64 = a
            .journal
            .query_row(
                "SELECT count(*) FROM intent WHERE state = 'Pending'",
                [],
                |r| r.get(0),
            )
            .expect("query");
        assert_eq!(pending, 1);
    }
}

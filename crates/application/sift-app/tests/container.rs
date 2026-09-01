//! What survives a restart, and what it costs when it does not.
//!
//! # The defect these were written against
//!
//! `Queue::new()` started empty and nothing in the product ever read an intent back: the only
//! two `SELECT … FROM intent` statements in the workspace were both inside a `#[cfg(test)]`
//! block. That was harmless for exactly as long as nothing persisted. The moment the container
//! became durable it would have meant that **every gesture a user watched succeed, and that had
//! not yet been issued, vanished at the next launch** — which is the precise failure D-74 makes
//! the journal undiscardable to prevent.
//!
//! So these tests do not check that a rebuild function exists. They archive a message, drop
//! everything, reopen the container, and look for the intent.

use sift_credentials::store::{CredentialStore, Item, StoreError};
use sift_foundation::identity::AccountId;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

fn scratch(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "sift-container-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

/// A credential store in memory.
///
/// The platform's own is the one Sift ships with, and a test cannot use it: it would write to
/// the developer's login keychain and prompt for permission. This has the same contract, which
/// is what these tests are about — the container asking for a key and getting one.
#[derive(Default)]
struct InMemory {
    items: Mutex<BTreeMap<(u128, &'static str), String>>,
}

impl CredentialStore for InMemory {
    fn write(&self, account: AccountId, item: Item, secret: &str) -> Result<(), StoreError> {
        self.items
            .lock()
            .map_err(|_| StoreError::Unavailable("poisoned".into()))?
            .insert((account.as_u128(), item.name()), secret.to_owned());
        Ok(())
    }

    fn read(&self, account: AccountId, item: Item) -> Result<String, StoreError> {
        self.items
            .lock()
            .map_err(|_| StoreError::Unavailable("poisoned".into()))?
            .get(&(account.as_u128(), item.name()))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    fn delete(&self, account: AccountId, item: Item) -> Result<(), StoreError> {
        self.items
            .lock()
            .map_err(|_| StoreError::Unavailable("poisoned".into()))?
            .remove(&(account.as_u128(), item.name()));
        Ok(())
    }
}

/// FR-4 enumerates the items, so the key is one of them. An erasure that walked a hard-coded
/// list would have missed the one that matters most: destroying it is what makes the files
/// unreadable, which is the difference between erasure being provable and being believed.
#[test]
fn the_database_key_is_one_of_the_items_erasure_enumerates() {
    assert!(
        Item::ALL.contains(&Item::DatabaseKey),
        "FR-4's enumeration does not cover the key the files are sealed under"
    );
}

#[test]
fn a_registry_survives_being_closed_and_reopened() {
    use sift_app::container::Container;
    let dir = scratch("registry");
    let store = InMemory::default();

    let first = {
        let mut container = Container::open(&dir, &store).expect("open");
        let a = container.register("gmail", "work").expect("register");
        let b = container.register("gmail", "personal").expect("register");
        assert_ne!(a.id, b.id, "two accounts, two identities");
        assert_ne!(a.ordinal.get(), b.ordinal.get());
        assert!(!a.writes_enabled, "an account is watched, not written to");
        vec![a, b]
    };

    let container = Container::open(&dir, &store).expect("reopen");
    let listed = container.accounts().expect("read back");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, first[0].id);
    assert_eq!(listed[1].display_name, "personal");
    std::fs::remove_dir_all(&dir).ok();
}

/// D-78: local identity is never reused. Removing an account must not free its ordinal, or the
/// next account inherits the local identities of the one that was removed.
#[test]
fn an_ordinal_is_never_reclaimed() {
    use sift_app::container::Container;
    let dir = scratch("ordinals");
    let store = InMemory::default();
    let mut container = Container::open(&dir, &store).expect("open");

    let first = container.register("gmail", "work").expect("register");
    container.forget(first.id, &store).expect("forget");
    let second = container.register("gmail", "work again").expect("register");

    assert!(
        second.ordinal.get() > first.ordinal.get(),
        "the removed account's ordinal was handed to its replacement: {} then {}",
        first.ordinal.get(),
        second.ordinal.get()
    );
    assert_ne!(
        second.id, first.id,
        "re-adding an account is a new account — D-89"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// FR-4, as a property rather than as a call: after erasure there is nothing left that names
/// the account, and nothing left that could read what it held.
#[test]
fn erasing_an_account_leaves_nothing_that_names_it() {
    use sift_app::container::Container;
    let dir = scratch("erase");
    let store = InMemory::default();
    let mut container = Container::open(&dir, &store).expect("open");
    let account = container.register("gmail", "work").expect("register");

    // Give it the two things an account has: files, and a key.
    let owner = sift_app::container::account_secret(&store, account.id).expect("a key");
    let paths = sift_store::account::AccountPaths::under(&dir, account.id);
    drop(
        sift_store::account::Account::open_sealed(&paths, account.id, &owner).expect("open sealed"),
    );
    assert!(paths.store.exists(), "the account has files to erase");

    container.forget(account.id, &store).expect("forget");

    assert!(
        container
            .accounts()
            .expect("read")
            .iter()
            .all(|a| a.id != account.id),
        "the registry still lists it"
    );
    assert!(!paths.store.exists(), "the store file is still there");
    assert!(!paths.journal.exists(), "the journal file is still there");
    for item in Item::ALL {
        assert_eq!(
            store.read(account.id, *item),
            Err(StoreError::NotFound),
            "`{}` survived the erasure",
            item.name()
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A crash between creating an account's files and recording it leaves exactly this. The files
/// are unreadable — the key is filed under an identity nothing lists — so they are dead bytes
/// rather than a disclosure, and NFR-14's budget is not a place to leave any.
#[test]
fn files_no_registry_row_claims_are_reported_as_orphans() {
    use sift_app::container::Container;
    let dir = scratch("orphans");
    let store = InMemory::default();
    let container = Container::open(&dir, &store).expect("open");
    assert!(container.orphans().expect("sweep").is_empty());

    let stray = AccountId::from_u128(0xdead_beef);
    let paths = sift_store::account::AccountPaths::under(&dir, stray);
    std::fs::write(&paths.store, b"left behind").expect("write");

    let found = container.orphans().expect("sweep");
    assert!(
        found.contains(&paths.store),
        "the stray file was not reported: {found:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The keystone. A gesture that was durably enqueued and not yet issued is still there after a
/// restart — which is what the journal being undiscardable is *for*, and what nothing in the
/// product actually did until the queue could be rebuilt.
#[test]
fn a_queued_gesture_survives_a_restart() {
    use sift_foundation::identity::{AccountOrdinal, LocalIdGenerator};
    use sift_mutations::intent::{Intent, State};
    use sift_mutations::queue::Queue;

    let dir = scratch("queued");
    let store = InMemory::default();
    let id = AccountId::from_u128(0x1234_5678_9abc_def0);
    let owner = sift_app::container::account_secret(&store, id).expect("a key");
    let paths = sift_store::account::AccountPaths::under(&dir, id);

    let ids = LocalIdGenerator::new(AccountOrdinal::new(0));
    let message = ids.next();

    // The first run: enqueue a move to folder 7, durably.
    {
        let account =
            sift_store::account::Account::open_sealed(&paths, id, &owner).expect("open sealed");
        let intent = Intent::MoveTo { folder: 7 };
        account
            .journal
            .execute(
                "INSERT INTO intent (id, undo_group, message_id, operation, parameter,
                                     intent_version, state, created_millis, per_message_seq,
                                     expires_millis)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, 'Pending', 100, 0, 999999)",
                rusqlite::params![
                    1u128.to_be_bytes().to_vec(),
                    9u128.to_be_bytes().to_vec(),
                    message.to_bytes().to_vec(),
                    intent.name(),
                    intent.parameter(),
                ],
            )
            .expect("the journal accepts the intent");
    }

    // The second run: nothing carried over but the files and the key.
    let account =
        sift_store::account::Account::open_sealed(&paths, id, &owner).expect("reopen sealed");
    let restored = read_back(&account.journal);
    let mut queue = Queue::new();
    queue.restore(restored);

    assert_eq!(queue.len(), 1, "the gesture did not survive the restart");
    let entry = &queue.entries()[0];
    assert_eq!(entry.state, State::Pending);
    assert_eq!(entry.message, message);
    assert_eq!(
        entry.intent,
        Intent::MoveTo { folder: 7 },
        "the move came back without its destination, which is not a move"
    );
    assert_eq!(entry.undo_group, Some(9));
    drop(account);
    std::fs::remove_dir_all(&dir).ok();
}

/// The same read the application performs, duplicated here only because it is private to it.
fn read_back(journal: &rusqlite::Connection) -> Vec<sift_mutations::queue::Restored> {
    use sift_mutations::intent::{Intent, State};
    use sift_mutations::queue::Restored;
    let mut statement = journal
        .prepare(
            "SELECT id, undo_group, message_id, operation, parameter, state, attempt_count,
                    created_millis, per_message_seq
             FROM intent WHERE state != 'Settled'",
        )
        .expect("prepare");
    let rows = statement
        .query_map([], |r| {
            let id: Vec<u8> = r.get(0)?;
            let undo: Option<Vec<u8>> = r.get(1)?;
            let message: Vec<u8> = r.get(2)?;
            let operation: String = r.get(3)?;
            let parameter: Option<String> = r.get(4)?;
            let state: String = r.get(5)?;
            let bytes: [u8; 16] = message.as_slice().try_into().expect("16 bytes");
            Ok(Restored {
                id: be(&id),
                undo_group: undo.as_deref().map(be),
                message: sift_foundation::identity::LocalId::from_bytes(bytes),
                intent: Intent::from_parts(&operation, parameter.as_deref())
                    .unwrap_or(Intent::Archive),
                state: State::from_name(&state),
                attempts: u32::try_from(r.get::<_, i64>(6)?).unwrap_or(0),
                created_millis: u64::try_from(r.get::<_, i64>(7)?).unwrap_or(0),
                sequence: u64::try_from(r.get::<_, i64>(8)?).unwrap_or(0),
            })
        })
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

fn be(bytes: &[u8]) -> u128 {
    let mut out = [0u8; 16];
    let n = bytes.len().min(16);
    out[16 - n..].copy_from_slice(&bytes[..n]);
    u128::from_be_bytes(out)
}

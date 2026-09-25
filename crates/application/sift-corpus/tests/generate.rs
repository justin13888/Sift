//! The corpus is a container the application opens, holding the shape it was asked for.
//!
//! These write a small corpus — the scale corpus's proportions, a thousand times smaller —
//! through the real container, the real sealed files and the real ingest, and then read it back
//! the way a launch does. A test that inspected its own bookkeeping would prove nothing about
//! what a cold start finds.

use sift_app::container::Container;
use sift_corpus::{CorpusError, KIND, Options, Shape, generate};
use sift_credentials::store::{CredentialStore, Item, StoreError};
use sift_foundation::identity::AccountId;
use sift_store::account::{Account, AccountPaths};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// A credential store in memory. The platform's own would write to the developer's keychain.
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

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "sift-corpus-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch");
        Self(d)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn small() -> Options {
    Options {
        shape: Shape::SCALE.divided(1000).expect("divide"),
        ..Options::scale()
    }
}

fn count(account: &Account, sql: &str) -> i64 {
    account
        .store
        .query_row(sql, [], |r| r.get(0))
        .expect("count")
}

#[test]
fn the_corpus_is_a_container_a_launch_opens_with_the_shape_it_was_asked_for() {
    let scratch = Scratch::new("shape");
    let keys = InMemory::default();
    let options = small();
    let mut pages = 0;
    let report = generate(&scratch.0, &keys, &options, &mut |_| pages += 1).expect("generate");

    assert_eq!(report.messages(), 500);
    assert_eq!(report.accounts.len(), 5);
    assert_eq!(report.accounts[0].inbox, 50);
    assert!(pages >= 5, "progress was never reported");
    assert!(report.index_token_bytes > 0, "no envelope was tokenized");

    // Read it back the way `App::open_container` does: the registry, then each account's
    // sealed files under the key the credential store holds for it.
    let container = Container::open(&scratch.0, &keys).expect("reopen the container");
    let registered = container.accounts().expect("accounts");
    assert_eq!(registered.len(), 5);
    let mut total = 0;
    for (row, written) in registered.iter().zip(&report.accounts) {
        assert_eq!(row.kind, KIND);
        assert_eq!(row.id, written.id);
        let owner = sift_app::container::account_secret(&keys, row.id).expect("key");
        let account =
            Account::open_sealed(&AccountPaths::under(&scratch.0, row.id), row.id, &owner)
                .expect("the account's files authenticate under its key");

        let messages = count(&account, "SELECT count(*) FROM message");
        assert_eq!(u64::try_from(messages).expect("count"), written.messages);
        total += messages;

        let inbox = count(
            &account,
            "SELECT count(*) FROM message_location l JOIN folder f ON f.id = l.folder_id
             WHERE f.special_use = 'Inbox'",
        );
        assert_eq!(u64::try_from(inbox).expect("count"), written.inbox);

        // Synchronized and quiescent: every folder has a cursor and is Live.
        let folders = count(&account, "SELECT count(*) FROM folder");
        let live = count(
            &account,
            "SELECT count(*) FROM folder_sync_state WHERE state = 'Live' AND cursor IS NOT NULL",
        );
        assert_eq!(folders, live, "a folder was left mid-backfill");
        assert_eq!(
            count(&account, "SELECT count(*) FROM folder WHERE watched = 0"),
            0,
            "a filled folder is outside the watched set"
        );

        // Ingest wrote what ingest writes: threads, and the account row a launch reads its
        // capability set from.
        assert!(count(&account, "SELECT count(*) FROM thread") > 0);
        assert_eq!(
            count(
                &account,
                "SELECT count(*) FROM message WHERE thread_id IS NULL OR subject IS NULL"
            ),
            0
        );
        let shape: Vec<u8> = account
            .store
            .query_row("SELECT capabilities FROM account", [], |r| r.get(0))
            .expect("account row");
        assert_eq!(shape, KIND.as_bytes());
    }
    assert_eq!(total, 500);
}

#[test]
fn a_container_that_already_holds_accounts_is_refused_and_left_alone() {
    let scratch = Scratch::new("refuse");
    let keys = InMemory::default();
    generate(&scratch.0, &keys, &small(), &mut |_| {}).expect("first");
    let before = Container::open(&scratch.0, &keys)
        .and_then(|c| c.accounts())
        .expect("accounts")
        .len();
    let second = generate(&scratch.0, &keys, &small(), &mut |_| {});
    assert!(
        matches!(second, Err(CorpusError::NotEmpty { accounts: 5 })),
        "{second:?}"
    );
    let after = Container::open(&scratch.0, &keys)
        .and_then(|c| c.accounts())
        .expect("accounts")
        .len();
    assert_eq!(before, after, "a refused run still registered accounts");
}

#[test]
fn one_seed_writes_the_same_mail_twice() {
    let keys_a = InMemory::default();
    let keys_b = InMemory::default();
    let a = Scratch::new("seed-a");
    let b = Scratch::new("seed-b");
    let options = Options {
        shape: Shape {
            accounts: 2,
            messages: 300,
            inbox: 40,
        },
        ..Options::scale()
    };
    let ra = generate(&a.0, &keys_a, &options, &mut |_| {}).expect("a");
    let rb = generate(&b.0, &keys_b, &options, &mut |_| {}).expect("b");
    assert_eq!(ra.index_token_bytes, rb.index_token_bytes);

    let content = |root: &PathBuf, keys: &InMemory, id: AccountId| -> Vec<(String, String, i64)> {
        let owner = sift_app::container::account_secret(keys, id).expect("key");
        let account =
            Account::open_sealed(&AccountPaths::under(root, id), id, &owner).expect("open");
        let mut stmt = account
            .store
            .prepare(
                "SELECT remote_id, subject, received_at_millis FROM message ORDER BY remote_id",
            )
            .expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    };
    for (x, y) in ra.accounts.iter().zip(&rb.accounts) {
        assert_eq!(content(&a.0, &keys_a, x.id), content(&b.0, &keys_b, y.id));
    }
}

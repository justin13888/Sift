//! The delta path, end to end, against the real schema.
//!
//! The adapter here is synthetic and deliberately nameless. D-12 is one reason — nothing
//! above the adapter layer may name a provider, and this crate is above it — but the better
//! one is that these are assertions about **the rules**, not about anybody's protocol: the
//! one-transaction rule, retirement rather than deletion, provenance that is written once.
//! An adapter that got any of them right by accident would still be tested here.

use sift_foundation::identity::{AccountId, AccountOrdinal, LocalIdGenerator};
use sift_provider::adapter::{
    Adapter, Change, Cursor, Delta, Envelope, Failure, MutationOutcome, Provenance, RemoteFolder,
    RemoteFolderId, RemoteMessageId, SpecialUse, WireMutation,
};
use sift_provider::capability::Capabilities;
use sift_store::account::{Account, AccountPaths};
use sift_sync::ingest;
use sift_sync::run::{self, Turn};
use std::cell::RefCell;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// A scratch account, on disk, through the real schema.
// ---------------------------------------------------------------------------

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "sift-ingest-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }

    fn open(&self) -> Account {
        let id = AccountId::from_u128(1);
        let account = Account::open(&AccountPaths::under(&self.0, id), id).expect("open");
        account
            .store
            .execute(
                "INSERT OR IGNORE INTO account (id, ordinal, display_name, capabilities)
                 VALUES (?1, 0, 'scratch', x'')",
                rusqlite::params![id.as_u128().to_be_bytes().to_vec()],
            )
            .expect("account row");
        account
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ids() -> LocalIdGenerator {
    LocalIdGenerator::new(AccountOrdinal::new(0))
}

// ---------------------------------------------------------------------------
// A scripted adapter. It answers from a list and records what it was asked.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Scripted {
    capabilities: Capabilities,
    folders: Vec<RemoteFolder>,
    pages: RefCell<Vec<Result<Delta, &'static str>>>,
    envelopes: Vec<Envelope>,
    asked_for: RefCell<Vec<RemoteMessageId>>,
    cursors_seen: RefCell<Vec<Option<Cursor>>>,
}

impl Scripted {
    fn new(folders: Vec<RemoteFolder>) -> Self {
        Self {
            capabilities: shape(),
            folders,
            pages: RefCell::new(Vec::new()),
            envelopes: Vec::new(),
            asked_for: RefCell::new(Vec::new()),
            cursors_seen: RefCell::new(Vec::new()),
        }
    }

    fn answering(mut self, pages: Vec<Result<Delta, &'static str>>, envelopes: Vec<Envelope>) -> Self {
        self.pages = RefCell::new(pages);
        self.envelopes = envelopes;
        self
    }
}

fn shape() -> Capabilities {
    use sift_provider::capability::{
        ArchiveSemantics, DeltaMechanism, IdStability, JunkReporting, LocationCardinality, Magnitude,
        PushMechanism, SnippetSource, TagSupport, ThreadOperations, TrashSemantics,
    };
    Capabilities {
        location_cardinality: LocationCardinality::OneOrMore,
        tag_support: TagSupport::ReadWrite,
        archive: ArchiveSemantics::RemoveFromInbox,
        trash: TrashSemantics::MoveToTrash,
        permanent_delete: false,
        thread_operations: ThreadOperations::Native,
        junk_reporting: JunkReporting::NativeReport,
        delta: DeltaMechanism::HistoryCursor,
        push: PushMechanism::PollOnly,
        id_stability: IdStability::StableGlobally,
        server_search: true,
        max_batch_size: Magnitude::Unknown,
        request_budget: Magnitude::Unknown,
        snippet_source: SnippetSource::ProviderSupplied,
        unrecognised: Vec::new(),
    }
}

impl Adapter for Scripted {
    type Error = &'static str;

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Self::Error> {
        Ok(self.folders.clone())
    }

    fn delta(
        &self,
        _folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, Self::Error> {
        self.cursors_seen.borrow_mut().push(cursor.cloned());
        let mut pages = self.pages.borrow_mut();
        if pages.is_empty() {
            return Err("no page scripted");
        }
        pages.remove(0)
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error> {
        self.asked_for.borrow_mut().extend_from_slice(ids);
        Ok(self
            .envelopes
            .iter()
            .filter(|e| ids.contains(&e.id))
            .cloned()
            .collect())
    }

    fn fetch_part(&self, _id: &RemoteMessageId, _part: &str) -> Result<Vec<u8>, Self::Error> {
        Ok(Vec::new())
    }

    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error> {
        Ok(vec![MutationOutcome::Applied; batch.len()])
    }

    fn watch(&self, _folders: &[RemoteFolderId]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn classify(error: &Self::Error) -> Failure {
        match *error {
            "cursor" => Failure::CursorInvalidated,
            "gone" => Failure::Permanent,
            _ => Failure::Transient,
        }
    }
}

fn inbox() -> RemoteFolder {
    RemoteFolder {
        id: RemoteFolderId("INBOX".into()),
        display_name: "Inbox".into(),
        special_use: Some(SpecialUse::Inbox),
    }
}

fn archive() -> RemoteFolder {
    RemoteFolder {
        id: RemoteFolderId("ARCH".into()),
        display_name: "Archive".into(),
        special_use: Some(SpecialUse::Archive),
    }
}

fn envelope(id: &str, subject: &str, received: u64) -> Envelope {
    Envelope {
        id: RemoteMessageId(id.into()),
        thread_id: Some(format!("t-{id}")),
        internet_message_id: Some(format!("<{id}@x.test>")),
        subject: Some(subject.into()),
        from: Some("someone@example.test".into()),
        to: vec!["me@example.test".into()],
        received_at_millis: received,
        origination_date_millis: Some(received),
        snippet: Some("a snippet".into()),
        folders: vec![RemoteFolderId("INBOX".into())],
        ..Envelope::default()
    }
}

fn page(changes: Vec<Change>, cursor: &str, more: bool) -> Result<Delta, &'static str> {
    Ok(Delta {
        changes,
        next: Cursor(cursor.as_bytes().to_vec()),
        more,
    })
}

fn present(id: &str, provenance: Provenance) -> Change {
    Change::Present {
        id: RemoteMessageId(id.into()),
        provenance,
    }
}

// ---------------------------------------------------------------------------
// Folders — D-83.
// ---------------------------------------------------------------------------

#[test]
fn the_inbox_is_watched_on_discovery_and_the_others_are_not() {
    // FR-43: a discovered folder is discovered, not adopted. The inbox is the exception,
    // because an account whose inbox is not watched is an account that shows nothing.
    let s = Scratch::new("watched");
    let account = s.open();
    let report = ingest::reconcile_folders(&account.store, &[inbox(), archive()]).unwrap();
    assert_eq!(report.discovered.len(), 2);
    let watched = ingest::watched_folders(&account.store).unwrap();
    assert_eq!(watched.len(), 1);
    assert_eq!(watched[0].1.0, "INBOX");
}

#[test]
fn a_folder_that_stops_appearing_is_retired_and_its_messages_stay_reachable() {
    // D-83. The difference between a folder disappearing from a sidebar and a user's mail
    // disappearing.
    let s = Scratch::new("retire");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox(), archive()]).unwrap();
    let arch = ingest::folder_local_id(&account.store, &RemoteFolderId("ARCH".into())).unwrap();
    ingest::set_watched(&account.store, arch, true).unwrap();

    let ids = ids();
    let delta = page(vec![present("m1", Provenance::Discovered)], "c1", false).unwrap();
    ingest::apply_page(
        &mut account.store,
        arch,
        &delta,
        &[envelope("m1", "kept", 100)],
        &ids,
    )
    .unwrap();

    let report = ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    assert_eq!(report.retired, vec!["ARCH".to_owned()]);

    let held: i64 = account
        .store
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 1, "retiring a folder deleted its messages");
}

#[test]
fn a_folder_that_comes_back_keeps_the_cursor_it_had() {
    // Deleting and recreating would restart a completed backfill.
    let s = Scratch::new("unretire");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox(), archive()]).unwrap();
    let arch = ingest::folder_local_id(&account.store, &RemoteFolderId("ARCH".into())).unwrap();
    ingest::apply_page(
        &mut account.store,
        arch,
        &page(vec![], "c-kept", false).unwrap(),
        &[],
        &ids(),
    )
    .unwrap();

    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    ingest::reconcile_folders(&account.store, &[inbox(), archive()]).unwrap();

    let same = ingest::folder_local_id(&account.store, &RemoteFolderId("ARCH".into())).unwrap();
    assert_eq!(same, arch, "the folder was recreated rather than un-retired");
    assert_eq!(
        ingest::cursor_of(&account.store, arch).unwrap(),
        Some(Cursor(b"c-kept".to_vec()))
    );
}

#[test]
fn a_display_name_is_normalized_before_it_is_stored() {
    // NFR-54. A folder name is a provider-controlled string like any other, and a right-to-
    // left override in one would reorder the sidebar around it.
    let s = Scratch::new("normalized-folder");
    let account = s.open();
    ingest::reconcile_folders(
        &account.store,
        &[RemoteFolder {
            id: RemoteFolderId("F".into()),
            display_name: "Rece\u{202e}ipts".into(),
            special_use: None,
        }],
    )
    .unwrap();
    let name: String = account
        .store
        .query_row(
            "SELECT display_name FROM folder WHERE remote_id = 'F'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(name, "Rece\u{202e}ipts", "the override reached the store raw");
}

// ---------------------------------------------------------------------------
// The one-transaction rule — D-82.
// ---------------------------------------------------------------------------

#[test]
fn a_page_and_its_cursor_land_together() {
    let s = Scratch::new("one-transaction");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();

    let report = ingest::apply_page(
        &mut account.store,
        folder,
        &page(
            vec![present("m1", Provenance::Delivered), present("m2", Provenance::Delivered)],
            "c1",
            false,
        )
        .unwrap(),
        &[envelope("m1", "one", 100), envelope("m2", "two", 200)],
        &ids(),
    )
    .unwrap();

    assert_eq!(report.inserted, 2);
    assert_eq!(report.delivered, 2);
    assert_eq!(
        ingest::cursor_of(&account.store, folder).unwrap(),
        Some(Cursor(b"c1".to_vec()))
    );
}

#[test]
fn a_page_that_cannot_be_written_leaves_the_cursor_where_it_was() {
    // The property the transaction exists for. A cursor past unapplied change is silent
    // data loss, and silent in the worst way: the messages were never seen, so nothing
    // afterwards is inconsistent and no repair pass could find it.
    let s = Scratch::new("atomic");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    ingest::apply_page(
        &mut account.store,
        folder,
        &page(vec![], "c-first", false).unwrap(),
        &[],
        &ids(),
    )
    .unwrap();

    // A page naming a folder that does not exist. The message row is written *first* and
    // the location that references the folder fails after it — so if the transaction were
    // not doing its job, a message would be left behind with no cursor describing it.
    let outcome = ingest::apply_page(
        &mut account.store,
        99_999,
        &page(vec![present("m1", Provenance::Delivered)], "c-second", false).unwrap(),
        &[envelope("m1", "one", 100)],
        &ids(),
    );
    assert!(outcome.is_err());

    let held: i64 = account
        .store
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 0, "a row written before the failure survived it");
    assert_eq!(
        ingest::cursor_of(&account.store, folder).unwrap(),
        Some(Cursor(b"c-first".to_vec())),
        "the cursor advanced past change that was not applied"
    );
    assert_eq!(
        ingest::cursor_of(&account.store, 99_999).unwrap(),
        None,
        "a cursor was written for a page that did not apply"
    );
}

#[test]
fn reapplying_a_page_changes_nothing_the_second_time() {
    // D-82 leans on this and records it as its weak point: cursor-first makes early deltas
    // redundant rather than lost, which is only safe because reapplication is safe.
    let s = Scratch::new("idempotent");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    let delta = page(vec![present("m1", Provenance::Delivered)], "c1", false).unwrap();
    let envelopes = [envelope("m1", "one", 100)];

    ingest::apply_page(&mut account.store, folder, &delta, &envelopes, &ids).unwrap();
    let second = ingest::apply_page(&mut account.store, folder, &delta, &envelopes, &ids).unwrap();

    assert_eq!(second.inserted, 0);
    assert_eq!(second.updated, 1);
    let held: i64 = account
        .store
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 1, "reapplying a page duplicated a message");
}

// ---------------------------------------------------------------------------
// Provenance — FR-23's definition, recorded at the only moment it can be.
// ---------------------------------------------------------------------------

#[test]
fn a_message_keeps_the_provenance_it_arrived_with() {
    // A later discovery of a message already held is not a second arrival, and rewriting it
    // would make a re-walk announce mail the user read last week.
    let s = Scratch::new("provenance");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();

    ingest::apply_page(
        &mut account.store,
        folder,
        &page(vec![present("m1", Provenance::Delivered)], "c1", false).unwrap(),
        &[envelope("m1", "one", 100)],
        &ids,
    )
    .unwrap();
    let report = ingest::apply_page(
        &mut account.store,
        folder,
        &page(vec![present("m1", Provenance::Discovered)], "c2", false).unwrap(),
        &[envelope("m1", "one", 100)],
        &ids,
    )
    .unwrap();
    assert_eq!(report.delivered, 0);

    let provenance: String = account
        .store
        .query_row("SELECT provenance FROM message WHERE remote_id = 'm1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(provenance, "Delivered");
}

#[test]
fn a_backfill_never_counts_as_an_arrival() {
    let s = Scratch::new("backfill-provenance");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let report = ingest::apply_page(
        &mut account.store,
        folder,
        &page(
            vec![present("m1", Provenance::Discovered), present("m2", Provenance::Discovered)],
            "c1",
            false,
        )
        .unwrap(),
        &[envelope("m1", "one", 100), envelope("m2", "two", 200)],
        &ids(),
    )
    .unwrap();
    assert_eq!(report.inserted, 2);
    assert_eq!(report.delivered, 0, "a backfill announced the whole mailbox");
}

// ---------------------------------------------------------------------------
// Removal — a message in two places does not vanish from one.
// ---------------------------------------------------------------------------

#[test]
fn leaving_one_folder_of_two_is_an_archive_rather_than_a_disappearance() {
    let s = Scratch::new("two-places");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox(), archive()]).unwrap();
    let in_id = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let arch = ingest::folder_local_id(&account.store, &RemoteFolderId("ARCH".into())).unwrap();
    let ids = ids();

    for folder in [in_id, arch] {
        ingest::apply_page(
            &mut account.store,
            folder,
            &page(vec![present("m1", Provenance::Discovered)], "c1", false).unwrap(),
            &[envelope("m1", "one", 100)],
            &ids,
        )
        .unwrap();
    }

    ingest::apply_page(
        &mut account.store,
        in_id,
        &page(
            vec![Change::Removed {
                id: RemoteMessageId("m1".into()),
            }],
            "c2",
            false,
        )
        .unwrap(),
        &[],
        &ids,
    )
    .unwrap();

    let held: i64 = account
        .store
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 1, "a message still in another folder was deleted");
    let locations: i64 = account
        .store
        .query_row("SELECT count(*) FROM message_location", [], |r| r.get(0))
        .unwrap();
    assert_eq!(locations, 1);
}

#[test]
fn leaving_the_last_folder_removes_the_message() {
    let s = Scratch::new("last-place");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    ingest::apply_page(
        &mut account.store,
        folder,
        &page(vec![present("m1", Provenance::Discovered)], "c1", false).unwrap(),
        &[envelope("m1", "one", 100)],
        &ids,
    )
    .unwrap();
    ingest::apply_page(
        &mut account.store,
        folder,
        &page(
            vec![Change::Removed {
                id: RemoteMessageId("m1".into()),
            }],
            "c2",
            false,
        )
        .unwrap(),
        &[],
        &ids,
    )
    .unwrap();
    let held: i64 = account
        .store
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 0);
}

#[test]
fn removing_a_message_this_account_never_held_is_not_an_error() {
    // Ordinary during a recovery, and during a re-walk that overlaps a delta.
    let s = Scratch::new("remove-unknown");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let report = ingest::apply_page(
        &mut account.store,
        folder,
        &page(
            vec![Change::Removed {
                id: RemoteMessageId("never-seen".into()),
            }],
            "c1",
            false,
        )
        .unwrap(),
        &[],
        &ids(),
    )
    .unwrap();
    assert_eq!(report.removed, 0);
}

// ---------------------------------------------------------------------------
// The driver.
// ---------------------------------------------------------------------------

#[test]
fn a_first_sync_asks_with_no_cursor_and_then_with_the_one_it_was_given() {
    let s = Scratch::new("driver-cursor");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(
        vec![
            page(vec![present("m1", Provenance::Discovered)], "c1", true),
            page(vec![present("m2", Provenance::Discovered)], "c2", false),
        ],
        vec![envelope("m1", "one", 100), envelope("m2", "two", 200)],
    );
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();

    let report = run::sync_folder(
        &adapter,
        &mut account,
        folder,
        &RemoteFolderId("INBOX".into()),
        &ids(),
        10,
    )
    .unwrap();
    assert_eq!(report.inserted, 2);

    let seen = adapter.cursors_seen.borrow();
    assert_eq!(seen[0], None, "the first turn presented a cursor");
    assert_eq!(seen[1], Some(Cursor(b"c1".to_vec())));
}

#[test]
fn only_messages_this_account_does_not_hold_are_asked_about() {
    // A resumed backfill that refetched every envelope would turn a resume into a full
    // refetch, which is what L-26's page size is meant to avoid.
    let s = Scratch::new("driver-fetch");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(
        vec![
            page(vec![present("m1", Provenance::Discovered)], "c1", false),
            page(
                vec![
                    present("m1", Provenance::Discovered),
                    present("m2", Provenance::Discovered),
                ],
                "c2",
                false,
            ),
        ],
        vec![envelope("m1", "one", 100), envelope("m2", "two", 200)],
    );
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    let remote = RemoteFolderId("INBOX".into());

    run::sync_folder(&adapter, &mut account, folder, &remote, &ids, 1).unwrap();
    run::sync_folder(&adapter, &mut account, folder, &remote, &ids, 1).unwrap();

    let asked: Vec<String> = adapter
        .asked_for
        .borrow()
        .iter()
        .map(|i| i.0.clone())
        .collect();
    assert_eq!(asked, vec!["m1".to_owned(), "m2".to_owned()]);
}

#[test]
fn a_flag_change_is_asked_about_even_though_the_message_is_already_held() {
    let s = Scratch::new("driver-flags");
    let mut account = s.open();
    let mut read = envelope("m1", "one", 100);
    read.read = true;
    let adapter = Scripted::new(vec![inbox()]).answering(
        vec![
            page(vec![present("m1", Provenance::Discovered)], "c1", false),
            page(
                vec![Change::FlagsChanged {
                    id: RemoteMessageId("m1".into()),
                }],
                "c2",
                false,
            ),
        ],
        vec![read],
    );
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    let remote = RemoteFolderId("INBOX".into());
    run::sync_folder(&adapter, &mut account, folder, &remote, &ids, 1).unwrap();
    run::sync_folder(&adapter, &mut account, folder, &remote, &ids, 1).unwrap();

    let flags: i64 = account
        .store
        .query_row("SELECT flags FROM message WHERE remote_id = 'm1'", [], |r| r.get(0))
        .unwrap();
    assert!(sift_store::flags::is_read(flags));
    assert_eq!(adapter.asked_for.borrow().len(), 2);
}

#[test]
fn an_invalidated_cursor_is_recovery_and_the_next_turn_takes_a_fresh_one() {
    // D-82: `Invalidated` is not a resting state. NFR-18 forbids requiring the user to ask.
    let s = Scratch::new("driver-invalidated");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(
        vec![
            page(vec![present("m1", Provenance::Discovered)], "c1", false),
            Err("cursor"),
        ],
        vec![envelope("m1", "one", 100)],
    );
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    let remote = RemoteFolderId("INBOX".into());

    run::sync_folder(&adapter, &mut account, folder, &remote, &ids, 1).unwrap();
    assert_eq!(
        run::sync_one_page(&adapter, &mut account, folder, &remote, &ids).unwrap(),
        Turn::Invalidated
    );
    assert_eq!(
        ingest::state_of(&account.store, folder).unwrap().as_deref(),
        Some("Invalidated")
    );
    run::clear_cursor(&account, folder).unwrap();
    assert_eq!(ingest::cursor_of(&account.store, folder).unwrap(), None);
    assert_eq!(
        ingest::state_of(&account.store, folder).unwrap().as_deref(),
        Some("Recovering"),
        "recovery is progress, not a fault"
    );
}

#[test]
fn a_settled_failure_is_recorded_with_the_reason_nfr29_requires() {
    let s = Scratch::new("driver-degraded");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(vec![Err("gone")], vec![]);
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let outcome = run::sync_one_page(
        &adapter,
        &mut account,
        folder,
        &RemoteFolderId("INBOX".into()),
        &ids(),
    );
    assert!(outcome.is_err());
    let reason: Option<String> = account
        .store
        .query_row(
            "SELECT degraded_reason FROM folder_sync_state WHERE folder_id = ?1",
            rusqlite::params![folder],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reason.as_deref(), Some("gone"));
}

#[test]
fn a_transient_failure_does_not_degrade_the_folder() {
    // The captive-portal case, reaching the folder state machine instead of the credential
    // store. An unparseable answer says nothing about the account.
    let s = Scratch::new("driver-transient");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(vec![Err("something else")], vec![]);
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let _ = run::sync_one_page(
        &adapter,
        &mut account,
        folder,
        &RemoteFolderId("INBOX".into()),
        &ids(),
    );
    assert_ne!(
        ingest::state_of(&account.store, folder).unwrap().as_deref(),
        Some("Degraded")
    );
}

#[test]
fn a_thread_is_joined_on_the_providers_conversation_and_keeps_one_identity() {
    // FR-11 prefers the provider's own identifier, and D-103 makes the identity local:
    // threads merge with the older identity surviving, and never split.
    let s = Scratch::new("threads");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let mut first = envelope("m1", "one", 100);
    let mut second = envelope("m2", "two", 200);
    first.thread_id = Some("conversation".into());
    second.thread_id = Some("conversation".into());

    ingest::apply_page(
        &mut account.store,
        folder,
        &page(
            vec![present("m1", Provenance::Delivered), present("m2", Provenance::Delivered)],
            "c1",
            false,
        )
        .unwrap(),
        &[first, second],
        &ids(),
    )
    .unwrap();

    let threads: i64 = account
        .store
        .query_row("SELECT count(*) FROM thread", [], |r| r.get(0))
        .unwrap();
    assert_eq!(threads, 1, "one conversation became two threads");
    let distinct: i64 = account
        .store
        .query_row("SELECT count(DISTINCT thread_id) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(distinct, 1);
}

#[test]
fn a_subject_is_normalized_once_before_it_is_stored() {
    // NFR-54, and the coupling D-81 requires: the store's copy and the index's copy come
    // from the same call rather than from two.
    let s = Scratch::new("normalized-subject");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let mut hostile = envelope("m1", "Invoice\u{202e}fdp.exe", 100);
    hostile.subject = Some("Invoice\u{202e}fdp.exe".into());
    ingest::apply_page(
        &mut account.store,
        folder,
        &page(vec![present("m1", Provenance::Delivered)], "c1", false).unwrap(),
        &[hostile],
        &ids(),
    )
    .unwrap();
    let subject: String = account
        .store
        .query_row("SELECT subject FROM message WHERE remote_id = 'm1'", [], |r| r.get(0))
        .unwrap();
    assert_ne!(
        subject, "Invoice\u{202e}fdp.exe",
        "a right-to-left override reached the store raw"
    );
}

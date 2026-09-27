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
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

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
    /// Set while a [`Watched`] hold is inside a step. D-122: no round trip may start then.
    inside: Rc<Cell<bool>>,
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
            inside: Rc::new(Cell::new(false)),
        }
    }

    /// Refuse, loudly, any round trip made while the store is held.
    fn outside(&self, call: &str) {
        assert!(
            !self.inside.get(),
            "{call} reached the provider while the account was held"
        );
    }

    fn answering(
        mut self,
        pages: Vec<Result<Delta, &'static str>>,
        envelopes: Vec<Envelope>,
    ) -> Self {
        self.pages = RefCell::new(pages);
        self.envelopes = envelopes;
        self
    }
}

fn shape() -> Capabilities {
    use sift_provider::capability::{
        ArchiveSemantics, DeltaMechanism, IdStability, JunkReporting, LocationCardinality,
        Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations, TrashSemantics,
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
        server_search: sift_provider::capability::ServerSearch::ALL,
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
        self.outside("enumerate_folders");
        Ok(self.folders.clone())
    }

    fn delta(
        &self,
        _folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, Self::Error> {
        self.outside("delta");
        self.cursors_seen.borrow_mut().push(cursor.cloned());
        let mut pages = self.pages.borrow_mut();
        if pages.is_empty() {
            return Err("no page scripted");
        }
        pages.remove(0)
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error> {
        self.outside("fetch_envelopes");
        self.asked_for.borrow_mut().extend_from_slice(ids);
        Ok(self
            .envelopes
            .iter()
            .filter(|e| ids.contains(&e.id))
            .cloned()
            .collect())
    }

    fn structure(
        &self,
        _id: &RemoteMessageId,
    ) -> Result<Vec<sift_provider::adapter::PartDescriptor>, Self::Error> {
        Ok(Vec::new())
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

    fn search(
        &self,
        _terms: &[sift_provider::adapter::SearchTerm],
        _limit: usize,
    ) -> Result<Vec<RemoteMessageId>, Self::Error> {
        Ok(Vec::new())
    }

    fn classify(&self, error: &Self::Error) -> Failure {
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
    assert_eq!(
        same, arch,
        "the folder was recreated rather than un-retired"
    );
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
    assert_ne!(
        name, "Rece\u{202e}ipts",
        "the override reached the store raw"
    );
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
            vec![
                present("m1", Provenance::Delivered),
                present("m2", Provenance::Delivered),
            ],
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
    // FR-23's rules are per folder, so each arrival says which folder it arrived in.
    assert_eq!(report.arrivals.len(), report.delivered);
    assert!(report.arrivals.iter().all(|a| a.folder == folder));
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
        &page(
            vec![present("m1", Provenance::Delivered)],
            "c-second",
            false,
        )
        .unwrap(),
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
        .query_row(
            "SELECT provenance FROM message WHERE remote_id = 'm1'",
            [],
            |r| r.get(0),
        )
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
            vec![
                present("m1", Provenance::Discovered),
                present("m2", Provenance::Discovered),
            ],
            "c1",
            false,
        )
        .unwrap(),
        &[envelope("m1", "one", 100), envelope("m2", "two", 200)],
        &ids(),
    )
    .unwrap();
    assert_eq!(report.inserted, 2);
    assert_eq!(
        report.delivered, 0,
        "a backfill announced the whole mailbox"
    );
    assert_eq!(report.newest, None);
}

#[test]
fn only_an_arrival_that_was_unread_is_new_mail_and_the_newest_is_named() {
    // FR-23: delivered **and unread at that moment**. A message read on another device before
    // this turn reached it arrived, and is still not something to announce; one whose envelope
    // never came back has nothing to say about whether it was read, or anything else.
    let s = Scratch::new("unread-arrivals");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let mut read = envelope("m-read", "already read", 900);
    read.read = true;
    let report = ingest::apply_page(
        &mut account.store,
        folder,
        &page(
            vec![
                present("m-old", Provenance::Delivered),
                present("m-new", Provenance::Delivered),
                present("m-read", Provenance::Delivered),
                present("m-bare", Provenance::Delivered),
            ],
            "c1",
            false,
        )
        .unwrap(),
        &[
            envelope("m-old", "older", 100),
            envelope("m-new", "newer", 300),
            read,
        ],
        &ids(),
    )
    .unwrap();

    assert_eq!(report.inserted, 4);
    assert_eq!(
        report.delivered, 2,
        "a read or envelope-less arrival was counted"
    );
    let newest = report.newest.expect("two arrivals and no newest");
    assert_eq!(newest.received_millis, 300);
    let named: String = account
        .store
        .query_row(
            "SELECT remote_id FROM message WHERE id = ?1",
            [newest.id.to_bytes().to_vec()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        named, "m-new",
        "the notification would open the wrong message"
    );
}

#[test]
fn absorbing_reports_keeps_the_newest_arrival_across_them() {
    // Summed per folder and again per account: the newest across both is what a notification
    // names, and an absorb that kept the first one it saw would name whichever folder came first.
    use sift_foundation::identity::LocalId;
    let arrival = |n: u128, at: u64| ingest::Arrival {
        id: LocalId::from_u128(n),
        received_millis: at,
        folder: 1,
    };
    let mut total = ingest::PageReport::default();
    let first = ingest::PageReport {
        inserted: 1,
        delivered: 1,
        newest: Some(arrival(1, 500)),
        arrivals: vec![arrival(1, 500)],
        ..Default::default()
    };
    let second = ingest::PageReport {
        inserted: 2,
        delivered: 1,
        newest: Some(arrival(2, 200)),
        arrivals: vec![arrival(2, 200)],
        ..Default::default()
    };
    total.absorb(&first);
    total.absorb(&second);
    total.absorb(&ingest::PageReport::default());
    assert_eq!(total.inserted, 3);
    assert_eq!(total.delivered, 2);
    assert_eq!(total.newest, Some(arrival(1, 500)));
    assert_eq!(total.arrivals, vec![arrival(1, 500), arrival(2, 200)]);
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
        .query_row(
            "SELECT flags FROM message WHERE remote_id = 'm1'",
            [],
            |r| r.get(0),
        )
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

// ---------------------------------------------------------------------------
// D-122: the store is held only across store work.
// ---------------------------------------------------------------------------

/// A hold that marks when it is inside a step, and can lose the account after some number of
/// them — the removal a person can make between two held steps.
struct Watched<'a> {
    account: &'a mut Account,
    ids: &'a LocalIdGenerator,
    inside: Rc<Cell<bool>>,
    steps: usize,
    gone_after: usize,
}

impl run::Hold for Watched<'_> {
    fn with<R>(&mut self, step: impl FnOnce(&mut Account, &LocalIdGenerator) -> R) -> Option<R> {
        if self.steps >= self.gone_after {
            return None;
        }
        self.steps += 1;
        self.inside.set(true);
        let out = step(self.account, self.ids);
        self.inside.set(false);
        Some(out)
    }
}

#[test]
fn no_round_trip_is_made_while_the_account_is_held() {
    let s = Scratch::new("held-round-trips");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(
        vec![
            page(vec![present("m1", Provenance::Discovered)], "c1", true),
            page(vec![present("m2", Provenance::Discovered)], "c2", false),
        ],
        vec![envelope("m1", "one", 100), envelope("m2", "two", 200)],
    );
    let ids = ids();
    let mut hold = Watched {
        account: &mut account,
        ids: &ids,
        inside: Rc::clone(&adapter.inside),
        steps: 0,
        gone_after: usize::MAX,
    };

    run::discover_folders_held(&adapter, &mut hold).unwrap();
    let report = run::sync_account_held(&adapter, &mut hold, 10).unwrap();
    assert_eq!(report.inserted, 2, "the walk did its work through the hold");
    // Folders; the watched list; then cursor, known, apply for each of two pages.
    assert_eq!(hold.steps, 8, "a round trip was folded into a held step");
    assert_eq!(
        ingest::cursor_of(&account.store, 1).unwrap(),
        Some(Cursor(b"c2".to_vec()))
    );
}

#[test]
fn an_account_removed_between_the_round_trip_and_the_write_is_written_nothing() {
    let s = Scratch::new("held-gone");
    let mut account = s.open();
    let adapter = Scripted::new(vec![inbox()]).answering(
        vec![page(
            vec![present("m1", Provenance::Discovered)],
            "c1",
            false,
        )],
        vec![envelope("m1", "one", 100)],
    );
    run::discover_folders(&adapter, &account).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    // The cursor and what is known are read; the account is gone by the time the page is.
    let mut hold = Watched {
        account: &mut account,
        ids: &ids,
        inside: Rc::clone(&adapter.inside),
        steps: 0,
        gone_after: 2,
    };

    let outcome =
        run::sync_one_page_held(&adapter, &mut hold, folder, &RemoteFolderId("INBOX".into()));
    assert!(matches!(outcome, Err(run::RunError::Gone)), "{outcome:?}");
    assert_eq!(
        adapter.asked_for.borrow().len(),
        1,
        "the round trip before the removal was made"
    );
    assert_eq!(
        ingest::cursor_of(&account.store, folder).unwrap(),
        None,
        "a page was written for an account that was gone"
    );
    let rows: i64 = account
        .store
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
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
            vec![
                present("m1", Provenance::Delivered),
                present("m2", Provenance::Delivered),
            ],
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
        .query_row("SELECT count(DISTINCT thread_id) FROM message", [], |r| {
            r.get(0)
        })
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
        .query_row(
            "SELECT subject FROM message WHERE remote_id = 'm1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(
        subject, "Invoice\u{202e}fdp.exe",
        "a right-to-left override reached the store raw"
    );
}

// ---------------------------------------------------------------------------
// What a server-side search found — FR-21.
// ---------------------------------------------------------------------------

#[test]
fn a_found_message_is_discovered_moves_no_cursor_and_keeps_an_identity_already_held() {
    let s = Scratch::new("found");
    let mut account = s.open();
    ingest::reconcile_folders(&account.store, &[inbox(), archive()]).unwrap();
    let folder = ingest::folder_local_id(&account.store, &RemoteFolderId("INBOX".into())).unwrap();
    let ids = ids();
    ingest::apply_page(
        &mut account.store,
        folder,
        &page(vec![present("m1", Provenance::Delivered)], "c1", false).unwrap(),
        &[envelope("m1", "held", 100)],
        &ids,
    )
    .unwrap();
    let held: Vec<u8> = account
        .store
        .query_row("SELECT id FROM message WHERE remote_id = 'm1'", [], |r| {
            r.get(0)
        })
        .unwrap();

    // One held, one in two folders, one in a folder nobody enumerated.
    let mut two = envelope("m2", "in two places", 200);
    two.folders = vec![
        RemoteFolderId("INBOX".into()),
        RemoteFolderId("ARCH".into()),
    ];
    let mut nowhere = envelope("m3", "nowhere known", 300);
    nowhere.folders = vec![RemoteFolderId("UNSEEN-FOLDER".into())];
    let found = ingest::ingest_found(
        &mut account.store,
        &[envelope("m1", "held", 100), two, nowhere],
        &ids,
    )
    .unwrap();

    assert_eq!(found.len(), 3);
    assert_eq!(
        found[0].1.to_bytes().to_vec(),
        held,
        "a message already held was given a second identity"
    );
    let count = |sql: &str| -> i64 { account.store.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT count(*) FROM message"), 3);
    assert_eq!(
        count(
            "SELECT count(*) FROM message_location l JOIN message m ON m.id = l.message_id
             WHERE m.remote_id = 'm2'"
        ),
        2
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM message_location l JOIN message m ON m.id = l.message_id
             WHERE m.remote_id = 'm3'"
        ),
        0,
        "a folder nobody enumerated was invented"
    );
    // FR-23: a search is never an arrival, and a held message keeps what it arrived with.
    assert_eq!(
        count("SELECT count(*) FROM message WHERE provenance = 'Discovered'"),
        2
    );
    assert_eq!(
        count("SELECT count(*) FROM message WHERE remote_id = 'm1' AND provenance = 'Delivered'"),
        1
    );
    // D-82: no folder's position moved, because a search is not a delta.
    let cursor: Vec<u8> = account
        .store
        .query_row(
            "SELECT cursor FROM folder_sync_state WHERE folder_id = ?1",
            [folder],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cursor, b"c1");
    assert_eq!(
        count("SELECT count(*) FROM folder_sync_state"),
        1,
        "a search wrote a folder's sync state"
    );
}

// ---------------------------------------------------------------------------
// D-44 — a move on an account whose identifiers do not survive one.
//
// The one place this file reads a real adapter's recording: the move the adapter's own replay
// test records, parsed by the adapter's own parser, walked through the real driver and the real
// store. What is under test is still the rule — the driver knows only the declared capability.
// ---------------------------------------------------------------------------

mod moves {
    use super::*;
    use sift_graph::wire;

    const BEFORE: &[u8] =
        include_bytes!("../../../providers/sift-graph/fixtures/envelopes-before-move.json");
    const AFTER: &[u8] =
        include_bytes!("../../../providers/sift-graph/fixtures/envelopes-after-move.json");
    const MOVED_OUT: &[u8] =
        include_bytes!("../../../providers/sift-graph/fixtures/delta-inbox-moved-out.json");
    const MOVED_IN: &[u8] =
        include_bytes!("../../../providers/sift-graph/fixtures/delta-archive-moved-in.json");

    fn envelopes(body: &[u8]) -> Vec<Envelope> {
        wire::parse_batch(body, usize::MAX)
            .unwrap()
            .iter()
            .map(|item| wire::parse_envelope(&item.body).unwrap())
            .collect()
    }

    fn live(body: &[u8]) -> Result<Delta, &'static str> {
        page(
            wire::parse_delta(body, true).unwrap().changes,
            "live",
            false,
        )
    }

    fn folder(id: &str, special_use: SpecialUse) -> RemoteFolder {
        RemoteFolder {
            id: RemoteFolderId(id.into()),
            display_name: id.into(),
            special_use: Some(special_use),
        }
    }

    /// An account holding inbox and archive, both watched, with the adapter's capabilities.
    /// `archive_first` puts the archive earlier in the walk, so its arrivals are applied
    /// before the inbox's departures.
    fn account_with(
        s: &Scratch,
        archive_first: bool,
        rounds: Vec<[Result<Delta, &'static str>; 2]>,
        held: Vec<Envelope>,
    ) -> (Account, Scripted, i64, i64) {
        let inbox = folder("AAMk-inbox", SpecialUse::Inbox);
        let archive = folder("AAMk-archive", SpecialUse::Archive);
        let folders = if archive_first {
            vec![archive, inbox]
        } else {
            vec![inbox, archive]
        };
        let mut pages = Vec::new();
        for [inbox_page, archive_page] in rounds {
            if archive_first {
                pages.extend([archive_page, inbox_page]);
            } else {
                pages.extend([inbox_page, archive_page]);
            }
        }
        let mut adapter = Scripted::new(folders).answering(pages, held);
        adapter.capabilities = sift_graph::capabilities();
        assert!(!adapter.capabilities.identifier_survives_a_move());
        let account = s.open();
        run::discover_folders(&adapter, &account).unwrap();
        let inbox =
            ingest::folder_local_id(&account.store, &RemoteFolderId("AAMk-inbox".into())).unwrap();
        let archive =
            ingest::folder_local_id(&account.store, &RemoteFolderId("AAMk-archive".into()))
                .unwrap();
        ingest::set_watched(&account.store, archive, true).unwrap();
        (account, adapter, inbox, archive)
    }

    fn local_of(account: &Account, remote: &str) -> Vec<u8> {
        account
            .store
            .query_row(
                "SELECT id FROM message WHERE remote_id = ?1",
                [remote],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn count(account: &Account, sql: &str) -> i64 {
        account.store.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn the_recorded_move(name: &str, archive_first: bool) {
        let s = Scratch::new(name);
        let seed = page(
            vec![
                present("AAMk-d1", Provenance::Discovered),
                present("AAMk-d2", Provenance::Discovered),
                present("AAMk-u1", Provenance::Discovered),
            ],
            "seeded",
            false,
        );
        let mut held = envelopes(BEFORE);
        held.extend(envelopes(AFTER));
        let (mut account, adapter, _, archive) = account_with(
            &s,
            archive_first,
            vec![
                [seed, page(vec![], "quiet", false)],
                [live(MOVED_OUT), live(MOVED_IN)],
            ],
            held,
        );
        let ids = ids();

        run::sync_account(&adapter, &mut account, &ids, 1).unwrap();
        let before = [
            local_of(&account, "AAMk-d1"),
            local_of(&account, "AAMk-d2"),
            local_of(&account, "AAMk-u1"),
        ];

        let report = run::sync_account(&adapter, &mut account, &ids, 1).unwrap();

        // Nothing lost, nothing invented, nothing left held.
        assert_eq!(count(&account, "SELECT count(*) FROM message"), 3);
        assert_eq!(
            count(
                &account,
                "SELECT count(*) FROM message WHERE remote_id IS NULL"
            ),
            0,
            "a departure outlived the round that held it"
        );

        // The unambiguous move keeps its identity — the shell's selection survives it — and
        // the provenance it arrived with: moved into the archive is not delivered there.
        let invoice = local_of(&account, "AAMk-n-u1");
        assert_eq!(invoice, before[2], "the unambiguous move lost its identity");
        let (provenance, location): (String, i64) = account
            .store
            .query_row(
                "SELECT m.provenance, l.folder_id FROM message m
                 JOIN message_location l ON l.message_id = m.id WHERE m.id = ?1",
                [&invoice],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(provenance, "Discovered");
        assert_eq!(location, archive);
        assert_eq!(
            count(
                &account,
                "SELECT message_count FROM thread WHERE remote_thread_id = 'conv-u'"
            ),
            1,
            "the rejoin counted one message twice in its thread"
        );

        // The duplicates corroborate against **both** departures. Choosing one would be a
        // guess, and a wrong guess is an archive applied to the wrong message.
        let d1 = local_of(&account, "AAMk-n-d1");
        let d2 = local_of(&account, "AAMk-n-d2");
        assert_ne!(d1, d2, "an ambiguous move merged two messages");
        for duplicate in [&d1, &d2] {
            assert!(
                !before.contains(duplicate),
                "an ambiguous arrival inherited an identity it could not prove was its own"
            );
        }

        assert_eq!(report.rejoined, 1);
        assert_eq!(report.inserted, 2);
        // FR-33: each ambiguous arrival is recorded with the two candidates it refused.
        assert_eq!(report.near_misses.len(), 2);
        assert!(report.near_misses.iter().all(|n| n.candidates == 2));
    }

    #[test]
    fn the_recorded_move_rejoins_the_unambiguous_message_and_leaves_the_ambiguous_distinct() {
        the_recorded_move("move-inbox-first", false);
    }

    #[test]
    fn a_move_rejoins_whichever_folder_the_round_walks_first() {
        // The arrivals are applied before the departures they rejoin.
        the_recorded_move("move-archive-first", true);
    }

    fn unread(id: &str) -> Envelope {
        let mut e = envelope(id, "Moved", 100);
        e.thread_id = Some("conversation".into());
        e.internet_message_id = Some("<moved@x.test>".into());
        e
    }

    #[test]
    fn an_unread_message_moved_in_is_not_announced_as_new_mail() {
        // The adapter reports every live presence as delivered, because it cannot tell an
        // arrival from a message moved in. The rejoin can, and FR-23 must hear it.
        let s = Scratch::new("move-unread");
        let (mut account, adapter, _, _) = account_with(
            &s,
            false,
            vec![
                [
                    page(vec![present("m1", Provenance::Discovered)], "c1", false),
                    page(vec![], "c1", false),
                ],
                [
                    page(
                        vec![Change::Removed {
                            id: RemoteMessageId("m1".into()),
                        }],
                        "c2",
                        false,
                    ),
                    page(
                        vec![present("m1-moved", Provenance::Delivered)],
                        "c2",
                        false,
                    ),
                ],
            ],
            vec![unread("m1"), unread("m1-moved")],
        );
        let ids = ids();
        run::sync_account(&adapter, &mut account, &ids, 1).unwrap();
        let before = local_of(&account, "m1");

        let report = run::sync_account(&adapter, &mut account, &ids, 1).unwrap();
        assert_eq!(local_of(&account, "m1-moved"), before);
        assert_eq!(report.delivered, 0, "a move was announced as new mail");
        assert_eq!(report.newest, None);
        assert!(
            report.arrivals.is_empty(),
            "a rejoined arrival is still offered to FR-23's rules: {:?}",
            report.arrivals
        );
        assert_eq!(report.rejoined, 1);
        assert_eq!(
            count(
                &account,
                "SELECT count(*) FROM message WHERE provenance = 'Delivered'"
            ),
            0
        );
    }

    #[test]
    fn a_departure_nothing_rejoins_is_deleted_even_when_the_round_fails() {
        // A move to an unwatched folder, or a delete: no arrival. And a round that fails after
        // the departure was applied still settles it rather than leaving a row nothing reaches.
        let s = Scratch::new("move-failed-round");
        let (mut account, adapter, _, _) = account_with(
            &s,
            false,
            vec![
                [
                    page(vec![present("m1", Provenance::Discovered)], "c1", false),
                    page(vec![], "c1", false),
                ],
                [
                    page(
                        vec![Change::Removed {
                            id: RemoteMessageId("m1".into()),
                        }],
                        "c2",
                        false,
                    ),
                    Err("the archive did not answer"),
                ],
            ],
            vec![unread("m1")],
        );
        let ids = ids();
        run::sync_account(&adapter, &mut account, &ids, 1).unwrap();
        assert!(run::sync_account(&adapter, &mut account, &ids, 1).is_err());
        assert_eq!(count(&account, "SELECT count(*) FROM message"), 0);
    }

    #[test]
    fn a_departure_a_crashed_round_left_behind_is_settled_by_the_next() {
        // The departure is marked in the store, not only in memory, so a window that never
        // closed does not strand it.
        let s = Scratch::new("move-stranded");
        let (mut account, adapter, inbox, _) = account_with(
            &s,
            false,
            vec![
                [
                    page(vec![present("m1", Provenance::Discovered)], "c1", false),
                    page(vec![], "c1", false),
                ],
                [page(vec![], "c3", false), page(vec![], "c3", false)],
            ],
            vec![unread("m1")],
        );
        let ids = ids();
        run::sync_account(&adapter, &mut account, &ids, 1).unwrap();
        // A holding window applies the departure, and the process ends before it settles.
        let mut window = ingest::MoveWindow::for_account(false);
        assert!(window.holds());
        ingest::apply_page_within(
            &mut account.store,
            inbox,
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
            &mut window,
        )
        .unwrap();
        assert_eq!(count(&account, "SELECT count(*) FROM message"), 1);

        run::sync_account(&adapter, &mut account, &ids, 1).unwrap();
        assert_eq!(count(&account, "SELECT count(*) FROM message"), 0);
    }

    /// A message's tag names, without the isolation marks NFR-54's normalizer stores them in.
    fn tags_of(account: &Account, message: &[u8]) -> Vec<String> {
        let mut stmt = account
            .store
            .prepare(
                "SELECT t.name FROM message_tag mt JOIN tag t ON t.id = mt.tag_id
                 WHERE mt.message_id = ?1 ORDER BY t.name",
            )
            .unwrap();
        stmt.query_map([message], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|name| {
                name.unwrap()
                    .trim_matches(|c| c == '\u{2068}' || c == '\u{2069}')
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn one_folder_walked_alone_settles_its_own_window_and_the_rejoin_takes_the_arrivals_tags() {
        // `sync_folder` is a round of its own: it holds what leaves the folder and settles it
        // before it returns. A reissue within one folder is a departure and an arrival in the
        // same walk. The rejoined message is described by what the provider says now, tags
        // included, and keeps nothing of what it said before.
        let s = Scratch::new("move-one-folder");
        let mut before_move = unread("m1");
        before_move.tags = vec!["Blue".into()];
        let mut after_move = unread("m1-reissued");
        after_move.tags = vec!["Red".into(), "Urgent".into()];
        let mut adapter = Scripted::new(vec![folder("AAMk-inbox", SpecialUse::Inbox)]).answering(
            vec![
                page(vec![present("m1", Provenance::Discovered)], "c1", false),
                page(
                    vec![
                        Change::Removed {
                            id: RemoteMessageId("m1".into()),
                        },
                        present("m1-reissued", Provenance::Delivered),
                    ],
                    "c2",
                    false,
                ),
            ],
            vec![before_move, after_move],
        );
        adapter.capabilities = sift_graph::capabilities();
        let mut account = s.open();
        run::discover_folders(&adapter, &account).unwrap();
        let remote = RemoteFolderId("AAMk-inbox".into());
        let inbox = ingest::folder_local_id(&account.store, &remote).unwrap();
        let ids = ids();

        run::sync_folder(&adapter, &mut account, inbox, &remote, &ids, 1).unwrap();
        let before = local_of(&account, "m1");
        assert_eq!(tags_of(&account, &before), vec!["Blue"]);

        let report = run::sync_folder(&adapter, &mut account, inbox, &remote, &ids, 1).unwrap();
        assert_eq!(report.rejoined, 1, "the folder's own window never settled");
        assert_eq!(
            count(
                &account,
                "SELECT count(*) FROM message WHERE remote_id IS NULL"
            ),
            0,
            "a departure outlived the walk that held it"
        );
        assert_eq!(count(&account, "SELECT count(*) FROM message"), 1);
        assert_eq!(local_of(&account, "m1-reissued"), before);
        assert_eq!(
            tags_of(&account, &before),
            vec!["Red", "Urgent"],
            "the rejoined message lost the tags it arrived with, or kept the ones it left with"
        );
    }

    #[test]
    fn an_account_whose_identifiers_survive_a_move_never_holds() {
        assert!(!ingest::MoveWindow::for_account(true).holds());
        assert!(!ingest::MoveWindow::default().holds());
    }
}

//! Where a delta page becomes rows — and the one transaction that makes it safe.
//!
//! # The rule this module exists to keep
//!
//! **A delta page and the cursor that follows it advance in one transaction.** D-82 states
//! it and gives the reason in four words: a cursor advancing past change that was not
//! applied is silent data loss. Silent in the worst way, too — the messages were never seen,
//! so nothing afterwards is inconsistent. There is no repair pass that could find it.
//!
//! Everything else here follows from that. The adapter is asked for a page, envelopes are
//! fetched for what the page names, and only then is one transaction opened that writes the
//! rows and the cursor together. If anything fails before the commit, the cursor is where it
//! was and the page is re-applied — which is safe because reapplication is safe, the property
//! D-82 leans on and records as its weak point.
//!
//! # What is *not* here
//!
//! Retry, backoff, and any decision about when to run. D-87 puts a stated delay on D-25's
//! wheel; this module is called when the scheduler says so and returns when it is done.

use crate::join;
use rusqlite::{Connection, Transaction, params};
use sift_foundation::identity::{LocalId, LocalIdGenerator};
use sift_foundation::normalize;
use sift_provider::adapter::{
    Change, Delta, Envelope, Provenance, RemoteFolder, RemoteFolderId, RemoteMessageId, SpecialUse,
};
use sift_store::flags;

/// What went wrong writing.
#[derive(Debug)]
pub enum IngestError {
    Store(rusqlite::Error),
    /// A folder the page names is not one this account knows about.
    UnknownFolder(RemoteFolderId),
}

impl From<rusqlite::Error> for IngestError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e)
    }
}

impl core::fmt::Display for IngestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "the store refused: {e}"),
            Self::UnknownFolder(id) => write!(f, "no folder `{}` in this account", id.0),
        }
    }
}

impl std::error::Error for IngestError {}

// ---------------------------------------------------------------------------
// Folders — D-83.
// ---------------------------------------------------------------------------

/// What reconciling an enumeration did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderReport {
    pub discovered: Vec<String>,
    pub retired: Vec<String>,
    pub unchanged: usize,
}

/// Reconcile an enumeration into the store.
///
/// **A folder that stops appearing is retired, not deleted** — D-83. Its messages stop being
/// present *in it* and stay reachable through search and threads, which is the difference
/// between a folder disappearing from a sidebar and a user's mail disappearing.
///
/// A folder that reappears is un-retired rather than re-created, so its cursor and its
/// backfill position survive. Deleting and recreating would restart a completed backfill.
///
/// # Errors
/// See [`IngestError`].
pub fn reconcile_folders(
    store: &Connection,
    remote: &[RemoteFolder],
) -> Result<FolderReport, IngestError> {
    let mut report = FolderReport::default();
    let known: Vec<(i64, String, bool)> = {
        let mut stmt = store
            .prepare("SELECT id, remote_id, retired FROM folder WHERE remote_id IS NOT NULL")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
        })?;
        rows.collect::<Result<_, _>>()?
    };

    for folder in remote {
        let display = normalize::for_display(&folder.display_name)
            .as_str()
            .to_owned();
        let special = folder.special_use.map(special_use_name);
        match known
            .iter()
            .find(|(_, remote_id, _)| *remote_id == folder.id.0)
        {
            Some((id, _, retired)) => {
                store.execute(
                    "UPDATE folder SET display_name = ?2, special_use = ?3, retired = 0 WHERE id = ?1",
                    params![id, display, special],
                )?;
                if *retired {
                    report.discovered.push(folder.id.0.clone());
                } else {
                    report.unchanged += 1;
                }
            }
            None => {
                store.execute(
                    "INSERT INTO folder (remote_id, special_use, display_name, watched)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        folder.id.0,
                        special,
                        display,
                        // FR-43: a newly discovered folder is *discovered*, not adopted. The
                        // inbox is the one exception, because an account whose inbox is not
                        // watched is an account that shows nothing.
                        i64::from(folder.special_use == Some(SpecialUse::Inbox))
                    ],
                )?;
                report.discovered.push(folder.id.0.clone());
            }
        }
    }

    for (id, remote_id, retired) in &known {
        if !retired && !remote.iter().any(|f| f.id.0 == *remote_id) {
            store.execute("UPDATE folder SET retired = 1 WHERE id = ?1", params![id])?;
            report.retired.push(remote_id.clone());
        }
    }
    Ok(report)
}

const fn special_use_name(use_: SpecialUse) -> &'static str {
    match use_ {
        SpecialUse::Inbox => "Inbox",
        SpecialUse::Archive => "Archive",
        SpecialUse::Sent => "Sent",
        SpecialUse::Trash => "Trash",
        SpecialUse::Spam => "Spam",
        SpecialUse::Drafts => "Drafts",
    }
}

/// A folder's local identity, which D-83 makes the key.
///
/// # Errors
/// See [`IngestError`].
pub fn folder_local_id(store: &Connection, remote: &RemoteFolderId) -> Result<i64, IngestError> {
    store
        .query_row(
            "SELECT id FROM folder WHERE remote_id = ?1",
            params![remote.0],
            |r| r.get(0),
        )
        .map_err(|_| IngestError::UnknownFolder(remote.clone()))
}

/// The folders FR-43 says to sync.
///
/// # Errors
/// See [`IngestError`].
pub fn watched_folders(store: &Connection) -> Result<Vec<(i64, RemoteFolderId)>, IngestError> {
    let mut stmt = store.prepare(
        "SELECT id, remote_id FROM folder
         WHERE watched = 1 AND retired = 0 AND remote_id IS NOT NULL
         ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get(0)?, RemoteFolderId(r.get::<_, String>(1)?)))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Put a folder into, or out of, FR-43's watched set.
///
/// # Errors
/// See [`IngestError`].
pub fn set_watched(store: &Connection, folder: i64, watched: bool) -> Result<(), IngestError> {
    store.execute(
        "UPDATE folder SET watched = ?2 WHERE id = ?1",
        params![folder, i64::from(watched)],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The cursor.
// ---------------------------------------------------------------------------

/// The cursor a folder resumes from, or `None` where it has never synced.
///
/// **Per folder, never per account.** One folder's validity change must not invalidate the
/// account.
///
/// # Errors
/// See [`IngestError`].
pub fn cursor_of(
    store: &Connection,
    folder: i64,
) -> Result<Option<sift_provider::adapter::Cursor>, IngestError> {
    let cursor: Option<Vec<u8>> = store
        .query_row(
            "SELECT cursor FROM folder_sync_state WHERE folder_id = ?1",
            params![folder],
            |r| r.get(0),
        )
        .unwrap_or(None);
    Ok(cursor.map(sift_provider::adapter::Cursor))
}

/// The state a folder is in.
///
/// # Errors
/// See [`IngestError`].
pub fn state_of(store: &Connection, folder: i64) -> Result<Option<String>, IngestError> {
    Ok(store
        .query_row(
            "SELECT state FROM folder_sync_state WHERE folder_id = ?1",
            params![folder],
            |r| r.get(0),
        )
        .ok())
}

/// Which remote identifiers in a page this account does not already hold.
///
/// # Errors
/// See [`IngestError`].
pub fn unknown_to_us(
    store: &Connection,
    ids: &[RemoteMessageId],
) -> Result<Vec<RemoteMessageId>, IngestError> {
    let mut stmt = store.prepare("SELECT 1 FROM message WHERE remote_id = ?1")?;
    let mut out = Vec::new();
    for id in ids {
        if !stmt.exists(params![id.0])? {
            out.push(id.clone());
        }
    }
    Ok(out)
}

/// What a page did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageReport {
    pub inserted: usize,
    pub updated: usize,
    pub removed: usize,
    /// Arrivals — messages the delta reported as *delivered*, that were not held before, and
    /// that were **unread at that moment**. FR-23's definition of new mail, recorded at the only
    /// moment it can be.
    ///
    /// A delivered message already read when it arrived — read on another device before this
    /// turn reached it — is not new mail, and neither is one whose envelope did not come back:
    /// nothing says whether it was read, and a notification with nothing to say is noise.
    pub delivered: usize,
    /// The most recently received of those arrivals — what FR-23's notification names and
    /// opens. `None` exactly when `delivered` is zero.
    pub newest: Option<Arrival>,
    /// Moves D-44 rejoined: an arrival that took back the local identity of the message that
    /// departed in the same window. Not counted in `inserted` — no message was created.
    pub rejoined: usize,
    /// Arrivals the scope proposed candidates for and the join refused — D-44's near misses,
    /// recorded for the FR-33 debug view rather than passed over silently.
    pub near_misses: Vec<NearMiss>,
}

/// An arrival D-44 could have joined and did not, because the candidates the scope proposed
/// did not corroborate to exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NearMiss {
    /// The arrival, which kept the new identity it was given.
    pub arrival: LocalId,
    /// How many departures the scope proposed for it.
    pub candidates: usize,
}

/// One message FR-23 counts as new mail, as much of it as a notification needs to name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    pub id: LocalId,
    /// D-55's server-assigned received time — what "newest" is ordered by.
    pub received_millis: u64,
}

impl PageReport {
    /// Fold another turn's report into this one.
    ///
    /// **One place, because it is summed in two.** A field added here and forgotten at one of
    /// the call sites would be a count that is right for one folder and wrong for an account.
    pub fn absorb(&mut self, other: &Self) {
        self.inserted += other.inserted;
        self.updated += other.updated;
        self.removed += other.removed;
        self.delivered += other.delivered;
        self.newest = newer(self.newest, other.newest);
        self.rejoined += other.rejoined;
        self.near_misses.extend_from_slice(&other.near_misses);
    }

    fn arrived(&mut self, arrival: Arrival) {
        self.delivered += 1;
        self.newest = newer(self.newest, Some(arrival));
    }
}

/// The later of two arrivals, by received time and then by identity — D-55's own order.
fn newer(a: Option<Arrival>, b: Option<Arrival>) -> Option<Arrival> {
    match (a, b) {
        (Some(a), Some(b)) => Some(
            if (b.received_millis, b.id.as_u128()) > (a.received_millis, a.id.as_u128()) {
                b
            } else {
                a
            },
        ),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Apply one delta page and advance its cursor, **in one transaction**.
///
/// `envelopes` are what the adapter returned for the identifiers this page names. A change
/// whose envelope is missing is applied for what it is — a presence, a removal — without
/// inventing content for it.
///
/// A removal is applied at once: a message leaving its last folder leaves the store. On an
/// account whose identifiers do not survive a move, that turns every move into a delete and an
/// arrival — which [`apply_page_within`] and a [`MoveWindow`] exist to avoid.
///
/// # Errors
/// See [`IngestError`].
pub fn apply_page(
    store: &mut Connection,
    folder: i64,
    page: &Delta,
    envelopes: &[Envelope],
    ids: &LocalIdGenerator,
) -> Result<PageReport, IngestError> {
    apply_page_within(
        store,
        folder,
        page,
        envelopes,
        ids,
        &mut MoveWindow::default(),
    )
}

/// D-44's move window: one sync round, across every folder it walks.
///
/// Under an identifier that does not survive a move, a move is reported as a removal from one
/// folder and a presence, under an identifier nobody has seen, in another — in either order,
/// on different pages, in different folders' deltas. So a window that **holds** keeps each
/// departure's row instead of deleting it, remembers each arrival, and [`settle_moves`] joins
/// the two once both sides of the round have been applied. A rejoined arrival takes back the
/// departed identity, so a selection, a queued intent or a notification that named it still
/// names the same message.
///
/// **A held departure is marked in the store, not only here.** Its row loses its remote
/// identifier (which is dead: the provider reissued it) along with its last location, so a
/// round that ends early — an error, a crash between two pages — leaves rows the next
/// [`settle_moves`] finds and resolves, rather than rows nothing will ever delete.
#[derive(Debug, Default)]
pub struct MoveWindow {
    holding: bool,
    arrived: Vec<Arrived>,
}

#[derive(Debug, Clone, Copy)]
struct Arrived {
    id: LocalId,
    received_millis: u64,
    /// Whether the page counted it as FR-23 new mail.
    delivered: bool,
}

impl MoveWindow {
    /// A window for an account, from its declared capability. One whose identifiers survive a
    /// move never holds: its removals are removals, and its presences are keyed on an
    /// identifier that already joins.
    #[must_use]
    pub fn for_account(identifier_survives_a_move: bool) -> Self {
        Self {
            holding: !identifier_survives_a_move,
            arrived: Vec::new(),
        }
    }

    /// Whether this window holds departures for [`settle_moves`].
    #[must_use]
    pub const fn holds(&self) -> bool {
        self.holding
    }
}

/// [`apply_page`], inside a [`MoveWindow`]. The page and its cursor are still one
/// transaction; what a holding window changes is only that a message leaving its last folder
/// is held for [`settle_moves`] rather than deleted.
///
/// # Errors
/// See [`IngestError`].
pub fn apply_page_within(
    store: &mut Connection,
    folder: i64,
    page: &Delta,
    envelopes: &[Envelope],
    ids: &LocalIdGenerator,
    window: &mut MoveWindow,
) -> Result<PageReport, IngestError> {
    let tx = store.transaction()?;
    let mut report = PageReport::default();
    // Remembered only once the transaction commits: an arrival from a page that rolled back
    // was never written, and settling it would reach for a row that does not exist.
    let mut arrived = Vec::new();

    for change in &page.changes {
        match change {
            Change::Present { id, provenance } => {
                let envelope = envelopes.iter().find(|e| e.id == *id);
                let outcome = upsert(&tx, Some(folder), id, envelope, *provenance, ids)?;
                match outcome {
                    Upsert::Inserted(local) => {
                        report.inserted += 1;
                        // FR-23: delivered, **and unread at this moment**.
                        let delivered = *provenance == Provenance::Delivered
                            && envelope.is_some_and(|e| !e.read);
                        if let Some(envelope) = envelope {
                            if delivered {
                                report.arrived(Arrival {
                                    id: local,
                                    received_millis: envelope.received_at_millis,
                                });
                            }
                            // Only an arrival with an envelope has anything D-44 could scope
                            // or corroborate on.
                            if window.holding {
                                arrived.push(Arrived {
                                    id: local,
                                    received_millis: envelope.received_at_millis,
                                    delivered,
                                });
                            }
                        }
                    }
                    Upsert::Updated => report.updated += 1,
                }
            }
            Change::FlagsChanged { id } => {
                if let Some(envelope) = envelopes.iter().find(|e| e.id == *id) {
                    let changed = tx.execute(
                        "UPDATE message SET flags = ?2 WHERE remote_id = ?1",
                        params![id.0, flags::word(envelope.read, envelope.flagged)],
                    )?;
                    report.updated += changed;
                }
            }
            Change::Removed { id } => {
                report.removed += remove_from(&tx, folder, id, window.holding)?;
            }
        }
    }

    // The cursor, in the same transaction as the change it describes. This is the line the
    // module exists for.
    tx.execute(
        "INSERT INTO folder_sync_state (folder_id, state, cursor, last_success_millis)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(folder_id) DO UPDATE SET
            state = excluded.state,
            cursor = excluded.cursor,
            last_success_millis = excluded.last_success_millis",
        params![
            folder,
            if page.more { "Backfilling" } else { "Live" },
            page.next.0,
            now_millis()
        ],
    )?;
    tx.commit()?;
    window.arrived.extend(arrived);
    Ok(report)
}

/// Close a [`MoveWindow`]: rejoin what arrived to what departed under D-44, delete every
/// departure nothing rejoined, and correct `report` — the round's summed report — for it.
///
/// One transaction. A rejoined arrival's row is folded into the departed one, which keeps its
/// local identity, its **provenance** (a message moved into a folder was not delivered there,
/// and FR-23 must not announce it) and everything cached against it, and takes the arrival's
/// remote identifier, envelope, flags, tags and locations. The arrival's row is then deleted;
/// its identity was never handed to anything but this round's report, which is corrected here.
///
/// An ambiguous arrival stays the distinct message it was inserted as, and is recorded as a
/// near miss. A window that does not hold does nothing.
///
/// # Errors
/// See [`IngestError`].
pub fn settle_moves(
    store: &mut Connection,
    window: &mut MoveWindow,
    report: &mut PageReport,
) -> Result<(), IngestError> {
    if !window.holding {
        return Ok(());
    }
    let arrived = core::mem::take(&mut window.arrived);
    let tx = store.transaction()?;

    // Every held departure, including any a round that ended early left behind.
    let departed: Vec<Held> = {
        let mut stmt = tx.prepare(&format!(
            "{HELD} WHERE m.remote_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM message_location l WHERE l.message_id = m.id)"
        ))?;
        let rows = stmt.query_map([], Held::read)?;
        rows.collect::<Result<_, _>>()?
    };
    // The arrivals still held as rows. One a later page of the same round removed again is
    // gone, and is not a candidate for anything.
    let mut arrivals: Vec<(Arrived, Held)> = Vec::with_capacity(arrived.len());
    {
        // Still present: an arrival a later page removed again is itself a held departure,
        // and must not be offered to itself.
        let mut stmt = tx.prepare(&format!(
            "{HELD} WHERE m.id = ?1 AND m.remote_id IS NOT NULL"
        ))?;
        for a in arrived {
            if let Some(held) = stmt
                .query_row(params![a.id.to_bytes().to_vec()], Held::read)
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })?
            {
                arrivals.push((a, held));
            }
        }
    }

    let outcomes = if departed.is_empty() {
        Vec::new()
    } else {
        join::rejoin(
            &arrivals.iter().map(|(_, h)| h.side()).collect::<Vec<_>>(),
            &departed.iter().map(Held::side).collect::<Vec<_>>(),
        )
    };
    let mut kept = vec![false; departed.len()];
    let mut rejoined = Vec::new();
    for ((arrival, held), outcome) in arrivals.iter().zip(&outcomes) {
        match *outcome {
            join::Resolution::One(d) => {
                merge_into(&tx, &departed[d], held)?;
                kept[d] = true;
                rejoined.push(arrival.id);
                report.rejoined += 1;
                report.inserted = report.inserted.saturating_sub(1);
                if arrival.delivered {
                    report.delivered = report.delivered.saturating_sub(1);
                }
            }
            join::Resolution::Distinct { near_misses } if near_misses > 0 => {
                report.near_misses.push(NearMiss {
                    arrival: arrival.id,
                    candidates: near_misses,
                });
            }
            join::Resolution::Distinct { .. } => {}
        }
    }
    for (held, kept) in departed.iter().zip(kept) {
        if !kept {
            tx.execute("DELETE FROM message WHERE id = ?1", params![held.id])?;
        }
    }
    tx.commit()?;

    // A rejoined arrival was never new mail. What FR-23 names is the newest of the rest.
    if arrivals
        .iter()
        .any(|(a, _)| a.delivered && rejoined.contains(&a.id))
    {
        report.newest = arrivals
            .iter()
            .filter(|(a, _)| a.delivered && !rejoined.contains(&a.id))
            .fold(None, |newest, (a, _)| {
                newer(
                    newest,
                    Some(Arrival {
                        id: a.id,
                        received_millis: a.received_millis,
                    }),
                )
            });
    }
    Ok(())
}

/// What [`settle_moves`] reads of one side of a move.
#[derive(Debug)]
struct Held {
    id: Vec<u8>,
    thread_id: Option<Vec<u8>>,
    conversation: Option<String>,
    internet_message_id: Option<String>,
    digest: Vec<u8>,
    rule_version: u32,
}

/// The scope is the thread's **remote** conversation identifier: a thread with none is a
/// message's own fallback thread, and proposes nothing.
const HELD: &str = "SELECT m.id, m.thread_id, t.remote_thread_id, m.internet_message_id,
                           m.fallback_digest, m.digest_rule_version
                    FROM message m LEFT JOIN thread t ON t.id = m.thread_id";

impl Held {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            thread_id: r.get(1)?,
            conversation: r.get(2)?,
            internet_message_id: r.get(3)?,
            digest: r.get(4)?,
            rule_version: r.get(5)?,
        })
    }

    fn side(&self) -> join::Side<'_> {
        join::Side {
            conversation: self.conversation.as_deref(),
            internet_message_id: self.internet_message_id.as_deref(),
            digest: &self.digest,
            rule_version: self.rule_version,
        }
    }
}

/// Fold an arrival's row into the departure it rejoined. The departure keeps its identity,
/// its provenance and what was cached against it (its body, its index text, its attachment
/// claims); everything the provider just said about the message comes from the arrival.
fn merge_into(tx: &Transaction<'_>, departed: &Held, arrival: &Held) -> Result<(), IngestError> {
    tx.execute(
        "UPDATE message AS d SET
            remote_id = a.remote_id, internet_message_id = a.internet_message_id,
            fallback_digest = a.fallback_digest, digest_rule_version = a.digest_rule_version,
            thread_id = a.thread_id, received_at_millis = a.received_at_millis,
            origination_millis = a.origination_millis, sender = a.sender,
            recipients = a.recipients, subject = a.subject, snippet = a.snippet,
            flags = a.flags, size_bytes = a.size_bytes
         FROM (SELECT * FROM message WHERE id = ?2) AS a
         WHERE d.id = ?1",
        params![departed.id, arrival.id],
    )?;
    tx.execute(
        "DELETE FROM message_tag WHERE message_id = ?1",
        params![departed.id],
    )?;
    tx.execute(
        "UPDATE message_tag SET message_id = ?1 WHERE message_id = ?2",
        params![departed.id, arrival.id],
    )?;
    tx.execute(
        "UPDATE OR IGNORE message_location SET message_id = ?1 WHERE message_id = ?2",
        params![departed.id, arrival.id],
    )?;
    // The arrival joined the departure's thread and counted itself into it. Two rows became
    // one, so the thread holds one message fewer than that count says — D-102's count is of
    // messages still held.
    if arrival.thread_id.is_some() && arrival.thread_id == departed.thread_id {
        tx.execute(
            "UPDATE thread SET message_count = max(message_count - 1, 1) WHERE id = ?1",
            params![arrival.thread_id],
        )?;
    }
    tx.execute("DELETE FROM message WHERE id = ?1", params![arrival.id])?;
    Ok(())
}

/// Ingest the envelopes a server-side search found — FR-21 — and say which local identity
/// each one has.
///
/// **No cursor moves.** A search is not a delta: it says a message exists and matches, never
/// what changed, so it writes rows and nothing about any folder's position. A message already
/// held is refreshed from its envelope and keeps its identity and its provenance; one not held
/// is inserted as **discovered**, never delivered — FR-23's new mail is decided by a delta and
/// only by one, and a search finding a year-old message is not an arrival.
///
/// A message lands in every folder its envelope names that this account knows. One naming none
/// it knows — a folder the enumeration has not reached — is still inserted, with no location,
/// because the person searched for it and must be able to open it; the delta that reaches the
/// folder gives it one. Everything is one transaction, so a failed search leaves nothing
/// half-written.
///
/// # Errors
/// See [`IngestError`].
pub fn ingest_found(
    store: &mut Connection,
    envelopes: &[Envelope],
    ids: &LocalIdGenerator,
) -> Result<Vec<(RemoteMessageId, LocalId)>, IngestError> {
    let tx = store.transaction()?;
    let mut found = Vec::with_capacity(envelopes.len());
    for envelope in envelopes {
        let folders: Vec<i64> = envelope
            .folders
            .iter()
            .filter_map(|remote| folder_local_id(&tx, remote).ok())
            .collect();
        let (first, rest) = match folders.split_first() {
            Some((first, rest)) => (Some(*first), rest),
            None => (None, &[][..]),
        };
        let local = match upsert(
            &tx,
            first,
            &envelope.id,
            Some(envelope),
            Provenance::Discovered,
            ids,
        )? {
            Upsert::Inserted(local) => local,
            Upsert::Updated => tx.query_row(
                "SELECT id FROM message WHERE remote_id = ?1",
                params![envelope.id.0],
                |r| {
                    let bytes: Vec<u8> = r.get(0)?;
                    Ok(LocalId::from_bytes(
                        bytes.as_slice().try_into().unwrap_or([0; 16]),
                    ))
                },
            )?,
        };
        for folder in rest {
            tx.execute(
                "INSERT OR IGNORE INTO message_location (message_id, folder_id) VALUES (?1, ?2)",
                params![local.to_bytes().to_vec(), folder],
            )?;
        }
        found.push((envelope.id.clone(), local));
    }
    tx.commit()?;
    Ok(found)
}

/// Record that a folder's cursor was refused, without touching the cursor itself.
///
/// D-82 puts the folder into `Invalidated`, which is **not a resting state**: NFR-18 forbids
/// requiring the user to ask, so recovery follows immediately. The stale cursor is left in
/// place rather than cleared, because a recovery that is itself interrupted must still be
/// able to say what it was recovering from.
///
/// # Errors
/// See [`IngestError`].
pub fn mark_invalidated(store: &Connection, folder: i64) -> Result<(), IngestError> {
    store.execute(
        "INSERT INTO folder_sync_state (folder_id, state) VALUES (?1, 'Invalidated')
         ON CONFLICT(folder_id) DO UPDATE SET state = 'Invalidated'",
        params![folder],
    )?;
    Ok(())
}

/// Record a degradation, with the reason NFR-29 requires be surfaced rather than hidden.
///
/// # Errors
/// See [`IngestError`].
pub fn mark_degraded(store: &Connection, folder: i64, reason: &str) -> Result<(), IngestError> {
    store.execute(
        "INSERT INTO folder_sync_state (folder_id, state, degraded_reason) VALUES (?1, 'Degraded', ?2)
         ON CONFLICT(folder_id) DO UPDATE SET state = 'Degraded', degraded_reason = excluded.degraded_reason",
        params![folder, reason],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Upsert {
    /// Carries the identity D-83 just assigned, because FR-23's notification has to name it.
    Inserted(LocalId),
    Updated,
}

/// `folder` is `None` only for a message a search found in no folder this account knows; a
/// delta always names the folder it is walking.
fn upsert(
    tx: &Transaction<'_>,
    folder: Option<i64>,
    remote: &RemoteMessageId,
    envelope: Option<&Envelope>,
    provenance: Provenance,
    ids: &LocalIdGenerator,
) -> Result<Upsert, IngestError> {
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT id FROM message WHERE remote_id = ?1",
            params![remote.0],
            |r| r.get(0),
        )
        .ok();

    let local = match &existing {
        Some(bytes) => {
            let key: [u8; 16] = bytes.as_slice().try_into().unwrap_or([0; 16]);
            LocalId::from_bytes(key)
        }
        None => ids.next(),
    };

    if let Some(envelope) = envelope {
        let thread = thread_for(tx, envelope, local)?;
        let digest = join::digest(envelope);
        // Normalized once, here, before anything indexes or renders it — NFR-54. The store's
        // copy and the index's copy are the same string because they come from the same
        // call, which is the coupling D-81 requires be asserted rather than assumed.
        let subject = envelope
            .subject
            .as_deref()
            .map(|s| normalize::for_display(s).as_str().to_owned());
        let snippet = envelope
            .snippet
            .as_deref()
            .map(|s| normalize::for_display(s).as_str().to_owned());

        if existing.is_some() {
            // **The provenance is not rewritten.** A message already held keeps the
            // provenance it arrived with: FR-23 defines new mail as delivered-and-unread at
            // the moment of delivery, and a later discovery of the same message is not a
            // second arrival.
            tx.execute(
                "UPDATE message SET
                    internet_message_id = ?2, fallback_digest = ?3, digest_rule_version = ?4,
                    thread_id = ?5, received_at_millis = ?6, origination_millis = ?7,
                    sender = ?8, recipients = ?9, subject = ?10, snippet = ?11,
                    flags = ?12, size_bytes = ?13
                 WHERE id = ?1",
                params![
                    local.to_bytes().to_vec(),
                    envelope.internet_message_id,
                    digest.to_vec(),
                    join::DIGEST_RULE_VERSION,
                    thread.to_bytes().to_vec(),
                    i64::try_from(envelope.received_at_millis).unwrap_or(i64::MAX),
                    envelope
                        .origination_date_millis
                        .map(|m| i64::try_from(m).unwrap_or(i64::MAX)),
                    envelope.from.clone().unwrap_or_default(),
                    envelope.to.join(", "),
                    subject,
                    snippet,
                    flags::word(envelope.read, envelope.flagged),
                    i64::try_from(envelope.size_estimate).unwrap_or(i64::MAX),
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO message (
                    id, remote_id, internet_message_id, fallback_digest, digest_rule_version,
                    thread_id, received_at_millis, origination_millis, sender, recipients,
                    subject, snippet, flags, size_bytes, provenance)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    local.to_bytes().to_vec(),
                    remote.0,
                    envelope.internet_message_id,
                    digest.to_vec(),
                    join::DIGEST_RULE_VERSION,
                    thread.to_bytes().to_vec(),
                    i64::try_from(envelope.received_at_millis).unwrap_or(i64::MAX),
                    envelope
                        .origination_date_millis
                        .map(|m| i64::try_from(m).unwrap_or(i64::MAX)),
                    envelope.from.clone().unwrap_or_default(),
                    envelope.to.join(", "),
                    subject,
                    snippet,
                    flags::word(envelope.read, envelope.flagged),
                    i64::try_from(envelope.size_estimate).unwrap_or(i64::MAX),
                    provenance_name(provenance),
                ],
            )?;
        }
        write_tags(tx, local, &envelope.tags)?;
    } else if existing.is_none() {
        // A presence with no envelope. The message exists and nothing is known about it yet;
        // recording it as present is honest, and inventing content for it would not be.
        tx.execute(
            "INSERT INTO message (id, remote_id, fallback_digest, digest_rule_version,
                                  received_at_millis, sender, recipients, provenance)
             VALUES (?1, ?2, ?3, ?4, 0, '', '', ?5)",
            params![
                local.to_bytes().to_vec(),
                remote.0,
                vec![0u8; 32],
                join::DIGEST_RULE_VERSION,
                provenance_name(provenance),
            ],
        )?;
    }

    if let Some(folder) = folder {
        tx.execute(
            "INSERT OR IGNORE INTO message_location (message_id, folder_id) VALUES (?1, ?2)",
            params![local.to_bytes().to_vec(), folder],
        )?;
    }

    Ok(if existing.is_some() {
        Upsert::Updated
    } else {
        Upsert::Inserted(local)
    })
}

/// D-103: a thread has local identity, assigned at creation.
///
/// Where the provider offers a conversation identifier, FR-11 prefers it — so a message
/// whose conversation is already known joins the existing thread, and the identity that
/// survives is the one that was there. Threads never split.
fn thread_for(
    tx: &Transaction<'_>,
    envelope: &Envelope,
    fallback: LocalId,
) -> Result<LocalId, IngestError> {
    let Some(remote) = envelope.thread_id.as_deref() else {
        return Ok(fallback);
    };
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT id FROM thread WHERE remote_thread_id = ?1",
            params![remote],
            |r| r.get(0),
        )
        .ok();
    if let Some(bytes) = existing {
        let key: [u8; 16] = bytes.as_slice().try_into().unwrap_or([0; 16]);
        let id = LocalId::from_bytes(key);
        tx.execute(
            "UPDATE thread SET
                last_activity_millis = max(last_activity_millis, ?2),
                message_count = message_count + 1
             WHERE id = ?1",
            params![
                bytes,
                i64::try_from(envelope.received_at_millis).unwrap_or(i64::MAX)
            ],
        )?;
        return Ok(id);
    }
    let subject = envelope
        .subject
        .as_deref()
        .map(|s| normalize::for_index(s).as_str().to_owned());
    tx.execute(
        "INSERT INTO thread (id, remote_thread_id, normalized_subject,
                             last_activity_millis, message_count)
         VALUES (?1, ?2, ?3, ?4, 1)",
        params![
            fallback.to_bytes().to_vec(),
            remote,
            subject,
            i64::try_from(envelope.received_at_millis).unwrap_or(i64::MAX)
        ],
    )?;
    Ok(fallback)
}

fn write_tags(tx: &Transaction<'_>, message: LocalId, tags: &[String]) -> Result<(), IngestError> {
    let key = message.to_bytes().to_vec();
    tx.execute(
        "DELETE FROM message_tag WHERE message_id = ?1",
        params![key],
    )?;
    for tag in tags {
        let name = normalize::for_display(tag).as_str().to_owned();
        tx.execute(
            "INSERT OR IGNORE INTO tag (name) VALUES (?1)",
            params![name],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM tag WHERE name = ?1", params![name], |r| {
            r.get(0)
        })?;
        tx.execute(
            "INSERT OR IGNORE INTO message_tag (message_id, tag_id) VALUES (?1, ?2)",
            params![key, id],
        )?;
    }
    Ok(())
}

/// Take a message out of one folder — and out of the store only where that was its last.
///
/// A message that is still in another folder is **not** deleted: on a provider whose
/// cardinality is one-or-more, leaving the inbox is an archive rather than a disappearance,
/// and deleting the row would lose a message the user can still see everywhere else.
///
/// Where `hold` is set — a [`MoveWindow`] that holds — a message leaving its last folder is
/// not deleted either: it keeps its row, loses its dead remote identifier, and waits for
/// [`settle_moves`] to rejoin it to an arrival or delete it.
fn remove_from(
    tx: &Transaction<'_>,
    folder: i64,
    remote: &RemoteMessageId,
    hold: bool,
) -> Result<usize, IngestError> {
    let Ok(id) = tx.query_row::<Vec<u8>, _, _>(
        "SELECT id FROM message WHERE remote_id = ?1",
        params![remote.0],
        |r| r.get(0),
    ) else {
        return Ok(0);
    };
    tx.execute(
        "DELETE FROM message_location WHERE message_id = ?1 AND folder_id = ?2",
        params![id, folder],
    )?;
    let remaining: i64 = tx.query_row(
        "SELECT count(*) FROM message_location WHERE message_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    if remaining == 0 {
        if hold {
            tx.execute(
                "UPDATE message SET remote_id = NULL WHERE id = ?1",
                params![id],
            )?;
        } else {
            tx.execute("DELETE FROM message WHERE id = ?1", params![id])?;
        }
    }
    Ok(1)
}

const fn provenance_name(p: Provenance) -> &'static str {
    match p {
        Provenance::Delivered => "Delivered",
        Provenance::Discovered => "Discovered",
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

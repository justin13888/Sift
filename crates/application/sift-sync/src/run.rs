//! Driving one account's sync: the loop that ties an adapter to a store.
//!
//! It is generic over [`Adapter`] and knows no provider — `cargo xtask invariants` enforces
//! that, and D-12 is why: a driver that special-cased one provider would have to be revisited
//! for the next three.
//!
//! # What this is not
//!
//! It is not a scheduler. Nothing here sleeps, retries, or decides when to run again; a
//! failure comes back classified, and D-25's wheel decides what to do with it. That is the
//! same split D-87 draws for a stated delay, applied to every other failure too.

use crate::ingest::{self, IngestError, PageReport};
use sift_foundation::identity::LocalIdGenerator;
use sift_provider::adapter::{Adapter, Change, Failure, RemoteFolderId, RemoteMessageId};
use sift_store::account::Account;

/// What went wrong running a turn.
#[derive(Debug)]
pub enum RunError {
    /// The provider failed, classified by the adapter that produced it.
    Provider {
        failure: Failure,
        said: String,
    },
    Store(IngestError),
}

impl From<IngestError> for RunError {
    fn from(e: IngestError) -> Self {
        Self::Store(e)
    }
}

impl core::fmt::Display for RunError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Provider { failure, said } => write!(f, "{said} ({failure:?})"),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

fn provider<A: Adapter>(error: A::Error) -> RunError
where
    A::Error: core::fmt::Display,
{
    RunError::Provider {
        failure: A::classify(&error),
        said: error.to_string(),
    }
}

/// Enumerate folders and reconcile them — D-83.
///
/// # Errors
/// See [`RunError`].
pub fn discover_folders<A: Adapter>(
    adapter: &A,
    account: &Account,
) -> Result<ingest::FolderReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let folders = adapter.enumerate_folders().map_err(provider::<A>)?;
    Ok(ingest::reconcile_folders(&account.store, &folders)?)
}

/// One page of one folder's sync.
///
/// The order is normative and it is the reason this function exists rather than being
/// inlined into a caller:
///
/// 1. Ask the adapter for a page **against the stored cursor**, which on a folder that has
///    never synced is `None` — and the adapter takes its cursor before it walks, per D-82.
/// 2. Fetch envelopes for what the page names, batched to the declared size.
/// 3. Write the rows **and the next cursor in one transaction**.
///
/// # Errors
/// See [`RunError`].
pub fn sync_one_page<A: Adapter>(
    adapter: &A,
    account: &mut Account,
    folder: i64,
    remote: &RemoteFolderId,
    ids: &LocalIdGenerator,
) -> Result<Turn, RunError>
where
    A::Error: core::fmt::Display,
{
    let cursor = ingest::cursor_of(&account.store, folder)?;
    let page = match adapter.delta(remote, cursor.as_ref()) {
        Ok(page) => page,
        Err(e) => {
            let error = provider::<A>(e);
            // The one failure that is progress. D-82 moves the folder to `Invalidated`,
            // which is not a resting state — NFR-18 forbids requiring the user to ask — so
            // the caller is told to recover rather than to back off.
            if let RunError::Provider {
                failure: Failure::CursorInvalidated,
                ..
            } = &error
            {
                ingest::mark_invalidated(&account.store, folder)?;
                return Ok(Turn::Invalidated);
            }
            if let RunError::Provider {
                failure: Failure::Permanent,
                said,
            } = &error
            {
                // NFR-29: surfaced with its reason, never hidden.
                ingest::mark_degraded(&account.store, folder, said)?;
            }
            return Err(error);
        }
    };

    // Only what the page names, and only what is worth asking about. A `Present` for a
    // message already held during a backfill is the ordinary case on a re-walk, and asking
    // for its envelope again would turn a resumed backfill into a full refetch.
    let mut wanted: Vec<RemoteMessageId> = Vec::new();
    for change in &page.changes {
        match change {
            Change::Present { id, .. } => wanted.push(id.clone()),
            Change::FlagsChanged { id } => wanted.push(id.clone()),
            Change::Removed { .. } => {}
        }
    }
    wanted.sort();
    wanted.dedup();
    let present_unknown = ingest::unknown_to_us(&account.store, &wanted)?;
    let flags_changed: Vec<RemoteMessageId> = page
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::FlagsChanged { id } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let mut fetch = present_unknown;
    for id in flags_changed {
        if !fetch.contains(&id) {
            fetch.push(id);
        }
    }

    let batch = adapter.capabilities().batch_size() as usize;
    let mut envelopes = Vec::with_capacity(fetch.len());
    for chunk in fetch.chunks(batch.max(1)) {
        envelopes.extend(adapter.fetch_envelopes(chunk).map_err(provider::<A>)?);
    }

    let report = ingest::apply_page(&mut account.store, folder, &page, &envelopes, ids)?;
    Ok(Turn::Applied {
        report,
        more: page.more,
    })
}

/// What one turn did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    Applied {
        report: PageReport,
        more: bool,
    },
    /// The cursor was refused. The caller recovers rather than backing off.
    Invalidated,
}

/// Walk a folder until it has no more pages.
///
/// `max_pages` bounds one turn so that a first sync of a large mailbox does not monopolise
/// the account — D-53's backfill is resumable precisely so it can be stopped, and L-26 is
/// the granularity it rewinds to.
///
/// # Errors
/// See [`RunError`].
pub fn sync_folder<A: Adapter>(
    adapter: &A,
    account: &mut Account,
    folder: i64,
    remote: &RemoteFolderId,
    ids: &LocalIdGenerator,
    max_pages: usize,
) -> Result<PageReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let mut total = PageReport::default();
    for _ in 0..max_pages {
        match sync_one_page(adapter, account, folder, remote, ids)? {
            Turn::Applied { report, more } => {
                total.inserted += report.inserted;
                total.updated += report.updated;
                total.removed += report.removed;
                total.delivered += report.delivered;
                if !more {
                    break;
                }
            }
            Turn::Invalidated => {
                // D-84: a fresh cursor, a re-enumeration, and reconciliation. The stored
                // cursor is cleared only by the recovery that replaces it, which is why the
                // next turn starts from `None` and takes a new one before walking.
                clear_cursor(account, folder)?;
                break;
            }
        }
    }
    Ok(total)
}

/// Drop a folder's cursor so the next turn takes a fresh one.
///
/// **D-84's recovery marks everything it observes as `Discovered`, including rows it
/// inserts.** That falls out of the adapter's own rule — a backfill discovers and only a
/// delta delivers — so a recovery cannot announce a mailbox the user has already read.
///
/// # Errors
/// See [`RunError`].
pub fn clear_cursor(account: &Account, folder: i64) -> Result<(), RunError> {
    account
        .store
        .execute(
            "UPDATE folder_sync_state SET cursor = NULL, state = 'Recovering' WHERE folder_id = ?1",
            rusqlite::params![folder],
        )
        .map_err(|e| RunError::Store(IngestError::Store(e)))?;
    Ok(())
}

/// Sync every folder FR-43 says to watch.
///
/// # Errors
/// See [`RunError`].
pub fn sync_account<A: Adapter>(
    adapter: &A,
    account: &mut Account,
    ids: &LocalIdGenerator,
    max_pages_per_folder: usize,
) -> Result<PageReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let watched = ingest::watched_folders(&account.store)?;
    let mut total = PageReport::default();
    for (folder, remote) in watched {
        let report = sync_folder(adapter, account, folder, &remote, ids, max_pages_per_folder)?;
        total.inserted += report.inserted;
        total.updated += report.updated;
        total.removed += report.removed;
        total.delivered += report.delivered;
    }
    Ok(total)
}

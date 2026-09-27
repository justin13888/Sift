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
//!
//! # Where the store is reached from — D-122
//!
//! Every read and write of the account goes through a [`Hold`], and **no provider call is made
//! inside one**. A caller that keeps the account behind a lock the shell's loop also takes
//! releases it for every round trip, so a gesture waits for a store write rather than for a
//! provider. Between two held steps the account may have been removed; the hold then answers
//! [`None`], and the walk stops with [`RunError::Gone`] having written nothing further.

use crate::ingest::{self, IngestError, MoveWindow, PageReport};
use sift_foundation::identity::LocalIdGenerator;
use sift_provider::adapter::{Adapter, Change, Failure, RemoteFolderId, RemoteMessageId};
use sift_store::account::Account;

/// How the driver reaches the account's store — D-122.
///
/// Each call is one short, held step: the closure touches the store and nothing else. It is a
/// trait rather than a `&mut Account` because the caller that matters holds the account inside
/// the application, behind a lock, and must be able to let go of it between steps.
pub trait Hold {
    /// Run `step` against the account, or answer `None` where the account no longer exists.
    fn with<R>(&mut self, step: impl FnOnce(&mut Account, &LocalIdGenerator) -> R) -> Option<R>;
}

/// An account the caller holds outright, for the whole walk.
///
/// What a caller with no lock to release uses — the harness, and the driver's own tests.
#[derive(Debug)]
pub struct Direct<'a> {
    pub account: &'a mut Account,
    pub ids: &'a LocalIdGenerator,
}

impl Hold for Direct<'_> {
    fn with<R>(&mut self, step: impl FnOnce(&mut Account, &LocalIdGenerator) -> R) -> Option<R> {
        Some(step(self.account, self.ids))
    }
}

/// One held step whose store work can fail.
fn held<H: Hold + ?Sized, R>(
    hold: &mut H,
    step: impl FnOnce(&mut Account, &LocalIdGenerator) -> Result<R, IngestError>,
) -> Result<R, RunError> {
    hold.with(step)
        .ok_or(RunError::Gone)?
        .map_err(RunError::Store)
}

/// What went wrong running a turn.
#[derive(Debug)]
pub enum RunError {
    /// The provider failed, classified by the adapter that produced it.
    Provider {
        failure: Failure,
        said: String,
    },
    Store(IngestError),
    /// The account was removed between two held steps. Nothing after the removal was written.
    Gone,
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
            Self::Gone => write!(f, "the account was removed while it was syncing"),
        }
    }
}

fn provider<A: Adapter + ?Sized>(adapter: &A, error: A::Error) -> RunError
where
    A::Error: core::fmt::Display,
{
    RunError::Provider {
        failure: adapter.classify(&error),
        said: error.to_string(),
    }
}

/// Enumerate folders and reconcile them — D-83.
///
/// # Errors
/// See [`RunError`].
pub fn discover_folders<A: Adapter + ?Sized>(
    adapter: &A,
    account: &Account,
) -> Result<ingest::FolderReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let folders = adapter
        .enumerate_folders()
        .map_err(|e| provider(adapter, e))?;
    Ok(ingest::reconcile_folders(&account.store, &folders)?)
}

/// [`discover_folders`] through a [`Hold`]: the enumeration is not held, the reconciliation is.
///
/// # Errors
/// See [`RunError`].
pub fn discover_folders_held<A: Adapter + ?Sized, H: Hold + ?Sized>(
    adapter: &A,
    hold: &mut H,
) -> Result<ingest::FolderReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let folders = adapter
        .enumerate_folders()
        .map_err(|e| provider(adapter, e))?;
    held(hold, |account, _| {
        ingest::reconcile_folders(&account.store, &folders)
    })
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
pub fn sync_one_page<A: Adapter + ?Sized>(
    adapter: &A,
    account: &mut Account,
    folder: i64,
    remote: &RemoteFolderId,
    ids: &LocalIdGenerator,
) -> Result<Turn, RunError>
where
    A::Error: core::fmt::Display,
{
    sync_one_page_held(adapter, &mut Direct { account, ids }, folder, remote)
}

/// [`sync_one_page`] through a [`Hold`] — D-122.
///
/// The three steps are held separately and the two round trips between them are not: the
/// cursor and what is already known are read under one hold each, and the page, its rows and
/// the next cursor are written under a third, in one transaction as before. The cursor read
/// first is still the one the page is applied against, because nothing else writes it while
/// this walk has the account's adapter.
///
/// # Errors
/// See [`RunError`].
pub fn sync_one_page_held<A: Adapter + ?Sized, H: Hold + ?Sized>(
    adapter: &A,
    hold: &mut H,
    folder: i64,
    remote: &RemoteFolderId,
) -> Result<Turn, RunError>
where
    A::Error: core::fmt::Display,
{
    one_page(adapter, hold, folder, remote, &mut MoveWindow::default())
}

fn one_page<A: Adapter + ?Sized, H: Hold + ?Sized>(
    adapter: &A,
    hold: &mut H,
    folder: i64,
    remote: &RemoteFolderId,
    window: &mut MoveWindow,
) -> Result<Turn, RunError>
where
    A::Error: core::fmt::Display,
{
    let cursor = held(hold, |account, _| ingest::cursor_of(&account.store, folder))?;
    let page = match adapter.delta(remote, cursor.as_ref()) {
        Ok(page) => page,
        Err(e) => {
            let error = provider(adapter, e);
            // The one failure that is progress. D-82 moves the folder to `Invalidated`,
            // which is not a resting state — NFR-18 forbids requiring the user to ask — so
            // the caller is told to recover rather than to back off.
            if let RunError::Provider {
                failure: Failure::CursorInvalidated,
                ..
            } = &error
            {
                held(hold, |account, _| {
                    ingest::mark_invalidated(&account.store, folder)
                })?;
                return Ok(Turn::Invalidated);
            }
            if let RunError::Provider {
                failure: Failure::Permanent,
                said,
            } = &error
            {
                // NFR-29: surfaced with its reason, never hidden.
                held(hold, |account, _| {
                    ingest::mark_degraded(&account.store, folder, said)
                })?;
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
    let present_unknown = held(hold, |account, _| {
        ingest::unknown_to_us(&account.store, &wanted)
    })?;
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
        envelopes.extend(
            adapter
                .fetch_envelopes(chunk)
                .map_err(|e| provider(adapter, e))?,
        );
    }

    let report = held(hold, |account, ids| {
        ingest::apply_page_within(&mut account.store, folder, &page, &envelopes, ids, window)
    })?;
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
pub fn sync_folder<A: Adapter + ?Sized>(
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
    sync_folder_held(
        adapter,
        &mut Direct { account, ids },
        folder,
        remote,
        max_pages,
    )
}

/// [`sync_folder`] through a [`Hold`] — D-122.
///
/// # Errors
/// See [`RunError`].
pub fn sync_folder_held<A: Adapter + ?Sized, H: Hold + ?Sized>(
    adapter: &A,
    hold: &mut H,
    folder: i64,
    remote: &RemoteFolderId,
    max_pages: usize,
) -> Result<PageReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let mut window = MoveWindow::for_account(adapter.capabilities().identifier_survives_a_move());
    let walked = walk_folder(adapter, hold, folder, remote, max_pages, &mut window);
    settled(hold, &mut window, walked)
}

/// Close a window whatever the round came to. A round that failed part-way still settles
/// what it applied, so a departure whose arrival was applied before the failure is rejoined
/// now rather than deleted by the next round, which will not see that arrival again. The
/// round's own error is the one returned.
///
/// A window that does not hold has nothing to settle, and takes no held step for it.
fn settled<H: Hold + ?Sized>(
    hold: &mut H,
    window: &mut MoveWindow,
    walked: Result<PageReport, RunError>,
) -> Result<PageReport, RunError> {
    if !window.holds() {
        return walked;
    }
    match walked {
        Ok(mut total) => {
            held(hold, |account, _| {
                ingest::settle_moves(&mut account.store, window, &mut total)
            })?;
            Ok(total)
        }
        Err(error) => {
            let _ = hold.with(|account, _| {
                ingest::settle_moves(&mut account.store, window, &mut PageReport::default())
            });
            Err(error)
        }
    }
}

fn walk_folder<A: Adapter + ?Sized, H: Hold + ?Sized>(
    adapter: &A,
    hold: &mut H,
    folder: i64,
    remote: &RemoteFolderId,
    max_pages: usize,
    window: &mut MoveWindow,
) -> Result<PageReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let mut total = PageReport::default();
    for _ in 0..max_pages {
        match one_page(adapter, hold, folder, remote, window)? {
            Turn::Applied { report, more } => {
                total.absorb(&report);
                if !more {
                    break;
                }
            }
            Turn::Invalidated => {
                // D-84: a fresh cursor, a re-enumeration, and reconciliation. The stored
                // cursor is cleared only by the recovery that replaces it, which is why the
                // next turn starts from `None` and takes a new one before walking.
                hold.with(|account, _| clear_cursor(account, folder))
                    .ok_or(RunError::Gone)??;
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
pub fn sync_account<A: Adapter + ?Sized>(
    adapter: &A,
    account: &mut Account,
    ids: &LocalIdGenerator,
    max_pages_per_folder: usize,
) -> Result<PageReport, RunError>
where
    A::Error: core::fmt::Display,
{
    sync_account_held(adapter, &mut Direct { account, ids }, max_pages_per_folder)
}

/// [`sync_account`] through a [`Hold`] — D-122.
///
/// # Errors
/// See [`RunError`].
pub fn sync_account_held<A: Adapter + ?Sized, H: Hold + ?Sized>(
    adapter: &A,
    hold: &mut H,
    max_pages_per_folder: usize,
) -> Result<PageReport, RunError>
where
    A::Error: core::fmt::Display,
{
    let watched = held(hold, |account, _| ingest::watched_folders(&account.store))?;
    let mut window = MoveWindow::for_account(adapter.capabilities().identifier_survives_a_move());
    let mut walk = || {
        let mut total = PageReport::default();
        for (folder, remote) in &watched {
            let report = walk_folder(
                adapter,
                hold,
                *folder,
                remote,
                max_pages_per_folder,
                &mut window,
            )?;
            total.absorb(&report);
        }
        Ok(total)
    };
    let walked = walk();
    settled(hold, &mut window, walked)
}

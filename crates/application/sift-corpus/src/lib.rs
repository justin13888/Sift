//! NFR-26's scale corpus: the population every performance and resource target is measured
//! against.
//!
//! `docs/product/reference-environment.md` fixes its shape — about five accounts, 500,000
//! messages, 50,000 of them in an inbox — and makes synthetic generation acceptable. This crate
//! is that generation.
//!
//! # What it writes, and through what
//!
//! **An installation container the application opens at launch**, not loose database files.
//! NFR-1's cold start is defined as a launch "with the store already populated from the scale
//! corpus", so the corpus has to be where a launch looks: a sealed registry naming every
//! account, each account's two sealed files under a key from the credential store, exactly as
//! adding an account writes them.
//!
//! **Every message goes through sync's own ingest** — [`sift_sync::ingest::apply_page`], a page
//! of L-26's size at a time, each page and its cursor in one transaction. Rows written by a
//! generator's own statements would look like the product's and not be them: a different
//! normalization, a missing thread row, a cursor that was never advanced. Every figure measured
//! against such a store would describe that difference. When the last page of a folder lands,
//! its sync state is `Live`, which is the reference environment's idle: every account
//! synchronized and quiescent.
//!
//! # What it does not do
//!
//! Bodies are not written. D-81 indexes body text at first fetch, and a corpus of never-read
//! mail is what a fresh first sync produces; the envelope is the unit L-20's budget and NFR-5's
//! population are both stated in. Content is deterministic from the seed; **local identities are
//! not**, because D-78 makes them Sift's own and time-ordered at ingest, and a generator that
//! chose them would be testing a store the product cannot produce.

mod content;
pub mod shape;

use rusqlite::params;
use sift_app::container::{self, Container};
use sift_credentials::store::CredentialStore;
use sift_foundation::identity::{AccountId, LocalIdGenerator};
use sift_foundation::limits::L26_BACKFILL_PAGE;
use sift_index::ingest::Document;
use sift_provider::adapter::{
    Change, Cursor, Delta, Provenance, RemoteFolder, RemoteFolderId, RemoteMessageId, SpecialUse,
};
use sift_store::account::{Account, AccountPaths};
use std::path::Path;

pub use shape::{AccountPlan, FolderPlan, Shape, ShapeError};

/// The kind every corpus account is registered as.
///
/// A capability shape rather than a provider kind, so that a launch reconnects it to nothing:
/// the reference environment's idle excludes network events, and an account with no adapter
/// has none to make. It is the shape adding an account by shape records, so the application
/// restores its capability set the same way.
pub const KIND: &str = "rich";

/// What a run is asked to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub shape: Shape,
    /// Names the content. Two runs with one seed write the same mail.
    pub seed: u64,
    /// The newest a message may be. Fixed rather than read from the clock, so a seed names
    /// one corpus rather than one per day it is generated on.
    pub anchor_millis: u64,
}

impl Options {
    /// 2026-01-01T00:00:00Z.
    pub const DEFAULT_ANCHOR_MILLIS: u64 = 1_767_225_600_000;
    pub const DEFAULT_SEED: u64 = 22;

    #[must_use]
    pub const fn scale() -> Self {
        Self {
            shape: Shape::SCALE,
            seed: Self::DEFAULT_SEED,
            anchor_millis: Self::DEFAULT_ANCHOR_MILLIS,
        }
    }
}

/// Why a run stopped.
#[derive(Debug)]
pub enum CorpusError {
    Shape(ShapeError),
    /// The container already holds accounts. A corpus written beside a person's own mail would
    /// be a measurement of neither, and a generator that cleared it first would be a deletion
    /// nobody asked for.
    NotEmpty {
        accounts: usize,
    },
    /// The container, the credential store, or an account's files refused.
    Container(String),
    Ingest(sift_sync::ingest::IngestError),
    Store(rusqlite::Error),
}

impl core::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Shape(e) => write!(f, "{e}"),
            Self::NotEmpty { accounts } => write!(
                f,
                "the container already holds {accounts} account(s); a corpus needs an empty one"
            ),
            Self::Container(e) => write!(f, "{e}"),
            Self::Ingest(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CorpusError {}

impl From<ShapeError> for CorpusError {
    fn from(e: ShapeError) -> Self {
        Self::Shape(e)
    }
}
impl From<sift_sync::ingest::IngestError> for CorpusError {
    fn from(e: sift_sync::ingest::IngestError) -> Self {
        Self::Ingest(e)
    }
}
impl From<rusqlite::Error> for CorpusError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e)
    }
}

/// How far a run has got, reported once per page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub account: usize,
    pub written: u64,
    pub total: u64,
}

/// What a run wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub accounts: Vec<AccountReport>,
    /// The bytes of token text the envelope index would hold for this population — D-81's
    /// ingest-stage document for every message. The store's file sizes are the other half of
    /// L-20's "two kilobytes per message including its index entry", and this is the half the
    /// store cannot report.
    pub index_token_bytes: u64,
}

impl Report {
    #[must_use]
    pub fn messages(&self) -> u64 {
        self.accounts.iter().map(|a| a.messages).sum()
    }

    /// Every account's two files, on disk.
    #[must_use]
    pub fn file_bytes(&self) -> u64 {
        self.accounts
            .iter()
            .map(|a| a.store_bytes + a.journal_bytes)
            .sum()
    }
}

/// One account's share of what was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountReport {
    pub id: AccountId,
    pub display_name: String,
    pub messages: u64,
    pub inbox: u64,
    pub folders: usize,
    pub store_bytes: u64,
    pub journal_bytes: u64,
}

/// Write the corpus into the container at `root`.
///
/// # Errors
/// The shape cannot be planned, the container already holds accounts, or the container, the
/// credential store, or the store refused.
pub fn generate<S: CredentialStore>(
    root: &Path,
    credentials: &S,
    options: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report, CorpusError> {
    let plan = options.shape.plan()?;
    let mut container = Container::open(root, credentials).map_err(CorpusError::Container)?;
    let existing = container.accounts().map_err(CorpusError::Container)?;
    if !existing.is_empty() {
        return Err(CorpusError::NotEmpty {
            accounts: existing.len(),
        });
    }

    let mut report = Report {
        accounts: Vec::with_capacity(plan.len()),
        index_token_bytes: 0,
    };
    for account in &plan {
        let (written, tokens) = write_account(
            root,
            &mut container,
            credentials,
            options,
            account,
            progress,
        )?;
        report.index_token_bytes += tokens;
        report.accounts.push(written);
    }
    Ok(report)
}

fn write_account<S: CredentialStore>(
    root: &Path,
    container: &mut Container,
    credentials: &S,
    options: &Options,
    plan: &AccountPlan,
    progress: &mut dyn FnMut(Progress),
) -> Result<(AccountReport, u64), CorpusError> {
    let registered = container
        .register(KIND, &plan.display_name)
        .map_err(CorpusError::Container)?;
    let owner =
        container::account_secret(credentials, registered.id).map_err(CorpusError::Container)?;
    let paths = AccountPaths::under(root, registered.id);
    let mut account = Account::open_sealed(&paths, registered.id, &owner)
        .map_err(|e| CorpusError::Container(e.to_string()))?;

    // The account row adding an account by shape writes, so a launch restores the same
    // capability set from it.
    account.store.execute(
        "INSERT INTO account (id, ordinal, display_name, capabilities) VALUES (?1, ?2, ?3, ?4)",
        params![
            registered.id.as_u128().to_be_bytes().to_vec(),
            i64::from(registered.ordinal.get()),
            plan.display_name,
            KIND.as_bytes().to_vec()
        ],
    )?;

    let remote: Vec<RemoteFolder> = plan
        .folders
        .iter()
        .map(|f| RemoteFolder {
            id: RemoteFolderId(f.remote_id.clone()),
            display_name: f.display_name.clone(),
            special_use: f.special_use,
        })
        .collect();
    sift_sync::ingest::reconcile_folders(&account.store, &remote)?;

    let ids = LocalIdGenerator::new(registered.ordinal);
    let mut writer = content::Writer::new(
        options.seed,
        plan.index,
        &plan.address,
        options.anchor_millis,
    );
    let total = plan.messages();
    let mut written = 0u64;
    let mut tokens = 0u64;

    for folder in &plan.folders {
        let remote_id = RemoteFolderId(folder.remote_id.clone());
        let local = sift_sync::ingest::folder_local_id(&account.store, &remote_id)?;
        // Every folder the corpus fills was synchronized, so every one is in FR-43's watched
        // set: the reference environment's idle is every account synchronized and quiescent,
        // and a filled folder nobody watches is a state no sync produces.
        sift_sync::ingest::set_watched(&account.store, local, true)?;
        let sent = folder.special_use == Some(SpecialUse::Sent);

        let mut remaining = folder.messages;
        let mut page_number = 0u64;
        loop {
            let this_page = remaining.min(L26_BACKFILL_PAGE);
            let envelopes: Vec<_> = (0..this_page)
                .map(|_| writer.envelope(&remote_id, sent))
                .collect();
            for e in &envelopes {
                tokens += index_bytes(e);
            }
            remaining -= this_page;
            page_number += 1;
            let page = Delta {
                changes: envelopes
                    .iter()
                    .map(|e| Change::Present {
                        id: RemoteMessageId(e.id.0.clone()),
                        // A backfill discovers; only a delta delivers.
                        provenance: Provenance::Discovered,
                    })
                    .collect(),
                next: Cursor(page_number.to_be_bytes().to_vec()),
                more: remaining > 0,
            };
            sift_sync::ingest::apply_page(&mut account.store, local, &page, &envelopes, &ids)?;
            written += this_page;
            progress(Progress {
                account: plan.index,
                written,
                total,
            });
            // An empty folder still gets its one page: its cursor is what says it synced.
            if remaining == 0 {
                break;
            }
        }
    }

    drop(account);
    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    Ok((
        AccountReport {
            id: registered.id,
            display_name: plan.display_name.clone(),
            messages: total,
            inbox: plan.inbox(),
            folders: plan.folders.len(),
            store_bytes: size(&paths.store),
            journal_bytes: size(&paths.journal),
        },
        tokens,
    ))
}

/// The token text D-81 would index for this envelope at ingest.
fn index_bytes(e: &sift_provider::adapter::Envelope) -> u64 {
    let doc = Document::at_ingest(
        e.subject.as_deref().unwrap_or(""),
        e.from.as_deref().unwrap_or(""),
        &e.to.join(", "),
        &[],
    );
    [&doc.subject, &doc.sender, &doc.recipients]
        .iter()
        .flat_map(|t| t.iter())
        .map(|t| t.text.len() as u64)
        .sum()
}

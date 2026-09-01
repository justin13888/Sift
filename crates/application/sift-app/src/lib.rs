//! The assembled application: accounts, their stores, their queues, and the adapters behind
//! them.
//!
//! # Why this is not in a shell any more
//!
//! This was `sift-harness`'s `app` module, and it could not have been anywhere else. It holds
//! an adapter, and until the error type was erased an adapter could only be held by naming
//! the provider that defines its error — which D-12 forbids in `application`, `presentation`
//! and `abi`, and `cargo xtask invariants` enforces. A shell was the one layer left.
//!
//! That was survivable while the harness was the only shell. It stops being survivable for
//! the macOS shell, which is Swift: it cannot construct a Rust adapter, so it cannot assemble
//! the application, so the application has to already exist below the boundary for it to
//! reach. Two shells assembling their own would also be two applications, which is the drift
//! `docs/architecture/shell-boundary.md` calls a defect.
//!
//! It holds no window, no selection and no observation. Those are the presentation layer's,
//! which sits above this one.

pub mod rows;

use sift_foundation::identity::{AccountId, AccountOrdinal, LocalId, LocalIdGenerator};
use sift_mutations::queue::Queue;
use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
    TrashSemantics,
};
use sift_provider::erased::ErasedAdapter;
use sift_store::account::{Account, AccountPaths};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The adapter an account is reached through.
///
/// Its error is erased, which is what lets this crate hold one at all: a
/// `Box<dyn Adapter<Error = E>>` would force this layer to name `E`, and naming `E` names the
/// provider D-12 forbids it from knowing.
pub type Live = Box<dyn ErasedAdapter>;

/// Insert an ingested message, and put it in the inbox.
///
/// Locations are a relation rather than a column, because D-12 makes cardinality a declared
/// capability: a message may be in one folder or several.
pub fn insert_message(
    account: &OpenAccount,
    id: LocalId,
    subject: &str,
    received_millis: u64,
) -> Result<(), String> {
    let key = id.to_bytes().to_vec();
    account
        .store
        .store
        .execute(
            "INSERT INTO message (id, fallback_digest, digest_rule_version,
                                  received_at_millis, sender, recipients, subject, provenance)
             VALUES (?1, ?2, 1, ?3, 'someone@example.test', '', ?4, 'Delivered')",
            rusqlite::params![
                key,
                vec![0u8; 32],
                i64::try_from(received_millis).unwrap_or(0),
                subject
            ],
        )
        .map_err(|e| e.to_string())?;
    account
        .store
        .store
        .execute(
            "INSERT INTO message_location (message_id, folder_id) VALUES (?1, 1)",
            rusqlite::params![id.to_bytes().to_vec()],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read the list back out of the store, in D-55's order.
///
/// **Received time, tiebroken on local identity** — and the same comparator has to serve
/// this query and the in-memory unified-inbox merge, because D-4 assembles that merge from
/// streams this produces.
pub fn list_messages(account: &OpenAccount) -> Result<Vec<(LocalId, String)>, String> {
    let mut stmt = account
        .store
        .store
        .prepare(
            "SELECT id, subject FROM message
             ORDER BY received_at_millis DESC, id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let key: Vec<u8> = r.get(0)?;
            let subject: Option<String> = r.get(1)?;
            Ok((key, subject.unwrap_or_default()))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (key, subject) = row.map_err(|e| e.to_string())?;
        let bytes: [u8; 16] = key.try_into().map_err(|_| "identity is not 16 bytes")?;
        out.push((LocalId::from_bytes(bytes), subject));
    }
    Ok(out)
}

/// A message's and a folder's provider identifiers, read out of the store.
///
/// The mutation queue cannot reach the store — they sit side by side in the application
/// layer and D-59 gives neither an edge to the other — so the lookup crosses as a trait the
/// shell implements. That is the same reason the durable `Issued` marker crosses as a
/// callback rather than being something the flush does for itself.
pub struct Remote<'a>(pub &'a rusqlite::Connection);

impl std::fmt::Debug for Remote<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A connection has no useful debug form and printing one is how a path or a
        // statement ends up in a log NFR-23 says nothing may reach.
        f.write_str("Remote(..)")
    }
}

impl sift_mutations::flush::Resolve for Remote<'_> {
    fn message(&self, local: LocalId) -> Option<sift_provider::adapter::RemoteMessageId> {
        self.0
            .query_row(
                "SELECT remote_id FROM message WHERE id = ?1",
                rusqlite::params![local.to_bytes().to_vec()],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .map(sift_provider::adapter::RemoteMessageId)
    }

    fn folder(&self, local: i64) -> Option<sift_provider::adapter::RemoteFolderId> {
        self.0
            .query_row(
                "SELECT remote_id FROM folder WHERE id = ?1",
                rusqlite::params![local],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .map(sift_provider::adapter::RemoteFolderId)
    }
}

/// One account, as the harness holds it.
pub struct OpenAccount {
    pub id: AccountId,
    pub capabilities: Capabilities,
    pub store: Account,
    pub queue: Queue,
    pub ids: LocalIdGenerator,
    /// The adapter, where this account has one.
    ///
    /// `None` is the capability-shape account: a store and a queue with no provider behind
    /// them, which is what the planner tests are driven against. **Nothing below the
    /// `account` command knows which provider a live one is** — that question is asked once,
    /// when the user adds it, and after that every command plans against what the account
    /// declares.
    pub adapter: Option<Live>,
    /// The subject of each ingested message, so the harness can print something a person
    /// recognises. A shell would read this from the store; keeping it here keeps the
    /// harness's own printing out of the store's query paths.
    pub subjects: BTreeMap<LocalId, String>,
}

impl std::fmt::Debug for OpenAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAccount")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

pub struct App {
    accounts: BTreeMap<String, OpenAccount>,
    /// NFR-23's single place, which the shell holds and never reaches into.
    ///
    /// The shell asks it to begin a flow and to complete one; it never reads a token, and
    /// there is no path from here to the credential store that does not go through it.
    pub broker: sift_credentials::oauth::Broker<sift_credentials::store::Platform>,
    /// Authorizations begun but not yet returned from, by account name.
    pub pending_authorization: BTreeMap<String, String>,
    /// D-99: a set keyed on identity with an anchor, cleared by a scope change.
    pub selection: Vec<LocalId>,
    pub open_message: Option<LocalId>,
    pub has_window: bool,
    pub root: Option<PathBuf>,
    next_intent: u128,
    next_ordinal: u16,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("accounts", &self.accounts.len())
            .finish_non_exhaustive()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            accounts: BTreeMap::new(),
            // On a platform with no backend this refuses every write, which is D-71 doing
            // its job: a security absence refuses, and the only fallback available here is a
            // file — exactly what the constraint forbids by name.
            broker: sift_credentials::oauth::Broker::new(sift_credentials::store::Platform),
            pending_authorization: BTreeMap::new(),
            selection: Vec::new(),
            open_message: None,
            has_window: false,
            root: None,
            next_intent: 0,
            next_ordinal: 0,
        }
    }

    /// Add an account with a provider behind it.
    ///
    /// Its capability set comes from the adapter rather than from a name, which is the
    /// difference between this and [`Self::add_account`]: a shape is something a test picks,
    /// and this is what an account actually declares.
    pub fn add_provider_account(&mut self, name: &str, adapter: Live) -> Result<AccountId, String> {
        let capabilities = adapter.capabilities().clone();
        let id = self.create_account(name, capabilities, "provider")?;
        self.accounts
            .get_mut(name)
            .ok_or("the account vanished")?
            .adapter = Some(adapter);
        Ok(id)
    }

    /// The identity an account will be given, so a flow can be keyed on it before the
    /// account's files exist.
    ///
    /// D-89: Sift's own, assigned when the account is added, independent of address and
    /// provider — and **re-adding is a new account**, which is what makes FR-4's erasure
    /// provable by enumeration.
    pub fn reserve_identity(&mut self) -> AccountId {
        AccountId::from_u128(u128::from(self.next_ordinal) + 1)
    }

    fn root(&mut self) -> PathBuf {
        self.root
            .get_or_insert_with(|| {
                // Process identifier **and** the clock. A pid alone is not unique for long:
                // the operating system reuses one within seconds, and a session that landed
                // on a reused directory would open a previous session's account files and
                // fail to insert its own account row — which is a flaky test that looks like
                // a bug in the store.
                let unique = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let d = std::env::temp_dir()
                    .join(format!("sift-harness-{}-{unique}", std::process::id()));
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(&d).expect("scratch root");
                d
            })
            .clone()
    }

    /// Add an account. Its capability set is chosen by name so that a test can exercise the
    /// planner against a shape rather than against a provider.
    pub fn add_account(&mut self, name: &str, shape: &str) -> Result<(), String> {
        let capabilities = shape_named(shape)?;
        self.create_account(name, capabilities, shape)?;
        Ok(())
    }

    fn create_account(
        &mut self,
        name: &str,
        capabilities: Capabilities,
        shape: &str,
    ) -> Result<AccountId, String> {
        if self.accounts.contains_key(name) {
            return Err(format!("account `{name}` already exists"));
        }
        // D-89: the identity is Sift's own, assigned when the account is added, and
        // independent of address and provider. Re-adding is a new account.
        let id = AccountId::from_u128(u128::from(self.next_ordinal) + 1);
        let ordinal = AccountOrdinal::new(self.next_ordinal);
        self.next_ordinal += 1;

        let root = self.root();
        let paths = AccountPaths::under(&root, id);
        let store = Account::open(&paths, id).map_err(|e| e.to_string())?;

        // A real account row and a real inbox, so that ingest writes through the same schema
        // a shell would read. A harness that kept its messages in a map would be a mock, and
        // would prove nothing about the store.
        store
            .store
            .execute(
                "INSERT INTO account (id, ordinal, display_name, capabilities)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    id.as_u128().to_be_bytes().to_vec(),
                    i64::from(ordinal.get()),
                    name,
                    shape.as_bytes().to_vec()
                ],
            )
            .map_err(|e| e.to_string())?;
        // A shape account has no provider to enumerate folders, so it is given the one it
        // needs. **A provider account is not**: D-83 assigns local identity on first
        // discovery, and pre-seeding a folder here would give the enumeration a row it did
        // not create and a remote identifier it did not choose.
        if shape != "provider" {
            store
                .store
                .execute(
                    "INSERT INTO folder (id, remote_id, special_use, display_name, watched)
                     VALUES (1, 'INBOX', 'Inbox', 'Inbox', 1)",
                    [],
                )
                .map_err(|e| e.to_string())?;
        }

        self.accounts.insert(
            name.to_owned(),
            OpenAccount {
                id,
                capabilities,
                store,
                queue: Queue::new(),
                ids: LocalIdGenerator::new(ordinal),
                adapter: None,
                subjects: BTreeMap::new(),
            },
        );
        Ok(id)
    }

    pub fn account(&mut self, name: &str) -> Result<&mut OpenAccount, String> {
        self.accounts
            .get_mut(name)
            .ok_or_else(|| format!("no account `{name}`"))
    }

    #[must_use]
    pub fn account_names(&self) -> Vec<&str> {
        self.accounts.keys().map(String::as_str).collect()
    }

    pub fn accounts(&self) -> impl Iterator<Item = (&String, &OpenAccount)> {
        self.accounts.iter()
    }

    /// The account whose **store** holds a message, by local identity.
    ///
    /// Distinct from [`Self::owner_of`], which answers from the harness's own map of what it
    /// ingested by hand. A synced message was never in that map, and asking the store is the
    /// only way to find it — which is also the only way a real shell could.
    #[must_use]
    pub fn owner_of_stored(&self, message: LocalId) -> Option<String> {
        self.accounts.iter().find_map(|(name, account)| {
            account
                .store
                .store
                .query_row(
                    "SELECT 1 FROM message WHERE id = ?1",
                    rusqlite::params![message.to_bytes().to_vec()],
                    |_| Ok(()),
                )
                .ok()
                .map(|()| name.clone())
        })
    }

    /// The account a message belongs to, by local identity.
    #[must_use]
    pub fn owner_of(&self, message: LocalId) -> Option<&String> {
        self.accounts
            .iter()
            .find(|(_, a)| a.subjects.contains_key(&message))
            .map(|(n, _)| n)
    }

    pub fn next_intent_id(&mut self) -> u128 {
        self.next_intent += 1;
        self.next_intent
    }

    /// D-99: affordances over a selection spanning accounts resolve to the **intersection**
    /// of those accounts' capabilities, not the union.
    ///
    /// The recorded cost is that the intersection silently shrinks available actions as a
    /// selection widens, with nothing saying why.
    #[must_use]
    pub fn selection_capabilities(&self) -> Option<Capabilities> {
        let mut owners: Vec<&OpenAccount> = Vec::new();
        for m in &self.selection {
            // The store rather than the harness's own map of what it ingested by hand: a
            // synced message was never in that map, and a selection over one would resolve
            // to no capabilities at all — every mutation absent, for a reason nothing said.
            let owner = self.accounts.values().find(|a| {
                a.subjects.contains_key(m)
                    || a.store
                        .store
                        .query_row(
                            "SELECT 1 FROM message WHERE id = ?1",
                            rusqlite::params![m.to_bytes().to_vec()],
                            |_| Ok(()),
                        )
                        .is_ok()
            })?;
            if !owners.iter().any(|o| o.id == owner.id) {
                owners.push(owner);
            }
        }
        let mut it = owners.into_iter();
        let first = it.next()?;
        let mut caps = first.capabilities.clone();
        for other in it {
            caps = intersect(&caps, &other.capabilities);
        }
        Some(caps)
    }
}

/// The capability intersection D-99 requires.
///
/// Every row narrows to the weaker of the two, because an affordance the union would offer
/// is one that fails on half the selection.
fn intersect(a: &Capabilities, b: &Capabilities) -> Capabilities {
    Capabilities {
        location_cardinality: if a.location_cardinality == b.location_cardinality {
            a.location_cardinality
        } else {
            LocationCardinality::ExactlyOne
        },
        tag_support: match (a.tag_support, b.tag_support) {
            (TagSupport::ReadWrite, TagSupport::ReadWrite) => TagSupport::ReadWrite,
            (TagSupport::None, _) | (_, TagSupport::None) => TagSupport::None,
            _ => TagSupport::ReadOnly,
        },
        archive: a.archive,
        trash: a.trash,
        permanent_delete: a.permanent_delete && b.permanent_delete,
        thread_operations: if a.thread_operations == b.thread_operations {
            a.thread_operations
        } else {
            ThreadOperations::ClientFanOut
        },
        junk_reporting: match (a.junk_reporting, b.junk_reporting) {
            (JunkReporting::NativeReport, JunkReporting::NativeReport) => {
                JunkReporting::NativeReport
            }
            (JunkReporting::None, _) | (_, JunkReporting::None) => JunkReporting::None,
            _ => JunkReporting::FolderMoveOnly,
        },
        delta: a.delta,
        push: a.push,
        id_stability: a.id_stability,
        server_search: a.server_search && b.server_search,
        // The smaller batch, because the larger would be refused by half the selection.
        max_batch_size: match (a.max_batch_size, b.max_batch_size) {
            (Magnitude::Known { value: x, source }, Magnitude::Known { value: y, .. }) => {
                Magnitude::Known {
                    value: x.min(y),
                    source,
                }
            }
            _ => Magnitude::Unknown,
        },
        request_budget: Magnitude::Unknown,
        snippet_source: a.snippet_source,
        unrecognised: Vec::new(),
    }
}

/// Capability shapes, named by what they *are* rather than by who has them.
///
/// D-12: everything above the adapter layer plans against capabilities, never provider
/// names — so the harness names shapes too. `cargo xtask invariants` would fail this file
/// otherwise, which is the rule doing its job.
fn shape_named(shape: &str) -> Result<Capabilities, String> {
    let base = Capabilities {
        location_cardinality: LocationCardinality::OneOrMore,
        tag_support: TagSupport::ReadWrite,
        archive: ArchiveSemantics::RemoveFromInbox,
        trash: TrashSemantics::MoveToTrash,
        permanent_delete: true,
        thread_operations: ThreadOperations::Native,
        junk_reporting: JunkReporting::NativeReport,
        delta: DeltaMechanism::ChangesQuery,
        push: PushMechanism::EventStream,
        id_stability: IdStability::StableGlobally,
        server_search: true,
        max_batch_size: Magnitude::Unknown,
        request_budget: Magnitude::Unknown,
        snippet_source: SnippetSource::ProviderSupplied,
        unrecognised: Vec::new(),
    };
    Ok(match shape {
        // Everything declared, everything native.
        "rich" => base,
        // One location, no tags, no junk reporting, no efficient delta — the pessimistic end
        // of what a probed account can turn out to be.
        "minimal" => Capabilities {
            location_cardinality: LocationCardinality::ExactlyOne,
            tag_support: TagSupport::None,
            archive: ArchiveSemantics::MoveToSpecialUse,
            trash: TrashSemantics::FlagAndExpunge,
            thread_operations: ThreadOperations::ClientFanOut,
            junk_reporting: JunkReporting::None,
            delta: DeltaMechanism::FullScan,
            push: PushMechanism::PollOnly,
            id_stability: IdStability::StablePerFolder,
            snippet_source: SnippetSource::ClientDerived,
            ..base
        },
        // Identifiers that do not survive a move, which forces D-44's corroborated join.
        "unstable-ids" => Capabilities {
            location_cardinality: LocationCardinality::ExactlyOne,
            id_stability: IdStability::UnstableOnMove,
            delta: DeltaMechanism::DeltaLink,
            push: PushMechanism::PollOnly,
            ..base
        },
        other => {
            return Err(format!(
                "unknown shape `{other}` — try rich, minimal, or unstable-ids"
            ));
        }
    })
}

/// Adding an account with a provider behind it, and walking it.
///
/// These sit here rather than in a shell because **both shells need the same ones**, and a
/// capability that exists for one shell and not the other is a defect in the boundary rather
/// than a feature of that shell.
impl App {
    /// D-65's fixture account: a real adapter over the recorded corpus rather than a socket.
    ///
    /// Not a convenience and not a mock. `docs/build/verification.md` puts the fixture harness
    /// in P0 **before the adapters it tests**, because it is what makes a provider's behaviour
    /// assertable "against servers nobody has" — and it is what lets a shell be driven, and
    /// looked at, with no account and no network at all.
    ///
    /// # Errors
    /// The account could not be created.
    pub fn add_replayed_account(&mut self, name: &str) -> Result<AccountId, String> {
        let descriptor = sift_registry::KINDS
            .first()
            .ok_or("this build has no provider adapters")?;
        self.add_provider_account(name, descriptor.replayed())
    }

    /// Discover folders and walk the delta, in that order.
    ///
    /// Folders first, because a delta needs somewhere to put what it finds and D-83 assigns
    /// local identity on discovery rather than on first use.
    ///
    /// # Errors
    /// The account has no provider behind it, or the walk failed. The adapter is returned to
    /// the account either way — a failed sync must not leave an account unreachable.
    pub fn sync(&mut self, name: &str, pages: usize) -> Result<SyncReport, String> {
        let account = self.account(name)?;
        let adapter = account
            .adapter
            .take()
            .ok_or("this account has no provider behind it")?;

        let outcome = sift_sync::run::discover_folders(adapter.as_ref(), &account.store).and_then(
            |folders| {
                sift_sync::run::sync_account(
                    adapter.as_ref(),
                    &mut account.store,
                    &account.ids,
                    pages,
                )
                .map(|page| SyncReport {
                    discovered: folders.discovered.len(),
                    inserted: page.inserted,
                    updated: page.updated,
                    removed: page.removed,
                    delivered: page.delivered,
                })
            },
        );

        // The adapter goes back before the result is examined. An account whose sync failed
        // is an account in a condition, not one that can never be reached again.
        account.adapter = Some(adapter);
        outcome.map_err(|e| e.to_string())
    }
}

/// What one turn of the sync loop did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncReport {
    pub discovered: usize,
    pub inserted: usize,
    pub updated: usize,
    pub removed: usize,
    /// FR-23's new mail: delivered-and-unread at this moment, and not reconstructable later.
    pub delivered: usize,
}

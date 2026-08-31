//! The application state a shell would hold, minus the window.

use sift_foundation::identity::{AccountId, AccountOrdinal, LocalId, LocalIdGenerator};
use sift_mutations::queue::Queue;
use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
    TrashSemantics,
};
use sift_store::account::{Account, AccountPaths};
use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// One account, as the harness holds it.
pub struct OpenAccount {
    pub id: AccountId,
    pub capabilities: Capabilities,
    pub store: Account,
    pub queue: Queue,
    pub ids: LocalIdGenerator,
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

#[derive(Debug, Default)]
pub struct App {
    accounts: BTreeMap<String, OpenAccount>,
    /// D-99: a set keyed on identity with an anchor, cleared by a scope change.
    pub selection: Vec<LocalId>,
    pub open_message: Option<LocalId>,
    pub has_window: bool,
    pub root: Option<PathBuf>,
    next_intent: u128,
    next_ordinal: u16,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn root(&mut self) -> PathBuf {
        self.root
            .get_or_insert_with(|| {
                let d = std::env::temp_dir().join(format!("sift-harness-{}", std::process::id()));
                std::fs::create_dir_all(&d).expect("scratch root");
                d
            })
            .clone()
    }

    /// Add an account. Its capability set is chosen by name so that a test can exercise the
    /// planner against a shape rather than against a provider.
    pub fn add_account(&mut self, name: &str, shape: &str) -> Result<(), String> {
        if self.accounts.contains_key(name) {
            return Err(format!("account `{name}` already exists"));
        }
        let capabilities = shape_named(shape)?;
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
        store
            .store
            .execute(
                "INSERT INTO folder (id, remote_id, special_use, display_name, watched)
                 VALUES (1, 'INBOX', 'Inbox', 'Inbox', 1)",
                [],
            )
            .map_err(|e| e.to_string())?;

        self.accounts.insert(
            name.to_owned(),
            OpenAccount {
                id,
                capabilities,
                store,
                queue: Queue::new(),
                ids: LocalIdGenerator::new(ordinal),
                subjects: BTreeMap::new(),
            },
        );
        Ok(())
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
            let owner = self
                .accounts
                .values()
                .find(|a| a.subjects.contains_key(m))?;
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

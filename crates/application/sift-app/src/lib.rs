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

pub mod attachment;
pub mod authorize;
pub mod container;
pub mod document;
pub mod rows;
pub mod search;
pub mod settings;

use sift_foundation::condition::AccountCondition;
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

/// What an account registered as a real mailbox is recorded as in the container.
///
/// Persisted, and read back on the next run to decide how to reconnect it. Not a provider
/// name: D-12 keeps those below the adapter layer, and a second provider will resolve through
/// the register rather than by widening this.
pub const PROVIDER_KIND: &str = "provider";

/// The same for D-65's recorded corpus, which reconnects to no network and no credential.
pub const REPLAYED_KIND: &str = "replayed";

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
    /// What this account was registered as, and what a later run reconnects it by.
    ///
    /// `"provider"` reaches a real mailbox through the credential store; `"replayed"` is
    /// D-65's recorded corpus; anything else is a capability shape with no provider at all.
    /// It is persisted in the container registry, because the alternative is a restored
    /// account that opens, shows the mail the last run left, and can never fetch another.
    pub kind: String,
    /// FR-2's re-authentication, latched.
    ///
    /// **Set only on a well-formed provider denial** — D-88's rule, and the whole of NFR-34's
    /// cascade defence: a captive portal answering a refresh with a login page produces an
    /// unparseable response, and treating that as authoritative would put every account into
    /// this state at once and tell a person Sift had lost their credentials when they were on
    /// a hotel network.
    ///
    /// Latched rather than recomputed, because the failure happens inside a refresh nothing
    /// else observes, and the condition has to outlive the call that discovered it.
    pub needs_authentication: bool,
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
    /// Whether Sift may issue this account's mutations to the provider — the read-only
    /// posture.
    ///
    /// **A new account starts `false`.** Triage still works in every respect a person can
    /// see: intents are built, checked against declared capabilities, given their D-85 undo
    /// group, written durably to the journal and applied optimistically to the view. What
    /// does not happen is the one thing that cannot be taken back — the request leaving the
    /// process.
    ///
    /// The point is that a mailbox can be connected, synced, read and triaged *before*
    /// anything is authorized to change it, and the person can look at what Sift intends to
    /// do before it does any of it. It is not a D-49 condition: the conditions table has
    /// *paused by the user*, which means sync stopped, and this is not that — consuming the
    /// account's one condition slot with a posture the user chose would hide a real fault
    /// behind it.
    pub writes_enabled: bool,
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
    /// D-28's capability tokens, and the answers behind them.
    ///
    /// **It outlives any one render.** The body view asks for a document's resources after
    /// the HTML has been handed over, so a broker created per render would answer nothing.
    ///
    /// Named for what it brokers rather than called `broker`, because the field above is the
    /// credential broker and two things called the same word in one struct is how the wrong
    /// one gets passed.
    pub resources: sift_broker::broker::Broker,
    pub selection: Vec<LocalId>,
    pub open_message: Option<LocalId>,
    pub has_window: bool,
    /// D-71 — whether the callback scheme this installation needs is one this shell claims.
    ///
    /// A bundle is what registers a scheme and the layer is not one, so the *fact* is the
    /// shell's. The **conclusion** is not, and that distinction was bought: the macOS shell
    /// used to pass a constant `true` beside a comment asserting its Info.plist registered the
    /// scheme, and when a configuration shipped without it the layer was told the opposite of
    /// the truth, D-71's refusal could not fire, and a user granted consent and came back to
    /// nothing. Across the ABI a shell now reports the schemes it claims and the layer decides.
    ///
    /// It gates the *start* of an authorization rather than the end, which is the whole point:
    /// discovering it afterwards means the browser page is the first anyone hears of it.
    pub scheme_is_registered: bool,
    /// The OAuth client this installation is configured with — empty where there is none.
    ///
    /// Per-installation configuration rather than a secret: a public client's identifier
    /// appears in every authorization URL it generates, which is why PKCE exists. The layer
    /// holds it because three things need it and none of them should hold their own copy —
    /// the scheme derivation, the authorization it begins, and the refresh that reconnects an
    /// account the last run left behind.
    pub oauth_client_id: String,
    pub root: Option<PathBuf>,
    /// The installation container, where one has been opened.
    ///
    /// `None` is the scratch mode the harness and the tests run in: a temporary root, no
    /// registry, and nothing that survives the process. It is **not** a fallback a shell can
    /// end up in by accident — a shell calls [`App::open_container`] and fails if it cannot.
    pub container: Option<container::Container>,
    next_intent: u128,
    next_ordinal: u16,
    /// D-25's wheel. **One, for the whole application**, which is what makes an additional
    /// account cost nothing at idle: its deadlines join fires that already exist rather than
    /// adding their own. `cargo xtask invariants` forbids a per-account sleep loop everywhere
    /// but in the scheduler crate, and this is the field that makes obeying it possible.
    wheel: sift_scheduler::wheel::Wheel,
    /// Boxed so a test can drive time by hand. A wheel whose only clock is the system's is
    /// a wheel whose wiring can only be tested by waiting a minute, which is the kind of test
    /// that gets marked ignored and then gets deleted.
    clock: Box<dyn sift_scheduler::clock::Clock>,
    /// What `arm_periodic` armed last time, so it can take it back rather than adding to it.
    armed: Vec<sift_scheduler::wheel::TimerId>,
    /// D-93's governor. **One serialized owner of "the current tier"**, so a critical signal
    /// arriving mid-L2 supersedes rather than interleaving.
    governor: sift_governor::Governor,
    /// When the last pressure signal was seen, so the hysteresis dwell is measured rather
    /// than assumed. L-19 releases a tier only after pressure has been clear for that long.
    ///
    /// **On the same clock as the wheel**, deliberately. Reading `Instant::now()` here would
    /// put a second clock in a struct that already has one — the dwell would then be
    /// unmeasurable by any test that drives time, and the wheel that re-ticks the governor
    /// would advance while the dwell it feeds did not.
    last_pressure_at: Option<sift_scheduler::clock::Monotonic>,
    /// The last level the platform reported.
    ///
    /// **Remembered because the signal is edge-triggered.** A platform pressure source fires
    /// when the state *changes*, so after pressure clears there are no further signals — and
    /// `Governor::tick` releases one tier per call. Without a remembered level and a clock to
    /// re-tick against, the tier would descend one step and stall there for the life of the
    /// process, with every cache below it still shed.
    last_pressure: sift_governor::Pressure,
    /// FR-8's *load once* — **the one message in front of the user**.
    ///
    /// Keyed on the message rather than on a token because accepting re-renders, and a
    /// re-render mints a new token and revokes the old one: an allowance held against the
    /// token died with the click that granted it.
    ///
    /// One rather than a set, and that is the contract rather than a size optimisation.
    /// `pipeline.md` says show-once *"applies to the message in front of the user and is not
    /// written down anywhere"*, so opening a different message ends it — a set would have made
    /// "once" mean "for the rest of the session, every time this message is opened", in every
    /// window at once, which is a durable allowance nobody asked for. It is also bounded by
    /// construction, in a process specified to run for weeks.
    allowed_once_message: Option<LocalId>,
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
        Self::with_clock(Box::new(sift_scheduler::clock::SystemClock))
    }

    /// The same, with the clock the scheduler reads supplied.
    ///
    /// The only caller that passes anything but the system clock is a test that needs a
    /// minute to pass without one elapsing.
    #[must_use]
    pub fn with_clock(clock: Box<dyn sift_scheduler::clock::Clock>) -> Self {
        Self {
            accounts: BTreeMap::new(),
            // On a platform with no backend this refuses every write, which is D-71 doing
            // its job: a security absence refuses, and the only fallback available here is a
            // file — exactly what the constraint forbids by name.
            broker: sift_credentials::oauth::Broker::new(sift_credentials::store::Platform),
            pending_authorization: BTreeMap::new(),
            resources: sift_broker::broker::Broker::new(),
            selection: Vec::new(),
            open_message: None,
            has_window: false,
            scheme_is_registered: false,
            oauth_client_id: String::new(),
            root: None,
            container: None,
            next_intent: 0,
            next_ordinal: 0,
            // L-31 is the coalescing window, and it is the same number the platform timer is
            // given as its leeway — one value, so the structure and the hint cannot disagree.
            wheel: sift_scheduler::wheel::Wheel::new(sift_foundation::limits::L31_WHEEL_SLACK),
            clock,
            armed: Vec::new(),
            governor: sift_governor::Governor::new(),
            last_pressure_at: None,
            last_pressure: sift_governor::Pressure::Normal,
            allowed_once_message: None,
        }
    }

    /// Open the installation container, and everything the last run left in it.
    ///
    /// # What happens here that did not happen before
    ///
    /// Each account's files are opened **sealed**, under a key derived from that account's own
    /// key by D-106's role separation, and **its queue is rebuilt from its journal**. The
    /// second one is the reason this is one call rather than two commits: until it existed,
    /// `Queue::new()` started empty and nothing in the product ever read an intent back — which
    /// was harmless only while nothing persisted. The first restart of a durable container
    /// would have silently discarded every gesture a user watched succeed and that had not yet
    /// been issued, which is precisely what D-74 makes the journal undiscardable to prevent.
    ///
    /// # Errors
    /// The container cannot be made or read, the credential store is unavailable — D-71 makes
    /// that a refusal, because the only fallback is a key in a file — or an account's files do
    /// not authenticate, which D-73 makes a refusal rather than a repair.
    pub fn open_container(&mut self, root: &std::path::Path) -> Result<usize, String> {
        let store = sift_credentials::store::Platform;
        let container = container::Container::open(root, &store)?;
        let registered = container.accounts()?;

        for row in &registered {
            let owner = container::account_secret(&store, row.id)?;
            let paths = AccountPaths::under(root, row.id);
            let account =
                Account::open_sealed(&paths, row.id, &owner).map_err(|e| e.to_string())?;

            let mut queue = Queue::new();
            queue.restore(read_intents(&account.journal)?);

            let capabilities = stored_capabilities(&account).unwrap_or_else(|| {
                shape_named("rich").unwrap_or_else(|_| unreachable!("`rich` is a shape"))
            });
            // **Made unique rather than allowed to collide.** A container written before the
            // refusal above could hold two rows with one display name, and inserting both
            // under it would drop the first — the same silent loss, arriving at every launch
            // instead of once. The ordinal is what already distinguishes them and never
            // repeats, so it is what the label borrows.
            let mut label = row.display_name.clone();
            if self.accounts.contains_key(&label) {
                label = format!("{} ({})", row.display_name, row.ordinal.get());
            }
            self.accounts.insert(
                label,
                OpenAccount {
                    id: row.id,
                    kind: row.kind.clone(),
                    capabilities,
                    store: account,
                    queue,
                    ids: LocalIdGenerator::new(row.ordinal),
                    adapter: None,
                    // Not latched across a restart: the grant may have been repaired in the
                    // provider's own console since, and a stale prompt asking a person to sign
                    // in to an account that works is a prompt they learn to dismiss.
                    needs_authentication: false,
                    writes_enabled: row.writes_enabled,
                    subjects: BTreeMap::new(),
                },
            );
            self.next_ordinal = self.next_ordinal.max(row.ordinal.get().saturating_add(1));
        }

        // Reported rather than removed silently: dead bytes in the container are NFR-14's
        // budget being spent on nothing, and a sweep that ran without saying so would be a
        // deletion nobody asked for.
        let orphans = container.orphans().unwrap_or_default();
        self.root = Some(root.to_path_buf());
        self.container = Some(container);
        let _ = orphans;
        Ok(registered.len())
    }

    /// Add an account with a provider behind it.
    ///
    /// Its capability set comes from the adapter rather than from a name, which is the
    /// difference between this and [`Self::add_account`]: a shape is something a test picks,
    /// and this is what an account actually declares.
    pub fn add_provider_account(&mut self, name: &str, adapter: Live) -> Result<AccountId, String> {
        self.add_account_of_kind(name, adapter, PROVIDER_KIND)
    }

    /// The same, saying what a later run should reconnect it as.
    ///
    /// The distinction is not cosmetic: a restored account with no adapter can fetch nothing,
    /// and the two kinds are reconnected by different means — one through the credential store
    /// and the network, one through the recorded corpus and neither.
    ///
    /// # Errors
    /// The account could not be created.
    pub fn add_account_of_kind(
        &mut self,
        name: &str,
        adapter: Live,
        kind: &str,
    ) -> Result<AccountId, String> {
        // **The label is the handle, so two accounts cannot share one.** Every account-taking
        // call names an account by it, and the map is keyed on it — so a second account under
        // an existing label used to replace the first in memory while leaving its row in the
        // registry, dropping a sealed store handle and a queue rebuilt from a journal, with
        // nothing said and nothing to say it to. D-89 warns rather than refuses about the same
        // *mailbox* being added twice; this is about the name, which is a different thing.
        if self.accounts.contains_key(name) {
            return Err(format!("an account named `{name}` already exists"));
        }
        let capabilities = adapter.capabilities().clone();
        let id = self.create_account(name, capabilities, kind)?;
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
                // Process identifier, the clock **and** a counter. A pid alone is not unique
                // for long: the operating system reuses one within seconds, and a session that
                // landed on a reused directory would open a previous session's account files
                // and fail to insert its own account row — which is a flaky test that looks
                // like a bug in the store.
                //
                // The counter is the other half, and it was missing. `as_nanos` reports at
                // whatever resolution the platform has, and two `App`s constructed in one
                // process read the same value often enough to matter — which gave two of them
                // one directory, and `remove_dir_all` below then deleted the other's files
                // underneath it. It surfaced as `database is locked` from a test that had
                // nothing to do with the one that took the directory. The same mistake was
                // made and fixed in the layer's ephemeral root; this is the other copy.
                use std::sync::atomic::{AtomicU64, Ordering};
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let unique = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                let d = std::env::temp_dir()
                    .join(format!("sift-harness-{}-{unique}-{n}", std::process::id()));
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
        //
        // Where a container is open the registry assigns it — 128 random bits, with an ordinal
        // that is monotonic and never reclaimed — and the files are **sealed**. Without one,
        // this is the scratch mode the harness runs in: a sequential identity in a temporary
        // directory that nothing outlives.
        let (id, ordinal, root, store) = match self.container.as_mut() {
            Some(container) => {
                let registered = container.register(shape, name)?;
                let root = container.root().to_path_buf();
                let owner =
                    container::account_secret(&sift_credentials::store::Platform, registered.id)?;
                let paths = AccountPaths::under(&root, registered.id);
                let store = Account::open_sealed(&paths, registered.id, &owner)
                    .map_err(|e| e.to_string())?;
                (registered.id, registered.ordinal, root, store)
            }
            None => {
                let id = AccountId::from_u128(u128::from(self.next_ordinal) + 1);
                let ordinal = AccountOrdinal::new(self.next_ordinal);
                let root = self.root();
                let paths = AccountPaths::under(&root, id);
                let store = Account::open(&paths, id).map_err(|e| e.to_string())?;
                (id, ordinal, root, store)
            }
        };
        self.next_ordinal = self.next_ordinal.max(ordinal.get().saturating_add(1));
        let _ = root;

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
        // needs. **An account with an adapter is not**: D-83 assigns local identity on first
        // discovery, and pre-seeding a folder here would give the enumeration a row it did
        // not create and a remote identifier it did not choose.
        //
        // The test is whether this is a *shape*, not whether it is the literal `"provider"`.
        // It was the latter, and the recorded corpus getting a kind of its own immediately
        // put a phantom inbox in front of an adapter that enumerates four folders — which the
        // harness's own session test caught. A second provider's kind would have done the same.
        if shape_named(shape).is_ok() {
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
                kind: shape.to_owned(),
                capabilities,
                store,
                queue: Queue::new(),
                ids: LocalIdGenerator::new(ordinal),
                adapter: None,
                needs_authentication: false,
                // Read-only until somebody says otherwise. The safe state is the default,
                // and it is the default at construction rather than at a call site that
                // could be forgotten.
                writes_enabled: false,
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
        self.add_account_of_kind(name, descriptor.replayed(), REPLAYED_KIND)
    }

    /// Give an account restored from the container a provider to reach again.
    ///
    /// # What each kind needs
    ///
    /// D-65's recorded corpus needs nothing: no credential, no network, and the same adapter
    /// this process would have built to add it.
    ///
    /// A real mailbox needs its access token, and the stored one is very likely spent — the
    /// provider's are good for an hour and a resident application is restarted after longer
    /// than that far more often than not. So this refreshes rather than presenting what it
    /// holds and hoping: the cost is one round trip per account per launch, and the failure it
    /// avoids is a whole sync rejected with nothing retrying it. D-88's rules all apply, and
    /// the ones that matter here are that the write precedes the use and the previous pair is
    /// retained, so a refresh interrupted between receiving a pair and using it does not
    /// strand the account.
    ///
    /// # Errors
    /// There is no such account, the build has no adapter for it, no client identifier is
    /// configured, or the credential store or the provider refused.
    pub fn reconnect(&mut self, name: &str) -> Result<(), String> {
        let kind = self.account(name)?.kind.clone();
        let descriptor = sift_registry::KINDS
            .first()
            .ok_or("this build has no provider adapters")?;

        // **Not a `match` over provider names.** The registry column is documented as a name
        // rather than something to branch on, and it will hold a real provider kind as soon as
        // there is a second one — so this asks the two questions it can answer instead. Is it
        // the recorded corpus? Is it one of the capability shapes the planner tests use? Every
        // other value is an account with a provider behind it, which keeps a future `gmail`
        // out of the arm that reports it as a shape.
        let adapter = if kind == REPLAYED_KIND {
            descriptor.replayed()
        } else if shape_named(&kind).is_ok() {
            return Err(format!(
                "`{name}` is a capability shape rather than an account with a provider"
            ));
        } else {
            // A container written before the corpus had a kind of its own records a fixture
            // account as a provider. It has no credentials, and asking the store first is what
            // turns that into a sentence naming the account rather than a store error naming
            // an item — and what keeps a network transport from being built for a flow that
            // cannot happen.
            // The local question first, so a build with no client never reaches the credential
            // store — which on this platform means never prompting for keychain access to
            // answer something already decided.
            if self.oauth_client_id.is_empty() {
                return Err(
                    "this build has no OAuth client configured, so an account it did not add \
                     cannot be reached"
                        .to_owned(),
                );
            }
            let id = self.account(name)?.id;
            if self.broker.usable(id).is_err() {
                return Err(format!(
                    "`{name}` has no stored credentials, so it cannot be reached. \
                     Remove it and add it again."
                ));
            }
            let registration =
                authorize::registration(authorize::default_kind(), &self.oauth_client_id.clone())?;
            let mut transport = sift_http::Https::to(&registration.profile.token.host)
                .map_err(|why| format!("the trust store could not be consulted: {why}"))?;
            let pair = match self.broker.refresh(&mut transport, &registration, id) {
                Ok(pair) => pair,
                Err(error) => {
                    // **Only a well-formed denial latches.** D-88 makes every other outcome
                    // transient, and NFR-34's cascade is what that rule defends: a captive
                    // portal answering with a login page is unparseable, not a denial, and
                    // treating it as one would ask a person to sign in to every account at
                    // once because they joined a hotel network.
                    if matches!(&error, sift_credentials::oauth::AuthError::Failed(kind)
                        if kind.is_non_transient())
                    {
                        self.account(name)?.needs_authentication = true;
                    }
                    return Err(error.to_string());
                }
            };
            // It worked, so whatever it was is over.
            self.account(name)?.needs_authentication = false;
            descriptor
                .connect(&pair.access)
                .map_err(|e| e.to_string())?
        };

        self.account(name)?.adapter = Some(adapter);
        Ok(())
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
        // D-95's pause, honoured where the work is rather than only where the badge is. It was
        // a flag that produced a condition and nothing else read it, so "Pause Syncing"
        // painted the annunciator and the next gesture that reached this function synced
        // anyway.
        if self.is_paused(name) {
            return Ok(SyncReport::default());
        }
        // An account the last run left behind opens with no adapter: the store is sealed on
        // disk and its queue is rebuilt from the journal, but nothing reaches the provider.
        // Reconnecting here rather than at `open_container` is deliberate — it is a network
        // round trip, and a launch that waits on the network is the opposite of what a
        // resident mail client should do. This is already a network call.
        if self.account(name)?.adapter.is_none() {
            self.reconnect(name)?;
        }
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

/// The read-only posture: what it gates, and what it does not.
impl App {
    /// Whether this account's mutations may leave the process.
    ///
    /// **The gate is here and nowhere else.** Not inside `flush_once`, which is a pure
    /// application function that should stay ignorant of policy, and not inside an adapter,
    /// where refusing would be per-provider policy D-12 forbids. An intent that reaches the
    /// flush has passed this, and that is the invariant worth testing.
    #[must_use]
    pub fn may_issue(&self, name: &str) -> bool {
        self.accounts.get(name).is_some_and(|a| a.writes_enabled)
    }

    /// Authorize, or withdraw authorization for, writes to one account.
    ///
    /// Withdrawing takes effect immediately for anything not yet issued. Intents already on
    /// the wire are not recalled — they settle or move to *Reconciling* like any other,
    /// because a request that has left cannot be unsent and pretending otherwise would be
    /// the one lie a mutation queue must not tell.
    ///
    /// # Errors
    /// No such account.
    pub fn set_writes_enabled(&mut self, name: &str, enabled: bool) -> Result<(), String> {
        let account = self
            .accounts
            .get_mut(name)
            .ok_or_else(|| format!("no account named `{name}`"))?;
        account.writes_enabled = enabled;
        let id = account.id;
        // Durable, and in the registry rather than in the account's own store. An
        // authorisation that only lived in memory would be withdrawn by a restart, which
        // sounds safe until a user turns it on, closes the window, and finds their triage
        // silently held again with nothing saying why.
        if let Some(container) = self.container.as_mut() {
            container.set_writes_enabled(id, enabled)?;
        }
        Ok(())
    }

    /// What a setting currently holds — the recorded value, or D-101's default.
    ///
    /// # Errors
    /// The key is not one this build has.
    pub fn setting(&self, key: &str) -> Result<settings::Value, String> {
        let setting = settings::by_key(key)
            .ok_or_else(|| format!("`{key}` is not a setting this build has"))?;
        let recorded = self
            .container
            .as_ref()
            .and_then(|c| c.setting(key).ok().flatten());
        Ok(recorded
            .and_then(|text| setting.default.parse_like(&text))
            .unwrap_or_else(|| setting.default.clone()))
    }

    /// Record a setting.
    ///
    /// # Errors
    /// The key is unknown, the value is not one it can hold, it is security state, or there is
    /// no container to record it in — which is the scratch mode, and saying so is better than
    /// accepting a value that will not survive the process.
    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        let container = self
            .container
            .as_mut()
            .ok_or("this session has no container, so a setting would not survive it")?;
        container.set_setting(key, value)
    }

    /// What an account setting currently holds — the recorded value, or D-101's default.
    ///
    /// # Errors
    /// There is no such account, or the key is not one this build has.
    pub fn account_setting(&self, name: &str, key: &str) -> Result<settings::Value, String> {
        let setting = settings::by_key(key)
            .ok_or_else(|| format!("`{key}` is not a setting this build has"))?;
        let id = self
            .accounts
            .get(name)
            .ok_or_else(|| format!("no account named `{name}`"))?
            .id;
        let recorded = self
            .container
            .as_ref()
            .and_then(|c| c.account_setting(id, key).ok().flatten());
        Ok(recorded
            .and_then(|text| setting.default.parse_like(&text))
            .unwrap_or_else(|| setting.default.clone()))
    }

    /// Record an account setting.
    ///
    /// # Errors
    /// There is no such account, the key is unknown or is not account-scoped, the value is not
    /// one it can hold, it is security state, or there is no container to record it in.
    pub fn set_account_setting(
        &mut self,
        name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let id = self
            .accounts
            .get(name)
            .ok_or_else(|| format!("no account named `{name}`"))?
            .id;
        let container = self
            .container
            .as_mut()
            .ok_or("this session has no container, so a setting would not survive it")?;
        container.set_account_setting(id, key, value)
    }

    /// FR-4 — erase an account: its files, its registry row, and every credential item.
    ///
    /// Provable by enumeration, which is what the registry is for: what it does not list does
    /// not exist. The ordinal is not freed, because D-78 never reuses local identity.
    ///
    /// # Errors
    /// There is no such account, or the registry refused.
    pub fn forget_account(&mut self, name: &str) -> Result<(), String> {
        let id = self
            .accounts
            .get(name)
            .ok_or_else(|| format!("no account named `{name}`"))?
            .id;
        // Closed first: the files cannot be removed from underneath an open connection on
        // every platform, and a half-removed account is the state this is preventing.
        self.accounts.remove(name);
        if let Some(container) = self.container.as_mut() {
            container.forget(id, &sift_credentials::store::Platform)?;
        }
        Ok(())
    }

    /// Send what is queued for one account, once.
    ///
    /// **The read-only posture is checked before anything is issued**, and before the adapter
    /// is even taken. Everything up to here has already happened: the intents were built,
    /// checked against declared capabilities, written durably and applied optimistically. This
    /// is the one step that cannot be taken back, and it is the one step an account that is
    /// only being watched does not take.
    ///
    /// It lives here rather than in a shell because both shells need it and D-17 makes a
    /// capability that exists in one of them a defect. The harness keeps two branches of its
    /// own on top of this — an account with no provider behind it, and stopping after the
    /// issue so the crash path can be driven — and both are test affordances rather than
    /// things a person does.
    ///
    /// # Errors
    /// There is no such account, it has no provider behind it, or the journal refused the
    /// durable marker D-85 requires before a request goes out.
    pub fn flush(&mut self, name: &str) -> Result<Flushed, String> {
        // A paused account holds its queue for the same reason a watched one does: the user
        // asked it to stop. D-58 makes pause a policy tier everything resolves to, and the
        // tier that still flushes is the network's rather than the user's.
        if self.is_paused(name) {
            return Ok(Flushed {
                authorized: false,
                held: self.account(name)?.queue.len(),
                report: sift_mutations::flush::FlushReport::default(),
                queued: self.account(name)?.queue.len(),
                error: None,
            });
        }
        if !self.may_issue(name) {
            return Ok(Flushed {
                authorized: false,
                held: self.held(name),
                report: sift_mutations::flush::FlushReport::default(),
                queued: self.account(name)?.queue.len(),
                error: None,
            });
        }
        // Reconnect for the same reason `sync` does: an account the last run left behind
        // opens with no adapter, and someone who triaged before the first sync of a session
        // would be told Sift cannot reach an account it can reach. This is already the one
        // call that goes to the provider, so the round trip costs nothing that was not
        // already being paid.
        if self.account(name)?.adapter.is_none() {
            self.reconnect(name)?;
        }
        let account = self.account(name)?;
        let adapter = account
            .adapter
            .take()
            .ok_or("this account has no provider behind it")?;

        // The durable half of D-85's marker. It is a callback because the journal is the
        // store's and the queue cannot reach it — the two sit side by side in this layer and
        // D-59 gives neither an edge to the other.
        let journal = &account.store.journal;
        let mut mark_issued = |ids: &[u128]| -> Result<(), String> {
            let transaction = journal.unchecked_transaction().map_err(|e| e.to_string())?;
            for id in ids {
                transaction
                    .execute(
                        "UPDATE intent SET state = 'Issued' WHERE id = ?1",
                        rusqlite::params![id.to_be_bytes().to_vec()],
                    )
                    .map_err(|e| e.to_string())?;
            }
            transaction.commit().map_err(|e| e.to_string())
        };

        let resolve = Remote(&account.store.store);
        let outcome = sift_mutations::flush::flush_once(
            adapter.as_ref(),
            &mut account.queue,
            &resolve,
            &mut mark_issued,
        );
        // Returned either way — a failed flush must not leave an account unreachable, for the
        // same reason a failed sync must not.
        account.adapter = Some(adapter);

        let (report, error) = match outcome {
            Ok(report) => (report, None),
            Err(failure) => (failure.report.clone(), Some(failure.error.to_string())),
        };
        Ok(Flushed {
            authorized: true,
            held: 0,
            report,
            queued: account.queue.len(),
            error,
        })
    }

    /// Arm every account's periodic obligations on the one wheel.
    ///
    /// **Aligned, and both kinds on the same instant.** The poll and the flush for one
    /// account land in one bucket, and every account's land on the same wall-clock multiple,
    /// so five accounts at idle cost the fires of one. That is D-94's arithmetic made real:
    /// an additional account is free as long as its work joins a fire that already exists.
    ///
    /// Idempotent by construction — it clears what it armed before re-arming, so a caller
    /// that runs it after every account change cannot accumulate duplicate timers, which
    /// would be a per-account sleep loop wearing the wheel's clothes.
    pub fn arm_periodic(&mut self) {
        use sift_scheduler::wheel::Work;

        for id in std::mem::take(&mut self.armed) {
            self.wheel.cancel(id);
        }

        let deadline = sift_scheduler::clock::next_aligned(
            self.clock.as_ref(),
            sift_foundation::limits::L30_SYNC_POLL,
        );
        let accounts: Vec<AccountId> = self.accounts.values().map(|a| a.id).collect();
        for account in accounts {
            self.armed
                .push(self.wheel.arm(deadline, Some(account), Work::Sync));
            self.armed.push(
                self.wheel
                    .arm(deadline, Some(account), Work::FlushMutations),
            );
        }

        // **A held tier arms the wheel even with no accounts.** The governor's hysteresis is
        // clocked by these fires, and arming only per account meant a fresh install with
        // nothing added went to L3 under pressure and stayed there for the life of the
        // process — every window destroyed, nothing to poll, and so nothing to bring it back.
        // This work belongs to the installation rather than to an account, which is what
        // `Maintenance` is for.
        if self.governor.tier() > sift_governor::Tier::L0 {
            self.armed
                .push(self.wheel.arm(deadline, None, Work::Maintenance));
        }
    }

    /// How long until the wheel's next fire, if anything is armed.
    ///
    /// The shell arms one platform timer for this and hands L-31 alongside it as the leeway,
    /// which is the "this may fire late, batch it" hint D-25 exists to express. `None` means
    /// nothing is scheduled and no timer should be armed at all — a resident process with
    /// nothing to do takes no wakeups, which is the requirement rather than an optimisation.
    #[must_use]
    pub fn next_wake(&self) -> Option<core::time::Duration> {
        let next = self.wheel.next_fire()?;
        let now = self.clock.monotonic();
        Some(next.since(now))
    }

    /// Fire everything due, do its work, and re-arm.
    ///
    /// A failure is recorded rather than propagated: one account that cannot reach its
    /// provider must not stop the fire that four other accounts are sharing.
    pub fn tick(&mut self) -> TickReport {
        use sift_scheduler::wheel::Work;

        let mut report = TickReport::default();
        let due = self.wheel.fire_due(self.clock.monotonic());
        for entry in due {
            let Some(id) = entry.account else { continue };
            let Some(name) = self.name_of(id) else {
                continue;
            };
            match entry.kind {
                // A paused account is armed like any other — disarming it would mean
                // resuming had to re-arm from somewhere — but it is not *reported* as having
                // polled. `sync` returns an empty report for it, so recording the name here
                // would make FR-34's panel show a paused account polling every minute.
                Work::Sync if self.is_paused(&name) => report.paused.push(name),
                Work::Sync => match self.sync(&name, 1) {
                    Ok(outcome) => {
                        report.inserted += outcome.inserted;
                        report.synced.push(name);
                    }
                    Err(why) => report.failures.push((name, why)),
                },
                Work::FlushMutations if self.is_paused(&name) => {}
                Work::FlushMutations => match self.flush(&name) {
                    Ok(f) if f.report.issued > 0 => report.flushed.push(name),
                    Ok(_) => {}
                    Err(why) => report.failures.push((name, why)),
                },
                // The remaining kinds are armed by the paths that own them — a retry by the
                // backoff curve, a watch renewal by the provider that holds the watch. Firing
                // them from here would be this function deciding policy it does not own.
                _ => {}
            }
        }
        // **The wheel is the governor's clock.** The platform's pressure source is
        // edge-triggered, so once the machine is calm nothing signals again — and the
        // hysteresis releases one tier per call. Re-ticking here against the last level the
        // platform reported is what lets L3 walk back to L0 one step at a time, as D-93
        // requires, instead of stopping at the first step. Re-ticking against the *last*
        // level rather than assuming calm is what keeps a still-critical system at L3.
        if self.governor.tier() > sift_governor::Tier::L0 {
            let pressure = self.last_pressure;
            // Discarded deliberately, and this is the reason: on a wheel fire the tier is
            // already at least what `last_pressure` demands — the entry point raised it when
            // the signal arrived — so this call can only release, never deepen. There is no
            // L3 to issue from here, and a branch pretending otherwise would be dead code
            // carrying a host callback.
            let _ = self.memory_pressure(pressure);
        }

        self.arm_periodic();
        report
    }

    /// The label an identity is open under, if it is open at all.
    fn name_of(&self, id: AccountId) -> Option<String> {
        self.accounts
            .iter()
            .find(|(_, a)| a.id == id)
            .map(|(name, _)| name.clone())
    }

    /// FR-8 — the user accepted this message's withheld content for this session.
    ///
    /// Recorded against the message rather than the open document, because accepting
    /// re-renders and a re-render replaces the token. It is deliberately not persisted:
    /// "once" that outlived the process would be a durable allowance nobody asked for.
    pub fn allow_remote_content_once(&mut self, id: LocalId) {
        self.allowed_once_message = Some(id);
    }

    /// The operating system says memory is under pressure — D-93.
    ///
    /// **Subscribed to, never polled.** Polling free memory is both a wakeup counted against
    /// NFR-11 and a worse signal than the one the system already computes, so this is only
    /// ever called from a platform pressure source.
    ///
    /// Returns the transition where one happened, so the boundary can issue its sheds. A shed
    /// is *issued* and never awaited: at L3 destroying windows is a host callback on the
    /// shell's main loop, and waiting on it from here would be a deadlock rather than a delay.
    pub fn memory_pressure(
        &mut self,
        pressure: sift_governor::Pressure,
    ) -> Option<sift_governor::Transition> {
        let now = self.clock.monotonic();
        let elapsed = self
            .last_pressure_at
            .map_or(core::time::Duration::ZERO, |then| now.since(then));
        self.last_pressure_at = Some(now);
        self.last_pressure = pressure;
        let transition = self.governor.tick(pressure, elapsed);
        if let Some(t) = &transition {
            self.shed(t);
        }
        transition
    }

    /// The tier the governor is holding.
    #[must_use]
    pub fn tier(&self) -> sift_governor::Tier {
        self.governor.tier()
    }

    /// Release what a tier says to release, for the caches this layer owns.
    ///
    /// The window destruction L3 also requires is not here and cannot be: only a shell owns a
    /// window. The boundary issues that one as a host callback.
    fn shed(&mut self, transition: &sift_governor::Transition) {
        // **Tokens are revoked only where the views holding them are actually destroyed**,
        // which today is L3 and only L3. D-67's callback set is closed at six and contains
        // nothing that can destroy a body view, so at L2 the window and the reader stay on
        // screen — and revoking there would leave a live document whose every resource
        // request answers `Revoked`, whose "Load images" button fails, and whose reason the
        // shell has no way to state. FR-33 requires the reason be given; a dead view that
        // says nothing is worse than a cache that was not released.
        //
        // L2's own targets — the body view, parsed-MIME, database memory, the search index —
        // are not held by this layer yet, so L2 transitions and releases nothing. That is
        // stated rather than hidden: the tier is real and the release is outstanding.
        if transition.to == sift_governor::Tier::L3 {
            self.resources.shed();
        }
    }

    /// Whether the user has paused this account — D-95, per account.
    #[must_use]
    pub fn is_paused(&self, name: &str) -> bool {
        self.account_setting(name, "sync.paused")
            .is_ok_and(|v| v == settings::Value::Flag(true))
    }

    /// How many intents are being held because writes are not authorized.
    ///
    /// Surfaced rather than silent: a queue that grows while nothing leaves is a state the
    /// user chose, and one they have to be able to see they chose.
    #[must_use]
    pub fn held(&self, name: &str) -> usize {
        self.accounts
            .get(name)
            .filter(|a| !a.writes_enabled)
            .map_or(0, |a| a.queue.len())
    }
}

/// What one turn of the flush did, including the turn an unauthorized account does not take.
///
/// **`authorized` is not an error case.** An account that is only being watched has a queue
/// that grows and sends nothing, and that is the state the user chose — so it is reported as a
/// result with a count in it rather than as a failure, which is what lets a surface say
/// "nothing has been sent, and nothing will be until you say so" instead of showing a fault.
#[derive(Debug, Clone)]
pub struct Flushed {
    pub authorized: bool,
    /// Intents held because writes are not authorized. Zero once they are.
    pub held: usize,
    pub report: sift_mutations::flush::FlushReport,
    /// What is still queued afterwards.
    pub queued: usize,
    /// The provider or the journal refused, in the layer's own words.
    pub error: Option<String>,
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

/// D-49's annunciator, resolved per account.
///
/// One badge shows the **single highest-precedence** condition rather than a list, and
/// `AccountCondition`'s `Ord` is that precedence — so resolving is a `min` over what applies.
/// `Healthy` renders nothing at all: a badge that is always present is a badge nobody reads.
impl App {
    /// What is currently true of one account.
    ///
    /// # Errors
    /// There is no such account.
    pub fn condition_of(&mut self, name: &str) -> Result<AccountCondition, String> {
        let mut applicable = Vec::new();
        let account = self.account(name)?;

        // The store answering at all is the test for this one, and it is a real query rather
        // than a flag: a flag says what was true when somebody last set it, and a full disk
        // does not set flags.
        if account
            .store
            .store
            .query_row("SELECT 1 FROM message LIMIT 1", [], |_| Ok(()))
            .is_err()
            && account
                .store
                .store
                .query_row("SELECT count(*) FROM message", [], |r| r.get::<_, i64>(0))
                .is_err()
        {
            applicable.push(AccountCondition::StorageUnavailable);
        }

        // D-95 puts the pause on the account rather than on the installation, and D-49 makes
        // it a condition of its own rather than a flag — the three pauses are three
        // conditions, because "you stopped this" and "the data cap stopped this" are answered
        // by the user differently.
        let paused = self.is_paused(name);
        let account = self.account(name)?;
        if paused {
            applicable.push(AccountCondition::PausedByUser);
        }
        // FR-2's one condition that must reach the user with no window open, and the highest
        // in D-49's precedence — an account Sift cannot reach is not usefully described by
        // anything else that is also true of it.
        if account.needs_authentication {
            applicable.push(AccountCondition::NeedsAuthentication);
        }

        let quarantined = account
            .queue
            .entries()
            .iter()
            .any(|e| e.state.needs_attention());
        if quarantined {
            applicable.push(AccountCondition::Attention);
        }

        // A watched account with a queue that cannot drain. The intents are safe and the user
        // has something to do about them, which is exactly what `Attention` means — and saying
        // nothing would leave a person watching triage pile up with no indication why.
        if !account.writes_enabled && !account.queue.is_empty() {
            applicable.push(AccountCondition::Attention);
        }

        Ok(AccountCondition::resolve(&applicable))
    }

    /// The one condition the annunciator draws, across every account, and how many accounts
    /// are in it.
    pub fn annunciator(&mut self) -> (AccountCondition, usize) {
        let names: Vec<String> = self
            .account_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let conditions: Vec<AccountCondition> = names
            .iter()
            .filter_map(|n| self.condition_of(n).ok())
            .collect();
        let worst = AccountCondition::resolve(&conditions);
        let affected = conditions.iter().filter(|c| **c == worst).count();
        (worst, affected)
    }
}

/// Read a journal's intents back, for [`Queue::restore`].
///
/// **The only place in the product that reads an intent back from disk.** Before this the two
/// `SELECT … FROM intent` statements in the workspace were both inside a `#[cfg(test)]` block,
/// which is why nothing failed: the queue was never rebuilt, and no test asked it to be.
fn read_intents(
    journal: &rusqlite::Connection,
) -> Result<Vec<sift_mutations::queue::Restored>, String> {
    let mut statement = journal
        .prepare(
            "SELECT id, undo_group, message_id, operation, parameter, state,
                    attempt_count, created_millis, per_message_seq
             FROM intent WHERE state != 'Settled' ORDER BY message_id, per_message_seq",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |r| {
            let id: Vec<u8> = r.get(0)?;
            let undo_group: Option<Vec<u8>> = r.get(1)?;
            let message: Vec<u8> = r.get(2)?;
            let operation: String = r.get(3)?;
            let parameter: Option<String> = r.get(4)?;
            let state: String = r.get(5)?;
            Ok((
                id,
                undo_group,
                message,
                operation,
                parameter,
                state,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        let (id, undo_group, message, operation, parameter, state, attempts, created, sequence) =
            row.map_err(|e| e.to_string())?;
        let bytes: [u8; 16] = message
            .as_slice()
            .try_into()
            .map_err(|_| "a message identity is not 16 bytes".to_owned())?;

        // NFR-48: an operation this build does not recognise is **quarantined** — held, shown,
        // and executable by a later build. Dropping it would lose a gesture; guessing at it
        // would perform one the user did not ask for.
        let (intent, state) =
            match sift_mutations::intent::Intent::from_parts(&operation, parameter.as_deref()) {
                Some(intent) => (intent, sift_mutations::intent::State::from_name(&state)),
                None => (
                    sift_mutations::intent::Intent::Archive,
                    sift_mutations::intent::State::Quarantined,
                ),
            };

        out.push(sift_mutations::queue::Restored {
            id: be_u128(&id),
            undo_group: undo_group.as_deref().map(be_u128),
            message: LocalId::from_bytes(bytes),
            intent,
            state,
            attempts: u32::try_from(attempts).unwrap_or(u32::MAX),
            created_millis: u64::try_from(created).unwrap_or(0),
            sequence: u64::try_from(sequence).unwrap_or(0),
        });
    }
    Ok(out)
}

/// What an account declared, as its own store recorded it.
fn stored_capabilities(account: &Account) -> Option<Capabilities> {
    let shape: Vec<u8> = account
        .store
        .query_row("SELECT capabilities FROM account LIMIT 1", [], |r| r.get(0))
        .ok()?;
    shape_named(std::str::from_utf8(&shape).ok()?).ok()
}

fn be_u128(bytes: &[u8]) -> u128 {
    let mut out = [0u8; 16];
    let n = bytes.len().min(16);
    out[16 - n..].copy_from_slice(&bytes[..n]);
    u128::from_be_bytes(out)
}

/// What one wheel fire did.
///
/// Reported rather than logged, because FR-34's runtime panel is what makes "an account Sift
/// is only watching has sent nothing" a thing a user can see rather than infer.
#[derive(Debug, Default, Clone)]
pub struct TickReport {
    pub synced: Vec<String>,
    /// Envelopes the fire actually brought in. **The number that says mail arrived**, as
    /// opposed to `synced`, which says only that the wheel reached the account.
    pub inserted: usize,
    pub flushed: Vec<String>,
    /// Accounts the wheel reached and did not poll, because the user paused them.
    pub paused: Vec<String>,
    pub failures: Vec<(String, String)>,
}

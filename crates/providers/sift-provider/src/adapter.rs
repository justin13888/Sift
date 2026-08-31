//! The adapter contract: six responsibilities, and nothing more.
//!
//! `docs/mail/provider-model.md` enumerates them and says "nothing more" in as many words.
//! The list is short on purpose — everything an adapter is *not* asked to do is something
//! that would otherwise be done four times, differently.
//!
//! Notably absent: threading (D-103 assigns local identity), identity joins (D-44 is one
//! rule serving four consumers), retry and backoff (D-87 puts a stated delay on the wheel
//! rather than in a sleep), conflict resolution (D-38 lives in the presentation layer),
//! and any decision about *when* to run (D-25's wheel owns that).

use crate::capability::Capabilities;

/// One part of a message, as the provider describes it.
///
/// **Owned by the MIME crate rather than by this one**, and the edge points the way D-59
/// requires: a part descriptor is an internet-message-format concept, the rendering layer's
/// stage 2 chooses among them, and the rendering layer may not reach an adapter. So the type
/// is defined below the adapters and used by them — which is the same shape as the layer
/// table's own rule, that where a lower crate needs something from a higher one, the lower
/// crate defines it.
pub use sift_mime::select::PartDescriptor;

/// A cursor into a provider's change feed. Opaque to everything above the adapter.
///
/// D-82 requires it be acquired **before** a folder's backfill walks history: backfill-first
/// loses every change in a window that on a large mailbox is hours, which is silent data
/// loss. The redundancy cursor-first creates is absorbed because reapplication is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor(pub Vec<u8>);

/// Whether a message was *delivered* or merely *discovered*.
///
/// **This must be recorded from the first build.** FR-23's notification rule ships three
/// phases later and defines new mail as delivered-and-unread-at-that-moment; it cannot be
/// reconstructed afterwards without a resynchronization NFR-18 forbids.
///
/// D-84's cursor recovery marks everything it observes as `Discovered`, **including rows it
/// inserts**, and D-102's re-ingest after eviction does the same. A backfill discovers; only
/// a delta delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// The delta reported it as an arrival.
    Delivered,
    /// Backfill, cursor recovery, a full scan, or re-ingest after eviction.
    Discovered,
}

/// What the adapter is asked to do, and nothing more.
pub trait Adapter {
    type Error;

    /// What this account can do. Everything above plans against this and never against a
    /// provider name.
    fn capabilities(&self) -> &Capabilities;

    /// 1. Enumerate folders.
    ///
    /// D-83: a folder that stops appearing here is **retired, not deleted**, and a new one
    /// is *discovered* rather than adopted — it is backfilled only if it is in FR-43's
    /// watched set.
    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Self::Error>;

    /// 2. Produce a delta against a cursor.
    ///
    /// The **only** path by which change is applied. Push answers "has something changed?";
    /// this answers "what changed?", and there is exactly one code path that applies it.
    fn delta(&self, folder: &RemoteFolderId, cursor: Option<&Cursor>)
    -> Result<Delta, Self::Error>;

    /// 3. Fetch envelopes.
    ///
    /// **Sift MUST NOT fetch whole messages.** Structure first, then only the part decided
    /// for display — a message carrying a 40 MB attachment costs a few kilobytes until the
    /// user asks for the attachment.
    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error>;

    /// 4a. Describe a message's structure, without fetching any part's bytes.
    ///
    /// This is what "structure first" means, stated as an operation rather than as an
    /// aspiration: a message carrying a forty-megabyte attachment costs a few kilobytes here,
    /// and the attachment's bytes are fetched only if [`Self::fetch_part`] asks for them.
    ///
    /// It is also stage 2 of [the pipeline](../../../../docs/rendering/pipeline.md)'s input:
    /// choosing which alternative to render is a decision over *this*, and a caller that
    /// could not see the parts would have to guess.
    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, Self::Error>;

    /// 4b. Fetch a specific body part.
    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, Self::Error>;

    /// 5. Apply a batch of mutations.
    ///
    /// Batched to [`Capabilities::batch_size`]. **A batch MUST NOT contain two intents for
    /// the same message**, because intents against one message apply in the order they were
    /// issued — always, including through batching and retry.
    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error>;

    /// 6. Expose a change-notification stream.
    ///
    /// A doorbell. It says something changed; it never says what.
    fn watch(&self, folders: &[RemoteFolderId]) -> Result<(), Self::Error>;

    /// Classify a failure this adapter produced.
    ///
    /// **The adapter says what kind; the scheduler says when.** That split is D-87's and it
    /// is why this is a classification rather than a retry: an adapter that decided when to
    /// try again would be a per-account sleep loop, which D-25 prohibits outright.
    ///
    /// It is required rather than defaulted because every wrong answer here is expensive in
    /// a different direction. A cursor invalidation mistaken for a fault degrades an account
    /// that only needed to recover; a fault mistaken for a cursor invalidation resyncs a
    /// mailbox for nothing. Neither is a sensible default.
    ///
    /// It takes `&self` rather than being an associated function, and that is not a
    /// stylistic choice: **the trait has to be object-safe.** A shell holds one account's
    /// adapter without knowing which of the four it is — that is the whole of D-12 — and an
    /// associated function with no receiver would force the shell to name the type, which is
    /// the special-casing the capability model exists to remove.
    fn classify(&self, error: &Self::Error) -> Failure;

    /// Present a different credential from now on.
    ///
    /// **Not a seventh responsibility.** The six are what an adapter *does*; this is the
    /// material it does them with, and every rule about that material lives in the credential
    /// broker: D-88's single-flight refresh, its write-before-use ordering, and its
    /// classifier. NFR-23 makes the broker the one place credential access is brokered, so
    /// this is the only way a secret reaches an adapter and the broker is the only caller.
    ///
    /// An adapter that authenticates some other way — or not at all — ignores it. Silence is
    /// correct here rather than a refusal: the broker cannot know which adapters take a
    /// bearer, and an error would make it find out by trying.
    fn present_credential(&self, _secret: &str) {}

    /// Bytes on the wire since this adapter was built — FR-36.
    ///
    /// Zero for an adapter that does not touch a socket. The replay harness is not on
    /// anybody's data plan.
    fn wire_bytes(&self) -> (u64, u64) {
        (0, 0)
    }
}

/// What a failure means to the layer that has to decide what happens next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The cursor is no longer accepted. **Progress, not a fault** — D-82 moves the folder
    /// to `Invalidated` and NFR-18 recovers without asking the user anything.
    CursorInvalidated,
    /// The credential presented was refused. Only the credential broker can decide whether
    /// the *grant* is gone; this says only that the request was not answered.
    CredentialRefused,
    /// The provider stated a delay. D-87 puts it on the wheel as a deadline, and the number
    /// is a floor rather than an instruction because the wheel coalesces.
    Throttled { retry_after_millis: u64 },
    /// Try again later, at the scheduler's discretion.
    Transient,
    /// The request went out and the answer did not come back. D-85 moves an intent to
    /// *Reconciling* rather than replaying it blindly.
    Unknown,
    /// Settled. Retrying changes nothing, and NFR-29 requires the reason be surfaced.
    Permanent,
}

/// A provider's own folder identifier. An **attribute** of a folder, never its key: D-83
/// gives every folder a local identity assigned on first discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteFolderId(pub String);

/// A provider's own message identifier. Also an attribute rather than a key — and under
/// [`IdStability::UnstableOnMove`](crate::capability::IdStability::UnstableOnMove) it does
/// not even survive a move.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteMessageId(pub String);

/// The semantic kind of a special-use folder — FR-5.
///
/// Resolved through the provider's own mechanism and **never by string-matching a display
/// name**: "a locale table is a bug". Where nothing resolves, Sift prompts the user once
/// and persists the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialUse {
    Inbox,
    Archive,
    Sent,
    Trash,
    Spam,
    Drafts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolder {
    pub id: RemoteFolderId,
    pub display_name: String,
    pub special_use: Option<SpecialUse>,
}

/// One page of change, with the cursor that follows it.
///
/// D-82: a page and its cursor advance **in one transaction**. A cursor advancing past
/// change that was not applied is silent data loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub changes: Vec<Change>,
    pub next: Cursor,
    /// Whether more pages follow. A resume rewinds to the last committed page, which is
    /// the granularity L-26 sets.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Present {
        id: RemoteMessageId,
        provenance: Provenance,
    },
    Removed {
        id: RemoteMessageId,
    },
    FlagsChanged {
        id: RemoteMessageId,
    },
}

/// One list row's worth of a message, as the adapter recovered it.
///
/// **Everything here is attacker-influenced.** Subject, display name and snippet are written
/// by whoever sent the mail, so nothing above this type may render one without NFR-54's
/// normalization and L-25's bound — which is why the layer that stores them and the layer
/// that shows them share one normalizer in the foundation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Envelope {
    pub id: RemoteMessageId,
    /// The provider's own conversation identifier where it has one.
    ///
    /// FR-11 prefers it over reconstruction, and D-44 uses it as the **scope** a join is
    /// performed within — never as the join's key.
    pub thread_id: Option<String>,
    /// The `Message-ID` header. D-44: it **narrows and never keys**.
    pub internet_message_id: Option<String>,
    /// `References` and `In-Reply-To`, in order. D-44's reference chain.
    pub references: Vec<String>,
    pub subject: Option<String>,
    /// The originator address, as D-44's digest needs it.
    pub from: Option<String>,
    pub to: Vec<String>,
    /// **The server's received time**, which D-55 makes the authoritative sort key. Not the
    /// `Date` header, which is the sender's and is only ever displayed.
    pub received_at_millis: u64,
    /// The sender's `Date` header. Displayed, never ordered on.
    pub origination_date_millis: Option<u64>,
    pub snippet: Option<String>,
    /// Every folder this message is in.
    ///
    /// A list rather than a field, because [`LocationCardinality`](crate::capability::LocationCardinality)
    /// is a declared capability: on a provider that says `OneOrMore` a message is in several
    /// places at once, and a single-valued field would silently pick one.
    pub folders: Vec<RemoteFolderId>,
    /// Tags applied to this message, as the provider identifies them — FR-37.
    ///
    /// Identifiers rather than names, because the two are not the same thing on every
    /// provider and only the adapter holds the mapping.
    pub tags: Vec<String>,
    pub read: bool,
    pub flagged: bool,
    /// What the provider says the whole message would cost to fetch. Advisory: the fetch is
    /// still bounded by L-13 against the transferred length, because a sender controls both.
    pub size_estimate: u64,
}

/// A mutation as the adapter will send it. Resolved from a provider-agnostic intent
/// **inside** the adapter — the layers above never construct one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMutation {
    pub message: RemoteMessageId,
    /// The client-assigned identifier, sent as an idempotency key where the provider
    /// accepts one.
    pub intent_id: u128,
    pub operation: Operation,
}

/// FR-13's closed set, at the wire boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Archive,
    DeleteToTrash,
    PermanentlyDelete,
    MoveTo(RemoteFolderId),
    SetRead(bool),
    SetFlagged(bool),
    AddTag(String),
    RemoveTag(String),
    ReportJunk,
    ReportNotJunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    Applied,
    /// Retryable. The scheduler decides when, under D-87 and L-24 — never the adapter, and
    /// never by sleeping.
    Transient,
    /// The provider explicitly refused.
    Refused,
    /// The request went out and the answer did not come back. D-85 moves the intent to
    /// *Reconciling*, and the adapter establishes server state before retrying rather than
    /// blindly replaying — which is what NFR-17 means by exactly-once *observable*.
    Unknown,
}

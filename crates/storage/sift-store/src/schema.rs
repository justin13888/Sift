//! The schema, and D-32's forward-only migrations.
//!
//! # Two files per account (D-74)
//!
//! An account is a **store** that may be discarded and a **journal** that may not.
//!
//! D-57 forces this: backup exclusion is a property of a *file*, and with the mutation
//! queue inside the account database there was no way to exclude the cache from backup
//! without excluding the queue with it. A file is the unit of discarding as well as the
//! unit of backup.
//!
//! The relationship between them is an **ordering, not an atomicity** requirement, and the
//! order is normative: **journal first, store second**. A crash between the two leaves an
//! intent enqueued whose optimistic effect was never applied — invisible and
//! self-correcting, because the overlay is derived from the queue. The reverse order would
//! lose a mutation the user watched succeed, which is why the order is stated rather than
//! left to whoever writes the code.
//!
//! **Both files version and migrate together, and a build MUST refuse to open an account
//! whose two halves disagree about their version.**
//!
//! # Forward-only (D-32)
//!
//! A schema version is stamped in each file, migrations are ordered and forward-only, and a
//! database newer than the running binary is **refused rather than guessed at**. The named
//! cost is real: a downgrade after a migration discards writes the user watched succeed,
//! and the exposure is uneven by channel — the App Store offers no downgrade, Homebrew Cask
//! and Flatpak both do.
//!
//! The recovery path carries an obligation that is easy to skip: **removal-and-resync after
//! a refused open MUST drain or export the queue before it destroys the database, and MUST
//! NOT do neither silently.** A queued mutation is the one thing a resync cannot restore.

use rusqlite::{Connection, Result as SqlResult};

/// The schema version this build writes.
///
/// Version 2 is [`STORE_MIGRATION_V2`]: the two lookups ingest makes per message, indexed.
pub const CURRENT_VERSION: u32 = 2;

/// The oldest version this build can migrate forward from — D-62's migration floor.
///
/// Advancing it is an amendment to `docs/build/packaging.md`, exactly as moving a
/// requirement between phases amends the roadmap, and it **must never be advanced past a
/// build that is still plausibly installed**.
pub const MIGRATION_FLOOR: u32 = 1;

/// What can go wrong opening an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The file was written by a newer build. **Refused, never guessed at.**
    ///
    /// The user's recovery is removal and resync — and the queue must be drained or
    /// exported first.
    TooNew { found: u32, understood: u32 },
    /// Older than D-62's floor. The migration chain that would have carried it forward has
    /// been pruned.
    BelowFloor { found: u32, floor: u32 },
    /// The store and the journal disagree. D-74: the account does not open.
    HalvesDisagree { store: u32, journal: u32 },
}

impl core::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooNew { found, understood } => {
                write!(f, "schema version {found} is newer than {understood}")
            }
            Self::BelowFloor { found, floor } => {
                write!(
                    f,
                    "schema version {found} is below the migration floor {floor}"
                )
            }
            Self::HalvesDisagree { store, journal } => {
                write!(
                    f,
                    "store is version {store} and journal is version {journal}"
                )
            }
        }
    }
}

impl std::error::Error for SchemaError {}

/// Decide what to do with a file at `found`.
pub fn admit(found: u32) -> Result<Migration, SchemaError> {
    if found > CURRENT_VERSION {
        return Err(SchemaError::TooNew {
            found,
            understood: CURRENT_VERSION,
        });
    }
    if found != 0 && found < MIGRATION_FLOOR {
        return Err(SchemaError::BelowFloor {
            found,
            floor: MIGRATION_FLOOR,
        });
    }
    Ok(if found == CURRENT_VERSION {
        Migration::None
    } else {
        Migration::Forward { from: found }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    None,
    Forward { from: u32 },
}

/// The two halves must agree before either is used.
pub fn admit_pair(store: u32, journal: u32) -> Result<Migration, SchemaError> {
    if store != journal {
        return Err(SchemaError::HalvesDisagree { store, journal });
    }
    admit(store)
}

/// The store's schema — everything that may be discarded and refetched.
///
/// The entity list is `docs/storage/data-model.md`'s. Two absences are decisions rather
/// than omissions: **credentials are not here and MUST NOT be** (NFR-23 puts them only in
/// the OS credential store), and **contacts are not here** because D-41 resolves display
/// names from the platform store on demand and keeps nothing — a contact entity would be a
/// second sync domain that scope excludes.
///
/// The **mutation queue and its overlays are not here either**: they are the journal, per
/// D-74.
pub const STORE_SCHEMA_V1: &str = r"
CREATE TABLE account (
    -- One row. The file *is* the account.
    id                  BLOB PRIMARY KEY NOT NULL,
    ordinal             INTEGER NOT NULL,      -- D-78's per-installation slot
    display_name        TEXT NOT NULL,
    capabilities        BLOB NOT NULL          -- the declared set, serialised
) STRICT;

CREATE TABLE folder (
    -- D-83: local identity is the key; the remote identifier is an attribute.
    id                  INTEGER PRIMARY KEY,
    remote_id           TEXT,
    -- FR-5: semantic, never a display name. 'a locale table is a bug'.
    special_use         TEXT,
    display_name        TEXT NOT NULL,
    -- D-83: a folder that vanishes is retired, not deleted. Its messages stop being
    -- present *in it* and are still reachable through search and threads.
    retired             INTEGER NOT NULL DEFAULT 0,
    -- FR-43: the watched set, per account, user-selectable and persisted.
    watched             INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE UNIQUE INDEX folder_remote ON folder(remote_id) WHERE remote_id IS NOT NULL;

CREATE TABLE folder_sync_state (
    folder_id           INTEGER PRIMARY KEY REFERENCES folder(id) ON DELETE CASCADE,
    -- D-82's six states.
    state               TEXT NOT NULL,
    -- Held per folder, never per account: one folder's validity change MUST NOT
    -- invalidate the account.
    cursor              BLOB,
    validity            TEXT,
    last_success_millis INTEGER,
    -- Surfaced under NFR-29 rather than hidden.
    degraded_reason     TEXT,
    -- D-53's resumable backfill position.
    backfill_position   BLOB
) STRICT;

CREATE TABLE thread (
    -- D-103: local identity assigned at creation. Threads merge with the older identity
    -- surviving, and never split.
    id                  BLOB PRIMARY KEY NOT NULL,
    remote_thread_id    TEXT,
    normalized_subject  TEXT,
    -- D-102: these describe the messages still held, not the messages that ever existed.
    last_activity_millis INTEGER NOT NULL,
    message_count       INTEGER NOT NULL
) STRICT;

CREATE TABLE message (
    -- D-78. Fixed width, time-ordered, installation-unique, never reused.
    id                  BLOB PRIMARY KEY NOT NULL,
    remote_id           TEXT,
    internet_message_id TEXT,
    -- D-44: corroborates a join the scope already proposed. Never keys one.
    fallback_digest     BLOB NOT NULL,
    -- D-104: a comparison across rule versions counts as NO corroboration.
    digest_rule_version INTEGER NOT NULL,
    thread_id           BLOB REFERENCES thread(id),
    -- D-55: the server's received time is the authoritative sort key.
    received_at_millis  INTEGER NOT NULL,
    -- The sender's Date header. Displayed only, never ordered on.
    origination_millis  INTEGER,
    sender              TEXT NOT NULL,
    recipients          TEXT NOT NULL,
    subject             TEXT,
    -- L-16, truncated rather than rejected; present only where the account declares a
    -- snippet source.
    snippet             TEXT,
    -- The *base* state of D-51. The pending overlay lives with the queue, not here.
    flags               INTEGER NOT NULL DEFAULT 0,
    has_attachments     INTEGER NOT NULL DEFAULT 0,
    size_bytes          INTEGER,
    mime_structure      BLOB,
    -- Null when the body is not cached. An FR-12 'not cached' state, distinct from
    -- 'not available'.
    body_blob           BLOB,
    -- FR-23 needs this three phases after it must first be recorded, and it cannot be
    -- reconstructed without a resync NFR-18 forbids.
    provenance          TEXT NOT NULL
) STRICT;

-- D-55's comparator, as an index: received time, tiebroken on local identity. The SAME
-- comparator must serve this query and the in-memory unified-inbox merge.
CREATE INDEX message_order ON message(received_at_millis DESC, id DESC);
CREATE INDEX message_thread ON message(thread_id);
-- D-44 narrows candidates with this; it never keys a join.
CREATE INDEX message_internet_id ON message(internet_message_id)
    WHERE internet_message_id IS NOT NULL;

-- Location is an axis (D-12). A message may be in one folder or several depending on the
-- account's declared cardinality, so this is a relation rather than a column.
CREATE TABLE message_location (
    message_id          BLOB NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    folder_id           INTEGER NOT NULL REFERENCES folder(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, folder_id)
) STRICT;

CREATE INDEX message_location_folder ON message_location(folder_id);

-- Tags are the other axis, present only where the account declares tag support.
CREATE TABLE tag (
    id                  INTEGER PRIMARY KEY,
    name                TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE message_tag (
    message_id          BLOB NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    tag_id              INTEGER NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, tag_id)
) STRICT;

-- FR-28: message authentication runs at ingest and its results are stored on the message,
-- feeding D-11's synthetic origin and the FR-33 debug view.
CREATE TABLE authentication_result (
    message_id          BLOB PRIMARY KEY REFERENCES message(id) ON DELETE CASCADE,
    signing_domain      TEXT,
    sender_policy_pass  INTEGER,
    alignment_pass      INTEGER
) STRICT;

-- The account's claim on a shared blob. Refcounts live in the shared index, not here.
CREATE TABLE blob_reference (
    content_address     BLOB NOT NULL,
    role                TEXT NOT NULL,
    message_id          BLOB REFERENCES message(id) ON DELETE CASCADE,
    PRIMARY KEY (content_address, role, message_id)
) STRICT;

-- D-101's account-scoped settings. Erased by FR-4 as part of the database.
CREATE TABLE account_policy (
    key                 TEXT PRIMARY KEY NOT NULL,
    value               TEXT NOT NULL
) STRICT;

-- The per-sender remote-content allowlist is SECURITY STATE, not a preference: write
-- access to it is write access to Sift's egress policy. It is keyed on the attested
-- synthetic origin and NEVER on a displayed sender.
CREATE TABLE sender_allowlist (
    attested_origin     TEXT PRIMARY KEY NOT NULL,
    allowed_at_millis   INTEGER NOT NULL
) STRICT;

CREATE TABLE sender_dark_mode (
    attested_origin     TEXT PRIMARY KEY NOT NULL,
    transform           INTEGER NOT NULL
) STRICT;
";

/// The journal's schema — the queue and its overlays. **Not discardable, and the only half
/// D-57 leaves in the backup.**
pub const JOURNAL_SCHEMA_V1: &str = r"
CREATE TABLE intent (
    -- Client-assigned, and the value sent as an idempotency key where a provider accepts
    -- one.
    id                  BLOB PRIMARY KEY NOT NULL,
    -- D-85's undo group: what a compensation acts over, assigned at the gesture.
    undo_group          BLOB,
    message_id          BLOB NOT NULL,
    -- FR-13's closed set, serialised. The discriminant only: NFR-48's quarantine keys on
    -- whether *this* build recognises the operation, and a name that carried its parameters
    -- would make an unrecognised parameter look like an unrecognised operation.
    operation           TEXT NOT NULL,
    -- What the gesture supplied and the register did not: a destination folder for a move,
    -- a name for a tag.
    --
    -- **Not optional decoration.** A move with no destination is not a move, and the first
    -- version of this table had no such column — so an intent read back from the journal
    -- would have been a `move-to` with nowhere to go. That is silent loss of a gesture the
    -- user watched succeed, which is the exact failure the journal exists to prevent.
    parameter           TEXT,
    -- NFR-48: serialised intents carry their own version, so a build that does not
    -- recognise one QUARANTINES it rather than dropping or guessing at it.
    intent_version      INTEGER NOT NULL,
    -- D-85's six states.
    state               TEXT NOT NULL,
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    created_millis      INTEGER NOT NULL,
    -- Intents against ONE message apply in the order they were issued. Always, including
    -- through batching and retry.
    per_message_seq     INTEGER NOT NULL,
    expires_millis      INTEGER NOT NULL
) STRICT;

CREATE INDEX intent_message ON intent(message_id, per_message_seq);
CREATE INDEX intent_state ON intent(state);

-- D-51's pending overlay. Written here so it survives termination; read from memory on the
-- message path, and rebuilt at startup.
CREATE TABLE overlay (
    intent_id           BLOB PRIMARY KEY REFERENCES intent(id) ON DELETE CASCADE,
    message_id          BLOB NOT NULL,
    -- What the intent contributes over the base state the delta writes.
    delta               TEXT NOT NULL
) STRICT;

CREATE INDEX overlay_message ON overlay(message_id);
";

/// Version 2 of the store: the lookups ingest makes **once per message**, indexed.
///
/// Sync resolves every change in a delta page by provider identifier — is this message held,
/// which local identity is it — and every threaded envelope by the provider's conversation
/// identifier. Version 1 indexed neither, so each lookup scanned the whole table beneath the
/// page-decryption layer, and a first sync cost the square of the mailbox: a backfill of ten
/// thousand messages took minutes and one of the reference environment's 50,000-message inbox
/// did not finish. The scale corpus found it, by being written through that path.
///
/// Partial, because a message recorded as present before its envelope arrived may have no
/// conversation identifier, and neither column is ever looked up by its absence. **Not
/// unique**: that would be a claim about provider identifiers this layer has no standing to
/// make, and a violation would turn a delta page into a refused transaction.
pub const STORE_MIGRATION_V2: &str = r"
CREATE INDEX message_remote ON message(remote_id) WHERE remote_id IS NOT NULL;
CREATE INDEX thread_remote ON thread(remote_thread_id) WHERE remote_thread_id IS NOT NULL;
";

/// Apply a schema to a fresh connection.
pub fn create(conn: &Connection, sql: &str, version: u32) -> SqlResult<()> {
    conn.execute_batch(sql)?;
    conn.pragma_update(None, "user_version", version)?;
    Ok(())
}

/// What one version adds to each half, in D-74's order: `(journal, store)`.
const fn step(version: u32) -> (&'static str, &'static str) {
    match version {
        2 => ("", STORE_MIGRATION_V2),
        _ => ("", ""),
    }
}

/// Carry both halves forward from `from` to [`CURRENT_VERSION`], one version at a time.
///
/// Each version is one transaction per half, and the half's version is stamped **inside** that
/// transaction, so a half is never left with a version its contents do not match. **Journal
/// first, store second** — D-74's order for everything written to the pair. A crash between the
/// two leaves the halves disagreeing, which the next open refuses rather than guesses at; that
/// is D-74's rule, and the window is the length of one index build.
///
/// A fresh account runs this too, from version 1, so there is one path to the current schema
/// rather than a creation script and a migration chain that could drift apart.
///
/// # Errors
/// The engine refused a statement. The half that failed is left at the version it had.
pub fn migrate(store: &Connection, journal: &Connection, from: u32) -> SqlResult<()> {
    for version in from.saturating_add(1)..=CURRENT_VERSION {
        let (journal_sql, store_sql) = step(version);
        for (conn, sql) in [(journal, journal_sql), (store, store_sql)] {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", version)?;
            tx.commit()?;
        }
    }
    Ok(())
}

/// The version stamped in a file. `0` means "fresh".
pub fn version_of(conn: &Connection) -> SqlResult<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map(|v| u32::try_from(v).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_store_is_refused_rather_than_guessed_at() {
        // D-32: "a refusal to open a database newer than the running binary understands."
        // Guessing would discard writes the user watched succeed.
        assert_eq!(
            admit(CURRENT_VERSION + 1),
            Err(SchemaError::TooNew {
                found: CURRENT_VERSION + 1,
                understood: CURRENT_VERSION
            })
        );
    }

    #[test]
    fn a_store_below_the_floor_is_refused() {
        // D-62's floor. Pruning too early strands users the project never knew it had,
        // which is why advancing it is an amendment rather than a deletion.
        let below = MIGRATION_FLOOR.saturating_sub(1);
        if below > 0 {
            assert!(matches!(admit(below), Err(SchemaError::BelowFloor { .. })));
        }
    }

    #[test]
    fn halves_that_disagree_do_not_open() {
        // D-74: "a build MUST refuse to open an account whose two halves disagree about
        // their version." A store migrated without its journal is an account whose overlay
        // no longer describes its base.
        assert_eq!(
            admit_pair(1, 0),
            Err(SchemaError::HalvesDisagree {
                store: 1,
                journal: 0
            })
        );
    }

    #[test]
    fn a_current_pair_needs_no_migration() {
        assert_eq!(
            admit_pair(CURRENT_VERSION, CURRENT_VERSION),
            Ok(Migration::None)
        );
    }

    /// A floor above what this build writes would refuse every store it creates. Checked at
    /// compile time, because it is a property of two constants rather than of a run.
    const _: () = assert!(MIGRATION_FLOOR <= CURRENT_VERSION);

    #[test]
    fn the_store_schema_holds_no_credentials_and_no_contacts() {
        // Two absences that are decisions rather than omissions. NFR-23 puts credentials
        // only in the OS credential store; D-41 resolves contact names on demand and keeps
        // nothing, because a contact entity would be a second sync domain scope excludes.
        let s = STORE_SCHEMA_V1.to_lowercase();
        for forbidden in ["token", "password", "credential", "refresh", "contact"] {
            assert!(
                !s.contains(forbidden),
                "the store schema mentions `{forbidden}`"
            );
        }
    }

    #[test]
    fn the_queue_is_in_the_journal_and_not_in_the_store() {
        // D-74 exists so that D-57 can exclude the cache from backup without excluding the
        // queue with it. If the queue were in the store, that would be unimplementable.
        assert!(JOURNAL_SCHEMA_V1.contains("CREATE TABLE intent"));
        assert!(!STORE_SCHEMA_V1.contains("CREATE TABLE intent"));
        assert!(JOURNAL_SCHEMA_V1.contains("CREATE TABLE overlay"));
        assert!(!STORE_SCHEMA_V1.contains("CREATE TABLE overlay"));
    }
}

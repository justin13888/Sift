//! D-82 — a folder is a state machine, and its cursor is taken before its backfill.
//!
//! # Push and delta are not the same thing
//!
//! **Push answers "has something changed?" — a doorbell. Delta answers "what changed?" —
//! the authoritative change feed, resumed from a cursor.** There is exactly one code path
//! that applies change, and it is the delta. A push that carried the change itself would be
//! a second such path, with its own bugs, exercised only on the providers that offer it.
//!
//! # Cursor-first is normative
//!
//! A new folder's delta cursor is acquired **before** its backfill begins. The alternative
//! — backfill first, then take a cursor — loses every change that happened during the walk,
//! which "on a large mailbox is hours". That is silent data loss, and it is silent in the
//! worst way: the messages were never seen, so nothing is inconsistent afterwards.
//!
//! Cursor-first instead makes early deltas **redundant** rather than lost, which is only
//! safe because reapplication is safe — a property D-82 asserts rather than measures, and
//! records as the weak point of the choice.

use sift_provider::adapter::Provenance;

/// D-82's six states.
///
/// `Unwatched` is deliberately **not** among them: an unwatched folder stops being
/// scheduled, and its stored state is *retained* so that re-watching does not restart from
/// `Unsynced`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderState {
    /// Known to exist, never contacted.
    Unsynced,
    /// Cursor held, walking history newest-first under D-53.
    Backfilling,
    /// Cursor current, deltas applied as they arrive.
    Live,
    /// The cursor is no longer accepted by the server.
    Invalidated,
    /// Re-establishing under NFR-18. The account shows *recovering*, which is progress
    /// rather than a fault.
    Recovering,
    /// A stated reason, surfaced under NFR-29.
    Degraded,
}

/// Why a folder left a state. Modelled so the transitions can be exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The scheduler reached this folder for the first time.
    FirstTurn,
    /// The backfill walked to the end of history.
    BackfillComplete,
    /// The server refused the cursor — a validity change, an expired link, a history
    /// identifier outside the retained window.
    CursorInvalidated,
    /// Recovery finished a complete enumeration.
    RecoveryComplete,
    /// A failure that is not transient.
    NonTransientFailure,
    /// Whatever was wrong cleared.
    Recovered,
    /// The user took the folder out of FR-43's watched set.
    Unwatched,
}

impl FolderState {
    /// The transition table, as D-82 states it.
    ///
    /// Returns `None` where the event does not apply, rather than inventing a transition —
    /// a folder that could reach any state from any event would not be a state machine.
    #[must_use]
    pub const fn on(self, event: Event) -> Option<Self> {
        use Event as E;
        use FolderState as S;
        Some(match (self, event) {
            (S::Unsynced, E::FirstTurn) => S::Backfilling,

            (S::Backfilling, E::BackfillComplete) => S::Live,
            // "Invalidated if the cursor dies mid-walk" — the cursor was taken first, so
            // there is one to lose.
            (S::Backfilling, E::CursorInvalidated) => S::Invalidated,
            (S::Backfilling, E::NonTransientFailure) => S::Degraded,

            (S::Live, E::CursorInvalidated) => S::Invalidated,
            (S::Live, E::NonTransientFailure) => S::Degraded,

            // Invalidated leaves to Recovering *immediately*: it is not a resting state, and
            // NFR-18 forbids requiring the user to ask.
            (S::Invalidated, _) => S::Recovering,

            (S::Recovering, E::RecoveryComplete) => S::Live,
            (S::Recovering, E::NonTransientFailure) => S::Degraded,

            (S::Degraded, E::Recovered) => S::Live,

            _ => return None,
        })
    }

    /// Whether the scheduler should be giving this folder turns.
    #[must_use]
    pub const fn wants_scheduling(self) -> bool {
        !matches!(self, Self::Degraded)
    }

    /// Whether the account should show *recovering* on this folder's account.
    ///
    /// D-49 makes that condition progress rather than a fault, and the user action is
    /// nothing — which is why a backfill and a cursor recovery share it.
    #[must_use]
    pub const fn is_progress(self) -> bool {
        matches!(self, Self::Backfilling | Self::Recovering)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unsynced => "Unsynced",
            Self::Backfilling => "Backfilling",
            Self::Live => "Live",
            Self::Invalidated => "Invalidated",
            Self::Recovering => "Recovering",
            Self::Degraded => "Degraded",
        }
    }
}

/// What a folder's lifecycle does when it stops being enumerated — D-83.
///
/// **A folder that disappears is retired, not deleted.** Its messages are treated as no
/// longer present *in it*; on a one-location provider they are then in no location at all
/// and are reachable only through search and threads. They are evicted by NFR-52 in the
/// ordinary way rather than deleted here.
///
/// D-83 records the cost honestly: a retired folder's state must be visible somewhere **or
/// it becomes a place mail hides**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// First time seen. Enters `Unsynced`, and is backfilled **only if it is in FR-43's
    /// watched set**.
    Discovered,
    /// Still enumerated.
    Present,
    /// Gone from enumeration. Retired rather than deleted, because deleting the row would
    /// take the local identity FR-43's watched set and the per-folder notification rules
    /// are keyed on.
    Retired,
}

/// Whether a rename can be recognised as one.
///
/// Where the provider's folder identity is stable, a rename is a rename. Where it is not —
/// plain IMAP without object identifiers — **Sift MUST NOT guess**: the old folder is
/// retired and the new one is discovered, losing the watched flag and that folder's
/// notification rules, and nothing else.
///
/// Guessing would be worse than losing a preference: a wrong match moves a mailbox's worth
/// of messages into a folder they were never in.
#[must_use]
pub const fn rename_is_recognisable(identity_is_stable: bool) -> bool {
    identity_is_stable
}

/// Everything cursor recovery observes is *discovered*, never *arrived*.
///
/// D-84 is explicit that this includes rows recovery **inserts**. The reason is FR-23: a
/// message is new when it was delivered and was unread at that moment, and a recovery
/// re-reading a mailbox that was always there has delivered nothing. Without this rule a
/// cursor invalidation would notify the user about every message in the folder.
///
/// D-102 applies the same rule to re-ingest after eviction, for the same reason.
#[must_use]
pub const fn recovery_provenance() -> Provenance {
    Provenance::Discovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_folder_backfills_on_its_first_turn() {
        assert_eq!(
            FolderState::Unsynced.on(Event::FirstTurn),
            Some(FolderState::Backfilling)
        );
    }

    #[test]
    fn a_cursor_can_die_mid_walk_because_it_was_taken_first() {
        // Cursor-first is what makes this transition possible at all. Under backfill-first
        // there would be no cursor to lose, and every change during the walk would be gone
        // without anything noticing.
        assert_eq!(
            FolderState::Backfilling.on(Event::CursorInvalidated),
            Some(FolderState::Invalidated)
        );
    }

    #[test]
    fn invalidation_leaves_immediately_and_never_rests() {
        // NFR-18: recovery is automatic. Invalidated is a transition, not a place to sit
        // waiting for the user to ask.
        for e in [
            Event::FirstTurn,
            Event::BackfillComplete,
            Event::CursorInvalidated,
            Event::NonTransientFailure,
            Event::Recovered,
        ] {
            assert_eq!(
                FolderState::Invalidated.on(e),
                Some(FolderState::Recovering)
            );
        }
    }

    #[test]
    fn recovery_returns_to_live_only_on_completion() {
        assert_eq!(
            FolderState::Recovering.on(Event::RecoveryComplete),
            Some(FolderState::Live)
        );
        assert_eq!(FolderState::Recovering.on(Event::BackfillComplete), None);
    }

    #[test]
    fn a_degraded_folder_stops_being_scheduled_until_the_cause_clears() {
        assert!(!FolderState::Degraded.wants_scheduling());
        assert_eq!(
            FolderState::Degraded.on(Event::Recovered),
            Some(FolderState::Live)
        );
    }

    #[test]
    fn events_that_do_not_apply_invent_no_transition() {
        // A machine that could reach any state from any event would not be one.
        assert_eq!(FolderState::Live.on(Event::FirstTurn), None);
        assert_eq!(FolderState::Unsynced.on(Event::BackfillComplete), None);
    }

    #[test]
    fn backfilling_and_recovering_are_progress_rather_than_faults() {
        // D-49 gives *recovering* no user action, because there is nothing for the user to
        // do. Presenting either as a fault would be the dishonesty FR-12 rejects elsewhere.
        assert!(FolderState::Backfilling.is_progress());
        assert!(FolderState::Recovering.is_progress());
        assert!(!FolderState::Degraded.is_progress());
        assert!(!FolderState::Live.is_progress());
    }

    #[test]
    fn unwatched_is_not_one_of_the_six() {
        // An unwatched folder stops being scheduled and *retains* its state, so re-watching
        // does not restart from Unsynced — which on a large mailbox would be hours.
        for s in [
            FolderState::Unsynced,
            FolderState::Backfilling,
            FolderState::Live,
            FolderState::Recovering,
            FolderState::Degraded,
        ] {
            assert_eq!(
                s.on(Event::Unwatched),
                None,
                "{} treated unwatched as a state",
                s.name()
            );
        }
    }

    #[test]
    fn a_rename_is_guessed_at_only_where_identity_is_stable() {
        assert!(rename_is_recognisable(true));
        assert!(!rename_is_recognisable(false));
    }

    #[test]
    fn nothing_recovery_observes_is_new_mail() {
        assert_eq!(recovery_provenance(), Provenance::Discovered);
    }
}

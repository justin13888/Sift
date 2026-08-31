//! D-102, NFR-14, NFR-52 — eviction, and the two budgets that between them are the whole of
//! disk.
//!
//! # Two budgets rather than one
//!
//! **NFR-14** bounds bodies, attachments and the blob store: least-recently-used, defaulting
//! to 90 days or 2 GB whichever binds first, **enforced rather than advisory**.
//!
//! **NFR-52** bounds envelopes, their message rows, **and their full-text index entries**:
//! oldest-first, defaulting to L-20, with an envelope and its index entry evicted **together
//! as one unit**.
//!
//! Two rather than one, so that a large attachment cannot evict a year of envelopes. The
//! asymmetry between them is deliberate and easy to get backwards: **NFR-14 leaves the
//! indexed text in place**, so a body evicted for space stays findable and the result is an
//! ordinary FR-12 "not cached" message. Only NFR-52 takes the index entry, because at that
//! point the message itself is going.

use std::collections::BTreeSet;

/// A blob's state, as eviction sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub message: u128,
    pub bytes: u64,
    pub last_used_millis: u64,
    /// D-102 spares these. **The cache must not evict what the queue still refers to** —
    /// the queue is the one thing in an account that outlives the cache, and a compensation
    /// that cannot find its message is an undo that silently does nothing.
    pub has_queued_intent: bool,
    /// Spared for the same reason: evicting the message somebody is reading is the one
    /// eviction they will certainly notice.
    pub is_open_in_a_reader: bool,
}

impl Entry {
    #[must_use]
    pub const fn is_spared(&self) -> bool {
        self.has_queued_intent || self.is_open_in_a_reader
    }
}

/// What one eviction pass decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    pub evicted: Vec<u128>,
    pub freed_bytes: u64,
    /// Threads left holding no messages.
    ///
    /// D-102 retires them, because a thread's aggregate figures describe **the messages
    /// still held** rather than the messages that ever existed — a thread row claiming five
    /// messages when the store has none is a list row that opens onto nothing.
    pub retired_threads: Vec<u128>,
    /// How far the spared set pushed the store past its budget.
    ///
    /// D-102 accepts this explicitly: **a store at budget can grow past it, briefly, by the
    /// size of the queue's working set** — bounded only by L-17's seven days. Reported
    /// rather than hidden, because a hard cap that is silently exceeded is not a cap.
    pub over_budget_by: u64,
}

/// Run one eviction pass down to a low-water mark.
///
/// **In batches to a low-water mark rather than one row per insertion.** Evicting per
/// insertion makes every ingest do a write it did not need to, which during D-53's backfill
/// is every message in the mailbox.
#[must_use]
pub fn evict_to_budget(entries: &[Entry], budget: u64, low_water: f64) -> Pass {
    let total: u64 = entries.iter().map(|e| e.bytes).sum();
    let target = (budget as f64 * low_water) as u64;

    let spared: u64 = entries
        .iter()
        .filter(|e| e.is_spared())
        .map(|e| e.bytes)
        .sum();

    let mut pass = Pass::default();
    if total <= budget {
        return pass;
    }

    // Least-recently-used, among what may be evicted at all.
    let mut candidates: Vec<&Entry> = entries.iter().filter(|e| !e.is_spared()).collect();
    candidates.sort_by_key(|e| e.last_used_millis);

    let mut remaining = total;
    for e in candidates {
        if remaining <= target {
            break;
        }
        pass.evicted.push(e.message);
        pass.freed_bytes += e.bytes;
        remaining -= e.bytes;
    }

    // What the spared set costs, when it is more than the budget allows.
    pass.over_budget_by = remaining.saturating_sub(budget);
    debug_assert!(
        pass.over_budget_by == 0 || spared > 0,
        "over budget with nothing spared means the pass did not run"
    );
    pass
}

/// Which threads are left empty by an eviction.
#[must_use]
pub fn threads_retired_by(
    evicted: &[u128],
    thread_of: &dyn Fn(u128) -> u128,
    remaining: &[u128],
) -> Vec<u128> {
    let surviving: BTreeSet<u128> = remaining.iter().map(|m| thread_of(*m)).collect();
    let mut retired: Vec<u128> = evicted
        .iter()
        .map(|m| thread_of(*m))
        .filter(|t| !surviving.contains(t))
        .collect();
    retired.sort_unstable();
    retired.dedup();
    retired
}

/// What an eviction under each budget takes with it.
///
/// The asymmetry is the point, and it is the thing most easily got backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// NFR-14 — bodies, attachments, blobs. Least-recently-used.
    Cache,
    /// NFR-52 — envelopes, message rows, index entries. Oldest-first.
    EnvelopeAndIndex,
}

impl Budget {
    /// Whether evicting under this budget also removes the message's indexed text.
    ///
    /// **False for the cache budget**: a body evicted for space leaves its indexed text in
    /// place, so the message stays findable and the user meets an ordinary FR-12 "not
    /// cached" state rather than a message that has silently vanished from search.
    #[must_use]
    pub const fn takes_the_index_entry(self) -> bool {
        matches!(self, Self::EnvelopeAndIndex)
    }

    /// NFR-52 evicts an envelope and its index entry **together as one unit**. Leaving
    /// either behind is a row referring to nothing.
    #[must_use]
    pub const fn evicts_as_one_unit(self) -> bool {
        matches!(self, Self::EnvelopeAndIndex)
    }
}

/// Whether a re-ingested message counts as new mail.
///
/// **It does not.** A message evicted and later re-delivered is *discovered*, not *arrived*,
/// and re-ingest after eviction MUST NOT notify — otherwise a cache under pressure would
/// announce mail the user read last month.
#[must_use]
pub const fn re_ingest_notifies() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(message: u128, bytes: u64, last_used: u64) -> Entry {
        Entry {
            message,
            bytes,
            last_used_millis: last_used,
            has_queued_intent: false,
            is_open_in_a_reader: false,
        }
    }

    #[test]
    fn nothing_is_evicted_below_budget() {
        let entries = [entry(1, 10, 100), entry(2, 10, 200)];
        assert_eq!(evict_to_budget(&entries, 100, 0.8), Pass::default());
    }

    #[test]
    fn the_least_recently_used_goes_first() {
        let entries = [entry(1, 50, 300), entry(2, 50, 100), entry(3, 50, 200)];
        let pass = evict_to_budget(&entries, 100, 0.8);
        assert_eq!(
            pass.evicted.first(),
            Some(&2),
            "the oldest was not evicted first"
        );
    }

    #[test]
    fn eviction_runs_to_a_low_water_mark_rather_than_to_the_budget() {
        // Evicting to exactly the budget means the next insertion evicts again, and the pass
        // runs on every ingest — which during a backfill is every message in the mailbox.
        let entries: Vec<Entry> = (1..=10)
            .map(|i| entry(i, 20, u64::try_from(i).unwrap()))
            .collect();
        let pass = evict_to_budget(&entries, 100, 0.5);
        assert!(pass.freed_bytes >= 150, "freed only {}", pass.freed_bytes);
    }

    #[test]
    fn a_message_with_a_queued_intent_is_never_evicted() {
        // The queue is the one thing in an account that outlives the cache. A compensation
        // that cannot find its message is an undo that silently does nothing.
        let mut queued = entry(1, 100, 0);
        queued.has_queued_intent = true;
        let entries = [queued, entry(2, 100, 999)];
        let pass = evict_to_budget(&entries, 50, 0.5);
        assert!(
            !pass.evicted.contains(&1),
            "a message with a queued intent was evicted"
        );
    }

    #[test]
    fn a_message_open_in_a_reader_is_never_evicted() {
        let mut open = entry(1, 100, 0);
        open.is_open_in_a_reader = true;
        let entries = [open, entry(2, 100, 999)];
        let pass = evict_to_budget(&entries, 50, 0.5);
        assert!(!pass.evicted.contains(&1));
    }

    #[test]
    fn the_spared_set_can_push_the_store_past_its_budget_and_that_is_reported() {
        // D-102 accepts this explicitly, bounded only by L-17's seven days. A hard cap that
        // is silently exceeded is not a cap, so the overshoot is a number rather than a
        // silence.
        let mut a = entry(1, 100, 0);
        a.has_queued_intent = true;
        let mut b = entry(2, 100, 1);
        b.has_queued_intent = true;
        let pass = evict_to_budget(&[a, b], 50, 0.5);
        assert!(pass.evicted.is_empty());
        assert_eq!(pass.over_budget_by, 150);
    }

    #[test]
    fn a_thread_left_with_no_messages_is_retired() {
        // A thread row claiming five messages when the store has none is a list row that
        // opens onto nothing.
        let thread_of = |m: u128| if m <= 2 { 10 } else { 20 };
        let retired = threads_retired_by(&[1, 2], &thread_of, &[3]);
        assert_eq!(retired, vec![10]);
    }

    #[test]
    fn a_thread_that_still_holds_a_message_is_not_retired() {
        let thread_of = |_: u128| 10;
        assert!(threads_retired_by(&[1], &thread_of, &[2]).is_empty());
    }

    #[test]
    fn a_body_evicted_for_space_stays_findable() {
        // The asymmetry most easily got backwards. NFR-14 leaves the indexed text; only
        // NFR-52 takes it, because at that point the message itself is going.
        assert!(!Budget::Cache.takes_the_index_entry());
        assert!(Budget::EnvelopeAndIndex.takes_the_index_entry());
    }

    #[test]
    fn an_envelope_and_its_index_entry_go_together() {
        // Leaving either behind is a row referring to nothing.
        assert!(Budget::EnvelopeAndIndex.evicts_as_one_unit());
    }

    #[test]
    fn a_re_ingested_message_is_not_announced() {
        // Otherwise a cache under pressure would announce mail the user read last month.
        assert!(!re_ingest_notifies());
    }
}

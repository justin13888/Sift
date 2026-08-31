//! The durable queue: ordering, coalescing, batching, and D-51's overlay.

use crate::intent::{Intent, Queued, State};
use sift_foundation::identity::LocalId;
use std::collections::BTreeMap;

/// What happens when two intents against one message meet in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coalesced {
    /// Both survive. The default, and the only safe answer when in doubt.
    Keep,
    /// Both disappear. Permitted only where the collapsed sequence has **the same effect at
    /// the server** — which for a pair that cancels out means no request at all.
    Annihilate,
    /// The later one subsumes the earlier.
    LaterWins,
}

/// Whether two consecutive intents against one message may collapse.
///
/// > "Coalescing is permitted only where the collapsed sequence has the same effect **at
/// > the server**."
///
/// The saving D-86 relies on arrives here for free: an archive and its compensation inside
/// one flush interval have no net server effect and collapse to nothing, **with no
/// mechanism that knows what an undo window is**. That is why the undo window withholds
/// nothing.
#[must_use]
pub fn coalesce(earlier: &Intent, later: &Intent) -> Coalesced {
    use Intent as I;
    match (earlier, later) {
        // Flag state is a final state: the server ends up somewhere, and a pair that returns
        // it to where it started is a pair the server never needed to hear about.
        (I::MarkRead, I::MarkUnread) | (I::MarkUnread, I::MarkRead) => Coalesced::Annihilate,
        (I::Flag, I::Unflag) | (I::Unflag, I::Flag) => Coalesced::Annihilate,
        (I::AddTag { name: a }, I::RemoveTag { name: b })
        | (I::RemoveTag { name: a }, I::AddTag { name: b })
            if a == b =>
        {
            Coalesced::Annihilate
        }

        // Repeats and overwrites of a final state.
        (I::MarkRead, I::MarkRead)
        | (I::MarkUnread, I::MarkUnread)
        | (I::Flag, I::Flag)
        | (I::Unflag, I::Unflag) => Coalesced::LaterWins,
        (I::MoveTo { .. }, I::MoveTo { .. }) => Coalesced::LaterWins,
        // Archive is a move, so a later move subsumes it.
        (I::Archive, I::MoveTo { .. }) => Coalesced::LaterWins,

        // **A report has a durable remote effect and no final state to reason about.**
        // Reporting junk trains a classifier; reporting not-junk trains it back. The two
        // together are not "nothing happened at the server" — they are two things that
        // happened. This is the pair the rule exists to exclude.
        (I::ReportJunk, I::ReportNotJunk) | (I::ReportNotJunk, I::ReportJunk) => Coalesced::Keep,

        // Permanent delete is never optimistic and never coalesced with anything: there is
        // no state to collapse toward.
        (_, I::PermanentlyDelete) | (I::PermanentlyDelete, _) => Coalesced::Keep,

        _ => Coalesced::Keep,
    }
}

/// D-51 — the pending overlay.
///
/// Local state is **base**, written by the authoritative delta, plus an overlay contributed
/// by enqueued unresolved intents. Everything the user sees reads *through* the overlay,
/// and **the delta never writes through it**.
///
/// The failure this prevents, stated concretely: a user archives a message, the row leaves
/// the list, a stale delta lands still describing the message as in the inbox, D-38
/// classifies that as concurrent change and corrects it silently — **and the archive
/// silently undoes itself**.
///
/// It is also what makes D-38 implementable at all: if the overlay is still present when
/// the server reports a conflicting state, *Sift's own intent failed*; if it was already
/// retired, the change was made elsewhere. Those are the two cases D-38 treats differently.
#[derive(Debug, Default, Clone)]
pub struct Overlay {
    per_message: BTreeMap<LocalId, Vec<Intent>>,
}

impl Overlay {
    /// Rebuild from the queue.
    ///
    /// This is what startup does. The overlay is written to the journal only so it survives
    /// termination; on the message path it is read from memory, and it is **derived from
    /// the queue** — which is what makes D-74's crash window self-correcting.
    #[must_use]
    pub fn rebuild(queue: &[Queued]) -> Self {
        let mut per_message: BTreeMap<LocalId, Vec<Intent>> = BTreeMap::new();
        let mut ordered: Vec<&Queued> = queue
            .iter()
            .filter(|q| q.state.contributes_overlay())
            .collect();
        ordered.sort_by_key(|q| (q.message, q.sequence));
        for q in ordered {
            per_message
                .entry(q.message)
                .or_default()
                .push(q.intent.clone());
        }
        Self { per_message }
    }

    /// The intents overlaying a message, in issue order.
    #[must_use]
    pub fn for_message(&self, message: LocalId) -> &[Intent] {
        self.per_message.get(&message).map_or(&[], Vec::as_slice)
    }

    /// Whether a message is hidden from the current view by an unresolved intent.
    ///
    /// D-102 uses this: **eviction MUST NOT evict what the queue still refers to**, because
    /// the queue is the one thing in an account that outlives the cache.
    #[must_use]
    pub fn covers(&self, message: LocalId) -> bool {
        self.per_message.contains_key(&message)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.per_message.is_empty()
    }
}

/// The durable queue, in memory. Persistence is the journal's.
#[derive(Debug, Default)]
pub struct Queue {
    entries: Vec<Queued>,
    next_sequence: BTreeMap<LocalId, u64>,
}

impl Queue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue an intent, assigning its per-message sequence.
    ///
    /// Coalescing runs against the last unresolved intent for the same message, because
    /// that is the only one it can be adjacent to at the server.
    pub fn enqueue(&mut self, id: u128, message: LocalId, intent: Intent, now_millis: u64) -> u128 {
        if let Some(last) = self
            .entries
            .iter()
            .rposition(|q| q.message == message && q.state == State::Pending)
        {
            match coalesce(&self.entries[last].intent, &intent) {
                Coalesced::Annihilate => {
                    self.entries.remove(last);
                    return id;
                }
                Coalesced::LaterWins => {
                    self.entries[last].intent = intent;
                    return self.entries[last].id;
                }
                Coalesced::Keep => {}
            }
        }
        let seq = self.next_sequence.entry(message).or_insert(0);
        let sequence = *seq;
        *seq += 1;
        self.entries.push(Queued {
            id,
            undo_group: None,
            message,
            intent,
            state: State::Pending,
            attempts: 0,
            created_millis: now_millis,
            sequence,
        });
        id
    }

    /// The next batch to issue.
    ///
    /// Two rules, and they pull against each other, which is why they are stated together:
    ///
    /// - **Intents against one message are issued strictly in sequence, and a batch MUST NOT
    ///   contain two intents for the same message.** Archive-then-move and move-then-archive
    ///   are different outcomes.
    /// - **Intents against different messages are unordered**, which is exactly what makes
    ///   batching possible at all.
    #[must_use]
    pub fn next_batch(&self, limit: u32) -> Vec<&Queued> {
        let mut seen = std::collections::BTreeSet::new();
        let mut batch = Vec::new();
        let mut pending: Vec<&Queued> = self
            .entries
            .iter()
            .filter(|q| q.state == State::Pending)
            .collect();
        pending.sort_by_key(|q| (q.message, q.sequence));
        for q in pending {
            if batch.len() >= limit as usize {
                break;
            }
            if seen.insert(q.message) {
                batch.push(q);
            }
        }
        batch
    }

    /// Move everything that was in flight when the process died into `Reconciling`.
    ///
    /// D-85: restart moves `Issued` to `Reconciling`, because a request that went out and
    /// whose answer never came back is exactly that situation — and inferring
    /// sent-versus-unsent at startup cannot be done correctly, which is why the transition
    /// into `Issued` is durable in the first place.
    pub fn recover_after_restart(&mut self) {
        for q in &mut self.entries {
            if q.state == State::Issued {
                q.state = State::Reconciling;
            }
        }
    }

    /// Expire anything past L-17, and retire its overlay.
    ///
    /// Returns what expired, so the account can enter *attention* and the user can be told
    /// which intent stopped. **Neither dropped nor retried forever**: the row stays in the
    /// queue and stays visible under FR-34.
    pub fn expire(&mut self, now_millis: u64) -> Vec<u128> {
        let mut expired = Vec::new();
        for q in &mut self.entries {
            if q.state.contributes_overlay() && q.is_expired(now_millis) {
                q.state = State::Expired;
                expired.push(q.id);
            }
        }
        expired
    }

    /// Quarantine an intent whose gating capability has gone away.
    ///
    /// Recognised, legal when enqueued, and no longer permitted. **Never executed and never
    /// discarded** — surfaced under NFR-48 so a later build or a restored capability can
    /// run it.
    pub fn quarantine(&mut self, id: u128) -> bool {
        for q in &mut self.entries {
            if q.id == id {
                q.state = State::Quarantined;
                return true;
            }
        }
        false
    }

    pub fn set_state(&mut self, id: u128, state: State) -> bool {
        for q in &mut self.entries {
            if q.id == id {
                q.state = state;
                return true;
            }
        }
        false
    }

    /// Remove settled rows.
    pub fn collect_settled(&mut self) {
        self.entries.retain(|q| q.state != State::Settled);
    }

    #[must_use]
    pub fn entries(&self) -> &[Queued] {
        &self.entries
    }

    #[must_use]
    pub fn overlay(&self) -> Overlay {
        Overlay::rebuild(&self.entries)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_foundation::limits::L17_INTENT_EXPIRY;

    fn msg(n: u128) -> LocalId {
        LocalId::from_u128(n)
    }

    #[test]
    fn a_pair_that_returns_the_server_to_where_it_started_collapses() {
        assert_eq!(
            coalesce(&Intent::MarkRead, &Intent::MarkUnread),
            Coalesced::Annihilate
        );
        assert_eq!(
            coalesce(&Intent::Flag, &Intent::Unflag),
            Coalesced::Annihilate
        );
        assert_eq!(
            coalesce(
                &Intent::AddTag {
                    name: "work".into()
                },
                &Intent::RemoveTag {
                    name: "work".into()
                }
            ),
            Coalesced::Annihilate
        );
    }

    #[test]
    fn tags_with_different_names_do_not_cancel() {
        assert_eq!(
            coalesce(
                &Intent::AddTag {
                    name: "work".into()
                },
                &Intent::RemoveTag {
                    name: "home".into()
                }
            ),
            Coalesced::Keep
        );
    }

    #[test]
    fn a_report_never_collapses_with_its_opposite() {
        // The pair the rule exists to exclude. Reporting junk trains a classifier and
        // reporting not-junk trains it back: that is two things that happened at the server,
        // not "nothing happened". There is no final state to reason about.
        assert_eq!(
            coalesce(&Intent::ReportJunk, &Intent::ReportNotJunk),
            Coalesced::Keep
        );
        assert_eq!(
            coalesce(&Intent::ReportNotJunk, &Intent::ReportJunk),
            Coalesced::Keep
        );
    }

    #[test]
    fn permanent_delete_coalesces_with_nothing() {
        for other in [Intent::Archive, Intent::MarkRead, Intent::Flag] {
            assert_eq!(
                coalesce(&other, &Intent::PermanentlyDelete),
                Coalesced::Keep
            );
            assert_eq!(
                coalesce(&Intent::PermanentlyDelete, &other),
                Coalesced::Keep
            );
        }
    }

    #[test]
    fn an_undo_inside_one_flush_interval_costs_nothing() {
        // D-86's saving, arriving for free: "archive and its compensation within one flush
        // interval have no net server effect and collapse to nothing, with no mechanism
        // that knows what an undo window is". That is *why* the undo window withholds
        // nothing.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::MarkRead, 0);
        q.enqueue(2, msg(1), Intent::MarkUnread, 0);
        assert!(q.is_empty(), "the pair survived to become two round trips");
    }

    #[test]
    fn intents_against_one_message_keep_their_order() {
        // Archive-then-move and move-then-archive are different outcomes, so the order is
        // preserved always — including through batching and retry.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.enqueue(2, msg(1), Intent::AddTag { name: "x".into() }, 0);
        let seqs: Vec<u64> = q.entries().iter().map(|e| e.sequence).collect();
        assert_eq!(seqs, vec![0, 1]);
    }

    #[test]
    fn a_batch_never_contains_two_intents_for_one_message() {
        // The rule that pulls against batching, and the one that must win. Two intents for
        // one message in a batch is two operations the provider may apply in either order.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.enqueue(2, msg(1), Intent::AddTag { name: "x".into() }, 0);
        q.enqueue(3, msg(2), Intent::Archive, 0);
        let batch = q.next_batch(50);
        assert_eq!(
            batch.len(),
            2,
            "the batch collapsed two messages into one slot"
        );
        let messages: Vec<LocalId> = batch.iter().map(|q| q.message).collect();
        let mut unique = messages.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            messages.len(),
            unique.len(),
            "a message appeared twice in one batch"
        );
    }

    #[test]
    fn a_batch_takes_the_earliest_intent_for_each_message() {
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.enqueue(2, msg(1), Intent::Flag, 0);
        let batch = q.next_batch(10);
        assert_eq!(batch[0].sequence, 0, "the later intent was issued first");
    }

    #[test]
    fn a_batch_respects_the_declared_maximum() {
        let mut q = Queue::new();
        for i in 0..100u128 {
            q.enqueue(i, msg(i), Intent::Archive, 0);
        }
        assert_eq!(q.next_batch(20).len(), 20);
    }

    #[test]
    fn restart_moves_everything_in_flight_to_reconciling() {
        // D-85: a request that went out and whose answer never came back is exactly this
        // situation, and inferring sent-versus-unsent at startup cannot be done correctly —
        // which is what the durable Issued marker buys.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.set_state(1, State::Issued);
        q.recover_after_restart();
        assert_eq!(q.entries()[0].state, State::Reconciling);
    }

    #[test]
    fn an_intent_expires_on_elapsed_time_rather_than_attempts() {
        // "An intent that failed twice in a week offline and one that failed two hundred
        // times in a minute are not the same situation."
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);

        let ms = L17_INTENT_EXPIRY.as_millis() as u64;
        assert!(q.expire(ms - 1).is_empty(), "expired before L-17");
        assert_eq!(q.expire(ms), vec![1]);
        assert_eq!(q.entries()[0].state, State::Expired);
    }

    #[test]
    fn an_expired_intent_is_neither_dropped_nor_retried_forever() {
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.expire(L17_INTENT_EXPIRY.as_millis() as u64);
        assert_eq!(q.len(), 1, "the intent was dropped");
        assert!(
            q.next_batch(10).is_empty(),
            "an expired intent was reissued"
        );
        assert!(q.entries()[0].state.needs_attention());
    }

    #[test]
    fn an_expired_or_quarantined_intent_retires_its_overlay() {
        // So the base state shows through, and the user is told which intent stopped rather
        // than seeing a local state nothing will ever make true.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        assert!(q.overlay().covers(msg(1)));

        q.expire(L17_INTENT_EXPIRY.as_millis() as u64);
        assert!(
            !q.overlay().covers(msg(1)),
            "an expired intent still hid the message"
        );

        let mut q2 = Queue::new();
        q2.enqueue(2, msg(2), Intent::AddTag { name: "x".into() }, 0);
        q2.quarantine(2);
        assert!(!q2.overlay().covers(msg(2)));
    }

    #[test]
    fn a_quarantined_intent_is_never_executed_and_never_discarded() {
        // NFR-48. Held, visible under FR-34, and executable by a later build or a restored
        // capability.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::AddTag { name: "x".into() }, 0);
        assert!(q.quarantine(1));
        assert_eq!(q.len(), 1, "quarantining discarded the intent");
        assert!(
            q.next_batch(10).is_empty(),
            "a quarantined intent was issued"
        );
    }

    #[test]
    fn the_overlay_is_rebuilt_from_the_queue_rather_than_stored_beside_the_base() {
        // What makes D-74's crash window self-correcting: an intent enqueued whose effect
        // was never applied is recovered by rebuilding, because the overlay is *derived*.
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.enqueue(2, msg(2), Intent::Flag, 0);
        let overlay = Overlay::rebuild(q.entries());
        assert!(overlay.covers(msg(1)) && overlay.covers(msg(2)));
        assert_eq!(overlay.for_message(msg(1)), &[Intent::Archive]);
        assert!(overlay.for_message(msg(99)).is_empty());
    }

    #[test]
    fn the_overlay_presents_intents_in_issue_order() {
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Flag, 0);
        q.enqueue(2, msg(1), Intent::AddTag { name: "x".into() }, 0);
        assert_eq!(
            q.overlay().for_message(msg(1)),
            &[Intent::Flag, Intent::AddTag { name: "x".into() }]
        );
    }

    #[test]
    fn a_settled_intent_stops_covering_the_message() {
        let mut q = Queue::new();
        q.enqueue(1, msg(1), Intent::Archive, 0);
        q.set_state(1, State::Settled);
        assert!(!q.overlay().covers(msg(1)));
        q.collect_settled();
        assert!(q.is_empty());
    }
}

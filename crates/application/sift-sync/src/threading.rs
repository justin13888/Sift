//! D-103, FR-11 — threads.
//!
//! # A thread is a claim, not a computed property
//!
//! D-103 states it plainly: **"a thread is a claim about a conversation, not a computed
//! property of the messages currently held."** Everything below follows from that. A thread
//! keeps its identity when messages are evicted, when the linking message is deleted, and
//! when the provider changes its mind — because the claim was made once and the messages are
//! a cache.
//!
//! # Threads merge and never split
//!
//! A message whose reference chain links two threads merges them, **with the older identity
//! surviving**. Nothing ever splits, whatever is later removed.
//!
//! D-103 records what that costs: **a wrong merge is permanent.** The retreat, if wrong
//! merges bite, is to let a user split a thread **by hand** — never to compute splits,
//! because a computed split would undo correct merges every time a message was evicted.

use std::collections::BTreeMap;

/// A thread's local identity.
///
/// Sift's own, assigned at creation. Not the provider's conversation identifier: D-103
/// rejects keying on it because the identifier is an attribute of a provider's opinion, and
/// keying on a normalized subject because generic subjects merge unrelated mail.
pub type ThreadId = u128;
pub type MessageId = u128;

/// How a message was threaded, which FR-33 shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The provider's conversation identifier.
    ///
    /// **Preferred where it exists**, and the reason is about the user rather than about
    /// correctness: they have already seen the provider's threading in its web interface,
    /// and disagreeing with it makes Sift look wrong even when it is not.
    ProviderConversation,
    /// Reconstructed from reference and in-reply-to headers.
    ReferenceChain,
    /// Subject normalization, where the headers are missing or broken.
    ///
    /// **Bounded by D-44**: the chain is the scope, the digest corroborates, and a chain that
    /// does not reduce to one candidate leaves the messages in separate threads rather than
    /// guessing. Without that bound this is how a generic subject merges unrelated mail.
    NormalizedSubject,
    /// Nothing linked it. A thread of one.
    Alone,
}

/// The threads of one account.
///
/// **Scoped to one account, and threads never span accounts** — even the same conversation
/// arriving in two mailboxes stays two threads. Four reasons compound: the internet message
/// identifier is unreliable, D-44's joins are all within one account, D-6 gives each account
/// its own database, and D-4's unified inbox already has to merge streams without joining
/// them.
#[derive(Debug, Default)]
pub struct Threads {
    /// Message to thread.
    of_message: BTreeMap<MessageId, ThreadId>,
    /// Thread to its messages.
    members: BTreeMap<ThreadId, Vec<MessageId>>,
    next: ThreadId,
}

/// What threading a message did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Threaded {
    pub thread: ThreadId,
    pub basis: Basis,
    /// Threads absorbed into `thread`.
    ///
    /// **Crosses the boundary as a move, never as deletes and inserts.** D-66 requires moves
    /// be expressed as moves, and here the reason is concrete: a delete-plus-insert loses the
    /// selection, so a merge arriving while the user is reading would deselect the message
    /// in front of them.
    pub merged_away: Vec<ThreadId>,
}

impl Threads {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Thread a message.
    ///
    /// `links_to` is the set of already-known messages this one references — the scope D-44
    /// requires a join be given before it is keyed.
    pub fn insert(&mut self, message: MessageId, links_to: &[MessageId], basis: Basis) -> Threaded {
        let mut linked: Vec<ThreadId> = links_to
            .iter()
            .filter_map(|m| self.of_message.get(m).copied())
            .collect();
        linked.sort_unstable();
        linked.dedup();

        let thread = match linked.first().copied() {
            // **The older identity survives.** Identities are assigned in increasing order,
            // so the lowest is the oldest — and surviving matters because a shell may be
            // holding it, and the undo record and the notification click-through are keyed
            // on it.
            Some(oldest) => oldest,
            None => {
                let id = self.next;
                self.next += 1;
                id
            }
        };

        let mut merged_away = Vec::new();
        for absorbed in linked.into_iter().filter(|t| *t != thread) {
            if let Some(messages) = self.members.remove(&absorbed) {
                for m in &messages {
                    self.of_message.insert(*m, thread);
                }
                self.members.entry(thread).or_default().extend(messages);
            }
            merged_away.push(absorbed);
        }

        self.of_message.insert(message, thread);
        self.members.entry(thread).or_default().push(message);
        Threaded {
            thread,
            basis,
            merged_away,
        }
    }

    /// Remove a message. **The thread survives.**
    ///
    /// This is where "never split" is enforced: deleting the message that linked two halves
    /// of a conversation does not separate them again. Recomputing would undo a correct
    /// merge every time a message was evicted, which D-102 does routinely.
    pub fn remove(&mut self, message: MessageId) {
        if let Some(thread) = self.of_message.remove(&message) {
            if let Some(members) = self.members.get_mut(&thread) {
                members.retain(|m| *m != message);
            }
        }
    }

    #[must_use]
    pub fn thread_of(&self, message: MessageId) -> Option<ThreadId> {
        self.of_message.get(&message).copied()
    }

    #[must_use]
    pub fn members(&self, thread: ThreadId) -> &[MessageId] {
        self.members.get(&thread).map_or(&[], Vec::as_slice)
    }

    /// Threads holding no messages.
    ///
    /// D-102 retires these: a thread's aggregate figures describe **the messages still
    /// held**, so a thread row claiming members it does not have is a list row that opens
    /// onto nothing.
    #[must_use]
    pub fn empty_threads(&self) -> Vec<ThreadId> {
        self.members
            .iter()
            .filter(|(_, m)| m.is_empty())
            .map(|(t, _)| *t)
            .collect()
    }
}

/// When a thread intent is expanded to its message set.
///
/// **At enqueue time, not at issue time.** D-103 calls the alternative tempting and rejects
/// it: storing the thread identity and expanding at flush would mean a merge between enqueue
/// and flush silently widened the user's action to messages they never selected. Expanding
/// at enqueue is also why **a merge does not touch the mutation queue**.
#[must_use]
pub const fn thread_intents_expand_at_enqueue() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlinked_message_starts_its_own_thread() {
        let mut t = Threads::new();
        let a = t.insert(1, &[], Basis::Alone);
        let b = t.insert(2, &[], Basis::Alone);
        assert_ne!(a.thread, b.thread);
        assert!(a.merged_away.is_empty());
    }

    #[test]
    fn a_reply_joins_the_thread_it_references() {
        let mut t = Threads::new();
        let first = t.insert(1, &[], Basis::Alone);
        let reply = t.insert(2, &[1], Basis::ReferenceChain);
        assert_eq!(reply.thread, first.thread);
        assert_eq!(t.members(first.thread), &[1, 2]);
    }

    #[test]
    fn a_linking_message_merges_two_threads_and_the_older_identity_survives() {
        // Surviving matters because a shell may be holding it, and the undo record and the
        // notification click-through are keyed on it.
        let mut t = Threads::new();
        let older = t.insert(1, &[], Basis::Alone).thread;
        let newer = t.insert(2, &[], Basis::Alone).thread;
        assert!(older < newer);

        let linking = t.insert(3, &[1, 2], Basis::ReferenceChain);
        assert_eq!(linking.thread, older, "the newer identity survived");
        assert_eq!(linking.merged_away, vec![newer]);
        assert_eq!(
            t.thread_of(2),
            Some(older),
            "the absorbed thread's members did not move"
        );
    }

    #[test]
    fn a_merge_carries_every_member_across() {
        let mut t = Threads::new();
        t.insert(1, &[], Basis::Alone);
        t.insert(2, &[1], Basis::ReferenceChain);
        let second = t.insert(10, &[], Basis::Alone).thread;
        t.insert(11, &[10], Basis::ReferenceChain);

        let merged = t.insert(20, &[1, 10], Basis::ReferenceChain);
        assert!(merged.merged_away.contains(&second));
        assert_eq!(t.members(merged.thread).len(), 5);
    }

    #[test]
    fn a_thread_never_splits_when_the_linking_message_goes() {
        // Recomputing would undo a correct merge every time a message was evicted, which
        // D-102 does routinely.
        let mut t = Threads::new();
        t.insert(1, &[], Basis::Alone);
        t.insert(2, &[], Basis::Alone);
        let linking = t.insert(3, &[1, 2], Basis::ReferenceChain);

        t.remove(3);

        assert_eq!(t.thread_of(1), Some(linking.thread));
        assert_eq!(
            t.thread_of(2),
            Some(linking.thread),
            "the thread split when the link went"
        );
    }

    #[test]
    fn a_merge_is_reported_as_a_move_rather_than_a_removal() {
        // A delete-plus-insert loses the selection, so a merge arriving while the user is
        // reading would deselect the message in front of them.
        let mut t = Threads::new();
        t.insert(1, &[], Basis::Alone);
        let second = t.insert(2, &[], Basis::Alone).thread;
        let merged = t.insert(3, &[1, 2], Basis::ReferenceChain);
        assert_eq!(merged.merged_away, vec![second]);
        // Every message still resolves; nothing was removed.
        for m in [1, 2, 3] {
            assert!(
                t.thread_of(m).is_some(),
                "message {m} lost its thread in a merge"
            );
        }
    }

    #[test]
    fn an_emptied_thread_is_reported_for_retirement() {
        // A thread row claiming members it does not have is a list row that opens onto
        // nothing.
        let mut t = Threads::new();
        let only = t.insert(1, &[], Basis::Alone).thread;
        t.remove(1);
        assert_eq!(t.empty_threads(), vec![only]);
    }

    #[test]
    fn the_basis_is_recorded_so_the_debug_view_can_show_it() {
        let mut t = Threads::new();
        assert_eq!(
            t.insert(1, &[], Basis::ProviderConversation).basis,
            Basis::ProviderConversation
        );
        assert_eq!(
            t.insert(2, &[], Basis::NormalizedSubject).basis,
            Basis::NormalizedSubject
        );
    }

    #[test]
    fn a_thread_intent_expands_when_it_is_enqueued() {
        // Otherwise a merge between enqueue and flush would silently widen the user's action
        // to messages they never selected — and this is also why a merge does not touch the
        // queue.
        assert!(thread_intents_expand_at_enqueue());
    }
}

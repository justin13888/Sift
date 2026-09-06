//! D-18's observation registry: what a shell is watching, and the batches it is sent.
//!
//! # Why this holds no callback
//!
//! A registry that owned the shell's callbacks would have to know about threading, about the
//! main-loop hop, and about the C representation — and it would then be testable only through
//! the boundary, which is the one place a test cannot easily reach.
//!
//! So it holds none. A caller registers what it wants watched, mutates the application
//! through this type, and asks [`Session::poll`] what changed. `poll` returns the batches;
//! *delivering* them is the boundary's job, because the boundary is where D-48's main-loop
//! hop and its non-reentrancy rule live. The whole of this crate is therefore ordinary safe
//! Rust that a test can drive directly.
//!
//! # Generations, and what they are not
//!
//! Every registration carries an identity that never repeats, and a generation that advances
//! when it is cancelled. The identity answers *which* observation a delivery is for; the
//! generation answers *whether it is still wanted*. Cancellation advances the generation and
//! drops the registration, so a batch already handed to the boundary — and possibly already
//! posted to the shell's loop — is recognisable as stale on arrival rather than something the
//! cancel had to wait for. Waiting is the deadlock D-48 names: the thread calling cancel is
//! the thread that would have to drain the delivery.

pub mod gesture;

use std::collections::BTreeMap;

use sift_app::App;
use sift_app::rows::MessageRow;
use sift_foundation::identity::AccountId;
use sift_presentation::change::{Batch, diff_by};

/// Which observation a delivery belongs to. Never reused within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObservationId(pub u64);

/// Whether a delivery is still wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);

/// What a registration watches.
///
/// An observation is **anchored on what it is looking at**, not on an integer range: a shell
/// holding a range would have to recompute it on every notification, which is the polling
/// D-18 rejected wearing different clothes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watching {
    /// One account's message list, or every account's merged, windowed to `limit`.
    Messages {
        account: Option<AccountId>,
        limit: u32,
    },
}

/// One batch, for one observation, under the generation it was computed at.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub observation: ObservationId,
    pub generation: Generation,
    pub batch: Batch<MessageRow>,
    /// The window as it now stands — every row, not only the ones entering it.
    ///
    /// **Beside the batch rather than instead of it.** D-18's change vocabulary is what a list
    /// should apply: a move drawn as a move keeps a row's identity, and the indices are in the
    /// post-batch space so a shell never recomputes them. That is what `batch` carries, and
    /// nothing across the ABI carries it yet.
    ///
    /// What a shell had instead was `batch.incoming` and no way to act on the rest, so it
    /// appended — and a sync that removes a message left the row on screen, pointing at a
    /// message the store no longer holds. Selecting it failed. This is what makes the shell's
    /// list *correct* while the vocabulary is still unwired: it costs an animation, and the
    /// alternative cost a lie.
    pub window: Vec<MessageRow>,
}

struct Registration {
    watching: Watching,
    generation: Generation,
    /// The window as the shell last saw it. The diff is against this rather than against a
    /// requery, because a requery would say what the world holds and not what changed.
    window: Vec<MessageRow>,
}

/// The application, plus what is being watched over it.
pub struct Session {
    app: App,
    observations: BTreeMap<u64, Registration>,
    next: u64,
    /// D-86's undo record, which lives here rather than in a shell — a window shell dies with
    /// its window, in a product that runs with no window at all.
    pub(crate) undoable: Option<crate::gesture::Undoable>,
    /// What the last gesture did and **where each message was**, so its compensation can be
    /// built. The origin is captured at the gesture because it stops being knowable after one.
    pub(crate) undone: Vec<(
        sift_foundation::identity::LocalId,
        sift_mutations::intent::Intent,
        Option<i64>,
    )>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("observations", &self.observations.len())
            .finish_non_exhaustive()
    }
}

impl Session {
    #[must_use]
    pub fn new(app: App) -> Self {
        Self {
            app,
            observations: BTreeMap::new(),
            // Deliberately not zero: zero is what an out-parameter holds when a registration
            // failed, and a valid handle must never be mistaken for one.
            next: 1,
            undoable: None,
            undone: Vec::new(),
        }
    }

    #[must_use]
    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// Register an observation. The first [`Self::poll`] after this delivers the whole window
    /// as inserts, which is how a shell fills a list it has never drawn.
    pub fn observe(&mut self, watching: Watching) -> ObservationId {
        let id = self.next;
        self.next += 1;
        self.observations.insert(
            id,
            Registration {
                watching,
                generation: Generation(0),
                window: Vec::new(),
            },
        );
        ObservationId(id)
    }

    /// Cancel one. Returns whether it was live.
    ///
    /// Advancing the generation before dropping the registration is what makes an already
    /// computed batch discardable rather than something to wait for.
    pub fn cancel(&mut self, id: ObservationId) -> bool {
        match self.observations.remove(&id.0) {
            Some(mut r) => {
                r.generation = Generation(r.generation.0 + 1);
                true
            }
            None => false,
        }
    }

    #[must_use]
    pub fn live(&self) -> usize {
        self.observations.len()
    }

    /// Recompute every live observation, and return only those that changed.
    ///
    /// An unchanged observation produces nothing at all. That is the difference between this
    /// and a reload: the common case by far is a signal that touched nothing this window is
    /// looking at, and a registry that answered with the window every time would repaint the
    /// list on every unrelated commit.
    ///
    /// # Errors
    /// The store could not be read. The registration is left holding the window it had, so a
    /// transient failure is a delivery that did not happen rather than a window that was
    /// emptied — a list that blanked itself on a locked database would be the worse answer.
    pub fn poll(&mut self) -> Result<Vec<Delivery>, String> {
        let mut out = Vec::new();
        let ids: Vec<u64> = self.observations.keys().copied().collect();

        for id in ids {
            let Some(registration) = self.observations.get(&id) else {
                continue;
            };
            let fresh = match registration.watching {
                Watching::Messages { account, limit } => self.project_messages(account, limit)?,
            };

            let Some(registration) = self.observations.get_mut(&id) else {
                continue;
            };
            // The extractor form, because `MessageRow` is the application layer's type and
            // `Identified` is this layer's trait — no crate may implement one for the other.
            //
            // Content is the digest combined with the ordering key and the overlay's opinion,
            // so a row whose pending intents changed is an update even where nothing the
            // server holds moved. A shell that was not told would draw a stale badge.
            let batch = diff_by(
                &registration.window,
                &fresh,
                |r| r.id.as_u128(),
                |r| {
                    let mut c = r.digest ^ r.received_millis;
                    c ^= u64::from(r.unread) << 1;
                    c ^= u64::from(r.flagged) << 2;
                    c ^= u64::from(r.has_attachments) << 3;
                    c ^= (r.pending.len() as u64) << 4;
                    c ^= u64::from(r.thread_count) << 8;
                    c
                },
            );
            if batch.is_empty() {
                continue;
            }
            registration.window = fresh;
            out.push(Delivery {
                observation: ObservationId(id),
                generation: registration.generation,
                batch,
                window: registration.window.clone(),
            });
        }
        Ok(out)
    }

    /// The rows one observation is looking at.
    ///
    /// Where no account is named this is the unified inbox — D-4 — and the merge is the
    /// presentation layer's own, over the same comparator the per-account read used, because
    /// "the same comparator" is a requirement rather than an implementation detail.
    fn project_messages(
        &self,
        account: Option<AccountId>,
        limit: u32,
    ) -> Result<Vec<MessageRow>, String> {
        let mut merged: Vec<MessageRow> = Vec::new();
        for (_, open) in self.app.accounts() {
            if account.is_some_and(|wanted| wanted != open.id) {
                continue;
            }
            merged.extend(sift_app::rows::message_rows(open, limit)?);
        }
        // D-55, applied across accounts as well as within one. Identity breaks ties so the
        // order is total — without that, two messages sharing a millisecond would swap
        // places between polls and the diff would report a move nobody caused.
        merged.sort_by(|a, b| {
            b.received_millis
                .cmp(&a.received_millis)
                .then_with(|| b.id.cmp(&a.id))
        });
        merged.truncate(limit as usize);
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(App::new())
    }

    #[test]
    fn a_handle_is_never_zero_and_never_repeats() {
        // Zero is what an out-parameter holds when a registration failed. A handle that
        // could be zero would be indistinguishable from one that was never written.
        let mut s = session();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..100 {
            let id = s.observe(Watching::Messages {
                account: None,
                limit: 50,
            });
            assert_ne!(id.0, 0);
            assert!(seen.insert(id), "a handle repeated");
        }
        assert_eq!(s.live(), 100);
    }

    #[test]
    fn cancelling_removes_exactly_one_observation() {
        // The defect the ABI carried until an observation had an identity: cancelling by
        // generation would have taken every observation or none.
        let mut s = session();
        let a = s.observe(Watching::Messages {
            account: None,
            limit: 50,
        });
        let b = s.observe(Watching::Messages {
            account: None,
            limit: 50,
        });
        assert!(s.cancel(a));
        assert_eq!(s.live(), 1, "cancelling one took the other");
        assert!(!s.cancel(a), "cancelling twice reported a second success");
        assert!(s.cancel(b));
        assert_eq!(s.live(), 0);
    }

    #[test]
    fn a_cancelled_observation_is_never_polled_again() {
        let mut s = session();
        let id = s.observe(Watching::Messages {
            account: None,
            limit: 50,
        });
        s.cancel(id);
        let deliveries = s.poll().expect("poll");
        assert!(
            deliveries.iter().all(|d| d.observation != id),
            "a cancelled observation was still delivered"
        );
    }

    #[test]
    fn an_empty_application_delivers_nothing_rather_than_an_empty_batch() {
        // A delivery is a change. With no accounts there is no change, and a shell that was
        // sent an empty batch on every poll would be doing the work D-18 exists to avoid.
        let mut s = session();
        s.observe(Watching::Messages {
            account: None,
            limit: 50,
        });
        assert!(s.poll().expect("poll").is_empty());
        assert!(s.poll().expect("poll").is_empty());
    }

    #[test]
    fn polling_twice_over_a_quiet_application_changes_nothing_the_second_time() {
        let mut s = session();
        s.app_mut().add_account("a", "rich").expect("account");
        s.observe(Watching::Messages {
            account: None,
            limit: 50,
        });
        let _ = s.poll().expect("poll");
        assert!(
            s.poll().expect("poll").is_empty(),
            "a quiet poll produced a delivery, so the list would repaint on every signal"
        );
    }
}

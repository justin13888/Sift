//! D-98's register, invoked — the one place a gesture becomes an intent.
//!
//! # Why this is not in a shell, and why it is not in the application either
//!
//! It was in a shell. The enqueue, the journal write, the undo group and the capability gate
//! all lived in the harness, which meant the macOS shell had to grow a second copy of them to
//! let a person archive anything. Two implementations of "what a gesture does" is precisely
//! what D-65's harness exists to prevent: its claim is that it drives the application through
//! the entry points a shell uses, and a shell that had to reimplement the gesture would make
//! that claim false about the only part that writes to disk.
//!
//! It is not in `sift-app` either, and the reason is D-59 rather than taste. The action
//! register is presentation-layer, `sift-app` is application-layer, and the edges point one
//! way. So the gesture lives here, beside D-18's registry, where both the register and the
//! application are in scope — which is also where a shell already is.
//!
//! # The order that matters
//!
//! **The journal is written before the store.** D-74 makes the store discardable and the
//! journal not, and the reverse order loses a mutation the user watched succeed. The failure
//! this order *can* leave is an intent enqueued whose optimistic effect was never applied,
//! which is invisible and self-correcting because the overlay is derived from the queue.
//!
//! # The undo group is assigned at the gesture
//!
//! Not per message. That is what makes FR-17's bulk operation one undoable unit rather than a
//! hundred, and it is decided here because here is where the gesture is.

use sift_foundation::identity::LocalId;
use sift_mutations::intent::Intent;
use sift_presentation::action::{self, Action, Context, MutationKind};
use sift_provider::capability::Capabilities;

use crate::Session;

/// D-86's undo record. **In the layer, not the shell.**
///
/// A window shell is destroyed when its window closes, in a product that runs with no window
/// at all — so a user who archives a message and closes the window would have a countdown that
/// dies with the view, which is not a decision anyone made. The layer holds it; a shell renders
/// it, and the always-on surface can present it when there is no window.
///
/// It survives a window closing and **not** a quit: a compensation offered at the next launch
/// would act on a gesture the user has lost the context for.
#[derive(Debug, Clone)]
pub struct Undoable {
    /// D-85's group. One gesture, however many messages — which is what makes FR-17's bulk
    /// operation one undoable unit rather than a hundred.
    pub undo_group: u128,
    pub action: &'static str,
    /// What it did, for the affordance's own words.
    pub intent: &'static str,
    pub messages: usize,
    /// When the gesture happened. The countdown is `L22_UNDO_WINDOW` from here.
    pub at_millis: u64,
    /// Whether FR-15's **timed** window applies, as opposed to ordinary reversibility.
    ///
    /// Only intents that remove the message from the view the user is looking at. Mark-read,
    /// flag and tag leave the message in front of the user, where the affordance that applied
    /// the change is also the one that reverses it — and a countdown on every message the
    /// reader marks read would make the mechanism worthless by making it constant.
    pub timed: bool,
}

impl Undoable {
    /// Whether the countdown is still running.
    #[must_use]
    pub fn within_window(&self, now_millis: u64) -> bool {
        self.timed
            && now_millis.saturating_sub(self.at_millis)
                < sift_foundation::limits::L22_UNDO_WINDOW.as_millis() as u64
    }

    /// Milliseconds left on the countdown, or zero.
    #[must_use]
    pub fn remaining_millis(&self, now_millis: u64) -> u64 {
        let window = sift_foundation::limits::L22_UNDO_WINDOW.as_millis() as u64;
        window.saturating_sub(now_millis.saturating_sub(self.at_millis))
    }
}

/// What one gesture did.
#[derive(Debug, Clone)]
pub struct Gesture {
    pub action: &'static str,
    /// Whether this action mutates at all. A navigation is a legal gesture that enqueues
    /// nothing, and reporting that as "0 enqueued" would read as a failure.
    pub mutates: bool,
    /// The intent's name, where there was one.
    pub intent: Option<&'static str>,
    /// D-85's group. One per gesture, so undo takes back the gesture rather than a message.
    pub undo_group: u128,
    /// What was durably enqueued, in selection order.
    pub enqueued: Vec<LocalId>,
    /// What was not, and why. A message whose account is gone is skipped rather than fatal:
    /// the rest of a bulk gesture is still the user's.
    pub skipped: Vec<(LocalId, String)>,
    /// Whether the overlay hides it before any round trip.
    pub optimistic: bool,
}

/// Why an action is unavailable.
///
/// D-98 makes an unavailable action **absent** rather than disabled, and concedes the cost in
/// its own words: "one keystroke does nothing in one account with no visible reason". A shell
/// shows nothing. This exists so that the harness, the debug view and a diagnosis can say
/// which of the two gates closed, without putting the explanation in front of a user who did
/// not ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// Nothing in the selection belongs to an account, so no capabilities are known.
    NoAccountBehindTheSelection,
    /// The account declares it cannot do this.
    CapabilitiesDoNotPermitIt,
    /// The selection, the open message or the window is missing.
    ScopeNotSatisfied,
}

impl Unavailable {
    #[must_use]
    pub const fn explain(self) -> &'static str {
        match self {
            Self::NoAccountBehindTheSelection => {
                "nothing in the selection belongs to an account, so no capabilities are known"
            }
            Self::CapabilitiesDoNotPermitIt => {
                "the account's declared capabilities do not permit it"
            }
            Self::ScopeNotSatisfied => {
                "its scope is not satisfied — check the selection, the open message, or the window"
            }
        }
    }
}

impl Session {
    /// What is true right now, for deciding enablement.
    #[must_use]
    pub fn action_context<'a>(&self, capabilities: &'a Option<Capabilities>) -> Context<'a> {
        Context {
            selection_len: self.app().selection.len(),
            has_open_message: self.app().open_message.is_some(),
            has_window: self.app().has_window,
            capabilities: capabilities.as_ref(),
        }
    }

    /// Whether an action is available — which is what decides whether a shell draws it.
    #[must_use]
    pub fn action_available(&self, id: &str) -> bool {
        let capabilities = self.app().selection_capabilities();
        action::by_id(id).is_some_and(|a| a.is_available(&self.action_context(&capabilities)))
    }

    /// Why an action is not available, or `None` where it is.
    #[must_use]
    pub fn why_unavailable(&self, id: &str) -> Option<Unavailable> {
        let capabilities = self.app().selection_capabilities();
        let action = action::by_id(id)?;
        if action.is_available(&self.action_context(&capabilities)) {
            return None;
        }
        Some(if action.mutates.is_some() && capabilities.is_none() {
            Unavailable::NoAccountBehindTheSelection
        } else if action.mutates.is_some() {
            Unavailable::CapabilitiesDoNotPermitIt
        } else {
            Unavailable::ScopeNotSatisfied
        })
    }

    /// The palette: a **filtered view of the register**, never a list of its own.
    #[must_use]
    pub fn palette(&self) -> Vec<&'static Action> {
        let capabilities = self.app().selection_capabilities();
        action::palette(&self.action_context(&capabilities))
    }

    /// Invoke an action over the current selection.
    ///
    /// `parameter` carries what the gesture supplies and the register does not — a folder for
    /// a move, a name for a tag. `confirmed` answers FR-14's single exception, and is ignored
    /// by every action that does not require it.
    ///
    /// # Errors
    /// The identifier is not in the register, the action is unavailable, a required parameter
    /// is missing, confirmation is required and absent, or the journal refused an intent —
    /// which is the one failure that must not be swallowed, because it means a gesture the
    /// user watched succeed would not survive a restart.
    pub fn invoke(
        &mut self,
        id: &str,
        parameter: Option<&str>,
        confirmed: bool,
        now: u64,
    ) -> Result<Gesture, String> {
        let action =
            action::by_id(id).ok_or_else(|| format!("no action `{id}` in the register"))?;
        if let Some(why) = self.why_unavailable(id) {
            return Err(format!("`{id}` is not available: {}", why.explain()));
        }

        let Some(kind) = action.mutates else {
            return Ok(Gesture {
                action: action.id,
                mutates: false,
                intent: None,
                undo_group: 0,
                enqueued: Vec::new(),
                skipped: Vec::new(),
                optimistic: false,
            });
        };

        let intent = build(kind, parameter)?;
        if intent.requires_confirmation() && !confirmed {
            // FR-14's single exception. **Confirmed before it is issued** rather than undone
            // after, and never applied optimistically ahead of the server — because there is
            // nothing to undo once it has happened.
            return Err(format!("`{id}` requires confirmation before it is issued"));
        }

        // At the gesture rather than per message: FR-17's bulk operation is one undoable unit.
        let undo_group = self.app_mut().next_intent_id();
        let mut enqueued = Vec::new();
        let mut skipped = Vec::new();
        let mut origins: Vec<(LocalId, Intent, Option<i64>)> = Vec::new();

        for message in self.app().selection.clone() {
            let Some(owner) = self
                .app()
                .owner_of(message)
                .cloned()
                .or_else(|| self.app().owner_of_stored(message))
            else {
                skipped.push((message, "no account holds this message".to_owned()));
                continue;
            };
            let intent_id = self.app_mut().next_intent_id();
            let account = self.app_mut().account(&owner)?;
            // **Where it was, captured now.** FR-15 reverses an archive by moving the message
            // back, and "back" is a fact that only exists before the gesture: once the overlay
            // hides the row and the provider applies the change, nothing says where it came
            // from. Read here, or the most common destructive action in the product has no
            // undo at all — which is exactly what the first run of this showed.
            let restore_to: Option<i64> = account
                .store
                .store
                .query_row(
                    "SELECT folder_id FROM message_location WHERE message_id = ?1
                     ORDER BY folder_id LIMIT 1",
                    rusqlite::params![message.to_bytes().to_vec()],
                    |r| r.get(0),
                )
                .ok();
            let sequence = account
                .queue
                .enqueue(intent_id, message, intent.clone(), now);

            // Journal first, store second — D-74's ordering. The reverse loses a mutation the
            // user watched succeed.
            account
                .store
                .journal
                .execute(
                    "INSERT INTO intent (id, undo_group, message_id, operation, intent_version,
                                         state, created_millis, per_message_seq, expires_millis)
                     VALUES (?1, ?2, ?3, ?4, 1, 'Pending', ?5, ?6, ?7)",
                    rusqlite::params![
                        intent_id.to_be_bytes().to_vec(),
                        undo_group.to_be_bytes().to_vec(),
                        message.to_bytes().to_vec(),
                        intent.name(),
                        i64::try_from(now).unwrap_or(i64::MAX),
                        i64::try_from(sequence).unwrap_or(i64::MAX),
                        i64::try_from(now.saturating_add(
                            sift_foundation::limits::L17_INTENT_EXPIRY.as_millis() as u64
                        ))
                        .unwrap_or(i64::MAX),
                    ],
                )
                .map_err(|e| format!("the journal refused the intent: {e}"))?;
            origins.push((message, intent.clone(), restore_to));
            enqueued.push(message);
        }

        // The record is kept only where there is something to take back. A gesture that
        // enqueued nothing has nothing to compensate, and offering undo for it would be a
        // control that does nothing.
        if !enqueued.is_empty() {
            self.undoable = Some(Undoable {
                undo_group,
                action: action.id,
                intent: intent.name(),
                messages: enqueued.len(),
                at_millis: now,
                timed: intent.removes_from_view(),
            });
            self.undone.clear();
            self.undone = origins;
        }

        Ok(Gesture {
            action: action.id,
            mutates: true,
            intent: Some(intent.name()),
            undo_group,
            enqueued,
            skipped,
            optimistic: intent.applies_optimistically(),
        })
    }

    /// What could be undone right now, if anything.
    #[must_use]
    pub fn undoable(&self) -> Option<&Undoable> {
        self.undoable.as_ref()
    }

    /// FR-15 — reverse the last gesture.
    ///
    /// **Executed as the compensation, never as a queue retraction.** The original may already
    /// have reached the server; a design that tries to cancel in flight has two outcomes to
    /// reason about, and one that always compensates has one.
    ///
    /// Nothing is withheld to make this cheap. D-86 rejected holding the intent back for the
    /// duration of the window, because that reintroduces a race at the end of every window
    /// against a flush that may already have started — on the most frequent destructive action
    /// in the product. The saving arrives anyway: intents flush on the scheduler's tick, and an
    /// archive and its compensation inside one interval collapse under the coalescing rule that
    /// already exists.
    ///
    /// **What a compensation restores is local state, never a remote side effect.** Reporting
    /// not-junk returns the message and tells the provider it was wrong; it cannot un-train the
    /// classifier, and this must not be described as promising that.
    ///
    /// # Errors
    /// There is nothing to undo, the intent has no compensation, or the journal refused one.
    pub fn undo_last(&mut self, now: u64) -> Result<Gesture, String> {
        let record = self.undoable.clone().ok_or("there is nothing to undo")?;
        if self.undone.is_empty() {
            return Err("there is nothing to undo".to_owned());
        }
        // Cloned rather than taken: a failed undo must leave the record standing. Consuming it
        // on the way *in* means a compensation that could not be built also destroys the only
        // affordance for building it, and the user is told there is nothing to undo about a
        // gesture they just watched happen.
        let undone = self.undone.clone();

        // The undo is its own gesture with its own group, because it is its own thing that
        // happened — and because undoing an undo is a redo, which falls out for free.
        let undo_group = self.app_mut().next_intent_id();
        let mut enqueued = Vec::new();
        let mut skipped = Vec::new();
        let mut name = "";

        for (message, original, restore_to) in undone {
            // `restore_to` was read at the gesture, before the overlay hid the row. A
            // compensation that moved mail to a folder nobody chose would be worse than one
            // that did not run, so an intent with no recorded origin is reported rather than
            // guessed at.
            let Some(compensation) = original.compensation(restore_to) else {
                skipped.push((
                    message,
                    format!("`{}` has no compensation", original.name()),
                ));
                continue;
            };
            name = compensation.name();
            let Some(owner) = self
                .app()
                .owner_of(message)
                .cloned()
                .or_else(|| self.app().owner_of_stored(message))
            else {
                skipped.push((message, "no account holds this message".to_owned()));
                continue;
            };
            let intent_id = self.app_mut().next_intent_id();
            let account = self.app_mut().account(&owner)?;
            let sequence = account
                .queue
                .enqueue(intent_id, message, compensation.clone(), now);
            account
                .store
                .journal
                .execute(
                    "INSERT INTO intent (id, undo_group, message_id, operation, intent_version,
                                         state, created_millis, per_message_seq, expires_millis)
                     VALUES (?1, ?2, ?3, ?4, 1, 'Pending', ?5, ?6, ?7)",
                    rusqlite::params![
                        intent_id.to_be_bytes().to_vec(),
                        undo_group.to_be_bytes().to_vec(),
                        message.to_bytes().to_vec(),
                        compensation.name(),
                        i64::try_from(now).unwrap_or(i64::MAX),
                        i64::try_from(sequence).unwrap_or(i64::MAX),
                        i64::try_from(now.saturating_add(
                            sift_foundation::limits::L17_INTENT_EXPIRY.as_millis() as u64
                        ))
                        .unwrap_or(i64::MAX),
                    ],
                )
                .map_err(|e| format!("the journal refused the compensation: {e}"))?;
            enqueued.push(message);
        }

        if enqueued.is_empty() {
            // The record stands, so the user can be told why rather than told nothing.
            return Err(format!(
                "`{}` could not be undone: {}",
                record.intent,
                skipped
                    .first()
                    .map_or("nothing was reversible", |(_, why)| why.as_str())
            ));
        }
        // Only now: the gesture has been reversed, so there is nothing left to reverse.
        self.undoable = None;
        self.undone.clear();
        Ok(Gesture {
            action: "undo.last-gesture",
            mutates: true,
            intent: Some(name),
            undo_group,
            enqueued,
            skipped,
            optimistic: true,
        })
    }
}

/// The intent a gesture builds. Parameters come from the gesture, never from the register.
fn build(kind: MutationKind, parameter: Option<&str>) -> Result<Intent, String> {
    Ok(match kind {
        MutationKind::Archive => Intent::Archive,
        MutationKind::DeleteToTrash => Intent::DeleteToTrash,
        MutationKind::PermanentlyDelete => Intent::PermanentlyDelete,
        MutationKind::MoveTo => Intent::MoveTo {
            folder: parameter
                .ok_or("move-to needs a folder")?
                .parse()
                .map_err(|_| "folder must be a number")?,
        },
        MutationKind::Flag => Intent::Flag,
        MutationKind::MarkRead => Intent::MarkRead,
        MutationKind::MarkUnread => Intent::MarkUnread,
        MutationKind::AddTag => Intent::AddTag {
            name: parameter.ok_or("add-tag needs a name")?.to_owned(),
        },
        MutationKind::RemoveTag => Intent::RemoveTag {
            name: parameter.ok_or("remove-tag needs a name")?.to_owned(),
        },
        MutationKind::ReportJunk => Intent::ReportJunk,
        MutationKind::ReportNotJunk => Intent::ReportNotJunk,
    })
}

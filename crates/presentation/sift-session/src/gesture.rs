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
            enqueued.push(message);
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

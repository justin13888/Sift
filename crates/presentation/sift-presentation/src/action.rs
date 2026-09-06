//! D-98 — every operation is a named action, and the register is an ABI surface.
//!
//! # Three consumers, one list
//!
//! The register exists because three things need the same list and would otherwise each
//! have their own: the keyboard model (FR-24 requires **every** action be reachable without
//! a pointer), the command palette — which is "a filtered view of the register" rather than
//! a list of its own — and **the shell test harness, which invokes actions by identifier**.
//!
//! That third one is what makes FR-24's testability claim real rather than aspirational.
//! `docs/build/verification.md` puts it plainly: the harness and the action registry are
//! one thing.
//!
//! # An unavailable action is absent, not disabled
//!
//! D-98 records this as its own weakest point, and it is worth stating rather than
//! discovering: hiding unavailable actions **makes the interface change shape between
//! accounts**, so one keystroke does nothing in one account with no visible reason. The
//! alternative — showing it greyed — was rejected because a palette full of things that
//! cannot be done is a worse palette.
//!
//! # Bindings are not rebindable yet, and this is the seam
//!
//! Default bindings are per platform; the **identifier is shared and the key is not**.
//! Rebinding is deferred rather than refused, and the stable identifiers here are what it
//! would enter through.

use sift_mutations::intent::Intent;
use sift_provider::capability::Capabilities;

/// What an action operates on. Its scope decides when it can be enabled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Application,
    Window,
    /// Requires a non-empty list selection.
    Selection,
    /// Requires an open message.
    OpenMessage,
}

/// Whether anything behind this boundary can actually carry an action out.
///
/// **The third reason an action can be absent, and the one that was missing.** D-98 makes an
/// unavailable action absent rather than greyed, and the mechanism for deciding that expressed
/// only two things: whether the scope is satisfied and whether the account's capabilities
/// permit the mutation. An action that no shell could perform at all was therefore offered in
/// every menu and every palette, and pressing it did nothing or beeped — which is precisely
/// what D-98's rule exists to prevent, arriving by a door the rule did not cover.
///
/// The reason travels with the identifier rather than living in a commit message, so a person
/// adding the missing piece finds the sentence that says what is missing next to the thing
/// that is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A shell can carry it out.
    Now,
    /// Nothing can, yet. **Not a preference and not a capability** — those are the other two
    /// reasons, and conflating them would tell a user their provider cannot do something that
    /// no provider can do here.
    NotYet(&'static str),
}

/// One action.
#[derive(Debug, Clone, Copy)]
pub struct Action {
    /// **Stable, and never reused.** The harness invokes by this, the palette filters on
    /// it, and both shells bind to it.
    pub id: &'static str,
    pub scope: Scope,
    /// The mutation this action issues, if it is one.
    ///
    /// D-98: "these are exactly FR-13's intent set; **the action set MUST NOT contain a
    /// mutation that is not an intent**." A mutation that is not an intent would be a write
    /// path outside the queue, which is a write with no compensation, no undo, and no
    /// crash-consistency.
    pub mutates: Option<MutationKind>,
    /// Whether anything can carry it out yet, and why not where it cannot.
    pub reach: Reach,
}

/// A mutating action's intent, named without its parameters — the parameters come from the
/// gesture, not the register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    Archive,
    DeleteToTrash,
    PermanentlyDelete,
    MoveTo,
    Flag,
    MarkRead,
    MarkUnread,
    AddTag,
    RemoveTag,
    ReportJunk,
    ReportNotJunk,
}

impl MutationKind {
    /// A representative intent, for capability gating. Parameters are placeholders because
    /// enablement does not depend on them.
    #[must_use]
    pub fn representative(self) -> Intent {
        match self {
            Self::Archive => Intent::Archive,
            Self::DeleteToTrash => Intent::DeleteToTrash,
            Self::PermanentlyDelete => Intent::PermanentlyDelete,
            Self::MoveTo => Intent::MoveTo { folder: 0 },
            Self::Flag => Intent::Flag,
            Self::MarkRead => Intent::MarkRead,
            Self::MarkUnread => Intent::MarkUnread,
            Self::AddTag => Intent::AddTag {
                name: String::new(),
            },
            Self::RemoveTag => Intent::RemoveTag {
                name: String::new(),
            },
            Self::ReportJunk => Intent::ReportJunk,
            Self::ReportNotJunk => Intent::ReportNotJunk,
        }
    }
}

const fn act(id: &'static str, scope: Scope) -> Action {
    Action {
        id,
        scope,
        mutates: None,
        reach: Reach::Now,
    }
}
const fn mutate(id: &'static str, kind: MutationKind) -> Action {
    Action {
        id,
        scope: Scope::Selection,
        mutates: Some(kind),
        reach: Reach::Now,
    }
}

/// An action the register declares and nothing can perform yet, with the reason.
const fn not_yet(id: &'static str, scope: Scope, why: &'static str) -> Action {
    Action {
        id,
        scope,
        mutates: None,
        reach: Reach::NotYet(why),
    }
}

/// The same, for one of FR-13's intents whose gesture has no surface to supply its parameter.
const fn mutate_not_yet(id: &'static str, kind: MutationKind, why: &'static str) -> Action {
    Action {
        id,
        scope: Scope::Selection,
        mutates: Some(kind),
        reach: Reach::NotYet(why),
    }
}

/// The action set, as D-98 enumerates it.
pub const ACTIONS: &[Action] = &[
    // Message and selection — exactly FR-13's intent set.
    mutate("message.archive", MutationKind::Archive),
    mutate("message.delete-to-trash", MutationKind::DeleteToTrash),
    mutate(
        "message.permanently-delete",
        MutationKind::PermanentlyDelete,
    ),
    // **The parameter has nowhere to come from, for either shell.** A tag is a string a
    // person types; a folder is one of a set, and no folder crosses this boundary — sync
    // discovers them into the store and nothing lists them back. So this is the layer's gap
    // rather than a missing picker, and it is absent until the folder list exists.
    mutate_not_yet(
        "message.move-to-folder",
        MutationKind::MoveTo,
        "no folder crosses this boundary yet, so there is no destination to move to",
    ),
    mutate("message.flag", MutationKind::Flag),
    mutate("message.mark-read", MutationKind::MarkRead),
    mutate("message.mark-unread", MutationKind::MarkUnread),
    mutate("message.add-tag", MutationKind::AddTag),
    mutate("message.remove-tag", MutationKind::RemoveTag),
    mutate("message.report-junk", MutationKind::ReportJunk),
    mutate("message.report-not-junk", MutationKind::ReportNotJunk),
    // Reading.
    act("read.open-message", Scope::Selection),
    act("read.open-in-standalone-reader", Scope::Selection),
    act("read.next-message", Scope::Window),
    act("read.previous-message", Scope::Window),
    act("read.next-unread", Scope::Window),
    act("read.previous-unread", Scope::Window),
    not_yet(
        "read.expand-thread",
        Scope::Selection,
        "the list presents no threads to expand — D-54's native rows are not built yet",
    ),
    not_yet(
        "read.collapse-thread",
        Scope::Selection,
        "the list presents no threads to collapse — D-54's native rows are not built yet",
    ),
    not_yet(
        "read.show-plain-text",
        Scope::OpenMessage,
        "the pipeline offers one rendering across this boundary, and it is the sanitized one",
    ),
    not_yet(
        "read.show-raw-source",
        Scope::OpenMessage,
        "a raw payload never crosses this boundary; FR-33's debug view is what shows the stages",
    ),
    act("read.toggle-dark-transform", Scope::OpenMessage),
    not_yet(
        "read.allow-remote-content-for-sender",
        Scope::OpenMessage,
        "the sender's origin is not attested, so there is no sender to record the choice against",
    ),
    not_yet(
        "read.find-in-message",
        Scope::OpenMessage,
        "finding inside a body needs a search surface the reader does not have, and the body \
         view runs no script that could provide one",
    ),
    // FR-41. These hand off to the platform's mail handler and construct nothing.
    act("read.reply", Scope::OpenMessage),
    act("read.reply-all", Scope::OpenMessage),
    act("read.forward", Scope::OpenMessage),
    // Navigation.
    not_yet(
        "navigate.next-folder",
        Scope::Window,
        "no folder crosses this boundary yet — sync discovers them and nothing lists them",
    ),
    not_yet(
        "navigate.previous-folder",
        Scope::Window,
        "no folder crosses this boundary yet — sync discovers them and nothing lists them",
    ),
    act("navigate.next-account", Scope::Window),
    act("navigate.previous-account", Scope::Window),
    act("navigate.unified-inbox", Scope::Window),
    act("navigate.focus-list", Scope::Window),
    act("navigate.focus-reader", Scope::Window),
    act("navigate.focus-sidebar", Scope::Window),
    act("navigate.focus-search", Scope::Window),
    // Search.
    act("search.begin", Scope::Window),
    act("search.clear", Scope::Window),
    act("search.narrow-to-account", Scope::Window),
    not_yet(
        "search.narrow-to-folder",
        Scope::Window,
        "no folder crosses this boundary yet, so there is nothing to narrow to",
    ),
    // Application.
    act("app.new-window", Scope::Application),
    act("app.close-window", Scope::Window),
    act("app.quit", Scope::Application),
    act("app.pause-sync", Scope::Application),
    act("app.resume-sync", Scope::Application),
    act("app.add-account", Scope::Application),
    act("app.open-settings", Scope::Application),
    act("app.open-message-debug-view", Scope::OpenMessage),
    act("app.open-runtime-panel", Scope::Application),
    act("app.command-palette", Scope::Application),
    // Undo acts over D-85's undo group rather than over a message.
    act("undo.last-gesture", Scope::Application),
];

/// What is true right now, for deciding enablement.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub selection_len: usize,
    pub has_open_message: bool,
    pub has_window: bool,
    /// The **intersection** of the capabilities of every account in the selection.
    ///
    /// D-99: a selection spanning accounts resolves affordances to the intersection rather
    /// than the union. The recorded cost is that the intersection silently shrinks available
    /// actions as a selection widens, with nothing saying why.
    pub capabilities: Option<&'a Capabilities>,
}

impl Action {
    /// Whether this action is available.
    ///
    /// **Not enabled means absent from the palette**, not shown disabled.
    #[must_use]
    pub fn is_available(&self, ctx: &Context<'_>) -> bool {
        // **First, because it is the reason that does not depend on state.** An action nothing
        // can perform is absent in every account and every selection, and reporting it as a
        // scope or a capability problem would tell a user their provider cannot do something
        // that no provider can do here.
        if matches!(self.reach, Reach::NotYet(_)) {
            return false;
        }
        let scope_ok = match self.scope {
            Scope::Application => true,
            Scope::Window => ctx.has_window,
            Scope::Selection => ctx.selection_len > 0,
            Scope::OpenMessage => ctx.has_open_message,
        };
        if !scope_ok {
            return false;
        }
        match (self.mutates, ctx.capabilities) {
            (Some(kind), Some(caps)) => kind.representative().permitted_by(caps),
            (Some(_), None) => false,
            (None, _) => true,
        }
    }
}

/// Look an action up by identifier — what the harness does.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// The palette's contents: a filtered view of the register, never a list of its own.
#[must_use]
pub fn palette(ctx: &Context<'_>) -> Vec<&'static Action> {
    ACTIONS.iter().filter(|a| a.is_available(ctx)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_provider::capability::{
        ArchiveSemantics, DeltaMechanism, IdStability, JunkReporting, LocationCardinality,
        Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations, TrashSemantics,
    };
    use std::collections::BTreeSet;

    fn caps(tags: TagSupport, junk: JunkReporting) -> Capabilities {
        Capabilities {
            location_cardinality: LocationCardinality::ExactlyOne,
            tag_support: tags,
            archive: ArchiveSemantics::MoveToSpecialUse,
            trash: TrashSemantics::MoveToTrash,
            permanent_delete: true,
            thread_operations: ThreadOperations::ClientFanOut,
            junk_reporting: junk,
            delta: DeltaMechanism::FullScan,
            push: PushMechanism::PollOnly,
            id_stability: IdStability::StablePerFolder,
            server_search: true,
            max_batch_size: Magnitude::Unknown,
            request_budget: Magnitude::Unknown,
            snippet_source: SnippetSource::ClientDerived,
            unrecognised: Vec::new(),
        }
    }

    fn full<'a>(c: &'a Capabilities) -> Context<'a> {
        Context {
            selection_len: 1,
            has_open_message: true,
            has_window: true,
            capabilities: Some(c),
        }
    }

    #[test]
    fn identifiers_are_unique() {
        // They are an ABI surface: the harness invokes by them, the palette filters on
        // them, and both shells bind to them. Two actions sharing one would make a keystroke
        // ambiguous and a test unrepeatable.
        let ids: BTreeSet<&str> = ACTIONS.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), ACTIONS.len(), "two actions share an identifier");
    }

    #[test]
    fn every_mutating_action_is_an_intent() {
        // D-98: "the action set MUST NOT contain a mutation that is not an intent." One that
        // was would be a write path outside the queue — no compensation, no undo, no
        // crash consistency.
        let c = caps(TagSupport::ReadWrite, JunkReporting::NativeReport);
        for a in ACTIONS.iter().filter(|a| a.mutates.is_some()) {
            let kind = a.mutates.expect("filtered");
            assert!(
                kind.representative().permitted_by(&c),
                "{} is not an intent a capable account permits",
                a.id
            );
        }
    }

    #[test]
    fn every_intent_has_an_action() {
        // The other direction: an intent with no action is a mutation the user cannot reach
        // from the keyboard, which FR-24 forbids.
        let mutating: BTreeSet<&str> = ACTIONS
            .iter()
            .filter_map(|a| a.mutates)
            .map(|k| k.representative().name())
            .collect();
        for expected in [
            "archive",
            "delete-to-trash",
            "permanently-delete",
            "move-to",
            "mark-read",
            "mark-unread",
            "flag",
            "add-tag",
            "remove-tag",
            "report-junk",
            "report-not-junk",
        ] {
            assert!(mutating.contains(expected), "no action issues `{expected}`");
        }
    }

    #[test]
    fn an_unavailable_action_is_absent_from_the_palette_rather_than_disabled() {
        // D-98's recorded weakness, made observable: the palette changes shape between
        // accounts, and this is where that happens.
        let no_tags = caps(TagSupport::None, JunkReporting::NativeReport);
        let visible: BTreeSet<&str> = palette(&full(&no_tags)).iter().map(|a| a.id).collect();
        assert!(
            !visible.contains("message.add-tag"),
            "a tag action survived on an account without tags"
        );
        assert!(visible.contains("message.archive"));
    }

    #[test]
    fn junk_actions_vanish_where_the_account_cannot_report() {
        let none = caps(TagSupport::ReadWrite, JunkReporting::None);
        let visible: BTreeSet<&str> = palette(&full(&none)).iter().map(|a| a.id).collect();
        assert!(!visible.contains("message.report-junk"));
        assert!(!visible.contains("message.report-not-junk"));
    }

    #[test]
    fn scope_gates_before_capability_does() {
        // An empty selection makes every selection-scoped action unavailable regardless of
        // what the account can do.
        let c = caps(TagSupport::ReadWrite, JunkReporting::NativeReport);
        let empty = Context {
            selection_len: 0,
            has_open_message: false,
            has_window: true,
            capabilities: Some(&c),
        };
        assert!(!by_id("message.archive").unwrap().is_available(&empty));
        assert!(!by_id("read.show-raw-source").unwrap().is_available(&empty));
        // Application scope survives, because quitting does not need a selection.
        assert!(by_id("app.quit").unwrap().is_available(&empty));
    }

    #[test]
    fn application_actions_work_with_no_window_open() {
        // Sift is resident with nothing on screen. Quitting, pausing and adding an account
        // all have to work in that state — FR-25's whole distinction depends on it.
        let nothing = Context {
            selection_len: 0,
            has_open_message: false,
            has_window: false,
            capabilities: None,
        };
        for id in [
            "app.quit",
            "app.pause-sync",
            "app.add-account",
            "app.command-palette",
        ] {
            assert!(
                by_id(id).unwrap().is_available(&nothing),
                "{id} needs a window"
            );
        }
        assert!(!by_id("app.close-window").unwrap().is_available(&nothing));
    }

    #[test]
    fn a_mutating_action_with_no_known_capabilities_is_unavailable() {
        // The failure direction that matters: without knowing what the account can do, an
        // action that writes must not be offered.
        let unknown = Context {
            selection_len: 1,
            has_open_message: true,
            has_window: true,
            capabilities: None,
        };
        assert!(!by_id("message.archive").unwrap().is_available(&unknown));
        assert!(
            by_id("read.toggle-dark-transform")
                .unwrap()
                .is_available(&unknown),
            "reading is not gated"
        );
    }

    #[test]
    fn an_action_nothing_can_perform_is_absent_whatever_the_account_declares() {
        // D-98's rule is that an unavailable action is absent rather than greyed, and until
        // `Reach` existed the mechanism could only express scope and capability — so an action
        // no shell could carry out was offered everywhere and did nothing when pressed.
        let everything = Context {
            selection_len: 1,
            has_open_message: true,
            has_window: true,
            capabilities: None,
        };
        for id in [
            "read.show-raw-source",
            "read.find-in-message",
            "navigate.next-folder",
            "search.narrow-to-folder",
        ] {
            let action = by_id(id).expect("in the register");
            assert!(
                matches!(action.reach, Reach::NotYet(_)),
                "`{id}` should carry the reason it cannot be performed"
            );
            assert!(
                !action.is_available(&everything),
                "`{id}` is offered in a context that satisfies its scope"
            );
        }
        // And the reason is a sentence rather than an empty marker: it is what a person
        // adding the missing piece reads first.
        for action in ACTIONS {
            if let Reach::NotYet(why) = action.reach {
                assert!(
                    why.len() > 20,
                    "`{}` says nothing about why it cannot be performed",
                    action.id
                );
            }
        }
    }

    #[test]
    fn the_harness_can_reach_every_action_by_identifier() {
        // The property FR-24's testability claim rests on, and what makes the command-driven
        // shell harness possible without UI automation.
        for a in ACTIONS {
            assert_eq!(by_id(a.id).map(|f| f.id), Some(a.id));
        }
        assert!(
            by_id("message.compose").is_none(),
            "there is no compose action, and never will be"
        );
    }

    #[test]
    fn nothing_in_the_register_composes_or_sends_a_message() {
        // The no-send constraint, checked where a reviewer would look for it. FR-41's reply,
        // reply-all and forward are present and hand off; nothing constructs a message.
        // Matched on whole segments rather than substrings: "sender" legitimately contains
        // "send", and a check that cannot tell those apart is one somebody deletes the first
        // time it is wrong.
        for a in ACTIONS {
            for segment in a.id.split(['.', '-']) {
                assert!(
                    !matches!(
                        segment,
                        "compose" | "send" | "sends" | "draft" | "drafts" | "outbox"
                    ),
                    "{} looks like a send path",
                    a.id
                );
            }
        }
        for handoff in ["read.reply", "read.reply-all", "read.forward"] {
            assert!(
                by_id(handoff).is_some(),
                "{handoff} is FR-41's answer and is missing"
            );
        }
    }

    #[test]
    fn undo_is_application_scoped_because_it_acts_over_a_group() {
        // D-85's undo group rather than a message: a bulk operation is one undoable unit, and
        // the record lives in this layer because a window shell dies with its window.
        assert_eq!(
            by_id("undo.last-gesture").unwrap().scope,
            Scope::Application
        );
    }

    #[test]
    fn the_debug_surfaces_are_actions_rather_than_hidden_builds() {
        // FR-33 and FR-34 ship in release behind a preference. They are reachable actions,
        // because a surface that only exists in a debug build is the untested one.
        assert!(by_id("app.open-message-debug-view").is_some());
        assert!(by_id("app.open-runtime-panel").is_some());
    }
}

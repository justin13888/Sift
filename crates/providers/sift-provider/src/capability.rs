//! D-12 — the declared capability set.
//!
//! # The binding rule
//!
//! Sift MUST NOT build "IMAP-with-special-cases". There is one provider abstraction with a
//! declared capability set, and the sync engine, the mutation queue and the UI plan
//! against **capabilities, never provider names**.
//!
//! > **A `match` on provider identity anywhere above the adapter layer is a defect.**
//!
//! That is enforced mechanically rather than by review: `cargo xtask invariants` fails the
//! build on a provider name appearing in the application, presentation or ABI layers, and
//! D-59 puts each adapter in its own crate that cannot see the others — so an adapter's
//! fitness is tested by whether it compiles against *this crate alone*.
//!
//! What that buys is stated as a UI rule: no tag support means no tag affordance; location
//! cardinality of exactly one means "add tag" renders as "move to folder". The interface
//! changes shape because the account's capabilities differ, not because somebody wrote an
//! `if`.
//!
//! # Five rules for growing the set
//!
//! These are what make a fifth provider an append rather than a redesign, so each is
//! encoded in a type rather than left as guidance.
//!
//! 1. **An absent capability means unsupported.** Never "assume yes", never "probe and
//!    hope". [`Capabilities`] has no `Option` that means "don't know" — see rule 5 for the
//!    one case that genuinely does not know.
//! 2. **An unrecognised capability is ignored, not fatal.** The planner MUST ignore and
//!    continue, and MUST NOT refuse the account. See [`Capabilities::unrecognised`].
//!    Deliberately the opposite of D-66's rule at the shell boundary, where an unknown
//!    discriminant is a **build** failure — and the difference is justified: both sides of
//!    that boundary ship in one binary, and a stored capability set does not.
//! 3. **A new capability value is additive within its own row.** Existing values keep their
//!    exact meaning. A value whose meaning shifts is a renumbering, not an addition.
//! 4. **A magnitude-valued capability declares a conservative default, never
//!    "unsupported".** See [`Magnitude`]. This is what makes Q-9 legal to leave open in all
//!    four adapter tables without blocking a single adapter from being written.
//! 5. **A probed capability is a cached observation, and a failed probe is not an
//!    observation.** See [`Probed`].

use sift_foundation::limits::L15_TAG_NAME_CHARS;

/// How many locations a message can be in at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationCardinality {
    /// Folders. A message is in exactly one.
    ExactlyOne,
    /// Mailbox or label membership. A message can be in several.
    OneOrMore,
}

/// D-12's second axis, kept separate from Location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagSupport {
    None,
    ReadOnly,
    /// Arbitrary user tags, bounded by [`L15_TAG_NAME_CHARS`].
    ReadWrite,
}

/// What archiving means on this account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSemantics {
    /// Remove the inbox from the message's locations. Nothing moves.
    RemoveFromInbox,
    /// Move it to the archive special-use folder.
    MoveToSpecialUse,
}

/// What deleting to trash means on this account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashSemantics {
    MoveToTrash,
    /// Set the deleted flag and expunge — the older protocol's model.
    FlagAndExpunge,
}

/// Whether a thread operation is one call or N.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadOperations {
    /// The provider applies it to the conversation.
    Native,
    /// Sift expands the intent to its message set. FR-38 governs partial failure.
    ClientFanOut,
}

/// D-40's three-valued row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunkReporting {
    /// A report call or keyword distinct from moving to the junk folder.
    NativeReport,
    /// Only a move. Sift offers a move **labelled as what it is**, and MUST NOT present a
    /// report affordance that silently does something else.
    FolderMoveOnly,
    /// Neither is offered. FR-39: absent, not approximated.
    None,
}

/// How change is enumerated. Named by mechanism rather than by provider, which is the
/// point: two providers sharing a mechanism share the code path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaMechanism {
    /// A monotonic history identifier. Falling outside its retained window is a cursor
    /// invalidation.
    HistoryCursor,
    /// An opaque continuation token, per folder, that expires.
    DeltaLink,
    /// A changes query against a state string.
    ChangesQuery,
    /// Modification-sequence deltas with quick resynchronization.
    ModSeqResync,
    /// No efficient delta. Resync cost is proportional to **mailbox size** rather than
    /// change volume, and NFR-29 requires that be surfaced rather than hidden.
    FullScan,
}

/// How the provider says something changed. A doorbell, never the change itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushMechanism {
    /// A long-polled stream needing no publicly reachable endpoint.
    EventStream,
    /// Watches many mailboxes over one connection. Preferred where available, because the
    /// connection budget is L-23 and the watched-folder count multiplies NFR-11.
    NotifyManyOverOne,
    /// Watches one mailbox per connection, and must be re-issued before L-27.
    IdleOnePerConnection,
    PollOnly,
}

/// Whether a remote identifier can be used as a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdStability {
    StableGlobally,
    /// Stable within a folder, keyed on the folder's validity identifier.
    StablePerFolder,
    /// **Changes when the message moves.** The single most consequential value in this
    /// enum: it forces D-44's corroborated join on every move, and a move that does not
    /// resolve to exactly one candidate presents as a delete plus an arrival.
    UnstableOnMove,
}

/// Where a list row's snippet comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetSource {
    ProviderSupplied,
    /// Derived by Sift — **only from a body it already fetched for its own reasons, and
    /// never by causing a fetch.** On some protocols fetching body text without the peek
    /// form sets the seen flag, so a backfill-derived snippet would mark an entire mailbox
    /// read. A list row with no snippet is honest.
    ClientDerived,
    None,
}

/// Rule 4 — a magnitude-valued capability.
///
/// The alternative that was rejected is declaring it *unsupported*, which would make the
/// planner refuse to batch at all rather than batch conservatively. So the value is either
/// known, with its source, or explicitly [`Unknown`](Magnitude::Unknown) — and unknown
/// plans against the most conservative operable value rather than blocking.
///
/// This is what makes Q-9 legal to leave open in all four adapter tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Magnitude {
    /// A value, and where it came from. The source is part of the declaration because
    /// "published limit" and "somebody guessed" are not the same claim.
    Known {
        value: u32,
        source: MagnitudeSource,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagnitudeSource {
    /// The provider documents it.
    Published,
    /// Sift measured it.
    Measured,
    /// Nobody knows; this is a deliberately small number.
    ConservativeDefault,
}

impl Magnitude {
    /// The value to plan against.
    ///
    /// `Unknown` resolves to `conservative` rather than to zero or to "unlimited" — the
    /// first would refuse all work and the second would invite a throttle.
    #[must_use]
    pub const fn plan_against(self, conservative: u32) -> u32 {
        match self {
            Self::Known { value, .. } => value,
            Self::Unknown => conservative,
        }
    }
}

/// Rule 5 — a probed capability, with the fact that it was probed.
///
/// Two absences that must not be stored as the same thing:
///
/// - **Absent at first contact** means unsupported.
/// - **Absent after a successful contact** means the probe failed.
///
/// So an unsuccessful probe **leaves the last successful answer standing**, and a
/// capability that has genuinely disappeared is surfaced under NFR-29 rather than silently
/// reducing what the account can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probed<T> {
    value: T,
    /// Milliseconds since the epoch of the last **successful** probe. `None` means the
    /// value was configured or assumed rather than observed.
    observed_at: Option<u64>,
}

impl<T: Copy + PartialEq> Probed<T> {
    /// A value that was never probed — declared statically by an adapter that knows.
    #[must_use]
    pub const fn declared(value: T) -> Self {
        Self {
            value,
            observed_at: None,
        }
    }

    /// A value observed by a successful probe.
    #[must_use]
    pub const fn observed(value: T, at_millis: u64) -> Self {
        Self {
            value,
            observed_at: Some(at_millis),
        }
    }

    #[must_use]
    pub const fn get(&self) -> T {
        self.value
    }

    #[must_use]
    pub const fn was_observed(&self) -> bool {
        self.observed_at.is_some()
    }

    /// Record a **successful** probe.
    ///
    /// Returns whether the capability changed, because a capability that has gone away is
    /// something NFR-29 must surface — and an intent already queued against it must be
    /// quarantined rather than executed or discarded.
    pub fn succeeded(&mut self, value: T, at_millis: u64) -> bool {
        let changed = self.value != value;
        self.value = value;
        self.observed_at = Some(at_millis);
        changed
    }

    /// Record a **failed** probe.
    ///
    /// Deliberately does nothing to the value. "A failed probe is not an observation" — a
    /// server that timed out has not told us it stopped supporting anything, and treating
    /// silence as a retraction would strip an account's capabilities on a bad network.
    pub const fn failed(&mut self) {}
}

/// A capability row an older build does not recognise.
///
/// Preserved rather than dropped, so that a build which does understand it finds it intact
/// — the same discipline NFR-48 applies to an unrecognised queued intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrecognised {
    pub row: String,
    pub value: String,
}

/// Everything the planner is allowed to know about an account.
#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    pub location_cardinality: LocationCardinality,
    pub tag_support: TagSupport,
    pub archive: ArchiveSemantics,
    pub trash: TrashSemantics,
    pub permanent_delete: bool,
    pub thread_operations: ThreadOperations,
    pub junk_reporting: JunkReporting,
    pub delta: DeltaMechanism,
    pub push: PushMechanism,
    pub id_stability: IdStability,
    pub server_search: bool,
    /// Q-9. All four adapters currently declare this `Unknown`.
    pub max_batch_size: Magnitude,
    /// Requests per period, which lets D-87 pace *before* a throttle rather than reacting
    /// to one.
    pub request_budget: Magnitude,
    pub snippet_source: SnippetSource,
    /// Rule 2: rows this build does not understand, kept rather than dropped.
    pub unrecognised: Vec<Unrecognised>,
}

impl Capabilities {
    /// The conservative batch size used when Q-9 leaves the row unknown.
    ///
    /// Small enough that no provider is known to refuse it, which is the only property it
    /// needs until Q-9 supplies a real value per adapter.
    pub const CONSERVATIVE_BATCH: u32 = 20;

    /// The batch size to plan an FR-17 bulk operation against.
    #[must_use]
    pub fn batch_size(&self) -> u32 {
        self.max_batch_size.plan_against(Self::CONSERVATIVE_BATCH)
    }

    /// Whether the tag affordance is offered at all — FR-37.
    #[must_use]
    pub const fn offers_tags(&self) -> bool {
        matches!(self.tag_support, TagSupport::ReadWrite)
    }

    /// Whether a tag name is acceptable. L-15's bound, and the charset it names.
    #[must_use]
    pub fn accepts_tag_name(&self, name: &str) -> bool {
        self.offers_tags()
            && !name.is_empty()
            && name.chars().count() <= L15_TAG_NAME_CHARS as usize
            && !name.chars().any(char::is_control)
    }

    /// Whether "add tag" should render as "move to folder" instead — D-12's own example of
    /// the UI binding to capabilities rather than to a provider name.
    #[must_use]
    pub const fn tagging_is_really_moving(&self) -> bool {
        matches!(self.location_cardinality, LocationCardinality::ExactlyOne)
            && matches!(self.tag_support, TagSupport::None)
    }

    /// Whether a junk **report** affordance exists, as opposed to a move.
    #[must_use]
    pub const fn offers_junk_report(&self) -> bool {
        matches!(self.junk_reporting, JunkReporting::NativeReport)
    }

    /// Whether a move to the junk folder should be offered, **labelled as a move**.
    #[must_use]
    pub const fn offers_junk_move(&self) -> bool {
        matches!(self.junk_reporting, JunkReporting::FolderMoveOnly)
    }

    /// Whether a remote identifier survives a move, and can therefore key a join.
    ///
    /// `false` forces D-44's corroborated join. It is a capability question rather than a
    /// provider question, which is exactly why the sync engine can ask it without knowing
    /// who it is talking to.
    #[must_use]
    pub const fn identifier_survives_a_move(&self) -> bool {
        !matches!(self.id_stability, IdStability::UnstableOnMove)
    }

    /// Whether push must be dropped in favour of long aligned polls on cellular.
    ///
    /// The rule inverts the usual preference and applies **only** to cellular: a keepalive
    /// is cheap in bytes and each one promotes the radio to a high-power state through the
    /// tail timer, which is what NFR-37 bounds.
    #[must_use]
    pub const fn push_costs_radio_wakeups(&self) -> bool {
        matches!(
            self.push,
            PushMechanism::IdleOnePerConnection | PushMechanism::NotifyManyOverOne
        )
    }

    /// How many connections watching `folders` folders costs.
    ///
    /// The number L-23's budget is spent against, and what decides how many folders may be
    /// watched at all when a provider cannot watch several over one connection.
    #[must_use]
    pub const fn connections_for(&self, folders: u32) -> u32 {
        match self.push {
            PushMechanism::IdleOnePerConnection => folders,
            PushMechanism::EventStream | PushMechanism::NotifyManyOverOne => 1,
            PushMechanism::PollOnly => 0,
        }
    }

    /// Rule 2, as a method: unrecognised rows are ignored and the account is not refused.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four declared tables, transcribed from the per-provider documents.
    ///
    /// These live in a test rather than in the adapter crates on purpose. D-12 sets the
    /// test for whether this abstraction is right — *"if the abstraction needs
    /// provider-name special-casing to accommodate them, the abstraction is wrong"* — and
    /// the only way to run that test before the adapters exist is to declare what each
    /// will say and check the planner copes with all four uniformly.
    mod declared {
        use super::super::*;

        pub(super) fn jmap() -> Capabilities {
            Capabilities {
                location_cardinality: LocationCardinality::OneOrMore,
                tag_support: TagSupport::ReadWrite,
                archive: ArchiveSemantics::RemoveFromInbox,
                trash: TrashSemantics::MoveToTrash,
                permanent_delete: true,
                thread_operations: ThreadOperations::Native,
                junk_reporting: JunkReporting::NativeReport,
                delta: DeltaMechanism::ChangesQuery,
                push: PushMechanism::EventStream,
                id_stability: IdStability::StableGlobally,
                server_search: true,
                max_batch_size: Magnitude::Unknown,
                request_budget: Magnitude::Unknown,
                snippet_source: SnippetSource::ProviderSupplied,
                unrecognised: Vec::new(),
            }
        }

        pub(super) fn graph() -> Capabilities {
            Capabilities {
                location_cardinality: LocationCardinality::ExactlyOne,
                tag_support: TagSupport::ReadWrite,
                archive: ArchiveSemantics::MoveToSpecialUse,
                trash: TrashSemantics::MoveToTrash,
                permanent_delete: true,
                thread_operations: ThreadOperations::Native,
                junk_reporting: JunkReporting::NativeReport,
                delta: DeltaMechanism::DeltaLink,
                push: PushMechanism::PollOnly,
                id_stability: IdStability::UnstableOnMove,
                server_search: true,
                max_batch_size: Magnitude::Unknown,
                request_budget: Magnitude::Unknown,
                snippet_source: SnippetSource::ProviderSupplied,
                unrecognised: Vec::new(),
            }
        }

        pub(super) fn gmail() -> Capabilities {
            Capabilities {
                location_cardinality: LocationCardinality::OneOrMore,
                tag_support: TagSupport::ReadWrite,
                archive: ArchiveSemantics::RemoveFromInbox,
                trash: TrashSemantics::MoveToTrash,
                permanent_delete: true,
                thread_operations: ThreadOperations::Native,
                junk_reporting: JunkReporting::NativeReport,
                delta: DeltaMechanism::HistoryCursor,
                push: PushMechanism::IdleOnePerConnection,
                id_stability: IdStability::StableGlobally,
                server_search: true,
                max_batch_size: Magnitude::Unknown,
                request_budget: Magnitude::Unknown,
                snippet_source: SnippetSource::ProviderSupplied,
                unrecognised: Vec::new(),
            }
        }

        /// The only one whose values are **probed rather than known**. This is the
        /// pessimistic end of the range: a server advertising nothing useful.
        pub(super) fn imap_minimal() -> Capabilities {
            Capabilities {
                location_cardinality: LocationCardinality::ExactlyOne,
                tag_support: TagSupport::None,
                archive: ArchiveSemantics::MoveToSpecialUse,
                trash: TrashSemantics::FlagAndExpunge,
                permanent_delete: true,
                thread_operations: ThreadOperations::ClientFanOut,
                junk_reporting: JunkReporting::None,
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

        pub(super) fn all() -> Vec<(&'static str, Capabilities)> {
            vec![
                ("jmap", jmap()),
                ("graph", graph()),
                ("gmail", gmail()),
                ("imap-minimal", imap_minimal()),
            ]
        }
    }

    #[test]
    fn every_declared_table_plans_without_special_casing() {
        // D-12's own test for whether this abstraction is right. Every planner question is
        // asked of every provider and every one answers — no branch anywhere needs to know
        // which is which.
        for (name, c) in declared::all() {
            assert!(c.is_usable(), "{name} is not usable");
            assert!(c.batch_size() > 0, "{name} plans a batch of nothing");
            let _ = c.offers_tags();
            let _ = c.offers_junk_report();
            let _ = c.identifier_survives_a_move();
            let _ = c.connections_for(3);
            let _ = c.push_costs_radio_wakeups();
        }
    }

    #[test]
    fn q9_being_open_does_not_block_a_single_adapter() {
        // All four declare the batch size unknown, and all four still plan. That is the
        // whole of what rule 4 buys, and why Q-9 "no longer blocks an adapter from being
        // written".
        for (name, c) in declared::all() {
            assert_eq!(c.max_batch_size, Magnitude::Unknown, "{name}");
            assert_eq!(c.batch_size(), Capabilities::CONSERVATIVE_BATCH, "{name}");
        }
    }

    #[test]
    fn a_known_magnitude_is_used_and_an_unknown_one_is_conservative() {
        let known = Magnitude::Known {
            value: 1000,
            source: MagnitudeSource::Published,
        };
        assert_eq!(known.plan_against(20), 1000);
        assert_eq!(Magnitude::Unknown.plan_against(20), 20);
    }

    #[test]
    fn the_tag_affordance_follows_the_capability_rather_than_the_provider() {
        // D-12's stated example: "no tag support means no tag affordance; location
        // cardinality of exactly one means 'add tag' renders as 'move to folder'".
        assert!(declared::jmap().offers_tags());
        assert!(!declared::imap_minimal().offers_tags());
        assert!(declared::imap_minimal().tagging_is_really_moving());
        assert!(!declared::jmap().tagging_is_really_moving());
    }

    #[test]
    fn junk_reporting_is_absent_rather_than_approximated() {
        // FR-39. A server with no junk support offers neither affordance, and a server with
        // only a folder offers a move *labelled as a move* — never a report button that
        // silently does something else.
        let none = declared::imap_minimal();
        assert!(!none.offers_junk_report() && !none.offers_junk_move());

        let mut move_only = declared::imap_minimal();
        move_only.junk_reporting = JunkReporting::FolderMoveOnly;
        assert!(
            !move_only.offers_junk_report(),
            "a move was presented as a report"
        );
        assert!(move_only.offers_junk_move());
    }

    #[test]
    fn the_one_provider_whose_identifiers_do_not_survive_a_move_is_visible_as_a_capability() {
        // "The single most important fact about this adapter" is a capability rather than a
        // provider name, which is what lets the sync engine ask about it without knowing
        // who it is talking to.
        assert!(!declared::graph().identifier_survives_a_move());
        for name in ["jmap", "gmail", "imap-minimal"] {
            let c = declared::all()
                .into_iter()
                .find(|(n, _)| *n == name)
                .unwrap()
                .1;
            assert!(c.identifier_survives_a_move(), "{name}");
        }
    }

    #[test]
    fn the_connection_budget_is_spent_by_the_push_mechanism() {
        // L-23 is 16 for the whole installation, and this is what decides how many folders
        // may be watched at all when a provider cannot watch several over one connection.
        assert_eq!(
            declared::gmail().connections_for(3),
            3,
            "one connection per mailbox"
        );
        assert_eq!(
            declared::jmap().connections_for(3),
            1,
            "one stream for all of them"
        );
        assert_eq!(
            declared::graph().connections_for(3),
            0,
            "polling holds nothing open"
        );

        let mut notify = declared::imap_minimal();
        notify.push = PushMechanism::NotifyManyOverOne;
        assert_eq!(
            notify.connections_for(10),
            1,
            "NOTIFY must be preferred for this reason"
        );
    }

    #[test]
    fn five_accounts_watching_three_folders_each_fits_the_connection_budget() {
        // The arithmetic L-23 is derived from, run against the declared tables rather than
        // asserted in prose: "five accounts watching three folders each, plus one for
        // on-demand work".
        let budget = u32::from(sift_foundation::limits::L23_CONNECTIONS as u16);
        let on_demand = 1;
        let accounts = 5;
        let watched_folders = 3;

        for (name, c) in declared::all() {
            let held = c.connections_for(watched_folders) * accounts + on_demand;
            assert!(
                held <= budget,
                "{name} would hold {held} connections against a budget of {budget}"
            );
        }

        // The mechanism that spends the most is one connection per watched mailbox, and it
        // is exactly what the budget was sized for — which is why NOTIFY "MUST be preferred
        // where available" and why, where neither is, Sift watches only the inbox and
        // refreshes other folders lazily on navigation.
        assert_eq!(
            declared::gmail().connections_for(watched_folders) * accounts + on_demand,
            budget,
            "the budget is no longer the arithmetic it was derived from"
        );
    }

    #[test]
    fn a_failed_probe_leaves_the_last_successful_answer_standing() {
        // Rule 5. A server that timed out has not told us it stopped supporting anything,
        // and treating silence as a retraction would strip an account's capabilities on a
        // bad network — which is the same mistake D-88 refuses to make about credentials.
        let mut probed = Probed::observed(TagSupport::ReadWrite, 1_000);
        probed.failed();
        assert_eq!(probed.get(), TagSupport::ReadWrite);
        assert!(probed.was_observed());
    }

    #[test]
    fn a_capability_that_genuinely_went_away_is_reported_as_changed() {
        // So that NFR-29 can surface it, and so that an intent already queued against it is
        // quarantined rather than executed or discarded.
        let mut probed = Probed::observed(TagSupport::ReadWrite, 1_000);
        assert!(
            probed.succeeded(TagSupport::None, 2_000),
            "the loss was not reported"
        );
        assert!(
            !probed.succeeded(TagSupport::None, 3_000),
            "an unchanged probe reported a change"
        );
    }

    #[test]
    fn absence_at_first_contact_and_absence_after_success_are_not_the_same_thing() {
        let never = Probed::declared(TagSupport::None);
        let mut lost = Probed::observed(TagSupport::ReadWrite, 1_000);
        lost.succeeded(TagSupport::None, 2_000);
        assert_eq!(never.get(), lost.get(), "the values agree");
        assert!(!never.was_observed());
        assert!(
            lost.was_observed(),
            "the two absences are stored as the same thing"
        );
    }

    #[test]
    fn an_unrecognised_capability_is_ignored_rather_than_fatal() {
        // Rule 2, and deliberately the opposite of D-66's rule at the shell boundary, where
        // an unknown discriminant is a *build* failure. The difference is justified: both
        // sides of that boundary ship in one binary, and a stored capability set does not.
        let mut c = declared::jmap();
        c.unrecognised.push(Unrecognised {
            row: "quantum-threading".to_owned(),
            value: "entangled".to_owned(),
        });
        assert!(
            c.is_usable(),
            "the account was refused over a row nobody understands"
        );
        assert_eq!(c.batch_size(), Capabilities::CONSERVATIVE_BATCH);
        assert_eq!(
            c.unrecognised.len(),
            1,
            "the row was dropped rather than preserved"
        );
    }

    #[test]
    fn a_tag_name_is_bounded_by_l15_and_excludes_control_characters() {
        let c = declared::jmap();
        assert!(c.accepts_tag_name("work"));
        assert!(!c.accepts_tag_name(""));
        assert!(!c.accepts_tag_name(&"x".repeat(L15_TAG_NAME_CHARS as usize + 1)));
        assert!(!c.accepts_tag_name("work\u{0007}"));
        assert!(
            c.accepts_tag_name(&"x".repeat(L15_TAG_NAME_CHARS as usize)),
            "the bound is inclusive"
        );
    }

    #[test]
    fn a_server_with_no_efficient_delta_is_visible_as_one() {
        // NFR-29: where resync cost is proportional to mailbox size rather than change
        // volume, Sift must degrade explicitly and visibly rather than quietly being slow.
        assert_eq!(declared::imap_minimal().delta, DeltaMechanism::FullScan);
        for name in ["jmap", "graph", "gmail"] {
            let c = declared::all()
                .into_iter()
                .find(|(n, _)| *n == name)
                .unwrap()
                .1;
            assert_ne!(c.delta, DeltaMechanism::FullScan, "{name}");
        }
    }

    #[test]
    fn only_client_derived_snippets_risk_marking_a_mailbox_read() {
        // The named hazard: fetching body text without the peek form sets the seen flag, so
        // a backfill-derived snippet would mark an entire mailbox read.
        assert_eq!(
            declared::imap_minimal().snippet_source,
            SnippetSource::ClientDerived
        );
        assert_eq!(
            declared::jmap().snippet_source,
            SnippetSource::ProviderSupplied
        );
    }
}

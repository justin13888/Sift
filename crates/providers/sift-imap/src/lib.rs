//! D-31, NFR-29 — the generic IMAP adapter.
//!
//! # The only adapter whose capabilities are probed rather than known
//!
//! The other three know what they can do. This one asks, and the asking is governed by the
//! provider model's fifth growth rule: **a probed capability is a cached observation, and a
//! failed probe is not an observation.** Absence at first contact means unsupported; absence
//! after a successful contact means the probe failed; and the two MUST NOT be stored as the
//! same thing — otherwise a bad network silently strips an account's capabilities.
//!
//! # Why the client is hand-written
//!
//! D-31 rejects an existing crate for three reasons, and the first is the one that decides
//! it: **NFR-29 needs the detail a general client abstracts away** — which extensions were
//! advertised, which were tried, and what the server actually did. A client that returns
//! "sync failed" cannot produce the *degraded* condition with a stated reason.
//!
//! Second, the NOTIFY-else-IDLE-else-poll preference, the re-issue before server and
//! network-address-translation timeouts, and the connection budget have to be exactly right,
//! and they are the parts a general client makes convenient rather than exact.
//!
//! Third: **server responses are remote input from a host the user chose but Sift does not
//! trust.** They need the same hardening posture as message bodies.
//!
//! D-31 records its own weakness plainly: **"small subset of IMAP" has defeated
//! better-resourced projects**, and this shows up as schedule rather than as design. The
//! contestable reading — start from a crate and replace it incrementally — is legitimate and
//! recorded.

use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, Probed, PushMechanism, SnippetSource, TagSupport,
    ThreadOperations, TrashSemantics,
};

/// What a server advertised.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Advertised {
    pub condstore: bool,
    pub qresync: bool,
    pub notify: bool,
    pub idle: bool,
    pub objectid: bool,
    /// Whether the server accepts arbitrary keywords, which is what tag support turns on.
    pub arbitrary_keywords: bool,
    /// Whether a junk special-use folder resolved.
    pub junk_special_use: bool,
    pub junk_keywords: bool,
    pub special_use: bool,
}

/// Derive the declared capability set from what a server advertised.
///
/// Every value here is an observation rather than a constant, which is why the whole
/// function exists: on this provider the capability table is a *report* about one server.
#[must_use]
pub fn capabilities_from(advertised: &Advertised) -> Capabilities {
    Capabilities {
        location_cardinality: LocationCardinality::ExactlyOne,
        tag_support: if advertised.arbitrary_keywords {
            TagSupport::ReadWrite
        } else {
            // Not "read-only": a server that will not accept arbitrary keywords offers no
            // tags at all, and offering a read-only tag view of nothing would be an
            // affordance for an empty set.
            TagSupport::None
        },
        archive: ArchiveSemantics::MoveToSpecialUse,
        trash: TrashSemantics::MoveToTrash,
        permanent_delete: true,
        // No native conversation identifier, so one thread intent becomes N message
        // operations — and FR-38's partial-failure semantics stop being theoretical.
        thread_operations: ThreadOperations::ClientFanOut,
        junk_reporting: if advertised.junk_keywords {
            JunkReporting::NativeReport
        } else if advertised.junk_special_use {
            // A move **labelled as what it is**. FR-39 forbids presenting a report affordance
            // that silently does something else.
            JunkReporting::FolderMoveOnly
        } else {
            JunkReporting::None
        },
        delta: if advertised.qresync && advertised.condstore {
            DeltaMechanism::ModSeqResync
        } else {
            // **Resync cost becomes proportional to mailbox size rather than change
            // volume**, which NFR-29 requires be surfaced rather than quietly endured.
            DeltaMechanism::FullScan
        },
        push: if advertised.notify {
            // MUST be preferred where available: it watches many mailboxes over one
            // connection, and L-23's budget is what decides how many folders may be watched
            // at all.
            PushMechanism::NotifyManyOverOne
        } else if advertised.idle {
            PushMechanism::IdleOnePerConnection
        } else {
            PushMechanism::PollOnly
        },
        id_stability: if advertised.objectid {
            IdStability::StableGlobally
        } else {
            // Keyed on folder validity plus UID. A validity change invalidates **every UID in
            // that folder**, which is a cursor invalidation rather than a corruption.
            IdStability::StablePerFolder
        },
        server_search: true,
        max_batch_size: Magnitude::Unknown,
        request_budget: Magnitude::Unknown,
        // No preview field exists. Derived **only from a body already fetched, never by
        // fetching one** — because fetching body text without the peek form sets the seen
        // flag, and a backfill-derived snippet would mark an entire mailbox read.
        snippet_source: SnippetSource::ClientDerived,
        unrecognised: Vec::new(),
    }
}

/// A probed capability set, with the discipline the fifth growth rule requires.
#[derive(Debug)]
pub struct Probe {
    tags: Probed<TagSupport>,
    junk: Probed<JunkReporting>,
}

impl Probe {
    /// A server never successfully contacted. Absence here means **unsupported**.
    #[must_use]
    pub const fn at_first_contact() -> Self {
        Self {
            tags: Probed::declared(TagSupport::None),
            junk: Probed::declared(JunkReporting::None),
        }
    }

    /// Record a successful probe. Returns whether anything **went away**, which NFR-29 must
    /// surface and which quarantines any intent already queued against it.
    pub fn succeeded(&mut self, advertised: &Advertised, at_millis: u64) -> bool {
        let c = capabilities_from(advertised);
        let tags_changed = self.tags.succeeded(c.tag_support, at_millis);
        let junk_changed = self.junk.succeeded(c.junk_reporting, at_millis);
        tags_changed || junk_changed
    }

    /// Record a failed probe.
    ///
    /// **Deliberately does nothing.** A server that timed out has not told us it stopped
    /// supporting anything, and treating silence as a retraction would strip an account's
    /// capabilities on a bad network — the same mistake D-88 refuses to make about a
    /// credential.
    pub const fn failed(&mut self) {
        self.tags.failed();
        self.junk.failed();
    }

    #[must_use]
    pub const fn tag_support(&self) -> TagSupport {
        self.tags.get()
    }

    #[must_use]
    pub const fn junk_reporting(&self) -> JunkReporting {
        self.junk.get()
    }

    /// Whether these values were ever actually observed.
    #[must_use]
    pub const fn was_observed(&self) -> bool {
        self.tags.was_observed()
    }
}

/// How many folders may be watched, given the connection budget.
///
/// Where neither NOTIFY nor enough connections are available, **Sift watches only the inbox
/// and refreshes other folders lazily on user navigation** — with FR-12's "not cached" state
/// carrying the difference honestly rather than pretending the folder is up to date.
#[must_use]
pub fn watchable_folders(advertised: &Advertised, budget: u32, wanted: u32) -> u32 {
    match capabilities_from(advertised).push {
        PushMechanism::NotifyManyOverOne | PushMechanism::EventStream => wanted,
        PushMechanism::IdleOnePerConnection => wanted.min(budget),
        // Polling holds nothing open, so the budget does not bind — what binds is NFR-11's
        // wakeup count, and the scheduler coalesces that.
        PushMechanism::PollOnly => wanted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capable() -> Advertised {
        Advertised {
            condstore: true,
            qresync: true,
            notify: true,
            idle: true,
            objectid: true,
            arbitrary_keywords: true,
            junk_special_use: true,
            junk_keywords: true,
            special_use: true,
        }
    }

    #[test]
    fn a_capable_server_declares_almost_everything() {
        let c = capabilities_from(&capable());
        assert_eq!(c.delta, DeltaMechanism::ModSeqResync);
        assert_eq!(c.push, PushMechanism::NotifyManyOverOne);
        assert_eq!(c.id_stability, IdStability::StableGlobally);
        assert!(c.offers_tags());
        assert!(c.offers_junk_report());
    }

    #[test]
    fn a_bare_server_declares_the_pessimistic_end() {
        let c = capabilities_from(&Advertised::default());
        assert_eq!(
            c.delta,
            DeltaMechanism::FullScan,
            "resync cost is mailbox-sized"
        );
        assert_eq!(c.push, PushMechanism::PollOnly);
        assert_eq!(c.id_stability, IdStability::StablePerFolder);
        assert!(!c.offers_tags());
        assert!(!c.offers_junk_report() && !c.offers_junk_move());
    }

    #[test]
    fn a_server_that_refuses_keywords_offers_no_tags_rather_than_read_only_ones() {
        // A read-only view of an empty set is an affordance for nothing.
        let mut a = capable();
        a.arbitrary_keywords = false;
        assert_eq!(capabilities_from(&a).tag_support, TagSupport::None);
    }

    #[test]
    fn a_junk_folder_without_keywords_offers_a_move_labelled_as_one() {
        // FR-39 forbids presenting a report affordance that silently does something else.
        let mut a = capable();
        a.junk_keywords = false;
        let c = capabilities_from(&a);
        assert!(!c.offers_junk_report());
        assert!(c.offers_junk_move());
    }

    #[test]
    fn notify_is_preferred_over_idle_where_both_are_advertised() {
        // It watches many mailboxes over one connection, and L-23's budget is what decides
        // how many folders may be watched at all.
        assert_eq!(capabilities_from(&capable()).connections_for(10), 1);
        let mut idle_only = capable();
        idle_only.notify = false;
        assert_eq!(capabilities_from(&idle_only).connections_for(10), 10);
    }

    #[test]
    fn without_notify_the_watched_set_is_bounded_by_the_connection_budget() {
        // Where neither is available, Sift watches only the inbox and refreshes the rest
        // lazily, with FR-12's "not cached" carrying the difference honestly.
        let mut idle_only = capable();
        idle_only.notify = false;
        assert_eq!(watchable_folders(&idle_only, 3, 10), 3);
        assert_eq!(
            watchable_folders(&capable(), 3, 10),
            10,
            "NOTIFY was bounded by connections"
        );
    }

    #[test]
    fn snippets_are_client_derived_which_is_why_they_may_never_cause_a_fetch() {
        // Fetching body text without the peek form sets the seen flag, and a
        // backfill-derived snippet would mark an entire mailbox read.
        assert_eq!(
            capabilities_from(&capable()).snippet_source,
            SnippetSource::ClientDerived
        );
    }

    #[test]
    fn threads_fan_out_because_there_is_no_native_conversation() {
        // Which makes FR-38's partial-failure semantics real rather than theoretical.
        assert_eq!(
            capabilities_from(&capable()).thread_operations,
            ThreadOperations::ClientFanOut
        );
    }

    #[test]
    fn absence_at_first_contact_means_unsupported() {
        let p = Probe::at_first_contact();
        assert_eq!(p.tag_support(), TagSupport::None);
        assert!(
            !p.was_observed(),
            "an unprobed value claimed to be an observation"
        );
    }

    #[test]
    fn a_failed_probe_leaves_the_last_successful_answer_standing() {
        // A server that timed out has not said it stopped supporting anything. Treating
        // silence as a retraction strips capabilities on a bad network.
        let mut p = Probe::at_first_contact();
        p.succeeded(&capable(), 1_000);
        assert_eq!(p.tag_support(), TagSupport::ReadWrite);
        p.failed();
        assert_eq!(
            p.tag_support(),
            TagSupport::ReadWrite,
            "a timeout removed tag support"
        );
        assert!(p.was_observed());
    }

    #[test]
    fn a_capability_that_genuinely_went_away_is_reported() {
        // So NFR-29 can surface it, and so an intent already queued against it is quarantined
        // rather than executed or discarded.
        let mut p = Probe::at_first_contact();
        p.succeeded(&capable(), 1_000);
        let mut lost = capable();
        lost.arbitrary_keywords = false;
        assert!(p.succeeded(&lost, 2_000), "the loss was not reported");
        assert_eq!(p.tag_support(), TagSupport::None);
    }

    #[test]
    fn an_unchanged_probe_reports_no_change() {
        let mut p = Probe::at_first_contact();
        p.succeeded(&capable(), 1_000);
        assert!(!p.succeeded(&capable(), 2_000));
    }
}

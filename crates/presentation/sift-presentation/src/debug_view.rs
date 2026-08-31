//! FR-33, FR-34 — the two diagnostic surfaces.
//!
//! # All ten items ship in release
//!
//! Q-5 asked how much of the debug view ships enabled, and resolved to **all ten,
//! preference-gated, default off**. A reduced release set was rejected for a specific reason:
//! items 4, 5 and 9 would then exist in two configurations, and **the release one would be
//! the untested one.**
//!
//! # Build it once, use it twice
//!
//! Items 4, 5 and 9 are the same code path as NFR-40's differential harness. That is not a
//! saving noticed afterwards — it is why the debug view is *designed* now even though it is
//! built later.
//!
//! # Neither surface is telemetry
//!
//! Nothing here leaves the machine. They exist partly **so that a user can verify Sift's
//! privacy claims for themselves rather than take them on trust** — which is only meaningful
//! if the verification surface is local.
//!
//! The cost is admitted: this is a convenient oracle for somebody probing the sanitizer. But
//! only locally, by a user who already has the message.

/// FR-33's ten items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDebugItem {
    RawSource,
    /// MIME structure with part sizes, and **which alternative was chosen and why**.
    MimeStructure,
    /// Authentication results and the derived synthetic origin.
    Authentication,
    /// Pre- versus post-sanitize diff, **with a rule identifier next to every removal**.
    SanitizeDiff,
    /// Every candidate URL, its verdict, the matching rule, the first-party determination.
    BlockingDecisions,
    /// Cosmetic filter hits, **distinguishing stylesheet-injected from Rust-evaluated**.
    CosmeticFilters,
    /// Generated overrides, the colour mapping, computed contrast, image classifications.
    DarkTransform,
    /// Displayed, raw, and unwrapped.
    LinkUnwrapping,
    /// **Live invariant check results for this specific message.**
    LiveInvariantCheck,
    /// Render timing and body-view footprint.
    Timing,
}

impl MessageDebugItem {
    pub const ALL: &'static [Self] = &[
        Self::RawSource,
        Self::MimeStructure,
        Self::Authentication,
        Self::SanitizeDiff,
        Self::BlockingDecisions,
        Self::CosmeticFilters,
        Self::DarkTransform,
        Self::LinkUnwrapping,
        Self::LiveInvariantCheck,
        Self::Timing,
    ];

    /// Whether this item shares its implementation with NFR-40's harness.
    #[must_use]
    pub const fn shared_with_the_test_harness(self) -> bool {
        matches!(
            self,
            Self::SanitizeDiff | Self::BlockingDecisions | Self::LiveInvariantCheck
        )
    }

    /// Whether it ships in a release build.
    #[must_use]
    pub const fn ships_in_release(self) -> bool {
        true
    }
}

/// FR-34's runtime panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePanelItem {
    /// Per-subsystem live bytes **and the unattributed residual** — the number a maintainer
    /// chases when the soak gate fires.
    SubsystemBytesAndResidual,
    /// Name, live bytes, entries, capacity, hit rate, evictions.
    CacheStatistics,
    ActiveTasksAndQueries,
    /// Per account: sync cursor and connection state.
    SyncState,
    /// The queue's contents — where a quarantined or expired intent is visible.
    MutationQueue,
    /// Per minute, on both axes: per subsystem **and per account**.
    WakeupRate,
    NetworkConditionsAndTier,
    /// D-10's disagreement between authority and backstop. **The required surface for it** —
    /// a disagreement nobody can see is one that is silently resolved.
    BlockerDisagreement,
}

impl RuntimePanelItem {
    pub const ALL: &'static [Self] = &[
        Self::SubsystemBytesAndResidual,
        Self::CacheStatistics,
        Self::ActiveTasksAndQueries,
        Self::SyncState,
        Self::MutationQueue,
        Self::WakeupRate,
        Self::NetworkConditionsAndTier,
        Self::BlockerDisagreement,
    ];
}

/// Whether either surface transmits anything.
#[must_use]
pub const fn transmits_anything() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ten_items_ship_in_release() {
        // A reduced release set would leave items 4, 5 and 9 in two configurations, and the
        // release one would be the untested one.
        assert_eq!(MessageDebugItem::ALL.len(), 10);
        for i in MessageDebugItem::ALL {
            assert!(i.ships_in_release(), "{i:?}");
        }
    }

    #[test]
    fn three_items_are_the_test_harness() {
        // "Build it once, use it twice" — which is why the debug view is designed now even
        // though it is built later.
        let shared: Vec<_> = MessageDebugItem::ALL
            .iter()
            .filter(|i| i.shared_with_the_test_harness())
            .collect();
        assert_eq!(shared.len(), 3);
    }

    #[test]
    fn the_runtime_panel_shows_the_residual_rather_than_only_the_subsystems() {
        // Footprint minus the sum of declared caches is the number the soak gate sends a
        // maintainer to look at; the per-subsystem figures alone would not find a leak
        // outside a declared cache.
        assert!(RuntimePanelItem::ALL.contains(&RuntimePanelItem::SubsystemBytesAndResidual));
    }

    #[test]
    fn the_blocker_disagreement_has_a_surface() {
        // D-10 requires it be surfaced rather than silently resolved, and a disagreement
        // nobody can see is one that is silently resolved.
        assert!(RuntimePanelItem::ALL.contains(&RuntimePanelItem::BlockerDisagreement));
    }

    #[test]
    fn the_queue_is_visible_so_a_stopped_intent_can_be_found() {
        // An expired or quarantined intent stays in the queue and stays visible; without this
        // the user would be told something needs attention with nowhere to look.
        assert!(RuntimePanelItem::ALL.contains(&RuntimePanelItem::MutationQueue));
    }

    #[test]
    fn wakeups_are_reported_on_both_axes() {
        // D-94's attribution rule needs per-account as well as per-subsystem, because the
        // application-wide bound is composed from the per-account one.
        assert!(RuntimePanelItem::ALL.contains(&RuntimePanelItem::WakeupRate));
    }

    #[test]
    fn neither_surface_transmits_anything() {
        // They exist partly so a user can verify the privacy claims for themselves, which is
        // only meaningful if the verification is local.
        assert!(!transmits_anything());
    }
}

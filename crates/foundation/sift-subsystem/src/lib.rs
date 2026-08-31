//! The frozen subsystem partition: eighteen exhaustive, non-overlapping rows.
//!
//! This is the **allocation-attribution partition**, and `docs/build/workspace.md` is
//! explicit that it **MUST NOT be read as a module layout**. The code partition is
//! D-59's crate graph and it is a different shape on purpose: several of the rows below
//! span crates, and one crate — the pipeline — changes its tag mid-run.
//!
//! # Why the partition has to be exhaustive and non-overlapping
//!
//! Explicit cache accounting is the primary mechanism, not the allocator hook. Every
//! cache reports its name, live bytes, entry count, capacity, hit rate and eviction
//! count. **The allocator hook exists to catch memory that is not in a declared cache** —
//! and what a maintainer chases when NFR-45's slope gate fires is the *residual*: total
//! footprint minus the sum of declared caches.
//!
//! That subtraction is why the two properties are not stylistic. **Overlap makes the
//! residual go negative. Non-exhaustiveness buries the leak signal.** Anything that fits
//! none of the rows below is a missing row, not a reason to widen one.
//!
//! # The names are stable, and re-partitioning is not a refactor
//!
//! NFR-45 gates on a slope over at least 72 hours and NFR-12 is a 14-day property.
//! Splitting or moving a subsystem **discontinues a history that cannot be regenerated** —
//! there is no way to recompute last fortnight's series under a new partition. Adding a
//! row for genuinely new code is fine; renaming or merging one is not.
//!
//! # The tag follows the allocation, not the use
//!
//! Three placements are settled here rather than left to whoever writes the code, because
//! each is a case where the allocating subsystem and the using subsystem differ:
//!
//! - **A decoded image buffer is [`Broker`](Subsystem::Broker)'s.** The durable
//!   classification record derived from it is [`Blobs`](Subsystem::Blobs)'. The dark
//!   transform's working state over it is [`Sanitize`](Subsystem::Sanitize)'s.
//! - **A caught panic is counted against the subsystem whose tag was current** under
//!   D-24's task-scoped tag — not against a counter of its own. A `Panics` row would break
//!   exhaustiveness by owning no memory.
//! - **The application shell is [`Shell`](Subsystem::Shell)'s**, alongside window shells,
//!   even though it outlives every window.

/// One row of the attribution partition.
///
/// The discriminants are not stable and must not be persisted; [`Subsystem::name`] is
/// what a stored series is keyed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subsystem {
    /// The native view hierarchy and everything created and destroyed with a window —
    /// **including the application shell**, which is resident for the life of the process
    /// and owns the always-on surface L3 must not destroy.
    Shell,
    /// View models, windowing, the unified-inbox merge, and the diff that feeds D-18's
    /// ordered change notifications.
    Presentation,
    /// Provider clients, connections, and in-flight request and response state.
    Adapters,
    /// Delta application, cursors, and per-folder sync state.
    Sync,
    /// Account databases **and the page-encryption layer beneath them**.
    Store,
    /// The full-text index and its query machinery.
    Index,
    /// The blob store, the shared blob index, and the content-addressing path. The
    /// durable image classification record lives here, though the decode that produced it
    /// does not.
    Blobs,
    /// The durable queue, D-51's pending overlays, and reconciliation.
    Mutations,
    /// The credential broker, and key material held in memory while in use.
    Credentials,
    /// The timing wheel and the connection budget.
    Scheduler,
    /// Condition detection, policy-tier evaluation, and data accounting.
    Network,
    /// MIME parsing and part selection — **pipeline stages 1 and 2**.
    Parse,
    /// The tree builder, the allowlist policy, and the CSS cascade — **pipeline stages 3,
    /// 4 and 6**. The pipeline therefore changes its tag mid-run, at a stage boundary,
    /// which is the single point D-92 also uses for panic containment and cancellation.
    Sanitize,
    /// The resource broker, its classification cache, **and the filter engine** — the
    /// 40 MB NFR-42 budgets and L1 sheds.
    Broker,
    /// The host side of the body view, and the engine processes' contribution to
    /// footprint. Under D-16 that contribution is counted, which is why Linux is measured
    /// as PSS rather than RSS: the engine is multi-process with shared mappings and RSS
    /// would double-count it.
    Bodyview,
    /// D-93's pressure governor and its tier state.
    Governor,
    /// NFR-55's bounded local log buffer.
    Logging,
    /// The async runtime, thread stacks, and allocator metadata.
    Runtime,
}

impl Subsystem {
    /// Every row. Walked by the attribution reporter, so a row absent here is a row that
    /// silently reports nothing.
    pub const ALL: &'static [Self] = &[
        Self::Shell,
        Self::Presentation,
        Self::Adapters,
        Self::Sync,
        Self::Store,
        Self::Index,
        Self::Blobs,
        Self::Mutations,
        Self::Credentials,
        Self::Scheduler,
        Self::Network,
        Self::Parse,
        Self::Sanitize,
        Self::Broker,
        Self::Bodyview,
        Self::Governor,
        Self::Logging,
        Self::Runtime,
    ];

    /// The stable name a stored series is keyed on.
    ///
    /// Changing one of these strings discontinues every series recorded under the old
    /// name. NFR-12 is a fourteen-day property, so that history cannot be recomputed.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Presentation => "presentation",
            Self::Adapters => "adapters",
            Self::Sync => "sync",
            Self::Store => "store",
            Self::Index => "index",
            Self::Blobs => "blobs",
            Self::Mutations => "mutations",
            Self::Credentials => "credentials",
            Self::Scheduler => "scheduler",
            Self::Network => "network",
            Self::Parse => "parse",
            Self::Sanitize => "sanitize",
            Self::Broker => "broker",
            Self::Bodyview => "bodyview",
            Self::Governor => "governor",
            Self::Logging => "logging",
            Self::Runtime => "runtime",
        }
    }

    /// Recover a row from a stored series' key.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    /// The index used by the per-CPU sharded counters D-24 requires — one contended
    /// atomic per subsystem would be its own performance problem, and NFR-44 bounds the
    /// whole mechanism at 2% in release.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// How many rows there are, for sizing a counter array at compile time.
    pub const COUNT: usize = Self::ALL.len();
}

impl core::fmt::Display for Subsystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_partition_has_eighteen_rows() {
        assert_eq!(
            Subsystem::COUNT,
            18,
            "docs/runtime/observability.md enumerates eighteen"
        );
    }

    #[test]
    fn every_row_appears_in_all_exactly_once() {
        // A row missing from ALL is a row that silently reports nothing, which is
        // non-exhaustiveness — and non-exhaustiveness buries the leak signal.
        let set: BTreeSet<Subsystem> = Subsystem::ALL.iter().copied().collect();
        assert_eq!(set.len(), Subsystem::COUNT, "a row appears twice in ALL");
    }

    #[test]
    fn names_are_unique() {
        let names: BTreeSet<&str> = Subsystem::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names.len(), Subsystem::COUNT, "two rows share a name");
    }

    #[test]
    fn a_name_round_trips() {
        for s in Subsystem::ALL {
            assert_eq!(Subsystem::from_name(s.name()), Some(*s));
        }
        assert_eq!(
            Subsystem::from_name("panics"),
            None,
            "a caught panic is not its own row"
        );
    }

    #[test]
    fn index_matches_position_in_all() {
        // The counter array is indexed by discriminant; if that stops matching ALL's
        // order, every counter reports another subsystem's bytes.
        for (i, s) in Subsystem::ALL.iter().enumerate() {
            assert_eq!(
                s.index(),
                i,
                "{s} is at position {i} but indexes {}",
                s.index()
            );
        }
    }

    #[test]
    fn the_names_are_the_ones_recorded_in_the_specification() {
        // These strings key a fourteen-day series. Renaming one discontinues history that
        // cannot be regenerated, so the names are pinned here rather than merely derived
        // from the variant.
        const RECORDED: &[&str] = &[
            "shell",
            "presentation",
            "adapters",
            "sync",
            "store",
            "index",
            "blobs",
            "mutations",
            "credentials",
            "scheduler",
            "network",
            "parse",
            "sanitize",
            "broker",
            "bodyview",
            "governor",
            "logging",
            "runtime",
        ];
        let actual: Vec<&str> = Subsystem::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(actual, RECORDED);
    }

    #[test]
    fn the_pipeline_is_split_across_two_rows() {
        // D-92: the subsystem tag changes mid-pipeline, at a stage boundary. Parse owns
        // stages 1 and 2; Sanitize owns 3, 4 and 6. Collapsing them would hide which half
        // of the pipeline is holding memory, which is the question NFR-41's budget and
        // R-9's doubt about the cascade both turn on.
        assert_ne!(Subsystem::Parse, Subsystem::Sanitize);
    }
}

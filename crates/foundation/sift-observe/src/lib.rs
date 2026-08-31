//! D-16, D-34, NFR-45 — what is counted, and what the soak gate reads.
//!
//! # Explicit cache accounting is the primary mechanism
//!
//! Not the allocator hook. **Every cache reports its name, live bytes, entry count,
//! capacity, hit rate and eviction count**, and those declared numbers are where the figures
//! come from. The allocator exists to catch what is *not* in a declared cache — and the
//! **residual**, total footprint minus the sum of declared caches, is what a maintainer
//! chases when the slope gate fires.
//!
//! # The metric is footprint, never RSS
//!
//! D-16: `phys_footprint` on macOS, **PSS** on Linux. On Linux the engine is multi-process
//! with shared mappings, so RSS double-counts it — and the figure that matters is "Sift's
//! process plus whatever engine processes it currently owns". Two metrics instead of one,
//! and neither is what most tooling reports by default.

pub mod cache;
pub mod soak;
pub mod wakeups;

/// How memory is measured, per platform.
///
/// Modelled rather than assumed, because using the wrong one silently produces numbers that
/// look plausible and are not comparable to anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// macOS.
    PhysFootprint,
    /// Linux. Shared mappings counted proportionally, which is what stops the engine's
    /// pages being counted once per process.
    ProportionalSetSize,
}

impl Metric {
    #[must_use]
    pub const fn for_this_platform() -> Self {
        if cfg!(target_os = "macos") {
            Self::PhysFootprint
        } else {
            Self::ProportionalSetSize
        }
    }

    /// Whether resident set size is ever the right measure.
    ///
    /// **Never.** It double-counts the engine's shared mappings on Linux, and on macOS it
    /// counts pages the process has already returned.
    #[must_use]
    pub const fn resident_set_size_is_acceptable() -> bool {
        false
    }
}

/// D-34 — the trace format.
///
/// An established viewer's protobuf format, chosen by the deciding consumer rather than by
/// taste: NFR-45's soak harness produces **a long, dense counter series over at least 72
/// hours**, and the JSON trace-event format is verbose text with no real representation for
/// one. A sampling-profiler format inverts the problem.
///
/// D-34 records its own weakness: if the soak harness ends up consuming counters through a
/// separate channel anyway, the argument no longer applies to the traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceFormat {
    ViewerProtobuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_set_size_is_never_the_measure() {
        // It double-counts the engine's shared mappings on Linux and counts returned pages on
        // macOS. Both produce numbers that look plausible and compare to nothing.
        assert!(!Metric::resident_set_size_is_acceptable());
    }

    #[test]
    fn each_platform_has_its_own_metric() {
        let m = Metric::for_this_platform();
        assert!(matches!(
            m,
            Metric::PhysFootprint | Metric::ProportionalSetSize
        ));
    }
}

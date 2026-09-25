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

/// This process's footprint, measured the way [`Metric::for_this_platform`] says.
///
/// `phys_footprint` on macOS, PSS on Linux, and `None` anywhere else or wherever the
/// platform refused — a missing sample, never a zero, because a zero in a fitted series reads
/// as the footprint collapsing and flatters the slope.
///
/// **This process only.** D-16 counts the body view's engine processes too, and on Linux
/// they are separate processes with shared mappings. A caller that owns engine processes
/// adds their PSS to this; one that owns none — the soak harness, which renders to HTML and
/// shows it nowhere — has nothing to add.
#[must_use]
pub fn footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        sift_alloc::phys_footprint()
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/smaps_rollup")
            .ok()
            .as_deref()
            .and_then(pss_from_smaps_rollup)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// CPU time this process has spent, user plus system, **in the platform's own units** —
/// timebase ticks on macOS, clock ticks on Linux.
///
/// Only ever divided by another reading from the same machine, which is what NFR-44's
/// benchmark does: wall time on a shared machine charges a trial for every other process that
/// ran beside it, and a 2% budget cannot be resolved through that.
#[must_use]
pub fn cpu_time() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        sift_alloc::cpu_time_units()
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/stat")
            .ok()
            .as_deref()
            .and_then(cpu_ticks_from_stat)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// `utime + stime` from a Linux `/proc/<pid>/stat` line.
///
/// Split after the **last** `)`, because the command name before it is in parentheses and
/// may itself contain spaces and parentheses.
#[must_use]
pub fn cpu_ticks_from_stat(line: &str) -> Option<u64> {
    let (_, rest) = line.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (state); utime and stime are fields 14 and 15.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    utime.checked_add(stime)
}

/// The `Pss:` line of a Linux `smaps_rollup`, in bytes.
///
/// Parsed rather than approximated from `statm`, which reports resident pages — exactly the
/// figure D-16 rejects, because it counts every shared page once per process.
#[must_use]
pub fn pss_from_smaps_rollup(text: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("Pss:")?;
        let mut words = rest.split_whitespace();
        let value: u64 = words.next()?.parse().ok()?;
        match words.next() {
            Some("kB") => value.checked_mul(1024),
            _ => None,
        }
    })
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
    fn pss_is_read_from_its_own_line_and_not_from_rss() {
        // The rollup lists Rss first. Reading the first number in the file would be the
        // double-counting figure D-16 rejects, and would look entirely plausible.
        let rollup = "00400000-7ffc0000 ---p 00000000 00:00 0    [rollup]\n\
                      Rss:              123456 kB\n\
                      Pss:               45678 kB\n\
                      Pss_Anon:          40000 kB\n\
                      Shared_Clean:       9000 kB\n";
        assert_eq!(pss_from_smaps_rollup(rollup), Some(45678 * 1024));
    }

    #[test]
    fn a_rollup_without_pss_is_a_missing_sample_rather_than_a_zero() {
        assert_eq!(pss_from_smaps_rollup("Rss: 100 kB\n"), None);
        assert_eq!(pss_from_smaps_rollup("Pss: 100 pages\n"), None);
        assert_eq!(pss_from_smaps_rollup(""), None);
    }

    #[test]
    fn cpu_ticks_survive_a_command_name_with_spaces_and_parentheses() {
        let stat = "4242 (sift (soak) x) S 1 4242 4242 0 -1 4194304 100 0 0 0 \
                    37 5 0 0 20 0 1 0 12345 0 0";
        assert_eq!(cpu_ticks_from_stat(stat), Some(42));
        assert_eq!(cpu_ticks_from_stat("4242 (short) S 1"), None);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_platform_reports_cpu_time() {
        assert!(cpu_time().is_some());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_platform_reports_a_footprint() {
        let bytes = footprint().expect("the platform's footprint is readable");
        assert!(bytes > 0);
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

//! Every cache declares itself.
//!
//! `docs/runtime/memory-pressure.md` calls this "the single most load-bearing line in this
//! document", and it is a code-review rule rather than an aspiration:
//!
//! > **Every cache has an explicit byte budget and an eviction policy. No unbounded map
//! > anywhere.**
//!
//! Without it the governor has nothing to release and the residual means nothing.

use sift_subsystem::Subsystem;

/// What a cache must be able to say about itself.
///
/// FR-34's panel shows all six. A cache that cannot answer one of them is a cache the
/// governor cannot reason about and a leak nobody can localise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub name: &'static str,
    pub owner: Subsystem,
    pub live_bytes: u64,
    pub entries: u64,
    /// **Explicit.** A cache with no capacity is the unbounded map the rule forbids.
    pub capacity_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl Report {
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Whether this cache is within its declared budget.
    #[must_use]
    pub const fn within_budget(&self) -> bool {
        self.live_bytes <= self.capacity_bytes
    }

    /// Whether it declares a budget at all.
    #[must_use]
    pub const fn is_bounded(&self) -> bool {
        self.capacity_bytes > 0
    }
}

/// Every declared cache, and what it adds up to.
#[derive(Debug, Default)]
pub struct Registry {
    reports: Vec<Report>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(&mut self, report: Report) {
        self.reports.retain(|r| r.name != report.name);
        self.reports.push(report);
    }

    #[must_use]
    pub fn total_declared(&self) -> u64 {
        self.reports.iter().map(|r| r.live_bytes).sum()
    }

    /// The number the slope gate sends a maintainer to look at.
    ///
    /// **Footprint minus the sum of declared caches.** A large residual means memory is
    /// being held somewhere nothing declared, which is exactly what the allocator hook exists
    /// to make findable.
    #[must_use]
    pub fn residual(&self, footprint: u64) -> i64 {
        i64::try_from(footprint).unwrap_or(i64::MAX)
            - i64::try_from(self.total_declared()).unwrap_or(i64::MAX)
    }

    /// Any cache that declared no budget.
    ///
    /// The rule is absolute, so this returning anything at all is a defect rather than a
    /// warning.
    #[must_use]
    pub fn unbounded(&self) -> Vec<&'static str> {
        self.reports
            .iter()
            .filter(|r| !r.is_bounded())
            .map(|r| r.name)
            .collect()
    }

    /// Any cache over its own budget.
    #[must_use]
    pub fn over_budget(&self) -> Vec<&'static str> {
        self.reports
            .iter()
            .filter(|r| !r.within_budget())
            .map(|r| r.name)
            .collect()
    }

    #[must_use]
    pub fn reports(&self) -> &[Report] {
        &self.reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(name: &'static str, live: u64, capacity: u64) -> Report {
        Report {
            name,
            owner: Subsystem::Broker,
            live_bytes: live,
            entries: 1,
            capacity_bytes: capacity,
            hits: 3,
            misses: 1,
            evictions: 0,
        }
    }

    #[test]
    fn a_cache_with_no_budget_is_reported_as_a_defect() {
        // "No unbounded map anywhere" is absolute, so this returning anything is a defect
        // rather than a warning.
        let mut r = Registry::new();
        r.declare(cache("classification", 100, 0));
        assert_eq!(r.unbounded(), vec!["classification"]);
    }

    #[test]
    fn the_residual_is_footprint_minus_what_was_declared() {
        // The number the slope gate sends a maintainer to look at.
        let mut r = Registry::new();
        r.declare(cache("a", 100, 1000));
        r.declare(cache("b", 200, 1000));
        assert_eq!(r.residual(1000), 700);
    }

    #[test]
    fn a_negative_residual_means_the_accounting_is_wrong() {
        // Overlap in the subsystem partition is what makes this happen, which is why the
        // partition must be non-overlapping.
        let mut r = Registry::new();
        r.declare(cache("a", 1000, 2000));
        assert!(r.residual(500) < 0);
    }

    #[test]
    fn a_cache_over_its_own_budget_is_named() {
        let mut r = Registry::new();
        r.declare(cache("over", 2000, 1000));
        r.declare(cache("fine", 500, 1000));
        assert_eq!(r.over_budget(), vec!["over"]);
    }

    #[test]
    fn redeclaring_replaces_rather_than_duplicates() {
        // A cache counted twice inflates the declared total and shrinks the residual, which
        // hides the leak the residual exists to find.
        let mut r = Registry::new();
        r.declare(cache("a", 100, 1000));
        r.declare(cache("a", 300, 1000));
        assert_eq!(r.total_declared(), 300);
        assert_eq!(r.reports().len(), 1);
    }

    #[test]
    fn hit_rate_is_defined_for_a_cache_nobody_has_used() {
        let r = Report {
            hits: 0,
            misses: 0,
            ..cache("cold", 0, 100)
        };
        assert!((r.hit_rate() - 0.0).abs() < f64::EPSILON);
    }
}

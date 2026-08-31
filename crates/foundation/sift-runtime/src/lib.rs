//! D-19, D-47, D-92 — the runtime, panic containment, and the stage boundary.
//!
//! # One runtime, plus a blocking pool
//!
//! D-19 chooses a single multi-threaded work-stealing runtime for concurrent work, **plus a
//! separate blocking pool** for database and filesystem calls. The alternatives — a
//! current-thread runtime pinned per subsystem, or a thread per connection — were rejected,
//! and the choice has one consequence that reaches everywhere: because a task migrates
//! between threads, **D-24's subsystem tag must be task-scoped and re-established at every
//! poll**, never a bare thread-local.
//!
//! The runtime's own timer is used **only for short I/O timeouts**. All periodic work goes
//! through D-25's wheel, because NFR-11 depends on coalescing and the runtime's timer has no
//! way to express "this may fire late, batch it".

pub mod pipeline;

/// Which pool work belongs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// The work-stealing runtime.
    Async,
    /// Database calls, filesystem calls — **and the rendering pipeline**.
    ///
    /// D-92 generalises the rule: **"work that will not yield promptly goes to the pool,
    /// whether or not it is I/O."** NFR-41's 30 ms is *uninterrupted CPU* rather than
    /// waiting, and a 30 ms non-yielding task on a runtime worker occupies exactly what
    /// NFR-7's 16 ms feedback and NFR-6's frame budget are competing for. Work stealing does
    /// not help, because there is nothing to steal from a task that never yields.
    Blocking,
}

/// Where a piece of work belongs.
#[must_use]
pub const fn pool_for(yields_promptly: bool) -> Pool {
    if yields_promptly {
        Pool::Async
    } else {
        Pool::Blocking
    }
}

/// Whether the runtime's timer is used for periodic work.
///
/// **No.** It schedules at millisecond granularity with no "this may fire late, batch it"
/// hint — and that hint is the entire mechanism by which wakeups coalesce.
#[must_use]
pub const fn runtime_timer_used_for_periodic_work() -> bool {
    false
}

/// D-47 — the release binary unwinds.
///
/// Abort-on-panic was rejected because the pipeline can catch a panic at a stage boundary and
/// degrade **that message** to FR-9's raw view, which aborting cannot. NFR-19 requires hostile
/// MIME never crash the process, and under D-2 that process holds every account's sync state,
/// the mutation queue and the shell — so aborting is the expensive choice here rather than
/// the conservative one.
#[must_use]
pub const fn panics_unwind() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_that_does_not_yield_goes_to_the_blocking_pool() {
        // Not because it is I/O — because it will not yield. A 30 ms non-yielding task on a
        // runtime worker occupies what NFR-7 and NFR-6 are competing for.
        assert_eq!(pool_for(false), Pool::Blocking);
        assert_eq!(pool_for(true), Pool::Async);
    }

    #[test]
    fn periodic_work_does_not_use_the_runtimes_timer() {
        assert!(!runtime_timer_used_for_periodic_work());
    }

    #[test]
    fn the_release_binary_unwinds() {
        // Aborting cannot degrade one message to the raw view, which is the whole of what
        // D-47 buys.
        assert!(panics_unwind());
        // Checked at compile time, because it is a property of the build rather than of a
        // run — and a run cannot observe an abort in order to report it.
        const { assert!(!cfg!(panic = "abort"), "this build aborts on panic") };
    }
}

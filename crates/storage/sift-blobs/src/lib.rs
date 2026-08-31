//! D-77, D-102, D-57 — the content-addressed blob store, and its eviction.
//!
//! # The cache is a tier, not the truth
//!
//! The provider is the source of truth. **Anything cached may be discarded at any moment
//! and the application must remain correct** — which is what licenses aggressive shedding
//! under pressure, encryption that refuses rather than repairs, and D-32's
//! removal-and-resync path.
//!
//! D-73 draws the consequence honestly: there is **no offline floor**. Bodies are cached
//! because a user read them, plus opportunistic prefetch on an unmetered network, and Sift
//! promises no quantity of mail readable without a network. "Read it before you lose signal"
//! is a workflow users should not have to know, and a small floor would cover most of the
//! disappointment cheaply — but any floor makes the cache undiscardable, which is the
//! property everything above depends on.
//!
//! Snappiness does not come from the body cache: list, search and triage are local reads off
//! **envelopes**, and an uncached body is budgeted at NFR-4's 600 ms.

pub mod evict;
pub mod store;

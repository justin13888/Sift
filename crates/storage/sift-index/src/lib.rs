//! D-5, D-79, D-80, D-81 — the full-text index.
//!
//! # Content-storing, and why the alternatives are foreclosed
//!
//! D-80 chooses an index that holds **its own copy** of the text it indexes, over the two
//! cheaper shapes, for a reason that only appears when eviction is considered: **after a
//! body is evicted the index is the only copy of that text on the machine.**
//!
//! An external-content index references a row that will not be there. A contentless index
//! stores postings only, so reindexing becomes impossible — and D-32 forbids rebuilding
//! derived state while NFR-18 forbids requiring a resynchronization, which between them
//! leave no way to recover. Snippets and highlighting lose their source, and the index
//! degrades silently.
//!
//! The cost is that body text is stored twice while the body is cached — under **different
//! budgets**, NFR-14 for the body and NFR-52 for the index, which is what stops a large
//! attachment evicting a year of searchable text. The retreat, if the cost proves too high,
//! is to index *less* body text per message; never an external-content index.

pub mod ingest;
pub mod merge;
pub mod query;

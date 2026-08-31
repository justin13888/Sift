//! The layer table of D-59, and the edges it names rather than infers.

/// The layers, ordered bottom-up. A crate's layer is its directory under `crates/`.
///
/// "Each may depend on those below and nothing above." The index in this array *is*
/// the ordering, so a dependency is legal only when the depended-on layer's index is
/// less than or equal to the depending layer's.
pub(crate) const LAYERS: &[&str] = &[
    "foundation",
    "storage",
    "rendering",
    "providers",
    "application",
    "presentation",
    "abi",
];

/// The four places `docs/architecture/overview.md` permits unsafe code. Everywhere
/// else it is refused rather than reviewed, and the refusal is a crate-level lint.
pub(crate) const UNSAFE_PERMITTED: &[&str] = &[
    "sift-abi",    // 1. the C ABI of D-17
    "sift-alloc",  // 2. the tagging global allocator of D-24
    "sift-crypto", // 3. the page-level encryption layer of D-42
    "sift-store",  // 4. the database engine's FFI under D-21
];

/// The provider adapters. D-59 names this edge rather than leaving it inferred: each
/// adapter is its own crate and *they may not see each other*. That is what makes
/// D-12's no-special-casing rule mechanical — an adapter's fitness is tested by
/// whether it compiles against the capability crate alone.
pub(crate) const ADAPTERS: &[&str] = &["sift-jmap", "sift-graph", "sift-gmail", "sift-imap"];

/// Prohibitions the layer ordering alone does not express, quoted from the layer table.
///
/// `(layer, forbidden layer, the words the table uses)`
pub(crate) const FORBIDDEN: &[(&str, &str, &str)] = &[
    // "Rendering ... may not reach: the store, the network, the adapters."
    // The broker is the one component in this layer with an edge outward, and it
    // expresses that edge as a trait it defines and the application layer implements.
    (
        "rendering",
        "storage",
        "the rendering layer may not reach the store",
    ),
    // "Providers ... may not reach: each other, the presentation layer, the store's
    // internals."
    (
        "providers",
        "storage",
        "an adapter may not reach the store's internals",
    ),
];

pub(crate) fn rank(layer: &str) -> Option<usize> {
    LAYERS.iter().position(|l| *l == layer)
}

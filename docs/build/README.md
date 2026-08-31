# Build and verification

How the specification becomes a repository, a binary, and evidence that the binary honours it.

**This area is normative, and it is where the "no implementation detail" rule stops.**
[docs/README](../README.md) forbids function signatures, schemas as data-definition language, and pinned
dependency versions, and it names no home for the decisions an engineer must nonetheless make before the
first commit: what the crates are, what may depend on what, how the Swift shell reaches the C ABI, which
machine runs which gate, and what a gate passing means. Those are architectural, they are expensive to
reverse, and leaving them unowned meant they would be chosen by whoever typed first.

The rule this area follows is the set's own: a dependency is named **where the choice is the decision**,
with its justification, exactly as [D-21](../storage/data-model.md) names SQLite. A version is never
pinned here. A tool invocation is never written here.

| Document | Contents |
|---|---|
| [Workspace](workspace.md) | The crate decomposition, the one-way dependency rule, unsafe confinement, feature flags, and the dependency policy |
| [Packaging](packaging.md) | How a Rust core and a Swift shell become one bundle, the version scheme, the channels, and how far back support reaches |
| [Verification](verification.md) | The gates, the machines that run them, what each pass condition is, and the harnesses that cannot be retrofitted |

## Related

- [Architecture overview](../architecture/overview.md) — D-8's Rust core, and the unsafe and dependency
  policies this area makes concrete
- [Shell boundary](../architecture/shell-boundary.md) and [view protocol](../architecture/view-protocol.md)
  — the ABI whose Swift side [workspace](workspace.md) generates
- [Platform baseline](../product/platform-baseline.md) — the identifiers and entitlements
  [packaging](packaging.md) spends
- [Reference environment](../product/reference-environment.md) — the rig and corpora
  [verification](verification.md) runs against

# Workspace

The crate decomposition, what may depend on what, and the policy every dependency is admitted under.

**Owns:** D-59, D-60.

## Why the code partition is not the subsystem partition

[Observability](../runtime/observability.md) enumerates sixteen subsystems and freezes the list, because
NFR-45's slope history is keyed on those names. **That partition is an allocation-attribution partition
and MUST NOT be read as a module layout.** It answers "which component's bytes are these", which is a
runtime question; a crate answers "what may this code reach", which is a compile-time one. The two
overlap and are not the same, and building one from the other produces sixteen crates whose boundaries
are drawn where memory happens to be charged rather than where dependencies need cutting.

## D-59 — A one-way crate graph, with the ABI as a leaf

**Chosen:** the core is a Cargo workspace of layered crates whose dependency edges all point one way,
from the shell boundary down toward the store; the C ABI is a leaf crate that nothing in the core depends
on, and no crate below the presentation layer may depend on anything above it.
**Rejected:** a single core crate with module-level layering; a crate per observability subsystem.

**Why a graph rather than modules.** Every structural claim in this set is a statement about what may
reach what — *"the shells MUST NOT be given a path to the store, the index, or a provider adapter"*
([shell boundary](../architecture/shell-boundary.md)), *"a `match` on provider identity anywhere above
the adapter layer is a defect"* ([provider model](../mail/provider-model.md)), *"no provider names above
the adapter layer"*. Inside one crate every one of those is a review convention, enforced by whoever
notices. Across crates they are compile errors. A design whose central properties are reachability
properties should be built out of the unit that enforces reachability, and this is the cheapest place the
project will ever be to draw those lines.

**The layers, and the direction.** Each may depend on those below it and on nothing above:

| Layer | Holds | May not reach |
|---|---|---|
| **ABI** | the C entry points of [D-17](../architecture/shell-boundary.md), and nothing else | — it is a leaf; no core crate depends on it |
| **Presentation** | [D-18](../architecture/presentation-layer.md)'s windowing, selection, formatting, merge and diff | provider adapters, the engine, any widget toolkit |
| **Application** | sync, the mutation queue, scheduling, the pressure governor, the credential broker | the ABI, any shell |
| **Providers** | the capability abstraction, and one crate per adapter | each other, the presentation layer, the store's internals |
| **Rendering** | MIME parse, sanitizer, blocker, dark transform, resource broker | the store, the network, the adapters |
| **Storage** | the store, blobs, the index, the encryption layer | everything above |
| **Foundation** | limits, identifiers, the state register, the error states | everything above |

Three edges are load-bearing enough to name rather than infer.

**Each provider adapter is its own crate, and they may not see each other.** This is what makes
[D-12](../mail/provider-model.md)'s no-special-casing rule mechanical: a Gmail behaviour cannot leak into
the IMAP adapter because there is no edge for it to travel along, and the abstraction's fitness is tested
by whether an adapter compiles against the capability crate alone.

**The rendering layer may not reach the network or the store.** [Pipeline](../rendering/pipeline.md)
stages take bytes and return trees; the [resource broker](../architecture/resource-broker.md) is the one
component that answers a fetch, and it is the only place in that layer with an edge outward. Nothing else
in a hostile-input path gets to open a socket or read a file, and the crate graph is what says so.

**The ABI is a leaf that nothing in the core depends on.** D-17 requires the ABI to be *the* contract —
*"anything a shell is permitted to use MUST be expressible across the C ABI"* — and the failure it names
is quiet: the GTK shell links Rust directly, reaches presentation API the ABI never exposed, and the two
shells drift. Making the ABI a leaf does not by itself stop that; what it does is make the ABI's surface
readable in one place, which is the tripwire D-17 asks for and could not evaluate.

**What it costs:** more crates than a reader would choose for a project this size, a workspace whose
build graph must be kept acyclic deliberately, and the friction of moving a type across a boundary when
the first design put it on the wrong side. Some of that friction is the point.

**Contestable because:** crate boundaries are the most expensive refactor in Rust, and this draws several
of them before any code has argued for them. If the layers above turn out to be one cohesive thing that
is always changed together, the graph will have bought compile-time enforcement of a boundary nobody was
tempted to cross, at the cost of every change touching four manifests.

## Unsafe code is confined by the crate graph, not by review

[Overview](../architecture/overview.md) confines unsafe code to four places — the C ABI, the tagging
global allocator, the page-encryption layer, and the database engine's foreign-function interface — and
says it is *"refused rather than reviewed"* everywhere else, *"because a policy that permits it anywhere
it seems justified is not a policy"*.

**That policy MUST be expressed as a crate-level prohibition in every crate but those four**, from the
first commit. A policy enforced by review is a policy that holds until a reviewer is busy, and the four
exceptions are already crate-shaped: each is a distinct component named in a distinct document. Anything
that later needs unsafe code somewhere else is asking for a fifth exception, and the value of writing it
this way is that it has to ask.

## Feature flags are a register, not a habit

[D-24](../runtime/observability.md) puts allocation stack capture behind a flag, and a flag introduced
without a register is how a build configuration nobody tests becomes shippable.

**Every conditional compilation flag MUST be recorded in this document with its default and what it costs
when enabled, and the release configuration MUST be one named set rather than an accumulation.** The
reason is NFR-44: attribution overhead is bounded *in release*, so "release" has to name a determinate
build. A target measured under one flag set and shipped under another has not been measured.

| Flag | Default | Effect |
|---|---|---|
| Allocation stack capture | off in release | [D-24](../runtime/observability.md)'s per-allocation backtraces. On, it exceeds NFR-44's 2% and is a debugging tool rather than a shipping one |
| Fault injection | off, and absent from release | The kill points and write-interception [verification](verification.md) requires for NFR-16. It MUST NOT be compilable into a shipped binary |

## Toolchain and edition

**The minimum supported toolchain is recorded once, in the workspace manifest, and raising it is a
deliberate change rather than a consequence of a dependency update.** No version is pinned in this
document, for the reason [docs/README](../README.md) gives; what is normative is that the floor exists,
that it is stated in one place, and that a build failing on the stated floor is a defect rather than an
invitation to raise it.

The same rule reaches vendored dependencies. [Overview](../architecture/overview.md) requires vendoring
because *"a Flatpak build has no network, so every crate must be present as a declared source"*, which
means a dependency's toolchain floor becomes Sift's on the day it is vendored.

## D-60 — The Swift declarations are generated from the ABI crate, and drift fails the build

**Chosen:** a C header is generated from the ABI crate's source and consumed by Swift through a module
map; the generated artefact is committed, and regenerating it in CI and finding a difference fails the
build.
**Rejected:** hand-written Swift declarations kept in step by review; a binding generator that owns the
boundary's shape rather than describing it.

**Why generated.** D-17 says the macOS shell *"binds it through generated Swift declarations"* and does
not say by what, which leaves the most dangerous file in the project — a second, independent statement of
a memory layout, in another language, at the one boundary
[shell boundary](../architecture/shell-boundary.md) concedes memory-safety bugs are possible — to be
maintained by hand. A hand-written declaration that disagrees with the Rust type does not fail to
compile. It miscompiles: it reads the wrong offset, and it does so at a boundary carrying
attacker-derived strings under NFR-54.

**Why committed, and why the drift check rather than generation-on-build.** Generating during the build
makes the Swift target depend on a working Rust toolchain at every step of a build a shell engineer runs
dozens of times, and it hides the boundary's growth: D-17's own tripwire is *"if the ABI surface grows
past what one file can hold, that is the signal this was the wrong shape"*, and a surface nobody ever
sees in a diff cannot trip it. Committing the header makes every addition to the boundary visible in
review, and the CI check makes it impossible for the committed copy to be stale. This is the discipline
[D-13](../mail/provider-model.md) already chose for hand-written Graph types — *"CI MUST diff the
hand-written types against the current published schema"* — applied to the boundary rather than to a
provider.

**Why a C header and a module map rather than a binding framework.** D-17 chose a C ABI as *"the
narrowest waist that Swift and Rust both speak natively"* and rejected *"per-language bindings generated
from a schema"*. A framework that owns the boundary would reintroduce exactly what that decision
declined, and would do it by making the ABI a generated consequence of Rust types rather than a written
contract — which is the opposite of [view protocol](../architecture/view-protocol.md)'s requirement that
the contract be stated and honoured.

**What it costs:** a generated artefact in version control, which reviewers must learn to read as output
rather than source, and a CI job that fails for a reason a first-time contributor will misread as a
spurious diff.

**Contestable because:** committing generated output is widely disliked and the alternative — generate,
never commit, and let the compiler catch nothing — is simpler right up to the first silent layout
mismatch. The bet is that the visibility of the boundary is worth more than the tidiness of the tree, and
that bet is exactly D-17's tripwire being made operable rather than a general preference about generated
files.

## Dependency policy

[Overview](../architecture/overview.md) states the policy in one sentence — *"dependencies are vendored
and vetted"* — and justifies only the vendoring. **Vetting is defined here, because a named obligation
with no mechanism is not one.**

**Three gates, and each MUST fail the build rather than warn.**

1. **Advisories.** The vendored tree is checked against a published advisory database. A firing advisory
   blocks the build. It is not muted; it is either upgraded, replaced, or — where neither is possible —
   recorded as an accepted exception with an expiry date, in this repository, where it is visible.
2. **Licences.** Every vendored crate's licence is checked against an allowlist. This is not
   housekeeping: **a single copyleft crate defeats the App Store channel exactly as
   [Q-17](../open-questions.md)'s copyleft filter list would**, even with a contributor agreement fully
   in place — which [D-113](../product/platforms-and-distribution.md) defers with the channel — because
   no agreement can relicense somebody else's work. Q-16
   is about Sift's own copyright and Q-17 about the artefacts Sift bundles; the dependency tree is a
   third population, two orders of magnitude larger than either, and [R-12](../open-questions.md) stops
   precisely at its edge — *"it counts the components this project builds and cannot count the ones it
   imports"*.
3. **New edges.** A dependency added to the workspace, or a transitive one that appears with an upgrade,
   is reviewed rather than absorbed. The threshold is deliberately low, because
   [threat model](../security/threat-model.md)'s primary adversary chooses the input this code parses,
   and a supply-chain compromise reaches that path with none of the defences the design spends on the
   sender.

**A crate on a hostile-input path is held to the unsafe rule above.** Sift refusing unsafe code
everywhere but four places buys nothing if a MIME or CSS dependency is a thin wrapper over unsafe
parsing, so that is a question the third gate asks rather than one the policy assumes away.

## Related

- [Architecture overview](../architecture/overview.md) — D-8, D-19, and the unsafe and vendoring policies
  this document makes enforceable
- [Shell boundary](../architecture/shell-boundary.md) — D-17, the contract D-60 generates the Swift side
  of
- [Packaging](packaging.md) — what the workspace is built into
- [Verification](verification.md) — where the three dependency gates run

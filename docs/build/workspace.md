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
   housekeeping: **a single crate whose licence reaches the whole binary defeats the App Store channel
   exactly as [Q-17](../open-questions.md)'s copyleft filter list would**, with [Q-16](../open-questions.md)'s
   contributor agreement fully in place, because no agreement can relicense somebody else's work. Q-16
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

### The licence allowlist

This answers [Q-21](../open-questions.md), which asked whether the dependency tree would be audited before
the first release or would make the channel decision by whatever happened to be vendored. **Neither: the
tree is audited on every change, from the first commit**, because the licence gate runs in the same sweep
as every other gate and fails the build rather than warning. There is no release-time audit to schedule,
because there is no moment at which an unaudited crate is in the tree.

**The test a licence has to pass is whether it constrains the terms of the work it is linked into.** That
is the property that collides with the App Store's terms, and "copyleft" is the wrong name for it:

- **Admitted — permissive licences:** Apache-2.0 (with or without the LLVM exception), MIT, MIT-0,
  BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CC0-1.0 and the Unlicense. They attach notice
  obligations and nothing else.
- **Admitted — MPL-2.0.** Its copyleft is per file: a larger work containing it may be distributed under
  any terms, provided the covered files' source stays available, and Sift's sources and its
  dependencies' are public already, so the obligation is met rather than newly incurred. It has no
  anti-circumvention clause and no installation-information requirement, which are what make the GPL
  family a problem for the store. **Admitting it is load-bearing:** [D-10](../rendering/content-blocking.md)
  names the filter engine, and that engine is MPL-2.0, so a stricter list would have overruled a decision
  the specification already made.
- **Rejected — GPL, LGPL and AGPL in every version, for crates.** The GPL family's terms reach the whole
  binary, and LGPL's relinking requirement cannot be honoured by a statically linked, sandboxed,
  store-signed application. Native libraries the platform supplies are a separate case, covered below.
  Sift's own crates are AGPL-3.0 and are outside the gate, because they are the copyright
  [Q-16](../open-questions.md) is about rather than somebody else's.
- **A crate offered under a choice of licences passes when any one of them is admitted**, since Sift takes
  it under that one.

**The bar is the strictest channel's, applied to every build.** [D-33](../product/platforms-and-distribution.md)
names three channels, and the App Store's terms are the strictest of them; holding every build to that
bar means no channel decision ever waits on re-auditing the tree, and every channel's build of one
platform carries the same crates. The crate graph does differ between platforms, because each platform's
shell and its platform bindings are that platform's alone, and the gate holds both graphs to the same bar.
Bundled artefacts differ by channel under [D-112](../product/platforms-and-distribution.md); linked code
does not, because one source tree per platform is simpler to reason about than a crate graph that
varies by channel, and the permissive-plus-MPL set has so far cost nothing to hold everywhere.

**A crate the gate rejects is replaced or not taken.** The allowlist does not grow to fit a dependency:
adding a licence to it is an amendment to this section, argued here against the test above, and the
gate's configuration follows this document rather than the other way round. The notes kept beside the
reviewed edge list the third gate keeps record the reasoning for the crates whose review had a
non-obvious answer, licence included. The MPL-2.0 case was first met there, when the filter engine
tripped the gate, and was decided in the gate's configuration. This section now states that decision as
policy.

**The gate's scope is the crate graph compiled into Sift's binary, and native libraries the platform
supplies are outside it.** The Linux shell links GTK4, libadwaita and WebKitGTK, which are LGPL. They are
shared libraries supplied by the pinned Flatpak runtime ([D-15](../product/platforms-and-distribution.md)),
and are never vendored or linked statically. The LGPL rejection above rests on relinking. A shared
library the runtime supplies can be replaced without relinking Sift, so the objection does not apply,
and none of these libraries reaches a macOS channel. The same reasoning covers the system frameworks the
macOS shell links. The Rust bindings to these libraries are crates, and the gate checks them like any
other. A native library that Sift itself vendors, builds, or links statically is not covered by this
exclusion. It would have to pass the test above like a crate.

## Related

- [Architecture overview](../architecture/overview.md) — D-8, D-19, and the unsafe and vendoring policies
  this document makes enforceable
- [Shell boundary](../architecture/shell-boundary.md) — D-17, the contract D-60 generates the Swift side
  of
- [Packaging](packaging.md) — what the workspace is built into
- [Verification](verification.md) — where the three dependency gates run

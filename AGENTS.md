# AGENTS.md

Operating instructions for agents and contributors working in this repository.

## What this repository is

**Sift** — a desktop mail client for reading, searching, and triaging mail across multiple accounts
(Gmail, Microsoft 365 / Outlook.com, JMAP, generic IMAP), on **macOS first and Linux second**. It is
designed to run all the time, so idle resource cost ranks above feature breadth.

The specification lives in **[`docs/`](docs/README.md)** and is normative. Start at
[`docs/README.md`](docs/README.md).

## Hard constraints

Violating any of these is a defect regardless of what a task appears to ask for. If a request requires
breaking one, stop and say so.

| Constraint | Where it is specified |
|---|---|
| **No send path.** No SMTP, JMAP submission, or provider send call may exist anywhere in any shipped binary | [scope](docs/product/scope.md) |
| **The body view has no network capability.** Only the internal scheme is registered; every other scheme is rejected at the engine policy layer. All resource loading routes through the broker (invariant N-1) | [webview isolation](docs/rendering/webview-isolation.md) |
| **No JavaScript in message bodies**, disabled at the engine level, not by sanitization | [webview isolation](docs/rendering/webview-isolation.md) |
| **No unbounded cache.** Every cache has an explicit byte budget, an eviction policy, and reports its live size | [memory pressure](docs/runtime/memory-pressure.md) |
| **No per-account or per-folder sleep loops.** All periodic work goes through the single coalesced scheduler | [scheduling](docs/runtime/scheduling.md) |
| **Credentials only in the OS credential store** — never in a database, log, or crash dump | [credentials](docs/security/credentials.md) |
| **Never a listening socket of any kind, for any purpose** — not TCP, not Unix domain, not abstract. The shell boundary is a function call, and OAuth returns through a registered URI scheme | [shell boundary](docs/architecture/shell-boundary.md) |
| **No provider names above the adapter layer.** Everything plans against declared capabilities | [provider model](docs/mail/provider-model.md) |
| **No garbage-collected runtime in the process.** Sift is one resident process; a GC's floor and pause jitter attack the idle-footprint target directly | [overview](docs/architecture/overview.md) |
| **Identifiers (FR, NFR, D, I) are stable** and are never renumbered or reused | [docs/README](docs/README.md) |

## Where to look

| Question | Document |
|---|---|
| What is in and out of scope? | [product/scope](docs/product/scope.md) |
| Why one process? Why a native shell? | [architecture/overview](docs/architecture/overview.md), [process-model](docs/architecture/process-model.md), [ui-shell](docs/architecture/ui-shell.md) |
| How do four providers become one abstraction? | [mail/provider-model](docs/mail/provider-model.md) |
| How does a specific provider behave? | [mail/providers/](docs/mail/providers/) |
| How are archive/delete/move applied and undone? | [mail/mutations](docs/mail/mutations.md) |
| What is stored, and for how long? | [storage/data-model](docs/storage/data-model.md), [cache-and-blobs](docs/storage/cache-and-blobs.md) |
| How is hostile HTML handled? | [rendering/pipeline](docs/rendering/pipeline.md), [sanitizer-invariants](docs/rendering/sanitizer-invariants.md) |
| What loads a message's images, and how is that decided? | [architecture/resource-broker](docs/architecture/resource-broker.md) |
| Is X out of scope, or just not built yet? | [product/scope](docs/product/scope.md) — non-goals versus deferred-with-a-seam |
| What are the memory, CPU, and network budgets? | [requirements](docs/requirements.md) |
| Why was X chosen over Y? | [decisions](docs/decisions.md) |
| What is still undecided? | [open-questions](docs/open-questions.md) |
| What does this term mean here? | [glossary](docs/glossary.md) |

## Working rules

**Specifications carry no implementation detail.** Documents under `docs/` state *what* and *why*. They
contain no code, no schema definitions, and no pinned versions. Keep it that way when editing them — a
named dependency belongs there only when the choice **is** the architectural decision.

**Changing a decision means changing two places.** Amend the owning document *and* its row in
[`docs/decisions.md`](docs/decisions.md). Leaving the two disagreeing is a defect. The same applies to
requirements and [`docs/requirements.md`](docs/requirements.md).

**Never resolve an open question by deleting it.** Write the answer into the owning document, then strike
the entry in [`docs/open-questions.md`](docs/open-questions.md) with a pointer to the answer.

**Every number in `docs/` is a hypothesis**, to be validated against the
[reference environment](docs/product/reference-environment.md). Do not cite one as a measured fact, and do
not adjust one to match a measurement without saying that is what happened.

**Reference requirements by identifier** in commits, reviews, and tests. That is what they are for.

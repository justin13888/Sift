# Architecture overview

The one structural claim everything else follows from, and the map of the layers that implement it.

**Owns:** D-8, D-19, D-47.

## The central claim

"Always running in the background" and "low idle footprint" are simultaneously satisfiable only if the
web engine is a **disposable resource** rather than a permanent one — created when a message is read and
destroyed when none is.

Three questions are commonly conflated. Separating them dissolves most of the tension:

| Question | Answer |
|---|---|
| Does *message-body rendering* need a browser engine? | **Yes, unavoidably.** Modern HTML email is table-soup with inline CSS, media queries, web fonts, and increasingly flex and grid. Anything short of a real engine renders visibly broken mail. |
| Does the *always-on* part need a browser engine? | **No.** Sync, storage, indexing, notification, and mutation queueing are pure Rust. |
| Does the *app chrome* need a browser engine? | **No** — see [D-1](ui-shell.md). This is a velocity-versus-footprint trade, not a correctness one. |

The engine is confined to one killable, respawnable surface. Because WebKit is itself multi-process, that
surface is genuinely out-of-process on both target platforms, and destroying it returns its memory to the
operating system — see [webview isolation](../rendering/webview-isolation.md). **This is why Sift does not
need a process split of its own to reclaim the engine**, which is the reasoning behind
[D-2](process-model.md).

## Layers

```
┌──────────────────────────────────────────────────────────────┐
│  Sift — one resident process                                 │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │ Shell — AppKit / GTK4, created and destroyed with the  │  │
│  │ window. No web engine. List, sidebar, reader chrome.   │  │
│  └────────────────────────────────────────────────────────┘  │
│                      ▲ in-process, C ABI                     │
│  ┌───────────────────▼────────────────────────────────────┐  │
│  │ Presentation layer — windowing, selection, sort,       │  │
│  │ formatting, result assembly                            │  │
│  ├───────────┬────────────┬──────────┬────────────────────┤  │
│  │ Provider  │ Sync       │ Store    │ Index              │  │
│  │ adapters  │ + schedule │ + blobs  │                    │  │
│  ├───────────┴─┬──────────┴──┬───────┴────────────────────┤  │
│  │ Mutation    │ Credential  │ Pressure governor          │  │
│  │ queue       │ broker      │                            │  │
│  ├─────────────┴──┬──────────┴───┬────────────────────────┤  │
│  │ Resource broker│ Sanitizer    │ Content blocker        │  │
│  └────────────────┴──────────────┴────────────────────────┘  │
└───────────────▲──────────────────────────────────────────────┘
                │ engine scheme handler — never a socket
┌───────────────▼──────────────────────────────────────────────┐
│  Body view (WebKit content process) — spawned on demand,     │
│  destroyed on teardown. No JS, no network, isolated store.   │
└──────────────────────────────────────────────────────────────┘
```

Everything above the body view is one address space. The **window shell** is the only part that is
created and destroyed with the window; everything beneath it is resident for the life of the process,
along with the thin **application shell** that owns the always-on surface — see
[UI shell](ui-shell.md), which separates the two lifetimes and says why FR-22 requires it.

The **resource broker** deserves note: every byte the body view loads passes through it. This is what
makes content blocking a decision function in code Sift owns rather than an interception of a fetch path
it does not. It has its own page — see [resource broker](resource-broker.md), and
[content blocking](../rendering/content-blocking.md) and
[webview isolation](../rendering/webview-isolation.md) for the decisions it applies.

## D-8 — Rust for the core

**Chosen:** Rust throughout the core, for everything except the macOS shell.
**Rejected:** Go, C++.

**Why.** The core is long-running, parses attacker-controlled input, and has a hard no-ratchet memory
requirement. Rust gives memory safety on the hostile-input path without a garbage collector — and a GC's
floor and pause jitter attack the idle-footprint target directly. A runtime with a GC MUST NOT be
introduced into the process; this is also why no language shim is used to reach provider SDKs, see
[provider model](../mail/provider-model.md).

**Contestable because:** the Rust GUI ecosystem is immature. That risk is mitigated by not using it — the
shells are AppKit and GTK4, and the shared logic below them is [presentation-layer](presentation-layer.md)
Rust with no widget toolkit in it. Swift is the only non-Rust language in the project.

## D-19 — One work-stealing async runtime

**Chosen:** a single multi-threaded, work-stealing async runtime for all concurrent work, with a separate
blocking pool for database and filesystem calls.
**Rejected:** a current-thread runtime pinned per subsystem; a thread per connection with blocking I/O.

**Why.** Fifteen watched folders across five accounts is dozens of concurrent operations that are almost
always waiting. A thread each would cost a stack each, which attacks the idle footprint target directly.
Work stealing keeps a full-scan resynchronization on one account from starving the others without hand-
balancing, which a per-subsystem pinned runtime would require.

Work stealing does constrain [allocation attribution](../runtime/observability.md): a task may resume on a
different worker thread than it started on, so the subsystem tag MUST be task-scoped and re-established at
each poll, never a bare thread-local set once. Attribution is specified against that constraint rather
than the runtime being chosen around it.

The runtime's own timer is used only for short I/O timeouts. **All periodic work goes through the
scheduler's timing wheel instead** — a general-purpose runtime timer knows nothing about platform wakeup
coalescing, which NFR-11 depends on. See [scheduling](../runtime/scheduling.md).

**Contestable because:** a pinned-per-subsystem runtime would make the thread-local attribution mechanism
sound by construction and give per-subsystem wakeup counts for free. That is a real simplification in the
observability story, traded away for scheduling fairness that has not yet been measured to matter.

## D-47 — Panics unwind, and they are caught at the pipeline's stage boundaries

**Chosen:** the release binary unwinds on panic; the [rendering pipeline](../rendering/pipeline.md)
establishes a catch boundary at each stage, and a caught panic degrades that message to FR-9's raw source
view. No unwind may cross the [C ABI](shell-boundary.md).
**Rejected:** aborting on panic; catching nothing and relying on the code being correct.

**Why this is a requirement rather than a build setting.** NFR-19 says a malformed or hostile message
"MUST NEVER crash the process", and the process in question holds every account's sync state, the mutation
queue, and credential material read out of the OS store to be used. Aborting on panic makes that
requirement **unachievable by construction**: a single slice index or arithmetic overflow anywhere in the
MIME parser, the sanitizer or the [cascade](../rendering/dark-mode.md) terminates the resident
application, and under [D-2](process-model.md) it takes every open window with it. The sender chooses the
input and pays nothing per attempt, so that is a remote denial of service on the user's mail available to
anyone who can send them a message.

Rust's memory safety is what makes the hostile-input path defensible under D-8, and it is not what makes
it *total*: a panic is the safe behaviour of a bounds check, not the absence of one. The panic is the
mechanism working. NFR-19 is a statement about what happens next, and without a catch boundary the answer
is "the process dies", which is the outcome the requirement names.

**The boundary is per stage rather than per process** because that is the granularity at which the state
is discardable. A stage's inputs are its bytes and its output is a fresh tree or document; abandoning one
loses a message, and the pipeline already has somewhere to put that message. Nothing upstream of stage 1
is invalidated, which is what makes the recovery honest rather than a caught panic that resumes into
unknown state.

**Nothing may unwind across the ABI.** An unwind through an `extern "C"` frame is undefined behaviour, and
[shell boundary](shell-boundary.md) already concedes this is the one place in the core where memory-safety
bugs are possible. Every exported entry point terminates unwinding and returns a failure the shell can
render, which is the same obligation the states-not-strings rule in
[presentation layer](presentation-layer.md) places on every other value crossing that line.

**What it costs:** a larger binary, unwinding tables, and a catch boundary that must be established and
tested at every stage rather than assumed. It also creates a category the code has to take seriously — a
caught panic is a defect that reached production, so it MUST be counted per subsystem under
[observability](../runtime/observability.md) and MUST NOT be silently absorbed as an ordinary parse
failure. A pipeline that quietly degrades a thousand messages a day is failing, not coping.

**Contestable because:** catching panics is widely and correctly regarded as a poor substitute for not
having them, and a caught panic leaves the offending stage's allocations to the allocator rather than to a
tidy teardown, which touches NFR-12's no-ratchet property in a way nothing has measured. If the fidelity
corpus and NFR-40's fuzzing drive the panic rate to zero, this decision buys only insurance — but it is
insurance against the one failure the product's central claim is about.

## Unsafe code and dependencies

Two policies follow from D-47 and D-8 and are code-review rules of the same kind as
[memory pressure](../runtime/memory-pressure.md)'s no-unbounded-cache line.

**Unsafe code is confined to four places and forbidden elsewhere**, from the first commit rather than as a
later cleanup: the C ABI of [D-17](shell-boundary.md), the tagging global allocator of
[D-24](../runtime/observability.md) together with the process footprint and CPU-time reads that are
measured against it (the system call macOS offers for both is the only route to the footprint the
residual subtracts its counters from), the page-level encryption layer of
[D-42](../storage/encryption.md), and the database engine's foreign-function interface under
[D-21](../storage/data-model.md). Those four are unavoidable and each is named in its own document as
carrying risk. Everywhere else — and in particular everywhere that parses attacker-controlled input —
unsafe code is refused rather than reviewed, because a policy that permits it anywhere it seems justified
is not a policy.

**Dependencies are vendored and vetted.** Vendoring is not optional in any case: a Flatpak build has no
network, so every crate must be present as a declared source, and discovering that late is a build-system
change rather than a configuration one. Vetting is the part that needs stating —
[R-12](../open-questions.md) counts the components this project builds and cannot count the ones it
imports, and a mail client that
parses hostile input is a supply-chain target whether or not anyone plans for it.

## Related

- [Process model](process-model.md) — why one process, and what it costs
- [Shell boundary](shell-boundary.md) — the contract between the core and the shells
- [UI shell](ui-shell.md) — the native shells and D-1
- [Presentation layer](presentation-layer.md) — the shared Rust layer beneath both shells

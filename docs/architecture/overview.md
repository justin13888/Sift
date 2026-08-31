# Architecture overview

The one structural claim everything else follows from, and the map of the layers that implement it.

**Owns:** D-8, D-19.

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

Everything above the body view is one address space. The **shell** is the only part that is created and
destroyed with the window; everything beneath it is resident for the life of the process.

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

## Related

- [Process model](process-model.md) — why one process, and what it costs
- [Shell boundary](shell-boundary.md) — the contract between the core and the shells
- [UI shell](ui-shell.md) — the native shells and D-1
- [Presentation layer](presentation-layer.md) — the shared Rust layer beneath both shells

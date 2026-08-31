# UI shell

**Owns:** D-1, FR-6, FR-22, FR-23, FR-24, NFR-1, NFR-2, NFR-6, NFR-7, NFR-27.

## D-1 — Native shell per platform; a web engine only for message bodies

**Chosen:** Swift and AppKit on macOS, GTK4 and libadwaita on Linux, with a shared Rust
[presentation layer](presentation-layer.md) beneath both. WebKit is used only to render message bodies.
**Rejected:** a web UI hosting the entire interface in a web engine.

**Why — and note that memory is the weakest of the three arguments:**

1. **Idle energy.** A native view hierarchy with nothing invalidated does *nothing* — no wakeups, no draw.
   A web engine with any CSS transition, animation callback, intersection observer, or timer keeps a
   compositor and an event loop alive. A web UI can be disciplined into near-zero idle, but that discipline
   must be maintained forever, across every dependency, and one careless transition on a hover state
   reintroduces the cost. For an app whose primary requirement is background residency on battery, this
   asymmetry outweighs the footprint difference.
2. **Scroll at scale.** Native list-view cell reuse is decades-mature and yields momentum, rubber-banding,
   and live resize for free. Virtualizing 100,000 rows in a web engine is achievable but is a continuous
   fight, and it degrades under precisely the conditions that matter here: memory pressure and background
   throttling.
3. **Memory.** Roughly a 30–40% reduction while the window is open. Meaningful, not order-of-magnitude —
   and a hypothesis to be validated against the [reference environment](../product/reference-environment.md),
   not a measurement.

Native input handling, text composition, RTL and CJK text, system text services, drag and drop, and screen
reader support all work by default in a native toolkit and are a permanent approximation project in a web
UI.

**What it costs:** two shell implementations. That cost is halved by putting list windowing, selection,
sort and filter, formatting, thread collapsing, and search-result assembly in the shared Rust presentation
layer, so the shells bind and lay out rather than decide.

**Contestable because:** it is roughly 2× the UI work, and this document reverses an earlier decision that
went the other way. Revisit only if P0 measurement shows the memory delta under ~50 MB *and* a genuinely
zero-wakeup web UI can be demonstrated.

## The Linux shell is native too

**GTK4 and libadwaita, natively, on Linux.** This was previously left open, to be settled at the end of
P2. [D-2](process-model.md) settles it earlier and in one direction, because a single process changes the
arithmetic: the chrome now lives in the process that must stay resident. A web UI there would hold a web
engine in memory permanently, for the interface, in an application whose central claim is that the engine
is disposable — see [overview](overview.md). The option is not merely worse than it was, it contradicts
the premise.

What remains true is the cost. A second native shell is a second full UI implementation rather than a
recompile, and GTK4's renderer sets the Linux idle floor through the toolkit residue D-2 accepts. Neither
is a reason to host an engine instead.

## Requirements

**FR-6.** The message list MUST be virtualized, showing sender, subject, snippet, date, flags, an
attachment indicator, and thread count. Sorting by date is required; grouping by thread MUST be
toggleable. How an opened thread is then presented — native rows over a single body view — is
[D-54](../rendering/webview-isolation.md), which decides it on isolation and memory grounds rather than
presentational ones.

**FR-22.** A menu-bar or tray presence is the always-on surface, offering at minimum: open, quit, and
pause sync. See [process model](process-model.md) for the quit semantics it must express.

**Windows are plural, and several requirements already assume it.** NFR-42 releases the filter engine when
the *last* window closes, and L3 in [memory pressure](../runtime/memory-pressure.md) destroys the shell
view hierarchy — which is every window, not one. Stating it here rather than leaving it implied matters
because the two read differently on a single-window reading and identically on a multi-window one, and the
resident layers beneath are shared by all of them: a window is a view onto the
[presentation layer](presentation-layer.md), never an instance of it. Anything scoped to "a window" that
is really scoped to "any window open" MUST say the latter.

**FR-23.** Native notifications for new mail, with per-account and per-folder rules and a global quiet
mode.

**FR-24.** Every action MUST be reachable from the keyboard without a pointer, including a command
palette. This is a hard requirement, not a power-user affordance — it is also what makes the app testable
without UI automation.

**NFR-27.** The message list, the reader, and message body content MUST all be navigable and announced by
a screen reader. Body content is rendered in a web engine, so this requirement crosses into the
[rendering pipeline](../rendering/pipeline.md).

## Performance targets

Hypotheses, to be validated against the [reference environment](../product/reference-environment.md).

| ID | Target | Method |
|---|---|---|
| **NFR-1** | Cold start — process launch to interactive list — under 400 ms at p95. Opening a window on an already-resident process is bounded by NFR-2, not this | instrumented trace |
| **NFR-2** | Warm folder or account switch under 50 ms at p95 | frame timing |
| **NFR-6** | List scroll sustains 60 fps, 120 where available; zero dropped frames over a 10,000-row fling | frame capture |
| **NFR-7** | Triage action reflected in the UI within 16 ms, optimistically, before any network round trip. **Permanent delete is excluded**, for the reason below | trace |

NFR-7 is a consequence of the [optimistic mutation model](../mail/mutations.md), not an independent
achievement — and it therefore inherits that model's single exception. Permanent delete is confirmed
before it is issued and is **not** applied optimistically, because there is no compensating intent for
destruction; [mutations](../mail/mutations.md) owns that reasoning. Stated without the exclusion, NFR-7
reads as a promise over every FR-13 intent that one of them cannot keep by construction, and a test
written from this row alone would fail on the one mutation the design deliberately made slow.

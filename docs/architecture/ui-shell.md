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

## The scaffold this decision rejects is abandoned, not merely disfavoured

D-1 rejects a web UI as an *option*. That is not the same as removing the one that exists, and the
distinction cost the project nothing while no code was being written.

**The repository's Tauri, React and Vite application is abandoned outright**, along with the Bun
toolchain and lockfile beside it and the Linux GTK3 development container that was the only declared
environment for a macOS-first product. None of it is a starting point, a reference, or a prototype to
migrate from. It is removed, and the documentation says so here rather than leaving an implementer to
infer it from a rejected alternative.

Saying it explicitly matters because the scaffold is not merely off-strategy — it breaks three things at
once, two of them gates that admit no phase in which they are allowed to fail:

- **D-1**, directly. It is the rejected option, running.
- **NFR-24**, which [shell boundary](shell-boundary.md) makes absolute with no carve-out. The
  development server binds a listening socket, and NFR-24 is a *Standing* gate — true of the first
  commit and every commit after — so a build that starts there is in violation before it does anything.
- **NFR-20**, whose disabled-at-the-engine-level guarantee a null content-security policy contradicts in
  spirit, in the one component the product's central claim is about.

What survives is the bundle identifier, and only because
[platform baseline](../product/platform-baseline.md) has since reserved it deliberately. Nothing else in
the scaffold is inherited.

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
pause sync. See [process model](process-model.md) for the quit semantics it must express, and
[D-58](../runtime/network-conditions.md) for what pausing does — which is defined once, as a policy tier,
so that this requirement and FR-36's cap three phases later cannot mean different things by one word.

**The shell has two lifetimes, and FR-22 is the requirement that proves it.** A menu-bar or tray presence
must exist when no window does — that is what "always-on surface" means, and FR-2 and FR-25 are both
specified in terms of it. But a shell described as *created with a window and destroyed with it* has no
state in which it can hold one, so on the previous reading the tray either does not exist when it is
needed or is held by the core, which would put AppKit in the layer
[presentation layer](presentation-layer.md) requires to have no widget toolkit in it. Neither is
acceptable and the choice between them was never made.

**The resolution is that "shell" names two things with different lifetimes, and they are separated here.**

- The **application shell** is resident for the life of the process. It owns the tray or menu-bar item,
  the application menu, notification delivery, and the [D-67](view-protocol.md) host callbacks. It holds
  no view hierarchy, no window, and nothing authoritative.
- A **window shell** is created when a window opens and destroyed when it closes, and is what every
  existing statement about the shell being disposable is about.

This costs nothing structurally and it is not a new component: the application shell is the small amount
of native code that must already exist for a process to be an application at all on either platform. What
changes is that it is named, so that "the shell is destroyed" has one meaning. **L3 in
[memory pressure](../runtime/memory-pressure.md) destroys every window shell and not the application
shell** — which is what that tier already intended, since a tier that removed the tray would make the
application unreachable in the state it is trying to survive, and its own text says L3 *"terminates
nothing"*.

**The application shell is still shell code, and the toolkit boundary is unmoved.** The core does not
link AppKit; the resident native code sits above the C ABI like the rest of the shell. That is the whole
point of separating the two lifetimes rather than moving the tray downward.

**Windows are plural, and several requirements already assume it.** NFR-42 releases the filter engine when
the *last* window closes, and L3 in [memory pressure](../runtime/memory-pressure.md) destroys the shell
view hierarchy — which is every window, not one. Stating it here rather than leaving it implied matters
because the two read differently on a single-window reading and identically on a multi-window one, and the
resident layers beneath are shared by all of them: a window is a view onto the
[presentation layer](presentation-layer.md), never an instance of it. Anything scoped to "a window" that
is really scoped to "any window open" MUST say the latter.

**FR-23.** Native notifications for new mail, with per-account and per-folder rules and a global quiet
mode.

**"New mail" needs a definition, and the obvious one is wrong.** A message is new when it was
**delivered** — discovered by a delta as an arrival — **and was unread at that moment**, and not
otherwise. A message
merely *discovered* is not new: [D-53](../mail/sync-engine.md)'s backfill discovers half a million of
them, NFR-18's cursor recovery rediscovers a mailbox, and the IMAP full-scan path rediscovers everything
every time. Without the distinction the first thing a new account does is post fifty thousand
notifications, and it does it worst on the server that was already degraded.

**That distinction is a property of the delta path and MUST be recorded from P1**, even though this
requirement is gated in P4. It cannot be reconstructed later: knowing whether a message arrived or was
merely seen for the first time requires having been there, and recovering it after the fact would mean the
resynchronization NFR-18 exists to avoid.

Notifications are coalesced per account on the [scheduler](../runtime/scheduling.md)'s existing tick
rather than posted per message, which costs no wakeups beyond the ones already being taken.

**Across accounts, the same message is announced once.** The
[presentation layer](presentation-layer.md) deliberately shows cross-account duplicates in the list and
marks them as such, by comparing the [D-44](../storage/data-model.md) fallback digests at display time.
The same comparison decides notification duplicates, and it is legal for the same reason that document
gives: marking is not joining, nothing durable is written, and no candidate crosses an account boundary. A
list that explains a duplicate is honest; two banners for one message is just noise.

Activating a notification opens that message, which under FR-25 may mean opening a window on a process
that has none.

## First run

Nothing else specifies what happens before an account exists, and one requirement already depends on a
screen that was never described: FR-3 in [accounts](../mail/accounts.md) says "the setup interface MUST
say which lookups it will perform before performing them".

**The account-less state is the add-account flow**, not an empty inbox with a hint in it.

**No permission is requested before the action that needs it.** Contacts access is requested when a name
would first be resolved under [D-41](presentation-layer.md), not at launch; notification permission when
the first account is added; background residency is asked for explicitly, in FR-25's own terms, rather
than assumed by a client that intends to keep running after its window closes. The reason is that a
platform records a refusal persistently and D-41 already accepts that a refused permission means the
feature is absent — so a prompt asked at the wrong moment is not a bug to fix later, it is a cohort of
users who will never have that feature.

**First run states that Sift does not send mail**, and what [FR-41](../product/scope.md) does instead. It
is the product's largest adoption objection and the answer is one sentence; a user who discovers it by
looking for a reply button has already formed the wrong impression.

**FR-24.** Every action MUST be reachable from the keyboard without a pointer, including a command
palette. This is a hard requirement, not a power-user affordance — it is also what makes the app testable
without UI automation.

**NFR-27.** The message list, the reader, and message body content MUST all be navigable and announced by
a screen reader. Body content is rendered in a web engine, so this requirement crosses into the
[rendering pipeline](../rendering/pipeline.md).

**The system's other accessibility preferences are honoured too, and two of them reach decisions made
elsewhere.** [Dark mode](../rendering/dark-mode.md) says the shell follows the system light or dark
appearance natively and calls that free in both toolkits; that is true of light and dark and not of the
rest.

- **Reduced motion** is the one with a design consequence rather than a presentational one, because
  [D-38](../mail/mutations.md)'s animation carries meaning. That document owns the answer.
- **Increased contrast** applies to the shell natively, and MUST also raise the threshold the dark
  transform's contrast repair targets — a user who asked the system for more contrast has not asked for it
  everywhere except inside the message.
- **Larger text** must reach the message body as well as the chrome. The body is a separate document that
  inherits nothing from the platform's text-size preference, so the scaling is applied through the base
  stylesheet [platforms and distribution](../product/platforms-and-distribution.md) already requires — the
  same place the font fallback chain is pinned. Without that, "larger text" enlarges everything except the
  mail, which is the part the user was trying to read.

This is the argument the [roadmap](../product/roadmap.md) already makes for NFR-50 against NFR-27, applied
to a third case: the *structural* half — a body that can scale, a reconciliation that reads without
motion, a contrast target that can move — is a P1 shape decision, even though the screen-reader work it
sits beside is genuine P4.

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

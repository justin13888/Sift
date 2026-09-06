# Rendering pipeline

How an attacker-controlled message becomes pixels.

**Owns:** D-92, FR-8, FR-9, NFR-3, NFR-4, NFR-19, NFR-28, NFR-41.

**Every message body is attacker-controlled input.** Not "may be" — the sender chose every byte, and the
sender is unauthenticated by default. The pipeline is designed on that assumption throughout.

## Stages

All stages run in the core, in Rust. Nothing reaches a web engine until the last one.

```
  raw bytes
      │
  1.  MIME parse           streaming; never materialize a large part
      │
  2.  part selection       prefer HTML from a multipart alternative;
      │                    fall back to plain text, linkified
      │
  3.  sanitize             allowlist over a parsed DOM, and every fetching
      │                    position — inline part reference or remote URL
      │                    alike — rewritten to the internal scheme
      │                                              → invariants I1–I10
      │
  4.  cosmetic filter      element hiding, style injection, procedural
      │                    selectors                 → content blocking
      │
  5.  bind                 each internal-scheme address bound to the
      │                    capability token of the view about to render it
      │
  6.  transform            optional dark-mode pass, opt-in
      │
  7.  render               isolated body view: no JS, no network
      │
  content height reported back to size the container
```

The height report at the bottom of that diagram is not free, and
[D-50](webview-isolation.md) is why: script is disabled engine-wide in the body view, so the mechanism
MUST be a non-script platform interface and is a P0 spike rather than an assumption.

Stage ordering is normative. Sanitization precedes cosmetic filtering so the filter operates on a
structure it can trust; rewriting happens *inside* sanitization so that no absolute external URL survives
the stage whose output I2 is asserted over; the dark transform runs last because it must not be able to
reintroduce anything the earlier stages removed.

## D-92 — The pipeline runs off the shared runtime, and one boundary serves three concerns

**Chosen:** stages run on the **blocking pool**, not on the shared work-stealing runtime's workers; a
render is **cancellable at stage boundaries** and is cancelled when its message is superseded; and the
stage boundary is where the subsystem tag changes, where the catch boundary sits, and where cancellation
is observed — one point, three concerns.
**Rejected:** running the pipeline on the shared runtime; a non-cancellable render; separate
synchronization points for attribution, panic containment and cancellation.

**Why not the shared runtime.** NFR-41 budgets stages 3 through 6 at *"under 30 ms at p95 for a
200 KB body"*, and that is 30 ms of uninterrupted CPU rather than 30 ms of waiting.
[D-19](../architecture/overview.md) chose a work-stealing runtime because *"dozens of concurrent
operations … are almost always waiting"*; a task that never yields for 30 ms is the opposite of what that
runtime is tuned for, and it occupies a worker that [NFR-7](../architecture/ui-shell.md)'s 16 ms
optimistic feedback and [NFR-6](../architecture/ui-shell.md)'s frame budget are competing for. Work
stealing does not help, because there is nothing to steal — the work is one long task.

**This broadens what the blocking pool is for, and the broadening is the point.** D-19 describes it as
being for *"database and filesystem calls"*. The property that actually matters is not that a call
touches a device; it is that it occupies a worker long enough to matter. The pipeline is the case that
makes the distinction visible, and the rule going forward is the general one: **work that will not yield
promptly goes to the pool, whether or not it is I/O.**

**Why the render is cancellable, which nobody had said.** [D-18](../architecture/presentation-layer.md)
argues cancellation for list windows because *"a fast scroll supersedes window requests faster than they
can be served"*. Arrowing through a thread under [D-54](webview-isolation.md) supersedes renders exactly
the same way and faster, because a render costs more than a window query — and that argument was never
made for the reader. An uncancellable pipeline means a user traversing ten messages pays for ten full
renders and sees the tenth after nine wasted 30 ms passes, with the pool occupied throughout.

**Cancellation is observed at stage boundaries and nowhere else.** A stage runs to completion or is
abandoned whole; nothing checks for cancellation inside the sanitizer's tree walk. That bounds the
latency of a cancellation to one stage rather than to the pipeline, which is enough, and it keeps every
stage a pure function of its input — which is the property [D-47](../architecture/overview.md) relies on
when it says abandoning a stage *"loses a message"* and *"nothing upstream of stage 1 is invalidated"*.

**Three concerns meet at the stage boundary, and that coincidence is worth making normative.**

| Concern | Why it lands there |
|---|---|
| **Panic containment** | [D-47](../architecture/overview.md) puts a catch boundary at each stage, because that is *"the granularity at which the state is discardable"* |
| **Attribution** | [D-24](../runtime/observability.md) requires the subsystem tag be task-scoped and *"re-established at each poll"*; the [subsystem partition](../runtime/observability.md) splits this pipeline across **Parse** and **Sanitize**, so the tag has to change mid-pipeline, and the stage boundary is the only place that is well defined |
| **Cancellation** | above |

Three mechanisms that each need a synchronization point, all landing on the same one, is what makes the
pipeline implementable rather than a place where three disciplines interleave badly. **An implementer who
introduces a fourth boundary for any of them has made the other two harder to reason about**, which is
worth stating because each would look locally reasonable.

**A shed that destroys the view mid-render cancels the render.** L2 in
[memory pressure](../runtime/memory-pressure.md) destroys the body view and L3 destroys every window,
either of which can arrive while a pipeline is running. The render is cancelled at its next stage
boundary and its output discarded; nothing is left half-applied, because the output is a fresh document
that simply never reaches a view. This is the same property that makes D-47's recovery honest.

**A caught panic produces a state, not just a degradation.** D-47 requires that it not be *"silently
absorbed as an ordinary parse failure"*, and the [state register](../architecture/state-register.md)
carries the state the reader shows and the [C ABI](../architecture/view-protocol.md) its own status. The
degradation to FR-9's raw view is what the user sees; the state is what distinguishes it from a message
that was merely unparseable, which is the distinction D-47 says must not be lost.

**What it costs:** a pool sized for long CPU tasks as well as for blocking I/O, and a cancellation check
that must be at every stage boundary rather than at the ones an implementer remembers.

**Contestable because:** giving the pipeline its own pool rather than sharing the blocking one would keep
a large decode from delaying a database read, and the reason it is not proposed is that two pools are two
things to size against [Q-12](../open-questions.md)'s unmeasured budgets. If contention between rendering
and storage shows up in the P0 measurements, splitting them is the answer rather than reweighting one.

## Rewriting is not the same decision as blocking, and they happen in different places

This is the distinction the stage list above exists to make, because getting it wrong falsifies I2 while
every test still passes.

**Stage 3 rewrites every fetching position, whether or not the resource will ever be fetched.** A position
that a filter rule already condemns is rewritten too. The alternative — leaving a condemned URL in the
document because nothing will load it — puts an external-scheme URL in a fetching position in the
document handed to the body view, which is exactly what [I2](sanitizer-invariants.md) denies, and it
would leave [N-1](webview-isolation.md) as the only thing standing between the message and the network
rather than the second of two.

**Whether a rewritten address actually yields bytes is decided by the
[resource broker](../architecture/resource-broker.md), at the moment the body view asks for it.** It has
to be, because the inputs to that decision outlive the document: FR-8's per-sender allowlist and the
active [network policy tier](../runtime/network-conditions.md) both change without the message changing,
and a decision baked into markup at stage 3 could only be revised by re-running this pipeline.

**Stage 4 is therefore cosmetic filtering plus bookkeeping.** Element hiding, style injection and
procedural selectors are structural edits and belong to the document. The network verdict computed here
is recorded for [FR-33](../runtime/observability.md)'s per-message view — every candidate URL, its verdict
and the matching rule — and is not the enforcement point. The broker is the enforcement point, and the
compiled engine rules of [D-10](content-blocking.md) are the backstop beneath it.

**Stage 5 exists because a capability token does not.** [D-28](webview-isolation.md) mints a token per
view, so stage 3's rewritten addresses are view-independent and stage 5 binds them to the view that is
about to render. Splitting the two is what lets a sanitized document be reused across views without ever
carrying a token that outlives one.

The ordering is also what makes the pipeline extensible without weakening it. [D-39](../product/scope.md)
defers end-to-end encrypted mail and names its seam as a stage between 1 and 2 — deliberately *above*
sanitization, so that decrypted content is subject to I1–I10 exactly as plaintext mail is. **Any stage
added later must enter above stage 3 for the same reason.** A stage that produced markup after
sanitization would be an exemption from every invariant this pipeline asserts.

Detail lives in [sanitizer invariants](sanitizer-invariants.md),
[content blocking](content-blocking.md), [sender origin](sender-origin.md), [dark mode](dark-mode.md),
[link handling](link-handling.md), and [webview isolation](webview-isolation.md).

## FR-8 — Full-fidelity HTML rendering

HTML bodies MUST render at full fidelity, with **remote content blocked by default** and a per-sender
allowlist.

Every remote image is a tracking pixel until proven otherwise. Blocking by default is the correct default
even before the blocker's heuristics apply — see [content blocking](content-blocking.md).

**Two affordances, and they are not the same decision.** *Show images once* applies to the message in
front of the user and is not written down anywhere; *always show images from this sender* writes the
per-sender allowlist, which is durable [security state](../storage/data-model.md). Offering only the
second would mean every act of curiosity permanently widens what Sift will fetch, which is not what a user
means when they want to see one newsletter.

**"Always" is unavailable for an unauthenticated sender, and the interface MUST say so rather than
quietly doing something weaker.** [Sender origin](sender-origin.md) requires the allowlist to key on the
synthetic origin, and an origin that resolved from nothing better than a From header is **unauthenticated
rather than null** — it has a domain, and that is worse than having none, because the thing there is to key
on is one the sender wrote. Keying on it would be trivially forgeable, which is the whole of D-11's
argument. Both states therefore refuse "always", by different routes: null has nothing to key on, and
unauthenticated has something that cannot be trusted. So an unattested message offers *show once* and an explanation, and the one-time
affordance is what makes that an acceptable answer rather than a refusal.

All body resources MUST route through the resource broker. The body view has no direct network access
whatsoever; this is invariant N-1 in [webview isolation](webview-isolation.md).

## FR-9 — Fallbacks

A plain-text view and a raw source view MUST be available for **any** message, including one that rendered
successfully. The raw view is also the degradation target when parsing fails.

## NFR-19 — Hostile input must not crash Sift

A malformed or hostile message MUST NEVER crash the process. Parse failure degrades to the raw source
view.

Sift is resident and holds every account's sync state and mutation queue, and under
[D-2](../architecture/process-model.md) it also holds the shell — so a remote-crash primitive in it is a
denial of service on the user's mail and takes any open window with it. Streaming parse (below) also
guards against memory exhaustion as a crash vector.

## NFR-28 — Text correctness

Sift MUST correctly handle non-UTF-8 charsets, encoded words in headers, parameter continuations, and
bidirectional text in headers.

This is unglamorous and it is where real mail breaks. Bidirectional text in headers is additionally a
*security* concern, not only a correctness one — see [link handling](link-handling.md).

## Streaming

MIME parsing MUST stream. A 200 MB attachment streams to the [blob store](../storage/cache-and-blobs.md);
only headers and structure land in memory. No stage of this pipeline may materialize a whole large part.

## Performance targets

Hypotheses, validated against the [reference environment](../product/reference-environment.md).

| ID | Target |
|---|---|
| **NFR-3** | Cached message open to body first paint under 80 ms at p95 |
| **NFR-4** | Uncached message open to body painted under 600 ms at p95 on a 50 Mbps link |
| **NFR-41** | Stages 3 through 6 complete in under 30 ms at p95 for a 200 KB body |

NFR-41 constrains the sanitizer, blocker, and transform together, because they are one user-visible delay.
It is also why the transform is a static pass over parsed CSS rather than anything iterative — see
[dark mode](dark-mode.md).

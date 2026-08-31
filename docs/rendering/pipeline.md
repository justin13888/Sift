# Rendering pipeline

How an attacker-controlled message becomes pixels.

**Owns:** FR-8, FR-9, NFR-3, NFR-4, NFR-19, NFR-28, NFR-41.

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

Stage ordering is normative. Sanitization precedes cosmetic filtering so the filter operates on a
structure it can trust; rewriting happens *inside* sanitization so that no absolute external URL survives
the stage whose output I2 is asserted over; the dark transform runs last because it must not be able to
reintroduce anything the earlier stages removed.

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

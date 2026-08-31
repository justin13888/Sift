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
  3.  sanitize             allowlist over a parsed DOM  → invariants I1–I10
      │
  4.  block                network + cosmetic decisions → content blocking
      │
  5.  rewrite              inline references and every remaining fetching
      │                    position rewritten to the internal scheme
      │
  6.  transform            optional dark-mode pass, opt-in
      │
  7.  render               isolated body view: no JS, no network
      │
  content height reported back to size the container
```

Stage ordering is normative. Sanitization precedes blocking so the blocker operates on a structure it can
trust; rewriting precedes rendering so that no absolute external URL survives into the document; the dark
transform runs last because it must not be able to reintroduce anything the earlier stages removed.

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

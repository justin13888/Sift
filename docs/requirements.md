# Requirements index

Every requirement identifier, its one-line statement, and the **one document that owns it**.

Normative text lives in the owning document, next to the design it constrains. This page is an index, not
a source. Where two documents appear to state the same requirement, the owner wins.

**Identifiers are stable and MUST NOT be renumbered.** They are used in review comments, commit messages,
and tests. A requirement that is dropped is struck through here with a reason; its number is never reused.

All performance and resource figures are **hypotheses to be validated** against the
[reference environment](product/reference-environment.md), never measured facts.

**Phase assignment is not here.** Which phase ships or gates a requirement is owned by the
[roadmap](product/roadmap.md), which carries the coverage table. Restating it here would give the mapping
two homes and one of them would go stale.

## Functional requirements

| ID | Requirement | Owner |
|---|---|---|
| FR-1 | Multiple accounts across Gmail, Microsoft 365/Outlook.com, JMAP, and generic IMAP | [mail/accounts](mail/accounts.md) |
| FR-2 | OAuth 2.0 with PKCE via system browser, returning through a registered URI scheme; tokens in the OS credential store; silent refresh | [security/credentials](security/credentials.md) |
| FR-3 | Generic IMAP manual configuration with autoconfiguration discovery attempt | [mail/accounts](mail/accounts.md) |
| FR-4 | Account removal provably erases database, blobs, and credentials | [mail/accounts](mail/accounts.md) |
| FR-5 | Special-use folders identified semantically, never by localized name | [mail/provider-model](mail/provider-model.md) |
| FR-6 | Virtualized message list with sender, subject, snippet, date, flags, attachments, thread count | [architecture/ui-shell](architecture/ui-shell.md) |
| FR-7 | Unified inbox across accounts | [architecture/presentation-layer](architecture/presentation-layer.md) |
| FR-8 | Full HTML body rendering, remote content blocked by default, per-sender allowlist | [rendering/pipeline](rendering/pipeline.md) |
| FR-9 | Plain-text fallback and raw source view for any message | [rendering/pipeline](rendering/pipeline.md) |
| FR-10 | Attachment list with lazy download, save, and warned open | [storage/cache-and-blobs](storage/cache-and-blobs.md) |
| FR-11 | Threading by provider identifier, reconstructed for plain IMAP | [mail/threading](mail/threading.md) |
| FR-12 | Offline read of cached mail; "not cached" distinguished from "not available" | [storage/cache-and-blobs](storage/cache-and-blobs.md) |
| FR-13 | Triage mutations expressed as provider-agnostic intents, over a closed and explicitly-stated set | [mail/mutations](mail/mutations.md) |
| FR-14 | Optimistic local application with a durable mutation queue | [mail/mutations](mail/mutations.md) |
| FR-15 | Every intent but permanent delete reversible through its compensation; a timed undo window additionally where the action removes the message from view | [mail/mutations](mail/mutations.md) |
| FR-16 | Conflict resolution: silent where unambiguous, non-blocking notice where not | [mail/mutations](mail/mutations.md) |
| FR-17 | Bulk operations over selections and search results, batched to provider limits | [mail/mutations](mail/mutations.md) |
| FR-18 | Replayed mutations never double-apply | [mail/mutations](mail/mutations.md) |
| FR-19 | Local full-text search, incremental as-you-type | [storage/search](storage/search.md) |
| FR-20 | Structured search operators | [storage/search](storage/search.md) |
| FR-21 | Server-side search fallback, merged and labelled by source | [storage/search](storage/search.md) |
| FR-22 | Menu-bar or tray presence as the always-on surface | [architecture/ui-shell](architecture/ui-shell.md) |
| FR-23 | Native notifications with per-account and per-folder rules and quiet mode | [architecture/ui-shell](architecture/ui-shell.md) |
| FR-24 | Full keyboard operability and a command palette | [architecture/ui-shell](architecture/ui-shell.md) |
| FR-25 | "Close the window, keep syncing" distinct from "quit entirely" | [architecture/process-model](architecture/process-model.md) |
| FR-26 | Updates delivered by platform channels only; no self-update; restart surfaced when the bundle is replaced | [architecture/process-model](architecture/process-model.md) |
| FR-27 | Content blocking with uBlock-syntax lists plus a bundled email list | [rendering/content-blocking](rendering/content-blocking.md) |
| FR-28 | Synthetic first-party origin derived from authenticated sender identity | [rendering/sender-origin](rendering/sender-origin.md) |
| FR-29 | Tracking-pixel heuristics independent of filter-list coverage, reasons logged | [rendering/content-blocking](rendering/content-blocking.md) |
| FR-30 | Link unwrapping with punycode-decoded, bidi-stripped display | [rendering/link-handling](rendering/link-handling.md) |
| FR-31 | Opt-in, default-off dark transform with per-message and per-sender control | [rendering/dark-mode](rendering/dark-mode.md) |
| FR-32 | Sender-declared dark mode honoured in preference to the transform | [rendering/dark-mode](rendering/dark-mode.md) |
| FR-33 | Per-message debug view exposing the full pipeline | [runtime/observability](runtime/observability.md) |
| FR-34 | Runtime debug panel: memory, caches, wakeups, sync state, queue, network | [runtime/observability](runtime/observability.md) |
| FR-35 | Network-condition detection, with a per-network user override offered wherever detection is used | [runtime/network-conditions](runtime/network-conditions.md) |
| FR-36 | Data-usage accounting per account per link class, with optional hard cap | [runtime/network-conditions](runtime/network-conditions.md) |
| FR-37 | Tags as a first-class concept distinct from Location | [mail/provider-model](mail/provider-model.md) |
| FR-38 | Thread mutations with defined partial-failure semantics under client fan-out | [mail/mutations](mail/mutations.md) |
| FR-39 | Report junk and not-junk as intents, where the account declares junk-reporting support | [mail/mutations](mail/mutations.md) |
| FR-40 | Contact name resolution for display, read-only from the platform contact store | [architecture/presentation-layer](architecture/presentation-layer.md) |
| FR-41 | Reply, reply-all and forward hand off to the platform's mail handler; Sift never constructs or transmits a message | [product/scope](product/scope.md) |
| FR-42 | Unsubscribe destination shown and opened in the system browser; never requested by Sift | [rendering/link-handling](rendering/link-handling.md) |
| FR-43 | Watched folder set per account, user-selectable and persisted | [runtime/scheduling](runtime/scheduling.md) |

## Performance

| ID | Requirement | Owner |
|---|---|---|
| NFR-1 | Cold start to interactive list under 400 ms p95 | [architecture/ui-shell](architecture/ui-shell.md) |
| NFR-2 | Warm folder or account switch under 50 ms p95 | [architecture/ui-shell](architecture/ui-shell.md) |
| NFR-3 | Cached message to body first paint under 80 ms p95 | [rendering/pipeline](rendering/pipeline.md) |
| NFR-4 | Uncached message to body painted under 600 ms p95 at 50 Mbps | [rendering/pipeline](rendering/pipeline.md) |
| NFR-5 | Search keystroke to results under 100 ms p95 at 500k messages | [storage/search](storage/search.md) |
| NFR-6 | List scroll sustains 60 fps with zero dropped frames over a 10k-row fling | [architecture/ui-shell](architecture/ui-shell.md) |
| NFR-7 | Triage action reflected in UI within 16 ms, before network — **permanent delete excluded**, being the one intent FR-14 does not apply optimistically | [architecture/ui-shell](architecture/ui-shell.md) |
| NFR-41 | Sanitize, block, and transform under 30 ms p95 for a 200 KB body | [rendering/pipeline](rendering/pipeline.md) |

## Resource

| ID | Requirement | Owner |
|---|---|---|
| NFR-8 | Resident idle footprint with no window open at or under 90 MB, excluding the filter engine — a placeholder pending P0 measurement | [runtime/memory-pressure](runtime/memory-pressure.md) |
| NFR-9 | Full application idle footprint, window open and no reader visible, at or under 150 MB, inclusive of NFR-42 | [runtime/memory-pressure](runtime/memory-pressure.md) |
| NFR-10 | Idle CPU at or under 0.1% over 5 minutes | [runtime/scheduling](runtime/scheduling.md) |
| NFR-11 | At most 2 wakeups per minute per account at idle, timer fires and socket wakes alike | [runtime/scheduling](runtime/scheduling.md) |
| NFR-12 | Footprint growth at or under 5% over 14 days: no ratchet | [runtime/memory-pressure](runtime/memory-pressure.md) |
| NFR-13 | L2 shed **issued** within 500 ms of signal, L3 within 1 second; reclaim itself is timed by NFR-46. L3 sheds in-process and terminates nothing | [runtime/memory-pressure](runtime/memory-pressure.md) |
| NFR-14 | Bodies, attachments and blobs bounded by the configured cache budget, hard-capped | [storage/cache-and-blobs](storage/cache-and-blobs.md) |
| NFR-15 | Network at idle at or under 1 KB per minute per account | [runtime/scheduling](runtime/scheduling.md) |
| NFR-42 | Filter engine memory at or under 40 MB with standard lists loaded, in every build whether bundled or imported as custom rules; bound to window lifetime, dropped at L1 | [rendering/content-blocking](rendering/content-blocking.md) |
| NFR-44 | Allocation-attribution overhead at or under 2% in release | [runtime/observability](runtime/observability.md) |
| NFR-45 | Soak harness in CI with a slope gate over at least 72 hours | [runtime/observability](runtime/observability.md) |
| NFR-46 | Body view torn down when unused; footprint returns within 1 second | [rendering/webview-isolation](rendering/webview-isolation.md) |
| NFR-48 | Schema migrations preserve envelopes, blobs, queued mutations with their pending overlays, and both scopes of policy state; never require resync | [storage/data-model](storage/data-model.md) |
| NFR-49 | Attachments written to disk carry the platform's untrusted-source provenance marking | [storage/cache-and-blobs](storage/cache-and-blobs.md) |
| NFR-53 | A sender-supplied filename never becomes a path; the final path is shown and nothing is overwritten | [storage/cache-and-blobs](storage/cache-and-blobs.md) |
| NFR-52 | Envelopes and their full-text index entries bounded by their own budget, evicted oldest-first together | [storage/cache-and-blobs](storage/cache-and-blobs.md) |

## Reliability and correctness

| ID | Requirement | Owner |
|---|---|---|
| NFR-16 | No data loss on abrupt termination; store and queue crash-consistent | [mail/mutations](mail/mutations.md) |
| NFR-17 | Mutations exactly-once observable from the user's perspective | [mail/mutations](mail/mutations.md) |
| NFR-18 | Cursor invalidation recovers automatically and visibly; no manual full resync | [mail/sync-engine](mail/sync-engine.md) |
| NFR-19 | Hostile MIME never crashes the process; parse failure degrades to raw view | [rendering/pipeline](rendering/pipeline.md) |

## Security and privacy

| ID | Requirement | Owner |
|---|---|---|
| NFR-20 | Zero JavaScript in message bodies, enforced at the engine level | [rendering/webview-isolation](rendering/webview-isolation.md) |
| NFR-21 | No unrequested network egress from message content | [rendering/webview-isolation](rendering/webview-isolation.md) |
| NFR-22 | No telemetry containing content, addresses, subjects, or domains | [security/privacy](security/privacy.md) |
| NFR-23 | Credentials only in the OS credential store | [security/credentials](security/credentials.md) |
| NFR-24 | Sift never opens a listening socket of any kind, for any purpose | [architecture/shell-boundary](architecture/shell-boundary.md) |
| NFR-25 | Body view sandboxed in its own data store | [rendering/webview-isolation](rendering/webview-isolation.md) |
| NFR-40 | Invariants I1–I10 verified by property, differential, and fuzz testing in CI | [rendering/sanitizer-invariants](rendering/sanitizer-invariants.md) |
| NFR-50 | Body-view isolation does not sever the accessibility tree | [rendering/webview-isolation](rendering/webview-isolation.md) |
| NFR-55 | Local logs carry no content, addresses, subjects, domains or credentials; bounded and time-limited | [product/platform-baseline](product/platform-baseline.md) |

## Network

| ID | Requirement | Owner |
|---|---|---|
| NFR-30 | Path change re-evaluated within 2 s; unknown metered maps to Conservative | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-31 | Minimal tier at or under 10 KB per hour per account | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-32 | Zero speculative prefetch in Conservative or Minimal | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-33 | Connections re-established within 5 s of a path change, nothing lost | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-34 | Captive portal handled without an authentication-failure cascade | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-35 | Per-network user override persisted and takes precedence | [runtime/network-conditions](runtime/network-conditions.md) |
| ~~NFR-36~~ | ~~Data usage accounted per account per link class with optional cap~~ — **struck as a verbatim duplicate of FR-36**, which owns this requirement. The number is retired and MUST NOT be reused | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-37 | At most 6 radio-waking events per hour per account on cellular | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-38 | Zero connection attempts in airplane mode or sleep; no retry storm on wake | [runtime/network-conditions](runtime/network-conditions.md) |
| NFR-39 | Byte ceiling on a single fetch without explicit confirmation | [runtime/network-conditions](runtime/network-conditions.md) |

## Compatibility and quality

| ID | Requirement | Owner |
|---|---|---|
| NFR-26 | Rendering correct across the fidelity corpus, with a cross-platform perceptual diff gate | [product/reference-environment](product/reference-environment.md) |
| NFR-27 | Full screen-reader support across list, reader, and body content | [architecture/ui-shell](architecture/ui-shell.md) |
| NFR-28 | Correct non-UTF-8 charsets, encoded words, parameter continuations, and bidi headers | [rendering/pipeline](rendering/pipeline.md) |
| NFR-29 | Graceful, surfaced degradation on limited IMAP servers | [mail/providers/imap](mail/providers/imap.md) |
| NFR-43 | Filter-list updates never block rendering; stale is acceptable, absent is not | [rendering/content-blocking](rendering/content-blocking.md) |
| NFR-47 | Dark transform meets the contrast threshold for at least 95% of the corpus | [rendering/dark-mode](rendering/dark-mode.md) |
| NFR-51 | Presentation-layer formatting, ordering and collation are locale-aware from the first commit | [architecture/presentation-layer](architecture/presentation-layer.md) |
| NFR-54 | Attacker-controlled strings normalized before they reach native chrome; address shown beside a display name | [architecture/presentation-layer](architecture/presentation-layer.md) |

## Sanitizer invariants

I1 through I10 and invariant N-1 are stated in
[rendering/sanitizer-invariants](rendering/sanitizer-invariants.md) and
[rendering/webview-isolation](rendering/webview-isolation.md) respectively.

I9 was restated as a non-addition property rather than a subset relation; the reasoning is in its owning
document. Its number and its meaning are unchanged.

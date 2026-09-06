# Limits

Every numeric bound Sift enforces at runtime, in one place.

**Owns:** L-1 through L-31.

**"Enforces at runtime" is the boundary, and it is narrower than the old wording.** This page previously
claimed every numeric bound in the project, which reached two populations it does not own and should not.
A **performance or resource target** — 400 ms, 90 MB, 5% over 14 days — belongs to its requirement in
[requirements](requirements.md), because exceeding it is a regression rather than a rule being applied. A
**gate threshold** — the perceptual-diff bound, the soak slope — belongs to
[verification](build/verification.md) under [D-64](build/verification.md), because CI enforces it and
Sift does not. What is here is the third kind: a number Sift itself checks, where crossing it changes
what the running application does.

## Why these are a register rather than constants

Three separate consumers must assert the *same* number, and
[sanitizer invariants](rendering/sanitizer-invariants.md) already says two of them share code: the
sanitizer itself, NFR-40's property generator — which has to generate inputs *at* the boundary to be
worth running — and FR-33 item 9's live per-message invariant check. A bound scattered across those three
is a bound that drifts, and **a property test cannot assert a limit it has to guess**.

I7 states that "nesting depth, node count, and attribute count are capped" and names no cap. The
[resource broker](architecture/resource-broker.md) requires that "dimensions and resource limits are
checked before any decoder is handed bytes" and names no limit. NFR-39 requires a byte ceiling on a single
fetch and gives no default. This page is where those become falsifiable.

**Every number here is a hypothesis** to be validated against the
[reference environment](product/reference-environment.md), on the same footing as every other figure in
this documentation set. They are chosen to be far above what legitimate mail uses and far below what
exhausts the machine, and the gap between those two is wide enough that precision is not the point.

## Exceeding a limit rejects; it does not truncate

**A message that exceeds any parse limit in the table below MUST degrade to the raw source view under
FR-9, and MUST NOT be rendered partially.** A resource that exceeds a resource limit MUST resolve to the
deterministic blocked answer the [resource broker](architecture/resource-broker.md) already defines, and
MUST NOT be handed over partially decoded.

The alternative — truncate and render what fits — is the tempting one and it is wrong twice. It shows the
user a partial message with nothing saying so, which is the class of dishonesty FR-12 rejects when it
insists "not cached" and "not available" stay distinct. And it hands the sanitizer's output contract to
the attacker: a document truncated mid-tree is a document whose structure the sender chose by choosing
where the cap fell, which is a parse-differential primitive of exactly the kind I8 exists to close.

Rejection also costs less than it appears. NFR-19 already requires parse failure to degrade to the raw
view, so the path exists, is tested, and is the one a user already meets on malformed mail.

**One exception, and it is not a message.** The snippet (L-16) is a summary Sift derives for the list, not
content it renders, so it is bounded by truncation rather than rejection. That is consistent with I9,
which permits removal and forbids invention.

## Parse limits

Asserted by the [pipeline](rendering/pipeline.md)'s stages 1 through 3 and by
[sanitizer invariants](rendering/sanitizer-invariants.md).

| ID | Bounds | Value | Notes |
|---|---|---|---|
| **L-1** | Bytes of a single body part decoded into the sanitizer | 8 MB | The part selected at stage 2, after transfer decoding. Larger parts exist and are almost never legitimate HTML |
| **L-2** | MIME parts per message | 1024 | Counted across the whole tree, not per level |
| **L-3** | MIME nesting depth | 32 | A `multipart` inside a `message/rfc822` inside a `multipart` is ordinary; thirty-two levels is an attack |
| **L-4** | Bytes of the header block | 256 KB | Applies before any header is decoded, so an encoded-word bomb is bounded before NFR-28's decoding runs |
| **L-5** | Header fields per message | 1024 | |
| **L-6** | DOM tree depth after parsing | 512 | The HTML5 tree builder's own recovery already collapses much deeper nesting; this bounds what survives it |
| **L-7** | DOM nodes per document | 250,000 | This is the cap NFR-41's 30 ms budget is actually a function of, and the one most likely to move once measured |
| **L-8** | Attributes per element | 256 | |
| **L-9** | CSS declarations resolved across all stylesheets and style attributes | 100,000 | The bound on [D-27](rendering/dark-mode.md)'s cascade, which is the pass most likely to miss NFR-41 |

## Resource limits

Asserted by the [resource broker](architecture/resource-broker.md), before a decoder is handed bytes.

| ID | Bounds | Value | Notes |
|---|---|---|---|
| **L-10** | Encoded bytes of a single image accepted for rendering | 32 MB | Checked against the declared length and enforced against the transferred length, since a sender controls both |
| **L-11** | Pixels of an image decoded for classification | 40 megapixels | [D-29](rendering/content-blocking.md) decodes only for the dark transform's classifier. This is the decode-bomb bound, and it is checked against the *declared* dimensions in the container before any decoder runs |
| **L-12** | Rasterized output of a vector image | the same 40 megapixels | A vector image declares no pixel dimensions of its own, so the bound is on what it is rasterized into. D-29 refuses what cannot be rasterized inside it |
| **L-13** | Bytes of a single fetch without explicit confirmation | 25 MB | The default NFR-39 in [network conditions](runtime/network-conditions.md) requires and does not supply. User-configurable, like the cache budget |

## Concurrency and connection limits

| ID | Bounds | Value | Notes |
|---|---|---|---|
| **L-23** | Concurrent provider connections per installation | 16 | The budget [scheduling](runtime/scheduling.md) names and does not number. It is that document's own arithmetic — five accounts watching three folders each — plus one for on-demand work, and it is what decides how many folders may be watched at all when a provider cannot watch several over one connection |
| **L-26** | Envelopes fetched per backfill page | 500 | The page size [D-53](mail/sync-engine.md)'s resumable backfill requires and does not state. It is the granularity a resume rewinds to, so it trades round trips against work repeated after an interruption |
| **L-29** | Concurrent resource loads per rendered document | 8 | The bound [D-91](architecture/resource-broker.md) requires so that one message with several hundred fetching positions cannot saturate the pool the store, the queue and search share. Requests beyond it queue rather than fail |

## Storage and display limits

| ID | Bounds | Value | Notes |
|---|---|---|---|
| **L-14** | Bytes of a single blob | 2 GB | Above the cache budget's own default, so in practice NFR-14 binds first; this exists so that a single attachment cannot be the thing that makes the budget unenforceable |
| **L-15** | Characters in a tag name | 256 | The "length and charset limits" the tag capability row in [provider model](mail/provider-model.md) refers to and does not state. The charset is Unicode scalar values excluding control characters, normalized under NFR-54 like every other attacker-controlled string |
| **L-20** | Default envelope and index budget | 1 GB | The value NFR-52 in [cache and blobs](storage/cache-and-blobs.md) calls user-configurable and does not supply, and which [D-53](mail/sync-engine.md) makes the *sole* bound on first sync. At roughly two kilobytes per message including its index entry it lands near the 500,000 messages [NFR-5](storage/search.md) is measured over, so the benchmark and the shipped default describe the same product rather than two |
| **L-25** | Characters in a header-derived display value | 998 | The length bound NFR-54 in [presentation layer](architecture/presentation-layer.md) requires and does not state, over display names, subjects, folder and tag names and attachment names. Chosen as the internet message format's own line bound, which is far above any legitimate value and far below a denial of service against native chrome. Truncation is at a grapheme boundary and follows normalization, per [D-100](architecture/presentation-layer.md) |
| **L-16** | Characters in a snippet | 280 | Truncated rather than rejected, per the exception above. Bounds the envelope, which NFR-52 in [cache and blobs](storage/cache-and-blobs.md) budgets and which is retained far longer than any body |

## Time limits

Durations Sift enforces. They are bounds on behaviour, not performance targets — a target belongs to its
requirement in [requirements](requirements.md), and appears here only if exceeding it changes what Sift
does rather than how fast it does it.

| ID | Bounds | Value | Notes |
|---|---|---|---|
| **L-17** | Age at which a queued intent stops being retried | 7 days | [D-85](mail/mutations.md) expires on elapsed time rather than attempts, because an intent that failed twice in a week offline and one that failed two hundred times in a minute are not the same situation. Long enough to cover an ordinary offline stretch; short enough that the conflict pile-up FR-16 must adjudicate stays adjudicable |

| **L-18** | Time with no reader visible before the body view is torn down | 30 seconds | The period NFR-46 in [webview isolation](rendering/webview-isolation.md) calls configured and does not supply. Long enough to survive switching folders and returning; short enough that the largest single allocation in the running application does not persist through an interruption. [D-90](rendering/webview-isolation.md) defines what counts as visible |

| **L-21** | Reader dwell before a message is marked read | 2 seconds | The value [D-52](mail/mutations.md) calls "a short configurable dwell" and does not supply, in the decision that argues this number sets the queue write rate, the flush wakeup rate against NFR-11 and the data-cap burn. Longer than arrow-key traversal and shorter than reading, which is the only property it has to have. May be set to off |
| **L-22** | Duration of FR-15's timed undo window | 10 seconds | Stated in [mutations](mail/mutations.md) since FR-15 was written and never registered here, which is the drift this page exists to prevent. [D-86](mail/mutations.md) explains why the window withholds nothing, so this bounds an affordance rather than a network delay |
| **L-24** | Cap on reconnection and retry backoff | 15 minutes, with ±25% jitter | The cap and jitter [scheduling](runtime/scheduling.md) requires and does not supply, and the number NFR-38's "no retry storm on wake" rests on. The cap matches the longest aligned poll interval, so a backed-off account rejoins an existing wheel fire rather than adding one |
| **L-27** | Interval at which an IMAP idle watch is re-issued | 29 minutes | Stated loosely as "roughly every 29 minutes" in [IMAP](mail/providers/imap.md). It is a bound rather than a preference: it sits below the protocol's own 30-minute expectation and below common network-address-translation timeouts, and exceeding it drops a watch silently |
| **L-28** | Interval between reattempts while a captive portal is present | 60 seconds | The cadence in [network conditions](runtime/network-conditions.md)'s offline-portal tier. Under [D-96](runtime/network-conditions.md) it is a bounded reattempt of the account's own next operation rather than a probe to a detection host |
| **L-30** | Aligned interval at which an idle account polls for new mail | 60 seconds | The cadence [scheduling](runtime/scheduling.md) requires and does not number. Aligned to the wall clock rather than to its own arming, so two accounts land on one instant without coordinating — which is how they share a wakeup instead of taking two. [D-94](runtime/scheduling.md) makes NFR-11's budget two wakeups per minute for the whole application, so a minute is the largest interval leaving room for a flush in the same minute and the smallest that does not spend the budget on polling alone. A floor on freshness rather than the only way mail arrives: push is preferred where a provider offers it |
| **L-31** | The scheduler's coalescing window | 5 seconds | Deadlines inside one window fire together, so this *is* [D-25](runtime/scheduling.md)'s "this may fire late, batch it" hint expressed as structure, and it is what the platform timer is given as its leeway. Larger means fewer wakeups and later work |
| **L-19** | Time the pressure signal must stay clear before a shed tier is released | 60 seconds | The hysteresis [D-93](runtime/memory-pressure.md) requires. Without it a system oscillating around the threshold reparses the 40 MB filter engine on every crossing. One value serves every tier, which that decision records as its weakest point |

## Changing a limit

**A limit MUST NOT be raised to accommodate a single message.** The corpus decides: if the
[fidelity corpus](product/reference-environment.md) contains legitimate mail that a bound rejects, the
bound is wrong and moves with a note saying which message moved it. If it does not, the message is the
adversary this design is for, and rejection is the correct outcome rather than a bug report.

**A new limit arrives here, not beside the code that enforces it.** This is the same rule
[sanitizer invariants](rendering/sanitizer-invariants.md) applies to the allowlist — an addition that
lands anywhere else is invisible to the two other consumers that must assert it, and the failure is
silent in both.

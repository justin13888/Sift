# Content blocking

Blocking at the quality bar of a serious browser blocker, using a structural advantage a browser does not
have.

**Owns:** D-10, D-29, FR-27, FR-29, NFR-42, NFR-43.

## The structural advantage

A browser extension intercepts requests it does not control. **Sift owns every byte.** Because the body
view has no network capability at all — invariant N-1 in [webview isolation](webview-isolation.md) — every
candidate load arrives at the resource broker as a question, not as a request already in flight.

The blocker is therefore a decision function inside Sift's own fetch path. That is far easier to reason
about and to test than interception, and it is why per-message blocking decisions can be enumerated
exactly in the [debug view](../runtime/observability.md). The component that path runs through is the
[resource broker](../architecture/resource-broker.md).

## D-10 — A browser-grade filter engine as authority, compiled engine rules as backstop

**Chosen:** an established Rust filter engine — the one powering a shipping browser's native blocker — as
the **authority**, with rules compiled into the web engine's own content-rule format as
defence in depth.
**Rejected:** a bespoke rule matcher, or engine content rules alone.

**Why.** The engine brings uBlock Origin-compatible syntax, which means the mature public filter lists are
consumable directly rather than transcribed. It also converts its rule set into the content-blocking
format that **both** target engines consume — a gift for this platform pair, giving a second in-engine
enforcement layer from the same rule source for nearly free.

The broker is the authority; the compiled rules are the backstop. **If the two ever disagree, that is a
bug**, and the debug view MUST surface the disagreement rather than silently taking either answer.

**NFR-42.** Filter engine memory MUST stay at or under 40 MB with the standard blocking and privacy lists
plus the bundled email list loaded. It is a declared, budgeted cache like any other, and it has an
eviction policy like any other: **the engine is loaded when a window opens, released when the last window
closes, and dropped at the L1 shed tier.** No window means no body view and therefore no caller, so
holding it resident with none open would spend the largest single cache budget in the design on a
component nothing can reach. See [memory pressure](../runtime/memory-pressure.md), where NFR-8 excludes it
and NFR-9 includes it.

Reloading is a list parse on window open, ahead of any message selection, so it falls on NFR-1 rather than
on NFR-3. The compiled engine-level backstop rules are installed with the body view's configuration and
follow the same lifetime.

**Being dropped at L1 leaves a window open with no authority loaded, and that state needs a stated
answer.** L1 is a mild-pressure tier, so a user may well open a message while the engine is gone. Sift
MUST NOT silently fall through to the compiled backstop, because the backstop is defence in depth against
a bug in the authority, not a substitute for it — treating it as one converts a memory-pressure event
into a quiet reduction in blocking that no surface reports. Nor may the engine be reloaded on demand: a
40 MB allocation in response to a pressure signal is the shed tier undoing itself.

The rule is therefore that **an absent authority denies.** With no engine loaded, every remote fetch is
refused, exactly as it would be for a sender the user has not allowed, and the reason recorded for
[FR-33](../runtime/observability.md) names the shed rather than a rule. FR-29's heuristics are Sift's own
code and continue to apply; cosmetic filtering is unaffected, having already been baked into the document
at sanitize time. The engine returns when pressure clears and the next window opens, and the failure
direction is the one this document chooses everywhere else: a message with missing images rather than a
message that quietly fetched something.

## FR-27 — Filter lists

Content blocking with uBlock Origin-syntax filter lists: the standard public blocking and privacy lists,
plus a **bundled email-specific list**. List subscriptions and custom rules MUST be user-manageable.

**NFR-43.** Filter-list updates MUST NEVER block rendering. A stale list is acceptable; an absent one is
not. Updates are network traffic and MUST obey the active policy tier in
[network conditions](../runtime/network-conditions.md).

## Where each half of blocking happens

The two halves run at different times against different inputs, and [the pipeline](pipeline.md)'s stage
list is where that split is normative. Stating it here too, from the blocker's side:

| Half | When | Why there |
|---|---|---|
| Cosmetic filtering — element hiding, style injection, procedural selectors | in the core, over the parsed tree, before the document is emitted | it changes the document, and a document is edited once |
| The network verdict — is this resource allowed | at request time, in the [resource broker](../architecture/resource-broker.md) | its inputs are the per-sender allowlist and the active policy tier, both of which change without the message changing |

The per-message pass still *computes* a network verdict for every candidate URL, and that computation is
what [FR-33](../runtime/observability.md) item 5 enumerates. It is a record, not the enforcement.
Enforcement is the broker's, with the compiled engine rules beneath it as the backstop D-10 describes.

## Cosmetic filtering without JavaScript

JavaScript is off, so the usual injection path is unavailable. Filters are split by class:

| Filter class | Mechanism |
|---|---|
| Plain element hiding | a generated stylesheet block — works with zero script |
| Style injection | the same generated stylesheet |
| Procedural selectors | evaluated **in Rust at sanitize time** against the parsed DOM; matching nodes are removed |
| Scriptlets | **not applicable** — there is no script to intercept. These rules are ignored entirely |

Procedural evaluation needs a selector engine over the parsed DOM. Some procedural cases can degrade to
plain CSS where the engine supports the relevant selectors natively — which is version-dependent on Linux
and is part of why the runtime is pinned, see
[platforms and distribution](../product/platforms-and-distribution.md).

## FR-29 — Email-specific heuristics

**Public filter lists target web advertising and cover email tracking poorly.** Heuristic blocking MUST
operate independently of filter-list coverage, and every heuristic block MUST log its reason.

**Tracking pixels.** Declared or intrinsic dimensions of two pixels or fewer; images hidden by display,
visibility, or zero opacity; URLs whose path carries a high-entropy per-recipient token; images with no
alternative text inside a zero-height container.

**CSS-vector tracking.** Background images, font sources, list-style images, cursors, border images, mask
images, and reflection properties are all fetching positions. Every one MUST route through the broker, and
they MUST be **enumerated explicitly** in the CSS transform pass rather than left to the sanitizer's
element allowlist, which does not see them.

**Click trackers** are handled as a display concern rather than a blocking one — the link fetches nothing
until clicked. See [link handling](link-handling.md).

## First-party determination

Filter syntax assumes a document origin, and an email has none. The origin is synthesized — see
[sender origin](sender-origin.md), which owns both that decision (D-11) and the rule that widens it to
known mail infrastructure for attested senders only (D-37).

## D-29 — Decode images only when something needs the pixels

**Chosen:** serve original bytes after bounded structural validation; decode fully only when the
[dark transform](dark-mode.md) needs a classification, cached by content hash. Vector images are
rasterized in the broker or refused.
**Rejected:** decoding and re-encoding every image to a normalized format; passing every type through
untouched.

**Why.** The transform is **opt-in and default-off** under FR-31, so for most users, most of the time,
nothing needs the pixels of anything. Transcoding universally would spend CPU on every image in every
message to serve a feature that is switched off, and would re-encode losslessly-delivered artwork lossily
in the process. Classification results are cached by content address, so when the transform *is* on, a
given image is decoded once ever — which is the property that makes the expensive path affordable.

Passing everything through untouched was rejected for one format rather than all of them. Vector images
are an XML surface with external references and their own scripting story, and handing them to the engine
means handing over a parser this pipeline never inspected. Rasterizing in the broker puts them back under
the same bounded-decode rule as everything else; refusing them is the honest fallback where rasterizing is
not practical.

"Bounded" is load-bearing in both paths: dimensions and resource limits MUST be checked before a decode is
attempted, because a decode bomb reaching Sift is a denial of service on the user's mail under
NFR-19.

**What it costs:** two paths through the broker instead of one, and metadata in original bytes is passed
to the engine rather than stripped. The latter is acceptable because the body view has no network and no
script, so metadata is inert there.

**Contestable because:** universal transcoding would give one decoder path facing hostile bytes, one
format to test, and metadata stripped by construction. If the fidelity corpus shows the engine's decoders
are the weak point rather than Sift's, that argument wins.

## Prefetch

**If the broker prefetches remote images, that prefetch *is* the tracking event.** Prefetch MUST be gated
on the same allow decision as display, and disabled entirely under a constrained policy tier. See NFR-32
in [network conditions](../runtime/network-conditions.md).

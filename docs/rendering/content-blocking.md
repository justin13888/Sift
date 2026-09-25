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

Reloading is a list parse on window open, ahead of any message selection. It was previously described as
falling on NFR-1 rather than on NFR-3; [D-69](../architecture/lifecycle.md) moves it off the cold-start
critical path, because a 40 MB parse is a large fraction of that budget spent preparing an authority the
first painted list does not consult. **What was being protected is unchanged and is the obligation that
matters: the engine MUST be loaded before the first body renders**, and until it is, the tier below
applies — the broker blocks by default, so a body reached before the engine is ready is under-permissive
rather than over-permissive. The compiled engine-level backstop rules are installed with the body view's
configuration and follow the same lifetime.

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
at sanitize time. **The engine returns when pressure has been clear for L-19 and a window is open**
— not when a *new* window opens, which is what this said before and which left a user who keeps one
window open and passes through L1 once with no authority for that window's life.
[D-93](../runtime/memory-pressure.md)'s hysteresis is what makes the two distinguishable: a reload
after sustained clearance is not the reload-on-demand this section refuses, because it is not a
response to a pressure signal.

The failure direction is the one this document chooses everywhere else: a message with missing images
rather than a message that quietly fetched something.

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
rasterized in the broker or refused. **Every parser, decoder and rasterizer that reads image bytes in the
core MUST be memory-safe**, and a format with no memory-safe decoder is served but never classified — see
[where the bytes are decoded](#where-the-bytes-are-decoded) below.
**Rejected:** decoding and re-encoding every image to a normalized format; passing every type through
untouched; decoding in a separate sandboxed process; accepting unsafe decoders in the core.

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
format to test, and metadata stripped by construction. It would also have to decode every format mail
carries, which the memory-safety constraint below does not permit. If the fidelity corpus shows the engine's decoders
are the weak point rather than Sift's, that argument wins.

**The placement question is separate from the transcoding question, and it is the sharper of the two.**
Decoding "in the broker" means decoding in the core, and under [D-2](../architecture/process-model.md) the
core is the single resident process holding every account's sync state, its mutation queue, and credential
material read out of the OS store to be used. The path this decision declined — letting the engine
decode — puts the same hostile bytes in WebKit's own content process: sandboxed, out of process, and
destroyed on teardown under NFR-46.

Read that way the asymmetry in the paragraph above runs opposite to how it reads. The question is not
whether Sift's decoders might turn out better than the engine's. It is that Sift's run **unsandboxed in
the process NFR-19 exists to protect**, while the engine's run in the one component the design is built to
kill and respawn. [D-8](../architecture/overview.md)'s argument for Rust on hostile-input paths applies
here at its strongest, and this is also where mature Rust decoders are least available for the formats
mail actually carries. This was [Q-14](../open-questions.md), and the section below answers it.

### Where the bytes are decoded

**The decode stays in the core, and memory safety is a stated constraint on it rather than a hope.**
Every component that reads image bytes on the broker's path — the structural validation that reads a
container's declared dimensions before [L-11](../limits.md) is checked, the classification decoder, and
the vector rasterizer under [L-12](../limits.md) — MUST be written in a memory-safe language, MUST contain
no unchecked memory access of its own on the path that reads hostile bytes, and MUST NOT call into a codec
written in a memory-unsafe language, **the platform's own image frameworks included**, since those run in
the same process. A dependency that satisfies this does so for its whole decoding path, not for its public
surface.

**A format with no decoder meeting that bar is served and not classified.** Its original bytes still go
to the engine after the same bounded structural validation, and the engine decodes them in its own
sandboxed, disposable content process, exactly as for any image while the transform is off. The dark
transform leaves such an image untransformed, which is the outcome FR-31's asymmetry already prefers when
Sift does not know what an image is. A vector format with no memory-safe rasterizer is refused, which was
already the stated fallback above.

**The narrowing is stated in the interface, not discovered by its absence.** A classification has an
explicit outcome for *not classified because no memory-safe decoder exists for this format*, distinct from
a decode that failed, a decode refused by a bound, and a classification that ran. Each is recorded against
the content hash, so a format is not retried every time it is seen, and each is surfaced with its reason in
the [debug view](../runtime/observability.md). The set of classifiable formats is therefore whatever the
build can decode safely, and it is a property a reader can inspect rather than infer.

**A decoder defect is then a degradation, which is what the threat model requires of every other layer.**
Memory safety turns an out-of-bounds read or write into a panic, and
[D-47](../architecture/overview.md) catches panics at the stage boundary, so the decode is a catch
boundary like any pipeline stage: a caught panic records the image as not classified and the message
renders with that image untransformed. It never reaches the process NFR-19 protects as memory corruption.
The bounds above remain what stops a decode bomb, because memory safety says nothing about how much memory
or time a correct decoder is persuaded to spend.

**Why this over a separate process.** Decoding in a sandboxed helper would put the bytes back in the
disposable half of the system, and it costs a process that [D-2](../architecture/process-model.md) is
built to avoid, for a feature FR-31 leaves off by default. The engine's content process cannot be borrowed
for it either: script is off there, so it can compute nothing on Sift's behalf, and
[D-50](webview-isolation.md) leaves no channel out of the body view to return an answer on. **Why this over accepting the placement.** Accepting it would leave
the one hostile input without a backstop as a known and permanent property of the resident process, when
the cost of closing it is a narrower set of images the transform can reason about — a cost paid only by
users who switched the transform on, only on formats nobody has a safe decoder for, and only as an
untransformed image.

**What it costs, and why it is still contestable.** Memory safety is a weaker boundary than a sandbox. It
excludes memory corruption; it does not exclude a logic defect that misclassifies, nor resource exhaustion
the bounds fail to anticipate, nor a defect in the language's own runtime or compiler. If the formats mail
carries in practice turn out to be mostly ones without a safe decoder, the transform degrades toward doing
nothing to images, and the separate-process answer — with the D-2 cost it carries — becomes the one to
reopen.

## Prefetch

**If the broker prefetches remote images, that prefetch *is* the tracking event.** Prefetch MUST be gated
on the same allow decision as display, and disabled entirely under a constrained policy tier. See NFR-32
in [network conditions](../runtime/network-conditions.md).

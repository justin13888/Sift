# Resource broker

The one component every byte the body view loads must pass through.

**Owns no identifier.** The broker's rules are stated by the documents that own the requirements they
serve, and this page gathers them in one place because they are otherwise spread across five. Where this
page and an owning document appear to disagree, the owner wins — see
[docs/README](../README.md) on one owner per identifier.

## Why it exists as a component rather than a code path

The [body view has no network capability](../rendering/webview-isolation.md) — invariant N-1. Only an
internal scheme is registered; every other scheme is rejected at the engine's policy layer. The
consequence is the structural advantage the whole blocking design rests on: a browser extension must
intercept requests it does not control, whereas **every candidate load in Sift arrives at one place as a
question, before anything is in flight**.

That place is the broker. Blocking is therefore a decision function inside code Sift owns, not an
interception of somebody else's fetch path, which is why per-message decisions can be
[enumerated exactly](../runtime/observability.md) rather than inferred.

The broker sits in the core, below the [shell boundary](shell-boundary.md). Resource loads do **not**
cross that boundary: they arrive from the engine's scheme handler and the broker answers them directly.
No shell is in the path.

## What arrives, and what the broker decides

Every request carries the [capability token](../rendering/webview-isolation.md) of the view that made it
(D-28), which is what scopes it to one message and makes revocation on teardown wholesale.

| Question | Decided by | Owner |
|---|---|---|
| Is this address valid for this view at all? | the per-view capability token; a fabricated or stale address resolves to nothing | [D-28](../rendering/webview-isolation.md) |
| Is this resource allowed? | the filter engine as authority, with compiled engine rules as an independent backstop | [D-10](../rendering/content-blocking.md) |
| Is it first-party? | the synthetic origin, widened to known infrastructure only for attested senders | [D-11, D-37](../rendering/sender-origin.md) |
| Does a heuristic block it regardless of list coverage? | the tracking-pixel and CSS-vector heuristics, each logging its reason | [FR-29](../rendering/content-blocking.md) |
| May it be fetched *now*? | the active network policy tier, and the byte ceiling on a single fetch | [NFR-32, NFR-39](../runtime/network-conditions.md) |
| What bytes are handed over? | original bytes after bounded structural validation; decode only when a classification is needed; vector images rasterized or refused | [D-29](../rendering/content-blocking.md) |

**Disagreement between the authority and the backstop is a bug**, and the
[debug view](../runtime/observability.md) MUST surface it rather than silently taking either answer. That
requirement is what makes the debug view load-bearing rather than a convenience.

## The broker is the enforcement point, and that is a placement decision

Every fetching position in a message reaches the broker, because [the pipeline](../rendering/pipeline.md)
rewrites all of them to the internal scheme at sanitize time — condemned ones included. The blocker's
per-message pass records a verdict for [FR-33](../runtime/observability.md); it does not decide the fetch.

**The decision has to live here rather than in the markup**, because its inputs outlive the document. The
per-sender allowlist under FR-8 changes when the user says "show this sender's images", and the
[network policy tier](../runtime/network-conditions.md) changes when the user walks out of the building.
A verdict baked into a rewritten document at sanitize time could only be revised by re-running the
pipeline, and a per-message decision cache would then need invalidating on both of those events.

**A denied address resolves to a deterministic blocked answer, not to nothing.** The two are different: a
fabricated or stale address resolves to nothing under D-28, and that is a defect being caught, whereas a
denied load is ordinary policy and the reader is entitled to know it happened. Conflating them would make
"this sender is blocked" and "this view has been torn down" the same event.

**The placeholder is inert, and every affordance for acting on it is native reader chrome.** This was
previously written as "a placeholder it can act on", which invited an in-document control — and there is
no channel for one, because [D-50](../rendering/webview-isolation.md) leaves no script and every
navigation is intercepted.

The deeper reason is that the channel should not be built even if it could be. **A clickable control
inside the document is one a sender can counterfeit.** I9 forbids Sift from inventing visible text, and
nothing forbids a sender from authoring a convincing imitation of Sift's own "show images" affordance —
next to which the genuine one would be indistinguishable. The genuine control writes the per-sender
allowlist, which [data model](../storage/data-model.md) calls security state on the grounds that write
access to it is write access to Sift's egress policy. A counterfeit that merely *looks* like it would
train the user to click the one that is not.

So the document shows that something was withheld and shows nothing that responds to a click, and the
reader's own chrome — outside the body view, where the sender cannot draw — carries the count, the reason,
and the two affordances below.

## Three rules that are easy to get wrong

**Prefetching is the tracking event.** If the broker fetches a remote image ahead of display, that fetch
*is* the disclosure the blocking exists to prevent. Prefetch MUST be gated on the same allow decision as
display, and disabled entirely under a constrained tier. The same trap applies to link unwrapping:
resolving a click-tracking wrapper by following it is the same mistake in a different position — see
[link handling](../rendering/link-handling.md).

**Bounded means bounded before the decode, not during it.** Dimensions and resource limits are checked
before any decoder is handed bytes, because a decode bomb reaching Sift is a denial of service on the
user's mail under NFR-19. The bounds are L-10 through L-12 in [limits](../limits.md), and a resource that
exceeds one resolves to the same deterministic blocked answer a denied load does. The broker is where the
hostile-input posture of the [pipeline](../rendering/pipeline.md) continues after sanitization, not where
it relaxes.

**Nothing here is a cache exemption.** The in-memory classification cache is declared, budgeted, and
sheddable like every other — see [memory pressure](../runtime/memory-pressure.md). What makes an image
classified *once ever* is not that cache but the durable record beneath it in the shared blob index, which
the cache reads through to — see [cache and blobs](../storage/cache-and-blobs.md).

## Egress

The broker is the only component that fetches remote content, and it does so **only** for resources the
user has explicitly allowed. That row of the [egress table](../security/privacy.md) is this component; a
fetch from anywhere else in the core for a message resource is a defect.

## Related

- [Body view isolation](../rendering/webview-isolation.md) — N-1, and why the broker is the only channel
- [Content blocking](../rendering/content-blocking.md) — the decisions the broker applies
- [Sender origin](../rendering/sender-origin.md) — how first-party is determined without a document origin
- [Network conditions](../runtime/network-conditions.md) — when a permitted fetch may actually happen
- [Observability](../runtime/observability.md) — where every decision is enumerated

# Sync engine

How Sift learns that mail changed, and what it fetches when it does.

**Owns:** NFR-18.

## Planning against capabilities

The sync engine plans against the delta and push capabilities an adapter declares — never against provider
identity. See [provider model](provider-model.md).

Two mechanisms, kept distinct:

- **Push** answers *"has something changed?"*. It is a doorbell.
- **Delta** answers *"what changed?"*. It is the authoritative change feed, resumed from a cursor.

Separating them means a provider whose push is impractical can still be efficient by polling the delta,
and a provider with excellent push still reconciles through the same delta path. There is exactly one
code path that applies change.

## The fetch discipline

**Sift MUST NOT fetch whole messages.** It fetches structure first, then only the part it has decided to
display. A message carrying a 40 MB attachment MUST cost a few kilobytes until the user asks for the
attachment.

This is the single largest lever on perceived speed and on the network targets in
[network conditions](../runtime/network-conditions.md). Envelopes are cheap and are what makes list
scrolling instant; bodies are fetched on demand; attachments are fetched only on request. See
[cache and blobs](../storage/cache-and-blobs.md) for what is retained.

## The snippet is part of the envelope only where the provider supplies one

FR-6 requires a snippet in every list row and the [glossary](../glossary.md) puts it in the envelope, "the
cheap per-message summary". For three of the four providers it is cheap, because a preview comes back with
the envelope. **For generic IMAP no such field exists**, and the difference has to be declared rather than
discovered — it is the *snippet source* capability in [provider model](provider-model.md).

Where the source is *client-derived*, a snippet MUST be derived only from a body Sift has already fetched
for its own reasons, and MUST NOT cause a fetch. A list row with no snippet is honest; a backfill that
fetches body text for half a million messages to fill one column is a different product, and it would
multiply the initial sync's cost against NFR-15 and NFR-31 for a line of grey text.

**There is a specific hazard at the end of the naive path, and it is why this is normative rather than
advisory.** On IMAP, fetching body text without the peek form of the command **sets the seen flag**. A
snippet derived during backfill without that care therefore marks the user's entire mailbox read — a
data-integrity incident produced by an unstated UI requirement, on the adapter where degradation is
already the norm, and one that no amount of care in [D-52](mutations.md)'s mark-read rule would prevent.

The snippet is derived after part selection and before sanitization is required — it is text, not markup —
and it is stored with the envelope, so it survives body eviction under NFR-14 exactly as the rest of the
envelope does. It is bounded by L-16 in [limits](../limits.md) and is truncated rather than rejected, and
because it is attacker-controlled text destined for a native list row it is normalized under NFR-54 in
[presentation layer](../architecture/presentation-layer.md) like every other such string.

## Scheduling

All periodic sync work MUST funnel through the single coalesced scheduler described in
[scheduling](../runtime/scheduling.md). Per-account or per-folder sleep loops are prohibited — they are
the dominant cause of idle battery drain, and the cost is in the *wakeups*, not the work.

Sync behaviour is further constrained by the active network policy tier, which can suppress prefetch
entirely or disable push in favour of long aligned polls. See
[network conditions](../runtime/network-conditions.md).

## NFR-18 — Cursor invalidation must never require a manual full resync

A full resync from scratch MUST NEVER be required for correctness. When a cursor is invalidated — an IMAP
folder's validity identifier changes, a delta link expires, a history identifier falls outside the
server's retained window — Sift MUST recover automatically **and visibly**.

Both halves matter. Automatic recovery without visibility produces an app that silently spends an hour of
a metered connection re-downloading a mailbox. Visible recovery without automation produces an app that
asks the user to fix something they cannot reason about.

Recovery MUST preserve local state that the server cannot reconstruct: queued mutations, their pending
overlays under [D-51](mutations.md), and cached bodies whose content addresses still match.

That list previously ended with "read state not yet flushed" as a fourth item, which read as a second,
separately durable write path. There is none: marking read is an intent under FR-13, every intent is in
the durable queue under FR-14, and unflushed read state is therefore queued mutations already named. The
phrase invited an implementer to build a batched side-channel with its own durability and its own
precedence against a delta, which is the one thing [D-51](mutations.md) exists to prevent.

## Degradation is explicit

Where a provider lacks an efficient delta mechanism, resynchronization cost is proportional to mailbox
size rather than to change volume. Sift MUST degrade **explicitly and visibly** rather than papering over
the difference — see [IMAP](providers/imap.md) and NFR-29 there. A user on a server without efficient
resync deserves to know why their client behaves differently, and a maintainer deserves a report that
distinguishes "slow" from "slow for a structural reason".

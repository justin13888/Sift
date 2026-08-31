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

Recovery MUST preserve local state that the server cannot reconstruct: queued mutations, cached bodies
whose content addresses still match, and read state not yet flushed.

## Degradation is explicit

Where a provider lacks an efficient delta mechanism, resynchronization cost is proportional to mailbox
size rather than to change volume. Sift MUST degrade **explicitly and visibly** rather than papering over
the difference — see [IMAP](providers/imap.md) and NFR-29 there. A user on a server without efficient
resync deserves to know why their client behaves differently, and a maintainer deserves a report that
distinguishes "slow" from "slow for a structural reason".

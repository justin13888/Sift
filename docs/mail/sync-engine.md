# Sync engine

How Sift learns that mail changed, and what it fetches when it does.

**Owns:** D-53, D-82, D-83, NFR-18.

## Planning against capabilities

The sync engine plans against the delta and push capabilities an adapter declares — never against provider
identity. See [provider model](provider-model.md).

Two mechanisms, kept distinct:

- **Push** answers *"has something changed?"*. It is a doorbell.
- **Delta** answers *"what changed?"*. It is the authoritative change feed, resumed from a cursor.

The delta MUST distinguish a message that **arrived** from one that was merely **discovered** — by a
backfill, or by a cursor recovery re-reading what was always there — and MUST record which, from the first
build. [FR-23](../architecture/ui-shell.md) needs it three phases later and cannot recover it after the
fact without the resynchronization NFR-18 forbids.

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

## D-53 — The first sync is a bounded, resumable, visible backfill

**Chosen:** on adding an account, fetch envelopes newest-first in bounded pages, resumable from a stored
position, with progress visible to the user, obeying the active
[policy tier](../runtime/network-conditions.md) like any other traffic. The extent is bounded by
NFR-52's envelope budget and by nothing else.
**Rejected:** a fixed time window; a fixed message count per folder.

**Why this needed a decision.** Nothing said how much history a new account fetches, and envelopes are the
long-lived tier — so this fixes the permanent floor of every store, the size of the search index NFR-5 is
measured over, and the first thing a user experiences. It is also the first thing that can exhaust an
FR-36 cap, on a connection the user may not have chosen.

**Why the budget is the only bound.** A time window and a message count are each a second limit that can
disagree with the first, and each has to be explained to the user in terms of a boundary they cannot see.
NFR-52 already bounds the envelope tier and already evicts oldest-first; letting it be the single bound
means the backfill simply runs until the budget binds, and the answer to "why is my old mail not here" is
the same budget the user can raise. One limit rather than two.

**Newest-first because relevance decays and interruption is normal.** A backfill that begins at the oldest
message is useless until it finishes; one that begins at the newest is useful immediately and remains
correct if it never finishes at all — which matters because it MUST be interruptible, and because on a
large mailbox it will be.

**Visible for the reason NFR-18 already gives.** That requirement demands recovery be automatic *and*
visible, on the reasoning that automatic-but-silent "produces an app that silently spends an hour of a
metered connection re-downloading a mailbox". A first backfill is that same operation, arriving at a
moment when the user has even less context for it, so it takes the same rule. The account's condition
while it runs is *recovering* in [failure model](../runtime/failure-model.md).

**What it costs:** on a large mailbox the first sync is long, and the product's first impression is a
progress indicator. It also means two accounts of very different sizes reach usefulness at very different
times, with nothing to explain that but the progress itself.

**Contestable because:** a smaller default would make first run faster for everyone and would be invisible
to most users, who never look past a few months of mail. The argument against is that local search is a
headline feature and its coverage would then silently stop at a boundary nobody chose, pushing queries
onto FR-21's server-side path without the user understanding why. A reader who thinks first-run speed
outweighs search coverage should argue for a default window rather than for a different bound.

## D-82 — A folder is a state machine, and the cursor is taken before the backfill

**Chosen:** every watched folder is in exactly one of six states, with the transitions below; a new
folder's delta cursor is acquired **before** its backfill begins, and changes the backfill has not yet
reached are applied against it without ordering hazard.
**Rejected:** the four stored columns as the whole specification; acquiring the cursor when the backfill
completes.

**Why states rather than columns.** [Data model](../storage/data-model.md) stores *"cursor, validity
identifier, last successful sync, degradation reason"* per folder, which is a set of fields and not a
specification: nothing said which combinations are legal, what a folder does on first contact, or how it
gets out of a failure. Two implementers reading those four columns build different engines, and the
difference shows up as mail that never arrives.

| State | Meaning | Leaves to |
|---|---|---|
| **Unsynced** | known to exist, never contacted | Backfilling, on first scheduled turn |
| **Backfilling** | cursor held, walking history newest-first under D-53 | Live on completion; Invalidated if the cursor dies mid-walk; Degraded on a non-transient failure |
| **Live** | cursor current, deltas applied as they arrive | Invalidated, Degraded, or Unwatched |
| **Invalidated** | the cursor is no longer accepted by the server | Recovering, immediately |
| **Recovering** | re-establishing under NFR-18 below | Live on success; Degraded if recovery itself fails |
| **Degraded** | a stated reason, surfaced under NFR-29 | Live when the underlying cause clears |

**Unwatched is not a state here.** A folder the user has removed from FR-43's watched set stops being
scheduled; its stored state is retained so that re-watching does not mean starting from Unsynced, which
would re-run a backfill for mail already held.

**The ordering hazard, which is the reason this decision exists.** D-53 walks history newest-first while
the server keeps changing underneath. Two orderings are available and only one is safe.

- **Cursor first, then backfill.** The cursor is taken at a point in time; the backfill walks history
  behind it; deltas from that cursor forward are applied as they arrive. A change to a message the
  backfill has not yet reached is applied to a row that does not exist yet, and the backfill later
  produces that message in its already-changed form — so the delta is redundant rather than lost.
- **Backfill first, then cursor.** Every change between the start of the walk and the acquisition of the
  cursor is never observed by either mechanism. On a large mailbox that window is hours.

The second loses mail silently, which is the failure [data model](../storage/data-model.md) names when it
says a cursor advancing past unapplied changes *"is silent data loss, and it is invisible until a user
notices a message that never arrived"*. **Cursor first is therefore normative**, and the redundancy it
causes is absorbed by the same rule that makes any delta safe to reapply.

**A delta page is applied and its cursor advanced in one transaction, per page.** The rule that *"advancing
a cursor and applying the change it describes MUST commit in one transaction"* is per page rather than
per response, so a crash in the middle of a multi-page delta resumes at the last committed page rather
than repeating or skipping the whole response. Under [D-74](../storage/data-model.md) this is entirely
within the store, so it remains one transaction.

**Concurrency is the scheduler's, not this document's.** How many folders sync at once is a wakeup and
connection-budget question owned by [scheduling](../runtime/scheduling.md); what this decision requires is
that concurrent folder syncs within one account serialize their **writes**, because
[D-6](../storage/data-model.md) permits one writer per store.

## D-83 — Folders have a lifecycle, and its identity is local

**Chosen:** a folder has a local identity assigned on first discovery; the remote identifier is an
attribute of it. A folder that disappears from enumeration is **retired**, not deleted, and its messages
are treated as no longer present in it.
**Rejected:** keying folders on the remote identifier; deleting a folder's rows when it stops being
enumerated.

**Why the remote identifier cannot be the key.** [Data model](../storage/data-model.md) already refuses
remote identifiers as keys for *messages* — *"one provider's identifiers change on move and IMAP
identifiers are only unique within a folder generation"* — and folders have the same problem with none of
the argument written down. An IMAP rename changes the identifier of a folder whose contents are unchanged,
and [FR-43](../runtime/scheduling.md)'s watched set, the per-folder notification rules in account policy,
and every message's location all point at it. Keyed on the remote identifier, a rename silently unwatches
a folder, drops its notification rules, and orphans its messages' location — none of which reports
anything.

**A rename is therefore a rename.** Where the provider's model makes the identity stable across a rename,
the local folder keeps its identity and only its display name changes. Where it does not — plain IMAP —
the adapter's re-enumeration produces a folder that looks new, and Sift MUST NOT guess: the old folder is
retired and the new one is discovered, which loses the watched flag and the notification rules for that
folder and loses nothing else. **Guessing wrongly here would silently move a user's notification rules
onto a folder they did not choose**, which is worse than the loss.

**Why retirement rather than deletion.** A folder can vanish from enumeration because it was deleted, or
because the server is answering incompletely — the same ambiguity [provider model](provider-model.md)
already resolves for probed capabilities, where *"a failed probe is not an observation"*. Deleting rows on
a bad enumeration would discard envelopes, index entries and any queued intent against them, on evidence
that may be a transient. Retirement keeps the rows, stops scheduling the folder, and lets a later
enumeration restore it.

**Messages in a retired folder are not deleted either.** They stop being present in that location, which
for a provider with one location per message means they are in no location and are shown only through
search and through threads — an honest state, and the one FR-12 already distinguishes. They are evicted by
NFR-52's budget in the ordinary way rather than by a special path.

**A new folder is discovered, not adopted.** It enters Unsynced and is backfilled only if it is in the
watched set; FR-43 makes that set the user's, so a server-side folder creation does not silently add work
or a new source of notifications.

**What it costs:** a local folder identity in every location reference, and a retired-folder state that
must be visible somewhere or it becomes a place mail hides.

**Contestable because:** retiring rather than deleting means a user who deletes a folder on the server
sees its mail linger locally until the budget evicts it, and "I deleted that" is a reasonable complaint.
The alternative is discarding data on a single incomplete enumeration, and the asymmetry between the two
mistakes is what decides it.

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

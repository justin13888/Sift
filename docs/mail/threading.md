# Threading

**Owns:** D-103, FR-11.

## FR-11 — Provider threads where available, reconstruction where not

Where a provider exposes a native conversation identifier, Sift MUST use it. Where it does not — plain
IMAP — Sift MUST reconstruct threads from the message's reference and in-reply-to headers, with subject
normalization as a fallback for messages whose headers are missing or broken.

Reconstruction is a join, so it is bounded by [D-44](../storage/data-model.md): the reference chain is the
scope that supplies its candidates, the fallback digest corroborates the result, and a chain that does not
reduce to one candidate leaves the messages in separate threads rather than guessing at one.

Provider identifiers are preferred not because reconstruction is hard but because the *user has already
seen* the provider's threading in its own web interface, and a client that groups differently reads as
broken even when it is arguably more correct.

## D-103 — A thread has a local identity, threads merge, and they never split

**Chosen:** a thread has a local identity assigned when it is created; a message whose reference chain
links two existing threads **merges** them, the older identity surviving; and a thread **never splits**,
whatever is later removed from it.
**Rejected:** keying a thread on the provider's conversation identifier or on a normalized subject;
splitting a thread when the message that linked it is deleted.

**Why a local identity.** [Data model](../storage/data-model.md) gives the Thread entity a *"remote thread
identifier"*, which plain IMAP does not have — so under reconstruction the entity has no key. It is also
the value [FR-38](mutations.md)'s thread-level intents, the shell's collapse state, and notification
click-through all point at, and every one of those needs something stable across a provider's identifier
changing. Same reasoning as [D-83](sync-engine.md)'s folders and [D-78](../storage/data-model.md)'s
messages, applied to the entity that had it least.

**Why merges happen at all, and why the design forces them.** FR-11 above reconstructs from reference
chains, and mail arrives out of order: two branches of one conversation can arrive before the message
that links them, producing two threads that are later shown to be one. That is not an error path — it is
the ordinary behaviour of a mailing list, and a design that could not merge would leave conversations
permanently split with the evidence to join them sitting in the store.

**The older identity survives**, which under [D-78](../storage/data-model.md)'s time-ordered identities
means the thread the user has seen for longer. The alternative — a new identity for the merged thread —
invalidates whatever the shell was holding for both.

**A merge does not touch the mutation queue**, because [FR-38](mutations.md) fans a thread intent out to
messages, and a thread intent is expanded to its message set **when it is enqueued** rather than when it
is issued. That is stated here because the alternative is genuinely tempting — storing the thread
identity and expanding at flush time is less data — and it makes a merge or an arrival silently change
what a queued intent acts on, which is the class of bug FR-38's own partial-failure rules exist to bound.

**A merge crosses the boundary as a move**, not as deletes and inserts: the retired thread's rows join the
survivor, and [view protocol](../architecture/view-protocol.md) requires moves be expressed as moves
because *"a delete-plus-insert destroys and recreates it, which is visible and which loses the
selection"*.

**Why threads never split.** Deleting the message that linked two branches does not disprove the link —
the reference chains that established it are still in the remaining messages, and re-deriving from what
survives would produce a different answer depending on what happened to be evicted. Splitting would also
strand every identity a user has selected, queued against, or been notified about. **A thread is a claim
about a conversation, not a computed property of the messages currently held**, and D-44's asymmetry
applies here as it does everywhere else: a wrongly merged thread is a display defect, a wrongly split one
loses a conversation the user was following.

**What it costs:** merges are visible, and a user watching a thread appear beside another and then absorb
it will find it odd. The alternative is finding two threads and never learning they were one.

**Contestable because:** never splitting means a thread merged wrongly is merged forever, and under
reconstruction a subject-normalization fallback can merge unrelated mail on a generic subject. FR-11 above
bounds that by requiring corroboration under D-44, so the wrong merge should be rare — but it is
permanent when it happens, and the retreat is to let a user split a thread by hand rather than to compute
splits.

## Threads never span accounts

A thread is scoped to one account. Even when the same conversation exists in two accounts — a common case
for anyone on a mailing list with two addresses — Sift MUST NOT merge them.

Merging would require a cross-account join on a message identifier, and that identifier is not reliable
enough to carry it: some servers and some senders duplicate or omit it. [D-44](../storage/data-model.md)
defines what Sift does instead, and it is scoped to one account precisely because this document is —
there is no corroboration strong enough to make a cross-account join safe. Merging
also collides with [one database per account](../storage/data-model.md) and with the cross-account paging
already required by the [unified inbox](../architecture/presentation-layer.md).

## Consequences for mutations

Whether a thread-level intent is one operation or N is a declared capability, and the partial-failure
semantics of the N case are defined in [mutations](mutations.md).

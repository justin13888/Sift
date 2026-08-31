# Threading

**Owns:** FR-11.

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

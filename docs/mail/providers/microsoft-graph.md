# Microsoft 365 and Outlook.com

Capability notes for the Microsoft Graph adapter. The abstraction it implements is in
[provider model](../provider-model.md).

Consumer and organizational accounts share one code path.

## Declared capabilities

| Capability | Value |
|---|---|
| Location cardinality | exactly one — folders |
| Tag support | read-write — categories |
| Archive semantics | move to the archive well-known folder |
| Trash semantics | move to deleted items |
| Permanent delete | supported |
| Thread operations | native conversation identifier |
| Junk reporting | native report — a report call distinct from the move to the junk folder |
| Delta mechanism | delta link, per folder |
| Push mechanism | poll only |
| ID stability | **unstable on move** |
| Server search | search over mail properties |
| Maximum batch size | **unknown — plans conservatively pending [Q-9](../../open-questions.md)** |

## Delta

A per-folder delta link. Delta links expire; expiry is a cursor invalidation and MUST recover per NFR-18
in [sync engine](../sync-engine.md).

## Push

Graph's change notifications require a publicly reachable endpoint, impractical for a desktop application.
Polling the delta is cheap and is the declared mechanism. Poll intervals MUST be coalesced onto the shared
scheduler — see [scheduling](../../runtime/scheduling.md).

## Unstable identifiers

**The message identifier changes when a message moves between folders.** This is the single most important
fact about this adapter, because it breaks the natural assumption that a remote identifier is a durable
join key.

The adapter MUST treat the internet message identifier together with the conversation identifier as the
join key across a move, and MUST NOT assume the remote identifier survives one. Note that internet message
identifiers are not reliably unique in the wild, so this join cannot assume success. Its fallback is
[D-44](../../storage/data-model.md): the conversation identifier is the scope, the fallback digest
corroborates the candidate, and a move that does not resolve to exactly one candidate is presented as a
delete plus an arrival rather than joined on a guess.

## Special-use folders

Resolved through stable well-known folder identifiers, never through localized display names. See FR-5 in
[provider model](../provider-model.md).

## Client

A hand-written subset covering the mail surface, with CI diffing the types against the published schema,
per [D-13](../provider-model.md). Graph's throttling behaviour — rate-limit responses carrying a retry
delay — deserves bespoke handling rather than a generic retry policy, which is part of why the client is
hand-written.

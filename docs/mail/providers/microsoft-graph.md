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
| Junk reporting | folder move only — the published API has no report call; see [Junk](#junk) |
| Delta mechanism | delta link, per folder |
| Push mechanism | poll only |
| ID stability | **unstable on move** |
| Server search | search over mail properties |
| Maximum batch size | **unknown — plans conservatively pending [Q-9](../../open-questions.md)** |
| Snippet source | provider-supplied |

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
[provider model](../provider-model.md). The stable API's folder resource carries no well-known-name
property, so the adapter resolves each well-known name by asking for the folder it denotes; a name the
account does not have resolves to nothing, and FR-5's prompt-once applies.

## Junk

This row was first written as *native report*. Building the wire protocol found that the stable API binds
no report action to a message: the report call exists only in the provider's preview surface, which is not
supported for production use. So the declared value is **folder move only**, and per
[D-40](../mutations.md) the account is offered a move to the junk folder labelled as a move, never a
report that is secretly one. If the report call reaches the stable API, this row returns to *native
report* by amendment.

## Authorization

A public client with PKCE and no secret, per [D-88](../../security/credentials.md), returning through
Sift's own registered URI scheme under [D-36](../../security/credentials.md) — never a loopback,
which NFR-24 forbids. The identity platform lets a mobile-and-desktop client name its own redirect, so
unlike Gmail the scheme is not derived from the client identifier.

**One authority serves both kinds of account.** The platform's *common* authority admits personal
accounts and work or school accounts alike, and the platform decides which directory holds the one a
person signs in with. Choosing an authority per account type would be the tenant branch D-12 forbids.

**The scope set is two scopes, and it is permanent:**

- the Graph resource's *read-write mail* permission — read, search, and every FR-13 intent this adapter
  declares, including permanent delete. It carries no submission right: the platform puts sending behind
  a permission of its own;
- *offline access* — the refresh token FR-2's silent refresh depends on. Without it the account stops
  working an hour after it is added.

No send permission is ever requested, and neither is a resource's *default* scope, which grants whatever
the client registration statically lists and so would let a send permission added in a portal reach the
consent screen without a code change. Both are declared to the profile as sending scopes, so asking for
either fails a test rather than a review. Widening the set later forces the entire install base through
re-consent, which is why D-88 calls it a minimum.

The authorization asks the platform to show its account chooser, because the browser session is shared
and a person adding a second mailbox would otherwise be handed straight back as the first.

**Revocation is not offered.** The platform has no endpoint for a public client to revoke its own grant,
so FR-4's erasure — which is local and provable — is the whole of removal on this provider.

## Client

A hand-written subset covering the mail surface, with CI diffing the types against the published schema,
per [D-13](../provider-model.md). Graph's throttling behaviour — rate-limit responses carrying a retry
delay — deserves bespoke handling rather than a generic retry policy, which is part of why the client is
hand-written.

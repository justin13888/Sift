# JMAP

Capability notes for the JMAP adapter. The abstraction it implements is in
[provider model](../provider-model.md).

JMAP is the best-behaved of the four providers and is therefore the **first provider implemented**, in the
P1 vertical slice — see [roadmap](../../product/roadmap.md). Building against the cleanest protocol first
means the abstraction is shaped by the protocol that fits it, then stretched by the ones that do not,
rather than being derived from an awkward case and generalized upward.

## Declared capabilities

| Capability | Value |
|---|---|
| Location cardinality | one or more — natively many-to-many mailbox membership |
| Tag support | read-write — keywords |
| Archive semantics | remove inbox from mailbox membership |
| Trash semantics | set mailbox membership to trash |
| Permanent delete | supported |
| Thread operations | native |
| Junk reporting | native report — the standard junk and not-junk keywords |
| Delta mechanism | changes query |
| Push mechanism | event stream |
| ID stability | stable globally |
| Server search | filter conditions in the query, negotiated per server |
| Maximum batch size | **unknown — plans conservatively pending [Q-9](../../open-questions.md)** |
| Snippet source | provider-supplied |

## Delta and push

A first-class changes mechanism, plus a changes query for filtered views, resumable from a state
identifier. Push is a long-polled event stream that needs no publicly reachable endpoint — the only
provider of the four where push is both good and practical.

Multiple method calls batch into a single request, which materially reduces round trips and therefore
wakeups. See [scheduling](../../runtime/scheduling.md).

## Special-use folders

Resolved through mailbox roles. See FR-5 in [provider model](../provider-model.md).

## Client

The specification is the schema; the surface is small enough to implement directly.

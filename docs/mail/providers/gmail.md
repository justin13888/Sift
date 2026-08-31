# Gmail

**Owns:** D-7.

Capability notes for the Gmail adapter. The abstraction it implements is in
[provider model](../provider-model.md).

## Declared capabilities

| Capability | Value |
|---|---|
| Location cardinality | one or more — system labels are locations except where another axis claims them, see [D-12](../provider-model.md) |
| Tag support | read-write — user labels |
| Archive semantics | remove from inbox |
| Trash semantics | move to trash |
| Permanent delete | **not supported** — see the scope section below |
| Thread operations | native |
| Junk reporting | native report — the spam system label, both directions |
| Delta mechanism | monotonic history cursor |
| Push mechanism | **poll only** — see D-7 below |
| ID stability | stable globally |
| Server search | full query syntax, including operators Sift's own grammar does not expose |
| Maximum batch size | **50 — published.** See below |
| Snippet source | provider-supplied |

## Delta

Gmail exposes a true change feed resumable from a monotonic history identifier. This is the delta
mechanism; it MUST be the only path by which change is applied.

The history window is finite. When the stored cursor falls outside it, the account has a cursor
invalidation and MUST recover per NFR-18 in [sync engine](../sync-engine.md).

Envelope fetches MUST request metadata only rather than full messages, and MUST use the batch endpoint.

## Batch size, and the row the capability model does not have

[Q-9](../../open-questions.md) is answered for this provider, and answering it exposed
something worth writing down.

The provider publishes **two** limits. The batch endpoint's hard cap is a hundred calls per
request, with its own guidance for this API being fifty to stay inside the rate limit; a
maximum chosen to be throttled is not a maximum worth planning against, so **50** is declared.
The label-change endpoint accepts a **thousand** identifiers in one call.

[The capability model](../provider-model.md) has one row here, and it governs both envelope
fetches and mutation batches. A single value must therefore be the minimum over the operations
it covers, so mutations pay for the tighter of the two. That is a capability the model does not
have rather than a compromise this adapter made.

The **request budget** stays unknown, and deliberately. The provider publishes it in quota
units per second rather than in requests, and the units differ per method — a history page and
a label change do not cost the same. Converting one into the other would be Sift inventing a
number and calling it published, which is exactly what rule 4 exists to prevent.

## D-7 — Push mechanism

**Chosen:** poll the change feed on the coalesced schedule. There is no doorbell.
**Rejected:** using IMAP IDLE purely as a wake signal, with the delta over the API.

**This decision was amended after the wire protocol was written**, and the thing that changed is a
fact about the provider's authorization model rather than about its protocols.

**IMAP access over OAuth requires the provider's full-mailbox scope, and that same scope authorizes
SMTP submission.** There is no read-only or modify-only scope that admits IMAP. So the hybrid is
reachable only by asking the user, on the consent screen, to grant Sift the ability to send mail as
them — and [D-88](../../security/credentials.md) forbids exactly that, in terms that leave no room: a
permanent minimum scope set with **no send or compose scope ever requested**, so that the no-send
constraint is checkable against an authorization screen. A granted submission capability is an
outbound message path whether or not any code calls it, and [scope](../../product/scope.md) makes that
constraint structural rather than a preference about which functions exist.

The original decision recorded its own contestability — "a coalesced poll of the change feed on the
order of every 30 seconds may be indistinguishable to the user, at half the code and one connection" —
and noted that the polling path "must exist and be good regardless" because the hybrid was disabled on
cellular anyway. That turns out not to have been a preference.

**What it costs.** Notification latency is a poll interval rather than sub-second, on every account
rather than only on cellular ones. Against that: one authentication path, one connection lifecycle,
and no connection held open, so this provider spends none of L-23's budget.

**Contestable because:** the provider's own push mechanism would give sub-second notification with no
IMAP scope at all — but it delivers to a publicly reachable endpoint, which NFR-24 forbids outright.
If a mechanism ever appears that needs neither a listening socket nor a submission scope, this
decision is the one to revisit, and the delta path is unchanged either way.

## Permanent delete, and why it is absent rather than approximated

Immediate permanent deletion is behind the same full-mailbox scope, for the same reason. So FR-13's
ninth intent is **unavailable on this provider**, and the capability table above says so.

This is the capability model working rather than a gap in it. [Mutations](../mutations.md) requires an
unsupported operation be *absent* rather than approximated — the rule it already applies to junk
reporting on an account that does not support it — so the affordance does not appear, and nothing
silently substitutes a move to trash for a request to destroy something.

**What it costs.** A user who wants a message gone rather than trashed must do it in the provider's
own interface. That is a real loss and it is the second thing this scope decision buys.

## Client

A hand-written subset, per [D-13](../provider-model.md) as amended. The published schema declares 79
methods; Sift uses ten of them, and two of the remaining sixty-nine send mail. The endpoints in use are
committed beside the adapter with the schema revision they were transcribed from, and both halves of
D-13's drift check run against that file.

## Authentication gate

Gmail access requires restricted OAuth scopes, and obtaining them is a **business-level blocker that MUST
be resolved before the adapter is written**. See [credentials](../../security/credentials.md).

The scope set is exactly one scope — the provider's *modify* scope — and it is permanent. It reads,
searches, labels, trashes, untrashes and reports junk in both directions. It cannot send, cannot
compose, and cannot permanently delete. Widening it later would force the entire install base through
re-consent, which is why [D-88](../../security/credentials.md) calls it a minimum rather than a
starting point.

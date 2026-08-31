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
| Permanent delete | supported |
| Thread operations | native |
| Junk reporting | native report — the spam system label, both directions |
| Delta mechanism | monotonic history cursor |
| Push mechanism | IDLE as a doorbell, delta over the API — see D-7 below |
| ID stability | stable globally |
| Server search | full query syntax, including operators Sift's own grammar does not expose |
| Maximum batch size | **unknown — plans conservatively pending [Q-9](../../open-questions.md)** |
| Snippet source | provider-supplied |

## Delta

Gmail exposes a true change feed resumable from a monotonic history identifier. This is the delta
mechanism; it MUST be the only path by which change is applied.

The history window is finite. When the stored cursor falls outside it, the account has a cursor
invalidation and MUST recover per NFR-18 in [sync engine](../sync-engine.md).

Envelope fetches MUST request metadata only rather than full messages, and MUST use the batch endpoint.

## D-7 — Push mechanism

**Chosen:** use IMAP IDLE purely as a wake signal — a doorbell — and take the actual delta over the API.
**Rejected:** pure API polling at a short interval.

**Why.** Gmail's own push mechanism requires a publicly reachable endpoint, which is impractical for a
desktop application. IDLE gives sub-second notification without one; the API gives an efficient delta. The
hybrid is the only way to get both.

**What it costs.** Two authentication paths and two connection lifecycles for one account, permanently.

**Contestable because:** a coalesced poll of the change feed on the order of every 30 seconds may be
indistinguishable to the user, at half the code and one connection. This decision MUST be re-examined
against measured perceived latency rather than assumed. Note also that on cellular the hybrid is disabled
outright — IDLE is dropped in favour of long aligned polls, see
[network conditions](../../runtime/network-conditions.md) — so the polling path must exist and be good
regardless.

## Client

A generated client tracking the published schema, per [D-13](../provider-model.md).

## Authentication gate

Gmail access requires restricted OAuth scopes, and obtaining them is a **business-level blocker that MUST
be resolved before the adapter is written**. See [credentials](../../security/credentials.md).

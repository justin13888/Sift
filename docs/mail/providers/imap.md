# Generic IMAP

**Owns:** D-31, NFR-29.

Capability notes for the generic IMAP adapter. The abstraction it implements is in
[provider model](../provider-model.md).

This is the only adapter whose capabilities are **probed rather than known**. Two IMAP servers can differ
more from each other than Gmail differs from JMAP, so the adapter's first job on connect is to determine
what this particular server can do and declare capabilities accordingly.

Probed declarations are held under the rule
[provider model](../provider-model.md) states for them: a probe that fails leaves the last successful
answer standing, and does not silently write the account down to *unsupported*. A capability that really
has gone away is surfaced under NFR-29 below, like every other degradation on this adapter.

## Declared capabilities

| Capability | Value |
|---|---|
| Location cardinality | exactly one — folders |
| Tag support | read-write **only if** the server advertises that it accepts arbitrary keywords; otherwise none |
| Archive semantics | move to the archive special-use folder |
| Trash semantics | move to trash, or set deleted and expunge, depending on the server |
| Permanent delete | supported — set deleted and expunge |
| Thread operations | client fan-out — see [mutations](../mutations.md) |
| Junk reporting | **probed** — native report where the server advertises the junk keywords; otherwise folder move only, or none where no junk special-use folder resolves |
| Delta mechanism | modification-sequence deltas where supported; otherwise full scan |
| Push mechanism | NOTIFY, else IDLE, else poll |
| ID stability | stable per folder, keyed on folder validity plus UID; globally stable only where the server offers object identifiers |
| Server search | the base search command, extended where the server advertises it |
| Maximum batch size | **unknown — plans conservatively pending [Q-9](../../open-questions.md)** |
| Snippet source | **client-derived** — no preview field exists; derived only from a body already fetched, never by fetching one |

## D-31 — A hand-written IMAP client

**Chosen:** a hand-written client covering the read-and-triage subset of the protocol.
**Rejected:** an existing Rust IMAP crate, used as published or forked.

**Why.** This is the same reasoning [D-13](../provider-model.md) applies to Microsoft Graph, and it applies
more strongly here. This is the only adapter whose capabilities are **probed rather than known**, and
NFR-29 requires that the resulting degradation be surfaced to the user with enough specificity to act on.
A general-purpose client abstracts away precisely the detail that requirement needs to expose — which
extensions were advertised, which were tried, and what the server actually did.

The parts that must be exactly right are also the parts a general client is least likely to get right for
this use: the NOTIFY-else-IDLE-else-poll preference, IDLE re-issue before server and NAT timeouts, and the
connection budget in [scheduling](../../runtime/scheduling.md) that decides how many folders may be
watched at all.

A server's responses are also **remote input from a host the user chose but Sift does not trust**, so the
response parser needs the same hardening posture as the rest of the pipeline, and needs it to be
auditable.

**What it costs:** IMAP's grammar is large and its dialects are many, and this is the adapter most likely
to meet a server that is wrong in a way no specification predicts. Hand-writing it means owning every one
of those encounters.

**Contestable because:** the read-and-triage subset is small, but "small subset of IMAP" has defeated
better-resourced projects than this one, and P2 is where that would show up as schedule rather than as
design. Starting from a crate and replacing it incrementally is a legitimate reading of the same facts.

## Delta

Where the server supports modification sequences and quick resynchronization, deltas are cheap and
resumption after disconnect is exact. **Without them, resynchronization cost is proportional to mailbox
size**, not to change volume.

A change in a folder's validity identifier invalidates every UID in that folder and is a cursor
invalidation per NFR-18 in [sync engine](../sync-engine.md).

## Push and the connection budget

IDLE watches one mailbox per connection and MUST be re-issued at L-27 in [limits](../../limits.md) to
survive server and NAT timeouts. NOTIFY watches many mailboxes over a single connection and MUST be
preferred where available.

Where neither is available, or where the connection budget in [scheduling](../../runtime/scheduling.md)
forbids it, Sift watches only the inbox and refreshes other folders lazily on user navigation.

On cellular connections IDLE is dropped entirely in favour of long aligned polls — see
[network conditions](../../runtime/network-conditions.md).

## Special-use folders

Resolved through the special-use extension, with the legacy listing extension as a fallback. Never through
display names. See FR-5 in [provider model](../provider-model.md).

## NFR-29 — Degradation is surfaced, not hidden

Sift MUST degrade gracefully on servers lacking efficient resynchronization or push, **and MUST surface
the degradation to the user**.

A server that forces full-scan resynchronization makes the account structurally slower and more
data-hungry than the others. Hiding that produces an unanswerable support question; showing it lets the
user act — by switching servers, by accepting the cost, or by narrowing which folders are watched.

The account's *degraded* condition is where this reaches the user, and
[failure model](../../runtime/failure-model.md) owns it along with the rest of what can be wrong with an
account.

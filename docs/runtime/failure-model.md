# Failure model

What can be wrong with an account, how the user is told, and what happens when the machine itself fails.

**Owns:** D-49.

## Why this page exists

Ten documents in this set require something to be surfaced to the user. A degraded IMAP server under
NFR-29, a quarantined intent under NFR-48, a disagreement between the filter engine and its backstop under
[D-10](../rendering/content-blocking.md), a bundle replaced on disk under FR-26, a contrast failure under
NFR-47, a failed token refresh under FR-2, a cursor recovering "automatically **and visibly**" under
NFR-18, a conflict notice under FR-16, a partially applied thread mutation under FR-38, and an image
withheld because L1 dropped the blocking authority.

**No document owns the surface they surface to.** Left there, each is implemented by whoever meets it
first, the product ships ten notification idioms, and the one always-on surface FR-22 provides has no rule
for which of them wins when the user has no window open — the situation FR-2 specifically requires a
prompt to reach them in.

Eight distinct account conditions have the same shape: each is implied by a different document, none is
enumerated, and nothing says how two of them compose.

## D-49 — One account condition, one annunciator

**Chosen:** every account has exactly one *condition* at a time, drawn from the enumerated set below and
computed from the independent facts that produce it. It is exposed as one value across the
[view protocol](../architecture/view-protocol.md), and every persistent thing wrong with an account
reaches the user through one surface.
**Rejected:** a flag per failing subsystem, each with its own presentation; a single generic error state.

**Why one value rather than a set of flags.** The shells and the always-on surface need to answer "is
something wrong with this account, and is it my problem" without adjudicating between subsystems. A set of
independent flags pushes that adjudication into two shells, which is the drift the
[presentation layer](../architecture/presentation-layer.md) exists to prevent, and it makes the tray's
behaviour a function of which flag happened to be set last.

**Why not one generic error either.** The conditions differ in what the user can do, which is the only
thing a status is for. *Needs authentication* is an action; *server is degraded* is an explanation;
*paused* is a thing the user did. Collapsing them produces a badge that means "something", which trains
the user to ignore it — the same failure [D-38](../mail/mutations.md) identifies when it refuses to
animate every reconciliation.

**What it costs:** a precedence order has to exist and be defended, and a condition computed from several
facts can hide a second one behind the first. The debug panel under FR-34 is where all the underlying
facts stay visible, which is what keeps the composition from being lossy in practice.

**Contestable because:** it assumes the conditions are mutually exclusive in the user's mind, and an
account that is both over its data cap and pointing at a degraded server has two problems that a single
value shows one of. If that pairing turns out to be common rather than rare, the answer is a primary
condition plus an explicit secondary list, not a return to flags.

## The conditions

Ordered by precedence: the first that applies is the account's condition.

| Condition | Set by | Cleared by | What the user can do |
|---|---|---|---|
| **Needs authentication** | a token refresh that failed non-transiently, FR-2 | a successful interactive re-authentication | re-authenticate; this is the one condition that MUST reach them with no window open |
| **Storage unavailable** | the credential store or the account database being unreadable — see below | the underlying access succeeding | retry, or unlock the credential store |
| **Paused by the user** | the always-on surface's pause, FR-22 | the user resuming | resume |
| **Paused by the data cap** | cumulative usage reaching the FR-36 cap | the accounting period rolling over, or the cap being raised | raise the cap, wait, or switch networks |
| **Recovering** | a cursor invalidation under NFR-18, or an initial backfill still running | recovery completing | nothing, and that is the point — it is progress, not a fault |
| **Degraded** | a probed capability that has gone away, or a server without efficient resynchronization, NFR-29 | a probe succeeding | narrow the watched folder set, accept the cost, or change servers |
| **Attention** | a quarantined intent under NFR-48, or a mutation whose failure FR-16 could not resolve unambiguously | the user acting on the item | act on the queued item |
| **Healthy** | — | — | — |

**The three pauses are three conditions, not one flag.** A user pause, a cap pause, and
[offline](network-conditions.md)'s absence of a path are distinguishable, they clear differently, and only
one of them is something the user did. Collapsing them means "resume" has three meanings and the surface
cannot say which one applies — and the user who paused sync deliberately gets the same badge as the user
whose train went into a tunnel.

**Offline is deliberately absent from this table.** It is a property of the network rather than of an
account, it is already a [policy tier](network-conditions.md), and every account shares it. An account
that is merely offline is not an account with something wrong with it.

**What this document does not own.** The conditions below are the account-scoped set. Every other
identified state the layer can raise — reconciliation notices, blocking reasons, not-cached versus
not-available, a quarantined intent, a caught panic — lives in the
[state register](../architecture/state-register.md) under [D-68](../architecture/state-register.md),
which refers to this table rather than copying it.

## Transient degradation is a notice, not a condition

A condition is standing: it persists until something clears it, and the user can act on it. The rest of
the "MUST be surfaced" list above is **episodic** — one conflict, one partially applied thread mutation,
one blocked image, one blocker disagreement — and belongs in the non-blocking notice FR-16 already
requires and in the per-message record FR-33 already collects, never in the account's condition.

The distinction matters because it is what stops an account showing a permanent badge for a thing that
happened once. It is the same judgement [D-38](../mail/mutations.md) makes about animation, applied to
status.

**A notice names its cause in the user's terms or it is not worth posting.** An image withheld because L1
dropped the filter engine MUST say the shed rather than naming a rule, which
[memory pressure](memory-pressure.md) already requires and which nothing else would have delivered.

## Durability ordering

**An intent MUST be durably enqueued before it is applied optimistically to local state**, not after.

FR-14 requires both and does not order them, and the order is the whole difference. Apply first and the
user watches the archive succeed; if the enqueue then fails — the disk is full, the store is unwritable —
the queue does not have the mutation and the local state says it happened. That is precisely the failure
[data model](../storage/data-model.md) says the durable queue exists to prevent, arriving through the gap
between two clauses of one requirement.

Enqueue first costs a durable write inside NFR-7's 16 ms budget for feedback. That budget is for the
*local* application before any network round trip, and a write-ahead append to a store already running in
that mode is affordable within it; the requirement was never a promise to skip disk.

## When the machine fails

Four situations are not covered anywhere else in this set, and each needs an answer before the code that
meets it is written.

**The disk is full.** Sift MUST stop accepting mutations and enter *storage unavailable* rather than
applying them optimistically and failing to record them. Sync stops writing; reading from what is already
cached continues. NFR-14's budget bounds Sift's own use and says nothing about the volume, so this is
reachable with the cache well inside its cap.

**An I/O error interrupts a sync.** The affected folder stops advancing and records its reason in the
per-folder sync state [data model](../storage/data-model.md) already carries; the account does not stop.
A cursor is never advanced past changes that were not applied, which that document already requires in one
transaction. Repeated failure on one folder surfaces as *degraded*, because a folder that has silently
stopped syncing while the account looks healthy is the failure this whole page is about.

**A database fails to authenticate.** [D-42](../storage/encryption.md) authenticates every page, so a
failed verification is indistinguishable from tampering and MUST NOT be treated as a recoverable read
error. The two adjacent cases already have rules — [D-32](../storage/data-model.md) refuses to open a
newer schema and drains the queue first, and [D-43](../storage/encryption.md) discards an orphaned blob
store wholesale — and this one takes the same shape: the account enters *storage unavailable*, the queue
is drained or exported before anything is destroyed, and recovery is removal and resync. **Draining first
is not optional**, because a queued mutation is the one thing a resync cannot restore.

**The credential store is unavailable at launch.** Every account key and D-43's per-installation secret
live there, so Sift can decrypt nothing without it — and under
[process model](../architecture/process-model.md) it may be resident with no window in which to ask.
Sift MUST enter *storage unavailable* for every account, MUST NOT retry in a loop, and MUST raise the
request through the always-on surface, exactly as FR-2 requires for a failed refresh and for the same
reason. A retry loop here is both futile and a wakeup
source counted against NFR-11.

This is a different question from [R-10](../open-questions.md), which asks whether the Flatpak secrets
portal supplies what is needed at all. This is the ordinary runtime state on both platforms where the
mechanism exists and the answer right now is *not yet* — a keychain the user has not unlocked, a secret
service that has not started.

## Related

- [Credentials](../security/credentials.md) — FR-2, and the re-auth prompt this page routes
- [Network conditions](network-conditions.md) — the policy tiers, including pause
- [Observability](observability.md) — FR-34, where the underlying facts stay visible
- [View protocol](../architecture/view-protocol.md) — how a condition reaches a shell

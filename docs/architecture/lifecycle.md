# Lifecycle

What happens between launch and an interactive list, what happens on the way out, and what is true when
a subsystem never starts.

**Owns:** D-69.

[Process model](process-model.md) settles that there is one resident process and what FR-25's two exits
mean to a user. It does not say what the process does when it starts, in what order, or what "started"
means — and NFR-1 is a phase gate measured against exactly that. The words *startup*, *initialization
order* and *shutdown sequence* appeared nowhere in this set, in a design whose most-cited number is a
400 ms budget over a sequence assembled from six other documents.

## D-69 — Startup is two phases, and only the first is on NFR-1's clock

**Chosen:** launch establishes a **critical path** — everything required to paint real rows and accept
input — and defers everything else to a **background phase** that runs after the list is interactive.
The critical path is per-account and parallel across accounts; the deferred phase is serialized behind
it and MUST NOT block input.
**Rejected:** initializing every subsystem before the window is shown; lazily initializing everything and
letting the first user action pay for it.

**Why the sequence needed writing down at all.** Assembled from the documents that own each step, a cold
launch contains: unlocking the credential store; opening N account databases through
[D-42](../storage/encryption.md)'s page decryption; running any forward-only migration under
[D-32](../storage/data-model.md); the **refcount rebuild** [data model](../storage/data-model.md)
requires *"after abnormal termination"* and does not bound; opening the two installation-scoped stores;
parsing the filter lists, which [content blocking](../rendering/content-blocking.md) explicitly places
*"on window open, ahead of any message selection, so it falls on NFR-1"*; detecting network conditions,
for which NFR-30 allows two seconds; arming the scheduler; and querying the first screen of rows.

Nothing said which of those NFR-1's 400 ms contains. A build that hits the target by deferring the
migration and one that hits it by parallelising database opens are different architectures, and the
second cannot be retrofitted onto the first once every subsystem has assumed it is already initialized.

**What the critical path is.** Reaching the state
[reference environment](../product/reference-environment.md) already defines as the end of NFR-1's
measurement — *"the first screen of rows is painted from real data and accepts input"* — requires the
credential store, the account keys, the account databases open, any migration those databases need, and
one query. Nothing else.

**What is deferred, and why each is safe to defer.**

| Deferred | Why it is not on the critical path |
|---|---|
| Filter-list parsing | It is bound to window lifetime under NFR-42 and is needed before a *message body* renders, not before a *list* paints. Deferring it is safe because the [resource broker](resource-broker.md) is the authority and blocks by default, so a body that renders before the engine is ready is under-permissive rather than over-permissive |
| Network-condition detection | NFR-30 permits two seconds and Conservative is the safe default: [D-14](../runtime/network-conditions.md) already requires an unknown metered state to map to Conservative rather than Unrestricted, so a not-yet-detected network behaves as a cautious one |
| The refcount rebuild | It is a repair path for blobs, and its worst outcome is disk that eviction was going to reclaim anyway. It MUST NOT gate a list of envelopes |
| Sync, push and the queue flush | Residency is not interactivity. A cold list painted from the store is correct before any network call |
| Contact resolution | [D-41](presentation-layer.md) resolves on demand, and the permission for it is requested at first use rather than at launch |

**The filter engine's deferral is the one that changes a stated fact.**
[Content blocking](../rendering/content-blocking.md) puts the load *on* NFR-1 as an accepted cost. This
decision moves it off, and the reason is that the cost is real and the coupling is not: a 40 MB list
parse is a large fraction of a 400 ms budget spent before the user can see anything, to prepare an
authority the first painted list does not consult. What the earlier statement was protecting is that the
engine be ready before the *first body* renders, and that obligation is unchanged and now stated where
it belongs.

**Parallel across accounts, serialized within one.** [D-6](../storage/data-model.md) gives each account
its own database precisely so that they do not contend, and [D-19](overview.md)'s runtime is what makes
opening five of them concurrently free of hand-balancing. Within an account the order is fixed — key,
open, migrate, query — because each step's input is the previous step's output.

**A migration is on the critical path, and that is deliberate.** It is the one deferred-looking step that
cannot be deferred: [D-32](../storage/data-model.md) is forward-only, so a build that painted rows from a
store it had not yet migrated would be reading a schema it does not understand. NFR-1 is a p95 over cold
launches on a populated store, not over the one launch that follows an upgrade; **a launch that runs a
migration is excluded from NFR-1's measurement**, and that exclusion is stated here rather than left to
whoever first sees an outlier.

**What it costs:** two phases mean two states the rest of the code can observe, and a subsystem that
reads another's state during the background phase can find it absent. That is the failure this decision
creates, and it is why the deferred set above is enumerated rather than described.

**Contestable because:** deferring the filter engine trades a slower first *body* for a faster first
*list*, and nobody has measured which the user notices. If the engine's load turns out to be fast enough
to sit inside NFR-1, this decision buys complexity for nothing — but it is cheap to reverse in that
direction and expensive in the other, which is why it is drawn here.

## Related

- [Process model](process-model.md) — D-2, FR-25's two exits, and FR-26
- [Reference environment](../product/reference-environment.md) — what "cold start" and "interactive"
  mean as a measurement
- [Memory pressure](../runtime/memory-pressure.md) — the tiers, whose L3 is the other way every window
  disappears
- [Failure model](../runtime/failure-model.md) — the conditions a failed subsystem resolves to

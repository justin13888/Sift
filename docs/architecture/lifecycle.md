# Lifecycle

What happens between launch and an interactive list, what happens on the way out, and what is true when
a subsystem never starts.

**Owns:** D-69, D-70, D-71, D-72.

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

## D-70 — Quitting is bounded, and it flushes nothing

**Chosen:** a quit tears down in a fixed order — stop accepting intents, tear down the body view, close
the stores, exit — under a stated bound, after which the process exits regardless. **No provider call is
awaited and no queue is flushed.**
**Rejected:** draining the mutation queue before exit; awaiting in-flight network calls; an unbounded
graceful shutdown.

**Why nothing is flushed, which reads wrong and is right.** [FR-25](process-model.md) calls the
close-versus-quit distinction *"the single most likely source of user distrust in the whole design"*, so
the instinct is to make quit do as much as possible. That instinct produces a slow quit and buys nothing,
because the property it is reaching for is already guaranteed: under
[failure model](../runtime/failure-model.md) an intent *"MUST be durably enqueued before it is applied
optimistically"*, so by the time the user has seen a triage action take effect, it is on disk. There is
no window in which quitting loses a mutation the user watched succeed.

An unflushed queue is therefore the ordinary state, not an error state, and quitting with one is exactly
the state the process is in every time [NFR-16](../mail/mutations.md) is exercised by an abrupt
termination. **A quit that did something a crash cannot do would mean the crash path was never the
supported one**, which is the opposite of what NFR-16 asserts.

**Why in-flight provider calls are abandoned rather than awaited.** A call already sent has an unknown
outcome, and waiting does not make it known — it makes it slower and then still unknown, because the
answer may never come. The queue already has the machinery for this: NFR-17 requires the adapter to
reconcile against server state before retrying rather than blindly replaying, and that path runs on the
next launch regardless of how this one ended.

**What quit MUST do, in order:** stop accepting new intents; tear down the body view, so no engine
process outlives the one that owns it; check-point and close the stores, which is the only step whose
omission costs anything, and costs recovery time rather than data; release the credential material held
in memory; exit.

**A quit MUST NOT block on [D-48](view-protocol.md)'s cancellation.** Cancellation rendezvous with
worker-side work and can wait; a quit that cancelled every live observation first would make the exit
path's duration depend on whatever query happened to be running. Teardown discards observations by
advancing their generation, which is [D-66](view-protocol.md)'s mechanism used in the direction it is
already correct for.

**The bound exists because the ordered path can hang.** A store that will not close, a body view that
will not tear down, and a blocking-pool task that will not return are all possible, and a quit that waits
forever is the behaviour FR-25 exists to prevent wearing the opposite costume — the user asked to quit
and the application did not. When the bound expires the process exits, which is a crash by another name,
which NFR-16 already covers.

**What it costs:** an unclean exit is a supported outcome rather than an exceptional one, so the recovery
path runs often enough to be ordinary — which is the argument for it, since a recovery path that never
runs is one nobody has tested.

**Contestable because:** it makes "quit" and "kill" nearly the same operation, and a reader may
reasonably want the deliberate exit to be the tidy one. The answer is that tidiness here would be
decorative: it would improve the case that is already safe and leave the case that is not.

## D-71 — A failure that is not an account's belongs to the process, and some refuse rather than degrade

**Chosen:** a second, process-scoped condition set beside [D-49](../runtime/failure-model.md)'s
account-scoped one; a subsystem whose absence weakens a **security** guarantee causes a refusal, and one
whose absence weakens a **resource or feature** guarantee causes a visible degradation.
**Rejected:** widening the account conditions to carry process failures; treating a failed subsystem as a
condition of every account at once.

**Why account conditions cannot carry these.** D-49 makes the annunciator per account deliberately, and
its own text puts offline outside the table because it is *"a property of the network rather than of an
account"*. The same reasoning excludes every subsystem failure: the filter lists failing to parse, the
body view's content process failing to spawn, the pressure-signal source failing to subscribe, the timing
wheel or the tagging allocator failing to start, and the URI-scheme registration not installing — which
[credentials](../security/credentials.md) already concedes is *"a first-run failure with no obvious
diagnosis"* and then specifies no behaviour for. Reporting any of them as a fault of all five accounts
would be false, and reporting them nowhere is what the design did.

**The split, and why it is not uniform.**

| Subsystem absent | Kind | Behaviour |
|---|---|---|
| The credential store | security | **Refuse.** Already stated: [failure model](../runtime/failure-model.md) enters *storage unavailable* for every account |
| The filter engine's lists | security | **Degrade, visibly and safely.** [Content blocking](../rendering/content-blocking.md) already rules that an absent authority denies, so the failure direction is a message with missing images. The process-scoped condition is what says so rather than leaving it silent |
| The body view | feature | **Degrade.** A message falls back to [FR-9](../rendering/pipeline.md)'s plain-text and raw views, which exist and are the same answer D-47 gives for a failed pipeline stage |
| The pressure-signal source | resource | **Degrade, and say so.** Sift then runs with no governor while claiming NFR-8 and NFR-9, which it cannot honour; caches keep their own budgets, so the loss is the shed tiers rather than every bound |
| The scheduler's timing wheel | resource | **Refuse.** Without it there is no coalesced work at all, and the fallback an implementer would reach for is the per-account sleep loop [scheduling](../runtime/scheduling.md) prohibits outright |
| The URI-scheme registration | security | **Refuse to begin an authorization**, and say why. [D-36](../security/credentials.md) returns the callback through that scheme; starting a flow whose answer cannot arrive teaches the user their credentials failed |

**Why security absences refuse and feature absences degrade.** A degraded feature is a smaller product;
a degraded security guarantee is the same product making a claim that is no longer true. This set's
constraints are stated as absolutes — no script, no unrequested egress, credentials only in the OS store,
never a listening socket — and a build that keeps running with one of them unenforceable is not degraded,
it is wrong. The filter engine is the instructive case: it looks like a security subsystem and its
absence is safe, because the design already made the failure direction deny rather than allow.

**What it costs:** a second condition scope, which is a second thing a shell renders and a second place a
reader has to look before answering "what can be wrong".

**Contestable because:** two scopes invite a third, and the honest risk is that "process condition"
becomes where anything awkward is filed. The defence is that the set above is closed and each entry names
the guarantee it protects; an entry that cannot name one does not belong.

## D-72 — View state is durable, and losing it is never an error

**Chosen:** window count and geometry, each window's folder or account, sort and grouping, and the
reader's open message are **durable installation policy**, written through the boundary by the shell and
restored on launch. Scroll position and an in-progress search are **not** restored.
**Rejected:** holding view state only in a live shell; restoring everything including transient state.

**Why anything is restored.** [L3](../runtime/memory-pressure.md) destroys every window and repaints
*"from cold on next activation"*, and [presentation layer](presentation-layer.md) forbids the layer to
hold *"state that only a live shell can reconstruct"*. Both are satisfied by losing all of it, which is
what the design said, and which means a memory-pressure event silently rearranges the user's workspace.
The same gap covers quit-and-relaunch, where NFR-1's *"interactive list"* presupposes some restored
context and never said which.

**Why durable rather than in-layer.** Making it durable is what satisfies the presentation layer's rule
rather than bending it: state read back from the store is not state only a live shell can reconstruct.
[Data model](../storage/data-model.md) enumerates nine kinds of durable user decision and this was not
among them; it is installation-scoped because a window is not an account's.

**Why scroll position and a running search are excluded.** They are the two whose restoration is worse
than their loss. A restored scroll position in a list that has changed underneath is a position in
different mail; a restored search is a query re-run against a store that has moved on, presented as
though the user had just typed it. Both fail by looking correct.

**Losing view state is never an error, and MUST NOT produce a state.** It is the one durable thing in
this design whose absence has an obviously right answer — one window, the default folder, default sort —
so a shell that finds none restores that and says nothing. This is the exception to the register's
general rule, and it is stated because the alternative is an error nobody can act on.

**What it costs:** writes on window movement, which MUST be coalesced rather than issued per event, for
the reason [scheduling](../runtime/scheduling.md) gives about everything else that happens continuously.

**Contestable because:** restoring the reader's open message means a launch that opens a message, which
runs the pipeline and spawns a body view on the cold-start path NFR-1 measures. The alternative — restore
the folder and not the message — is defensible and loses the thing users most often want back.

## Related

- [Process model](process-model.md) — D-2, FR-25's two exits, and FR-26
- [Reference environment](../product/reference-environment.md) — what "cold start" and "interactive"
  mean as a measurement
- [Memory pressure](../runtime/memory-pressure.md) — the tiers, whose L3 is the other way every window
  disappears
- [Failure model](../runtime/failure-model.md) — the conditions a failed subsystem resolves to

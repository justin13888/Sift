# Mutations

The only writes Sift performs, and the subsystem that performs them.

**Owns:** D-38, D-40, D-51, D-52, FR-13, FR-14, FR-15, FR-16, FR-17, FR-18, FR-38, FR-39, NFR-16, NFR-17.

Budget for this as a first-class subsystem, not a thin adapter method. It is where "read-only plus triage"
quietly becomes expensive, and it is the only place in Sift where a bug can lose a user's mail.

## FR-13 — Mutations are intents, never wire operations

The permitted mutations are: archive, delete to trash, **permanently delete**, move to folder, mark read
or unread, flag or star, add or remove a tag, and report junk or not-junk (FR-39). Each is expressed as a
**provider-agnostic intent** — for example, *archive this thread* — and resolved to wire operations
inside the adapter.

The reason is that the wire operations diverge violently for identical user meaning:

| Intent | Gmail | Graph | JMAP | IMAP |
|---|---|---|---|---|
| Archive | remove the inbox system label | move to the archive well-known folder | remove inbox from mailbox ids | move to the archive special-use folder |
| Delete | add the trash system label | move to deleted items | set mailbox ids to trash | move to trash, or set deleted and expunge |
| Permanent delete | delete, bypassing trash | permanently delete | destroy | set deleted and expunge |

Modelling these as wire operations at the call site would put a provider `match` in the UI, and every new
provider would then edit every call site. See [provider model](provider-model.md).

## Permanent delete is an intent because a capability already promised the affordance

[Provider model](provider-model.md) declares a **permanent delete** capability whose stated meaning is
"whether a destructive delete affordance is offered at all", and all four adapters declare it supported.
An affordance that no intent realises is a capability nothing reads — the row would be decoration, and a
future adapter declaring it unsupported would change nothing. Naming the intent is what makes the
capability load-bearing.

It is the one mutation FR-15 cannot cover, and that is a property of the operation rather than a gap in
the undo model: there is no compensating intent for destruction, because the message is gone from the
server. **Permanent delete MUST therefore be confirmed before it is issued rather than undone after**, and
it MUST NOT be optimistically applied ahead of the server under FR-14 — the whole point of optimism is
that the local state can be corrected when the server disagrees, and here it cannot. This is the single
exception to FR-14, and it is stated here rather than as a footnote because it inverts that requirement
rather than qualifying it.

It is deliberately not offered as a bulk operation over search results under FR-17. Bulk destruction over
a result set the user has not read is the one place where a wrong query and a wrong click compose into
unrecoverable loss, and the product this documentation set describes has no reason to make that cheap.

## D-40, FR-39 — Reporting junk is an intent, and it is not a move

**FR-39.** Report-junk and report-not-junk MUST exist as intents wherever an account declares support for
them, and MUST be absent from the UI where it does not — the ordinary capability rule from
[provider model](provider-model.md), not a special case.

**Chosen:** model junk reporting as its own intent pair, resolved per account against a declared
**junk reporting** capability with three values: *native report*, *folder move only*, or *none*.
**Rejected:** treating "report spam" as a move to the junk location; omitting it from the mutation set
entirely.

**Why.** Modelling it as a move is wrong in the only way that matters to the user: a move relocates one
message, while a report **trains the provider's classifier** so that similar mail is caught before it is
ever delivered. Those are different operations with different durable effects, and every provider Sift
targets exposes the second as something other than a folder change — a system label on Gmail, a report
call alongside the move on Graph, `$junk` and `$notjunk` keywords in JMAP, junk keywords or a special-use
move on IMAP. A client that quietly downgrades a report to a move gives the user a worse mailbox forever
and reports success while doing it.

Omitting it entirely was the other option, and it fails the product statement. Triage is the whole
product, and on most real mailboxes junk is the second most common triage action after archive. A client
whose mutation set is weaker than the web interface it replaces gives the user a reason to keep the web
interface open, which is the outcome this product exists to avoid.

**Not-junk matters as much as junk**, and is the half clients usually skip: rescuing a false positive
without telling the provider leaves the classifier just as wrong tomorrow. It is also what makes FR-15's
undo work here with no new machinery: the two intents compensate each other, which is exactly the
compensating-intent model rather than an exception to it. What undo cannot do is un-train the classifier,
so the undo window returns the message and reports not-junk, and does not promise more than that.

**Where it does not exist, it is absent, not approximated.** An account declaring *folder move only*
offers a move to the junk location under FR-13, labelled as what it is; it MUST NOT present a report
affordance that silently does something else. An account declaring *none* offers neither.

**What it costs:** one more capability, one more intent pair through the queue, undo, bulk and thread
fan-out paths, and the acknowledgement that reporting is the one mutation whose effect Sift cannot verify
— the classifier's response is invisible to a client.

**Contestable because:** it widens FR-13's mutation set, which this documentation set otherwise treats as
closed, and it is the one place the scope boundary moved rather than being clarified. A reader who thinks
triage means only "where does this message go" should argue for the *folder move only* value applying
everywhere and this decision reducing to a label.

## The intent set is closed today and has already grown once — here is how it grows again

FR-13 calls its set closed, and D-40 above widened it anyway. That is not a contradiction so much as a
missing rule: [provider model](provider-model.md) wrote three normative rules for growing the capability
table and this document wrote none for growing the intent set, even though the two grow together and
D-40 grew both at once. The rules below are that omission closed, and they are deliberately the same
shape, because an implementer who has read one should not have to guess at the other.

**A new intent arrives with a capability that gates it.** Never as an intent every adapter is assumed to
support. The capability follows [provider model](provider-model.md)'s first rule — absent means
unsupported, so the affordance is absent — which is what let D-40 add junk reporting without touching an
adapter that does not offer it. An intent with no gating capability is a claim that all four providers,
and the fifth nobody has written yet, can realise it.

**A new intent declares its compensation, and whether it gets a timed window.** FR-15 below is keyed on
exactly these two answers, and permanent delete shows what the first one costs when it is *none*: no undo,
confirmation instead, and an explicit exclusion from FR-14 and NFR-7. An intent that quietly answers
neither is how the undo window stops covering part of the product without anyone deciding that it should.

**A new intent is a schema change to the queue, and behaves like one.** Serialized intents outlive
upgrades under [NFR-48](../storage/data-model.md), so a build predating an intent can meet one in a queue
it otherwise reads. That document already requires such an intent to be quarantined and surfaced rather
than dropped or guessed at; naming the coupling here is what makes it a step in adding an intent rather
than a rule someone has to remember.

**Widening the set is a scope change and says so.** [Scope](../product/scope.md) lists the permitted
mutations, so the two documents move together or they disagree. D-40 is the precedent: it argued the case
in the open, recorded that it moved the boundary, and amended both.

Together these mean a sixth intent lands as a new capability row, a new intent, and a compensation — with
no migration for queues that already exist, and no adapter changed that does not offer it. That is the
same property the capability model is for, applied to the other half of the pair.

## FR-14 — Optimistic application, durable queue

Intents MUST be applied to local state immediately, giving UI feedback before any network round trip
(NFR-7, see [UI shell](../architecture/ui-shell.md)), and MUST be enqueued in a **durable queue that
survives process death and offline periods**. Permanent delete is the one exception, for the reason given
above: an optimistic state that cannot be corrected is not optimism.

The queue is part of the store, not memory. See [data model](../storage/data-model.md). The intent is
durably enqueued **before** it is applied locally, which [failure model](../runtime/failure-model.md)
orders and explains.

## D-51 — Optimistic state is an overlay, and a delta applies underneath it

**Chosen:** a message's local state is a **base**, which the authoritative delta writes, plus a **pending
overlay** contributed by intents that have been enqueued and not yet resolved. Everything the user sees
reads through the overlay; the delta never writes through it. An overlay entry is retired when its intent
succeeds, is compensated, or is reconciled under D-38.
**Rejected:** one materialized state that both the delta and the optimistic apply write to.

**Why this needed deciding at all.** FR-14 applies an intent to local state immediately; the
[sync engine](sync-engine.md) calls the delta "the authoritative change feed" and says "there is exactly
one code path that applies change". Both are right, neither yields, and nothing said what happens when
they meet. They meet constantly: a delta describing the state *before* an archive routinely arrives after
the archive, because the request that produced it was in flight when the user acted.

The materialized answer is the one an implementer reaches for, and it loses triage. The user archives, the
row leaves the list, a delta computed before the archive lands, the row returns — and D-38, reading a
divergence it did not cause, classifies it as **concurrent change** and corrects it *silently*. So the
archive silently undoes itself. D-38's own text calls that outcome "how a client loses a user's trust in
exactly the operation it exists to perform"; it would be reached not because the reconciliation policy is
wrong but because the ordering rule underneath it was missing.

The overlay makes the two writers non-competing. The delta is free to be authoritative about what the
server says, which is what NFR-18's recovery and every adapter's applier need. The overlay is free to be
authoritative about what the user did, which is what FR-14 and NFR-7 promise. Divergence becomes a
comparison between them at a defined moment rather than a race between two writers to the same field.

**It is also what makes D-38 implementable.** That decision requires distinguishing "this changed under
me" from "my own operation did not land", and records the cost as "the queue must retain enough about each
intent to distinguish" them. The overlay is where that is retained: an intent whose overlay entry is still
present when the server reports a conflicting state failed, and one whose entry was already retired
describes a change made elsewhere.

**What it costs.** The [data model](../storage/data-model.md) has to represent it, and a read of a message
is a read of two things rather than one — on the path NFR-2's 50 ms folder switch and NFR-6's scrolling
both run through. Every read path in the [presentation layer](../architecture/presentation-layer.md) reads
through the overlay, so a path that forgets to is a bug that shows the user the server's opinion instead
of their own.

**Contestable because:** the overlay is small and short-lived in the ordinary case — most intents resolve
in under a second — so a reader may reasonably argue that a materialized state plus careful sequencing at
the applier would do, at less cost on the read path. The answer is that "careful sequencing" is a property
nothing can test for, and its failure mode is silent and indistinguishable from ordinary reconciliation.

## D-52 — A message is read when the user reads it

**Chosen:** read is set when the user selects a message in the reader and it remains selected past a short
dwell. Never on list traversal, never on hover, and never on a message whose body was not rendered —
opening a thread marks read only the messages actually shown. The dwell is configurable, including off.
**Rejected:** marking read immediately on selection; never marking read automatically.

**Why this belongs here rather than in a shell.** Marking read is an FR-13 intent, so its trigger decides
how many intents the highest-frequency mutation in the product generates. That number sets the queue's
write rate, the flush wakeup rate against NFR-11 and NFR-15, and the data-cap burn under FR-36. A rule
invented per shell would give the two platforms different resource profiles for the same user behaviour.

**Why not immediately on selection.** FR-24 requires every action to be keyboard-reachable, so users
traverse the list with the arrow keys — and under immediate marking, walking past forty messages marks
forty messages read, each a durable queued intent and a provider call. FR-15 deliberately gives mark-read
no timed undo window, on the reasoning that the message stays in front of the user; that reasoning holds
for one deliberate mark and not for forty incidental ones. The result is a mutation that is individually
unnoticed and collectively hard to reverse.

**Why not manual-only either**, though it is the most defensible-sounding option for a triage-first
client. Unread counts are how most people decide whether to open a mail client at all, and a client whose
counts only fall when the user says so has redefined the number rather than served it.

**What it costs:** a preference, and a dwell timer that MUST NOT run while the window is unfocused — a
message left selected while the user is elsewhere has not been read.

**Contestable because:** the dwell is a guess about attention, and any specific duration is wrong for
someone. If measurement or complaint shows it wrong more often than right, the retreat is manual-only
rather than immediate marking, because the failure directions are not symmetric: failing to mark read is
visible and correctable by the user, while marking read wrongly hides mail they never saw.

## FR-15 — Undo

**Every intent except permanent delete has a compensating intent, and MUST be reversible through it.**
Reversal is executed as that compensation, never as a queue retraction.

**An intent that removes a message from the view the user is looking at MUST additionally offer a
timed undo window** — 10 seconds by default. Today that is archive, delete to trash, move, and report
junk. These are the actions where the user loses sight of what they did and has nothing left to click,
which is what a window is for; mark-read, flag and tag leave the message in front of the user, where the
affordance that applied the change is also the affordance that reverses it. Offering a countdown toast for
every message the reader marks read would make the mechanism worthless by making it constant.

Stating a rule rather than a list is deliberate. The list was previously "archive, delete, and move",
which D-40 falsified the moment it added an intent whose whole argument is that report-junk and
report-not-junk compensate each other. A rule keyed on the property that decides the question cannot go
stale the next time the set grows — and the growth rules above require a new intent to answer both halves
of it before it lands.

Compensation rather than retraction is the correct model because the original may already have reached the
server. A design that tries to cancel in flight has two outcomes to reason about; a design that always
compensates has one.

**What a compensation restores is local state, never a remote side effect.** Reporting not-junk returns
the message and tells the provider it was wrong; it cannot un-train the classifier. Undo promises the
first and MUST NOT be described as promising the second.

## What the queue guarantees about order

FR-17 batches, FR-38 fans out, and D-51 above lets intents accumulate against a message before any of them
reaches the server. None of that is safe without an ordering rule, and there was none.

**Intents against one message apply in the order they were issued.** Always, including through batching
and retry. This is the rule FR-17's batching must be built around rather than discover: *archive* then
*move* and *move* then *archive* put the message in different places, so a batcher that groups by
operation type and loses the per-message order produces a wrong final location — silently, without an
error, and unreproducibly.

**Intents against different messages are unordered**, which is what makes batching possible at all.

**Coalescing is permitted only where the collapsed sequence has the same effect *at the server* as the
sequence it replaces.** Marking read and then unread within one flush collapses to nothing, and nothing is
lost. Reporting junk and then not-junk does **not** collapse, even though the message ends where it
started, because D-40 above establishes that a report trains the provider's classifier — the two
reports have durable remote effects that the pair of local states does not describe. That is the test:
coalescing reasons about final state, and an intent whose point is a side effect has no final state to
reason about.

**An intent expires.** An intent that has not succeeded within a stated period stops being retried, and
the account enters the *attention* condition in [failure model](../runtime/failure-model.md) with the
intent still in the queue and still visible under FR-34. It is neither dropped nor retried forever.

Both alternatives are worse in the same direction. Retrying forever means a queue drained after weeks
offline replays archives onto messages the server has since deleted, handing FR-16 hundreds of conflicts
to adjudicate at once, on a schedule the user did not choose. Dropping silently loses a mutation the user
watched succeed, which is the single failure [data model](../storage/data-model.md) says the queue exists
to prevent. Surfacing is the only option that loses nothing, and it is the same answer NFR-48 already
gives for an intent that cannot be executed for a different reason.

## Undo is a compensation, which is why FR-15 and FR-38 do not conflict

Read together, FR-15 offers to reverse any intent but permanent delete, and FR-38 forbids rolling back a
partially applied thread mutation. They look like a contradiction and are not, but the reconciliation was
never written, so two implementers would build opposite behaviours from the same two paragraphs.

**Undoing a partially applied thread mutation compensates what landed and leaves what did not.** Seven of
ten archived and three pending becomes seven un-archived and three cancelled where cancellation is still
possible — never ten un-archived, which would be the rollback FR-38 refuses, and never a refusal to undo,
which would be FR-15 not applying to the case it is most needed in.

The general form is already in FR-15 and is worth naming here: **a compensation acts on what is true, not
on what was intended.** That is what makes it safe to offer against an operation whose outcome is partly
unknown, and it is the same property that lets a compensation run after the original has already reached
the server.

**A bulk operation under FR-17 is one undoable unit.** A user who archives a selection of five hundred
undoes the gesture they made, not five hundred gestures they did not.

## An intent whose capability has gone

[Provider model](provider-model.md) requires a probed capability that disappears to be surfaced under
NFR-29, and [data model](../storage/data-model.md) requires an intent the build does not *recognise* to be
quarantined. Neither reaches the case in between: an intent that is recognised, was legal when it was
enqueued, and whose gating capability has since gone away — a queued tag-add against a server that stopped
advertising arbitrary keywords, or a junk report on an account whose junk-reporting capability has dropped
to *none*.

**Such an intent MUST be quarantined and surfaced under NFR-48's existing rule, never executed against a
capability the account no longer declares and never discarded.** The mechanism already exists and needed
only to be pointed at this case; executing it anyway would be a write to the user's mail on the strength
of a declaration that has been withdrawn.

## FR-16 — Conflict resolution

When server state has diverged — the message was already moved or deleted elsewhere — Sift MUST resolve
silently where the outcome is unambiguous, and surface a **non-blocking** notice where it is not.

Blocking on a conflict in a mail client is worse than the conflict. The user is triaging; stopping them to
adjudicate a message that another client already archived is a failure of the design, not diligence.

## FR-17 — Bulk operations

Bulk operations MUST work over both selections and search results, batched to the provider's declared
maximum batch size — or, where that is unknown, to the conservative default the
[magnitude-capability rule](provider-model.md) requires. Permanent delete is excluded from the
search-result form, as stated above.

## FR-18, NFR-16, NFR-17 — Crash safety and exactly-once behaviour

**NFR-16.** No data loss on abrupt termination at any point. The mutation queue and the store MUST be
crash-consistent, with a write-ahead log and a durable commit.

**FR-18, NFR-17.** A mutation replayed after a crash MUST NOT double-apply, and mutations MUST be
**exactly-once observable from the user's perspective**.

The phrasing is deliberate. Exactly-once delivery is not achievable against a remote server that may have
applied an operation before the connection dropped. What is achievable is that the *observable outcome* is
correct: intents are idempotent where the provider permits, and where it does not, the adapter reconciles
against server state before retrying rather than blindly replaying.

## FR-38 — Thread fan-out and partial failure

Where an account declares client fan-out for thread operations, one thread-level intent expands into N
message-level operations. Partial failure MUST have defined semantics:

**A partially applied thread mutation is reported as partially done, and the remainder is retried. It MUST
NOT be rolled back.**

Rollback on mail is worse than the failure it corrects: un-archiving seven messages the user already
watched leave the inbox is a more confusing outcome than seven archived and three pending.

## D-38 — Reconciliation is silent for concurrent change, animated when Sift was wrong

**Chosen:** two paths, not one. Where the divergence is a **concurrent change made elsewhere** — another
client archived the message, the server moved it — the correction is applied **silently**. Where **Sift's
own action failed**, the correction is **animated** and accompanied by a non-blocking notice naming what
did not take effect.
**Rejected:** reconciling silently everywhere; animating every correction; blocking until the server
confirms.

**Why.** These are two different events wearing one name, and the user's model of each is different. A
message that leaves the list because another client moved it is *news*: the user did not act, nothing of
theirs failed, and an animation plus a notice would report a non-event on every device the user owns. A
triage action that visibly took effect and then silently reverted is the opposite — the user acted, they
watched it succeed, and the state they are looking at is now a lie. Reverting that without saying so is
how a client loses a user's trust in exactly the operation it exists to perform.

Blocking until confirmed was rejected on the same grounds as FR-16: it converts every slow network into a
stalled triage session, and it defeats FR-14 and NFR-7, which exist to put feedback ahead of the round
trip.

Animating everything was rejected because it makes the common case — ordinary multi-device convergence —
noisy, and a correction the user learns to ignore is not a correction.

**Implementation is not per shell.** This policy MUST live in the
[presentation layer](../architecture/presentation-layer.md), which owns optimistic state, so the two
shells cannot diverge in behaviour. A shell that implemented its own reconciliation would make the two
platforms differ on the one surface where a difference reads as a bug.

**What it costs:** the queue must retain enough about each intent to distinguish "this changed under me"
from "my own operation did not land" at reconciliation time, which is a real distinction to carry rather
than a flag to set. Correlating a server-side state to the intent that failed is the work.

**Contestable because:** it assumes users perceive the two cases differently. If they do not — if a row
moving unexpectedly reads the same either way — the second path buys nothing and the honest answer is to
reconcile silently everywhere and let the mutation queue surface failures in its own surface under FR-34.
Related but distinct is R-8: even the best policy here is visible when the server disagrees often.

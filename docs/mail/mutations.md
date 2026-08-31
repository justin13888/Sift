# Mutations

The only writes Sift performs, and the subsystem that performs them.

**Owns:** D-38, D-40, D-51, D-52, D-85, D-86, FR-13, FR-14, FR-15, FR-16, FR-17, FR-18, FR-38, FR-39,
NFR-16, NFR-17.

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
opening a thread marks read only the messages actually shown. The dwell defaults to L-21 in
[limits](../limits.md) and is configurable, including off.
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

## D-86 — The undo window withholds nothing, and it belongs to the layer

**Chosen:** an intent under FR-15's timed window is enqueued and flushed on the queue's ordinary
schedule, with **no special withholding**; undo within the window is a compensation like any other, and
the existing coalescing rule collapses the pair where it can. The undo record lives in the presentation
layer, keyed by D-85's undo-group identifier, and survives a window closing but not a quit.
**Rejected:** holding the intent back from the network for the duration of the window; giving the undo
stack to the shell.

**Why nothing is withheld.** Withholding is the obvious optimization — wait ten seconds, and an undone
archive costs no traffic at all. It reintroduces exactly what FR-15 above rejects: *"a design that tries
to cancel in flight has two outcomes to reason about; a design that always compensates has one."* A
withheld intent has a race at the end of every window, against a flush that may already have started, on
the single most frequent destructive action in the product.

**The saving arrives anyway, through a rule that already exists.** Intents flush on the scheduler's tick,
not instantly, and coalescing is permitted *"where the collapsed sequence has the same effect at the
server as the sequence it replaces"*. An archive and its compensation within one flush interval have no
net effect at the server and therefore collapse to nothing — so an undo that happens before the next
flush costs no traffic, without any mechanism that knows what an undo window is. Where the flush already
went out, the compensation is a second round trip, which is the honest cost of having actually done the
thing the user asked for.

**Why the record cannot live in the shell.** [View protocol](../architecture/view-protocol.md) lets a
shell use local identity *"for the undo stack"*, which reads as the shell owning it — and a window shell
is destroyed when its window closes, in a product that runs with no window at all. A user who archives a
message and closes the window has a countdown that dies with the view, which is not a decision anyone
made. The layer holds the record; a shell renders it, and the always-on surface can present it when no
window exists, through the [D-67](../architecture/view-protocol.md) host callback that already carries
core-initiated events.

**The window is session-scoped and general reversibility is not.** The timed window expires, and after it
the message is still reversible — FR-15 gives every intent but permanent delete a compensation, reachable
through the ordinary interface for as long as the message exists. Only the *countdown* is transient.
**It does not survive a quit**, because a compensation offered at the next launch would act on a gesture
the user has lost the context for, and an undo affordance whose subject the user cannot see is a worse
promise than no affordance.

**What it costs:** an undo that misses the flush interval costs two provider round trips and doubles that
action's contribution to [FR-36](../runtime/network-conditions.md)'s accounting. That is a real cost on a
metered connection, and it is bounded by how often users undo rather than by how often they archive.

**Contestable because:** withholding is genuinely cheaper for the common case and the race it introduces
is small and well understood. The argument against is not that the race is unmanageable but that the
design already chose compensation over cancellation once, deliberately, and having two answers to "did it
reach the server" in one subsystem is worse than paying for one of them.

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

**An intent expires.** An intent that has not succeeded within L-17 in [limits](../limits.md) stops
being retried, and
the account enters the *attention* condition in [failure model](../runtime/failure-model.md) with the
intent still in the queue and still visible under FR-34. It is neither dropped nor retried forever.

Both alternatives are worse in the same direction. Retrying forever means a queue drained after weeks
offline replays archives onto messages the server has since deleted, handing FR-16 hundreds of conflicts
to adjudicate at once, on a schedule the user did not choose. Dropping silently loses a mutation the user
watched succeed, which is the single failure [data model](../storage/data-model.md) says the queue exists
to prevent. Surfacing is the only option that loses nothing, and it is the same answer NFR-48 already
gives for an intent that cannot be executed for a different reason.

## D-85 — The queue's states, and what a crash mid-flight means

**Chosen:** an intent occupies one of six states; retry is exponential with a cap and jitter under the
scheduler's own rule; an intent whose request was issued is durably marked **before** it is sent, so a
crash resolves to *reconciling* rather than to a blind replay; and every intent carries a client-assigned
identifier and an optional undo-group identifier.
**Rejected:** a state column with no enumeration; retry counted but unscheduled; distinguishing sent from
unsent by inference at startup.

**Why.** [Data model](../storage/data-model.md) gives the queue row *"state, attempt count, creation
time, per-message sequence, expiry"* and the set of states was written nowhere. Four separate rules in
this document — ordering, coalescing, expiry, and D-38's reconciliation — are all statements about
transitions in a machine nobody had drawn.

| State | Meaning | Leaves to |
|---|---|---|
| **Pending** | durably enqueued, overlay applied, not yet issued | Issued, Coalesced, Quarantined, Expired |
| **Issued** | a request carrying this intent has been sent | Settled on success; Pending on a retryable failure; Reconciling on an unknown outcome |
| **Reconciling** | the outcome is unknown and the adapter is establishing server state | Settled or Pending |
| **Quarantined** | unrecognised, or its gating capability is gone | Settled, when a later build or a restored capability executes it |
| **Expired** | retried past L-17 without success | terminal until the user acts |
| **Settled** | applied, compensated, or reconciled away | terminal; the row is removed |

**A crash while Issued is the case that had no answer.** NFR-17 requires the adapter to *"reconcile
against server state before retrying rather than blindly replaying"*, which is only possible if something
distinguishes "never sent" from "sent, outcome unknown" — and nothing in the row did. **The transition
into Issued is durable and happens before the request leaves**, so a restart finds Issued intents and
moves them to Reconciling. The cost is one durable write per issue, on the journal
[D-74](../storage/data-model.md) separates for exactly this kind of traffic.

Getting this wrong is not symmetric. Assuming unsent replays an archive onto a message the server already
moved, which FR-18 forbids; assuming sent drops a mutation the user watched succeed, which is the failure
[data model](../storage/data-model.md) says the queue exists to prevent. Reconciling is the only state
that assumes neither.

**Every intent carries an identifier the client assigned**, which is what makes D-38 implementable: that
decision's own cost line is *"correlating a server-side state to the intent that failed is the work"*,
and a queue with no correlation key leaves it to be inferred from message and operation, which is
ambiguous exactly when several intents against one message are in flight. Where a provider accepts a
client-supplied idempotency key, this is the value to send, which converts Reconciling from an
enumeration into a question the server can answer.

**Retry is the scheduler's, not the queue's.** [Scheduling](../runtime/scheduling.md) already requires
exponential backoff with a cap and jitter for reconnection and prohibits per-account sleep loops; a
retrying intent is periodic work and goes through the same timing wheel, with the same curve. Attempt
count drives the backoff and does not itself terminate anything — **L-17 does**, on elapsed time rather
than on attempts, because an intent that failed twice in a week offline and one that failed two hundred
times in a minute are not the same situation and attempt counting cannot tell them apart.

**An expired or quarantined intent retires its overlay.** D-51 retires an overlay when its intent
*"succeeds, is compensated, or is reconciled"*, and neither of these matches — so on a literal reading
the user sees an archive that will never happen, permanently, with no rule saying whether that was
intended. **It was not.** The overlay is retired when the intent leaves the
executable set, the base state shows through, and the user is told through the *intent expired* or
*intent quarantined* state in the [state register](../architecture/state-register.md). Showing the truth
plus a notice is strictly better than showing a comfortable lie, and it is the same choice FR-16 makes
for conflicts.

**Flush concurrency, bounded by the ordering rule already stated.** Intents against different messages may
be issued concurrently and batched; intents against one message are issued **strictly in sequence**, and
a batch MUST NOT contain two intents for the same message. That is the rule above about order, expressed
as a constraint on the batcher rather than left for it to discover — which is what that section asks for
when it says batching must be built around the ordering rule rather than meet it by accident.

**An undo-group identifier makes FR-17's promise expressible.** *"A bulk operation is one undoable
unit"* is unimplementable if five hundred intents carry nothing linking them; the group identifier is
assigned at the gesture and is what a compensation acts over.

**What it costs:** two identifiers and a durable write per issue, on the highest-frequency mutation path
in the product, which is the write rate D-52 already warns sets the flush wakeup rate against NFR-11 and
NFR-15.

**Contestable because:** the durable Issued marker buys correctness for a crash window that is
milliseconds wide, at a cost paid on every single intent forever. A design that accepted blind replay and
leaned entirely on server-side idempotency would be simpler and would be right on the providers that
supply it — and wrong, silently, on the ones that do not.

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

**The animation is carrying meaning, so it needs a non-animated form.** Both platforms let a user ask for
reduced motion, and honouring that request would delete the distinction this decision exists to draw —
the animation is not decoration here, it is the signal that separates "your action failed" from "something
changed elsewhere". A reduced-motion user would get the silent path for both cases, which is the outcome
this decision rejects.

So the distinction MUST have an expression that does not depend on movement: under reduced motion the
correction is accompanied by the same non-blocking notice, and the row is marked as changed by a means the
platform's own accessibility settings do not suppress. What may not happen is the correction becoming
silent because the animation was suppressed. Stating it here rather than in a shell is deliberate — a
shell that quietly dropped the animation would drop half of D-38 without anyone deciding to.

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

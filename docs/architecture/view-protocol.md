# View protocol

What crosses the shell boundary, and under what contract.

**Owns:** D-48, D-66, D-67.

[Shell boundary](shell-boundary.md) settles *that* the boundary is a narrow C ABI and argues why
([D-17](shell-boundary.md)). [Presentation layer](presentation-layer.md) settles what the layer beneath it
owns, and that observation is windowed and cancellable ([D-18](presentation-layer.md)). Neither states the
contract an implementer needs on the first day: which thread anything happens on, who owns a value that
crosses, what an index in a change notification refers to, and what a shell is entitled to assume about an
identifier it holds.

D-17 says the ownership rules "must be written down rather than inferred". This is that page. It exists
separately because those two documents argue about the boundary and this one specifies it, and mixing the
two would bury a contract inside a rationale.

## D-48 — Main-loop delivery, no reentrancy, synchronous cancellation

**Chosen:** three rules, together.

1. **Every observer callback is delivered on the shell's own main loop.** The layer performs the hop; the
   shell never does.
2. **A shell calls into the layer from its main thread only, and never from inside a callback.** The
   boundary is not reentrant.
3. **Cancellation is synchronous.** When a cancellation call returns, no further callback for that
   observation will arrive, on any thread, ever.

**Rejected:** delivering on whichever runtime worker completed the work and letting each shell marshal;
best-effort cancellation in which a callback may still be in flight when cancellation returns.

**Why delivery is the layer's job.** AppKit and GTK4 both require view mutation on their own main loop,
and [D-19](overview.md) chose a work-stealing runtime, so a completion lands on an arbitrary worker
thread. Something must hop. If the layer does not, both shells grow a hand-rolled marshal — and they will
not be the same marshal, because the two toolkits' idioms for it differ. That is precisely the drift
[shell boundary](shell-boundary.md) calls a defect: "a capability that exists for one shell and not the
other is a defect in this boundary rather than a Linux feature", arriving through the delivery path
instead of the API surface. Doing it once, below both, is also the only place it can be tested once.

**Why reentrancy is forbidden rather than defined.** A shell that calls back into the layer from inside a
change notification — selecting a row in response to an insert, say — either deadlocks against whatever
guards the window's state, or re-enters diff computation while a batch is half-applied and corrupts the
index space the batch is expressed in. Both are possible to define away and neither is possible to test
comprehensively. Forbidding it makes the layer's internal locking straightforward and makes the rule a
shell author can hold in their head: **receive, record, return; act on the next turn of the loop.**

**Why cancellation must be synchronous.** [D-18](presentation-layer.md) says cancellation "is not an
optimization here" because a fast scroll supersedes window requests faster than they can be served and a
search supersedes a query on every keystroke under NFR-5. What that argument leaves out is the failure
mode: a shell cancels an observation because it is about to free the context that observation writes into.
Best-effort cancellation makes that a use-after-free **at a C ABI**, on the one boundary shell-boundary.md
already concedes memory-safety bugs are possible. NFR-6's zero-dropped-frame fling generates these races
at scroll rate, and L3 in [memory pressure](../runtime/memory-pressure.md) generates them in bulk by
destroying every window and its view hierarchy while requests are in flight.

**What it costs.** The hop is a real cost against NFR-2's 50 ms folder switch and NFR-7's 16 ms optimistic
feedback — one main-loop turn per delivery, which is the budget those targets were going to spend anyway
on the frame that displays the result. Synchronous cancellation costs a rendezvous with in-flight work
rather than a flag set, so a cancellation call can block briefly, and it must therefore never be made from
a place that cannot afford to wait.

**Contestable because:** synchronous cancellation from the main thread is a main-thread block, which is
the thing NFR-6 exists to prevent. The bet is that the rendezvous is short because the work being
cancelled is a query rather than a network round trip, and that bet is unmeasured. If it proves wrong, the
answer is an explicit quiescence handshake — cancel, then await confirmation before freeing — and not a
retreat to best-effort, because the retreat reintroduces a use-after-free rather than a slow frame.

## D-66 — The boundary's representation, stated once

**Chosen:** one calling convention for every entry point, one string representation, one aggregate shape,
and build-time exhaustiveness in place of runtime tolerance.
**Rejected:** per-call conventions chosen for each function's convenience; opaque row handles with
per-field accessors; a runtime fallback for a discriminant a shell does not recognise.

[Shell boundary](shell-boundary.md) requires that *"every type crossing the boundary needs an explicit,
stable representation"* and that the ownership rules *"be written down rather than inferred"*. The rules
above do the ownership half. This does the representation half, and it is one decision rather than five
because the argument is the same for all of them: [D-17](shell-boundary.md) keeps the surface narrow so
that its rules can be held in one file, and a boundary with five conventions cannot be.

### Every call returns a status, and results leave through out-parameters

**No entry point encodes failure in its return value's domain.** Every function returns a status; a
result reaches the caller through an out-parameter the caller owns and the layer writes. Sentinel
returns, null-means-error, and a thread-local last-error are all excluded.

A thread-local last-error is the one worth naming, because it is conventional in C and wrong here:
[D-19](overview.md) is a work-stealing runtime, and D-48's rule 2 confines shell calls to the main
thread — so a last-error is *nearly* safe, which is worse than either safe or unsafe. The convention that
is correct only while a rule elsewhere holds is the convention that breaks when that rule is relaxed.

**The status distinguishes three things, and the third is why this is a requirement rather than a
style.** An operation may succeed; it may fail in an identified way, in which case the failure is an
identified state the shell renders — the section below says who owns those; or it may have been
terminated by a caught panic. [D-47](overview.md) requires that a caught panic *"MUST NOT be silently
absorbed as an ordinary parse failure"* and that it be counted per subsystem — and if the boundary
collapses it into the same status as an ordinary failure, the count is unobtainable at exactly the layer
where a shell would otherwise report the defect. A caught panic is therefore its own status value,
everywhere.

### Strings are UTF-8 with an explicit length, and are never NUL-terminated

**Every string crossing the boundary is a pointer and a byte length, in UTF-8**, and the length is
authoritative. Nothing on this boundary scans for a terminator.

This is a security property rather than a convenience. [NFR-54](presentation-layer.md) makes the
presentation layer responsible for normalizing attacker-controlled text — display names, subjects,
snippets, folder and tag names, attachment names — *before* it crosses, and a terminator-delimited
representation makes the crossing itself lossy in an attacker-reachable way: a byte the sender chose
truncates the value, and the shell renders a prefix of a subject while the layer believes it handed over
the whole one. Every truncation on this boundary MUST be one the presentation layer performed
deliberately, under a bound in [limits](../limits.md), and none MUST be a property of the encoding.

**Validity is established once, where normalization happens, and is not re-checked by the shell.** The
layer guarantees well-formed UTF-8 on this boundary in the same way the sanitizer guarantees it under
[I10](../rendering/sanitizer-invariants.md); a shell that validated again would be asserting a property
it cannot repair, and a shell that validated *instead* would be the second normalization site NFR-54
exists to prevent.

### Rows cross as a contiguous array, borrowed for the callback

**A batch of list rows crosses as one contiguous array of fixed-layout records, borrowed under the
ownership rule above**, with each record's text fields carried as pointer-and-length into storage the
layer owns for the duration of the delivery.

The alternative — an opaque row handle plus one accessor per field — is what a boundary designed for
safety rather than for this workload would choose, and it is arithmetically excluded. FR-6's list carries
roughly ten fields, and [NFR-6](ui-shell.md) requires zero dropped frames over a ten-thousand-row fling
with cell reuse; per-field accessors turn one delivery into six figures of boundary crossings for a
gesture whose entire budget is frame-shaped. The contiguous array is what lets a shell bind a batch in
one pass.

**What that costs is stated plainly**: fixed-layout records are the part of this boundary that
[D-17](shell-boundary.md) concedes memory-safety bugs are possible in, and D-60's generated declarations
in [workspace](../build/workspace.md) exist precisely so that the two sides' idea of that layout cannot
disagree silently.

### An unknown discriminant is a build failure, not a runtime case

**Every enumerated value crossing this boundary MUST be handled exhaustively by both shells, checked when
the project is built. There is no runtime fallback for an unrecognised discriminant, and one MUST NOT be
added.**

This is deliberately the opposite of the rule [provider model](../mail/provider-model.md) states for
capabilities — *"an unrecognised capability is ignored, not fatal"* — and the difference is that a
capability set is written by an adapter and read by a build that may be older, while **both sides of this
boundary ship in one binary**. D-17 says so in its own argument for a C ABI: there is *"no version skew
to detect, because both sides ship in one binary"*. Nothing here is ever older than anything else here.

So a runtime fallback would not be tolerance; it would be a hiding place. [D-56](presentation-layer.md)
requires that *"a state either has a rendering in both shells or it has none"*, and the way that rule
fails is not by anyone deciding against it — it is by a shell quietly rendering "something went wrong"
for a state whose real rendering was never written, on one platform, for as long as nobody looks. A build
failure is the only enforcement that cannot be deferred, and it is the same enforcement
[workspace](../build/workspace.md) applies to the boundary's own layout.

### The main-loop hop is arranged at initialization, by the shell

D-48 rule 1 requires the layer to deliver on the shell's main loop while
[presentation layer](presentation-layer.md) forbids it to contain a widget toolkit. **The shell therefore
supplies, once at initialization, the means of scheduling work onto its own loop**, and the layer calls
it. AppKit and GTK4 both provide such a primitive natively; neither is reachable from a toolkit-free
layer, and neither needs to be.

**Deliveries are coalesced per observation rather than posted per notification.** A delta applying
thousands of envelopes must not become thousands of main-loop items — that is NFR-6's frame budget spent
on scheduling — so the layer accumulates and posts a batch, and D-48's atomic-batch rule already says
what a shell does with it. Where the shell cannot keep up, the layer coalesces further rather than
growing an unbounded queue: a queue that grows is an unbounded cache by another name, which
[memory pressure](../runtime/memory-pressure.md) forbids outright.

### Cancellation rendezvous with workers, and discards what is already posted

D-48 requires that after cancellation returns, no further callback arrives *"on any thread, ever"*, and
its own contestability note worries only about how long the rendezvous takes. There is a sharper problem
it does not reach: **a delivery already posted to the main loop will still arrive**, and the thread
calling `cancel` is the thread that would have to drain it — so a cancellation that waited for posted
deliveries would deadlock against itself, deterministically, every time.

**Each observation carries a generation, every posted delivery carries the generation it was posted
under, and cancellation advances it.** Cancellation then rendezvous with *worker-side* work only, which
is the short wait D-48 bets on, and a stale delivery that reaches the main loop is discarded by
comparing generations rather than waited for. The guarantee D-48 states is preserved exactly — no
callback for that observation is *delivered* after cancellation returns — without requiring the main
thread to wait on itself.

This is what makes the use-after-free D-48 is about actually closed. The shell cancels because it is
about to free the context the observation writes into; with a generation check, the posted delivery that
would have written into freed memory never reaches the shell at all.

**What it costs:** a generation on every observation and a comparison on every delivery, and the
discipline that no delivery path may skip the check. The check is cheap; remembering it exists is the
part that needs writing down.

**Contestable because:** generations solve the posted-delivery race and do not solve a shell that frees
its context without cancelling first, which remains undefined behaviour that no mechanism here detects.
The honest position is that this boundary depends on the shell obeying a protocol, and narrowing the
protocol is the only defence available at a C ABI.

## Change notifications

Delivered under D-48's rules, and expressed so that a shell applying them in order arrives at the state
the layer holds.

**A batch is atomic and its indices are in the post-batch space.** The shell applies the whole batch or
none of it, and every index refers to the result. **Moves are expressed as moves**, not as a delete paired
with an insert, because the two are not equivalent to a list view: a move animates and preserves the row's
identity and its cell, while a delete-plus-insert destroys and recreates it, which is visible and which
loses the selection D-18 already requires the layer to own.

This convention is stated rather than inferred because the two toolkits' batch-update interfaces do not
agree about it, and an inconsistent batch is a hard failure rather than a glitch on at least one of them.
Choosing in the layer means one shell translates; leaving it unstated means both shells translate
differently and the flagship [unified inbox](presentation-layer.md) view — the merged, independently
mutating stream D-4 admits is the hard case — is where the disagreement first shows.

**An observation is anchored, not an integer range.** D-4 merges per-account streams that mutate
independently, so a window expressed as "rows 40 through 80" is already wrong by the time it is served.
The shell declares the window it is looking at and the layer maintains it across change.

## D-67 — The boundary has a second direction, and it is not an observation

**Chosen:** the shell registers, once at initialization, a small closed set of **host callbacks** the
layer may invoke on its own initiative; they are not scoped to any view, they are not cancellable, and
they survive the destruction of every window.
**Rejected:** modelling core-initiated events as observations the shell must register; letting the core
reach a toolkit directly.

**Why anything is needed at all.** The protocol as previously stated is entirely shell-initiated: the
shells *"send intents and requests and receive prepared view models"*, and every callback answers an
observation the shell registered. Six things in this design are core-initiated and answer no observation,
and each of them is a requirement rather than a convenience:

| Event | Requirement | Why no observation can carry it |
|---|---|---|
| Destroy every window | L3 in [memory pressure](../runtime/memory-pressure.md) | The governor decides; a window cannot observe its own destruction being ordered |
| Re-authentication is needed | [FR-2](../security/credentials.md) | Its own text requires the prompt *"through the always-on surface"* because there may be nothing on screen |
| The bundle was replaced | [FR-26](process-model.md) | The restart prompt has no window to be requested from |
| A notification was activated | [FR-23](ui-shell.md) | *"May mean opening a window on a process that has none"* |
| The account condition changed | [D-49](../runtime/failure-model.md) | The annunciator must reach the user with no window open, by that decision's own rule |
| An authorization callback arrived | [D-36](../security/credentials.md) | The platform delivers it to the process, not to a view |

**Why a closed registered set rather than a general channel.** Every one of these is a case where the
core must reach the *application*, not a view, so the natural implementation is for the core to hold
something toolkit-shaped — and that is the one thing [presentation layer](presentation-layer.md) forbids,
since *"the shared logic below them is Rust with no widget toolkit in it"* is the premise D-1's whole
two-shell economics rests on. Registering callbacks inverts it: the shell hands the layer function
pointers, the layer knows nothing about what they do, and the toolkit stays entirely above the boundary.

Keeping the set **closed and enumerated here** is what stops it becoming the general escape hatch that
dissolves the command-and-view-model shape. A seventh host callback is an amendment to this table, which
is the same discipline [mutations](../mail/mutations.md) applies to FR-13's intent set.

**They are delivered under D-48's rules like everything else** — on the main loop, non-reentrant — with
one difference that must be stated: **a host callback has no observation and therefore no generation**,
so the D-66 discard mechanism does not apply to it. What replaces it is that the set above is
process-scoped: a host callback is valid for as long as the process is, and the shell unregisters only at
shutdown. This is why they are not cancellable, and why cancellability would be meaningless.

**What it costs:** a second shape on a boundary D-17 wants narrow, and a set that will be under pressure
to grow every time something in the core wants to tell somebody.

**Contestable because:** six entries is enough that a general event channel with an identified payload
would be a smaller surface than six signatures, and would fold neatly into the
[state register](state-register.md)'s identified-and-parameterized shape. That design is
better if the set grows and worse while it is small, because it replaces six checked signatures with one
that carries a discriminant — reintroducing exactly the runtime dispatch D-66 refused.

## Ownership

**Every value crossing the boundary is owned by the layer and borrowed by the shell for the duration of
the call or callback that delivered it.** A shell that needs a value beyond that copies it. The layer
frees nothing that a shell might still hold, and a shell frees nothing the layer allocated.

One rule rather than a per-type convention, because per-type conventions are what the boundary cannot
afford: D-17 accepts that memory-safety bugs are possible here and keeps the surface narrow so that the
rules can be held in one file. A borrow-for-the-call rule is checkable by reading a signature; a mixture
of owned and borrowed returns is checkable only by reading every call site in two languages.

## Identifiers a shell holds

**A message's local identity is stable for as long as that message exists in that account**, and a shell
may use it as a key for selection, for rendering the undo affordance, and for notification click-through.
**The undo record itself is the layer's**, under [D-86](../mail/mutations.md), because a window shell is
destroyed when its window closes and a countdown that dies with the view is not a decision anyone made.

The exception is stated because it exists and would otherwise be discovered: under
[D-44](../storage/data-model.md), a move on a provider with unstable identifiers that does not corroborate
to exactly one candidate "presents as a delete plus an arrival". The message under the user's selection
acquires a new identity, and the shell learns about it as a delete and an insert like any other. That is
the correct behaviour — a false join is data loss and a missed one is a display defect — but a shell that
assumed identity survived every provider operation would hold a key to nothing.

## Errors

**A failure crossing the boundary is an identified state, never a message.** The layer reports which
failure occurred and the parameters that distinguish it; the shell decides what to say and in which
language. The states are enumerated in the [state register](state-register.md); the account conditions
among them are owned by [failure model](../runtime/failure-model.md), which the register refers to rather
than restates.

## Related

- [Shell boundary](shell-boundary.md) — D-17, and why the boundary is a C ABI at all
- [Presentation layer](presentation-layer.md) — D-18, and what the layer owns
- [Failure model](../runtime/failure-model.md) — the states an error resolves to

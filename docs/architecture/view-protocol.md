# View protocol

What crosses the shell boundary, and under what contract.

**Owns:** D-48.

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
may use it as a key for selection, for the undo stack, and for notification click-through.

The exception is stated because it exists and would otherwise be discovered: under
[D-44](../storage/data-model.md), a move on a provider with unstable identifiers that does not corroborate
to exactly one candidate "presents as a delete plus an arrival". The message under the user's selection
acquires a new identity, and the shell learns about it as a delete and an insert like any other. That is
the correct behaviour — a false join is data loss and a missed one is a display defect — but a shell that
assumed identity survived every provider operation would hold a key to nothing.

## Errors

**A failure crossing the boundary is an identified state, never a message.** The layer reports which
failure occurred and the parameters that distinguish it; the shell decides what to say and in which
language. The states themselves, and the account conditions they compose into, are owned by
[failure model](../runtime/failure-model.md).

## Related

- [Shell boundary](shell-boundary.md) — D-17, and why the boundary is a C ABI at all
- [Presentation layer](presentation-layer.md) — D-18, and what the layer owns
- [Failure model](../runtime/failure-model.md) — the states an error resolves to

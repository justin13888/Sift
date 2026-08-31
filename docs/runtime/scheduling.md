# Scheduling and wakeups

**Owns:** D-25, D-94, FR-43, NFR-10, NFR-11, NFR-15.

## The enemy is wakeups, not cycles

Battery cost at idle is dominated by **timer wakeups preventing the processor from entering deep sleep
states**, not by the work performed once awake. An app that does almost nothing, twenty times a second, is
worse for battery than one that does a little, once a minute.

Every rule below follows from that.

## Rules

**One scheduler, coalesced.** All periodic work MUST funnel through a single timing wheel with jitter and
slack, so the processor wakes once per interval rather than N times. **Per-account or per-folder sleep
loops are prohibited.** This is a code-review rule.

**Push over poll, except on cellular.** Where a provider offers practical push it MUST be preferred. The
exception inverts on cellular links and is described in [network conditions](network-conditions.md).

**Connection budget.** Five accounts watching three folders each is fifteen sockets and fifteen
keepalive timers; the bound is L-23 in [limits](../limits.md). Sift MUST prefer mechanisms that
watch many mailboxes over one connection where the provider offers them; otherwise it watches only
the inbox and refreshes other folders lazily on user navigation. See
[IMAP](../mail/providers/imap.md).

**FR-43.** The set of watched folders MUST be per account,
user-selectable, and persisted as account policy. The default is the inbox plus the special-use
folders FR-5 resolves; every other folder is refreshed lazily on navigation, and FR-12's "not
cached" state carries the difference honestly. The watched-folder count is the multiplier on NFR-11
and NFR-15, both of which are stated per account, so leaving it unstated meant every idle target was
measured against a configuration nobody had chosen. It is also an affordance the set already
promised without creating: NFR-29 in [IMAP](../mail/providers/imap.md) tells a user on a degraded
server that they can act "by narrowing which folders are watched", and until now nothing let them.
**Platform integration.** Sift MUST use each platform's coalescing-friendly scheduling facilities
rather than raw timers, and MUST respect low-power and metered states. The mechanism is D-25 below.

**Backoff on failure.** Reconnection after network loss MUST use exponential backoff with the cap
and jitter of L-24 in [limits](../limits.md). Hot-looping reconnect is the classic "the mail client
ate my battery on a flaky hotspot" bug and MUST NOT be possible by construction.

## D-94 — NFR-11 is an application bound wearing a per-account label

**Chosen:** NFR-11 is met when the **total** number of wakeups the application takes at idle is at most
two per minute, regardless of how many accounts are configured. The per-account wording stands, and this
states what it composes to.
**Rejected:** reading it as two per minute per account, which permits ten at five accounts; changing the
attribution rule so that a coalesced fire counts once.

**Why it needed saying.** [Observability](observability.md) requires that *"a coalesced fire is
attributed to every account whose work it served, not to the one that happened to set the deadline"*,
because counting it once *"would let five accounts share a wakeup and report a fifth of one each, which
is the arithmetic by which a budget is met on paper"*. That rule is right. Composed with NFR-11's
per-account phrasing, it means a single coalesced fire spends one wakeup against **every** account's
budget at once — so five accounts sharing two fires per minute is exactly at the limit, and a third fire
puts all five over. **The budget is therefore two fires per minute for the whole application**, and the
per-account reading that permits ten is arithmetically excluded by the attribution rule rather than by
anything NFR-11 says.

**Two consequences, and both are wanted.** An additional account costs **nothing** as long as its work
joins existing fires — which is the strongest possible statement of what
D-25's shared wheel is for, and a far better property than "each account may add two". And a
third fire per minute fails the gate for every account simultaneously, which is correct: at idle, a third
fire is a coalescing regression and it does not matter which account provoked it.

**Why not fix it by changing the attribution instead.** Counting a coalesced fire once would make NFR-11
read naturally as ten fires at five accounts, and would make the wheel's central benefit
*arithmetically invisible in the only metric that measures it* — a design that coalesced nothing and one
that coalesced perfectly would report the same per-account figure. The attribution rule exists precisely
to prevent that, so the wording is what gives way.

**This is not [R-3](../open-questions.md).** That risk asks whether two per minute is *achievable* with
fifteen live connections. This asks what the number means, and the two are independent: if R-3 proves the
target unreachable and it is raised, the composition stated here is unchanged.

**What it costs:** a requirement whose plain reading is wrong, which is why the row above now points
here. It also means the gate is insensitive to *which* account regressed, and the per-subsystem and
per-account attribution [observability](observability.md) requires is what recovers that for diagnosis —
the gate says the application failed, the counters say where to look.

**Contestable because:** an application bound that does not scale with accounts is unusually strict, and a
user with ten accounts is asking for more work than a user with one. The defence is that idle is defined
as no work outstanding, so what is being bounded is the cost of *waiting*, and waiting for ten things
should not cost more than waiting for one — that is what a timing wheel is.

## D-25 — A Sift-owned timing wheel on platform timers

**Chosen:** one timing wheel owned by Sift, armed through the platform's own coalescing-capable timer —
a dispatch source with an explicit leeway on macOS, an absolute-mode timer file descriptor on Linux.
**Rejected:** the async runtime's built-in timer; handing periodic work to the operating system's job
scheduler outright.

**Why.** The runtime chosen in [D-19](../architecture/overview.md) ships a perfectly good timer wheel, and
it is the wrong one for this purpose. A general-purpose runtime timer schedules at millisecond granularity
with no way to tell the kernel "this may fire late, batch it with something else" — and **that hint is the
entire mechanism by which wakeups coalesce**. Using it would mean measuring NFR-11 against a timer that
structurally cannot meet it.

Handing periodic work to the platform job scheduler instead gives the best possible idle behaviour, but it
is coarse, awkward to test deterministically, and it splits scheduling policy across two places — Sift's
own rules for tiering and backoff would then live somewhere other than the thing that decides when work
runs.

Owning the wheel keeps one place that knows about jitter, slack, alignment, and per-subsystem accounting,
and lets the platform do the one thing only it can do. The runtime's timer is still used, but only for
short I/O timeouts, where coalescing is meaningless.

**What it costs:** a timing wheel is a real data structure with real edge cases — clock jumps, suspend and
resume, and cancellation among them. NFR-38's "no retry storm on wake" is a property of this component
specifically, and it MUST handle a system wake as an explicit event rather than discovering it through
expired timers.

**The wheel runs on a monotonic clock, and that is normative rather than incidental.** A wall clock moves
when the system corrects its time or the offset changes, and every deadline armed behind the new reading
fires at once — five accounts' worth, simultaneously, from the component whose own requirement forbids
exactly that. NFR-38's retry storm would arrive from the ordinary operation of time synchronization rather
than from a network event.

Where an interval must align to wall-clock instants — NFR-37's aligned polls, which exist so that several
accounts wake together — the alignment is computed against the wall clock and the resulting deadline is
armed on the monotonic one, so a correction moves the *next* alignment rather than firing every deadline
already set. A system wake re-derives deadlines from the current time as the explicit event above
requires, rather than letting a monotonic clock that did not advance during sleep decide nothing is due.

**Contestable because:** it is a component built rather than used, in a project that elsewhere argues
against that. The justification is narrow — the platform hint is unavailable any other way — and if
measurement shows the runtime's timer meets NFR-11 regardless, this decision does not earn itself.

## Targets

Hypotheses, validated against the [reference environment](../product/reference-environment.md).

| ID | Target |
|---|---|
| **NFR-10** | Idle CPU at or under 0.1%, averaged over 5 minutes with no network events |
| **NFR-11** | At most 2 wakeups **per minute** per account at idle — timer fires and socket wakes alike — coalesced onto the shared scheduler. D-94 below states what that means once coalescing is accounted for, because the two compose to something narrower than the wording suggests |
| **NFR-15** | Network at idle at or under 1 KB per minute per account, steady-state keepalive |

**NFR-11 counts socket wakes as well as timer fires, and that too is a coherence fix.** It previously said
"timer wakeups" while [observability](observability.md) instruments both, so the requirement named half of
what its own instrument measured — and a socket wake prevents deep sleep exactly as a timer fire does,
which is this document's opening argument. Counting only timers would have let fifteen keepalive-bearing
connections meet the budget on paper. See [reference environment](../product/reference-environment.md) for
the protocol.

**NFR-11 previously read "per second", and was restated for internal coherence — not adjusted to match a
measurement.** Nothing has been measured. At the five accounts the
[reference corpus](../product/reference-environment.md) specifies, the old figure permitted ten wakeups a
second, sustained, with no work to do — which is the pathology the opening section of this document
describes and rejects, and which cannot coexist with NFR-10's 0.1%. A budget that licenses the behaviour
its own document forbids is a defect in the specification whatever the hardware later says. The
per-minute figure is a hypothesis on exactly the same footing as every other number here.

**These may be unachievable at five accounts.** Holding fifteen encrypted connections alive means periodic
renegotiation, NAT keepalives, and re-arming timers; "idle" is not free even with zero work. If measurement
says so, the resolution is to drop secondary folders to polling, not to quietly restate the target. This
is [tracked as a risk](../open-questions.md).

## Measurement

Wakeups MUST be counted and reported, not inferred — **per subsystem and, because NFR-11 is stated per
account, per account as well.** The two axes answer different questions and neither substitutes for the
other; [observability](observability.md) owns how a coalesced fire is attributed across both, which is the
part that is easy to get wrong in the direction that flatters the budget.

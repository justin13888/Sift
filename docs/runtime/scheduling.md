# Scheduling and wakeups

**Owns:** D-25, NFR-10, NFR-11, NFR-15.

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

**Connection budget.** Five accounts watching three folders each is fifteen sockets and fifteen keepalive
timers. Sift MUST prefer mechanisms that watch many mailboxes over one connection where the provider
offers them; otherwise it watches only the inbox and refreshes other folders lazily on user navigation.
See [IMAP](../mail/providers/imap.md).

**Platform integration.** Sift MUST use each platform's coalescing-friendly scheduling facilities rather
than raw timers, and MUST respect low-power and metered states. The mechanism is D-25 below.

**Backoff on failure.** Reconnection after network loss MUST use exponential backoff with a cap and
jitter. Hot-looping reconnect is the classic "the mail client ate my battery on a flaky hotspot" bug and
MUST NOT be possible by construction.

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

**Contestable because:** it is a component built rather than used, in a project that elsewhere argues
against that. The justification is narrow — the platform hint is unavailable any other way — and if
measurement shows the runtime's timer meets NFR-11 regardless, this decision does not earn itself.

## Targets

Hypotheses, validated against the [reference environment](../product/reference-environment.md).

| ID | Target |
|---|---|
| **NFR-10** | Idle CPU at or under 0.1%, averaged over 5 minutes with no network events |
| **NFR-11** | At most 2 timer wakeups **per minute** per account at idle, coalesced onto the shared scheduler |
| **NFR-15** | Network at idle at or under 1 KB per minute per account, steady-state keepalive |

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

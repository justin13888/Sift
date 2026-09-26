# Memory pressure

**Owns:** D-20, D-93, NFR-8, NFR-9, NFR-12, NFR-13.

## D-93 — The governor is one serialized task, shedding is issue-and-forget, and restoring waits

**Chosen:** tier transitions are evaluated by a **single serialized task**; a shed is *issued* to each
owner and never waited on, so NFR-13's deadline is an issue deadline; the governor **holds no lock a shed
target needs**; and a tier is restored only after the pressure signal has stayed clear for L-19, one step
at a time.
**Rejected:** shedding synchronously; evaluating transitions concurrently; restoring the moment pressure
clears.

**Why serialized.** Pressure signals arrive in bursts, and a governor that handled each concurrently
could have an L2 shed and an L3 shed in flight against the same caches. Serializing makes "the current
tier" a value with one writer, and a signal arriving during a transition supersedes rather than
interleaves — so L3 arriving mid-L2 finishes as L3, which is the outcome anyone would want and not the
outcome concurrent handling gives.

**Why "issued" and not "completed", which NFR-13 already says and does not explain.** A shed reaches
caches owned by many subsystems, some mid-use, and at L3 it destroys every window — which under
[D-67](../architecture/view-protocol.md) is a host callback delivered on the shell's main loop.
**Waiting for that would mean the governor blocking on a main loop it does not control.** Issue-and-forget
keeps NFR-13's 500 ms a property of the governor rather than of whichever subsystem is slowest, and the
actual reclamation is timed separately by [NFR-46](../rendering/webview-isolation.md).

**The deadlock this avoids, and it is a real one.** [D-48](../architecture/view-protocol.md) makes
cancellation synchronous and called from the main thread, so the main loop can be inside a cancellation
rendezvous at the moment L3 needs it. If the governor waited for window destruction, and cancellation
waited for anything the governor held, the two would deadlock — and L3 is precisely the tier that,
in [D-48](../architecture/view-protocol.md)'s own words, *"generates these races in bulk"*. Two rules
close it: the governor waits for nothing, and it holds no lock a shed target needs, so a subsystem can
always complete the work it is in the middle of before dropping a cache.

**Why restoring waits, and shedding does not.** Shedding is urgent and cheap; restoring is neither. With
no hysteresis, a system oscillating around the pressure threshold reparses the 40 MB filter engine every
time it crosses — spending the largest allocation in the design repeatedly to satisfy a signal that has
not settled. **Pressure must stay clear for L-19 before a tier is released, and tiers are released one
step at a time**, so recovering from L3 passes through L2 and L1 rather than restoring everything at
once into a system that was under pressure a moment ago.

**This is what fixes the filter engine's permanent absence.** [Content
blocking](../rendering/content-blocking.md) says the engine returns *"when pressure clears **and**
the next window opens"*, which for a user who keeps one window open and passes through L1 once means
it never returns for that window's life — in the tier this document itself calls *"the tier Sift
will spend real time in"*. That was a consequence of having no hysteresis: without a settling rule,
reloading on clearance alone would be the shed undoing itself, which that document correctly
refuses. With L-19 the two are distinguishable. **The engine returns when pressure has been clear
for L-19 and a window is open** — not when a *new* window opens — and the no-reload-on- demand rule
keeps its meaning, because a reload after sustained clearance is not a response to a pressure
signal.

**What it costs:** a delay between a system recovering and Sift behaving fully again, during which
[FR-33](observability.md)'s reason for a withheld image still names the shed. That is honest and it is
slower than it looks to a user watching memory free up.

**Contestable because:** one dwell serves every tier and every cache, and they are not alike — the body
view is cheap to rebuild and the filter engine is not, so a single number is wrong for at least one of
them. Per-cache dwells would be better and are not proposed, because the tiers are defined as
compositions rather than as independent caches and splitting them would undo that.

## Subscribe to pressure; do not poll for it

Sift MUST subscribe to the operating system's own memory-pressure signals rather than polling free memory.
Polling is both a wakeup source and a worse signal — the OS knows about pressure before free memory
reflects it.

| Platform | Signal |
|---|---|
| macOS | the dispatch memory-pressure source, with its normal, warning, and critical levels |
| Linux | cgroup v2 pressure-stall information, pollable with thresholds |

## Shed tiers

| Tier | Trigger | Action | Target |
|---|---|---|---|
| **L0** | normal | steady state | NFR-9 — at or under 150 MB, window open |
| **L1** | mild | drop decoded-image caches, rendered-body caches, and prefetch queues; release the filter engine | L0 less the filter engine and those caches |
| **L2** | warning | destroy the body view; drop parsed-MIME caches; release database memory; shrink the search index cache | L1 less the body view and those caches; **never below the NFR-8 floor, because a window is still live** |
| **L3** | critical | destroy every window and every window shell's view hierarchy, leaving the application shell that owns the always-on surface; drop every remaining cache to its floor; collect the allocator; repaint from cold on next activation | toolkit residue plus the resident floor — the NFR-8 state with every cache at its floor |

**Tier targets are compositions, not percentages, and the change is a coherence fix rather than a
measurement.** They previously read −30% at L1 and −50% at L2 against L0's 150 MB, which is arithmetically
impossible against the other two numbers on this page. −50% is 75 MB, while NFR-8 puts the floor with **no
window at all** at 90 MB — so L2, which keeps the window, was required to reach a figure below the
window-less floor. It was also required to reach a figure below L3's, since L3's stated target *is* that
floor: the deepest tier had the loosest number, which inverts the ordering the tiers exist to express.

A percentage cannot be a shed target here, because what a tier can release is bounded by what it holds,
and the residue underneath it is not compressible by asking. Each tier is therefore stated as the tier
above it minus the things that tier releases, which is falsifiable in exactly the way the percentages were
not: every term is a declared, reported cache size under [observability](observability.md), so the
subtraction can be checked rather than believed.

L2's largest single win is destroying the body view, which is genuinely out-of-process on both target
engines — see NFR-46 in [webview isolation](../rendering/webview-isolation.md). That reclaim does not
depend on Sift's own process count, which is the reasoning behind
[D-2](../architecture/process-model.md).

**L3 no longer terminates anything.** Under the earlier two-process design it killed the UI process
outright. With one process it is instead the deepest in-process shed: every window and its view hierarchy
goes, every cache drops to its floor, and the allocator is told to return what it can. What stays is the
[application shell](../architecture/ui-shell.md) — a tier that removed the tray would leave the
application unreachable in the state it is trying to survive, which is why this tier terminates nothing.
What cannot be returned is **toolkit residue** — whatever AppKit or GTK4 keeps resident once
initialized. That residue is the honest floor, and it is why L3's target is stated as a composition
rather than a number.

If the operating system needs more than L3 can give, it will terminate the process, and Sift MUST be
correct across that — the store and the mutation queue are crash-consistent under NFR-16 in
[mutations](../mail/mutations.md), so termination costs a repaint and nothing else.

**L1 is not a rare event on the reference rig, and what depends on it should be read that way.** The
[reference environment](../product/reference-environment.md) specifies a deliberately unflattering
machine — a 2020-era laptop with 8 GB of memory. Mild pressure there, with a browser and the rest of the
user's work already resident, is nearer a steady state than an incident. L1 is the tier Sift will spend
real time in, not a corner it occasionally visits.

That matters because of what L1 releases. Dropping the filter engine puts
[content blocking](../rendering/content-blocking.md) into its absent-authority state, in which every
remote fetch is denied. That is the failure direction that document chooses deliberately and it is the
right one — but the consequence on this rig is that a user who has allowed a sender's images may still see
them withheld, repeatedly, for a reason that is neither the sender nor the network. The reason MUST be
reported as the shed it is under [FR-33](observability.md), which that document already requires. What is
recorded here is only that the state is ordinary rather than exceptional, so its cost to the user should
be weighed at that frequency rather than at an incident's.

## D-20 — mimalloc, with purge driven by the governor

**Chosen:** mimalloc as the global allocator, with a bounded purge delay, and an explicit collect issued
by the pressure governor as part of L3.
**Rejected:** jemalloc with background purging; the system allocator.

**Why.** NFR-12 is an allocator problem before it is a cache problem. An allocator that does not return
freed arenas to the operating system will ratchet footprint upward over a multi-week uptime no matter how
disciplined the caches are, and the shed tiers need a way to *ask* for that return at a moment of their
choosing rather than hoping decay reaches it eventually.

jemalloc's long-uptime fragmentation behaviour is the best understood of the three, but its background
purge thread is a periodic wakeup, and **NFR-11 counts wakeups**. Disabling that thread moves purging onto
allocation paths, which is exactly the wrong place for it. The system allocator offers almost no purge
control on macOS.

**What it costs:** a non-default allocator in a process that also runs AppKit or GTK, both of which
allocate through the same global. Interposition is total, so an allocator bug is an application bug.

**Contestable because:** the ranking here rests on the claim that a background purge thread is a
meaningful fraction of the NFR-11 budget, which is unmeasured. If it is not, jemalloc's fragmentation
record is the stronger argument and this decision inverts.

## Preconditions

These make the tiers actually work. Without them the governor has nothing to release.

- **Every cache has an explicit byte budget and an eviction policy. No unbounded map anywhere.** This is a
  code-review rule, not an aspiration, and it is the single most load-bearing line in this document.
- **Every cache reports its size**, so the governor acts on declared numbers rather than guesses. See
  [observability](observability.md).
- **MIME parsing streams.** A large attachment goes to the [blob store](../storage/cache-and-blobs.md);
  only headers and structure occupy memory. See [pipeline](../rendering/pipeline.md).
- **The shell owns no authoritative state**, so destroying it at L3 loses nothing that must be recovered
  from the network. See [presentation layer](../architecture/presentation-layer.md).
- **Lists are virtualized with a fixed window.** Rendering 200,000 rows defeats everything above. See
  [UI shell](../architecture/ui-shell.md).

## Targets

Hypotheses, validated against the [reference environment](../product/reference-environment.md). Measured
as `phys_footprint` on macOS and PSS on Linux, never RSS — see [observability](observability.md).

| ID | Target |
|---|---|
| **NFR-8** | Resident idle footprint with no window open at or under 90 MB at the reference corpus, **excluding the filter engine**, which is not loaded in this state |
| **NFR-9** | Full application idle — window open, **no reader visible**, filter engine loaded — at or under 150 MB, **inclusive of NFR-42's 40 MB**. The reading peak is a different state, and no requirement budgets it yet — see below |
| **NFR-12** | Footprint growth at or under 5% over 14 days of continuous uptime: **no ratchet** |
| **NFR-13** | L2 shedding is **issued** within 500 ms of Sift's receipt of the signal, L3 within 1 second — see the note on the two clocks below, and D-93 for what *issued* means and why the governor never waits |

**The filter engine is bound to window lifetime, and NFR-8 and NFR-9 were restated to say so — a
coherence fix, not a measurement.** At 40 MB under NFR-42 it is the largest declared cache in this
documentation set, and it previously appeared in no shed tier at all, which contradicted the first
precondition above in the one case where that precondition mattered most. It is now **loaded when a
window opens and released when the last window closes**, and dropped at L1 under pressure.

The lifetime follows from what the engine is for. Nothing renders a message body while no window exists,
so at the NFR-8 state the engine has no possible caller — it was 44% of a budget spent on a component
that could not be used. Reloading at window open costs a list parse well before any message is selected,
so NFR-3's 80 ms first paint is untouched; the reload is on NFR-1's cold-start path rather than on the
reading path, which is the right place for it.

**NFR-8's number is the least trustworthy figure in this documentation set.** It was 60 MB when a
window-less daemon could avoid linking a toolkit at all. Under [D-2](../architecture/process-model.md) it
must instead absorb toolkit residue, and the Linux figure is expected to be the worse of the two because
GTK4's renderer loads a graphics driver stack it cannot unload. P0 MUST measure both platforms and replace
this number; it is a placeholder standing in for a measurement, not an estimate anyone should defend.

**NFR-9's number inherits that placeholder, and its own definition contradicted another requirement.** It
previously read "window open, body view warm", but NFR-46 in
[webview isolation](../rendering/webview-isolation.md) requires the body view to be torn down after a
configured period with no reader visible. The idle state this target names is the state *after* that
teardown, so it cannot hold a warm view, and NFR-9 now says so. That is a coherence fix, not a
measurement.

**The repair is not a rescue.** Decomposed against the figures on this page the target still reads: 90 MB
of window-less floor, plus NFR-42's 40 MB of filter engine, leaving **20 MB for a live window** — tight
rather than arithmetically hopeless, which is what the previous reading was when it asked those same
20 MB to hold a WebKit content process as well.

**What the repair exposes is a third state that no requirement budgets.** Reading a message means the
window, the filter engine, and a warm body view at once, and that view is the largest single allocation in
the running application — the one L2 exists to reclaim. [D-54](../rendering/webview-isolation.md) fixes
that count at exactly one, so the state is determinate rather than a function of how a thread is
presented; before it, "a warm body view" named a quantity nothing bounded. It sits above L0, it is what a
user sees for most of the time they are actually using Sift, and no NFR names it. NFR-8, NFR-9 and that
reading peak MUST be re-derived together once P0 has measured toolkit residue and a warm body view: they
are one budget stated at three lifecycle points, and moving any one of them alone reintroduces exactly the
incoherence the tier targets above were just corrected for. Tracked with NFR-8's placeholder in
[open questions](../open-questions.md).

**NFR-13 and NFR-46 measure different clocks, and the distinction is normative.** NFR-13 bounds the time
from the pressure signal to the governor having *issued* every release the tier calls for — caches
dropped, the body view told to tear down, the allocator asked to collect. NFR-46 bounds the separate,
slower step of the operating system actually returning the body view's pages, at up to 1 second from
teardown. Stated as one clock the pair would contradict each other, because L2's largest action is
precisely the teardown NFR-46 times: a 500 ms budget cannot contain a 1 second reclaim. Stated as two,
they compose — the governor is prompt, the kernel is not instantaneous, and each is separately testable.

**NFR-12 is the hardest requirement in this documentation set.** It is an allocator, fragmentation, and
cache-discipline problem that only appears in long-running soak tests. The soak harness that detects it
MUST exist in **P0**, not P4 — see [observability](observability.md) and
[roadmap](../product/roadmap.md), which state the same phase.

### Provisional figures

**This section changes no target.** The figures below come from the interim rig in the
[reference environment](../product/reference-environment.md). No target may be accepted on that
machine's evidence, and it covers one of the four platform and architecture pairs that P0 owes. So
NFR-8, NFR-9 and the reading peak are **not** re-derived here, and the placeholders above stand. What
this records is the first cell of the derivation, together with what measuring it showed about the
states the targets name.

**The protocol.** The shipping Release binary runs against a scratch container and a scratch keychain,
with the recorded fixture corpus loaded. It is driven from outside the process into three states and
left to settle for 30 seconds in each. The figure is the median of five `phys_footprint` samples under
D-16. It covers Sift **and every engine process it owns**, found through the application's own launchd
domain. The protocol is automated, and the per-integration tier runs it natively on both architectures.

| State | Sift | Content | GPU | Networking | Total |
|---|---:|---:|---:|---:|---:|
| Window open, nothing selected (NFR-9's state) | 32–34 | 17–25 | 10–11 | 4–5 | **64–74** |
| One message selected (the reading peak) | 33–34 | 17–26 | 11–14 | 4–5 | **67–78** |
| L3, body view released: every window destroyed, allocator collected (NFR-8's state) | 32 | — | 12–13 | — | **45** |
| L3, body view not released | 35 | 24 | 12 | 4 | **75** |

The figures are MiB, from three runs on arm64 with macOS 26.6 and a 16 KB page. The fixture corpus is not
the scale corpus. It measures toolkit residue and the body view, which is what Q-12 is about. It does
not measure how the resident floor grows with the index. Loading the scale corpus through the protocol
needs an answer that nobody is present to give: the corpus writer creates the account keys, so the
application reading them raises an access prompt.

The figures show four things:

- **Both placeholders have headroom on this cell.** The L3 total is half of NFR-8's 90 MB. The
  window-open total is under half of NFR-9's 150 MB. That is evidence that the code path fits, and
  nothing more, because this machine is the opposite of unflattering.
- **The window-open state contains a content process that NFR-9's definition excludes.** The shell
  builds the body view along with the reader, before anything is selected. So the state NFR-9 names,
  with no reader visible and the view torn down under NFR-46, never occurs in the current shell. The
  measured window figure is the reading state minus one document, not NFR-9's state. The reading peak's
  increment over it is small for the same reason: most of the body view's cost is already paid.
- **The one engine process L3 does not release is WebKit's GPU process, at about 13 MB.** L3's target
  says toolkit residue plus the resident floor. The GPU process belongs to neither, but it is owned by
  Sift and is therefore counted. Whether it is released at all once no web view exists is WebKit's
  decision rather than Sift's. Until that is shown otherwise, it is part of the floor.
- **L3 did not reliably release the body view.** Across five launches, the content process outlived
  L3 in three. In one of them, a heap inspection showed the list, sidebar and reader controllers and the
  web view still alive after the window controller that owned them had gone. When that happens, the
  "floor" is 75 MB rather than 45, and it is the window-open figure less nothing. A tier whose largest
  reclaim happens only some of the time is a defect. It is tracked as a defect, and it is not averaged
  into a figure.

What remains before the three targets can be re-derived together: the same protocol on Intel; Linux,
with PSS in place of `phys_footprint`, which has no instrument yet; the scale corpus; and the reference
rig itself, which [Q-10](../open-questions.md) has not chosen.

# Roadmap

Phasing, and what each phase is allowed to leave unproven.

The ordering principle: **prove the risks that can kill the design before building on top of them.** A
client that becomes fast later never does, so performance and resource targets are gates on every phase
rather than a final-phase activity.

## P0 — Prove the risky parts

Nothing else starts until these resolve. Each is capable of invalidating a settled decision.

| Spike | Resolves |
|---|---|
| Hardened body webview on both platforms against the hostile-HTML corpus | [D-3](../rendering/webview-isolation.md), invariant N-1 |
| Memory soak harness with allocation attribution | [D-1](../architecture/ui-shell.md), NFR-12, and [D-24](../runtime/observability.md)'s attribution overhead against NFR-44 |
| Toolkit residue measured on both platforms, window destroyed and allocator collected; a warm body view measured alongside it | [D-2](../architecture/process-model.md) and **NFR-8's and NFR-9's numbers, with the reading peak neither of them names**, which are placeholders and MUST be re-derived together — see [Q-12](../open-questions.md) |
| Sandboxed login-item residency on macOS | whether the App Store is a channel at all — [D-33](platforms-and-distribution.md) |
| Google OAuth restricted-scope verification path | the top business risk — see [credentials](../security/credentials.md) |

The soak harness and allocation attribution MUST be built here rather than later; they cannot be
retrofitted, and NFR-12 is only observable over weeks. See [observability](../runtime/observability.md),
which states the same phase requirement.

**The toolkit-residue spike is what makes NFR-8 a number rather than a guess.** It is listed here rather
than in P1 because [D-2](../architecture/process-model.md) rests on it, and because the Linux figure is
the one most likely to invalidate a settled decision. It MUST also measure a warm body view — not because
NFR-9 contains one, since NFR-46's teardown means it cannot, but because the state that *does* contain one
is what reading a message costs and no requirement budgets it. The shed-tier targets are stated as
subtractions from that family of figures, so replacing any one of them alone leaves the tiers describing a
budget that no longer exists.

## P1 — Vertical slice

One provider (JMAP, the cleanest protocol — see [JMAP](../mail/providers/jmap.md)), one account, read plus
archive, local search, and the shell boundary. Instrument NFR-1, NFR-3, NFR-5, NFR-8, and NFR-10 from the
first commit.

## P2 — Providers

Gmail, Microsoft Graph, and generic IMAP behind the capability abstraction. If the abstraction needs
provider-name special-casing to accommodate them, the abstraction is wrong — see
[provider model](../mail/provider-model.md).

The Linux shell is native — see [UI shell](../architecture/ui-shell.md), which settles this rather than
deferring it to the end of this phase as previously planned.

## P3 — Triage depth

The full mutation intent model, undo, conflict resolution, and bulk operations. This is a first-class
subsystem, not an adapter method. See [mutations](../mail/mutations.md).

## P4 — Scale and polish

Unified inbox, memory-pressure shed tiers L1 through L3, accessibility, and the visual-regression corpus.
Auto-update is absent from this list because [D-33](platforms-and-distribution.md) removed it.

## Coverage

Every phase gate below is keyed on "what the phase has shipped", so that phrase has to be answerable
without interpretation. This table is where it is answered: **every FR appears exactly once in Ships, and
every live NFR exactly once across the whole table.** A requirement in no row is not deferred, it is
unowned, and that is a defect in this document rather than a decision anyone made.

**Standing is not a phase.** It holds the properties that are true of the first commit and every commit
after, and that no phase can be said to reach. A phase that violates one has not deferred it, it has
broken it — which is why they are gated everywhere rather than somewhere. Four of them are the hard
constraints the project is defined by: NFR-20 no script in bodies, NFR-21 no unrequested egress, NFR-23
credentials only in the OS store, NFR-24 never a listening socket. NFR-14 is the no-unbounded-cache rule,
NFR-16 and NFR-19 are crash-consistency and surviving hostile MIME, and NFR-51 says "from the first
commit" in its own text.

| Phase | Ships | Gates |
|---|---|---|
| Standing | — | NFR-14, NFR-16, NFR-19, NFR-20, NFR-21, NFR-22, NFR-23, NFR-24, NFR-51 |
| P0 | — (spikes only) | NFR-8, NFR-9, NFR-12, NFR-44, NFR-45 |
| P1 | FR-2, FR-5, FR-6, FR-8, FR-9, FR-11, FR-12, FR-13, FR-14, FR-19, FR-24, FR-25, FR-28, FR-30, FR-33, FR-34 | NFR-1, NFR-3, NFR-5, NFR-10, NFR-11, NFR-15, NFR-25, NFR-28, NFR-40, NFR-41, NFR-46 |
| P2 | FR-1, FR-3, FR-4, FR-20, FR-21, FR-37 | NFR-2, NFR-18, NFR-29, NFR-30, NFR-31, NFR-32, NFR-33, NFR-34, NFR-35, NFR-37, NFR-38, NFR-39, NFR-48 |
| P3 | FR-15, FR-16, FR-17, FR-18, FR-38, FR-39 | NFR-7, NFR-17 |
| P4 | FR-7, FR-10, FR-22, FR-23, FR-26, FR-27, FR-29, FR-31, FR-32, FR-35, FR-36, FR-40 | NFR-4, NFR-6, NFR-13, NFR-26, NFR-27, NFR-42, NFR-43, NFR-47, NFR-49, NFR-50 |

NFR-36 is absent because it is struck — see [requirements](../requirements.md).

Seven assignments are not obvious from the phase descriptions, and each is a claim worth disagreeing with:

**FR-13 and FR-14 are P1, not P3.** The slice ships archive, and archive is an intent applied
optimistically through a durable queue. The mechanism cannot be added afterwards without rewriting
how the slice mutates anything. P3 ships the rest of the set — undo, conflict resolution, bulk,
fan-out, junk — which is what "triage depth" means once the mechanism exists.

**FR-33 and FR-34 are P1.** This phase already requires instrumenting NFR-1, NFR-3, NFR-5, NFR-8 and
NFR-10 from the first commit, and the per-message debug view and the runtime panel are the surfaces that
do it.
This is the retrofit argument P0 makes for the soak harness, one phase later.

**FR-28 and FR-30 are P1.** A phase that renders HTML bodies under FR-8 cannot defer synthetic origin or
honest link display, because those are not decorations on the pipeline — they are two of the things that
make its hostile-input claim true. Shipping the renderer without them proves the wrong thing.

**FR-4 is P2.** Provable erasure arrives with multiple accounts rather than before them; with one account
it is untestable in the way that matters, since there is nothing it must leave behind.

**FR-20 is P2, with FR-21.** Structured operators and the server-search fallback are one feature seen from
two sides: the fallback's whole job is translating the same operators into what a provider will accept, so
splitting them across phases means specifying the operators twice.

**NFR-48 is P2.** It is the first phase in which an installation from a previous phase must survive an
upgrade. In P1 there is no deployed schema to migrate from.

**NFR-45 is P0.** P0's prose already mandates the soak harness and cites only NFR-12 and NFR-44 — but
NFR-45 is the requirement that specifies the harness and its slope gate, so it was being gated on by name
nowhere.

## Phase gates

Every phase MUST satisfy every requirement the coverage table gates it on — performance, resource,
reliability, security, network and quality alike — measured against the
[reference environment](reference-environment.md). The gate was previously stated over performance and
resource targets only, which left four of the seven requirement classes gated by nothing at all. A rule
that lets a phase regress a security or network target without noticing is not a gate.

A phase MUST NOT be declared complete while a requirement in its Ships column is unmet. Deferring a target
or moving a requirement between phases requires amending the table above, in this document, which is the
point of having one place to amend.

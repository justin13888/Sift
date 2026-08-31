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
| Toolkit residue measured on both platforms, window destroyed and allocator collected; a warm body view measured alongside it | [D-2](../architecture/process-model.md) and **NFR-8's and NFR-9's numbers**, which are currently placeholders and MUST be re-derived together — see [Q-12](../open-questions.md) |
| Sandboxed login-item residency on macOS | whether the App Store is a channel at all — [D-33](platforms-and-distribution.md) |
| Google OAuth restricted-scope verification path | the top business risk — see [credentials](../security/credentials.md) |

The soak harness and allocation attribution MUST be built here rather than later; they cannot be
retrofitted, and NFR-12 is only observable over weeks. See [observability](../runtime/observability.md),
which states the same phase requirement.

**The toolkit-residue spike is what makes NFR-8 a number rather than a guess.** It is listed here rather
than in P1 because [D-2](../architecture/process-model.md) rests on it, and because the Linux figure is
the one most likely to invalidate a settled decision. It MUST also measure a warm body view, because
NFR-9 is NFR-8 plus the filter engine plus a live window plus that view, and the shed-tier targets are
stated as subtractions from the pair — replacing one of them alone leaves the tiers describing a budget
that no longer exists.

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

## Phase gates

Every phase MUST satisfy the performance and resource targets applicable to the functionality it has
shipped, measured against the [reference environment](reference-environment.md). Deferring a target to a
later phase requires amending this document.

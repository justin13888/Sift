# Open questions and risks

Two lists. **Open questions** are decisions not yet made; each one blocks or reshapes something specific.
**Risks** are assumptions already load-bearing in the design that may not hold.

Nothing here may be resolved by deletion. Resolving an open question means recording the answer in its
owning document and striking the entry here with a pointer to it.

## Open questions

| ID | Question | Blocks | Context |
|---|---|---|---|
| **Q-1** | ~~Mail-service-hosted images and the third-party definition~~ **Resolved:** a bundled infrastructure list, first-party only when the origin is attested. The attested-pair map is recorded as the refinement | — | [D-37](rendering/sender-origin.md) |
| **Q-2** | ~~Invariant I9 needs tightening~~ **Resolved:** restated as a non-addition property. The cited conflicts did not exist — a subset relation already permitted removal, and the transform adds CSS rather than text | — | [sanitizer invariants](rendering/sanitizer-invariants.md) |
| **Q-3** | ~~Gradient handling in the dark transform~~ **Resolved:** transformed only when every stop is below a chroma threshold; mixed-stop gradients excluded by construction; image gradients go to the classifier | — | [dark mode](rendering/dark-mode.md) |
| **Q-4** | ~~Confirm legacy word-processor conditional content is stripped~~ **Resolved:** confirmed, as a consequence of three independent allowlist rules, and therefore asserted as a regression vector rather than inferred | — | [sanitizer invariants](rendering/sanitizer-invariants.md) |
| **Q-5** | ~~How much of the debug view ships enabled in release~~ **Resolved:** all ten items, preference-gated, default off. A reduced release set would have broken the shared identity with the test harness | — | [observability](runtime/observability.md) |
| **Q-6** | ~~Does the Linux shell get built natively at all~~ **Resolved:** yes, GTK4 and libadwaita. [D-2](architecture/process-model.md) settled it early — a web UI would hold an engine resident for chrome | — | [UI shell](architecture/ui-shell.md) |
| **Q-7** | ~~Optimistic-UI reconciliation policy~~ **Resolved:** silent for concurrent change, animated with a non-blocking notice when Sift's own action failed. Two questions, not one | — | [D-38](mail/mutations.md) |
| **Q-8** | ~~The fallback join when the internet message identifier is absent or duplicated~~ **Resolved:** the identifier narrows candidates and never keys a join; a stored fallback digest corroborates; ambiguity resolves to distinct messages. The three joins are all within one account, which is what let one rule serve them. R-5 stays open against the digest | — | [D-44](storage/data-model.md) |
| **Q-9** | **Maximum batch size per provider.** FR-17 requires bulk operations be batched to a declared maximum. All four provider documents carry the row and all four declare it unknown. The [magnitude-capability rule](mail/provider-model.md) now makes *unknown* a legal declaration that plans conservatively, so this no longer blocks an adapter from being written — but each row still needs a real value and a source: published limit, measured, or conservative default | The throughput of every bulk operation under FR-17; no longer its correctness | [provider model](mail/provider-model.md) |
| **Q-10** | **The reference rig specification.** The owning document says it MUST be recorded once chosen and it has not been. Every performance and resource figure in this set is unfalsifiable until it is | Every NFR with a number in it, and the P0 gates | [reference environment](product/reference-environment.md) |
| **Q-11** | **Filter-list integrity.** A hostile or compromised list injects CSS into every message body through the generated stylesheet. Signing, pinning, or sanitizing list content are all plausible; none is specified | FR-27's subscription model | [threat model](security/threat-model.md), [content blocking](rendering/content-blocking.md) |
| **Q-12** | **NFR-8's and NFR-9's numbers, and the reading peak neither of them names.** NFR-8 was restated against toolkit residue when [D-2](architecture/process-model.md) reversed and is now a placeholder rather than an estimate; Linux is expected to be worse and may reopen D-2. NFR-9 inherits it, and repairing its definition against NFR-46 exposed a third state — window, filter engine and a warm body view at once, which is what reading a message costs — that no requirement budgets at all. **All three MUST be re-derived together**, not one at a time | NFR-8, NFR-9, the unbudgeted reading peak, the shed-tier targets that subtract from them, and D-2's standing | [memory pressure](runtime/memory-pressure.md) |
| **Q-13** | **Whether the platform contact store is reachable under Flatpak.** [D-41](architecture/presentation-layer.md) resolves display names from the platform's own contact store; under Flatpak that runs through a portal, and whether one exists with the needed read access is unverified. Same shape as R-10 | FR-40 on Linux only | [presentation layer](architecture/presentation-layer.md), [platforms and distribution](product/platforms-and-distribution.md) |

## Risks

Ordered by how much of the design they would invalidate.

| ID | Risk |
|---|---|
| **R-1** | **Gmail restricted-scope verification is a business blocker, not a technical one.** It requires a recurring third-party security assessment or the client is capped and shows an unverified warning. The escape hatches each reshape the product. **Resolve before writing the Gmail adapter.** See [credentials](security/credentials.md) |
| **R-2** | **No footprint ratchet over 14 days (NFR-12) is the hardest requirement in this set.** It is an allocator, fragmentation, and cache-discipline problem visible only in long soak tests. The [soak harness](runtime/observability.md) must exist in P0, not P4 |
| **R-3** | **"Idle" is not free even with zero work.** Fifteen live encrypted connections mean renegotiation, NAT keepalives, and re-arming timers. NFR-10 and NFR-11 may be unachievable at five accounts without dropping secondary folders to polling. See [scheduling](runtime/scheduling.md) |
| **R-4** | **The mutation intent model is where "read-only" quietly becomes expensive.** Divergent archive and delete semantics, identifiers that change on move, thread fan-out, and undo after a server-side change are a deep well. Budget it as a subsystem. See [mutations](mail/mutations.md) |
| **R-5** | **The internet message identifier is not reliably unique.** Some servers and senders duplicate it; some omit it. [D-44](storage/data-model.md) now defines the fallback, so this is mitigated rather than unaddressed — but the risk is unchanged in kind: D-44 corroborates with a digest over headers assumed to survive transit, and that assumption has no corpus behind it. Measuring it belongs with the [fidelity corpus](product/reference-environment.md) |
| **R-6** | **Two rendering engine ports multiply QA rather than adding to it.** The fidelity corpus must pass on both, and font and colour-management differences produce visible diffs that layout equivalence does not prevent. See [platforms and distribution](product/platforms-and-distribution.md) |
| **R-7** | **Dark mode for email has no correct industry answer.** The compromise is chosen deliberately in [dark mode](rendering/dark-mode.md) and will still disappoint someone |
| **R-8** | **Optimistic UI plus eventual consistency produces visible flicker.** Related to [D-38](mail/mutations.md), but distinct: even the best reconciliation policy is visible when the server disagrees often |
| **R-9** | **The full CSS cascade ([D-27](rendering/dark-mode.md)) may not fit NFR-41.** It is a style system in miniature sharing a 30 ms budget with the sanitizer and the blocker, and its fallback is a visibly worse product rather than a slower one |
| **R-10** | **Flatpak may not grant what D-14 needs.** Network-condition detection reads NetworkManager over D-Bus; the portal alternative supplies neither the metered flag nor the link class. Credential access has the same shape. See [platforms and distribution](product/platforms-and-distribution.md) |
| **R-11** | **No urgent-fix path exists.** [D-33](product/platforms-and-distribution.md) gives delivery to platform channels, so a security fix reaches users on App Review's and the distribution system's schedule, in a product whose primary adversary chooses the input |

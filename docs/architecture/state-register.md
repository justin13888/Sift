# State register

Every identified state that crosses the shell boundary, and what a parameter may be.

**Owns:** D-68.

[D-56](presentation-layer.md) requires that no user-visible sentence cross the boundary: the layer emits
*"identified, parameterized states"* and each shell renders them. It names the cost —
*"every condition, error and explanation needs an identifier and a parameter list"* — and then that cost
was owned by nothing. [View protocol](view-protocol.md) delegated the states to
[failure model](../runtime/failure-model.md), which enumerates the eight **account conditions** and
explicitly scopes the rest out: *"transient degradation is a notice, not a condition."*

So a dozen states that D-56 names in its own argument had no identifier, no parameter list, and no home.
This page is that home, and it exists separately for the reason [limits](../limits.md) exists separately:
these are asserted by three consumers — the layer that raises them, the macOS shell, and the GTK shell —
and a set that lives beside whichever code first needed it is invisible to the other two.

## D-68 — One register, and prose is the only thing that never crosses

**Chosen:** every state the layer can raise is identified here with a parameter list; a parameter may be
a **content value**, a number, a duration, a timestamp, an identifier, or another state, and MUST NOT be
prose.
**Rejected:** a register per subsystem, beside the code that raises each state; parameters that may carry
a rendered fragment.

**Why one register rather than one per subsystem.** The whole value of D-56 is that a state with no
rendering is *visibly* missing. That only holds if the set of states is enumerable in one place: a
per-subsystem list makes "which states has the GTK shell not rendered yet" a question answered by reading
every subsystem, which is the question that stops being asked. This is the same argument
[limits](../limits.md) makes for itself and
[sanitizer invariants](../rendering/sanitizer-invariants.md) makes for the allowlist.

**Why a parameter may not be prose.** A parameter carrying a rendered fragment defeats the decision
entirely and does it invisibly: the state is identified, the shell renders a template, and one slot in
the template is a sentence the layer wrote in one language. Every argument in D-56 applies to that
sentence and none of them notices it, because from the outside the boundary looks like it is carrying
identified states. **A parameter carries a value; the shell supplies every word around it.**

**What it costs:** a state that wants to say something the register cannot express has to become two
states, or gain a parameter, rather than gaining a string. That is the friction working.

**Contestable because:** the no-prose rule is easy to state and hard to hold at the edges — a provider's
own error text, an unsubscribe destination's label, a folder name a server chose. The register's answer
is that those are content values, not prose, and the distinction is defensible but it is a line
reasonable people will put in slightly different places.

## The two string categories, which is what D-56 and NFR-54 were disagreeing about

Read side by side in the same document, D-56 says *"no user-visible string crosses the shell boundary"*
and [NFR-54](presentation-layer.md) says sender and recipient display names, subjects, snippets, folder
and tag names and attachment names *"MUST be normalized before it crosses the boundary"*. Taken
literally they contradict each other. The distinction that reconciles them was never named, so it is
named here and both rules are true under it.

| Category | What it is | Crosses? |
|---|---|---|
| **Content value** | A value that came from the user's mail or from a provider: a display name, subject, snippet, folder or tag name, attachment name, address, an unsubscribe destination, a provider's own diagnostic text | **Yes**, as data. Normalized under NFR-54 first. Never translated, because Sift did not write it |
| **Chrome prose** | A sentence Sift says to the user: a condition, an explanation, a confirmation, a label, an empty state | **Never.** It is a state identifier and parameters, rendered by the shell |

The test is authorship. **Sift never translates what it did not write, and never ships what it did
write across the boundary.** A subject is not translated because translating a subject is nonsense; a
sentence explaining why an account is degraded is not returned as a string because that would make the
layer choose a language. NFR-54 governs the first column and D-56 governs the second, and neither reaches
the other's.

**A content value is attacker-controlled and stays so.** Being a parameter does not launder it: the shell
places it into chrome that a screen reader will announce and a menu may display, which is exactly the
path NFR-54 exists for and the path [threat model](../security/threat-model.md) notes *"no sanitizer
invariant sees"*. Normalization happens once, in the layer, before the value becomes a parameter.

## The register

Account conditions are **not restated here**. [Failure model](../runtime/failure-model.md) owns the
enumerated, precedence-ordered set under [D-49](../runtime/failure-model.md), and this table would be a
second copy going stale — the failure [docs/README](../README.md) names when it says the indexes are
navigation and never source. They are listed as one row so that the register is complete by reference.

| State | Parameters | Raised by |
|---|---|---|
| Account condition | the condition, plus whatever that condition carries | [D-49](../runtime/failure-model.md), which owns the set |
| Reconciliation notice | the intent, the message or thread it applied to, what the server holds now | [D-38](../mail/mutations.md) |
| Degradation explanation | the account, the capability lost, whether it was probed or declared | NFR-29 in [provider model](../mail/provider-model.md) |
| Not cached | the message | [FR-12](../storage/cache-and-blobs.md) — distinct from the next row, which is the whole point of FR-12 |
| Not available | the message, the reason it cannot be fetched now | [FR-12](../storage/cache-and-blobs.md) |
| Resource blocked | the count, and per resource the verdict and the rule or heuristic that produced it | [FR-33](../runtime/observability.md), [content blocking](../rendering/content-blocking.md) |
| Resource unavailable | the count, and per resource why — decrypt failure, missing blob, timeout, or revoked document | [D-91](resource-broker.md), which distinguishes this from both a block and a fabricated address |
| Content withheld by a shed | the count | L1 in [memory pressure](../runtime/memory-pressure.md), which requires the shed be named rather than a rule |
| Blocker disagreement | the resource, the two verdicts | [D-10](../rendering/content-blocking.md) |
| No special-use folder resolved | the account, the semantic kind, the candidate folders | [FR-5](../mail/provider-model.md) |
| No mail handler configured | — | [FR-41](../product/scope.md), which forbids an affordance that silently does nothing |
| Message is encrypted and unreadable | the scheme, where known | [D-39](../product/scope.md)'s deferral, which requires the state rather than a blank body |
| Contrast repair failed | the message | NFR-47 in [dark mode](../rendering/dark-mode.md) |
| Attachment written | the final path, as written | [NFR-53](../storage/cache-and-blobs.md), which requires the path be shown |
| Intent quarantined | the intent, the account, why — unrecognised, or a capability withdrawn | [data model](../storage/data-model.md), [mutations](../mail/mutations.md) |
| Intent expired | the intent, the account, when it was enqueued | [mutations](../mail/mutations.md) |
| Stage failed on a caught panic | the message, the pipeline stage | [D-47](overview.md), which forbids absorbing this as an ordinary parse failure |

**A caught panic is in this table deliberately.** D-47 requires that it *"MUST NOT be silently absorbed as
an ordinary parse failure"*, and until it had a state of its own there was nothing for a shell to render
that was not exactly that absorption. It is also why [D-66](view-protocol.md) gives a caught panic its own
status value: the two are the same requirement seen from the two sides of the boundary.

## Adding a state

**A new state arrives here, with its parameters, in the same change that raises it.** A state raised by
the layer and absent from this table is a state one shell will render and the other will not, which is
the drift [shell boundary](shell-boundary.md) calls a defect rather than a Linux feature.

**Every state MUST have a rendering in both shells or in neither**, which is D-56's rule and is checkable
against this table rather than against memory. Under [D-66](view-protocol.md) the check is a build-time
one: exhaustiveness over this set is enforced when the project is built, so a state added here without a
rendering does not compile.

## Related

- [Presentation layer](presentation-layer.md) — D-56, which requires this register, and NFR-54, which
  governs the other string category
- [View protocol](view-protocol.md) — D-66's status values and exhaustiveness rule, and D-67's host
  callbacks, which carry states to a shell with no window
- [Failure model](../runtime/failure-model.md) — D-49 and the account conditions this register refers to
- [Limits](../limits.md) — the register this one is modelled on

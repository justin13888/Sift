# Sift — design documentation

Sift is a desktop mail client for **reading, searching, and triaging** mail across multiple accounts. It
is designed to run all the time, so its idle resource cost ranks above feature breadth. It does not send
mail, and never will — see [scope](product/scope.md).

This documentation set is **normative**. It is the specification rather than a description of an
implementation, and where the two disagree the specification is what is wrong with the code — not the
other way round.

**An implementation now exists**, and the distinction still matters. What is built is a macOS application
that syncs, renders and triages mail through the hardened pipeline; a command-driven harness that drives
the same application with no window; and one provider adapter, exercised against a recorded corpus. What
is not built is listed in [the roadmap](product/roadmap.md), and nothing below has been rewritten to
describe what happens to exist: a requirement that is not yet met is still a requirement.

## Reading order

New to the project, read these five in order. They take about fifteen minutes and everything else assumes
them.

1. [Scope](product/scope.md) — what Sift is and what it refuses to be
2. [Architecture overview](architecture/overview.md) — the one structural claim everything follows from
3. [Provider model](mail/provider-model.md) — how four providers become one abstraction
4. [Rendering pipeline](rendering/pipeline.md) — how attacker-controlled input becomes pixels
5. [Decisions](decisions.md) — the hundred-odd calls that shaped the rest, and where each is weak

## Indexes

| Index | Contents |
|---|---|
| [Requirements](requirements.md) | Every FR and NFR, with its owning document |
| [Decisions](decisions.md) | Every D-identifier, what was rejected, and why it is contestable |
| [Open questions](open-questions.md) | Unmade decisions (Q) and load-bearing assumptions that may not hold (R) |
| [Glossary](glossary.md) | Terms with a specific meaning here |
| [Limits](limits.md) | Every numeric bound Sift enforces at runtime, and what happens when one is exceeded |
| [State register](architecture/state-register.md) | Every identified state that crosses the shell boundary, with its parameters |

## The graph

**Product** — what is being built, for whom, on what, and in what order.

- [Scope](product/scope.md) · [Platforms and distribution](product/platforms-and-distribution.md) ·
  [Platform baseline](product/platform-baseline.md) ·
  [Reference environment](product/reference-environment.md) · [Roadmap](product/roadmap.md)

**Architecture** — process structure and the layers of the application.

- [Overview](architecture/overview.md) · [Process model](architecture/process-model.md) ·
  [Lifecycle](architecture/lifecycle.md) ·
  [Shell boundary](architecture/shell-boundary.md) · [UI shell](architecture/ui-shell.md) ·
  [Presentation layer](architecture/presentation-layer.md) ·
  [View protocol](architecture/view-protocol.md) ·
  [State register](architecture/state-register.md) · [UI surface](architecture/ui-surface.md) ·
  [Resource broker](architecture/resource-broker.md)

**Mail** — providers, sync, and the only writes Sift performs.

- [Provider model](mail/provider-model.md) · [Accounts](mail/accounts.md) ·
  [Sync engine](mail/sync-engine.md) · [Mutations](mail/mutations.md) · [Threading](mail/threading.md)
- Per provider: [Gmail](mail/providers/gmail.md) ·
  [Microsoft Graph](mail/providers/microsoft-graph.md) · [JMAP](mail/providers/jmap.md) ·
  [Generic IMAP](mail/providers/imap.md)

**Storage** — what is kept, where, and for how long.

- [Data model](storage/data-model.md) · [Cache and blobs](storage/cache-and-blobs.md) ·
  [Search](storage/search.md) · [Encryption](storage/encryption.md)

**Rendering** — the hostile-input pipeline.

- [Pipeline](rendering/pipeline.md) · [Body view isolation](rendering/webview-isolation.md) ·
  [Sanitizer invariants](rendering/sanitizer-invariants.md) ·
  [Content blocking](rendering/content-blocking.md) · [Sender origin](rendering/sender-origin.md) ·
  [Dark mode](rendering/dark-mode.md) · [Link handling](rendering/link-handling.md)

**Runtime** — the behaviours that make an always-on app tolerable.

- [Scheduling and wakeups](runtime/scheduling.md) · [Memory pressure](runtime/memory-pressure.md) ·
  [Network conditions](runtime/network-conditions.md) · [Failure model](runtime/failure-model.md) ·
  [Observability](runtime/observability.md)

**Security** — the adversary and the boundaries.

- [Threat model](security/threat-model.md) · [Credentials](security/credentials.md) ·
  [Privacy](security/privacy.md)

**Build** — how the specification becomes a repository, a binary, and evidence.

- [Build and verification](build/README.md) — [Workspace](build/workspace.md) ·
  [Packaging](build/packaging.md) · [Verification](build/verification.md)

**Cross-cutting** — normative pages that belong to no single area.

- [Limits](limits.md) — the numeric bounds three separate consumers must assert identically

## Conventions

**Normative language.** MUST, MUST NOT, SHOULD, and MAY carry their usual specification force. Prose that
avoids them is context, not requirement.

**No implementation detail.** These documents specify *what* and *why*, never *how* in code. No function
signatures, no schemas as data-definition language, no pinned dependency versions. A named dependency
appears only where the choice **is** the architectural decision, and then with its justification.

**One owner per identifier.** Every FR, NFR, D, I, and L identifier is stated in exactly one document,
next to the design it constrains. Other documents link to it and never restate it. The indexes above are
navigation, not source.

**Identifiers are stable.** They appear in review comments, commits, and tests. Numbers are never reused,
and a dropped requirement is struck through with a reason rather than deleted.

**Numbers are hypotheses.** Every performance and resource figure is a target to be validated against the
[reference environment](product/reference-environment.md), not a measurement. Treating any of them as
established fact is a misreading.

**Decisions carry their weaknesses.** Each decision records what was rejected and why the choice is
contestable. A decision without its counter-argument is an assertion.

**Open questions are not resolved by deletion.** Answering one means writing the answer into its owning
document and striking the entry in [open questions](open-questions.md) with a pointer.

**The design is settled *pending* nine open questions, three of which are load-bearing. That is a
weaker claim than settled, and the distinction between the two groups is the useful part.**

The two that can still move the design: Q-10 leaves every number in this set unfalsifiable until the
reference rig is recorded. And **Q-12 belongs in this group rather than the next one**: it is
usually described as a set of numbers needing measurement, but [its own entry](open-questions.md) and
[process model](architecture/process-model.md) both say a bad Linux figure reopens D-2 — the process
model, which is the second document in the reading order and the thing most of the rest hangs from. A
question that can reverse a settled structural decision is not bounded, whatever its units.

Q-14 joins them for the same reason. It asks where hostile image bytes are decoded, and one of its three
answers is "in a separate process" — which is D-2 again, arrived at from the rendering side rather than
the memory one. Q-15 was the third of this kind until it was answered: D-27's resolved cascade depended
on viewport width while the transform built from it did not, and the answer — a body view pinned to the
width the cascade resolves at — keeps D-27 whole rather than narrowing it.

The remaining six are bounded. Q-9 needs a number per provider and now has a rule that makes its absence
legal in the meantime. Q-13 asks whether one feature works on one platform. **Q-19 asks what a
permanent address commits Sift to, given that nothing self-updates** — the rule
[D-33](product/platforms-and-distribution.md) now states for every address a build calls, reaching the
crash-report upload, which has a decision about its contents,
a row in the egress table, and no address, operator, or answer about whether it ships at all. Q-20 asks
whether one guarantee survives one sandbox, and Q-22 asks for the metric and threshold
[D-64](build/verification.md) requires of every gate and this one lacks.

One of the six is bounded in design terms and urgent in every other sense, because it cannot be
revisited. **Q-21 asks what the vendored dependency tree licenses** — the crates, which neither a
contributor agreement nor a bundled-artefact audit reaches, and where one copyleft crate defeats the App
Store channel whenever that channel returns.

Q-16 — what licence the App Store channel requires of Sift's own code — is now
[D-113](product/platforms-and-distribution.md): the App Store channel is deferred, AGPL-3.0 is kept, and
no contributor agreement is required. The question reopens if and when the channel is reconsidered, before
the first external contribution it would need; until then an external contribution costs nothing, and
after it each one is a consent that reconsidering the store must obtain.

Q-17 — the licences of the filter lists, the sender-infrastructure list and the fonts Sift bundles but
does not own — was Q-16's deadline reached from outside, and is now
[D-112](product/platforms-and-distribution.md): the copyleft public lists ship in the Cask and Flatpak
builds and not in the App Store build, the two lists nobody publishes are written by the Sift project,
and every font is under the Open Font License. What it leaves behind is a channel that would block less by
default than the other two, and two lists that fall under D-113 with the rest of Sift's own code.

Q-8 — the fallback join when the internet message identifier is absent or duplicated — was the fourth, and
is now [D-44](storage/data-model.md). What it leaves behind is a risk rather than a question: R-5, that
the headers D-44 corroborates against are assumed rather than measured to survive transit.

Q-11 — filter-list integrity, an unclosed injection path into every message body — was the first of the
load-bearing group, and is now [D-111](rendering/content-blocking.md): every list ships in the binary and
there is no list-update channel. What it leaves behind is R-11, now permanent for lists as for code. It
also answered Q-18, which asked what the list endpoint's address and payload would commit Sift to once
shipped: with no endpoint, nothing is frozen, and [D-33](product/platforms-and-distribution.md) keeps the
argument as a rule for every address a build calls, including a list endpoint anyone reinstates.

Everything not on that list is decided. **D-59 onward decide the parts a build runs into rather than
reasons about** — the on-disk byte layouts, the boundary's representation, the process lifecycle, the
screens and the action vocabulary, and the project's own [build and verification](build/README.md)
machinery. Those were never open questions, because nobody had asked them; they were the answers an
engineer would have invented alone on the first day, and several of them cannot be taken back.

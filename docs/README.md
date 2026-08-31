# Sift — design documentation

Sift is a desktop mail client for **reading, searching, and triaging** mail across multiple accounts. It
is designed to run all the time, so its idle resource cost ranks above feature breadth. It does not send
mail, and never will — see [scope](product/scope.md).

This documentation set is **normative**. It is the specification, not a description of an implementation;
no code exists yet.

## Reading order

New to the project, read these five in order. They take about fifteen minutes and everything else assumes
them.

1. [Scope](product/scope.md) — what Sift is and what it refuses to be
2. [Architecture overview](architecture/overview.md) — the one structural claim everything follows from
3. [Provider model](mail/provider-model.md) — how four providers become one abstraction
4. [Rendering pipeline](rendering/pipeline.md) — how attacker-controlled input becomes pixels
5. [Decisions](decisions.md) — the forty-three calls that shaped the rest, and where each is weak

## Indexes

| Index | Contents |
|---|---|
| [Requirements](requirements.md) | Every FR and NFR, with its owning document |
| [Decisions](decisions.md) | Every D-identifier, what was rejected, and why it is contestable |
| [Open questions](open-questions.md) | Unmade decisions (Q) and load-bearing assumptions that may not hold (R) |
| [Glossary](glossary.md) | Terms with a specific meaning here |

## The graph

**Product** — what is being built, for whom, on what, and in what order.

- [Scope](product/scope.md) · [Platforms and distribution](product/platforms-and-distribution.md) ·
  [Reference environment](product/reference-environment.md) · [Roadmap](product/roadmap.md)

**Architecture** — process structure and the layers of the application.

- [Overview](architecture/overview.md) · [Process model](architecture/process-model.md) ·
  [Shell boundary](architecture/shell-boundary.md) · [UI shell](architecture/ui-shell.md) ·
  [Presentation layer](architecture/presentation-layer.md) ·
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
  [Network conditions](runtime/network-conditions.md) · [Observability](runtime/observability.md)

**Security** — the adversary and the boundaries.

- [Threat model](security/threat-model.md) · [Credentials](security/credentials.md) ·
  [Privacy](security/privacy.md)

## Conventions

**Normative language.** MUST, MUST NOT, SHOULD, and MAY carry their usual specification force. Prose that
avoids them is context, not requirement.

**No implementation detail.** These documents specify *what* and *why*, never *how* in code. No function
signatures, no schemas as data-definition language, no pinned dependency versions. A named dependency
appears only where the choice **is** the architectural decision, and then with its justification.

**One owner per identifier.** Every FR, NFR, D, and I identifier is stated in exactly one document, next
to the design it constrains. Other documents link to it and never restate it. The indexes above are
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

**The design is settled *pending* five open questions, three of which are load-bearing. That is a weaker
claim than settled, and the distinction between the two groups is the useful part.**

The three that can still move the design: Q-10 leaves every number in this set unfalsifiable until the
reference rig is recorded. Q-11 is an unclosed injection path into every message body, in a product whose
central claim is about hostile input. And **Q-12 belongs in this group rather than the next one**: it is
usually described as two numbers needing measurement, but [its own entry](open-questions.md) and
[process model](architecture/process-model.md) both say a bad Linux figure reopens D-2 — the process
model, which is the second document in the reading order and the thing most of the rest hangs from. A
question that can reverse a settled structural decision is not bounded, whatever its units.

The other two are bounded. Q-9 needs a number per provider and now has a rule that makes its absence legal
in the meantime. Q-13 asks whether one feature works on one platform. Neither reaches a decision.

Q-8 — the fallback join when the internet message identifier is absent or duplicated — was the fourth, and
is now [D-44](storage/data-model.md). What it leaves behind is a risk rather than a question: R-5, that
the headers D-44 corroborates against are assumed rather than measured to survive transit.

Everything not on that list is decided.

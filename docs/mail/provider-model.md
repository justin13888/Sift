# Provider model

The abstraction every provider is reached through.

**Owns:** D-12, D-13, D-30, FR-5, FR-37.

## The rule

Sift MUST NOT build IMAP-with-special-cases. It defines one provider abstraction with a **declared
capability set**, and the sync engine, mutation queue, and UI plan against *capabilities* rather than
provider names.

**The UI binds to capabilities, never to provider names.** If an account declares no tag support, the tag
affordance is absent for that account. If an account's location cardinality is exactly one, "add tag"
renders as "move to folder". A `match` on provider identity anywhere above the adapter layer is a defect.
This is the only way the abstraction survives contact with a fifth provider.

## Capabilities an adapter declares

| Capability | Values | Meaning and UI consequence |
|---|---|---|
| Location cardinality | exactly one; one or more | Whether a message can be in several locations at once. Drives move-versus-add affordances |
| Tag support | none; read-only; read-write (with length and charset limits) | Whether tags exist and whether they can be edited |
| Archive semantics | remove from inbox; move to special-use folder | How the archive intent is realised |
| Trash semantics | move to trash; flag and expunge | How the delete intent is realised |
| Permanent delete | supported or not | Whether the permanent-delete intent is offered at all — see [mutations](mutations.md), which owns its confirmation and no-undo rules |
| Thread operations | native; client fan-out | Whether a thread-level intent is one call or N — see [mutations](mutations.md) |
| Junk reporting | native report; folder move only; none | Whether report-junk and report-not-junk are offered — see [D-40](mutations.md) |
| Delta mechanism | monotonic history cursor; OData delta link; JMAP changes; QRESYNC; full scan | How [sync](sync-engine.md) discovers change |
| Push mechanism | event stream; IDLE; NOTIFY; poll only | How sync learns that change exists |
| ID stability | stable globally; stable per folder; unstable on move | Whether a remote identifier may be used as a join key |
| Server search | per-provider capability set | What can be delegated to the server — see [search](../storage/search.md) |
| Maximum batch size | integer, or unknown | Batching limit for bulk operations. Magnitude-valued: unknown means "plan conservatively", never "unsupported" — see the growth rules below |

Adapter responsibilities are: enumerate folders, produce a delta against a cursor, fetch envelopes, fetch
a specific body part, apply a batch of mutations, and expose a change-notification stream. Nothing more.

## The capability set is open, and that is what makes a fifth provider cheap

This table will grow. [D-32](../storage/data-model.md) already anticipates "a stored capability set that
will gain fields", and D-40's junk-reporting row above is the first one added after the fact. Three rules
make that growth non-breaking, and they are normative:

**An absent capability means unsupported.** Never "assume yes", never "probe and hope". A build reading a
capability set written by an older build, or by an adapter that does not declare a given capability, MUST
treat it as declining the capability — so the affordance is absent, exactly as FR-37 already requires for
tags. Defaulting the other way would turn every new capability into a silent claim that every existing
adapter supports it.

**An unrecognised capability is ignored, not fatal.** An adapter MAY declare a capability the planner does
not know about; the planner MUST ignore it and continue, never refuse the account. This is what lets a
newer adapter's declaration outlive a downgrade, and it is the same direction of failure D-32 chose when
it refused to *read* a newer schema — decline the unknown, do not guess at it.

**A new capability value is additive within its own row.** Adding a value to an existing capability — a
third trash semantic, say — MUST leave the existing values meaning exactly what they meant. A value whose
meaning shifts is a renumbering by another name, and the same rule that protects identifiers protects
these.

**A capability that carries a magnitude declares a conservative default, never "unsupported".** The first
rule reads a missing capability as a refusal, which is right for every row that answers *can it*, and
wrong for every row that answers *how much*. Maximum batch size is the row that exposes this: absent, it
cannot mean "no batching supported", because batching is not a feature an adapter opts into — it is a
limit the server imposes whether or not anyone has looked it up. So a magnitude-valued capability MUST
declare either a value with its source — published limit, measured, or conservative default — or the
explicit **unknown** state, and the planner MUST treat unknown as the most conservative value it can
operate at rather than as a refusal to plan. This is what lets [Q-9](../open-questions.md) sit open in
four adapter tables without any of those tables being ill-typed, and it is the rule a future
magnitude-valued capability — a rate limit, a maximum request size — inherits without further argument.

Together these mean a fifth provider lands as a new adapter and new rows, with no migration for accounts
that already exist. That is the property the whole capability model is for, and it is worth more than any
individual row in the table.

## D-12 — Location and Tags are separate concepts

**Chosen:** model **Location** (where a message is) and **Tags** (many-to-many user labels) as distinct
axes, and map each provider onto both.
**Rejected:** a flat folder model with labels as a Gmail special case.

**Why.** The many-to-many concept exists on effectively every provider; only its name differs. Gmail is
the odd one out not for *having* labels but for **conflating labels with location**.

| Concept | Gmail | Microsoft Graph | JMAP | IMAP |
|---|---|---|---|---|
| Location | system labels | folder (exactly one) | mailbox ids (one or more) | folder (exactly one) |
| Tags | user labels | categories | keywords | custom keywords, where the server permits them |
| Read state | absence of an unread system label | read flag | seen keyword | seen flag |
| Flagged | starred system label | flag | flagged keyword | flagged flag |

**Ruling:** in the Gmail adapter, system labels map to **Location** and user labels map to **Tags**. This
matches the user's mental model, keeps location cardinality meaningful, and makes Gmail structurally
similar to JMAP rather than a special case.

**FR-37.** Tags are a first-class concept distinct from Location, rendered only where the account declares
tag support.

## D-13 — Hand-written client subsets; no additional language runtime

**Chosen:** hand-written request and response types for Microsoft Graph; a generated client for Gmail,
tracking the published schema. No shim in another language.
**Rejected:** a Go or other-language sidecar to reuse an official SDK.

**Why.** Adding a garbage-collected runtime to an app whose primary requirement is idle footprint costs a
GC, a floor of tens of megabytes, pause jitter, and either a third process or a foreign-function boundary.
That attacks the resource targets directly, and it is unnecessary: both proprietary providers publish
machine-readable schemas, so a Rust client is a code-generation or transcription problem, not a
reverse-engineering one.

Between the two Rust paths — generate from schema, or hand-write the subset — the deciding factor is
surface area. A read-and-triage client needs on the order of 12 to 20 endpoints per provider. Graph's
published schema covers an enormous product surface and must be sliced hard before generation is viable,
so hand-writing its mail subset is smaller, faster to compile, and easier to audit, and it gives exact
control over throttling and retry behaviour. Gmail's generated client is already scoped to one API and
tracks schema revisions, so generation is the cheaper path there.

Where clients are hand-written, CI MUST diff the hand-written types against the current published schema
and fail on drift. The schema stays the source of truth for correctness even when it is not the source of
the code.

## D-30 — Rust TLS, verifying against the operating system's trust store

**Chosen:** a Rust TLS implementation, verifying certificates against the platform trust store.
**Rejected:** the platform's own TLS stack on each platform; a bundled root store.

**Why.** Every provider connection carries the user's mail and is authenticated with the user's
credentials, and TLS record parsing is remote-input parsing — the same argument D-8 makes for the core
applies here, and it argues against a large C implementation in the process.

Verification is the half that is easy to get wrong. A **bundled** root store is reproducible and
host-independent, and it breaks generic IMAP immediately: corporate inspection proxies and
enterprise-issued or self-signed certificates are routine on the servers FR-3's manual configuration path
exists to reach. Deferring to the platform trust store means enterprise policy, user-installed roots, and
administrator distrust decisions all work without Sift implementing any of them.

**What it costs:** trust evaluation differs between the two platforms in ways that are visible only on
unusual certificates, so certificate-failure behaviour must be tested per platform rather than reasoned
about once.

**Contestable because:** the platform stack would make trust behaviour exactly right by construction, and
would match what the user's other mail clients do on the same machine. That is a real argument for
consistency; it is outweighed here by keeping a parser of remote input in Rust.

## FR-5 — Special-use folders resolve semantically

Each account exposes a folder or label tree in which special-use locations — inbox, archive, sent, trash,
spam, drafts — are identified **semantically, regardless of localized display names**.

Resolution MUST NOT use string matching on folder names. It uses the provider's own mechanism: the IMAP
special-use extension with its legacy fallback, Graph's stable well-known folder identifiers, Gmail's
system label identifiers, JMAP's mailbox roles. If nothing resolves, Sift MUST prompt the user once and
persist the answer.

A German Exchange server's localized deleted-items folder must not require a locale table. A locale table
is a bug.

## Related

- [Accounts](accounts.md) — adding, configuring, and removing accounts
- [Sync engine](sync-engine.md) — planning against delta and push capabilities
- [Mutations](mutations.md) — planning against archive, trash, and thread capabilities
- Per-provider notes: [Gmail](providers/gmail.md), [Microsoft Graph](providers/microsoft-graph.md),
  [JMAP](providers/jmap.md), [IMAP](providers/imap.md)

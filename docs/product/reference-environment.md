# Reference environment

**Owns:** NFR-26, D-115.

Every performance and resource figure in this documentation set is a **hypothesis to be validated**, not a
measured fact. A number without a defined machine and corpus is unfalsifiable, and arguing about such a
number is wasted effort. This document defines what those numbers are measured against.

The reference rig and corpus MUST be fixed before any performance target is debated or accepted.

## Reference rig

A deliberately unflattering machine, not a current developer workstation: approximately a 2020-era laptop
with 8 GB of memory. Targets that hold only on the newest hardware do not describe the product.

The exact machine specification MUST be recorded here once chosen, and MUST NOT be silently upgraded — a
target met by changing the rig has not been met.

**That machine has not been chosen.** [Q-10](../open-questions.md) stays open on it.

### Interim rig — provisional

Until the reference rig exists, figures are produced on the developer machine below. It is recorded so
that a figure can say which machine produced it. It is **not** the reference rig, and it does not satisfy
the requirement above.

| Property | Value |
|---|---|
| Machine | MacBook Pro, 16-inch, November 2023 (Mac15,7) |
| Chip | Apple M3 Pro: 12 CPU cores (6 performance, 6 efficiency), 18 GPU cores |
| Architecture | arm64 only; page size 16 KB |
| Memory | 36 GB |
| Storage | 512 GB internal solid-state drive, APFS |
| Display | Built-in 3456 × 2234 Retina |
| Operating system | macOS 26.6 |

These rules apply to every figure measured on it:

- **Each figure is labelled provisional and names this rig.** It is still a hypothesis, per the rule in
  the [documentation index](../README.md). It is never a measured fact about the product, and it does not
  pass or fail any NFR or phase gate.
- **It is the opposite of unflattering.** It has four and a half times the reference memory, a
  current-generation chip and a fast drive. A figure that meets its target here is evidence that the code
  path works, not that the target holds. A figure that misses its target here is still evidence that
  it misses.
- **It covers one of D-46's two macOS architectures and none of Linux.** Per [the protocol](#the-protocol),
  nothing measured here stands in for x86-64 or Linux. Those have no rig of any kind.
- **Recording the reference rig replaces this machine; it does not upgrade the reference rig.** No target
  may be accepted on this machine's evidence. When the reference rig is recorded, provisional figures
  are re-measured on it rather than carried forward.

## Reference corpus

Three distinct corpora serve three distinct purposes.

**Scale corpus** — for performance and resource targets. Approximately 5 accounts, 500,000 messages
total, 50,000 of them in an inbox. Synthetic generation is acceptable; the shape matters more than the
content.

The generated corpus settles what those three numbers leave open, so that two runs describe the same
population:

- **The 50,000 are one inbox**, the largest account's. NFR-1's first screen and the unified inbox's merge
  are both paid against the largest folder a person has, and the same count spread across five inboxes
  would measure neither. The other four inboxes are small.
- **The accounts are unequal**, in descending shares, because a person's are: one carries most of their
  mail, and the merge [search](../storage/search.md) performs on every keystroke is dominated by the
  largest index.
- **Every message enters through the path synchronization writes through**, a backfill page at a time,
  and every folder ends synchronized. Rows written any other way would look like the product's and not
  be them, and every figure measured against them would describe the difference. That is also what the
  corpus is for before any figure is taken: it is the first thing to push the ingest path to this
  population, and doing so found per-message lookups that made a first sync cost the square of the
  mailbox.
- **Content is deterministic from a seed**, and shaped like mail where shape matters: threads, a sent
  folder, read and unread, and a share of subjects in scripts the index trigrams rather than segments.
  **Bodies are absent**, which is what a first sync of never-read mail produces; the envelope is the unit
  the default envelope-and-index budget and NFR-5's population are both stated in.
- **It is written into the application's own container**, under keys in the platform credential store,
  so the launch after generation is NFR-1's cold start "with the store already populated".

**Fidelity corpus** — for rendering correctness. Real-world messages spanning marketing HTML, transactional
mail, mailing-list traffic, CJK, RTL, and plain text, plus every published mutation-XSS payload as a
permanent regression vector. See [sanitizer invariants](../rendering/sanitizer-invariants.md). It is
checked into the tree beside the sanitizer, and what it admits is D-115.

### D-115 — The fidelity corpus admits nothing without a recorded provenance

**Chosen:** every message in the fidelity corpus carries one of two provenances. A **captured** message is
real mail, scrubbed of addresses, subjects and content under [NFR-22](../security/privacy.md) before it
is committed — the rule the provider fixtures already follow. A **constructed** message is written for the
corpus after the structure of real mail of its category — table layout, conditional word-processor markup,
preheaders and tracking pixels for marketing mail; captioned, scoped tables for transactional mail; quoted
replies and list footers for mailing-list traffic; ruby, mixed scripts and both writing directions — with
every name, address and host replaced by reserved example domains. Every category MUST be present, and a
file with no provenance fails the corpus's own test. Every mutation-XSS vector carries the publication its
shape comes from, or the earlier vector it varies, and an expectation — accepted with nothing forbidden
surviving, or refused by a bound. **Vector identifiers are permanent**: a vector is never renumbered,
reused or removed, which is what "permanent regression vector" means.
**Rejected:** a corpus of captured mail only, which cannot exist until someone donates mail and scrubs it,
and would leave every gate below with nothing to run over until then; synthetic generation, as the scale
corpus uses, because a generator reproduces its own idea of structure rather than the structure real
senders write — the thing this corpus is for.

**Why.** The gates that depend on this corpus — NFR-26's snapshots, NFR-47's contrast measurement, NFR-40's
dual-parser divergence and fuzzing, and the limits register's rule that the corpus decides whether a bound
is wrong — all need *something* to run over, and each is worth more running today over a small honest set
than waiting for a large captured one. Recording provenance is what keeps the two kinds from being confused:
a constructed message is evidence about the structure it was built to reproduce and about nothing else, and
it says so where the corpus is read. The first run over it found a real defect — the sanitizer was not
idempotent on any message with a head, because the whitespace between head elements moved on a reparse —
which no hand-written invariant test had reached.

**What it costs:** a constructed message proves only the structures its author thought to include. The
quirks nobody has catalogued — a particular sender's broken nesting, a charset declared three different
ways — arrive only with captured mail, and until they do the limits register's "the corpus decides" is
deciding over a sample that was chosen rather than met.

**Contestable because:** a constructed corpus can drift towards the markup the sanitizer already handles,
since its author knows the sanitizer. The provenance column makes the ratio visible and captured messages
are what correct it; nothing yet requires the ratio to move.

**Relevance corpus** — for search ranking. Real queries against a known mailbox, each recorded with the
message the person issuing it was actually looking for. This corpus MUST exist, and it is the one that
does not yet.

It is listed because [D-5](../storage/search.md) gates its own reconsideration on "measured ranking
failure" while conceding that ranking quality is where it is weakest and that ranking *is* the product —
and neither corpus above can measure that. The scale corpus is synthetic, where "the shape matters more
than the content"; the fidelity corpus answers whether a message *renders*, not whether the right one came
back first. A decision whose stated falsification condition has no instrument is settled by default rather
than on evidence.

Synthetic generation is **not** acceptable here, unlike the scale corpus: a generated query has no correct
answer that was not generated alongside it, so a synthetic relevance corpus measures the generator. This
is the only corpus of the three that requires human judgement to build, and that cost is the reason to
size it deliberately rather than aspire to it.

## NFR-26 — Rendering correctness gate

Rendering MUST be correct across the fidelity corpus, verified by visual-regression snapshots per
platform **and** by a macOS-versus-Linux perceptual diff gated in CI on a threshold. Per-platform
snapshots alone would let the two platforms drift apart while each remains self-consistent.

This gate is a CI job, not a manual spot check.

## Measurement discipline

- Memory is measured as `phys_footprint` on macOS and PSS on Linux, never RSS. See
  [observability](../runtime/observability.md).
- Resource and latency targets are gates on every phase, not a final-phase activity. See
  [roadmap](roadmap.md).
- The soak harness that validates long-uptime behaviour runs against this rig and corpus. See
  [observability](../runtime/observability.md).

## The protocol

The rig and the corpus say *what* a number is measured against. They do not say what procedure produces
it, and without that a target is not falsifiable either. Two engineers with this rig and this corpus can
report NFR-5 figures several-fold apart depending only on whether the ninety-fifth percentile is taken
over runs or over the keystrokes within one, and whether the page cache was warm. Every phase gate in the
[roadmap](roadmap.md) rests on the answer.

**Idle** — the state NFR-8, NFR-9, NFR-10, NFR-11 and NFR-15 are stated over — means: every account
synchronized and quiescent, no user input for at least five minutes, no window open where the requirement
says so, and no message arriving. Network events are excluded from NFR-10 by its own text and are excluded
from the others on the same basis; an arriving message is work, and a requirement about idle is not about
work.

**Cold start** — NFR-1 — means a process launch with the operating system's caches cold and the store
already populated from the scale corpus, ending when the message list is interactive: the first screen of
rows is painted from real data and accepts input. It is measured from launch, not from window creation,
and it is deliberately not the common case for a login item — opening a window on a resident process is
NFR-2's number, which is why NFR-1 says so explicitly.

**Latency percentiles** are taken over at least 200 operations spread across at least 5 process launches,
discarding the first operation of each launch, which is a separate measurement rather than a warm-up to be
hidden. Where a requirement's own owning document names a cold path, that path is measured cold:
[D-42](../storage/encryption.md) already requires NFR-5 to be measured with page decryption in place, on
the first search after launch and the first keystroke after an L2 shed, rather than against a warm cache.

**Slope**, for NFR-45, is a least-squares fit over the sampled footprint series with the first hour
discarded, gated on the fitted slope rather than on the difference between endpoints. Endpoints would let
a single sample at either end pass or fail a 72-hour run.

**Both architectures and both platforms report separately, and neither is averaged into the other.**
[D-46](platform-baseline.md) makes page size differ between the two macOS architectures, which changes
what `phys_footprint` reports and what a purge returns, so a single blended figure would describe no
machine that exists.

**One requirement and its instrument disagreed, and the requirement is what moved.** NFR-11 counts "timer
wakeups" while [observability](../runtime/observability.md) instruments timer fires **and socket wakes**.
A socket wake prevents deep sleep exactly as a timer fire does, which is the whole basis of
[scheduling](../runtime/scheduling.md)'s opening argument, so the instrument was measuring the right thing
and the requirement was naming half of it. NFR-11 now covers both — see its owning document.

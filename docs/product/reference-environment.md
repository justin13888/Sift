# Reference environment

**Owns:** NFR-26.

Every performance and resource figure in this documentation set is a **hypothesis to be validated**, not a
measured fact. A number without a defined machine and corpus is unfalsifiable, and arguing about such a
number is wasted effort. This document defines what those numbers are measured against.

The reference rig and corpus MUST be fixed before any performance target is debated or accepted.

## Reference rig

A deliberately unflattering machine, not a current developer workstation: approximately a 2020-era laptop
with 8 GB of memory. Targets that hold only on the newest hardware do not describe the product.

The exact machine specification MUST be recorded here once chosen, and MUST NOT be silently upgraded — a
target met by changing the rig has not been met.

## Reference corpus

Three distinct corpora serve three distinct purposes.

**Scale corpus** — for performance and resource targets. Approximately 5 accounts, 500,000 messages
total, 50,000 of them in an inbox. Synthetic generation is acceptable; the shape matters more than the
content.

**Fidelity corpus** — for rendering correctness. Real-world messages spanning marketing HTML, transactional
mail, mailing-list traffic, CJK, RTL, and plain text, plus every published mutation-XSS payload as a
permanent regression vector. See [sanitizer invariants](../rendering/sanitizer-invariants.md).

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

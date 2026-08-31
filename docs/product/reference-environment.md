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

Two distinct corpora serve two distinct purposes.

**Scale corpus** — for performance and resource targets. Approximately 5 accounts, 500,000 messages
total, 50,000 of them in an inbox. Synthetic generation is acceptable; the shape matters more than the
content.

**Fidelity corpus** — for rendering correctness. Real-world messages spanning marketing HTML, transactional
mail, mailing-list traffic, CJK, RTL, and plain text, plus every published mutation-XSS payload as a
permanent regression vector. See [sanitizer invariants](../rendering/sanitizer-invariants.md).

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

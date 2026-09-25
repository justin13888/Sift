# Gates

D-63 tiers gates by the machine they need, and only the cheapest tier blocks a change.

| Tier | Machine | Blocks | Where |
|---|---|---|---|
| Per-change | ordinary hosted runners | a merge | [`per-change.yml`](per-change.yml) |
| Per-integration | dedicated macOS and Linux machines, on a fixed cadence | a release | [`per-integration.yml`](per-integration.yml) — a hosted scaffold, weekly; the dedicated machines are not yet owned |
| Continuous | the reference rig, indefinitely, reporting a series rather than a verdict | a phase | not yet — needs the dedicated hosts and #3 |

## The per-integration scaffold

Until the dedicated hosts exist, [`per-integration.yml`](per-integration.yml) runs the tier on
hosted runners — one Intel, one Apple silicon — on a weekly schedule and on demand. Each leg
refuses to run unless it is on the architecture it names and not translated, then runs the
tests, the fault-injection tests, and the macOS shell build natively. A separate job runs the
schema-diff gate against the live discovery document.

**What it cannot stand in for**, stated so that its green is not read as more than it is:

- **R-13, compounded.** A hosted image is re-imaged on the provider's schedule, moving the
  macOS version under every gate with no Sift commit. Each run writes the image, OS build,
  page size and Xcode it ran on to its summary, so a moved figure can be put beside a moved
  host — but a baseline taken here is a baseline of an image, not of a machine.
- **No dedicated host.** No soak, no footprint figures, no Linux leg beyond per-change's.
- **No real session.** The three gates below still need a signed build in one.

"Blocks a release" is policy rather than mechanism until a release path checks this
workflow's latest run on the commit it releases.

**Every gate states its metric, its threshold, and how the threshold was derived (D-64),
and a threshold moves only by amendment — never in the run that is failing it.**

Two gates currently report a figure and block nothing, and that state is a roadmap
obligation rather than a permanent option:

- **NFR-26**'s perceptual diff, which needs a metric as well as a number — #15.
- **NFR-47**'s contrast threshold — #26.

## What is deliberately not here

**Three per-change gates need a real macOS session rather than a runner image**, and each
fails *silently* on the wrong host, which is worse than not running: Keychain behaviour
(D-45 binds item access to the creating code's designated requirement), entitlements and
sandbox behaviour including NFR-49's provenance marking — **verified on both macOS
channels rather than once** — and login-item residency, which P0 makes a spike deciding
whether the App Store is a channel at all (#35).

**Signing credentials live with the machines that need them, and the release path is the
only job that holds them.**

## Traceability

A requirement is verified by tests that name it, so "which tests prove NFR-17" is
answerable by search. A gate the roadmap's coverage table names for a phase must have an
instrument before that phase is declared complete; **a requirement with no instrument is
unmet**, not deferred.

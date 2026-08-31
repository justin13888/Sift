# Gates

D-63 tiers gates by the machine they need, and only the cheapest tier blocks a change.

| Tier | Machine | Blocks | Where |
|---|---|---|---|
| Per-change | ordinary hosted runners | a merge | [`per-change.yml`](per-change.yml) |
| Per-integration | dedicated macOS and Linux machines, on a fixed cadence | a release | not yet — needs #21 |
| Continuous | the reference rig, indefinitely, reporting a series rather than a verdict | a phase | not yet — needs #21 and #3 |

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

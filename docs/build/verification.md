# Verification

The gates, the machines that run them, what a pass means, and the harnesses that cannot be added later.

**Owns:** D-63, D-64, D-65.

[Reference environment](../product/reference-environment.md) fixes the rig, the corpora and the
measurement protocol. [Sanitizer invariants](../rendering/sanitizer-invariants.md) fixes how I1–I10 are
tested. Between them they cover one subsystem thoroughly and leave three questions the
[roadmap](../product/roadmap.md) depends on: which machine runs a gate, what number it passes at, and
what instrument exists to run it at all.

## D-63 — Gates are tiered by the machine they need, and only the cheapest tier blocks a change

**Chosen:** three tiers. **Per-change** gates run on ordinary hosted runners and block a merge.
**Per-integration** gates run on dedicated macOS and Linux machines on a fixed cadence and block a
release. **Continuous** gates — the [NFR-45](../runtime/observability.md) soak above all — run
indefinitely on the reference rig, report a series rather than a verdict, and block a **phase**.
**Rejected:** running every gate on every change; treating the soak as a pull-request check.

**Why tiering rather than one gate set.** NFR-45 requires a slope *"over at least 72 hours"*. A 72-hour
job cannot gate a change; attaching it to one either blocks work for three days or is quietly made
advisory, and an advisory memory gate is how [NFR-12](../runtime/memory-pressure.md) — which
[R-2](../open-questions.md) calls the hardest requirement in the set — stops being enforced without
anyone deciding that. Separating what a gate blocks from how often it runs is what lets a slow gate stay
mandatory.

**The machines, because they are a cost nobody had accepted.**
[Reference environment](../product/reference-environment.md) requires that *"both architectures and both
platforms report separately, and neither is averaged into the other"*, and the rig is a 2020-era laptop
class of machine. That is **three dedicated hosts** — an Intel Mac, an Apple-silicon Mac, and a Linux
machine — each occupied for the length of a soak run, none of which a hosted runner supplies. They are
part of the project's standing cost in the same way the corpora are, and a phase gated on a measurement
no machine can take is not gated.

**Until the hosts are owned, the per-integration tier runs on hosted runners as a scaffold**, one
Intel and one Apple-silicon, on the same cadence and blocking the same thing. It exists so the tier
runs and fails loudly rather than being absent, and it buys exactly one property the per-change tier
lacks: every build and test runs **natively on each architecture**, where per-change runs one
architecture's tests translated on the other. Each leg MUST establish that it is on the architecture
it names, untranslated, before running anything — a runner label that resolves to the other
architecture would pass every step and prove nothing, which is the silent failure this section keeps
naming. What the scaffold does **not** supply is stated so its pass is not read as more than it is:

- **It compounds [R-13](../open-questions.md).** A hosted image is re-imaged on the provider's
  schedule, moving the macOS version and its engine under every gate with no Sift commit. Each run
  MUST record the host it ran on — image, OS build, architecture, page size, platform toolchain — so
  a moved figure can be set beside a moved host, but a baseline taken on the scaffold is a baseline of
  an image rather than of a machine, and is never promoted to a reference baseline.
- **It is not a dedicated host.** No soak (NFR-45's 72 hours exceed what a hosted job may run), no
  footprint figures for NFR-8, NFR-9 or NFR-12 (a shared virtual machine is not the
  [reference environment](../product/reference-environment.md)), and no Linux leg beyond the
  per-change tier's.
- **It is not a real session**, so the three gates below remain absent from it.

The dedicated hosts remain this decision's cost; the scaffold defers paying it and does not discharge
it.

**Three per-change gates need a real macOS session rather than a runner image**, and are called out
because each fails silently on the wrong host rather than erroring:

- **Keychain behaviour.** [D-45](../product/platform-baseline.md) binds item access to the creating
  code's designated requirement, so a credential test proves nothing when run against an unsigned binary
  in a headless session.
- **Entitlements and sandbox behaviour**, including NFR-49's provenance marking, which
  [platform baseline](../product/platform-baseline.md) already requires to be *"verified on both macOS
  channels rather than once"* because the mechanism differs between them.
- **Login-item residency**, which [P0](../product/roadmap.md) makes a spike deciding whether the App
  Store is a channel at all.

**Signing credentials live with the machines that need them**, and the release path is the only job that
holds them. This is stated because the alternative — a signing identity available to every job — makes
every gate a supply-chain surface in a project whose [dependency policy](workspace.md) treats that
seriously.

**What it costs:** hardware nobody wants to own, three environments to keep in step, and a release gate
whose feedback arrives long after the change that broke it.

**Contestable because:** a project this size would normally accept hosted runners and blended figures,
and would be right to until the first time a target passes on one architecture and fails on the other
with a single number hiding it. [R-13](../open-questions.md) already describes the neighbouring problem —
a macOS engine version that moves under the rendering gate on somebody else's schedule — and owning the
machines does not fix that one.

## D-64 — Every gate states its metric and its threshold, and a failing run may not move either

**Chosen:** a gate is not a gate until three things are written down: what is measured, what value
passes, and how that value was arrived at. A threshold is recorded here or in the requirement's owning
document, and **changing one requires the same amendment a requirement change requires** — never as part
of the change that is failing it.
**Rejected:** thresholds chosen at the call site; thresholds tuned until the suite is green.

**Why the rule is about *when* a threshold may move.** Three gates in this set specify a procedure and no
value, and the failure mode is not that they are unmeasured — it is that the first number will be chosen
by the engineer whose build is failing, which selects for the number that makes it pass.
[Reference environment](../product/reference-environment.md) already guards the neighbouring case for the
rig — *"a target met by changing the rig has not been met"* — and this is the same rule one column over.
[R-13](../open-questions.md) names the mechanism by which it goes wrong quietly: snapshot regeneration
*"is exactly the operation that can absorb a real regression without anyone seeing it"*.

The three that were procedure without a value:

**NFR-45's slope** is derived rather than chosen, and derives from [NFR-12](../runtime/memory-pressure.md).
NFR-12 permits 5% footprint growth over 14 days, so the soak passes when the fitted slope — least-squares,
first hour discarded, per the measurement protocol — extrapolated over 14 days stays within 5% of that
run's own post-first-hour baseline. Expressing it as a fraction of the run's own baseline rather than as
an absolute byte rate is deliberate: NFR-8 and NFR-9 are placeholders under
[Q-12](../open-questions.md), and a gate keyed on an absolute number would have to be re-derived every
time they move, which is exactly when nobody would notice it had been loosened.

**NFR-26's perceptual diff** needs a metric as well as a number, and the metric is the harder half: a
pixel-difference count fails on font hinting and passes on a colour inversion, which is backwards for
what this gate is for. The metric MUST be perceptual, MUST be recorded here once chosen, and its
threshold MUST be calibrated against pairs from the [fidelity corpus](../product/reference-environment.md)
that are already agreed to render acceptably — so the number describes the corpus rather than the current
build. Until both are recorded the gate reports a figure and blocks nothing, and that state is a
[roadmap](../product/roadmap.md) obligation rather than a permanent option.

*The metric is recorded; the number is not.* [Q-22](../open-questions.md) is answered as far as the
metric and the rule that derives the number, and stays open on the number itself:

- **Metric: ꟻLIP**, the published perceptual difference for rendered images. Each pair — a message's
  macOS render and its Linux render, at the same layout width and device scale — produces a per-pixel
  error map in which a difference counts by how visible it is at a fixed viewing distance, not by how
  many pixels it touches. That is the inversion the gate needs. Hinting and anti-aliasing move glyph
  edges by a fraction of a pixel, which the metric's contrast-sensitivity filtering attenuates at the
  viewing distance; an inverted colour, a missing block or a reflowed column changes colour and
  structure over an area, which it does not. Colour is compared in a perceptual space, so a hue swap at
  equal lightness is an error, which a luminance-only structural measure would not see.
- **Statistic: the 99th percentile of each message's error map**, gated per message. A mean over the
  map lets a localised regression — a dropped button in a long marketing message — dilute to nothing
  across the surrounding white space; a percentile fails whenever at least one per cent of the message
  is visibly wrong. The percentile is itself a hypothesis, fixed here so that it is not chosen by a
  failing build, and it moves only by the same amendment the threshold does. The mean is reported beside
  it and gates nothing.
- **Viewing condition: the reference implementation's default**, fixed and recorded with the threshold,
  because the attenuation above depends on it. Changing it changes every figure, and is the same
  amendment a threshold change is.
- **Both of NFR-26's comparisons use it.** The per-platform snapshot comparison uses the same metric and
  statistic, with its own threshold, so that it measures render nondeterminism and nothing else. Its
  calibration set is every fidelity-corpus message rendered repeatedly by one build on one machine, each
  pair of those renders being one comparison. No person reviews those pairs: one build rendering one
  message is by definition the intended output, so every such pair is agreed without review. What carries
  over from the rule below is the rest of it — the threshold is the largest statistic over the set, with
  no margin, and the same negative controls, qualified the same way, MUST exceed it.

**How the threshold is derived.** The calibration set is the fidelity-corpus messages whose render pair
a person has reviewed and agreed renders acceptably on both platforms, each agreement recorded with the
two engine versions it was made against. The threshold is the **largest statistic over that set**, with no
margin: every agreed pair passes by construction, and a pair worse than the worst one anybody accepted
fails. A margin would be a second number chosen by judgement, which is what D-64 exists to remove. A
failing pair is resolved by fixing the render or by a person agreeing to it — which adds it to the set,
and so moves the threshold, by amendment and never in the run it failed.

**The derivation carries its own falsifier.** Calibration renders also yield negative controls of two
types — a colour inversion and a hue rotation at equal lightness — and, **for each control type**, the
smallest statistic over the renders that qualify for it MUST exceed the threshold. If it does not, the
metric cannot separate what the gate is for from what it is not, and this choice reopens rather than the
threshold moving. Every render qualifies for inversion. A render qualifies for hue rotation only when its
chromatic area is larger than the share of the message the statistic reads — more than one per cent at
the 99th percentile — because a hue rotation leaves achromatic pixels unchanged: a plain-text message
rotates to itself, scores nothing, and says nothing about the metric. What counts as a chromatic pixel is
fixed with the threshold and moves only by the same amendment. A control type no calibration render
qualifies for leaves the falsifier unperformed, which is recorded as such and never read as a pass.

**Rejected:** a pixel-difference count, for the reason above; a luminance-only structural similarity,
which is blind to hue and, having no viewing distance, reads sub-pixel glyph shifts as structure; the
difference measures tuned for image compression, whose purpose is to detect differences at the
threshold of visibility, so that every hinting difference — which is visible, close up — fails; a mean
over the error map, for dilution.

**What is still missing is the number**, and the run that produces it cannot happen yet: it needs render
pairs from both platforms, and the Linux shell is deferred. Until that run is recorded here the gate still
reports and blocks nothing. The remainder is #95; recording the number strikes
Q-22.

**NFR-47's contrast threshold** is deliberately deferred by its owning document to the same corpus, and
that deferral stands. What is new is that the deferral is registered here as an outstanding gate value
rather than left as a sentence in a rendering document, so that the set of gates with no pass condition is
enumerable rather than discovered.

**What it costs:** a gate can exist, run, and block nothing for a period, which is uncomfortable and is
still better than a threshold that means whatever the last failing build needed it to mean.

**Contestable because:** it front-loads calibration work onto a corpus that
[Q-10](../open-questions.md) says does not exist yet, so in the short term it converts two silent gates
into two loud absences. That is the intended trade and it is not obviously the right one if the corpora
slip.

## D-65 — Three harnesses are P0, for the reason the soak harness is

**Chosen:** a fault-injection harness, a provider fixture-and-replay harness, and a command-driven shell
harness are built in P0 alongside the soak harness, before the subsystems they test.
**Rejected:** building each when its phase arrives.

**Why.** [The roadmap](../product/roadmap.md) makes this argument once and applies it to one thing: the
soak harness and allocation attribution *"MUST be built here rather than later; they cannot be
retrofitted"*. The argument is not about memory. It is about instruments that must exist before the code
they measure, and three more requirements have exactly that shape — two of them **Standing** gates,
asserted on the first commit and every commit after, with no instrument specified anywhere.

**The fault-injection harness** serves [NFR-16](../mail/mutations.md), NFR-17 and FR-18: crash
consistency, exactly-once observable behaviour, and replay that never double-applies. Those are not
assertions a unit test makes; they need controlled termination at chosen points, interrupted and
reordered writes, and a restart that inspects the result. It is the only thing that can catch
[failure model](../runtime/failure-model.md)'s durability ordering — *"an intent MUST be durably enqueued
before it is applied optimistically"* — because the violation is invisible in every run that does not
crash at the one instruction between the two. It is compiled only into test builds, per
[workspace](workspace.md)'s flag register.

**The provider fixture-and-replay harness** serves all four adapters and the capability model. Recorded
exchanges are what make an adapter testable without an account, what make NFR-29's *"graceful, surfaced
degradation on limited IMAP servers"* assertable against servers nobody has, and what turn
[D-31](../mail/providers/imap.md)'s own warning — that *"small subset of IMAP"* has defeated
better-resourced projects — into a growing corpus rather than a hope. Fixtures are captured from real
servers and **MUST be scrubbed of addresses, subjects and content before they are committed**, which is
[NFR-22](../security/privacy.md)'s rule reaching the test tree; a fixture set that carries correspondence
metadata is a privacy defect that outlives every release.

**The command-driven shell harness** is [FR-24](../architecture/ui-shell.md)'s own claim taken
seriously. That requirement says full keyboard operability *"is also what makes the app testable without
UI automation"* — which is an assertion that a test can drive the application through its action
vocabulary. That is only true if the action registry is a real surface reachable from a test, so the
harness and the registry are one thing, and the registry crosses the boundary under
[D-56](../architecture/presentation-layer.md) like every other identified thing.

**The soak harness stands on two of these.** It drives the assembled application through the surface
the command-driven harness drives, over the replay harness's recorded corpus, so a 72-hour run needs no
account and no network and measures nothing but Sift. Its slope gate is the derived value above, fitted
once and reused for the per-subsystem decomposition, and it runs in the continuous tier and nowhere else.
Its companion, NFR-44's overhead benchmark, has a pass, a failure, **and a no-verdict band**: a machine
too noisy to resolve 2% says so rather than reporting whichever side of the line the noise landed on,
which is what keeps it off the hosted scaffold for the same reason the soak is. The method is in
[observability](../runtime/observability.md#nfr-45--soak-harness).

**What it costs:** three harnesses in a phase that ships no features, on top of the two the roadmap
already puts there. P0 is now most of the project's testing infrastructure and none of its product.

**Contestable because:** front-loading five harnesses before a vertical slice exists risks building them
against a design that then moves, and the honest counter-argument is that a harness written after the
subsystem fits it better. The answer is the roadmap's own: fitting better is worth less than existing at
all, for properties that are unobservable in any single run.

## Traceability

**A requirement is verified by tests that name it.** [docs/README](../README.md) already says
identifiers exist to be referenced *"in commits, reviews, and tests"*; this makes the consequence
explicit, so that "which tests prove NFR-17" is answerable by search rather than by memory.

**A gate the coverage table names for a phase MUST have an instrument before that phase is declared
complete.** The [roadmap](../product/roadmap.md) forbids completing a phase while a requirement in its
Ships column is unmet, and a requirement with no instrument is unmet in the only sense that matters —
nobody can say whether it holds.

## The schema-diff job

[D-13](../mail/provider-model.md) requires CI to diff hand-written Graph types against the published
schema. That job depends on a third party's document at a third party's address, which makes it the one
gate that can fail for reasons unrelated to any change.

**It runs on a cadence rather than per change, its failure blocks a release rather than a merge, and an
unreachable schema is reported as unavailable rather than as a pass.** The last clause is the one that
matters: a fetch failure silently treated as "no diff" turns the gate off permanently the first time the
address moves, which is the failure D-13 exists to prevent, arriving through the mechanism meant to
prevent it.

## Related

- [Reference environment](../product/reference-environment.md) — the rig, the three corpora, and the
  measurement protocol these gates apply
- [Observability](../runtime/observability.md) — NFR-45 and the instrumentation the soak samples
- [Sanitizer invariants](../rendering/sanitizer-invariants.md) — NFR-40's five methods, the one
  subsystem that already had a taxonomy
- [Roadmap](../product/roadmap.md) — the phase gates these serve
- [Workspace](workspace.md) — the dependency gates, and the flag the fault-injection harness lives behind

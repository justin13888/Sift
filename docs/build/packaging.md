# Packaging

How a Rust core and a Swift shell become one bundle, what a version means across three channels, and how
far back support reaches.

**Owns:** D-61, D-62.

[Platform baseline](../product/platform-baseline.md) settles the identifiers, the deployment floor, the
entitlements and the on-disk layout. [Platforms and distribution](../product/platforms-and-distribution.md)
settles the channels themselves under [D-33](../product/platforms-and-distribution.md). Neither says how
the artefact is assembled, what number is on it, or when a build stops being one Sift can still open the
store of. Those are the three things here.

## D-61 — The native toolchain drives, and the Rust core is a library it links

**Chosen:** on macOS the platform's own application toolchain owns the bundle and the signing, and the
Rust core is built into a static library it links; the Rust workspace never produces the `.app`. On Linux
the relationship inverts — Cargo owns the binary and the GTK shell is Rust in the same workspace.
**Rejected:** Cargo driving the platform toolchain as a build step; a bespoke script assembling the bundle
from parts.

**Why the native toolchain owns the macOS side.** Everything [D-45](../product/platform-baseline.md) and
the entitlement set commit to is expressed in the platform's own project format and enforced by its own
signing machinery: two sandboxed configurations from one identity, an access group prefixed by the team
identifier, an address-book usage description, registered URL types, a login item, and a universal
artefact under [D-46](../product/platform-baseline.md). A build system that reimplements that assembly
owns the correctness of code signing, and code signing failures are discovered at submission, which is
the worst place to discover them. Being an ordinary application project that happens to link a static
library keeps every one of those on rails.

**Why a static library rather than a dynamic one.** A dynamic library inside a sandboxed, notarized
bundle is a second signed artefact with its own load path and its own hardened-runtime posture, bought in
exchange for a link step nobody needs — the two halves ship together, always, and there is no third
consumer. It also keeps [D-59](workspace.md)'s ABI leaf genuinely a leaf: one archive, one symbol
surface, and no runtime resolution that could find a different one.

**Why the inversion on Linux is not an inconsistency.** [D-1](../architecture/ui-shell.md) makes the GTK
shell Rust, so there is no second toolchain to hand the build to and no bundle format to honour —
the Flatpak manifest wraps an ordinary Cargo binary. The asymmetry is D-1's, not this decision's.

**What it costs:** the macOS build is not reproducible from `cargo build` alone, so a contributor needs
the platform toolchain to produce anything runnable, and CI needs a macOS host for even a smoke build.
[Verification](verification.md) already needs macOS hosts for other reasons, which is the only thing that
makes this affordable.

**Contestable because:** it means the two platforms are assembled by different systems, so a build-time
concern — a feature flag, a vendored source path, an embedded asset — must be expressed twice and can
drift in exactly the way [D-17](../architecture/shell-boundary.md) refuses to let the shells drift. The
mitigation is that the list of such concerns is short and enumerated in [workspace](workspace.md); if it
grows, this decision is the thing to revisit.

## D-62 — One version, three channels, and a support floor that is stated rather than implied

**Chosen:** a single user-facing version identifies a source revision and is the same on every channel; a
channel's own build counter is a packaging detail and never a version. Support is expressed as a
**migration floor**: the oldest schema version a current build can still migrate forward from, stated in
[data model](../storage/data-model.md)'s terms, advanced only deliberately, and never advanced past a
build that is still plausibly installed.
**Rejected:** per-channel versions; an unbounded migration chain; a time-based support window.

**Why one version.** [D-33](../product/platforms-and-distribution.md) ships two channels from the first
release and defers a third, the App Store, each with independent review and packaging latency, so at any
moment they carry different builds. If each channel also carried its own numbering, no user could answer "what do you have"
and no bug report could be matched to a revision. One version across all three makes the channels differ
in *when* rather than in *what*. The App Store build number space that
[platform baseline](../product/platform-baseline.md) already reserves keeps doing its own job underneath
this — monotonic for the life of the record — and is not what a user or an issue is keyed on.

**Why the support floor is the sharp end, and why nobody had accepted it.** Two settled decisions compose
into an obligation neither of them states. [D-32](../storage/data-model.md) makes migrations
**forward-only**, so a build must migrate from whatever it finds. D-33 removes any ability to make a user
upgrade, and its own section already observes that under Flatpak or a Cask a build a user never
upgrades stays installed for what *"may be years"*. Composed, the migration chain must reach back to the oldest build still in the
field, **forever**, and nothing in this set had said how far that is or that it is ever pruned. That is a
permanent maintenance liability accepted by omission, which is the failure mode this documentation set
otherwise refuses everywhere.

Making it a floor rather than a promise of eternity does two things. It bounds the migration code a build
must carry and test, which would otherwise grow without limit and be exercised by nobody. And it makes
retiring a step a **decision with a visible cost** — a user below the floor meets a refusal to open their
store, which under D-32's own rules means removal and resync rather than corruption, and which
[failure model](../runtime/failure-model.md) already has a shape for. Advancing the floor is therefore
amending this document, exactly as moving a requirement between phases is amending the
[roadmap](../product/roadmap.md).

**What it costs:** a migration chain that is longer than any single release needs, kept compiling and
kept tested against fixtures of every schema version above the floor. That fixture set is a
[verification](verification.md) obligation, not a nice-to-have: an untested migration is worse than an
absent one, because it runs.

**Contestable because:** a floor is a number nobody can choose correctly before the product has an
install base, and choosing it early risks either carrying dead code for years or stranding users the
project never knew it had. The alternative — never prune — is not obviously worse until the first time
somebody has to prove a six-year-old migration still works.

## Signing and notarization differ by channel, and only one of them is the store's problem

The App Store path is signed and reviewed by the platform. **The Cask build is not**, and it needs its
own path: a distribution certificate under the same team identifier [D-45](../product/platform-baseline.md)
fixes, the hardened runtime, and notarization before the artefact is published — with stapling, so that a
first launch does not depend on the user being online.

The hardened runtime is not a formality here. [Threat model](../security/threat-model.md) puts an
attacker who enters Sift's address space out of scope **because the platform prevents it**, and names
that as a boundary Sift MUST NOT weaken. On the Cask path Sift chooses its own runtime posture, so an
entitlement that re-permits debugger attachment moves a whole class of attack into scope with no document
here changing. That is the same obligation the entitlement set in
[platform baseline](../product/platform-baseline.md) carries, arriving from the signing side.

## The channel artefacts have owners, and the owner is part of the channel

D-33 chooses three channels and stops. A channel is not only a format:

- **The Homebrew Cask** is a definition in a tap. Its zap stanza names the on-disk layout and the
  Keychain service and access-group names, both of which
  [platform baseline](../product/platform-baseline.md) records as effectively permanent *because* they
  are published there. Whether the tap is the project's own or a submission to the central one is a
  choice that binds those strings either way.
- **The Flatpak manifest** carries the application id, which is simultaneously the D-Bus well-known name,
  the desktop-file name, the portal identity, and the data root — so it is the same permanence the
  identifier register describes, in a file maintained on somebody's schedule.

**Each MUST have a named owner in this repository before the first release**, for the reason
[R-11](../open-questions.md) already gives about urgent fixes: a channel whose artefact nobody owns has a
release path that works until the day it matters.

## Release notes are a requirement of having no self-update

D-33 gives delivery to platform channels, so a user learns what changed from the channel's own listing
and from nothing else — there is no in-app update surface to carry it, by construction, and FR-26 only
surfaces that a restart is needed.

**Every release MUST carry notes stating what changed, and a release that fixes a security defect MUST
say so.** Under [R-11](../open-questions.md) a fix reaches users on App Review's and the distribution
system's schedule; a user who cannot tell an urgent release from a routine one has no way to prioritise
the one action available to them, which is to take the update sooner.

## Related

- [Platform baseline](../product/platform-baseline.md) — D-45, D-46, the identifier register, the
  entitlements and the on-disk layout this document spends
- [Platforms and distribution](../product/platforms-and-distribution.md) — D-33 and D-15, the channels
- [Workspace](workspace.md) — what is built
- [Verification](verification.md) — the machines that sign, notarize and test these artefacts
- [Data model](../storage/data-model.md) — D-32, whose forward-only rule the migration floor bounds

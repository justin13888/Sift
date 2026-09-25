# Platforms and distribution

**Owns:** D-9, D-15, D-33, D-112.

## D-9 — macOS first, then Linux. Windows is out of scope.

**Chosen:** macOS ships first; Linux second, behind an explicit go/no-go decision point.
**Rejected:** simultaneous macOS + Windows + Linux.

**Why.** Narrowing to two platforms collapses the rendering problem to a *single engine family*: WKWebView
and WebKitGTK are the Cocoa and GTK ports of WebKit, sharing WebCore, the CSS engine, and the HTML parser.
Layout behaviour converges strongly in a way it would not with Chromium in the mix. This narrowing is what
makes [D-1](../architecture/ui-shell.md) — native shells with WebKit only for message bodies — affordable,
and it removes WebView2 from the toolbox entirely.

**Contestable because:** it forgoes the largest desktop market, and it makes the Linux shell a second full
UI implementation rather than a recompile. That second shell is native — GTK4 and libadwaita, settled in
[UI shell](../architecture/ui-shell.md) — so the cost is real and accepted rather than open.

## D-15 — Distribute Linux builds via Flatpak

**Chosen:** Flatpak as the primary Linux distribution channel.
**Rejected:** distro packages against the system WebKitGTK.

**Why.** Version skew is the dominant Linux rendering risk: WebKitGTK on a conservative stable distro can
trail a rolling distro by a year or more. The [dark-mode transform](../rendering/dark-mode.md) and
[cosmetic filtering](../rendering/content-blocking.md) both depend on selector support that varies across
that range, and compiled content-rule storage requires a recent-enough engine. Flatpak pins the runtime,
which converts an unbounded compatibility matrix into one known version.

A **minimum WebKitGTK version MUST be declared and enforced at startup**; running below it MUST fail
loudly rather than degrade silently.

Flatpak constrains more than the engine version, and two of those constraints reach decisions made
elsewhere. [D-14](../runtime/network-conditions.md) reads network state from NetworkManager over D-Bus,
which requires a permission Flathub may resist; the desktop portal's network monitor is coarser and
supplies neither the metered flag nor the link class D-14 depends on. Credential access likewise runs
through the secrets portal rather than the session bus directly. Both are
[tracked as risks](../open-questions.md) rather than assumed away.

## D-33 — Platform channels only; Sift never updates itself

**Chosen:** deliver through each platform's own channels — Homebrew Cask on macOS, Flatpak on Linux — from
the first release. Sift implements no update mechanism. **The Mac App Store is a deferred channel, not a
rejected one:** it does not ship with the first release, and it is reconsidered only together with the
licensing question D-112, below, reopens.
**Rejected:** a signed delta self-updater; the App Store from the first release, which would require a
contributor agreement now (see D-112).

**Why.** An application that is resident on a user's machine and reads their mail is the wrong place to
put a bespoke code-delivery path. Platform channels bring signing, transport, rollback and revocation that
are maintained by people whose job that is, and they are what users already trust for everything else.

**One path once survived this reasoning without ever having been measured against it.** Filter lists and
the sender-infrastructure list were to update from a source Sift operates, and what arrives becomes CSS
injected into every message body. That is content delivery rather than code delivery, but the gap between
the two is narrower than the words suggest, and the sentence above applies to it more nearly than to
anything else Sift does. [D-111](../rendering/content-blocking.md) measures it against this decision and
removes it: every list ships in the binary and reaches users through these same channels.

The two macOS channels reach different people, and neither reaches the other's audience: Homebrew Cask is
how technically-inclined macOS users install software, while the App Store is where everyone else looks.
They differ only in packaging and entitlements, not in the process model — a consequence of
[D-2](../architecture/process-model.md), which replaced a background agent with an ordinary login item,
the sandboxed path the App Store supports. That is why deferring the store costs no architecture: the Cask
build is already sandboxed under one identity ([D-45](platform-baseline.md)), so adding the store later is a
packaging and review path, not a redesign.

**What it costs:** until the App Store channel is reconsidered, Sift on macOS reaches only the
technically-inclined half of its audience. Under Flatpak, updates arrive when the user's system decides, so
a security fix lands on the platform's schedule rather than Sift's. Reconsidering the store later brings
back what deferring it saves: a second macOS build configuration and review-and-release path, and App
Review latency with no way to bypass it for an urgent fix.

**Contestable because:** giving up self-update means giving up any ability to push an urgent fix, in a
product whose primary adversary sends attacker-controlled input by design. If a serious vulnerability ever
needs to reach users faster than the slowest channel allows, this is the decision that prevented it.

## Licensing is a distribution constraint, and the only one that cannot be undone

Distribution through the App Store requires that Sift's copyright holder be able to license it under the
terms the store imposes, which a copyleft licence does not permit a mere licensee to do. The repository is
licensed AGPL-3.0, so this is live rather than hypothetical: the copyright holder can dual-license today
because they hold all of the copyright, and that stays true only for as long as every contribution arrives
under an agreement preserving it.

**It is the only irreversible item in this documentation set.** Every other decision here can be revisited
by amending a document. This one cannot: a contribution that lands uncovered cannot be covered afterwards
once its author is unreachable or unwilling. D-33 defers the channel that creates the constraint, so D-112
does not spend that once; it states when it comes due.

## D-112 — AGPL-3.0 only; no contributor agreement while the App Store is deferred

**Chosen:** the public licence stays AGPL-3.0, and contributions are accepted under it, inbound as outbound,
with no contributor licence agreement, no relicensing right, and no commit trailer. The Mac App Store
channel that would need more is deferred by D-33. **The agreement question reopens if and when that channel
is reconsidered, and it MUST be answered before the first external contribution that channel would need**
— that is, before the store build would contain any contribution not the copyright holder's own.
**Rejected:** requiring a contributor licence agreement now; relicensing permissively; keeping the App
Store in the first release.

**Why.** The App Store is the only thing that asks the copyright holder for more than AGPL-3.0 grants
everyone, and the first release does not use it. An agreement required now would buy a relicensing right
for a channel that is not shipping, at the price every contributor agreement carries: some contributors
decline any agreement on principle, and the rest carry a formality the first time they contribute.
Relicensing permissively is irreversible outright — every release made under it stays under it — and it
gives up the one thing the copyleft licence was chosen for: that nobody ships a modified mail client that
reads their users' mail without publishing what it changed.

**What it costs:** the store is deferred, not free. Every external contribution merged under AGPL-3.0 alone
is one the holder cannot ship under the store's terms without that contributor's later consent. So
reconsidering the channel after external contributions have landed means obtaining each author's consent
or replacing their work, and that cost grows with every contribution. While all of the copyright is the
holder's own, reconsidering costs nothing.

**Contestable because:** it trades the reach D-33 names — the macOS users who look only in the App Store —
for contributor goodwill that has not yet been tested, in a project whose [R-1](../open-questions.md)
escape hatch assumes a contributor community. If the store turns out to matter before any external
contribution lands, the agreement can still be adopted at no loss.

Whatever the answer when it reopens, it governs Sift's own copyright only. No contributor agreement can
relicense somebody else's work, so the artefacts Sift bundles but does not own and the dependency tree it
vendors — [Q-17](../open-questions.md) and [Q-21](../open-questions.md) — stand against the same channel
on their own.

The versions, architectures, entitlements and permanently-consumed identifiers those channels commit Sift
to are in [platform baseline](platform-baseline.md), which owns D-45 and D-46.

## Per-platform surface

The platform-specific surface MUST stay confined to the layers below. Everything else is portable Rust.

| Concern | macOS | Linux |
|---|---|---|
| App shell | AppKit | GTK4 + libadwaita |
| Body rendering engine | WKWebView | WebKitGTK |
| Background lifecycle | login item | desktop portal autostart |
| Credential storage | Keychain | Secret Service |
| Memory-pressure signal | dispatch memory-pressure source | cgroup v2 PSI |
| Network conditions | system path monitor | NetworkManager over D-Bus |
| Memory metric | `phys_footprint` | PSS |

See [UI shell](../architecture/ui-shell.md), [process model](../architecture/process-model.md),
[memory pressure](../runtime/memory-pressure.md), [network conditions](../runtime/network-conditions.md),
and [observability](../runtime/observability.md).

## Font and colour divergence

Fonts will cause more visible cross-platform difference than layout will. The same family list resolves
through different font stacks to faces with different metrics; different metrics change line wrapping,
which shifts table-based email layouts. Emoji and CJK fallback chains differ likewise.

Sift MUST bundle a font set and pin the fallback chain explicitly in the base stylesheet injected into
message bodies, rather than inheriting the platform default. Colour management also differs — macOS is
colour-managed end to end, Linux varies by compositor — which matters because the dark transform's colour
maths assumes a known working space.

Verification is a macOS-versus-Linux perceptual diff over the corpus, gated in CI. See
[reference environment](reference-environment.md).

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

**Chosen:** deliver through each platform's own channels — the Mac App Store and Homebrew Cask on macOS,
Flatpak on Linux — all from the first release. Sift implements no update mechanism.
**Rejected:** a signed delta self-updater; a single channel per platform.

**Why.** An application that is resident on a user's machine and reads their mail is the wrong place to
put a bespoke code-delivery path. Platform channels bring signing, transport, rollback and revocation that
are maintained by people whose job that is, and they are what users already trust for everything else.

**One path once survived this reasoning without ever having been measured against it.** Filter lists and
the sender-infrastructure list were to update from a source Sift operates, and what arrives becomes CSS
injected into every message body. That is content delivery rather than code delivery, but the gap between
the two is narrower than the words suggest, and the sentence above applies to it more nearly than to
anything else Sift does. [D-111](../rendering/content-blocking.md) measures it against this decision and
removes it: every list ships in the binary and reaches users through these same channels.

**Every address a shipped build calls is permanent.** Nothing here can make a user upgrade, so a build
keeps calling the addresses it shipped with for as long as it stays installed — and under Flatpak or a
Cask a user never upgrades, that may be years. An endpoint Sift operates is therefore a commitment to
answer at that address, in that payload format, for the life of the oldest build still in the field. It
is cheap to shape before the first release and impossible to reshape after it. The list endpoint was
where this first applied, and D-111 answers it by not building the endpoint: the first release fixes no
list address and no list payload. Two obligations stand in its place:

- **Reinstating a list endpoint reopens D-111, and the permanence returns with it.** An endpoint of that
  kind MUST carry a version in its address from the outset and serve a payload that states its own
  version, both fixed before the first build that calls it ships. Neither can be added afterwards,
  because the builds already in the field would never request the versioned form.
- **The same argument applies to every other address a build calls.** The crash-report endpoint is
  [Q-19](../open-questions.md), and the connectivity probe [D-96](../runtime/network-conditions.md)
  declined is permanent for the same reason.

Two macOS channels rather than one because they reach different people, and neither reaches the other's
audience: Homebrew Cask is how technically-inclined macOS users install software, while the App Store is
where everyone else looks. They differ only in packaging and entitlements, not in the process model — a
consequence of [D-2](../architecture/process-model.md), which replaced a background agent with an ordinary
login item, the sandboxed path the App Store supports. One difference is in what they carry rather than
how they are packaged: [D-112](#d-112--bundle-only-what-each-channels-licence-can-carry) leaves the
copyleft filter lists out of the App Store build, because that channel cannot carry their terms.

**What it costs:** two macOS build configurations and two review-and-release paths, plus App Review
latency on one of them with no way to bypass it for an urgent fix. Under Flatpak, updates arrive when the
user's system decides, so a security fix lands on the platform's schedule rather than Sift's.

**Contestable because:** giving up self-update means giving up any ability to push an urgent fix, in a
product whose primary adversary sends attacker-controlled input by design. If a serious vulnerability ever
needs to reach users faster than the slowest channel allows, this is the decision that prevented it.

## Licensing is a distribution constraint, and the only one that cannot be undone

Distribution through the App Store requires that Sift's copyright holder be able to license it under the
terms the store imposes, which a copyleft licence does not permit a mere licensee to do. The repository is
licensed AGPL-3.0, so this is live rather than hypothetical: the copyright holder can dual-license today
because they hold all of the copyright, and that stays true only for as long as every contribution arrives
under an agreement preserving it.

**Contributions MUST therefore be accepted under such an agreement.** This sits in a distribution document
because the App Store channel is what requires it — remove that channel and the requirement goes with it.

**It is the only irreversible item in this documentation set.** Every other decision here can be revisited
by amending a document. This one cannot: a contribution that lands uncovered cannot be covered afterwards
once its author is unreachable or unwilling. Everything else the set calls contestable stays contestable.
This is contestable exactly once, and its deadline is the first external contribution rather than any
date.

The alternatives are real and none of them has been argued here, so it is
[an open question](../open-questions.md) rather than a settled decision.

The versions, architectures, entitlements and permanently-consumed identifiers those channels commit Sift
to are in [platform baseline](platform-baseline.md), which owns D-45 and D-46.

## D-112 — Bundle only what each channel's licence can carry

The section above is about Sift's own copyright. This one is about the artefacts Sift ships and does not
own — the public filter lists, the fonts, and anything a list is derived from — which **no contributor
agreement can relicense**, so a single copyleft or share-alike artefact defeats the App Store channel with
that agreement fully in place. Naming them is the decision, because the names are what carry the terms.

**Chosen:**

| Artefact | What ships | Terms | Channels |
|---|---|---|---|
| Public blocking list | EasyList | dual GPL-3.0-or-later and CC BY-SA 3.0-or-later; taken under the GPL | Homebrew Cask, Flatpak |
| Public privacy list | EasyPrivacy | as EasyList | Homebrew Cask, Flatpak |
| Bundled email list | written by the Sift project, in the tree | Sift's own | every channel |
| Sender-infrastructure list | curated by the Sift project, in the tree | Sift's own | every channel |
| Fonts | the set named under [font and colour divergence](#font-and-colour-divergence) | SIL Open Font License 1.1, every face | every channel |

**Rejected:** shipping the public lists in every channel and relying on the share-alike licence's
permission; dropping the App Store channel so that every build carries the same lists; replacing the public
lists everywhere with permissively licensed ones; lists whose terms forbid commercial use.

**Why the public lists split by channel.** The mature uBlock-syntax lists that FR-27 was written for are
all copyleft: EasyList and EasyPrivacy as above, uBlock Origin's own lists and AdGuard's under GPL-3.0. The
widely used list that is not, Peter Lowe's, forbids any use that makes anybody money, and a list Sift may
ship only while nobody is paid is not one it can build a channel on. The GPL option is the right one to take because
Sift itself is AGPL-3.0, which the GPL's own terms allow to be combined with GPL-3.0 work — so the Cask and
Flatpak builds carry these lists on the same footing as Sift's own code. The App Store build cannot:
the store's usage rules add restrictions that GPL-3.0 forbids a distributor to add, and the share-alike
option does not escape it, because CC BY-SA forbids applying technological measures that restrict what a
recipient may do with the material, which the store's delivery does. Dropping the channel to keep one
list set would decide [Q-16](../open-questions.md) as a side effect of a question about somebody else's
lists; replacing the public lists everywhere would degrade the two channels that can carry them to match
the one that cannot.

**The App Store build is not left without the lists; it is left without Sift distributing them.** FR-27's
custom rules are imported from a file the user chooses, so a user who wants EasyList in that build obtains
it and imports it, and the licence governs their copy, not Sift's distribution. A subscription carried over
from a Cask installation — the two builds share one identity under D-45 — to a list the running build
does not carry is kept and shown as inactive, never silently dropped and never silently satisfied.

**Sift writes the other two lists itself.** The email list and the
[sender-infrastructure list](../rendering/sender-origin.md) are authored in the tree, so they are Sift's
own copyright and fall under the contributor agreement [Q-16](../open-questions.md) asks about rather than
under this section. **Neither may be derived from a copyleft list**, since that would import the terms this
decision exists to keep out of the App Store build; an entry is justified from the provider's own
documentation or from observed mail, and records which.

**Is the compiled form an adaptation?** Treated as one, and it does not matter where it ships. The rules
compiled for the engine and the content-blocking backstop are derived from a list at build time, so they
ship in exactly the builds that carry that list, under its terms and with its notices. The stylesheet
generated from cosmetic rules is produced on the user's machine and injected into a body there; it is
never conveyed to anyone, so neither licence's distribution conditions reach it.

**Every build carries the notices of what it bundles** — each artefact's licence text and the attribution
its terms ask for, EasyList's included — and **a font is shipped unmodified**. Subsetting or otherwise
altering a face makes it a modified version under the Open Font License, which may not use a reserved font
name; shipping the published files avoids renaming them and the obligation with it.

**What it costs.** The App Store build's blocking is weaker than the other two by default. Remote content
is off until a sender is allowed, FR-29's heuristics are Sift's own and run everywhere, and the email list
is where email tracking is actually covered, so the loss falls on senders the user has already allowed —
but it is a real difference between two builds that otherwise look identical, and the one reaching the
less technical audience is the weaker one. Unmodified fonts cost install size, the CJK faces above all, and
Sift has no install-size budget that says whether that is acceptable.

**Contestable because:** a channel whose blocking is weaker by construction is a product difference the
user cannot see from the store listing. If a permissively licensed list of comparable coverage appears, or
EasyList's authors grant an exception, the split should close; if the App Store channel is ever dropped
under Q-16, it closes by itself.

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
message bodies, rather than inheriting the platform default. The set is chosen for metric
compatibility, because a face with the metrics of the family an email names wraps the way its author saw
it, and that is what keeps a table layout from shifting:

| Family an email names | Bundled face |
|---|---|
| Arial, Helvetica, and the generic sans-serif | Liberation Sans |
| Times New Roman, Times, and the generic serif | Liberation Serif |
| Courier New, Courier, and the generic monospace | Liberation Mono |
| Calibri | Carlito |
| Cambria | Caladea |
| Georgia | Gelasio |

The fallback chain after the named face is pinned in this order: Noto Sans for any Latin, Greek or
Cyrillic character the named face lacks, then Noto Sans CJK, then Noto Color Emoji. Every face is under
the SIL Open Font License 1.1, and [D-112](#d-112--bundle-only-what-each-channels-licence-can-carry) is why that is a
requirement rather than a coincidence. A family not in the table — Verdana and Tahoma are the common ones,
and no metric-compatible face under a licence every channel can carry is known for either — resolves to
the generic family it declares. **A script outside the chain falls through to the platform**, and that is
the divergence left open: it is named here rather than discovered in a snapshot.

**This set is the rendering baseline, so it is chosen before the corpora are built.** Replacing a face
later changes wrapping everywhere that face is used, which invalidates every NFR-26 snapshot and NFR-47
measurement at once. Colour management also differs — macOS is
colour-managed end to end, Linux varies by compositor — which matters because the dark transform's colour
maths assumes a known working space.

Verification is a macOS-versus-Linux perceptual diff over the corpus, gated in CI. See
[reference environment](reference-environment.md).

# Channel artefacts

D-33 delivers through **Homebrew Cask on macOS and Flatpak on Linux from the first release.**
The Mac App Store is deferred, not rejected, and is reconsidered only together with D-113's
licensing question. Sift implements no update mechanism, and FR-26 requires it
detect that its own bundle has been replaced and surface a restart prompt rather than
continuing against replaced resources.

**Each of these files must have a named owner in this repository before the first release.**
That is not administrative: each carries values that become permanent on publication.

| Artefact | What it makes permanent |
|---|---|
| [`homebrew/sift.rb`](homebrew/sift.rb) | The on-disk layout — which fixes D-77's blob fan-out depth — and the Keychain service and access-group names |
| [`flatpak/net.justinchung.Sift.yml`](flatpak/net.justinchung.Sift.yml) | The application id, which is simultaneously the D-Bus well-known name, the desktop-file name, the portal identity, and the data root |

## One version, every channel

D-62: **a single user-facing version identifies a source revision and is the same on every
channel.** A channel's own build counter is a packaging detail and never a version.

Support is a **migration floor** — the oldest schema version a current build can still migrate
forward from — advanced only deliberately and **never past a build that is still plausibly
installed**. Under D-33 there is no way to force an upgrade, and its own section
([platforms and distribution](../docs/product/platforms-and-distribution.md#d-33--platform-channels-only-sift-never-updates-itself))
observes that a build a user under Flatpak or a Cask never upgrades stays installed for what "may be years".

The obligation that creates is real work rather than a note: **a migration chain kept
compiling and kept tested against fixtures of every schema version above the floor.**

## What D-33 costs, stated

No self-update, so **no way to push an urgent fix** faster than each channel delivers it — in a
product whose primary adversary chooses the input. That is R-11, and it has no mitigation in this
directory. Until the App Store is reconsidered, macOS reaches only the users who install through a
Cask; reconsidering it brings back a second macOS build configuration and review-and-release
path, and App Review latency with no bypass.

**Every release carries notes stating what changed, and a release that fixes a security defect
must say so.** That is required rather than good practice, because D-33 leaves no in-app
update surface through which anything else could say it.

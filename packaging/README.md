# Channel artefacts

D-33 delivers through **Homebrew Cask on macOS and Flatpak on Linux from the first release.**
The Mac App Store is deferred, not rejected, and is reconsidered only together with D-113's
licensing question. Sift implements no update mechanism, and FR-26 requires it
detect that its own bundle has been replaced and surface a restart prompt rather than
continuing against replaced resources.

**Each of these files must have a named owner in this repository before the first release.**
That is not administrative: each carries values that become permanent on publication. The
owner is recorded in [`CODEOWNERS`](../CODEOWNERS), so a change to either file requests their
review.

| Artefact | Owner | What it makes permanent |
|---|---|---|
| [`homebrew/sift.rb`](homebrew/sift.rb) | @justin13888 | The on-disk layout — which fixes D-77's blob fan-out depth — and the Keychain service and access-group names |
| [`flatpak/net.justinchung.Sift.yml`](flatpak/net.justinchung.Sift.yml) | @justin13888 | The application id, which is simultaneously the D-Bus well-known name, the desktop-file name, the portal identity, and the data root |

### What the Cask publishes

`brew uninstall` and `brew upgrade` both run the Cask's **uninstall** stanza, so it removes the
bundle and nothing else: a stanza that deleted the container there would erase every account,
the blob store and the installation secret on each upgrade. User data goes only under **zap**
(`brew uninstall --zap`), which the user asks for explicitly.

| Published name | Value | Where it comes from |
|---|---|---|
| Data root, sandboxed build (D-45) | `~/Library/Containers/net.justinchung.sift` — the account databases, the shared blob index, the installation policy store, and the blob store under D-77's two-level, one-byte fan-out all live inside it | The bundle identifier; the platform places a sandboxed app's Application Support there |
| Data root, unsandboxed build | `~/Library/Application Support/net.justinchung.sift` | Where the same code resolves Application Support outside the sandbox — every build before the team identifier exists |
| Keychain service | `net.justinchung.sift`, one generic-password item per account and credential kind | `KEYCHAIN_SERVICE` in `sift-foundation` |
| Keychain access group | `<team>.net.justinchung.sift` | `keychain_access_group` in `sift-foundation`; the team identifier is still outstanding |

The zap removes the trees whole rather than naming anything inside them, so the fan-out depth
is permanent because every installed copy's store is laid out by it, not because the stanza
spells it out.

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

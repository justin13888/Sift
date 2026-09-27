# The macOS shell

D-61: **the platform's own application toolchain owns the bundle**, and the Rust core is
built into a static library it links. The Rust workspace never produces the `.app`.

```
mise run macos           # build it
mise run macos -- --run  # build it and launch it
```

That task is four commands in two toolchains, and the shape of it *is* D-61:

```
cargo build --release -p sift-abi        # produces libsift_abi.a
cargo xtask header                       # asserts the committed header matches the ABI
xcodegen generate --spec shells/macos/project.yml --project shells/macos
xcodebuild ... -allowProvisioningUpdates archive   # then -exportArchive for Developer ID
```

The header step is not ceremony. The project links `-lsift_abi` against a committed header, so
without it a drifted ABI links successfully against a stale declaration.

A Release `Sift.app` lands in `shells/macos/.build/export/`, a Debug one in
`shells/macos/.build/xcode/Build/Products/Debug/`, so `mise run clean-swift` still reclaims the
whole build tree.

## Why the project is generated

`shells/macos/project.yml` is the committed spec; `Sift.xcodeproj` is generated from it and is
not committed. D-61 rejects *"a bespoke script assembling the bundle from parts"*, and nothing
here assembles one — XcodeGen emits a project, and Xcode compiles, links, assembles, signs and
stamps the bundle. What D-61 protects is that the Info.plist, the URL types, the entitlements
and the signing posture live in the platform's own format and are enforced by its own
machinery, and they do. What it buys is a spec a reviewer can read instead of four hundred
lines of UUID-keyed plist.

`Info.plist` is committed beside it, because it is the file every one of those obligations
eventually lands in.

## Every build is signed under the team, sandboxed and hardened

D-45. The bundle is signed under team `854G577S2Y` with the entitlements in `Sift.entitlements`
— the sandbox, outgoing network, user-selected file access, the Keychain access group
`854G577S2Y.net.justinchung.sift`, and the address book — and the hardened runtime, in **both**
configurations. What differs is the identity:

| | Debug | Release |
|---|---|---|
| Identity | Apple Development | Developer ID Application, timestamped |
| How | `build` | `archive`, then `-exportArchive` with `ExportOptions.plist` |
| `get-task-allow` | yes, so a debugger attaches | never |

The access group is a restricted entitlement: a process claiming it without a provisioning
profile that authorises it is killed at launch. So the build needs the developer account signed
in to Xcode (Settings → Accounts), and passes `-allowProvisioningUpdates` so Xcode fetches the
profile — and, on a Mac's first Debug build, registers that Mac with the team.

`mise run macos` then reads the bundle back: `codesign --verify --strict`, the entitlements
**exactly** as declared (plus the application and team identifiers the profile stamps, and
`get-task-allow` on Debug), the hardened-runtime flag, the signing identity, and on Release the
timestamp. A bundle that lost an entitlement fails the task rather than a user's Keychain.

**A machine with no identity** — CI first — builds with `--ad-hoc` (implied when `CI=true`):
the same entitlements and runtime, with only the identity overridden. That checks the bundle
builds and carries the right set; the platform will not launch it, so `--ad-hoc` refuses
`--run`.

**Accounts added by an earlier, ad-hoc build must be added again.** Their credentials sit in
the login keychain under the old ad-hoc signature, which the team-signed bundle neither reads
nor migrates; it keeps its own in the data-protection keychain, in the access group above. The
same goes for `~/Library/Application Support/net.justinchung.sift`: a sandboxed build keeps its
data inside `~/Library/Containers/net.justinchung.sift` instead, and starts empty.

D-46's universal artefact is likewise not built here: `cargo` produces one architecture, so the
project builds the one it has.

## What `--run` gives you

Sift launches as an accessory — `LSUIElement`, no dock icon — and the menu-bar envelope is the
always-on surface FR-22 requires. But it **opens a window and joins the dock**, because the
account-less state *is* the add-account flow rather than an empty inbox, and the shell raises
Sift to a regular application while any window is open. So it appears in the dock and the
application switcher, and `--run` prints the bundle path and the pid rather than asserting a
launch it never confirmed.

On a first run that is the add-account screen. On any later run it is the mail: the layer is
asked how many accounts the container holds, so an account added once is found again, its store
opened sealed and its queue rebuilt from its journal. The window-less accessory state is reached
by closing the window, not by starting.

`--run` quits whatever is already running under the bundle identifier before launching, because
`open` resolves a running application by that identifier — so without it you would be looking at
the previous build, or at an installed copy, with nothing to tell you. It asks for a quit rather
than signalling one, so D-70's teardown runs.

Closing that window does not quit — FR-25 — it lowers Sift back to the menu bar. "Quit Sift
entirely" is the deliberate act, worded the same in the tray and in the application menu
because neither may be the silent consequence of the other.

## The OAuth client, and the two things that must agree about it

There is one client per provider kind, each per-installation configuration and none committed.
`mise run macos` reads them from `oauth-client.<kind>.txt` or `$SIFT_OAUTH_CLIENT_ID_<KIND>`
(`oauth-client.txt` and `$SIFT_OAUTH_CLIENT_ID` still configure the Google one), derives the
callback scheme the Google client requires, and writes all of it into `OAuthClient.xcconfig`,
which the generated project reads. The bundle carries the clients as a `SiftOAuthClients`
dictionary keyed by the core's opaque kind strings; the shell copies it across the boundary
without reading it, and asks the core which providers it can offer. **In a file rather
than in the environment, and that is not stylistic.** The spec used to set these to
`${VAR:-default}`; XcodeGen implements no such operator and passed the token through verbatim,
so the values reached a bundle only because `xcodebuild` inherits the environment this task had
exported. Every other way of building produced a bundle with no client and no derived scheme,
silently — an Info.plist key whose expansion is empty is *removed* rather than emptied.

So the task reads the built `Info.plist` back and fails when the client or the scheme is not in
it. It used to print what it intended and never look at what it made, and the difference was
paid by a user who granted consent in a browser and came back to nothing.

Sift checks the same thing at runtime, and this is the part that matters: it reports the client
and the schemes its bundle claims, and the **core** decides whether the one this client requires
is among them. Where it is not, the sign-in is refused before a browser opens, naming both. A
shell that asserted the answer instead is what let the failure through.

A sign-in started in Sift returns through the platform's authentication session rather than
through the launch services database — same system browser, same cookies, and the callback comes
straight back to the process holding the PKCE verifier. `application(_:open:)` stays wired
because the bundle still registers the scheme under D-109.

**Which matters more than it sounds, once both configurations exist.** Debug and Release are the
same bundle identifier claiming the same derived scheme from two paths, so the launch services
database resolves the scheme to whichever registered last — build Debug and a callback routed by
the operating system reaches the Debug copy, whichever one you are looking at. D-45 gives Sift
one identity deliberately and this is a development-only consequence of it, but it is not
fixable from here, and it is one more reason a flow started in this process comes back to it.

## What this costs, from D-61

- The macOS build is **not reproducible from `cargo build` alone**.
- A contributor needs the platform toolchain.
- **CI needs a macOS host for even a smoke build.**
- A build-time concern — a feature flag, a vendored source path, an embedded asset — has to
  be expressed **twice**, and can drift the way D-17 refuses to let the shells drift. The
  mitigation is that the list is short and enumerated in `docs/build/workspace.md`.

## Two channels, one identity

D-45 gives the App Store build and the Homebrew Cask build **one bundle identifier, one team
identifier, and sandboxing for both**. Two identifiers would mean two containers, two
per-installation secrets, and under D-43 a channel switch that destroys the blob cache and
re-authenticates every account. The cost accepted is that the two builds cannot be installed
side by side.

The Cask build is not reviewed by the platform, so it needs its own path: a distribution
certificate under the same team identifier, the hardened runtime, and **notarization with
stapling** — so a first launch does not depend on the user being online.

**The hardened runtime is load-bearing rather than hygiene.** The threat model puts an
in-address-space attacker out of scope *because the platform prevents it*; an entitlement
re-permitting debugger attachment would move a whole class of attack into scope silently.

## Entitlements

The app sandbox itself; outgoing network connections; user-selected read-write file access
for FR-10's save; Keychain access groups for NFR-23; and the address-book entitlement **with
its usage description**, without which FR-40 is silently absent rather than degraded.

## Not built here

`mise run harness` drives the whole application with no window at all (D-65), and the Linux
shell is an ordinary Cargo binary — `mise run linux` (`crates/shells/sift-gtk`).

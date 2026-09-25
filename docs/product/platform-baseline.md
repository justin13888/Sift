# Platform baseline

The floor Sift is built against, and the identifiers it consumes permanently.

**Owns:** D-45, D-46, NFR-55.

[Platforms and distribution](platforms-and-distribution.md) settles *which* platforms and *which*
channels. This page settles what those choices commit Sift to before a line is written — the versions,
the architectures, the entitlements, and above all the strings that can never be taken back.

## Why a register of identifiers exists at all

Almost every decision in this documentation set is contestable and amendable; that is the property the
[decision index](../decisions.md) is built around. **The identifiers below are not.** A bundle identifier
consumed by an App Store record is never released, even after the record is deleted. A Flathub
application id cannot be renamed — a new id is a new application, and existing users are not upgraded to
it. A URI scheme is a first-come namespace with no registry and no arbiter.

[Q-16](../open-questions.md), now [D-113](platforms-and-distribution.md), observed that licensing is "the only irreversible item in this
documentation set". That was true of the *decisions*. It was not true of the set as a whole, because the
set named none of these strings and therefore could not see that it was about to spend them.

## D-45 — One application identity across both macOS channels

**Chosen:** the Mac App Store build and the Homebrew Cask build share one bundle identifier and one Team
identifier, and **both are sandboxed**, with the same container and the same Keychain access group.
**Rejected:** two identifiers and two coexisting applications; sharing an identifier while leaving the
Cask build unsandboxed.

**Why.** [D-33](platforms-and-distribution.md) asserts the two channels "differ only in packaging and
entitlements". That sentence decides this question and was written in passing, so the consequence was
never followed through: the bundle identifier is simultaneously the sandbox container directory, the
Keychain access-group suffix, and the key by which the operating system decides which installed copy
receives an authorization callback.

Follow it through and the alternative is worse than it looks. Two identifiers means two applications, two
containers, and two per-installation secrets — and [D-43](../storage/encryption.md) requires a store whose
secret is gone to be **discarded wholesale**. So under two identifiers the ordinary act of moving from one
channel to the other destroys the user's entire blob cache and re-authenticates every account, and
[D-32](../storage/data-model.md)'s own analysis says a queued mutation is precisely what a resync cannot
restore. A distribution choice would silently become a data-loss path.

One identity avoids that, but only if the container and the credential ACLs actually match, and they match
only if both builds are sandboxed. Keychain item access binds to the creating code's designated
requirement; a sandboxed and an unsandboxed build of the same bundle identifier are not the same
requirement.

**What it costs:** the Cask build accepts sandbox restrictions it does not need, and every entitlement
below has to be right for a channel that could have avoided them. It also means the two builds cannot be
installed side by side, which removes a debugging convenience.

**Contestable because:** it makes the direct-download build pay for the store's constraints, in a project
whose Linux distribution decision ([D-15](platforms-and-distribution.md)) accepted a narrower install base
for a similar reason and called it out as a cost. [D-33](platforms-and-distribution.md) now defers the App
Store channel rather than dropping it, so this decision stands: it is what lets the store be added later
without splitting the identity. If the channel is ever dropped outright rather than deferred, this decision
has no remaining argument and should be reversed rather than kept out of habit.

## D-46 — macOS 13 or later, universal

**Chosen:** a deployment target of macOS 13, and a universal binary covering both Intel and Apple silicon.
**Rejected:** an earlier deployment target; an Apple-silicon-only build.

**Why the version floor is a structural decision rather than a support-matrix one.** The sandbox-legal way
for an application to register itself for background residency changed. The modern mechanism registers the
**main application**. The older one requires a **separate helper executable inside the bundle** whose job
is to launch it — which is a second process, in the design whose second-most-important decision is
[one resident process](../architecture/process-model.md), and whose P0 gate measures toolkit residue in a
single address space. A lower floor would therefore make D-2 false on the App Store channel specifically,
and the P0 spike would be measuring an architecture that is not the one shipped.

Three further requirements set the same floor from other directions: the non-script find mechanism
NFR-20 leaves as the only option (see [webview isolation](../rendering/webview-isolation.md)), the
per-page script control the same requirement is resolved against, and whatever satisfies NFR-50's
accessibility bridge.

**Why universal rather than Apple silicon only.** [D-23](../storage/cache-and-blobs.md) argues against a
SHA-256 hardware mandate precisely because the accelerating extension "arrived late on mainstream Intel
mobile parts — later than the 2020-era laptop the reference environment specifies as the rig". That
argument presumes Intel is in scope. Dropping Intel would not merely narrow the audience; it would
retire the reasoning behind a decision this set already made, and would replace the deliberately
unflattering reference rig with a friendlier one.

**What it costs, and it is not small.** Page size differs between the two architectures, which changes
allocator arena granularity, what `phys_footprint` reports, and what a purge actually returns. **NFR-8,
NFR-9 and NFR-12's slope are therefore different numbers on the two architectures**, so
[Q-12](../open-questions.md)'s re-derivation produces one answer per architecture rather than one answer,
and NFR-26 needs a baseline on each.

**Contestable because:** that doubling lands on the P0 measurements that gate everything else, and Intel
Macs are a shrinking population. A reader who thinks the cost outweighs D-23's argument should say so
before P0 rather than after, because dropping Intel later is cheap and adding it later means re-running
every measurement.

## The reserved identifier register

Each of these is consumed once, for the life of the project. **Each MUST be recorded here before first
submission to any channel**, and MUST NOT thereafter be changed.

| Identifier | Value | Constraint |
|---|---|---|
| Bundle identifier | `net.justinchung.sift` | Reverse-DNS under a domain the copyright holder controls. One value for both macOS channels, per D-45. Permanently bound to the App Store record and never reusable |
| Team identifier | **outstanding** — fixed by the developer account | Prefixes the Keychain access group; fixed by the developer account |
| OAuth redirect URI scheme | `net.justinchung.sift` | Registered in the bundle's URL types. Derived from the OAuth client where a provider requires it, so it is bound to the bundle identifier and to R-1's verified client — see [credentials](../security/credentials.md) |
| Flatpak application id | `net.justinchung.Sift` | Must correspond to a namespace the publisher controls. It is also the D-Bus well-known name, the desktop-file name, the portal identity for autostart and permission grants, and the data root. Renaming creates a new application with no upgrade path |
| Keychain service and access-group names | service `net.justinchung.sift`; access group `<team>.net.justinchung.sift` | Published in the Cask uninstall stanza, so a change breaks uninstall for existing users |
| App Store build number space | begins at 1 | Monotonically increasing for the life of the app record; a number is never reused or decreased |

Two of the six are **not** free choices once the first is made. The Keychain access group is the team
identifier prefixed to the bundle identifier, and the OAuth redirect scheme is derived — from the bundle
identifier for a provider that lets an application name its own redirect, and **from the OAuth client
identifier for one that does not**. So the bundle identifier is the only one of the three that is genuinely
decided, and the other two follow.

**The second case was found by generating a real authorization URL rather than by reading documentation.**
Google's "Desktop app" client type expects a `127.0.0.1` loopback redirect, which
[NFR-24](../architecture/shell-boundary.md) forbids outright — *never a listening socket of any kind, for
any purpose*. Its iOS/macOS client type is the only one compatible with that constraint, and it accepts
exactly one scheme: the client identifier with its components reversed. A build therefore registers **two**
schemes — Sift's own, and the one derived from whichever client it was configured with — and the derived
one is empty on a build with no client, which is a build that runs against the recorded corpus and cannot
sign in to anything. A client identifier is configuration rather than a secret: it appears in every
authorization URL the flow generates, which is why PKCE exists. The team identifier is the one value here that is
issued rather than chosen, and it is outstanding until the developer account exists.

**The bundle identifier arrived by accident and was inspected rather than kept.** It was declared by a
Tauri scaffold that [D-1](../architecture/ui-shell.md) rejects and that opened a local development server
against NFR-24; that scaffold is now abandoned and removed, for the reasons D-1's own document gives. The
string it left behind was `com.justin13888.sift`, and inheriting it would have failed the one constraint
the row above places on this identifier: **the copyright holder does not control `justin13888.com`.** It
is a code-hosting account name, which is not a domain, and reverse-DNS under a namespace somebody else may
register later is exactly the collision the convention exists to prevent.

So the value is `net.justinchung.sift`, under a domain the copyright holder does hold, and **the scaffold
leaves nothing behind at all.** That is a worse outcome than the one previously recorded here and a better
one than shipping the alternative: this identifier is permanently bound to the App Store record and can
never be reused, so the last moment it costs nothing to change is the moment before first submission.
Inheriting a string by default is how the most permanent identifier in the project would get chosen by
accident, and the correction is what that warning was for.

## Two schemes, and only one of them is registered

Sift has a registered URI scheme for the authorization callback ([D-36](../security/credentials.md)) and
an internal scheme for body-view resources ([D-28](../rendering/webview-isolation.md)). Different
documents call each of them "the scheme".

| Scheme | Value | Registered with |
|---|---|---|
| Authorization callback | `net.justinchung.sift` | the operating system, in the bundle's URL types |
| Body-view resources | `sift-resource` | the web engine, and nowhere else |

The internal scheme's value is deliberately **not** derived from the bundle identifier. The two must be
impossible to confuse at a glance in a debug view, a filter rule, or a policy callback, because the whole
of the rule below is that one of them is reachable from outside the process and the other MUST NOT be.

**The internal scheme MUST NOT appear in the bundle's registered URL types, on either platform.** It is
registered with the web engine and nowhere else. Registering it with the operating system would let any
local process hand a [capability token](../rendering/webview-isolation.md) address to Sift, which is the
one thing D-28's unguessability is for — and the [threat model](../security/threat-model.md) names the
registered scheme as one of only two local attack surfaces Sift has.

## Credential storage attributes

[D-43](../storage/encryption.md) puts a per-installation secret in the credential store beside the account
keys, and two attributes of that item decide whether its own reasoning holds.

**Items MUST NOT be synchronizable.** A synchronizing credential item would silently redefine
"installation" as *per account holder* rather than per installation, placing the same convergent key on
every machine the user owns — which contradicts encryption.md's flat statement that "cross-machine
convergence was never a property this design had or wanted".

**Items MUST NOT be device-only either**, and this is the less obvious half. A device-only secret does not
survive a migration to new hardware, and D-43 then requires the blob store that secret keyed to be
discarded wholesale. Device-only would therefore convert every hardware upgrade into a total cache loss,
by the specification's own rule, for a threat the specification does not claim to address.

The item is available after first unlock and no earlier, which is what a resident application registered
as a login item needs, and it is why [failure model](../runtime/failure-model.md) has to state what
happens between login and first unlock.

## "Installation" means one user account on one machine

The term keys [D-43](../storage/encryption.md)'s content addresses and appears in a dozen places
undefined. It means **one user account on one machine**: two people sharing a Mac have two installations,
two secrets, two blob stores and no deduplication between them, which is the correct answer for a store
whose contents are one person's mail.

This is a definition rather than a decision because changing it later is not a migration. The secret keys
the content address itself, so re-scoping it re-derives every address in the store — a total cache
invalidation, and one that NFR-48 would not permit to happen silently.

## On-disk layout

Account databases, the shared blob index, the installation policy store and the blob store live together
under the application's own container, and **MUST NOT live in any location the operating system may purge
on its own**.

The reason is specific rather than tidy. A purge of a cache location would remove blobs while leaving the
encrypted shared blob index still referencing them. [Data model](../storage/data-model.md) provides a
refcount rebuild for exactly one situation — after abnormal termination — and a background purge is not
that; the store would be quietly wrong with nothing scheduled to notice. The blob store is evictable by
Sift, under NFR-14, and that is not the same property as being evictable by anyone else.

The layout is published in the Cask uninstall stanza and depended on by every installed copy, so it joins
the register above in practice even though it is not a single string.

## Entitlements, and the requirement that differs between channels

The sandbox entitlement set is: the app sandbox itself, outgoing network connections, user-selected
read-write file access for FR-10's save, Keychain access groups for NFR-23, and the address-book
entitlement with its usage description — without which [FR-40](../architecture/presentation-layer.md) is
silently absent rather than degraded.

**NFR-49 MUST be verified on both macOS channels rather than once.** The mechanism by which a saved
attachment acquires the platform's untrusted-source marking is not the same for a sandboxed build and an
unsandboxed one. It is stated once, in [cache and blobs](../storage/cache-and-blobs.md), for a product
that has two macOS build shapes — so an untested channel can ship without it and the requirement's own
wording notices nothing. D-45 narrows this by sandboxing both builds; it does not remove the obligation to
test both.

## NFR-55 — Local logs are correspondence metadata, and are bounded

**NFR-55.** Local diagnostic logs MUST NOT contain message content, addresses, subjects, or domains; MUST
NOT contain credential material; MUST be bounded by a declared byte budget with oldest-first eviction; and
MUST be retained no longer than a stated period.

Logging is absent from this documentation set as a subject. NFR-22 in [privacy](../security/privacy.md)
constrains *telemetry*, which is traffic that leaves the machine; NFR-23 in
[credentials](../security/credentials.md) contains the only clause that assumes logs exist at all, and it
assumes it in passing. A log file is neither.

Three properties make it worth its own requirement rather than an inference from the other two.

**It is the same exposure NFR-23 already reasons about, one file over.** That requirement keeps
credentials out of the database because "databases get backed up, synced between machines, copied for
debugging, and attached to bug reports". All four are true of logs, and a log is *more* likely to be
attached to a bug report than a database is, because attaching it is what a maintainer asks for.

**The platform default is worse than a file.** On macOS the ordinary logging facility writes to a
system-wide store readable by any administrator, and its redaction is opt-out per interpolation rather
than opt-in — so a sender address logged with the wrong specifier is published outside Sift's control, and
cannot be unpublished on a machine that already ran that build.

**A log is the one unbounded thing the no-unbounded rule does not reach.**
[Memory pressure](../runtime/memory-pressure.md) requires every cache to declare a budget and NFR-14
hard-caps the disk, and neither reaches a log file — in a process designed to run for fourteen days at a
time.

Nothing here restricts what the [debug views](../runtime/observability.md) may show. Those are local
surfaces that transmit nothing and are read by the user in front of the machine; a log is a file that
outlives the session and travels.

## Related

- [Platforms and distribution](platforms-and-distribution.md) — which platforms and which channels
- [Reference environment](reference-environment.md) — the rig those channels are measured on
- [Encryption](../storage/encryption.md) — what the per-installation secret protects
- [Failure model](../runtime/failure-model.md) — what happens when the credential store is unavailable

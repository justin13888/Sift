# Process model

**Owns:** D-2, FR-25, FR-26.

## D-2 — One resident process, with a disposable web engine content process

**Chosen:** a single resident application process holding both the core and the native shell, with the
web engine's content process created on demand and destroyed when no message is being read.
**Rejected:** a long-lived core daemon plus a separately launched, disposable UI process.

**Why.** This reverses an earlier position, and the reasoning that produced the original answer is what
tells a future reader when to revisit. The two-process design rested on the claim that a single process
could hide its window but "cannot return the web engine's address space". **That claim is false on both
target engines.** WebKit is multi-process: a body view's content runs in its own process, and destroying
the view returns that memory regardless of how many processes Sift itself has. This document
contradicted [webview isolation](../rendering/webview-isolation.md) and
[memory pressure](../runtime/memory-pressure.md), both of which already said so.

What a second process would actually have bought is the ability to return **toolkit residue** — whatever
AppKit or GTK4 leaves resident after its window is destroyed, once caches are dropped and the allocator
is purged. That is a real quantity, but it is far smaller than a web engine, and it falls near the
revisit trigger the earlier decision wrote for itself: a delta "on the order of 30 MB rather than 150 MB".

Removing the second process also removes a local socket and its wire contract, schema versioning across
that socket, peer-credential verification, daemon activation and supervision, and **version skew as a
category**. That is a subsystem's worth of machinery deleted, not a simplification at the margin.

**What it costs:** the deepest shed tier can no longer terminate anything, so the idle floor is set by
toolkit residue rather than by a process that never linked a toolkit — see
[memory pressure](../runtime/memory-pressure.md). A crash in shell code now takes sync down with it,
bounded by NFR-16 in [mutations](../mail/mutations.md).

**Contestable because:** the toolkit-residue figure is measured on one of the four platform and
architecture pairs P0 owes, and only provisionally. The Linux figure is still the weakest number in this
documentation set — GTK4's renderer loads a graphics driver stack with no clean in-process release path.
If P0 measurement shows Linux residue far above the macOS figure, the two-process design returns as the
answer for that platform, and a per-platform process model would be worse than either uniform answer. See
[roadmap](../product/roadmap.md).

The first figure is macOS on Apple silicon, from the provisional interim rig in the
[reference environment](../product/reference-environment.md), and so it passes no gate. At L3, when the body view
was released, the whole process held about 32 MB and the one engine process that outlived every window
held about 13 MB. That L3 does not reliably release the view is a defect in the shell, not residue. See
[memory pressure](../runtime/memory-pressure.md#provisional-figures). The whole process is an upper
bound on toolkit residue, because it contains the core's own floor as well. Even read as that bound,
**what a second process could have returned is about 45 MB. That is on the order of 30 MB and not
150 MB**, so this decision's revisit trigger is not met on that platform. Intel and Linux are
unmeasured, and Linux is still the platform that decides whether this decision stands.

## Lifecycle

Sift MUST be registered for background residency using the platform's own per-user mechanism — a login
item on macOS, the desktop portal's autostart request on Linux. It MUST NOT be a system-wide service,
and MUST NOT require elevated privileges.

The Linux mechanism is the portal rather than a systemd user unit, because
[D-15](../product/platforms-and-distribution.md) distributes through Flatpak, and a sandboxed
application cannot install a unit on the host.

Residency is independent of whether a window exists. The process MUST continue to sync, apply queued
mutations, and post notifications with no window open, and MUST NOT hold a window's resources while none
is visible.

## FR-25 — Quitting must be unambiguous

"Close the window, keep syncing" and "quit entirely" MUST be distinct, clearly labelled user actions.
Neither MUST be the silent consequence of the other. A user who believes they have quit the application
and finds it still resident has been misled; this is the single most likely source of user distrust in
the whole design.

A single process makes this easier to express than the two-process design did, because closing a window
on a still-running application is ordinary platform behaviour rather than something to explain. Closing
the window sheds the shell; quitting terminates the process and stops sync. The always-on surface that
expresses both is the menu-bar or tray presence — see [UI shell](ui-shell.md).

## FR-26 — Updates

Updates MUST be delivered by the platform's own channel, signed by it. Sift MUST NOT implement a
self-update mechanism.

Because Sift ships as one binary, an update cannot leave two components at different versions — the skew
that the earlier two-process design had to detect and prevent does not arise. What remains is that the
application is resident while its bundle is replaced underneath it. Sift MUST detect that its own bundle
has changed on disk and MUST surface a restart prompt rather than continuing indefinitely against
replaced resources.

Channels are per platform and are described in
[platforms and distribution](../product/platforms-and-distribution.md).

# Credentials

**Owns:** D-36, FR-2, NFR-23.

## FR-2 — Authentication

OAuth 2.0 with PKCE, conducted through the **system browser**, returning through a registered URI scheme
handler (D-36). Refresh tokens live in the OS credential store only. Refresh MUST be silent; failure MUST
produce an explicit re-auth prompt rather than a silent sync stall.

The system browser rather than an embedded view is not a convenience choice: an embedded authentication
view sees the user's password, defeats the provider's own phishing protections, and is increasingly
refused by providers outright.

Generic IMAP accounts using password authentication follow the same storage rule. See
[accounts](../mail/accounts.md).

Because Sift stays resident with no window (see [process model](../architecture/process-model.md)), a
failed refresh may occur when there is nothing on screen to prompt. The re-auth prompt MUST therefore be
raised through the always-on surface in [UI shell](../architecture/ui-shell.md), and the affected account
MUST show as needing attention rather than merely stalling.

## D-36 — Authorization returns through a URI scheme, not a socket

**Chosen:** register an application URI scheme and receive the authorization code through the operating
system's handler.
**Rejected:** a loopback HTTP redirect on an ephemeral port; the device authorization flow.

**Why.** The loopback redirect is the pattern providers recommend for desktop applications, and it
**contradicts NFR-24** — it opens a listening socket, which that requirement prohibits for any purpose.
That contradiction sat unremarked in this documentation set while NFR-24 was stated absolutely.

It could have been resolved with a carve-out: a listener bound to loopback, on an ephemeral port, open
only during an interactive flow. A scheme handler resolves it better, by removing the socket rather than
excusing it. NFR-24 then stays absolute and unqualified, which is a far cheaper property to test and to
audit than a bounded exception — and combined with [D-2](../architecture/process-model.md) removing the
IPC socket, it makes "Sift never listens" literally true.

The device authorization flow also avoids a socket, and was rejected on experience: entering a code in
another window is a markedly worse first run, and support for it across the mail scopes Sift needs is
inconsistent.

**What it costs:** a registered scheme per platform, and its Flatpak counterpart. A scheme handler is also
a surface any local application can invoke, so the authorization state parameter is doing real work
against a forged callback rather than being ceremony.

**Contestable because:** providers document and test the loopback path most thoroughly, and a scheme
registration that fails to install is a first-run failure with no obvious diagnosis. If a provider refuses
scheme redirects for the scopes Sift needs, NFR-24 gets its carve-out after all.

## NFR-23 — Storage boundary

**Credentials MUST exist only in the OS credential store — never in a database, never in logs, never in
crash dumps.**

The reason is mundane and decisive: databases get backed up, synced between machines, copied for
debugging, and attached to bug reports. A credential in the database travels with all of that. See
[encryption](../storage/encryption.md).

Credential access MUST be brokered in one place, so that "which code can read a token" has a single,
reviewable answer. That broker sits in the core; no shell code reaches it.

Crash reports are scrubbed and opt-in — see [privacy](privacy.md).

## The Gmail verification blocker

**This is a business-level blocker, not a technical one, and it is the top risk in the entire project.**

Gmail access — both API and IMAP-over-OAuth — requires **restricted** scopes. Restricted scopes require
the provider's verification process, including a recurring third-party security assessment that is
expensive and repeats. Without it, users see an unverified-application warning and the client is capped at
a small number of users. Every serious third-party Gmail client has had to solve this.

**It MUST be resolved before a line of the Gmail adapter is written** — see
[roadmap](../product/roadmap.md), where it is a P0 gate.

The escape hatches each reshape the product rather than merely delaying it, and each one collides with a
decision made elsewhere. Reading them against that decision is what turns this from a list of options into
a single question.

**Requiring each user to supply their own OAuth client identifier is incompatible with the App Store
channel.** [D-33](../product/platforms-and-distribution.md) ships two macOS channels on the argument that
they reach different people, and it names the App Store as where "everyone else" looks — the non-technical
half, explicitly. Asking that audience to create a cloud project and paste a client identifier is not a
first run they complete. So this hatch is available to the Homebrew Cask and Flatpak builds and absent
from the one channel it would matter most for.

**Restricting Gmail support to organizational tenants** gives up the consumer Gmail user, who is the
largest single population this product could serve.

What is left once both are read against D-33 is not an engineering choice between three options. It is:
**fund a recurring third-party security assessment, or ship a client whose largest provider is unreachable
through its largest channel.** That is a budget question rather than a design one, which is why it is
settled before the adapter is written rather than discovered during it. Tracked in
[open questions](../open-questions.md).

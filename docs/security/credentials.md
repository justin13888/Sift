# Credentials

**Owns:** D-36, D-88, FR-2, NFR-23.

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
MUST show as needing attention rather than merely stalling. *Needs authentication* is the
highest-precedence condition in [failure model](../runtime/failure-model.md), which owns how it composes
with everything else that can be wrong with an account.

## D-88 — The flow, in the detail that decides whether an account survives it

**Chosen:** Sift is a **public** client with no embedded secret; the scope set per provider is fixed
before the client is verified and is permanent; a refresh is **single-flight** per account; a rotated
token pair is written to the credential store **before** it is used and the previous pair is retained
until the new one has succeeded once; and a refresh failure is non-transient **only** when the provider
returns a well-formed error explicitly denying the grant.
**Rejected:** an embedded client secret; per-request refresh; treating any refresh failure as
authoritative; discarding the previous token pair on receipt of a new one.

FR-2 gives the shape — OAuth 2.0, PKCE, system browser, silent refresh — and each of the five points
below is a place where the obvious implementation loses the user's account rather than degrading.

**Public client, no embedded secret.** A secret shipped in a binary distributed through
[three channels](../product/platforms-and-distribution.md) is not a secret, and treating it as one would
make every security property that rested on it false. PKCE is what replaces it, which is why FR-2 names
it rather than treating it as an option.

**The scope set is permanent, and it is smaller than it looks.** A scope string is baked into the
verified client and into every existing user's grant, so widening it later forces the entire install base
through re-consent. Each provider document records its own set. Two rules hold across all of them: the
set is the minimum for read, search and the FR-13 intent set, and **no send or compose scope is ever
requested**, which is [the no-send constraint](../product/scope.md) expressed where a reviewer can check
it against an authorization screen rather than against a code path.

**Refresh is single-flight per account.** Push, delta, body fetch and queue flush run concurrently under
[D-19](../architecture/overview.md), so an expired token produces several simultaneous refusals and, with
no coordination, several simultaneous refreshes. Against a provider that rotates refresh tokens, the
second refresh presents a token the first has already spent, and the provider's correct response is to
invalidate the grant — **so the uncoordinated implementation logs the user out by trying too hard.** One
refresh at a time per account, with the others awaiting its result.

**The write precedes the use, and the old pair is kept until the new one works.** The dangerous window is
between receiving a rotated pair and durably storing it: a crash there leaves the provider having retired
the old refresh token and Sift having lost the new one, which is a total lockout requiring interactive
re-authentication. So the new pair is written first and used second, and the previous pair is retained —
marked superseded — until the new one has completed one request. Providers commonly tolerate the old
token briefly, which is exactly the window this exploits, and retaining it costs one extra credential
item for seconds.

**A refresh failure is non-transient only when the provider says so.**
[Failure model](../runtime/failure-model.md) enters *needs authentication* on a refresh *"that failed
non-transitively"* and never defined the term. The classification is narrow deliberately: only a
well-formed provider error explicitly denying the grant counts. A transport failure, a 5xx, a timeout, a
throttling response under [D-87](../mail/provider-model.md), and **anything that is not a well-formed
provider error response** are all transient.

That last clause is doing specific work. [NFR-34](../runtime/network-conditions.md) warns that a captive
portal returns *"plausible-looking HTTP responses"* and must not *"cascade into re-prompting for
credentials on every account at once"* — which is precisely what a lenient classifier produces the moment
a user opens a laptop on hotel wifi. **A response that does not parse as the provider's own error
document is evidence about the network, not about the grant.**

**Removal revokes, best-effort, and never blocks.** [FR-4](../mail/accounts.md) makes removal *"provably
erase"* local state, and most users will assume that includes Sift's access to their mailbox — an
assumption that is wrong unless the grant is revoked at the provider. So removal attempts revocation. It
**MUST NOT** block on it, because a user removing an account while offline, or after the credential has
already expired, must still be able to remove it; the local erasure FR-4 specifies is what is provable,
and revocation is best-effort on top.

**The in-flight flow's secrets are held in memory and nowhere else.** The PKCE verifier and the state
parameter exist before the account does, so they cannot live in an account store, and they are worthless
afterwards, so they MUST NOT live in the credential store either. They are held for the duration of one
authorization, bounded by a timeout, and discarded. **Concurrent authorizations correlate by their state
parameter** — which is the work D-36 below already says that parameter is doing against a forged callback,
serving a second purpose here — and a callback whose state matches no flow in progress is discarded
without comment.

**What it costs:** a refresh path with a coordination primitive, a transient credential item, and a
classifier whose default answer is "try again", which means a genuinely revoked grant takes longer to
surface than it would under a lenient rule.

**Contestable because:** the conservative classifier delays the *needs authentication* prompt for a user
whose access really was revoked, and FR-2's whole point is that a silent sync stall is unacceptable. The
answer is the asymmetry: a late prompt is an annoyance, and a prompt raised on every account because a
hotel router answered a refresh with a login page is an application that appears to have lost the user's
credentials.

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

**A registration that did not install is checked before a flow starts, not discovered after one.**
[D-71](../architecture/lifecycle.md) makes it a process-scoped refusal: Sift MUST NOT begin an
authorization whose callback has nowhere to arrive, because the user then experiences a working
credential entry followed by silence, and concludes their credentials were rejected. The check is cheap
and the alternative is the diagnosis problem below, met by every affected user.

**Contestable because:** providers document and test the loopback path most thoroughly, and a scheme
registration that fails to install is a first-run failure with no obvious diagnosis. If a provider
refuses scheme redirects for the scopes Sift needs, NFR-24 gets its carve-out after all.

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

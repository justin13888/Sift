# Body view isolation

The containment boundary around message rendering.

**Owns:** D-3, D-28, D-50, N-1, NFR-20, NFR-21, NFR-25, NFR-46, NFR-50.

## D-3 — Bodies render in a separate, hardened document

**Chosen:** a dedicated body view — its own document, its own data store, its own process where the engine
provides one.
**Rejected:** rendering message HTML into the application's own DOM with scoped CSS.

**Why.** Scoped CSS is defeatable; a separate document is the only real boundary. This is also *why* the
native shell decision costs nothing here: the body view exists in either architecture, so the shell being
native does not add a web engine, it removes one. See [D-1](../architecture/ui-shell.md).

**What it costs:** an extra process and the plumbing to size a document rendered outside the shell's
layout.

## Invariant N-1 — The body view has no network capability

**Only an internal scheme is registered. Every other scheme — http, https, websockets, data, blob, file —
is rejected at the engine's policy layer. All resource loading routes through the resource broker.**

This is the structural claim the whole content-blocking design rests on. A browser extension must
intercept requests it does not control; Sift **owns every byte**, so blocking is a decision function in
its own fetch path rather than an interception of someone else's. See
[resource broker](../architecture/resource-broker.md) for the component, and
[content blocking](content-blocking.md) for the decisions.

It is enforceable at the engine level on both target platforms: register a custom scheme handler, reject
every other scheme in the navigation policy callback, and use a dedicated non-persistent data store.

## D-28 — The internal scheme addresses per-view capabilities

**Chosen:** each body view mints an unguessable token at creation; resources are addressed under that
token, and the whole token is invalidated when the view is torn down.
**Rejected:** addressing resources by content hash; addressing them by message and part identifier.

**Why.** The internal scheme is the **only** channel into the body view, which makes its addressing model
a security boundary rather than a naming convenience. Two properties are required of it, and the obvious
schemes fail one each.

Addressing by content hash is trivially cacheable and deduplicates for free, but a content hash is a
**stable global identifier**: the same image in two messages yields the same URL. That is a correlation
channel between messages, which is what NFR-25 exists to close, and it makes a guessed address a valid
one.

Addressing by message and part identifier is readable and debuggable, but those identifiers are guessable
and stable across views, so one view could request another message's parts and revocation on teardown
would mean nothing.

A per-view token gives both properties at once. It is unguessable, so a fabricated address resolves to
nothing; it is scoped to one view, so two messages share no address space; and revocation is wholesale
rather than per-resource, which matters because NFR-46 tears views down routinely and a stale token MUST
NOT outlive the view that minted it.

**What it costs:** nothing caches across views, since addresses do not repeat. Deduplication still happens
underneath, in the [content-addressed store](../storage/cache-and-blobs.md), but the engine's own cache
cannot help — which is acceptable given the data store is non-persistent anyway.

**Contestable because:** it puts an unguessable token in every URL in the FR-33 debug view, making the
most useful diagnostic surface in the product harder to read. The debug view should resolve tokens back to
message and part for display rather than the scheme being weakened to suit it.

## Requirements

**NFR-20.** Zero JavaScript execution in message bodies, **enforced at the engine level, not by
sanitization alone**. JavaScript is disabled in the body view's configuration, in the sense D-50 below
settles.

Essentially no legitimate email needs script. Disabling it removes the JIT, the timers, and the
fingerprinting surface, and cuts memory materially. It also means a sanitizer failure to strip script is a
defence-in-depth degradation rather than an incident — see
[sanitizer invariants](sanitizer-invariants.md).

## D-50 — "Disabled" means the engine, not the page

**Chosen:** script is disabled engine-wide for the body view. Neither page content nor Sift itself may
execute script in it.
**Rejected:** disabling script from page content while retaining host-initiated evaluation.

**Why this is a decision and not a restatement.** Both target engines expose *two* settings, and NFR-20's
words are satisfied by either. One disables script that arrives with the document while leaving
host-injected and host-evaluated script running; the other disables the engine's script support outright.
The difference is invisible in the requirement's phrasing and decisive in what it promises.

The narrower setting is genuinely tempting, because it makes three awkward things easy at once: the
content height this pipeline reports back, find-in-message under FR-24, and richer per-message
diagnostics for FR-33. It also leaves a live script engine in the body view's process, executing on a
document assembled from attacker-controlled input.

That is what decides it. [Sanitizer invariants](sanitizer-invariants.md) lists "JavaScript disabled at the
engine level" as I1's **independent backstop**, and states that an I1 regression is therefore "a
defence-in-depth degradation, not an incident". That claim is only true under the wider setting. Under the
narrower one the backstop is a policy about where script came from, enforced by the same engine whose
parser disagreeing with Sift's is the entire mutation-XSS class I8 exists to catch. Choosing the narrower
setting would not merely weaken the guarantee; it would falsify a sentence this set already relies on in
the one place its central claim is made.

**What it costs, and the cost is real work rather than a shrug.** Two mechanisms that would have been
one-liners now need a platform answer and P0 proof:

- **Content height.** [The pipeline](pipeline.md) ends by reporting content height back to size the
  container, and the obvious route on one target platform does not exist. The mechanism MUST be a
  non-script platform interface, and it MUST be demonstrated in P0 rather than assumed — see
  [roadmap](../product/roadmap.md). If none exists, the retreat is a body view pinned to a fixed layout
  width, which is one of the answers [Q-15](../open-questions.md) is already weighing for a different
  reason.
- **Find-in-message.** FR-24 makes every action keyboard-reachable, so finding text in a message is a
  requirement rather than a convenience. Both engines offer a script-free find facility; it carries a
  minimum-version floor, which is one of the constraints setting [D-46](../product/platform-baseline.md).

**Contestable because:** it spends real implementation effort to protect a backstop against a failure that
the sanitizer is separately tested not to have, and a reader who trusts I1's own verification may think
the narrower setting is free. The answer is that backstops are for the case where that trust is misplaced,
and a backstop that shares a failure mode with the thing it backs up is not one.

**NFR-21.** No unrequested network egress from message content. Remote resources are blocked by default.

**NFR-25.** The body view runs with the tightest sandbox each platform offers, in its own data store — no
cookies, cache, or local storage shared with anything else. Two messages MUST NOT be able to correlate
through shared storage, and neither MUST be able to observe application state.

A content security policy of `default-src 'none'`, permitting only the internal scheme for images and
fonts and inline styles, is applied as an additional layer. It is a backstop; N-1 is the guarantee.

**Sift's own bundled fonts are not addressed through the internal scheme**, and stating that avoids a
collision between two decisions that were made separately.
[Platforms and distribution](../product/platforms-and-distribution.md) requires a bundled font set with a
pinned fallback chain, because font divergence causes more visible cross-platform difference than layout
does. D-28 mints a fresh capability token per view and accepts that "nothing caches across views". Serving
the bundled set through the internal scheme would therefore re-transfer and re-parse a CJK-capable
fallback chain on every message open, against NFR-3's 80 ms budget, for bytes that are identical every
time and are Sift's own.

They are instead registered with the platform's font machinery for the life of the process and named by
family in the base stylesheet, so the body view resolves them with no resource load at all. This is not a
hole in N-1: N-1 governs what the *message* can cause to be loaded, and a family name in Sift's own
stylesheet is not attacker-influenced. A font the **sender** declares remains a fetching position under
I2, is rewritten like every other, and is decided by the broker like every other.

**NFR-46.** The body view MUST be torn down after a configured period with no reader visible, and its
footprint MUST return to approximately zero within 1 second of teardown.

Teardown is real on both target engines: destroying the view releases the engine's out-of-process content
process. This is the mechanism by which the L2 shed tier in [memory pressure](../runtime/memory-pressure.md)
reclaims the largest single allocation in the running app.

**NFR-50.** Isolation MUST NOT sever the accessibility tree. Message body content is announced by the
screen reader as part of the reader, and this is verified on both platforms.

NFR-27 in [UI shell](../architecture/ui-shell.md) requires the body to be navigable and announced, and
that requirement lands here rather than there, because everything on this page works against it. The body
is a **separate document** (D-3) in a **separate process**, addressed through **opaque per-view tokens**
(D-28), in a **non-persistent isolated store** (NFR-25), with **script disabled** (NFR-20) — five
properties chosen to stop the body reaching anything, and the accessibility tree is the one thing that
must cross anyway.

Two consequences are normative. Whatever bridges the tree into the host's hierarchy is an interface into
the body view, so it is subject to N-1's rule that the body view originates nothing: it exposes structure
and text outward and MUST NOT become a path by which the body reaches application state. And because the
sanitizer strips document-level and structural elements under I3 and I4, the accessible name of what
remains derives from content the sanitizer preserved — so alternative text, table structure, heading
level and reading order MUST survive sanitization, and their loss is an NFR-50 defect rather than a
cosmetic one.

Verification belongs in the fidelity corpus gate rather than in manual spot checks — see
[reference environment](../product/reference-environment.md).

## Navigation

All navigation attempts MUST be intercepted and MUST NOT proceed in place. See
[link handling](link-handling.md).

# Body view isolation

The containment boundary around message rendering.

**Owns:** D-3, D-28, N-1, NFR-20, NFR-21, NFR-25, NFR-46, NFR-50.

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
sanitization alone**. JavaScript is disabled in the body view's configuration.

Essentially no legitimate email needs script. Disabling it removes the JIT, the timers, and the
fingerprinting surface, and cuts memory materially. It also means a sanitizer failure to strip script is a
defence-in-depth degradation rather than an incident — see
[sanitizer invariants](sanitizer-invariants.md).

**NFR-21.** No unrequested network egress from message content. Remote resources are blocked by default.

**NFR-25.** The body view runs with the tightest sandbox each platform offers, in its own data store — no
cookies, cache, or local storage shared with anything else. Two messages MUST NOT be able to correlate
through shared storage, and neither MUST be able to observe application state.

A content security policy of `default-src 'none'`, permitting only the internal scheme for images and
fonts and inline styles, is applied as an additional layer. It is a backstop; N-1 is the guarantee.

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

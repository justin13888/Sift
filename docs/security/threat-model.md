# Threat model

Who the attacker is, what they control, and where each boundary sits.

## The adversary

**The primary adversary is the sender of a message.** They control every byte of the content, they are
unauthenticated by default, and they can send an unlimited number of attempts at no cost. This is an
unusually favourable position for an attacker and an unusually clear one to design against.

Secondary adversaries: another process running as the same user on the machine; anyone with filesystem
access to a stolen or shared device. Sift presents no listening socket to the first of these — see NFR-24
in [shell boundary](../architecture/shell-boundary.md) — so its local attack surface is the registered
URI scheme and the files it writes.

**"No listening socket" is not the same as "nothing can reach it", and the difference is worth naming
rather than inferring from NFR-24's absoluteness.** The registered scheme is delivered by the platform's
own launch machinery, and on Linux the single-instance mechanism is a well-known name on the session bus;
both are addressable by any process running as the user. What NFR-24 buys is that the endpoint is the
platform's, with the platform's own access rules, rather than one Sift wrote — and that the input arriving
on it is a URL rather than an arbitrary protocol. That input is untrusted like any other: the
authorization state parameter in [D-36](../security/credentials.md) is doing real work against a forged
callback, and it is the reason that requirement calls it more than ceremony.

**Out of scope:** a malicious provider, and physical attacks against a running machine.

### The same-user process is in scope for the disk and out of scope for the address space

Read carelessly, the two paragraphs above contradict each other: a process running as the same user is
named a secondary adversary, and an attacker with code execution as that user was previously listed as
out of scope. Both statements are wanted, and the line between them is the one
[D-42](../storage/encryption.md) already argues from, so it is stated here rather than left to be
rediscovered by whoever writes the key handling.

**In scope: what a peer process can reach without entering Sift.** The files Sift writes, the registered
URI scheme, the session bus name on Linux, and anything the OS hands out on request. This is the half
D-42 exists for — its own text says whole-disk encryption "is transparent to every process once the user
logs in, so it does nothing about the second half — and the second half is why per-account keys exist at
all". Against this adversary, per-account keys held in the OS credential store with the platform's own
access rules are a real boundary: reading the file yields ciphertext, and reaching the key means
satisfying the credential store rather than satisfying the filesystem.

**Out of scope: what an attacker reaches by entering Sift.** Attaching a debugger, injecting code, or
otherwise reading the running process's memory. Once that is possible, decrypted pages, credential
material read out of the store to be used, and the keys themselves are all readable, and no design in
this set claims otherwise — [privacy](privacy.md) makes the same concession about live credential
material in memory.

**This is a boundary the platform provides and Sift MUST NOT weaken.** It rests on the hardened runtime
on macOS and on the kernel's own restriction of debugger attachment on Linux, so an entitlement, a build
setting, or a packaging choice that re-permits attachment moves a whole class of attack from out of
scope to in scope silently, without any document here changing. That is the reason this reads as a
requirement rather than as a definition.

## Attacker-controlled inputs

| Input | Reaches | Primary defence |
|---|---|---|
| MIME structure and headers | the parser | streaming parse, bounded output, degradation to raw view — NFR-19 |
| HTML body | the sanitizer, then the body view | [sanitizer invariants](../rendering/sanitizer-invariants.md) I1–I10 |
| CSS | the sanitizer, the blocker, the dark transform | parsed as a tree, fetching positions enumerated explicitly |
| Remote resource URLs | the resource broker | blocked by default; [content blocking](../rendering/content-blocking.md) |
| Images | the decoder, **in the resident process** | bounded decode by memory-safe decoders only; a format without one is served unclassified; classification cached by content hash — [D-29](../rendering/content-blocking.md#where-the-bytes-are-decoded) |
| Link targets | the confirmation UI | punycode decoding and bidi stripping — [link handling](../rendering/link-handling.md) |
| Header text — display names, subject | the **native** list, reader chrome, notifications and the tray | normalized once at the boundary — NFR-54 in [presentation layer](../architecture/presentation-layer.md). No sanitizer invariant sees this path |
| The `Date` header | list ordering, if it were trusted | it is not: [D-55](../architecture/presentation-layer.md) orders on the server's received time |
| MIME filename parameter | the **filesystem**, as the name of a saved attachment | never used as a path; derived, normalized and shown in full before the write — NFR-53 in [cache and blobs](../storage/cache-and-blobs.md) |
| Filter lists | the filter engine, and generated stylesheets | stale-tolerant; never blocking; bundled in the binary and never fetched — [D-111](../rendering/content-blocking.md). **Integrity rests on the build**, narrowed rather than closed — see below |

## Trust boundaries

| Boundary | Crossing | Enforcement |
|---|---|---|
| Sender → Sift | message bytes | parse never trusts structure; bounded and streaming |
| Sift → body view | sanitized, rewritten HTML | body view has no network and no script — invariant N-1, NFR-20 |
| Body view → Sift | resource requests, navigation attempts | every one is a question answered by the broker, not a request in flight |
| Sift → OS credential store | key and token access | credentials never enter the database, logs, or dumps — NFR-23 |

## Layered defence

The design is explicitly layered, and each layer names what backstops it. A single failure should be a
degradation, not an incident:

- A sanitizer script-stripping failure is backstopped by JavaScript being disabled at the engine level.
- A sanitizer URL-rewriting failure is backstopped by the body view having no network capability.
- A blocker rule failure is backstopped by compiled engine-level content rules from the same source.
- A containment failure is backstopped by the body rendering in its own document with its own data store.
- An image decoder defect is backstopped by the decoder being memory-safe and its panics being caught.

**One attacker-controlled input in the table above is handed to the trusted half of the system rather than
the disposable half.** Image bytes are decoded in the core for classification under
[D-29](../rendering/content-blocking.md), where no process boundary stands behind the decoder. What stands
there instead is a stated constraint: every component reading image bytes in the core MUST be
memory-safe, a format without such a decoder is served to the engine and never classified, and the
broker's work on each image request is a [D-47](../architecture/overview.md) catch boundary. A decoder
defect is therefore a caught and counted panic that answers that one image *unavailable*, rather than a
memory-safety bug in the resident process. This is a weaker backstop than
the sandbox the engine's own decoders run in — it excludes corruption, not misclassification or exhaustion
the bounds miss — and [D-29](../rendering/content-blocking.md#where-the-bytes-are-decoded) records why it
was chosen over a separate process. It answered [Q-14](../open-questions.md).

The invariants **without** a backstop — idempotence, boundedness, parse stability, no content invention,
encoding determinism — are where review attention belongs. See
[sanitizer invariants](../rendering/sanitizer-invariants.md).

## A narrowed gap: filter-list content

Filter lists are listed above as attacker-controlled input, and the defence recorded for them —
stale-tolerant, never blocking — addresses *availability* only. It does not address content. Element
hiding and style injection are realised as a **generated stylesheet injected into every message body**, so
a hostile or compromised list can inject CSS everywhere, and no list-content sanitization requirement
exists anywhere in this documentation set.

This was recorded as a real gap rather than a deferred choice, and [Q-11](../open-questions.md) asked the
prior question: whether Sift should operate a list channel at all.
[D-33](../product/platforms-and-distribution.md) removed self-update on the reasoning that an application
resident on a user's machine, reading their mail, "is the wrong place to put a bespoke code-delivery
path", and a list-update channel is a bespoke delivery path into every message body, carrying content that
becomes CSS. Signing the lists was one answer; not having the channel was the other, and the one
consistent with D-33.

**[D-111](../rendering/content-blocking.md) takes the second.** Every list ships inside the binary and
changes only through the platform channels, so nothing reaches the generated stylesheet at run time that
did not arrive in a signed build, and there is no channel left for an attacker to compromise after
release. What remains is narrower and is stated rather than closed: a list compromised upstream at the
moment it is taken into the source tree reaches the next build, and the defence there is the review that
change receives — the supply-chain position of any vendored dependency. A user's own custom rules also
become CSS, and are the user's input rather than an adversary's.

It cut both ways, and the cost is recorded where the decision is. An endpoint that was already signed and
revocable would have been most of the machinery [R-11](../open-questions.md) says Sift does not have for
urgent fixes. Removing it makes R-11 permanent for lists as D-33 already made it for code.

## Non-defences

Sift does not claim to defend against a provider that serves malicious content as if it were mail, nor
against a compromised operating system credential store. It also does not attempt to detect phishing by
content analysis; what it does is refuse to *misrepresent* a destination — see
[link handling](../rendering/link-handling.md).

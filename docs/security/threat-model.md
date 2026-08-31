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

**Out of scope:** an attacker who already has the user's live session with code execution as that user,
a malicious provider, and physical attacks against a running machine.

## Attacker-controlled inputs

| Input | Reaches | Primary defence |
|---|---|---|
| MIME structure and headers | the parser | streaming parse, bounded output, degradation to raw view — NFR-19 |
| HTML body | the sanitizer, then the body view | [sanitizer invariants](../rendering/sanitizer-invariants.md) I1–I10 |
| CSS | the sanitizer, the blocker, the dark transform | parsed as a tree, fetching positions enumerated explicitly |
| Remote resource URLs | the resource broker | blocked by default; [content blocking](../rendering/content-blocking.md) |
| Images | the decoder, **in the resident process** | bounded decode; classification cached by content hash. The placement is [an open question](../open-questions.md) |
| Link targets | the confirmation UI | punycode decoding and bidi stripping — [link handling](../rendering/link-handling.md) |
| Filter lists | the filter engine, and generated stylesheets | stale-tolerant; never blocking; fetched over the network policy tier. **Integrity is an open gap** — see below |

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

**One attacker-controlled input in the table above reaches no layer at all.** Image bytes are decoded in
the core for classification under [D-29](../rendering/content-blocking.md), so a decoder defect is a
memory-safety bug in the resident process rather than a degradation something else catches — and it is the
one hostile input the design hands to the trusted half of the system rather than the disposable half. That
placement is tracked as [an open question](../open-questions.md).

The invariants **without** a backstop — idempotence, boundedness, parse stability, no content invention,
encoding determinism — are where review attention belongs. See
[sanitizer invariants](../rendering/sanitizer-invariants.md).

## An unresolved gap: filter-list content

Filter lists are listed above as attacker-controlled input, and the defence recorded for them —
stale-tolerant, never blocking — addresses *availability* only. It does not address content. Element
hiding and style injection are realised as a **generated stylesheet injected into every message body**, so
a hostile or compromised list can inject CSS everywhere, and no signing or list-content sanitization
requirement exists anywhere in this documentation set.

This is a real gap rather than a deferred choice, and it is [tracked as a risk](../open-questions.md).

**The prior question is whether Sift should operate that channel at all.**
[D-33](../product/platforms-and-distribution.md) removed self-update on the reasoning that an application
resident on a user's machine, reading their mail, "is the wrong place to put a bespoke code-delivery
path". The list-update rows in the [egress table](privacy.md) are a bespoke delivery path into every
message body, from a source Sift operates, carrying content that becomes CSS. That is content delivery
rather than code delivery, but it is the shape D-33's own sentence describes, reached from a different
direction. Signing the lists is one answer. Not having the endpoint — shipping lists bundled and updating
them through the platform channel with the rest of the binary — is the other, and it is the one consistent
with D-33.

It cuts both ways, which is why this is a question rather than a conclusion. An endpoint that is already
signed and revocable is most of the machinery [R-11](../open-questions.md) says Sift does not have for
urgent fixes. Keeping it changes what R-11 costs; removing it makes R-11 permanent.

## Non-defences

Sift does not claim to defend against a provider that serves malicious content as if it were mail, nor
against a compromised operating system credential store. It also does not attempt to detect phishing by
content analysis; what it does is refuse to *misrepresent* a destination — see
[link handling](../rendering/link-handling.md).

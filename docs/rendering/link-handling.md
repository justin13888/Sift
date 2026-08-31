# Link handling

What happens between a click in a message and a page in a browser.

**Owns:** FR-30, FR-42.

## Navigation never happens in place

All navigation attempts from the body view MUST be intercepted. Nothing navigates within the body view;
the document that renders a message renders only that message. This is part of invariant N-1 — see
[webview isolation](webview-isolation.md).

On an intercepted navigation, Sift MUST show the **real resolved destination** and open it in the system
browser only on explicit confirmation.

## Presenting the destination honestly

The displayed URL MUST be **punycode-decoded and stripped of bidirectional overrides**.

Both defences address the same attack: a URL that renders as one domain and resolves to another.
Internationalized domain names permit homograph substitution, and bidirectional control characters can
reverse the apparent order of a URL's components so that the visible text and the actual host disagree.
Showing the raw string is not enough; showing a naively decoded string is worse. See also NFR-28 in
[the pipeline](pipeline.md).

## FR-30 — Unwrapping click trackers

Marketing mail routinely wraps every link in a click-tracking redirector. Sift MUST display the
**unwrapped final destination**, with the raw wrapper available on request.

This is a **display** concern, not a blocking one. The wrapper fetches nothing until clicked, and when
clicked it opens in the system browser — outside Sift's control entirely. What Sift owes the user is an
honest answer to "where does this actually go" *before* they commit.

Where the destination is recoverable from the wrapper without a network request, Sift MUST recover it
locally. Sift MUST NOT resolve a wrapper by fetching it: following the redirect is itself the tracking
event, which is the same trap as image prefetch — see [content blocking](content-blocking.md). Where the
destination is not locally recoverable, Sift MUST say so rather than guess.

Tracking query parameters MUST be stripped using the removal rules already carried by the filter lists,
rather than a bespoke parameter list.

## FR-42 — Unsubscribing is shown, never performed

**FR-42.** Where a message declares an unsubscribe destination, Sift MUST surface it in the reader, under
the same display rules as any other link, and MUST open it in the system browser on explicit confirmation.
**Sift MUST NOT issue an unsubscribe request itself**, by any method.

Unsubscribing is among the most common things a person wants to do with the mail a triage client is for,
so leaving it out entirely would be the kind of gap that keeps a web interface open — which
[mutations](../mail/mutations.md) names as the outcome this product exists to avoid.

Performing it is nonetheless out of the question, for two independent reasons. The historical form is a
message, which [scope](../product/scope.md)'s no-send constraint forbids outright. The modern form is an
HTTP request, which would be egress from somewhere other than the
[resource broker](../architecture/resource-broker.md) — falsifying the completeness of the
[egress table](../security/privacy.md) — and the address it goes to carries a high-entropy per-recipient
token, which is precisely what FR-29 in [content blocking](content-blocking.md) treats as evidence that a
resource is tracking the reader. Sift would be issuing the request its own heuristics exist to prevent.

Showing the destination costs nothing and gives the user the action. Opening it in the browser is the same
handoff FR-41 makes for replying, for the same reason: the capability belongs to another application, and
Sift's job is to be honest about where the user is going rather than to go there for them.

A `mailto:` unsubscribe destination is shown and reported as requiring a mail handler, exactly as FR-41's
absent-handler case is. It is not silently omitted, because a user who cannot see it cannot know it
existed.

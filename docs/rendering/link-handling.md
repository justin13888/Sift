# Link handling

What happens between a click in a message and a page in a browser.

**Owns:** FR-30.

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

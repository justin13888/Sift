# Search

**Owns:** D-5, FR-19, FR-20, FR-21, NFR-5.

## D-5 — Local full-text search via the embedded database's FTS extension

**Chosen:** the embedded database's built-in full-text index, with diacritic-insensitive Unicode
tokenization, plus a trigram index if substring matching proves necessary.
**Rejected:** a dedicated embedded search engine.

**Why.** A second index is a second consistency problem and a second memory budget, in a process whose
primary requirement is idle footprint. Keeping the index inside the same transactional store as the
messages means it cannot drift from them, which removes an entire class of "search says it exists but it
doesn't" bug.

**What it costs.** Weaker ranking, and clumsier phrase and wildcard handling.

**Contestable because:** ranking quality is the product here — a search that returns the right message
fourth is a worse product than one that returns it first. Revisit **only on measured ranking failure**
against the [relevance corpus](../product/reference-environment.md), not on preference — and note that
this condition could not fire until that corpus was named, because neither the scale nor the fidelity
corpus can measure whether the right message came back first.

## FR-19 — Local search

Full-text search across cached mail, incremental as the user types, with sub-second results.

## FR-20 — Structured operators

The query grammar MUST support at minimum: sender, recipient, subject, attachment presence, unread state,
location, before and after dates, and quoted phrases.

Operators are part of the contract because they are how a triage-oriented client is actually driven — and
they are how [bulk operations over search results](../mail/mutations.md) become useful rather than
dangerous.

## FR-21 — Hybrid search and provenance

Local results MUST be merged with server-side search for the uncached tail, and results MUST be
**labelled by source**.

Provenance is not a nicety. The cache is bounded, so some mail is inevitably unsearchable locally. Showing
which results came from where makes "why didn't it find that" an answerable question instead of a trust
failure. What a given account can delegate to the server is a declared capability — see
[provider model](../mail/provider-model.md).

Server-side search is a network operation and is therefore subject to the active policy tier in
[network conditions](../runtime/network-conditions.md).

## NFR-5 — Latency

Keystroke to results under 100 ms at p95 over 500,000 messages, measured on the
[reference environment](../product/reference-environment.md) with a dedicated benchmark harness.

The index's memory use is a declared, budgeted cache like any other — see
[memory pressure](../runtime/memory-pressure.md). Its use of **disk** is bounded by NFR-52 in
[cache and blobs](cache-and-blobs.md), together with the envelopes it indexes; an index entry is evicted
with its message row and never on its own, which is what preserves D-5's no-drift argument.

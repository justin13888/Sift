# Search

**Owns:** D-5, D-79, FR-19, FR-20, FR-21, NFR-5.

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

**Search is scoped to every account by default**, narrowable to one account or one folder. This is the
scope NFR-5 already assumes without saying so: its 500,000 messages are the whole
[scale corpus](../product/reference-environment.md) across five accounts, not one account's share of it,
so the target is a statement about the merged case — five separate indexes, each behind
[D-42](encryption.md)'s page-decryption layer, merged in the
[presentation layer](../architecture/presentation-layer.md) on every keystroke.

Stating the default matters because the number means different things at different scopes, and a
per-folder default would let NFR-5 be met by a build that never does the hard thing. It also matters for
the product: a user searching for a message rarely knows which account it arrived in, which is the same
observation [D-4](../architecture/presentation-layer.md) makes about the unified inbox.

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

## D-79 — The merge ranks on comparable features, not on scores from different corpora

**Chosen:** the presentation layer orders merged results by a **relevance signal it computes itself**
from features that do not depend on any one index's corpus statistics; each index's own score is used
only to order results *within* one account. Where a query carries no relevance signal at all, the order
falls back to [D-55](../architecture/presentation-layer.md)'s list order.
**Rejected:** merging on the per-index relevance scores directly; normalizing scores across indexes;
ordering everything by received time.

**Why the obvious merge is wrong.** FR-19 above scopes search to every account by default and describes
the mechanism — *"five separate indexes … merged in the
[presentation layer](../architecture/presentation-layer.md) on every keystroke"*. A full-text relevance
score is computed against the statistics of the corpus it came from: how rare a term is *in that index*.
Five accounts are five different corpora, so the same document scores differently depending only on which
account it landed in, and a term that is rare in a small work account and common in a large personal one
produces scores that cannot be compared at all. Sorting by them looks like ranking and is closer to
sorting by account size.

Normalizing them is worse, because it looks principled. There is no shared distribution to normalize
against, and a per-index rescaling makes the top result of a three-message account outrank a genuinely
better match in a half-million-message one.

**What the layer computes instead.** Features that mean the same thing in every account: which field
matched, whether the match was a phrase or scattered terms, how many of the query's terms are present,
and recency. None of them consults corpus statistics, so all of them are comparable across accounts by
construction. The per-index score is not discarded — it is the right tool for its own scope, and it
orders results within one account where the corpus *is* shared.

**Why this is D-5's weak point being made explicit rather than repaired.** D-5 concedes that ranking
quality is where that choice is weakest and that *"a search that returns the right
message fourth is a worse product than one that returns it first"*. This decision does not fix that. It
makes sure the merge does not *add* a second, larger ranking error on top of it — and it is the thing the
[relevance corpus](../product/reference-environment.md) will actually be measuring, since that corpus is
defined over real queries against a known mailbox rather than over one account's share of it.

**Server results interleave by the same features, and are labelled.** FR-21's server-side results arrive
ranked by somebody else's algorithm and usually carry no score at all, so they cannot join a score-based
merge under any scheme. They can join a feature-based one, because the features are computed from the
message rather than from the index that found it. Provenance labelling under FR-21 stays exactly as it
is, and is what lets a user see that the ordering mixed two sources.

**The no-relevance case is not an edge case.** A query of pure structured operators — unread mail in a
folder, everything with an attachment before a date — has no terms to be relevant about, and a ranking
built from term features would order it arbitrarily. Falling back to the list order means such a query
returns the same order the folder would, which is what a user issuing it expects and is already the
order they read in.

**What it costs:** a second ranking implementation beside the index's own, which must be cheap enough to
run on every keystroke inside NFR-5's 100 ms over the merged set, and which is one more thing that can be
wrong about relevance.

**Contestable because:** a hand-built feature ranking is a worse ranker than a well-tuned corpus-aware one
within a single account, so this trades peak quality for cross-account coherence. If measurement against
the relevance corpus shows the merged ranking is materially worse than the per-account one, the honest
retreat is not to normalize scores but to reconsider [D-6](data-model.md)'s per-account databases — which
is the revisit D-6 already names, arriving from the search side.

## NFR-5 — Latency

Keystroke to results under 100 ms at p95 over 500,000 messages, measured on the
[reference environment](../product/reference-environment.md) with a dedicated benchmark harness.

The index's memory use is a declared, budgeted cache like any other — see
[memory pressure](../runtime/memory-pressure.md). Its use of **disk** is bounded by NFR-52 in
[cache and blobs](cache-and-blobs.md), together with the envelopes it indexes; an index entry is evicted
with its message row and never on its own, which is what preserves D-5's no-drift argument.

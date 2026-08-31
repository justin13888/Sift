# Cache and blobs

**Owns:** D-23, FR-10, FR-12, NFR-14, NFR-49, NFR-52.

## The cache is not the source of truth

The local cache is a **bounded, evictable tier**. The provider is the source of truth. Anything the cache
holds may be discarded at any moment — under [memory pressure](../runtime/memory-pressure.md), under a
disk budget, or on eviction — and the application MUST remain correct when it is.

This framing is what permits aggressive shedding without a correctness argument each time.

## Tiering

**Envelopes are kept for as long as their own budget allows**, which in practice means far longer than any
body. They are small, and they are what makes list scrolling instant. An account whose envelopes are
retained is browsable and searchable offline even with no bodies cached at all.

They were previously described as retained *indefinitely*, and that was the one place this documentation
set contradicted its own hard constraint. See NFR-52.

**Bodies and attachments are bounded.** Default budget: 90 days or 2 GB, whichever binds first, evicted
least-recently-used. The budget is user-configurable.

The ninety days is wall-clock age and the last-use ordering is recorded on the same clock, so that the two
halves of one policy cannot disagree after a time correction. This is the opposite choice from the
[timing wheel](../runtime/scheduling.md), and deliberately: a retention policy is about calendar time,
while a deadline is about elapsed time.

## Blob store

Bodies and attachments live in a **content-addressed store on disk**, keyed by a **keyed** cryptographic
hash of the plaintext content — see [D-43](encryption.md), which explains why a bare hash of the
plaintext is not usable as the address — with small parts inlined into the account database instead.
Blobs are reference-counted and collected when their count reaches zero; the count is held in the shared
blob index and its ordering rules are in [data model](data-model.md).

Content addressing gives deduplication for free, which matters more in mail than elsewhere: the same
attachment arrives repeatedly, in multiple accounts, forwarded and re-forwarded. It also makes the
[image classification](../rendering/dark-mode.md) keyable by content hash.

**"Classified once ever" is a claim about durability, and it therefore needs a durable home.** A
classification is the output of a full decode — the one expensive operation
[D-29](../rendering/content-blocking.md) exists to avoid — and it is a pure function of the bytes, so
recomputing it is waste rather than correctness. If it lived only in an in-memory cache, the L1 shed tier
in [memory pressure](../runtime/memory-pressure.md) would discard it and "once ever" would be false in
exactly the conditions under which decoding is least affordable.

Classifications are therefore held in the **shared blob index**, beside the hash, size, last use and
reference count already recorded there — see [data model](data-model.md). The in-memory classification
cache is a read-through cache over that record, declared and budgeted and sheddable like any other; the
record beneath it is not. It is a few bytes per distinct image, it is evicted with the blob it describes
rather than on its own schedule, and it is encrypted under the per-installation secret with the rest of
the index under [D-43](encryption.md).

Reference counting is what makes [account removal](../mail/accounts.md) correct rather than approximate.

## D-23 — BLAKE3 as the content address

**Chosen:** BLAKE3, both as the store's key and as the input to the convergent blob keys in
[D-22](encryption.md).
**Rejected:** SHA-256, with or without a hardware-acceleration requirement.

**Why — and the first thing to record is what this decision is *not* about.** Blob hashing is dominated by
I/O in every realistic case. Hashing a typical body is a rounding error against NFR-3's 80 ms budget, and
hashing a large attachment is under two per cent of the time spent transferring it. **Nothing hashes at
idle**, so this choice has no bearing on NFR-12 or on battery. The primitive is not
performance-load-bearing, and a future reader who "optimizes" it on speed grounds is solving a problem
that does not exist.

What decided it is the absence of a hardware requirement. SHA-256 is the more conservative primitive and
is the one nobody argues about, but its competitive throughput depends on a CPU extension that arrived
late on mainstream Intel mobile parts — later than the 2020-era laptop the
[reference environment](../product/reference-environment.md) specifies as the rig. Making acceleration a
hard startup requirement could therefore exclude the reference machine itself and narrow the Linux install
base, to buy back a millisecond nobody perceives. BLAKE3 performs uniformly across the target CPUs with no
mandate at all, and its keyed derivation feeds D-22 without a separate step.

That last property turned out to be worth more than it looked. [D-43](encryption.md) keys the content
address itself under a per-installation secret, which with BLAKE3 is a mode of the same primitive rather
than a construction bolted around a different one.

**Contestable because:** SHA-256 is the primitive a security reviewer expects to find, and choosing
anything else costs a paragraph of explanation forever — this one. If that friction outweighs the
uniformity argument, SHA-256 with acceleration treated as an optimization and never as a requirement is a
perfectly good answer.

## FR-10 — Attachments

An attachment list MUST be shown with lazy download, save-to-disk, and open-with-system-handler. Opening
an executable type MUST require an explicit warning first.

Attachments MUST NOT be fetched as part of message fetch. See the fetch discipline in
[sync engine](../mail/sync-engine.md). A single fetch is additionally capped by byte ceiling under
constrained network conditions — see NFR-39 in [network conditions](../runtime/network-conditions.md).

## NFR-49 — Saved attachments carry their provenance

A file written by save-to-disk MUST carry the platform's own marking for content that arrived from an
untrusted source — the quarantine attribute on macOS, so that Gatekeeper evaluates it on first open.

The reason is that FR-10's warning protects only the path through Sift. A user who saves an attachment and
opens it an hour later from their file manager gets no warning from Sift and never will, because Sift is
not involved. The provenance marking is what carries the fact forward to the system component whose job it
is — and every browser already does this, so an attachment that arrives by mail would otherwise be treated
*more* trustingly than the same bytes downloaded from the web.

Where a platform offers no equivalent mechanism, that MUST be stated in the interface rather than assumed
away: the user is then relying on FR-10's warning alone, and deserves to know it.

## FR-12 — Offline read, and honest state

Any cached message MUST be readable offline. The UI MUST clearly distinguish **"not cached"** from **"not
available"**.

These are different facts and conflating them is a lie of omission. "Not cached" means Sift can fetch it
when the network returns. "Not available" means the server no longer has it. A user deciding whether to
find another way to reach a message needs to know which they are looking at.

## NFR-14, NFR-52 — Two budgets, and together they are the whole of disk

**NFR-14.** Bodies, attachments and the blob store MUST stay within the user-configured cache budget to
within 5%, and the cap MUST be **enforced, never advisory**. A cache that grows forever is not a cache.

**NFR-52.** Envelopes, the message rows carrying them, and their
[full-text index](search.md) entries MUST stay within a second declared, user-configurable budget, evicted
oldest-first, with an envelope and its index entry evicted together as one unit.

**NFR-14 was false as written, and the repair is a coherence fix rather than a measurement.** It said disk
was bounded by the cache budget and hard-capped, while this document retained envelopes indefinitely and
[search](search.md) indexed body text. Neither was inside the budget and nothing else bounded them, so the
one requirement bounding disk did not reach the two artefacts that grow forever — in a set whose
[hard constraints](../../AGENTS.md) open with "no unbounded cache". At the 500,000 messages NFR-5 is
stated over, the index alone is not a rounding error against a 2 GB body budget.

Two budgets rather than one widened cap, because the two tiers are evicted on different principles and
merging them would let a large attachment evict a year of envelopes. NFR-14's is least-recently-used over
things the provider will hand back on demand; NFR-52's is oldest-first over the cheap summary that makes
the client usable offline, and it is the one a user should rarely reach.

**Eviction under NFR-52 takes the index entry with the envelope, and eviction under NFR-14 does not.** A
body evicted for space leaves its indexed text in place, which is what keeps a message findable after its
body is gone — the search result is then an ordinary FR-12 "not cached" message that is re-fetched when
opened. This also preserves [D-5](search.md)'s argument that the index "cannot drift" from the messages,
because the index entry and the message row are created and destroyed in the same transaction. Only when
the message row itself goes does its index entry go with it.

Eviction MUST be driven by the budget rather than by a periodic sweep hoping to keep up, and blob
collection MUST run as part of eviction rather than as a separate hoped-for pass.

**What this costs the user is legibility, and it is owed to them.** An account whose oldest envelopes have
been evicted is no longer complete, and local search over it no longer covers everything the provider
holds. That is exactly the confusion [FR-21](search.md)'s labelling by source exists to answer, and this
is the second thing that makes it load-bearing rather than a nicety.

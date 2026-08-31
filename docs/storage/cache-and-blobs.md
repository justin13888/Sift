# Cache and blobs

**Owns:** D-23, D-57, D-73, D-77, FR-10, FR-12, NFR-14, NFR-49, NFR-52, NFR-53.

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

## D-77 — A blob is a chunked, sealed file whose name is its address

**Chosen:** a blob is stored as a sequence of independently sealed chunks under one header; its filename
is its content address and it is placed in a directory fan-out derived from that address; a decrypt
failure discards the file, and the plaintext hash is **not** re-verified on read.
**Rejected:** one authenticated seal over the whole blob; a flat directory; storing blobs under a
per-account name.

**Why chunks rather than one seal.** [L-14](../limits.md) permits a blob of 2 GB, and a single seal over
the whole file makes it unreadable until all of it has been read and authenticated. Three things in this
design already need less than the whole file: FR-10's hand-off to the platform's preview facility,
[NFR-39](../runtime/network-conditions.md)'s byte ceiling on a single fetch, and a body part that streams
into the pipeline under [NFR-19](../rendering/pipeline.md)'s never-materialize-a-large-part rule. A
whole-file seal forecloses all three, and would do it invisibly — everything works until the first large
attachment.

**Chunks inherit [D-76](encryption.md)'s nonce rule exactly**, with the chunk index taking the place of
the page number and the same durable counter beneath it. A blob is written once and never rewritten, so
the reuse hazard is smaller here than in a database — but "smaller" is not a construction, and one rule
for both is what makes it reviewable.

**Why the filename is the address.** The address is already a keyed hash under
[D-43](encryption.md), which is what makes it safe to expose: it is not derivable by anyone who does not
hold the per-installation secret, so the filename discloses nothing about the content to a filesystem
observer. That is the property that lets the store be a flat mapping from address to file with no index
lookup on the read path. A per-account name would defeat [D-22](encryption.md)'s deduplication, which is
the entire reason the store is shared.

**Why a fan-out.** A single directory holding hundreds of thousands of entries is a performance cliff on
some filesystems and an operational hazard on all of them. The fan-out is derived from the address so it
requires no state to compute, and its depth is one more thing that is fixed at first release, because
[platform baseline](../product/platform-baseline.md) records that the on-disk layout *"is published in
the Cask uninstall stanza and depended on by every installed copy"*.

**Why the plaintext hash is not re-verified on read.** The authenticated seal already proves the bytes are
the bytes Sift wrote, under a key an attacker does not hold. Re-hashing proves the same thing again, at
the cost of reading the whole blob before yielding any of it — which is exactly the property chunking
exists to provide. **The two failures are different events and only one of them is real**: a decrypt
failure means the file was tampered with or corrupted, and a hash mismatch on correctly-decrypting bytes
would mean Sift stored the wrong content under an address, which is a defect rather than an attack. The
second is worth asserting in tests and not on every read.

**A blob that fails to authenticate is deleted, not repaired**, and its reference count is left alone —
the accounts still legitimately reference that content; they simply do not have it. The next fetch
re-downloads it. This is D-73's discardability doing the work again, and it is why a corrupt attachment
is a re-download rather than an error.

### Orphans in both directions, and only one of them is a leak

Reference counting handles the case the design already names: a count reaching zero collects the file.
Two other states exist and neither had a rule.

**A file with no index row** — a write that completed after the crash that lost its index entry, or a
file left by a partial download. It is invisible to eviction, because eviction reads the index, so it is
unbounded disk that NFR-14's budget cannot see. **The refcount rebuild [data model](data-model.md)
already requires after abnormal termination MUST also collect files the index does not name**, which is
the same pass and the same trigger.

**An index row with no file** — a file deleted beneath Sift, or the failure above already handled. It is
harmless and MUST NOT be an error: the row is dropped and the content is treated as not cached, which is
a state FR-12 already has and the user already understands.

The asymmetry is the point. A missing file is a cache miss; a missing row is a leak, and a leak in the
one place D-73 says the user may throw everything away is the failure that makes them unable to.

**What it costs:** per-chunk overhead, a header format that joins the identifier register in permanence,
and a collection pass that must walk the store's directories rather than only its index.

**Contestable because:** chunking is complexity bought for large attachments in a client that does not
send mail and whose users mostly receive small ones. If the corpus shows the large-attachment case is
rare, a whole-file seal is simpler and the preview hand-off could copy to a temporary file instead —
which is a real alternative, and a worse one only because it writes plaintext to disk to avoid reading
ciphertext from it.

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

**What counts as an executable type is decided from three sources, and disagreement between them is itself
a reason to warn.** The declared media type, the sniffed content, and the extension of the name about to
be written are each incomplete alone, and the platform does not consult the one a naive check would: what
happens when a file is opened is decided by its **extension**, not by the type the sender declared. A
message can therefore declare `text/plain` on a file whose name ends in an executable extension and defeat
any type-based check entirely. Sift MUST warn on the union of the three, and MUST treat a mismatch among
them as suspicious in its own right — a sender who labels an executable as a document has told Sift
something about their intent.

**Previewing an attachment through the platform's own preview facility is permitted and preferred over
building one.** It decodes hostile bytes out of process, in a component maintained by the platform, which
is strictly better than the alternative of teaching Sift to render more formats. This is a *different*
decode path from the classification decode [Q-14](../open-questions.md) is about, and does not wait on it.
Dragging an attachment out to the file system is likewise permitted, and MUST carry NFR-49's provenance
marking exactly as a save does — it is the same act of writing sender-controlled bytes to disk.

## NFR-53 — A sender-supplied name never becomes a path

**NFR-53.** Sift MUST NOT use a sender-supplied filename as a filesystem path. The name written is derived
from it under the normalization NFR-54 in
[presentation layer](../architecture/presentation-layer.md) applies to every attacker-controlled string,
with path separators, traversal segments, absolute prefixes, control characters and reserved names
removed; the exact final path MUST be shown to the user before the write; and an existing file MUST NOT
be overwritten.

This is the first place Sift writes attacker-controlled bytes to a user-chosen location under an
attacker-chosen name, and no document covered it. NFR-28 in [the pipeline](../rendering/pipeline.md)
already requires decoding parameter continuations and encoded words in headers — so the *output* of that
decoding is arbitrary sender-chosen text, and it is that text which was going to become a path.

The failure that makes this a requirement rather than hygiene is the one
[link handling](../rendering/link-handling.md) already defends against for URLs: a right-to-left override
inside a filename produces a name that renders as a document and executes as a program. Showing the final
path is what makes that visible, and it is the same remedy — display the thing honestly rather than try to
detect malice.

The [threat model](../security/threat-model.md) carries the filename parameter and the save path as inputs
in its own table, which they were previously absent from.

## D-57 — The cache is excluded from backup; the queue is not

**Chosen:** the blob store and the account databases' cache-shaped content are excluded from the
platform's backup mechanism. **The mutation queue is not excluded.**
**Rejected:** backing up everything; excluding the whole data directory.

**Why exclude.** The store is a refillable cache of a provider's data, and it is bounded by NFR-14 at a
default of 2 GB per installation. Backing that up spends the user's backup capacity on bytes their mail
provider already holds — and it spends it on bytes that will not work when restored, because
[D-43](encryption.md)'s per-installation secret lives in the credential store and does not travel with the
files. A restored copy is either useless or, in the case the secret is genuinely gone, precisely the
orphaned store D-43 requires to be **discarded wholesale**.

**Why the queue is the exception, and why a blanket exclusion is wrong.** A queued mutation is the one
thing in the store that the provider does not know about, which
[D-32](data-model.md) already establishes when it explains why a downgrade must drain the queue first.
Excluding the whole data directory would therefore make queued triage the single unrecoverable loss in the
product, quietly, as a side effect of a storage-efficiency decision.

**This is implementable only because an account is two files.** Backup exclusion operates on files, so
"exclude the store and not the queue" is not an operation that exists while both live in one database.
[D-74](data-model.md) splits them, and this decision is what forced the split.

**What it costs:** two exclusion scopes rather than one, and a restore that produces an account with
pending mutations and no cached bodies — a state that must be correct, and which the cache-is-not-the-
source-of-truth framing at the top of this document already requires it to be.

**Contestable because:** the first real restore onto a new machine is when D-43's orphaned-store path is
exercised for the first time, and this decision makes that path more likely rather than less by ensuring
the blobs are absent rather than merely unreadable. That is the correct outcome and it does mean the code
which has never run is the code every migrating user meets.

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

## D-73 — Offline is best-effort, and there is no floor

**Chosen:** bodies are cached because a user read them, plus opportunistic prefetch on an unmetered
network. Sift promises no quantity of mail readable without a network, and MUST NOT be built around one.
**Rejected:** a stated retention target that sync actively fills; requiring a network, and treating the
store as a pure latency cache.

**Why this needed a decision at all.** Sift's position on offline was correct in four documents and
asserted in none. This one opens with the cache never being authoritative; FR-12 above requires any
cached message to be readable; [D-58](../runtime/network-conditions.md) gives offline two tiers that
queue everything; and the mutation queue survives weeks without a network. A reader can hold all four and
still not know whether offline usefulness is a **promise**. It is not, and the difference decides whether
[D-53](../mail/sync-engine.md)'s no-bodies-during-backfill rule is a limitation to be worked around or
the design.

**Why no floor.** A guaranteed corpus of recent mail means sync fetching bodies nobody asked for — which
is the body backfill D-53 refuses, on the grounds that *"a backfill that fetches body text for half a
million messages to fill one column is a different product"*. It would also spend a user's data
allowance on a prediction, in an application whose network behaviour is otherwise governed entirely by
what the user did. Best-effort keeps FR-12's *"not cached"* state honest: it is the truthful report of a
cache, rather than the symptom of a promise that went unmet.

**Why not the other direction either — requiring a network buys nothing.** It is worth stating because
the trade looks appealing and is not: **snappiness does not come from the body cache.** The list, search
and triage are local reads off envelopes in every design, and an uncached body is already budgeted at
NFR-4's 600 ms. Requiring a network would therefore leave the store, its encryption, its eviction and its
budgets exactly as they are — the envelope cache is what NFR-1, NFR-2 and NFR-5 are about — while adding
a round trip to every message open and stranding the durable queue, which exists so that triage survives
a bad network.

**The counterweight, which is what licenses everything else.** Because there is no floor, **every byte on
this machine is discardable**, which is this document's opening claim taken to its conclusion. That is
what permits shedding under pressure without a correctness argument each time, what permits
[encryption](encryption.md) to define a strict on-disk format that refuses rather than repairs, and what
makes [D-32](data-model.md)'s removal-and-resync path an acceptable last resort rather than data loss. A
guaranteed offline corpus would have quietly withdrawn all three, because a store you promised something
about is a store you cannot throw away.

**What it costs:** a user who boards a plane without opening their mail first has envelopes, search over
those envelopes, and few bodies. That is a real disappointment and it is the honest one, since the
alternative was to have downloaded their mail on cellular to prepare for it.

**Contestable because:** "read it before you lose signal" is a workflow users should not have to know,
and a small floor — the current folder's visible screen, say — would cover most of the disappointment for
little data. The reason it is not here is that any floor makes offline a promise, and a promise makes the
cache undiscardable, which is the property the rest of the design spends.

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

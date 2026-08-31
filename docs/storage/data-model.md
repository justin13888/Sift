# Data model

What Sift stores, and how it is partitioned.

**Owns:** D-6, D-21, D-32, D-44, NFR-48.

## D-6 — One database per account

**Chosen:** a separate embedded relational database file per account, with a shared blob store.
**Rejected:** a single database holding all accounts.

**Why.** The embedded database permits one writer at a time. Per-account files give parallel writers
across accounts, make [account removal](../mail/accounts.md) a file deletion, and contain the blast radius
of corruption to one account.

**What it costs.** No cross-account SQL. A [unified inbox](../architecture/presentation-layer.md) must be
assembled in memory by merging per-account result streams, and correct paging over that merge is the main
cost of this decision.

**Contestable because:** the unified inbox is a shipped feature (D-4), so the cost is paid in the default
view rather than an edge case. If the merge proves to be the dominant complexity in the presentation
layer, this decision is the one to revisit.

## D-21 — The embedded database is SQLite

**Chosen:** SQLite, with its full-text extension serving [D-5](search.md).
**Rejected:** a pure-Rust embedded key-value store paired with a separate index; a client-server database.

**Why.** This document and [search](search.md) already described SQLite without naming it — one writer at
a time, write-ahead logging, a built-in full-text index, an optional trigram index. Naming it is not new
information; it is the difference between a decision a reader can act on and one they must infer.

The convention in this documentation set is that a dependency is named where the choice **is** the
architectural decision. That is the case twice over here. D-5 chose "the embedded database's full-text
index" over a dedicated search engine, and that sentence is unactionable without knowing whose index, with
what ranking function and what tokenizer. D-6's entire argument rests on single-writer semantics and
per-file isolation, which are properties of this engine rather than of embedded databases generally.

A key-value store with a separate index was rejected for the reason D-5 already gives: a second index is a
second consistency problem and a second memory budget.

**What it costs:** a C dependency in a project that otherwise argues hard for Rust on hostile-input paths.
The mitigation is that the database never parses attacker-controlled bytes — everything reaching it has
already been through the [pipeline](../rendering/pipeline.md) — so it is not on the surface D-8 is about.

**Contestable because:** D-5 concedes that ranking quality is where this choice is weakest, and ranking is
the product. Naming the engine here makes that weakness concrete rather than resolving it.

## Entities

Per-account, unless noted.

| Entity | Holds | Notes |
|---|---|---|
| Account | provider, display name, declared capabilities | one row; the file *is* the account |
| Folder | remote identifier, semantic kind | kind is semantic, never a display name — see FR-5 |
| Folder sync state | per folder: cursor, validity identifier, last successful sync, degradation reason | one row per folder; see below |
| Message | remote identifier, internet message identifier, fallback identity digest, thread identifier, location, sender, recipients, subject, received time, origination date, flags, attachment presence, size, MIME structure, body reference, snippet | body reference is null when not cached; received time is the server's and is what [D-55](../architecture/presentation-layer.md) orders on, while the origination date is the sender's `Date` header and is only displayed; the snippet is present only where the account declares a snippet source, and is bounded by L-16; the digest is computed at ingest under D-44. Location and flags are the **base** state [D-51](../mail/mutations.md) defines; the pending overlay is held with the queue, not here |
| Thread | remote thread identifier, normalized subject, last activity, message count | scoped to the account — see [threading](../mail/threading.md) |
| Tag | tag identity and display name, and its membership | present only where the account declares tag support |
| Full-text index | subject, body text, sender text, recipient text | see [search](search.md) |
| Mutation queue | serialized intent, intent schema version, state, attempt count, creation time, per-message sequence, expiry, and the pending overlay the intent contributes | durable across process death *and upgrades* — see [mutations](../mail/mutations.md). The overlay lives here because it is retired with the intent that created it, never independently |
| Blob reference | content hash, role | the account's claim on a shared blob; refcounts live elsewhere, see below |
| Authentication result | per message: signing domain, sender-policy and alignment outcomes | feeds the [synthetic origin](../rendering/sender-origin.md) |
| Account policy | per-sender remote-content allowlist, per-sender dark-mode choice, notification rules per folder | user decisions scoped to this account — see below |

Credentials are **not** in this list and MUST NOT be stored here. See
[encryption](encryption.md) and [credentials](../security/credentials.md).

**Contacts are not in this list either, and their absence is a decision rather than an omission.**
Display-name resolution reads the platform's own contact store on demand and keeps nothing — see
[D-41](../architecture/presentation-layer.md). A contact entity here would be the second sync domain
[scope](../product/scope.md) excludes, and a durable record of the user's correspondents that nobody
asked for.

## Policy state has two scopes, and neither was previously stored anywhere

Requirements across this documentation set assume a durable record of what the user has decided, and
until now no document said where any of it lives. The per-sender allowlist FR-8 requires, the per-sender
dark-mode persistence FR-31 requires, the per-network override FR-35 requires, the list subscriptions
FR-27 requires, the cache budget NFR-14 calls user-configurable, and the preference FR-33 hides the debug
view behind are all durable user decisions with no entity, no owner, and no migration coverage. That is a
gap of the same shape as the one [D-43](encryption.md) closed for the shared blob index: state that
belongs to no account, and is therefore reached by no rule written per account.

**Two scopes, and the split follows erasure.** State that is meaningless without its account is
per-account and lives in the account database, so that FR-4's removal takes it with everything else.
State that outlives any one account is installation-scoped and lives in a single installation store
beside the shared blob index.

| Scope | Holds | Erased by |
|---|---|---|
| Account policy | per-sender allowlist, per-sender dark-mode choice, per-account and per-folder notification rules, the watched folder set under [FR-43](../runtime/scheduling.md), and the backfill's resume position | [FR-4](../mail/accounts.md) account removal, as part of the database |
| Installation policy | filter-list subscriptions and custom rules, per-network overrides keyed by network identity, cache and data budgets, the FR-36 accounting window and its running counters, whether sync is paused by the user, quiet-mode and notification defaults, dark-mode default, the debug-view preference, and the view state [D-72](../architecture/lifecycle.md) makes durable | uninstallation only |

**The per-sender allowlist is security state, not a preference, and MUST be treated as such.** It is the
record of which senders may cause the [resource broker](../architecture/resource-broker.md) to make a
network request, so write access to it is write access to Sift's egress policy — the same reasoning
[encryption](encryption.md) applies to authenticated storage generally, arriving at the same conclusion
from a different direction. It MUST be keyed on the **attested synthetic origin** and never on a displayed
sender, which is [sender origin](../rendering/sender-origin.md)'s rule restated at the point of storage
rather than a second rule.

**The installation store is encrypted and authenticated under the per-installation secret**, exactly as
the shared blob index is, and for the same reason D-43 gives: it belongs to no account, so the per-account
keys of [D-22](encryption.md) cannot cover it, and left in the clear it is both a profile of the user's
networks and a writable handle on the allowlist's installation-wide half.

**Both stores are covered by NFR-48 below.** A migration that preserved envelopes and queued mutations
while silently discarding a user's allowlist would satisfy the old wording and still hand every sender's
images back to blocked-by-default without saying so.

## Sync state is per folder, not per account

Cursors are held per folder, not on the account. This is not an implementation detail: two of the four
providers make it structural. A delta link is issued **per folder** on Microsoft Graph, and an IMAP folder
carries both a validity identifier and its own modification sequence. An account-level cursor cannot
express either, and cursor invalidation under NFR-18 is a per-folder event — one folder's validity change
MUST NOT invalidate the account.

Advancing a cursor and applying the change it describes MUST commit in one transaction. A cursor that
advances past changes that were not applied is silent data loss, and it is invisible until a user notices
a message that never arrived.

## The shared blob index owns refcounts

The blob store is shared across accounts and content-addressed, so a reference count is a property of the
*store*, not of any one account. It therefore lives in a **shared blob index** alongside the store — hash,
size, last use, count, and the image classification derived from the content under
[dark mode](../rendering/dark-mode.md) — while each account database holds only its own references. The
index is encrypted under the per-installation secret in [D-43](encryption.md); it belongs to no single
account, so the per-account keys of [D-22](encryption.md) cannot cover it.

This split needs an ordering rule, because the two are separate databases and cannot commit atomically:

- **On add**, write the account's reference first, then increment the shared count.
- **On remove**, decrement the shared count first, then drop the account's reference.

Both orders fail toward **over-counting** — a blob retained that could have been collected — never toward
under-counting, which would free bytes another account still points at. Over-retention is bounded by the
hard cap in NFR-14, so the failure mode costs disk that eviction was going to reclaim anyway.

A refcount rebuild from the union of all account references MUST exist, and MUST run after abnormal
termination. It is a repair path, not the collection mechanism; collection itself runs as part of eviction
per NFR-14 in [cache and blobs](cache-and-blobs.md).

## Identity

Local identity is Sift's own, assigned on ingest. Remote identifiers are attributes, not keys, because
[one provider's identifiers change on move](../mail/providers/microsoft-graph.md) and IMAP identifiers are
only unique within a folder generation.

**The internet message identifier is not reliably unique.** Some servers and some senders duplicate it,
and some omit it. Any deduplication or cross-folder join keyed on it MUST have a defined fallback and MUST
NOT assume uniqueness. That fallback is D-44 below.

## D-44 — Identity joins are corroborated, and an uncorroborated join does not happen

**Chosen:** the internet message identifier narrows a join's candidates and never keys it; the result is
corroborated against a **fallback identity digest** stored per message; and a candidate set that does not
reduce to one resolves to distinct messages.
**Rejected:** a normalized-header digest promoted to a second identity key; accepting the ambiguity and
never joining at all.

Three documents required a fallback here and none defined one — [threading](../mail/threading.md)'s
reconstruction, the cross-folder join above, and the move join on
[Microsoft Graph](../mail/providers/microsoft-graph.md). All three are joins *within one account*, because
threading forbids merging across them, and that is what lets one rule serve all three.

**Every join is scoped before it is keyed.** Candidates are drawn from within one account and from the
bounded set the join's own context supplies: the reference chain for FR-11 reconstruction, the
conversation identifier for the Graph move join, the sibling folder for a cross-folder join. Sift MUST NOT
discover a join by scanning a store for identifier equality. Scoping is what does the work; the identifier
only orders the candidates within a scope that was already small.

**The digest corroborates a candidate; it does not key one.** The digest is taken at ingest over a
normalized tuple — originator address, origination date, normalized subject, and the message's own
reference chain — chosen because those are what a compliant relay carries unchanged. It MUST be used only
to confirm or reject a candidate the scope already proposed. Promoted to a key, it would join a
mailing-list copy to a direct copy of the same mail, which is right for deduplication and wrong for the
move join, where the two are separate objects the user can move independently.

**Ambiguity resolves to distinct messages.** Where the identifier is absent, or the candidate set is not a
singleton after corroboration, Sift MUST NOT join, and MUST record the near-miss for the FR-33 debug view
rather than passing over it silently.

**Why.** The two failure directions are not symmetric, so a rule that must guess should default to the
recoverable one. A missed join shows a message twice, or shows two threads where there should be one: it
is visible, the user can see both copies, and nothing is destroyed. A false join collapses two distinct
messages into one entity — and because FR-13 intents act on the entity, the next archive or delete reaches
mail the user never saw. Failing to merge is a display defect; merging wrongly is data loss wearing a
display defect's clothes.

**What it costs.** A fixed-size digest per message, computed on a path that already parses the headers.
Duplicates survive where a server both omits the identifier and rewrites the corroborating headers, and
that combination is not rare on the mailing lists where the problem is worst. On Microsoft Graph, a move
that cannot be resolved presents as a delete plus an arrival, so local flags and read state for that
message do not survive it. Changing the normalization rules is a reindex of one column, not a resync,
which is what keeps this inside NFR-48.

**Contestable because:** the normalization tuple is a hypothesis about which headers survive transit, and
no corpus stands behind it — it belongs in the fidelity corpus of the
[reference environment](../product/reference-environment.md), and [R-5](../open-questions.md) stays open
against it. If measurement shows the tuple collides materially, or diverges across an ordinary relay hop,
the retreat is the rejected third option: never join. That costs duplicate display and the Graph move, and
nothing else, which is why it is a retreat rather than a redesign.

## D-32 — Migrations are versioned, forward-only, and never resync

**Chosen:** a schema version stamped in each account database, ordered forward-only migrations, and a
refusal to open a database newer than the running binary understands.
**Rejected:** an additive-only schema that any version can read; dropping and rebuilding derived state on
schema change.

**Why.** No migration story existed at all, in a design that has a durable queue of **serialized intents
outliving upgrades** and a stored capability set that will gain fields. That is not a gap that stays
theoretical past the second release.

Refusing to open a newer database matters because Sift is user-installed software that can be downgraded,
and silently reading a future schema is how a downgrade corrupts data instead of merely failing.

Additive-only was rejected because the schema accretes dead fields permanently and old and new code
disagree about which are authoritative — a subtler failure than a migration that runs. Rebuild-derived-
state was rejected because rebuilding a full-text index over 500,000 messages is exactly the visible,
expensive, network-free-but-not-free resync that NFR-18 works to avoid.

**Forward-only means the chain has no natural end, and one is stated elsewhere.** A build must migrate
from whatever it finds, and [D-33](../product/platforms-and-distribution.md) removes any ability to make
a user upgrade — so without a bound the chain reaches back to the oldest build still installed, forever.
[D-62](../build/packaging.md) bounds it with a stated migration floor and makes advancing that floor a
deliberate amendment. Nothing about forward-only changes; what changes is that the obligation it creates
is now owned by something.

**Contestable because:** forward-only means a downgrade is unsupported once a migration has run, and the
only recovery is account removal and resync. For a cache-shaped store that is defensible; a reader who
finds it indefensible should argue for reversible migrations, not for additive-only.

**The store is cache-shaped in every place but one, and that is where this costs something.** NFR-48 below
preserves queued mutations across an *upgrade*. Nothing preserves them across the downgrade this decision
makes unsupported, because the stated recovery is removal and resync, and a resync restores what the
provider knows. A queued mutation is exactly what the provider does not know yet. So a downgrade after a
migration discards writes the user watched succeed — the one failure [mutations](../mail/mutations.md)
says the queue exists to prevent, arriving down a path that document does not cover.

That is not an argument for reversing D-32. It is a requirement on the recovery path: removal-and-resync
after a refused open MUST drain or export the queue before it destroys the database, and MUST NOT do
neither silently. The exposure is also uneven across channels, which is worth knowing before judging how
theoretical it is — the Mac App Store offers users no way to downgrade, while Homebrew Cask and Flatpak
both do. See [platforms and distribution](../product/platforms-and-distribution.md).

**NFR-48.** A schema migration MUST preserve envelopes, cached blobs, queued mutations together with the
pending overlays [D-51](../mail/mutations.md) attaches to them, and **both scopes of policy state above**,
and MUST NOT require a resynchronization from the provider. Serialized intents carry their own
version so that an intent enqueued by an older build remains executable after upgrade.

**An intent the executing build does not recognise MUST be quarantined and surfaced — never dropped, and
never guessed at.** This is the other half of forward-only migration. Refusing to open a newer database
prevents the common downgrade, but an intent can also be unrecognised without a schema change: FR-13's
list is closed today and D-40 has already extended it once, so a build that predates an intent type may
still meet one in a queue it can otherwise read. Dropping it silently loses a mutation the user watched
succeed, which is the single failure [mutations](../mail/mutations.md) says the queue exists to prevent;
executing a best guess at it is worse, because the guess is a write to the user's mail. Holding it,
showing it in the queue under FR-34, and letting a later build execute it is the only option that loses
nothing.

## Durability

The store runs in write-ahead-logging mode with a durable commit, satisfying NFR-16 in
[mutations](../mail/mutations.md). The store is authoritative; the [cache tier](cache-and-blobs.md) is not.

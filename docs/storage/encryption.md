# Encryption at rest

What is encrypted, what is not, and where the keys live.

**Owns:** D-22, D-42, D-43.

## Message data

Account databases, the blob store, the shared blob index, and the installation policy store MUST be
encrypted at rest, with keys wrapped by the operating system's credential store — Keychain on macOS,
Secret Service on Linux. The mechanism for each is D-42 and D-43 below; naming all four here matters
because the last two belong to no account and were previously covered by no rule written per account.

The threat this addresses is a stolen or shared device and casual filesystem access by other software on
the same machine, not a determined attacker with the user's session — see
[threat model](../security/threat-model.md) for what is and is not in scope.

## D-22 — Per-account database keys, convergent blob keys

**Chosen:** each account database is encrypted under its own per-account key. Each blob is encrypted under
a key derived from the blob's own content, and that blob key is wrapped per-account.
**Rejected:** encrypting the shared blob store under one device key; giving each account its own blob
store and abandoning cross-account deduplication.

**Why.** This resolves a direct contradiction between two requirements that were both stated as normative.
Keys were required to be **per-account**, so that [removing an account](../mail/accounts.md) destroys the
ability to read its data. The blob store was simultaneously required to be **shared and deduplicated
across accounts**, on the reasoning that the same attachment arrives repeatedly and is forwarded between
addresses. Two accounts cannot both read one blob under strictly per-account keys. One of the two had to
give, or a scheme had to reconcile them.

Deriving each blob's key from its content reconciles them. Two accounts holding identical bytes derive
identical keys independently, so the ciphertext deduplicates exactly as the plaintext would, while each
account holds only its own wrapped copy of that key. Removing an account destroys its wrapped keys, so its
blobs become unreadable through it even if a file survives in a backup — which is what the per-account
rule was protecting.

The content address is computed over **plaintext**, not ciphertext. Deduplication is otherwise impossible,
since two encryptions of the same bytes under different keys do not match.

**What it costs:** a wrapped key per blob per account, and two distinct disclosures that must not be
conflated.

The first is **content equality**: anyone who can read the store can tell that two accounts hold the same
attachment. That is not a new leak; it is exactly what deduplication means, and a design that
deduplicates has already accepted it.

The second is larger and was previously left unstated. Convergent derivation from plaintext means an
observer who *already holds a candidate file* can derive its key and its address and check whether the
store contains it — **confirmation of a known file**, which is the canonical weakness of convergent
encryption and is strictly stronger than equality between two opaque blobs. Equality tells an observer
that two unknown things match; confirmation tells them exactly what one of them is. D-43 below closes it.

**Contestable because:** the simpler answer is to drop cross-account deduplication and give each account
its own store. That removes the contradiction outright, makes FR-4's erasure trivially provable, and costs
only duplicate storage under a budget that is hard-capped anyway. If per-blob key wrapping proves to be
the complexity that breaks the store, that is the retreat.

## D-42 — Every database is encrypted at the page level, beneath the database engine

**Chosen:** a page-level encryption layer sitting underneath the database engine, so that every page
written to disk is encrypted and authenticated under the account key from D-22, and the engine above it
is unmodified.
**Rejected:** encrypting individual columns in the application; relying on whole-disk or home-directory
encryption supplied by the operating system.

**Why.** This closes a collision between two decisions that were each settled without reference to the
other. This document requires account databases encrypted at rest;
[D-21](data-model.md) names the engine as SQLite, which has no encryption of its own. Something had to
supply it, and the three candidates are not equivalent.

Application-level column encryption fails outright rather than merely costing something: the full-text
index [D-5](search.md) depends on cannot tokenize ciphertext, so an encrypted subject or body column is
unsearchable. Sift would have to keep a plaintext index of exactly the content the encryption exists to
protect, which is not a weaker version of the requirement but its inverse.

Whole-disk encryption fails the stated threat. This document names it precisely: a stolen or shared
device *and* casual filesystem access by other software on the same machine. Whole-disk encryption is
transparent to every process once the user logs in, so it does nothing about the second half — and the
second half is why per-account keys exist at all.

Page-level encryption leaves the index, the query planner and the write-ahead log working exactly as
D-21 and D-5 assume, because everything above the page layer sees plaintext. The write-ahead log MUST be
covered by the same layer; a journal written in the clear would defeat the whole arrangement while
looking correct in every test that inspects only the main file.

**The same layer covers the two installation-scoped stores.** The shared blob index and the installation
policy store are databases of the same engine holding no account's data, so D-43 supplies their key and
this decision supplies their mechanism — page-level, write-ahead log included, authenticated. Naming both
halves matters because D-43 settles *under what secret* and would otherwise leave *by what means* to
inference, which is the shape of gap that produced D-42 in the first place.

**What it costs:** a cryptographic layer beneath a C dependency, in a project that argues hard for Rust
on parsing paths. The mitigation D-21 already states applies here too — the database sees no
attacker-controlled bytes — but it does not extend to the crypto layer itself, where a bug is not a
parsing failure but the **total, silent loss of an account's store**. This layer needs the same review
posture as the sanitizer despite facing none of the same input.

**It also sits under every read, and no latency target has been re-checked against it.** This decision was
settled after [D-5](search.md) and NFR-5, and neither was revisited. NFR-5 asks for keystroke-to-results
under 100 ms at p95 over 500,000 messages, and under [D-6](data-model.md) that is a full-text query
against five separate account databases on every keystroke, merged in the
[presentation layer](../architecture/presentation-layer.md) — with every page any of them touches now
passing through this layer first.

A warm page cache above the crypto layer absorbs most of that, which is why this is a check to run rather
than an objection to the decision. Where it would show is the cold path: the first search after launch,
and the first keystroke after the L2 shed tier in [memory pressure](../runtime/memory-pressure.md) has
released database memory and shrunk the index cache. **NFR-5 MUST be measured with this layer in place**
rather than against a plaintext store, or the number it reports is not the number the product has.

**Contestable because:** the retreat is to encrypt only the blob store and leave account databases in the
clear. That is a real weakening rather than a simplification: envelopes are retained **indefinitely** by
[cache and blobs](cache-and-blobs.md) while bodies are evicted, so the unencrypted half would be the
permanent half — every sender, subject, and date the user has ever received, readable by anything running
as them. A reader who finds page-level encryption too heavy should argue for a different mechanism, not
for dropping the requirement.

## D-43 — Content addresses and blob keys are derived under a per-installation secret

**Chosen:** derive both the blob store's content address and D-22's convergent blob key with BLAKE3 in
**keyed** mode, under a per-installation secret held in the OS credential store. Encrypt the shared blob
index — and the installation policy store beside it — under that same secret.
**Rejected:** leaving addresses and keys as bare hashes of plaintext; giving each account its own index.

**Why.** Two loose ends, one fix.

The first is that the **shared blob index has no stated protection at all**. It is neither an account
database nor the blob store, so the rule at the top of this document reaches it by neither name — yet it
holds a hash, size, last-use time and reference count for every blob on the machine. Left in the clear it
is a manifest of the store's contents.

The [installation policy store](data-model.md) has the identical shape and is covered here for the
identical reason: it belongs to no account, so per-account keys cannot reach it, and it holds both a
profile of the user's networks and the installation-wide half of the allowlist that governs egress.
Confidentiality is the lesser half there — **authentication is the point**, because an attacker who can
rewrite an allowlist entry has turned filesystem write access into a standing instruction to fetch
remote content. That is the same conversion the integrity rule below names, reached from policy rather
than from content.

The second is the confirmation-of-a-known-file disclosure named in D-22's cost above, and it is not fixed
by encrypting the index alone: convergent derivation means the address and the key are both computable
from the plaintext by anyone who holds a candidate copy. Keying the derivation is what removes that. A
secret the observer does not have now sits between the plaintext and both values, so a candidate file no
longer yields anything to check against.

**Deduplication is unaffected**, which is what makes this nearly free. The blob store is local, so
convergence was only ever needed *within one installation* — two accounts on the same machine derive the
same key from the same bytes because they share the secret. Cross-machine convergence was never a
property this design had or wanted.

Per-account indexes were rejected for the reason [data model](data-model.md) already gives: a reference
count is a property of the store, not of any one account, and splitting it re-creates the
under-counting hazard the ordering rules there exist to avoid.

**This strengthens [D-23](cache-and-blobs.md) rather than retreating from it.** D-23 chose BLAKE3 partly
for its keyed derivation and used it to feed D-22; this uses the same primitive one step earlier. Had the
address been SHA-256, keying it would have meant bolting on a construction rather than selecting a mode.

**What it costs:** one more secret in the credential store, and its loss makes the entire blob store
unreadable — a hard-cap-bounded cache the provider can refill, but a visible and confusing loss all the
same. It MUST therefore be covered by the same key-identifier rule as everything else below, so that a
store orphaned by a lost secret is recognisable as such rather than merely corrupt.

**Recognising an orphaned store is not the same as handling it, and the gap between the two is not
benign.** The shared blob index holds the reference counts, and the index is encrypted under the same lost
secret — so an installation that cannot read it can neither enumerate the store, nor collect from it, nor
evict from it. NFR-14 in [cache and blobs](cache-and-blobs.md) is a hard cap enforced by eviction, and
eviction reads the index. An orphaned store is therefore not merely unreadable: it is unbounded disk that
the one requirement bounding disk can no longer reach.

**An orphaned store MUST therefore be discarded wholesale rather than retained.** Every blob under a key
identifier whose secret is gone is unrecoverable by construction, and this is a cache the provider
refills — the same reasoning D-22 above already accepts when it makes an account's blobs unreadable on
removal. Discarding costs a refetch of whatever the user opens next. Retaining costs a permanently
unaccountable directory that grows once and never shrinks.

**Contestable because:** it accepts a single per-installation secret whose compromise re-enables the
confirmation attack across every account at once, where per-account derivation would contain it — at the
cost of the cross-account deduplication D-22 went to some length to preserve. That trade is the whole
point of D-22 and is not worth reversing here, but it is where an attacker with the secret gets the most.

## Integrity, not only confidentiality

Encryption on both paths MUST be authenticated. A store that decrypts attacker-influenced bytes without
verifying them hands modified content to the [pipeline](../rendering/pipeline.md) as though Sift had
written it, which converts filesystem write access into content injection.

## Credentials are elsewhere

**OAuth refresh tokens and passwords MUST NEVER be stored in an account database, in any Sift-managed
file, in logs, or in crash dumps.** They live only in the OS credential store. See
[credentials](../security/credentials.md), which owns NFR-23.

This separation matters because the database is backed up, synced, copied between machines, and attached
to bug reports. Credentials must not travel with it.

## Key lifecycle

Account keys are per-account, so that [removing an account](../mail/accounts.md) destroys the ability to
read its data even if a file is later recovered from a backup. Key destruction MUST be part of the removal
path, not a consequence of it.

Every encrypted artefact MUST carry a key identifier, so that a key can be rotated without a flag day and
so that a file encrypted under a destroyed key is recognisable as such rather than merely unreadable.

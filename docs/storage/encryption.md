# Encryption at rest

What is encrypted, what is not, and where the keys live.

**Owns:** D-22, D-42, D-43, D-75, D-76.

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
clear. That is a real weakening rather than a simplification: envelopes are retained under a budget that
binds **far later** than the body budget does — see NFR-52 in [cache and blobs](cache-and-blobs.md) — so
the unencrypted half would be the long-lived half: every sender, subject, and date across years of mail,
readable by anything running as them. A reader who finds page-level encryption too heavy should argue for
a different mechanism, not for dropping the requirement.

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

## D-75 — The primitives are the platform's, behind one portable interface

**Chosen:** the authenticated cipher and the key-derivation function are the platform's own audited
implementations where the platform offers them, reached through a single interface that both platforms
implement identically. **The on-disk format is one format**, produced and consumed byte-for-byte the same
on macOS and Linux.
**Rejected:** a vendored Rust implementation used identically on both platforms; per-platform formats
chosen for each platform's convenience.

**Why the platform's primitives.** This is the most security-critical code in the product and the place
where a defect is total rather than partial: D-42 already concedes it is one of the four places unsafe
code is permitted, and
[decisions](../decisions.md) records that a bug here *"is total loss of an account, not a parse
failure"*. A platform crypto library is audited, maintained, frequently hardware-accelerated, and updated
by the operating system on a schedule Sift does not control and does not want to — which under
[D-33](../product/platforms-and-distribution.md), where Sift never updates itself, is the one dependency
whose fixes should not wait for App Review.

**Why one format nonetheless.** The primitives may come from two libraries; the bytes may not come in two
shapes. A per-platform format would make a store non-portable, which sounds acceptable for a product with
no sync until it reaches the two places it is not: the [fidelity](../product/reference-environment.md) and
scale corpora are fixtures shared by both platforms' test runs, and
[verification](../build/verification.md) requires both platforms to report against the same data. A
format that differs by platform makes a cross-platform fixture impossible and hides a class of bug in the
gap.

**What "one interface" obliges.** Both platforms MUST implement the same construction with the same
parameters, and the interface MUST be narrow enough that a reviewer can confirm they agree by reading it.
Where a platform library cannot supply the chosen construction, the fallback is a vendored implementation
of **that** construction — never a different one chosen because it was available.

**What it costs:** two implementations of one interface, and a class of bug — the two platforms
disagreeing — that a single vendored implementation would not have. That is precisely what the
byte-for-byte fixture check in [verification](../build/verification.md) exists to catch, and it is a test
that must exist from the first commit rather than after the first divergence.

**Contestable because:** a single vendored, audited Rust implementation is simpler, is portable by
construction, and removes the disagreement class entirely — at the cost of putting the product's most
security-critical code in the supply chain [workspace](../build/workspace.md) treats as a threat, and of
owning its patch cadence in a product that cannot self-update. The two arguments genuinely oppose each
other and this decision is the weaker of the two only if the platform libraries turn out to disagree in
practice.

## D-76 — What the page format has to nail down

**Chosen:** each page is independently sealed with an authenticated cipher; the seal's nonce is derived
from the page number **and a per-write counter** that is part of the file's state, never from the page
number alone and never randomly. Every file carries a header identifying it as a Sift store, its format
version, and the identifier of the key it is sealed under.
**Rejected:** deriving the nonce from the page number alone; a random nonce per write; a single seal over
the whole file.

**Why the nonce rule is the whole decision.** A page is rewritten every time its contents change, and an
authenticated cipher that seals two different plaintexts under the same key and nonce is not weakened —
it is broken, in a way that leaks plaintext and forges ciphertext. **Page number alone is exactly that
failure**, and it is the derivation an implementer reaches for first because it is stateless and obvious.
It does not fail a test. It does not corrupt a file. Nothing observable goes wrong, and the property the
encryption exists for is simply absent.

A random nonce avoids reuse only probabilistically, and the probability is a function of how many times
pages are rewritten over a store's life — which in an application that is resident for months and rewrites
the same hot pages continuously is the wrong side of the birthday bound to be relying on. A counter that
advances on every write is the construction that is correct by argument rather than by odds, and it costs
a monotonic value in the file's own state.

**Why per page rather than per file.** D-42 already argues this from the database engine's side: the
index, the query planner and the write-ahead log work unchanged because everything above the page layer
sees plaintext. A whole-file seal would mean
reading a store to open it, which is NFR-1's budget spent on a 2 GB file.

**What every file carries, and why each field is not optional.**

| Field | Why |
|---|---|
| A magic identifying a Sift store, and the format version | [D-32](data-model.md) refuses to open a newer schema rather than guessing at it; a file that cannot say what it is forces the same guess one layer lower, where the guess is a decryption attempt against arbitrary bytes |
| The key identifier | The key lifecycle below already requires it. Without it, a file encrypted under a destroyed key is *"merely unreadable"* rather than recognisable, which is the difference between FR-4 being provable and being believed |
| The write counter's high-water mark | It is what makes nonce derivation resumable across a restart. A counter that resets on open reuses every nonce it has already issued, which is the failure above arriving through the recovery path |

**Everything in the file is covered, including the journal and the write-ahead log.** D-42 already says
the write-ahead log MUST be covered — *"a journal written in the clear would defeat the whole arrangement
while looking correct in every test that inspects only the main file"* — and the same sentence now
applies to [D-74](data-model.md)'s account journal, which holds intents naming the user's mail.

**A file that fails to authenticate is discarded, never repaired.** This follows D-42's existing rule
that a failed verification *"MUST NOT be treated as a recoverable read error"*, and it is affordable
because [D-73](cache-and-blobs.md) makes the store discardable — the account resynchronizes, and
[D-74](data-model.md) keeps the journal in a separate file so pending triage survives the store being
thrown away. **The store is the half that may be discarded; a journal that fails to authenticate is
exported rather than discarded**, because nothing can reconstruct it.

**What it costs:** per-page overhead for the seal and its nonce, which reduces the usable bytes in every
page and therefore changes the store's size and its page-fault behaviour — a cost
[Q-10](../open-questions.md)'s rig must measure, since D-42 already records page decryption as
unvalidated against the search budget. And a counter that must be
durable, which is one more thing that must survive an abrupt termination correctly.

**Contestable because:** the counter is state, and state that must be monotonic across crashes is exactly
the kind of thing that is subtly wrong for a long time. A construction with a nonce large enough to be
chosen randomly without a birthday concern would remove it, at the cost of per-page overhead — and that
trade is a real one that should be re-examined once the page overhead is measured rather than assumed.

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
so that a file encrypted under a destroyed key is recognisable as such rather than merely unreadable. The
identifier is one of the header fields D-76 requires.

**The hierarchy has one shape, and the other two readings are excluded.** "Wrapped by the credential
store" admits three readings — a per-account key held directly as a credential item; a per-account key
wrapped by a key-encrypting key that is the credential item; or a per-account key **derived** from
D-43's per-installation secret and an account identifier. **The account key is held directly as its own
credential item**, and the other two are refused.

Derivation is refused because it makes the per-installation secret load-bearing for every account:
D-43 already requires a store orphaned by a lost secret to be *"discarded wholesale"*, and under
derivation that event silently extends from the blob store to every account database at once — turning a
cache loss into total loss, through a coupling no document states. A key-encrypting key is refused for a
smaller reason: it buys one rewrap-instead-of-many at rotation time, and costs a second secret whose
compromise is equivalent to compromising all of them, in a design that already has exactly one such
secret and treats it as a named risk.

**This is what makes FR-4 provable rather than believed.** [Account removal](../mail/accounts.md)
requires that erasure be *"verifiable by test"*. With the key held directly, removal deletes one
credential item and the account's ciphertext is unreadable by construction — a property a test can assert
by attempting to open the files afterwards. Under derivation the key is recomputable from a secret that
still exists, so removal would have to be proven by the absence of files rather than by the absence of a
key, which is a strictly weaker claim about a device that may hold backups.

### D-106 — One key per *file*, derived by role from the key its owner holds

**Chosen:** the credential item stays exactly as above — one account key, held directly — and the key a
file is actually sealed under is **derived from it by role**, one role per file. The key identifier in
each header is derived from the same owner by a separate context.
**Rejected:** sealing an account's two files under the account key itself.

**Why, and this corrects a defect rather than refining a preference.** D-76 derives its nonce as
`page number ‖ write counter`, and the counter is part of *the file's own state* — it lives in that
file's header and resumes above the persisted high-water mark. Within one file that is exactly right.

But [D-74](data-model.md) gives every account **two** files, and the paragraphs above put one key over
both. Each file independently issues counter 0 for its page 0, counter 1 for its page 1, and so on. Same
key, same nonce, different plaintext — which is the one thing AES-GCM must never be asked to do. It costs
confidentiality (identical keystream, so the two plaintexts are recoverable from each other) and
authenticity along with it. The additional authenticated data binds the key identifier, which under one
key is identical for both files, so it does not save it.

It would also not have been noticed. D-76's own text says of this class of mistake that it *"does not
fail a test, does not corrupt a file, and nothing observable goes wrong"*, and the test that walks the
counter space walks a single file's, so it passes either way.

Derivation by role closes it: identical `(page, counter)` pairs across the two files are harmless because
they are under different keys.

**What it does not change.** FR-4's proof is untouched, which is the property the paragraphs above chose
this hierarchy for. The account key is still the one credential item, derivation is still one-way, and
destroying that item still makes both files unreadable *by construction* rather than by a promise to
overwrite them. Nothing here reintroduces the rejected reading: the owner of a derived key is the account
key, never D-43's per-installation secret, so a lost secret still costs the blob store and not every
account database.

The same separation applies to the two installation-scoped files, whose owner **is** the per-installation
secret — and that also domain-separates a secret D-43 otherwise uses both as a BLAKE3 key and as key
material, with nothing between the two uses.

**The contexts are permanent.** Changing one makes every file under it unreadable, which is a key
destruction wearing the clothes of a refactor.

**What it costs:** one more derivation per file open, and a closed role set that every new sealed file
must be added to — an omission being a collision rather than a compile error, which is why the set is an
enumeration rather than a free-form string.

**Contestable because:** it adds a layer to a hierarchy whose whole argument above was that the simplest
of three readings is the right one. The answer is that this is not a fourth reading of "wrapped by the
credential store" — the credential item is unchanged and so is every property claimed for it — it is the
separation between *what is stored* and *what a given file is sealed under*, which the original text did
not distinguish because it was written before there were two files per account.

**Rotation is lazy, and bounded by the identifier.** A rotation installs a new key, and pages are
re-sealed under it as they are next written; both generations are readable while any page carries the old
identifier, and the old key is destroyed only when none does. Eager rewriting of a 2 GB store on a
schedule the user did not ask for is the alternative, and it is worse in a resident application. **The
per-installation secret of D-43 is the exception and does not rotate lazily**, because its rotation
re-derives every convergent blob address — the total cache invalidation
[platform baseline](../product/platform-baseline.md) already describes for a re-scoped installation. It
is therefore a discard-and-refill of the blob store, which [D-73](cache-and-blobs.md) makes affordable
and which MUST be stated as what it is rather than presented as a rotation like the others.

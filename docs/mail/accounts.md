# Accounts

Account lifecycle: adding, configuring, and removing.

**Owns:** D-89, FR-1, FR-3, FR-4.

## FR-1 — Multiple accounts, multiple providers

Sift MUST support adding accounts for Gmail, Microsoft 365 and Outlook.com, JMAP, and generic IMAP, with
multiple accounts per provider. Accounts are independent: [one database each](../storage/data-model.md),
independent sync cursors, independent credentials.

Authentication is covered in [credentials](../security/credentials.md) — including FR-2 and the OAuth
verification risk that gates Gmail support.

## FR-3 — Generic IMAP configuration

Generic IMAP accounts MUST support manual host, port, and TLS configuration, and MUST first attempt
autoconfiguration by discovery — service records, the conventional autoconfig host for the domain, and the
public provider database.

Discovery is an attempt, never a requirement. Failure to discover MUST fall through to manual entry
without an error state; many valid servers are undiscoverable.

**Discovery is the only place Sift contacts a host the user did not choose, and it MUST be disclosed and
skippable.** Querying the public provider database sends the domain of the address being added to a third
party, before that account exists and therefore before any provider connection is legitimate. NFR-22 in
[privacy](../security/privacy.md) names domains explicitly as correspondence metadata, so this step cannot
be treated as ordinary configuration traffic: the setup interface MUST say which lookups it will perform
before performing them, and MUST offer manual entry as a first-class path rather than as the fallback
after a failed probe. The service-record and per-domain autoconfig lookups reach the user's own mail
domain and are not subject to this; the public database is. It is carried as a row in the
[egress table](../security/privacy.md).

Capability declaration for a generic IMAP account is *probed*, not configured — see
[IMAP](providers/imap.md) and [provider model](provider-model.md).

## D-89 — An account has an identity of its own, and re-adding does not resume

**Chosen:** an account is identified by a value Sift assigns when it is added, independent of the address,
the provider, and any server-side identifier. Adding the same mailbox twice is permitted and **warned
about**; removing and re-adding produces a new account with no relationship to the old one.
**Rejected:** identifying an account by its address; identifying it by provider and address; silently
refusing a duplicate.

**Why not the address.** It is the obvious key and it changes: an alias becomes primary, a domain is
renamed, a work account migrates tenant. Keyed on the address, every one of those is the account
disappearing and a new one arriving — losing the watched folder set, the per-sender allowlist, the
per-folder notification rules, and every local identity a shell holds. Keyed on Sift's own value, they
are a display change.

**What the identity is for.** It names the account's two files under [D-74](../storage/data-model.md),
its credential items under [credentials](../security/credentials.md), and its rows in the shared blob
index. It is the value that must not change, which is why it is the one value the user cannot influence.

**Why a duplicate is warned about rather than refused.** Two accounts on one mailbox is a real
configuration — a second account with different scopes, or one being replaced while the other is kept —
and Sift cannot tell that case from a mistake. Refusing it silently would also be a lie in the common
case where the addresses merely *look* the same: [D-44](../storage/data-model.md) already establishes
that address comparison is corroboration rather than proof. So Sift says the mailbox appears to be
already configured and lets the user decide, which is the same posture FR-3 above takes toward discovery.

**Re-adding is a new account, and the user MUST be told before removal, not after.** Under
[D-6](../storage/data-model.md) removal deletes the account's files, so re-adding starts from nothing:
the backfill runs again, every local identity is new, and the watched set, allowlist and notification
rules are gone. That matters because **remove-and-re-add is what a user reaches for to fix a broken
account**, and it is the most destructive thing the interface offers. FR-4's confirmation MUST say what
is lost, in those terms — not that data will be erased, which is what the user wants, but that their
per-account decisions will be, which is not.

## FR-4 — Removal is erasure

Removing an account MUST provably erase its database, its blobs, and its credential entries.

"Provably" means the removal path is verifiable by test, not merely believed: after removal, no file
belonging to that account remains on disk and no credential entry remains in the OS store. Orphaned blobs
are a specific hazard, because the blob store is content-addressed and refcounted — see
[cache and blobs](../storage/cache-and-blobs.md). Removal MUST decrement refcounts and collect what falls
to zero, rather than leaving another account's identical attachment to imply retention of this one's.

Per-account database isolation is what makes this cheap; it is one of the reasons for
[D-6](../storage/data-model.md).

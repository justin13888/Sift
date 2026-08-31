# Accounts

Account lifecycle: adding, configuring, and removing.

**Owns:** FR-1, FR-3, FR-4.

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

## FR-4 — Removal is erasure

Removing an account MUST provably erase its database, its blobs, and its credential entries.

"Provably" means the removal path is verifiable by test, not merely believed: after removal, no file
belonging to that account remains on disk and no credential entry remains in the OS store. Orphaned blobs
are a specific hazard, because the blob store is content-addressed and refcounted — see
[cache and blobs](../storage/cache-and-blobs.md). Removal MUST decrement refcounts and collect what falls
to zero, rather than leaving another account's identical attachment to imply retention of this one's.

Per-account database isolation is what makes this cheap; it is one of the reasons for
[D-6](../storage/data-model.md).

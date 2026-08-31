# Scope

What Sift is, what it will never be, what it defers, and why each boundary is drawn where it is.

**Owns:** D-39.

## Product statement

Sift is a desktop mail client for **reading, searching, and triaging** mail across multiple accounts.
It is designed to be running all the time, so its idle resource cost ranks above feature breadth.

## In scope

- Read, search, and triage across multiple accounts and multiple providers. See
  [provider model](../mail/provider-model.md).
- Mutations limited to: archive, delete to trash, permanently delete, move, flag/star, mark read/unread,
  add/remove tag, and report junk or not-junk. See [mutations](../mail/mutations.md).
- Full-fidelity HTML message rendering with hostile-content isolation. See
  [rendering pipeline](../rendering/pipeline.md).
- Offline read of cached mail. See [cache and blobs](../storage/cache-and-blobs.md).

## Non-goals

These are not "later"; they are **excluded by design**.

| Excluded | Consequence if admitted |
|---|---|
| Sending, replying, forwarding, drafts, outbox | see below |
| Calendar, contacts, tasks — except contact *name resolution* for display | a second sync domain with its own delta model and conflict semantics |
| Server-side rule and filter management | provider-divergent rule languages, no shared abstraction |
| Mobile | a third platform family and a different resource model |

## The no-send constraint

There MUST be no SMTP, JMAP submission, or provider `sendMail` code path anywhere in any shipped binary.
This is a structural guarantee, not a product preference.

Removing send removes, in one stroke: MIME *composition*, address canonicalization for envelopes,
DKIM/SPF alignment on outbound, draft sync conflict resolution, and the entire "did it actually send"
reliability problem — a class of failure that dominates mail-client engineering effort.

The constraint degrades through requests that individually sound small. A reply box needs composition;
composition needs drafts; drafts need sync; sync needs conflict resolution. Any change that introduces
an outbound message path is a scope change requiring this document to be amended first.

Sift MAY link to the user's configured mail handler for a reply; it MUST NOT construct or transmit the
message itself.

## Deferred, with a defined insertion point

**This section is not the non-goals table, and the difference is load-bearing.** A non-goal is excluded
by design: admitting it invalidates something structural, and the table above records what. A *deferred*
item is one Sift does not do yet, whose later addition would not contradict any decision — recorded here
with **the seam it would enter through**, so that adding it later is an append rather than a redesign.

Anything entered here MUST name that seam. An item nobody can name a seam for is a non-goal in disguise,
and belongs in the table above with its consequence written out.

There is one such item today.

## D-39 — End-to-end encrypted and signed mail is deferred, not excluded

**Chosen:** Sift does not decrypt S/MIME or OpenPGP mail and does not verify their signatures. An
encrypted part renders as an explicit **"encrypted — Sift cannot read this"** state, with FR-9's raw
source view intact so nothing is hidden from the user.
**Rejected:** implementing either scheme now; rendering the armored block as though it were body text.

**Why.** Three subsystems, none of which exists yet. Private-key custody is a different threat model from
the one [encryption at rest](../storage/encryption.md) addresses: D-22's keys protect a cache the provider
could refill, whereas a mail private key is irreplaceable and its loss is permanent, which changes backup,
rotation and export from conveniences into requirements. It needs a key-management interface, in a product
whose entire UI budget is [two native shells](../architecture/ui-shell.md). And signature verification
would feed [D-11](../rendering/sender-origin.md)'s synthetic origin, which currently derives from transport
authentication alone — a third source of attestation with its own precedence rules.

Against that, the fraction of mail affected is small, and a user who receives encrypted mail today already
has a tool that reads it.

**This is deferred rather than excluded because nothing above forbids it.** Unlike sending, it introduces
no outbound message path and violates no constraint in this document.

**The seam.** It enters as a decryption stage between stages 1 and 2 of
[the rendering pipeline](../rendering/pipeline.md) — after MIME parse, before part selection, so that
decrypted content passes through sanitization, blocking and rewriting exactly as plaintext mail does and
gains no exemption from I1–I10. Storage is a private-key store alongside
[credentials](../security/credentials.md), never in an account database. Verification results extend the
authentication record [FR-28](../rendering/sender-origin.md) already stores per message.

**What it costs to defer:** users who need it are not served, and they are disproportionately the
privacy-motivated users this product otherwise appeals to.

**Contestable because:** the seam is cheap to describe and expensive to honour. A decryption stage that
must not leak plaintext into the blob store, the search index, or a crash report touches
[cache and blobs](../storage/cache-and-blobs.md), [search](../storage/search.md) and
[D-35](../security/privacy.md) — and each of those is a decision made without this stage in mind. If the
feature is wanted at all, wanting it *early* is cheaper than wanting it late.

## Related

- [Platforms and distribution](platforms-and-distribution.md) — which operating systems, in what order
- [Roadmap](roadmap.md) — the order in which in-scope work is attempted
- [Requirements index](../requirements.md)

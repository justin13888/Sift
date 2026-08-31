# Scope

What Sift is, what it will never be, what it defers, and why each boundary is drawn where it is.

**Owns:** D-39, FR-41.

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
| Registering as the system mail handler, or for `mailto:` | FR-41's handoff would resolve to Sift, which then needs a compose surface — the first step of the degradation this document describes below |
| Honouring read receipts and disposition-notification requests | an outbound message path. The *request* is still surfaced; see below |
| Setting the answered flag after a handoff | Sift asserting an outbound message it did not send and cannot verify, which FR-41 already forbids in a different position |
| Creating, renaming, or deleting folders | a second mutation domain with its own capability rows, its own conflict semantics, and it breaks FR-5's semantic resolution the moment a user renames a special-use folder |
| Emptying the trash | a bulk permanent delete, which [mutations](../mail/mutations.md) excludes for the reason it gives: a wrong query and a wrong click composing into unrecoverable loss |
| Snooze, pin, mute-thread, and other local-only per-message state | local state the provider cannot refill inverts the "cache is not the source of truth" framing that permits aggressive shedding, and makes NFR-18 recovery and D-32's resync lossy |
| Translated interface strings in the first release | none, given [D-56](../architecture/presentation-layer.md). Without that rule this row would be a non-goal that quietly becomes permanent |

## The no-send constraint

There MUST be no SMTP, JMAP submission, or provider `sendMail` code path anywhere in any shipped binary.
This is a structural guarantee, not a product preference.

Removing send removes, in one stroke: MIME *composition*, address canonicalization for envelopes,
DKIM/SPF alignment on outbound, draft sync conflict resolution, and the entire "did it actually send"
reliability problem — a class of failure that dominates mail-client engineering effort.

The constraint degrades through requests that individually sound small. A reply box needs composition;
composition needs drafts; drafts need sync; sync needs conflict resolution. Any change that introduces
an outbound message path is a scope change requiring this document to be amended first.

## FR-41 — The reply handoff is a requirement, not a permission

**FR-41.** Sift MUST offer reply, reply-all and forward actions that hand off to the user's configured
mail handler, and MUST NOT construct or transmit the message itself.

This was previously a MAY, and the upgrade is the point. The no-send constraint has a consequence the
section above states nowhere: **Sift alone cannot answer mail, so a user who installs only Sift has a mail
client they cannot reply from.** That is this product's largest adoption objection, and leaving its only
remedy optional made the answer to it an optional feature.

What "hands off" means needs specifying, because the failure cases are where a handoff stops being honest.

- The action MUST pass the message's **identification** — recipients, subject, and the identifier being
  replied to — to the platform's handler, and MUST NOT depend on Sift knowing what the handler then does.
- It MAY additionally pass quoted text of the original. **That is the closest this comes to composition
  and is deliberately capped there:** no MIME construction, no attachment handling, no encoding or
  alignment decisions, and no code path that could grow into one. A reader checking the no-send constraint
  should check exactly this line.
- Where **no handler is configured**, Sift MUST say so and MUST NOT present a reply affordance that
  silently does nothing. This is the rule [mutations](../mail/mutations.md) already applies to junk
  reporting on an account that does not support it: absent, not approximated.
- The reply is composed and sent by another application, so it belongs to that application's record of
  what was sent. Sift MUST NOT imply otherwise, and in particular MUST NOT show a handed-off message as
  sent or as part of the thread until the provider returns it through the ordinary
  [sync](../mail/sync-engine.md) path like any other message.

**This is not a deferred item under the section below.** A handoff introduces no outbound message path,
so there is nothing here to defer — it is the constraint being made liveable rather than being softened.

### Four consequences of no-send, stated rather than discovered

The table above excludes four things that a reader will otherwise assume are oversights. Each is a real
loss and each is written down here so that the cost of the no-send constraint is visible in one place.

**Sift must not be the system's mail handler.** FR-41 hands off to "the user's configured mail handler",
and if that handler is Sift the handoff resolves to itself. So Sift MUST NOT register for the role or for
the `mailto:` scheme, and a `mailto:` link in a message body resolves through the platform's handler like
any other — where the resolution names Sift, that is the *no handler configured* case FR-41 already
requires to be stated rather than silently doing nothing.

**A user cannot tell which messages they have replied to.** The answered flag is real, syncing,
per-message state on all four providers, and Sift does not set it, because doing so would assert an
outbound message it did not send and cannot verify. A reply composed in a browser will never set it
either. This is the second-largest honest cost of the no-send constraint, after the inability to reply at
all, and it is the one most likely to be reported as a bug.

**A read receipt is refused but not concealed.** Sift cannot honour a disposition-notification request
without sending. It does surface that one was asked for, beside the blocked-remote-content indicator,
because it is the non-image half of the same attempt to learn that a message was opened — and this
product's position everywhere else is to report what a message tried to do rather than absorb it silently.

**Unsubscribing is a link, not an action Sift takes.** The one-click form is an HTTP request and the older
form is a `mailto:`; the second is excluded above and the first would be an egress path outside the
[resource broker](../architecture/resource-broker.md), in a table that claims to be complete — carrying a
per-recipient token, which is the same signal FR-29 treats as evidence of tracking. Sift therefore
displays the destination and opens it in the system browser on confirmation, and issues nothing itself.
That is [FR-42](../rendering/link-handling.md).

## Deferred, with a defined insertion point

**This section is not the non-goals table, and the difference is load-bearing.** A non-goal is excluded
by design: admitting it invalidates something structural, and the table above records what. A *deferred*
item is one Sift does not do yet, whose later addition would not contradict any decision — recorded here
with **the seam it would enter through**, so that adding it later is an append rather than a redesign.

Anything entered here MUST name that seam. An item nobody can name a seam for is a non-goal in disguise,
and belongs in the table above with its consequence written out.

Three items today. Two are small enough to state in a line each, and the third has its own section.

**Printing a message.** The seam is an additional consumer of stage 7 of
[the pipeline](../rendering/pipeline.md), with a print stylesheet appended to the base stylesheet that
[platforms and distribution](platforms-and-distribution.md) already requires. Note that print media
queries are resolved by [D-27](../rendering/dark-mode.md)'s cascade, so the machinery is present.

**Saving a message as a file.** The seam is a read from the [blob store](../storage/cache-and-blobs.md)
of bytes Sift already holds, written under NFR-53's path rules like an attachment. No new machinery, and
no new egress.

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

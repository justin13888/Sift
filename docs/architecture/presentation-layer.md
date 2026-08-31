# Presentation layer

The shared Rust layer that makes two native shells affordable.

**Owns:** D-4, D-18, D-41, D-55, D-56, D-99, D-100, FR-7, FR-40, NFR-51, NFR-54.

## Purpose

The presentation layer turns "two UI codebases" into "two view layers over one UI codebase". It sits
above the store and below the shells, and it owns every decision a shell would otherwise have to make for
itself:

- list windowing and paging
- selection state and multi-selection semantics — see below
- sort, filter, and grouping
- date, size, and address formatting — locale-aware under NFR-51
- contact name resolution for display — D-41
- thread collapsing
- search result assembly and merge — see [search](../storage/search.md)
- capability-driven affordance resolution — see [provider model](../mail/provider-model.md)

It exposes a platform-neutral **view model plus command protocol** across the
[shell boundary](shell-boundary.md). The shells bind to view models, lay them out, and forward events.
They MUST NOT reimplement any of the above.

The layer is resident for the life of the process; the shells are not. A shell is created when a window
opens and destroyed when it closes, so **the presentation layer MUST hold no state that only a live shell
can reconstruct** — see [process model](process-model.md).

## Designing it before the second consumer exists

This layer MUST be designed as platform-neutral from the first commit, while only the macOS shell exists.
The failure mode is well understood: with one consumer, the layer's API silently acquires that consumer's
assumptions — its threading model, its cell-reuse protocol, its string conventions — and the second shell
then requires either a rewrite or an adapter that undoes the benefit.

Concretely, the layer MUST NOT expose types, lifecycles, or callback shapes that mirror a specific
toolkit's widget model.

## D-4 — Unified inbox in v1

**Chosen:** a cross-account unified inbox is in scope for the first release.
**Rejected:** per-account views only.

**Why.** It is the primary reason a user runs a multi-account client at all; without it, the product is
several single-account clients sharing a window.

**What it costs.** [One database per account](../storage/data-model.md) means a unified view cannot be a
single query. Cross-account paging and sort must be assembled in memory here, by merging per-account
result streams — and correct paging over a merged, independently-mutating set of sources is genuinely
hard, not merely tedious.

**Contestable because:** cutting it simplifies this layer substantially. It is scheduled late — see
[roadmap](../product/roadmap.md) — precisely so it can be cut without stranding work.

**FR-7.** A unified inbox across accounts. Threads MUST NOT span accounts, see
[threading](../mail/threading.md).

### The unified inbox shows cross-account duplicates

This is a decision rather than an oversight, and it is the most visible day-one consequence of three
decisions made elsewhere, so it is stated here where the user meets it.

A message delivered to two of the user's addresses — a mailing list is the ordinary case, not the exotic
one — is two messages in two accounts with two local identities. [Threading](../mail/threading.md) forbids
merging threads across accounts, and [D-44](../storage/data-model.md) scopes every identity join to within
a single account. Nothing in the design can collapse the pair. So in the flagship view of a multi-account
client the user sees both copies, and archiving one does nothing to the other.

The alternative was rejected in those documents rather than this one: a cross-account join would have to
key on the internet message identifier, which [R-5](../open-questions.md) says is not reliably unique, and
a false join is unrecoverable under FR-13 because the next archive or delete reaches mail the user never
saw. Duplicate display is the failure that stays visible and costs nothing. It is chosen.

What this layer owes the user is that the duplication is **legible rather than mysterious**. Where the
same message is present in more than one account, the merged view MUST mark the copies as such rather than
presenting them as unrelated arrivals. This is [FR-12](../storage/cache-and-blobs.md)'s rule about
distinguishing "not cached" from "not available" applied to a different confusion: the user is entitled to
know which of two odd-looking states they are in.

**Marking is not joining.** The two messages stay two entities, a mutation applies only to the one it was
issued against, and the marking is a display property computed at merge time by comparing the fallback
identity digests D-44 already stores. Nothing durable is written, and no candidate crosses an account
boundary.

## Selection

This layer owns selection, and owning it means stating three things that decide behaviour elsewhere.

**Opening a folder selects nothing.** Auto-selecting the first row renders a body and, under
[D-52](../mail/mutations.md), starts a dwell that will mark read a message the user never chose — so
walking through folders would silently consume unread mail. It is the only choice that cannot do that.

**Selection is keyed on local identity**, which [view protocol](view-protocol.md) guarantees is stable for
as long as the message exists. Keying on a row index is what makes a keyboard archive land on a different
message than the highlighted one when a delivery arrives between the keystroke and its handling, and that
is a data-loss class of bug rather than a visual one.

**A selected message that leaves the result set clears the selection** rather than sliding to a neighbour.
Sliding means the next keystroke acts on a message the user has not looked at, which is the same failure
one step removed.

## D-55 — The list is ordered by when the server received the message

**Chosen:** the **server-assigned received time** is the authoritative sort key, with local identity as a
stable tiebreak. The `Date` header is what the reader displays and is never what the list is ordered by.
**Rejected:** ordering on the `Date` header; ordering on received time with no defined tiebreak.

**Why the header cannot order the list.** It is written by the sender, and the sender is the adversary the
[threat model](../security/threat-model.md) is built around. A message dated ten years in the future pins
itself to the top of the inbox permanently, on every device, and no triage action removes it because
nothing about it is wrong except a header nobody validates. That is a defect a sender can cause on
purpose, at no cost, and it is one this documentation set would otherwise have shipped without noticing —
the header is not in the attacker-controlled-input table's list of things that reach a decision.

Received time is not attacker-controlled: every provider Sift targets assigns one, and it is the order the
user has already seen in that provider's own web interface, which is the same argument
[threading](../mail/threading.md) uses for preferring provider conversation identifiers.

**Why the tiebreak is load-bearing rather than a detail.** D-4 above merges per-account result
streams in memory, because [D-6](../storage/data-model.md) forbids cross-account SQL. A merge needs a
**total** order: with equal keys and no tiebreak, two accounts' rows may interleave differently on
successive merges, so paging can duplicate or skip a row, and D-18 below cannot state where a row
moved because there is no stable answer. Received times collide routinely — bulk mail delivered in the
same second is the common case, not the exotic one.

**One consequence worth stating rather than discovering.** NFR-51 makes collation locale-aware, and any
sort that falls back to a text comparison must use **the same comparator** in each account's own query and
in the in-memory merge. A merge that compares differently from the queries feeding it produces a wrong
sequence while every per-account result is individually correct — which is the hardest kind of ordering
bug to see.

**What it costs:** an indexed column per account database that is not the header the user sees, and a
reader that can show a date differing from the row's position in the list. That divergence is real and is
the honest one: the header says when the sender says they wrote it, and the list says when it arrived.

**Contestable because:** for the overwhelming majority of mail the two agree, so this buys correctness in
a case most users never meet, at the price of a second timestamp in the schema and a permanent small
inconsistency between the list and the reader. A reader who thinks that is the wrong trade should argue
for displaying received time as well, not for ordering on the header.

## D-18 — View models are observed, and observation is cancellable

**Chosen:** the shells register observers over a declared window of a result set and receive change
notifications against it; every outstanding request carries a cancellation handle.
**Rejected:** the shells polling for changes; the layer pushing whole result sets on every change.

**Why.** A mail client's list changes underneath the user constantly — delivery, sync, and optimistic
mutation all mutate a visible window. Polling would reintroduce the periodic wakeups that
[scheduling](../runtime/scheduling.md) exists to eliminate, and it cannot meet NFR-7's 16 ms feedback.
Pushing whole sets defeats NFR-6, because a native list view needs to know *which* rows moved to animate
and reuse cells correctly rather than reloading.

**Cancellation is not an optimization here.** A fast scroll supersedes window requests faster than they
can be served, and a search supersedes a query on every keystroke under NFR-5. Without cancellation the
layer performs work for a window nobody is looking at any more, against a merged multi-account stream
where that work is expensive — see D-4 above.

**What it costs:** change notifications must be expressed as ordered insert, delete and move operations
against a known window, and the layer must guarantee a shell that applies them in order arrives at the
same state. That is materially harder than replacing a list. The index space those operations are
expressed in, and the threading and cancellation rules they are delivered under, are
[D-48](view-protocol.md).

**Contestable because:** it puts diff computation in the layer for the benefit of toolkit APIs that
consume diffs. If both shells end up reloading anyway, this machinery is unearned.

## D-41, FR-40 — Contact names come from the platform, read-only

**FR-40.** Where a message's sender or recipient corresponds to a contact the user already has, Sift
SHOULD display that contact's name in place of, or alongside, the address.

**Chosen:** resolve display names in strict order — the display name the message itself carries, then the
**platform's own contact store**, read-only and on demand, behind a bounded memoization cache like any
other (see [memory pressure](../runtime/memory-pressure.md)). Sift stores no contact data of its own.
**Rejected:** synchronizing a provider's contacts API; building an address book by observing mail.

**Why.** [Scope](../product/scope.md) excludes contacts as a sync domain but carves out *name resolution
for display*, and that carve-out only stays cheap if it introduces neither a second delta model nor a
second store. The platform store is where the user's contacts already are, it is already reconciled
across their devices by machinery that is not Sift's, and reading it costs one lookup and no bytes on the
network.

Synchronizing a provider's contacts API is precisely the excluded second sync domain: its own cursors,
its own conflict semantics, its own migrations. Deriving an address book from observed mail is worse than
it sounds — it is a durable record of who the user corresponds with, built without being asked, in a
product whose [privacy posture](../security/privacy.md) treats correspondence metadata as sensitive
enough to keep out of telemetry entirely. Sift should not construct locally what it refuses to transmit.

**Resolution is display-only and never authoritative.** A resolved name MUST NOT feed the per-sender
allowlist, the [synthetic origin](../rendering/sender-origin.md), or any blocking decision — those key on
attested identity under D-11, and a contact entry is user-editable data that an attacker can influence by
sending mail. A friendly name next to a spoofed address must not make the address any more trusted.

**What it costs:** a platform permission on both operating systems, and a feature that is simply absent
where the permission is refused. Absence is the correct behaviour there — the address is still shown.

**Contestable because:** users whose contacts live only in their mail provider get nothing, which on
Microsoft 365 in particular is the common case rather than the edge one. If that turns out to be most
users, the provider contacts API returns as a question, and the answer would then have to face the second
sync domain honestly rather than through this back door.

Whether Flatpak's portal supplies this at all is [an open question](../open-questions.md), of the same
shape as the network and credential portal risks in
[platforms and distribution](../product/platforms-and-distribution.md).

## NFR-51 — Locale awareness is designed in, not retrofitted

**Every formatted value this layer produces MUST be locale-aware from the first commit** — dates and
relative times, sizes, numbers, address and name ordering, and the collation used for sorting.

This is the same argument this document already makes about the second shell, applied to a second axis.
Formatting that assumes one locale does not fail loudly; it produces plausible output that is wrong for
most of the world, and correcting it later changes the type of every formatted value crossing the
[C ABI](shell-boundary.md) at once — precisely the boundary D-17 says must stay narrow and stable.

Name ordering deserves naming: it is not a string concatenation, and getting it wrong is a way of
addressing a user incorrectly rather than merely formatting a value oddly.

Translation of interface strings is a separate concern belonging to each shell. This requirement is about
the layer beneath them not foreclosing it.

## D-99 — Multi-selection, the query selection, and what a mixed selection can do

**Chosen:** selection is a set keyed on local identity, per window, with an anchor for range extension; it
survives a sort or grouping change and is cleared by a folder, account or search change. A selection over
search results MAY instead be a **query selection** — the query and a count, never an enumerated list.
Affordances over a selection spanning accounts resolve to the **intersection** of those accounts'
capabilities.
**Rejected:** enumerating a large result selection; taking the union of capabilities; refusing a
cross-account selection.

**Why this needed writing.** This layer's opening list promises *"selection state and multi-selection
semantics — see below"*, and every rule below it is about a single selection: opening a folder selects
nothing, selection keys on identity, a departed message clears it. The multi-selection half was promised
and never delivered, in a layer both shells bind to.

**Anchor and extension, and what survives.** A range extends from an anchor set by the last unextended
selection, which is the behaviour both toolkits' users expect and neither toolkit decides. **A sort or
grouping change preserves the selection** because it is keyed on identity rather than position — the same
argument the single-selection rule already makes about row indices. **A folder, account or search change
clears it**, because a selection carried across a scope change is a set of messages the user can no
longer see, and the next keystroke would act on it.

**The query selection is a different kind, and the ABI has to know it.**
[FR-17](../mail/mutations.md) requires bulk operations over *"selections and search results"*, and a
result set at the 500,000 messages [NFR-5](../storage/search.md) is measured over is not something to
enumerate — not across the boundary, not in memory, and not into five hundred thousand rows of
[D-85](../mail/mutations.md) queue. So "everything matching this query" crosses as the query and a count,
and is expanded per account at flush time, inside the batching
[FR-17](../mail/mutations.md) already requires.

**It is expanded once, and what it expanded to is recorded.** A query re-evaluated at flush time would
act on a different set than the user saw — mail arrives between the gesture and the flush — so expansion
happens when the gesture is made, and the resulting identities are what the intents and the
[D-85](../mail/mutations.md) undo group carry. The count shown to the user is therefore the count acted
on, which is the only version of this a confirmation dialog can honestly state.

**A mixed selection resolves to the intersection.** In the unified inbox (D-4 above) a
selection can span accounts with different declared capabilities, and three answers were available. The
union offers an action that will fail for some of the selection, which
[provider model](../mail/provider-model.md) forbids in principle — an affordance that is absent when
unsupported cannot be present when unsupported for half the selection. Refusing cross-account selection
makes the flagship view untriageable in bulk, which is what it exists for. **The intersection is the only
answer where every offered action applies to everything selected**, which is the property a user assumes
without being told.

**What it costs:** a second selection kind on the boundary, and an intersection that silently shrinks the
available actions as a selection widens — a user who adds one IMAP message to a Gmail selection loses the
tag action with nothing saying why.

**Contestable because:** that last effect is genuinely confusing, and showing the union with per-message
failure afterwards is how several mail clients behave. It is rejected because the failures arrive later,
individually, as [FR-16](../mail/mutations.md) conflicts the user cannot connect to the gesture that
caused them.

## D-100 — Bidirectional text is isolated, and normalization precedes truncation

**Chosen:** bidirectional control characters are **isolated**, not stripped; normalization runs **before**
[L-16](../limits.md)'s truncation; and the normalized form is the form
[FR-19](../storage/search.md) indexes.
**Rejected:** stripping bidi controls; truncating first; normalizing for display only.

**Why isolate rather than strip.** NFR-54 below says *"stripped or isolated"*, and those are two different
products. Stripping removes the controls that make legitimate Arabic and Hebrew subjects read correctly,
so a defence against spoofing becomes a bug for every user who receives mail in those languages —
in a requirement that sits beside NFR-51's locale-awareness-from-the-first-commit. **Isolation neutralizes
the attack without altering the text**: the run cannot escape its own bounds to reorder the chrome around
it, and inside those bounds it renders as its author intended. That is what
[link handling](../rendering/link-handling.md) is reaching for when it warns that a naive treatment *"is
worse"*, applied to headers rather than to URLs.

**Why normalization must precede truncation.** L-16 bounds a snippet and NFR-54 bounds display lengths, and
if truncation runs first it can cut a value in the middle of an isolate — producing an unbalanced control
sequence, which is precisely the state isolation exists to prevent, manufactured by Sift's own bound. It
can also split a grapheme. Normalizing first and truncating the normalized form at a grapheme boundary is
the only order in which both properties hold, and it is the order an implementer optimizing for "bound it
early, cheaply" would get backwards.

**Why the indexed form is the normalized form.** [D-81](../storage/search.md) already pins the index's
normalization form to this requirement's. Stating the direction here closes the loop: what a user sees is
what is indexed, so searching for exactly the string on screen finds it. The alternative fails
invisibly — two forms that are visually identical and compare unequal.

**What it costs:** isolation is more work than deletion and produces text a naive downstream consumer can
still mishandle, so the property holds only as far as the boundary. Beyond it, the shells render what they
are given.

**Contestable because:** isolating leaves attacker-chosen control characters in the string, and a reviewer
looking for the safest possible answer would strip. The defence is that safety measured only against the
attack ignores the users the attack is not about, and this set already refuses that trade in
[dark mode](../rendering/dark-mode.md) and in [I9](../rendering/sanitizer-invariants.md)'s refusal to
invent text.

## NFR-54 — Untrusted text is normalized once, here

**NFR-54.** Every attacker-controlled string this layer emits — sender and recipient display names,
subject, snippet, folder and tag names, attachment names — MUST be normalized before it crosses the
boundary: bidirectional control characters **isolated** under D-100 above, other control characters
removed,
internationalized domains rendered under the same rules as a URL, and length bounded. Where a display name
is shown, the address MUST be shown alongside it and MUST NOT be replaced by it.

**This closes a hole the set already pointed at and did not fill.** NFR-28 in
[the pipeline](../rendering/pipeline.md) says bidirectional text in headers is "additionally a *security*
concern, not only a correctness one" and refers the reader to
[link handling](../rendering/link-handling.md) — which strips bidi overrides and decodes punycode **for
URLs only**. There is no rule for headers anywhere. The requirement gestures at a defence that does not
exist.

The attack is the one link-handling.md already defends against, moved one surface over. A right-to-left
override in a subject reorders what the list row appears to say. A display name that *is* an address —
`security@bank.example <attacker@evil.tld>` — reads as the sender in every client that shows the name and
hides the address. Neither touches the body view, so none of the sanitizer's ten invariants sees it, and
both are rendered by the native list, the reader chrome, the notification banner and the tray.

**Once, here, is the whole point.** This layer is the single place both shells receive text from, so one
normalization serves both and neither shell can forget. Doing it per call site means doing it in two
languages across two toolkits, and the one that is missed will be a surface nobody thought of — the
notification banner, most likely, which is seen by a user who has no window open and no context.

It is stated as a requirement rather than left to review for the reason NFR-51 gives about locale: it does
not fail loudly. A subject that renders backwards looks like a subject, and nothing crashes.

## D-56 — The layer returns states, not sentences

**Chosen:** no user-visible string crosses the [shell boundary](shell-boundary.md). This layer emits
identified, parameterized states; each shell renders and translates them.
**Rejected:** returning display strings from the layer, translated or otherwise.

**Why this is a boundary rule rather than a translation feature.** NFR-51 above makes every *formatted
value* locale-aware from the first commit, and then says translating interface strings is "a separate
concern belonging to each shell". Read together those leave a gap, because this layer demonstrably
produces user-visible prose: [D-38](../mail/mutations.md)'s notice naming what did not take effect,
NFR-29's explanation of why an account is degraded, [FR-12](../storage/cache-and-blobs.md)'s distinction
between not-cached and not-available, [FR-33](../runtime/observability.md)'s blocking reasons,
[FR-5](../mail/provider-model.md)'s prompt when no special-use folder resolves, and every condition in
[failure model](../runtime/failure-model.md).

Something has to render those. If it is this layer, then the layer has a locale, the C ABI carries prose,
and NFR-51's own warning applies — correcting it later "changes the type of every formatted value crossing
the boundary at once". If it is the shells, the boundary stays narrow and the question of which languages
ship becomes answerable at any time without touching it.

**It is also what keeps the two shells honest.** A state either has a rendering in both shells or it has
none; a string returned from the layer would be identical in both by construction, which sounds like
consistency and is actually the layer quietly deciding presentation — the thing
[shell boundary](shell-boundary.md) says the ABI must not become "the poorer of two interfaces".

**The states are enumerated in the [state register](state-register.md), which also draws the line
between this rule and NFR-54.** Read side by side those two look contradictory — nothing user-visible
crosses, and yet subjects and display names must be normalized *before* they cross. The reconciliation is
authorship: a **content value** came from the user's mail and crosses as data, normalized and never
translated; **chrome prose** is a sentence Sift wrote and never crosses at all. [D-68](state-register.md)
owns that distinction and the parameter rule that follows from it.

**What it costs:** every condition, error and explanation needs an identifier and a parameter list, and
adding one means touching both shells rather than one layer. That cost is the mechanism: a state nobody
has rendered is visibly missing, where an untranslated string is not.

**Contestable because:** it front-loads work for a product that ships in one language, and a small team
may reasonably prefer English strings from the layer until a second locale is real. The answer is that the
second consumer arrives before the second locale does — the Linux shell — and this is the same argument
this document already makes about designing the layer before its second consumer exists.

## Optimistic state

The presentation layer is where optimistic mutation results are reflected before the network confirms
them, and where a server-side disagreement is reconciled. The reconciliation policy is
[D-38](../mail/mutations.md) and MUST be implemented here rather than per-shell, so the two platforms
cannot diverge in behaviour.

# Network conditions

**Owns:** D-14, D-58, FR-35, FR-36, NFR-30 through NFR-39 (NFR-36 struck — see below).

## D-14 — Build a small network-conditions abstraction

**Chosen:** a purpose-built, tightly-scoped internal crate presenting one condition type and one watch
stream, with per-platform backends.
**Rejected:** an existing cross-platform crate.

**Why.** No unified crate exists. Each platform has a good native answer — a system path monitor on macOS,
NetworkManager over D-Bus on Linux — and they are entirely disjoint in shape. The abstraction over them is
small; this is roughly a file, not a project, and the alternative is per-platform conditionals scattered
through the sync engine.

What the abstraction reports: reachability (offline, captive portal, limited, online); link class (wired,
wifi, cellular, tethered, VPN, unknown); metered as a three-valued flag; a constrained-mode flag; and a
stable network identity for per-network overrides.

**When nothing resolves, report unknown. Never guess "not metered."** The cost of a wrong optimistic guess
is the user's data allowance; the cost of a wrong pessimistic guess is a delayed prefetch.

## FR-35 — The override is what makes this reliable

**Detection will be wrong sometimes** — tethered Ethernet, a corporate VPN over cellular, a mobile hotspot
appearing as wifi. A persisted **per-network user override that always wins** is what converts unreliable
detection into reliable behaviour, and it MUST be designed in from the start rather than bolted on when
detection is found wanting.

## Policy tiers

| Tier | Trigger | Behaviour |
|---|---|---|
| **Unrestricted** | wired or wifi, definitely unmetered | full body and inline-image prefetch, attachment prefetch under NFR-39's ceiling, filter-list updates |
| **Conservative** | metered unknown or guessed, or cellular | envelopes only; bodies on demand; no image prefetch; no filter-list updates; mutations flush normally |
| **Minimal** | metered, or constrained mode | inbox envelopes only; longest poll interval; mutations flush — they are bytes; everything else deferred |
| **Offline — portal** | captive portal | queue everything; one portal probe per 60 seconds; no connection churn |
| **Offline — no path** | offline, airplane mode, or system sleep | queue everything; **zero connection attempts of any kind, including the portal probe**, until the path returns |
| **Paused** | the user pausing sync under FR-22, or cumulative usage reaching FR-36's cap | no delta, no push, no prefetch, no list updates; connections torn down under NFR-33; **mutations still flush** |

Mutations flush in every tier above offline, **including Paused**. They are tiny, and a triage action that
does not take effect because the user is on cellular is a broken product.

## D-58 — Pause is a policy tier, not a second mechanism

**Chosen:** pause is the sixth row of the table above, and everything that pauses sync resolves to it.
**Rejected:** a separate pause mechanism beside the tiers; leaving FR-22's pause and FR-36's cap to be
implemented independently.

**Why this had to be settled rather than left.** Two requirements use the word *pause* and neither defines
it. [FR-22](../architecture/ui-shell.md) puts "pause sync" on the always-on surface and ships in **P1**;
FR-36 below says the data cap "pauses sync" and ships in **P4**. Three phases apart, one word, and no
shared definition — so the default outcome was two behaviours that differ in ways nobody chose, on a
surface where the difference reads as a bug.

**The contradiction it resolves is in this document.** FR-36's cap pausing sync sat three sections below
the sentence saying mutations flush in every tier above offline. Read together they point opposite ways,
and the resolution is the one this document already argued for: **mutations still flush while paused.**
They are bytes, a cap measured in megabytes is not defended by withholding them, and a triage action that
silently does not take effect is the broken product that sentence describes. What pause stops is
*fetching* — which is what consumes an allowance and what the user meant.

**Why a tier rather than a flag.** The tiers already express "what network behaviour is permitted right
now", they already compose with the offline states, and NFR-33 already defines how connections are torn
down when the tier changes. A separate mechanism would duplicate all three and would have to be reconciled
with them at every decision point.

**What it does not do is collapse the reasons.** The tier is one behaviour; *why* an account is in it is
the account's condition in [failure model](../runtime/failure-model.md), which keeps user-paused and
cap-paused distinct because they clear differently and only one of them is something the user did.

**What it costs:** the tier table now has a row that is not a network condition, in a document about
network conditions, and D-14's abstraction reports nothing that produces it. The tier is therefore chosen
from the network's answer *and* from state Sift holds, which is a slightly wider input than the table
previously implied.

**Contestable because:** a reader may argue the cap and a user pause are different enough that sharing a
tier hides something — a capped account resumes on its own when the period rolls over, and a paused one
never does. The condition model carries that difference; if it proves that users need to see it in
behaviour rather than in status, the answer is two rows, not two mechanisms.

**Offline is two states, not one, and collapsing them contradicts NFR-38.** A captive portal is a *usable*
path that lies about its responses, so probing it is the only way to learn that the user has signed in —
NFR-34 requires exactly that probe. Airplane mode and system sleep are the absence of a path, where a
probe can only fail, and NFR-38 says **zero** connection attempts with no qualifier. One row carrying both
triggers and one behaviour read as licensing a probe every sixty seconds in airplane mode, which is the
retry-storm pathology [scheduling](scheduling.md) exists to prevent, spelled out as policy. The
reachability values D-14 reports already distinguish the two; the tier table now does too.

## On cellular, polling beats push

**This inverts the usual rule and applies only to cellular.** Push keepalives are cheap in *bytes* but each
one promotes the radio to a high-power state and holds it there through the tail timer. Polling every
fifteen minutes at an aligned instant costs far less energy than a persistent connection that whispers
every few minutes.

Therefore on cellular, IDLE is dropped in favour of long aligned polls. See
[IMAP](../mail/providers/imap.md) and [Gmail](../mail/providers/gmail.md), whose push design must remain
correct without it.

## Requirements

**FR-36.** Cumulative data usage MUST be accounted per account per link class, be visible to the user, and
support an optional user-set hard cap that moves the account to the **Paused** tier under D-58.

"Cumulative" needs a period or it is unanswerable, and the period is a **rolling window whose length the
user sets alongside the cap**, defaulting to thirty days. A calendar month was the alternative and is
worse for the case the cap exists for: a mobile allowance that renews mid-month leaves a user capped for
weeks with a counter that will not reset until a date unrelated to their billing. The counter and the
window are installation-scoped policy — see [data model](../storage/data-model.md), which previously
carried the budgets and not the accounting they imply.

| ID | Requirement |
|---|---|
| **NFR-30** | Detect a path change and re-evaluate the policy tier within 2 seconds. Unknown metered state maps to Conservative, never Unrestricted |
| **NFR-31** | In Minimal tier, at or under 10 KB per hour per account steady-state, excluding user-initiated fetches |
| **NFR-32** | **Zero speculative prefetch of any kind** — bodies, images, attachments, filter lists — in Conservative or Minimal |
| **NFR-33** | On a path change, all connections torn down within 5 seconds and re-established within 5 seconds **of a usable path being available**: no stuck sockets, no duplicate delivery, no lost mutations. Where the new path is offline, teardown is the whole requirement — re-establishment waits, per NFR-38 |
| **NFR-34** | Captive portals detected and handled: one probe per 60 seconds, no authentication-failure cascade, no credential re-prompt |
| **NFR-35** | The override is persisted **by network identity** and takes precedence over detection every time. FR-35 owns the feature; this is the testable property — that a returning network is recognised as the same one, and that detection never overrules a stored answer |
| ~~NFR-36~~ | ~~Data usage accounted per account per link class, user-visible, with an optional hard cap~~ — **struck: a verbatim duplicate of FR-36 above.** Two identifiers for one requirement means a test can satisfy one while the other silently lapses, and dropping either reads as dropping a distinct guarantee. FR-36 is the owner; the number is retired and MUST NOT be reused |
| **NFR-37** | On cellular, at most 6 radio-waking events per hour per account at idle |
| **NFR-38** | Airplane mode or system sleep produces **zero** connection attempts until the path returns. No retry storm on wake |
| **NFR-39** | A single message fetch never exceeds a configurable byte ceiling without explicit confirmation. The default is L-13 in [limits](../limits.md) |

**NFR-31 previously read 50 KB per hour, and was restated for internal coherence — not adjusted to match a
measurement.** NFR-15 in [scheduling](scheduling.md) caps *unrestricted* idle at 1 KB per minute per
account, which is 60 KB per hour. The old figure therefore asked the second-most-restricted tier, with
persistent connections dropped in favour of long aligned polls, to save seventeen per cent — a tier that
changes the connection model and then barely changes the bytes. 10 KB per hour is consistent with
NFR-37's four to six aligned polls in the same period carrying envelopes and nothing else. It is a
hypothesis on the same footing as every other number here.

NFR-34 matters more than it appears: a captive portal returns plausible-looking HTTP responses to every
request, and a client that treats those as authentication failures will cascade into re-prompting for
credentials on every account at once. NFR-39 guards against a 100 MB attachment arriving on a tethered
link.

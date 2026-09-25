# Network conditions

**Owns:** D-14, D-58, D-95, D-96, FR-35, FR-36, NFR-30 through NFR-39 (NFR-36 struck — see below).

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
| **Unrestricted** | wired or wifi, definitely unmetered | full body and inline-image prefetch, attachment prefetch under NFR-39's ceiling |
| **Conservative** | metered unknown or guessed, or cellular | envelopes only; bodies on demand; no image prefetch; mutations flush normally |
| **Minimal** | metered, or constrained mode | inbox envelopes only; longest poll interval; mutations flush — they are bytes; everything else deferred |
| **Offline — portal** | captive portal | queue everything; one bounded reattempt per L-28 against the account's own provider — D-96, never a probe to a detection host; no connection churn |
| **Offline — no path** | offline, airplane mode, or system sleep | queue everything; **zero connection attempts of any kind, including the reattempt above**, until the path returns |
| **Paused** | the user pausing sync under FR-22, or cumulative usage reaching FR-36's cap | no delta, no push, no prefetch; connections torn down under NFR-33; **mutations still flush** |

Mutations flush in every tier above offline, **including Paused**. They are tiny, and a triage action that
does not take effect because the user is on cellular is a broken product.

No tier mentions filter-list updates, which earlier revisions ran only when unrestricted. Under
[D-111](../rendering/content-blocking.md) every list ships in the binary, so there is no list traffic for a
tier to allow or defer.

## D-95 — The tier is per account, pause is per account, and the tray gesture is not a state

**Chosen:** the policy tier is evaluated **per account**. The user-paused flag is per-account state;
FR-22's tray control is a **gesture** that sets it on every account, not a separate global switch.
**Rejected:** a single application-wide tier; a global paused flag beside the per-account ones.

**Why this needed settling.** Three documents disagreed about the scope of one word. This document made
pause a row in a table whose other five rows are derived from the network, which reads as global;
[failure model](failure-model.md) makes *paused by the user* and *paused by the data cap* **per-account
conditions with different clearing rules**; and [data model](../storage/data-model.md) put *"whether sync
is paused by the user"* in **installation** policy, singular. An implementer had to pick, and the pick
decides both the tray's behaviour and a storage schema.

**Per account wins because two of the six rows already are.** FR-36's cap is per account by its own
wording, and a capped account resumes on its own while a user-paused one never does — D-58 below names
that difference as its own weakest point. A global tier cannot express either. The network-derived rows
are simply the same value for every account when the path is shared, which costs nothing to represent per
account and immediately buys the case where it is not shared.

**Why the tray control is a gesture rather than a state.** A global flag *beside* per-account flags is two
sources of truth for one question, and every such pair eventually disagrees — a user pauses globally,
resumes one account, and no rule says what the tray shows. Making it a gesture removes the question:
"pause sync" sets every account paused, "resume" clears every account the user paused, and the tray
reflects the accounts rather than remembering a click. **A newly added account is not paused**, which is
the behaviour a global flag would have got wrong in the direction users notice least and like least.

**What it costs:** the tray must summarize N states into one control, and "some accounts are paused" is a
state it has to be able to show rather than round to one of the two.

**Contestable because:** most users have one or two accounts and will never see the difference, so this
is precision bought for a minority, at the cost of a tray affordance that is genuinely harder to design
than a switch.

## D-96 — Portal detection uses the connection Sift already has, and names no host

**Chosen:** a captive portal is detected from the behaviour of connections to **the user's own
providers** — a TLS handshake that fails in a way consistent with interception, or a response that is not
the provider's protocol. Sift contacts **no** detection endpoint, its own or anyone else's.
**Rejected:** probing a well-known connectivity-check URL; standing up a Sift-operated probe endpoint.

**Why not a probe endpoint, which is the conventional answer.** The tier table above requires a probe
every 60 seconds while a portal is present, and its destination was never named — while
[privacy](../security/privacy.md) calls its egress table *"the complete set of permitted outbound
connections"* and adds *"anything else is a defect"*. A recurring 60-second beacon from a resident
application is exactly the *"coarse record of when this machine is awake and roughly where"* that document
spends two paragraphs on for the list-update rows it has since struck under
[D-111](../rendering/content-blocking.md), and it would have been worse: those were occasional, this is
once a minute.

It is also permanent, for the reason [Q-18](../open-questions.md) gives about the list endpoint —
[D-33](../product/platforms-and-distribution.md) means a build keeps calling the address it shipped with
for as long as it stays installed. Q-18 is scoped to the list endpoint and does not reach this one, so
adding a probe endpoint would have created a permanent operational commitment without anyone noticing it
was one.

**Why the provider connection is a better signal anyway.** A portal that intercepts a TLS connection
cannot present a valid certificate for the provider's name, so interception is *distinguishable from
being offline* — which is the entire distinction this tier exists to draw. A connectivity-check endpoint
answers "is there a portal between me and that host", which is a proxy for the question Sift actually has:
"can I reach my mail". Asking the real question is both more accurate and free, because the connection
attempt was going to happen.

**The 60-second cadence stays and is now retry rather than probe.** It is a bounded reattempt of the
account's own next scheduled operation, on the wheel like everything else under
[D-87](../mail/provider-model.md)'s rule, so it takes no wakeup of its own and adds no destination.

**What it costs:** detection is per account rather than global, so a portal is discovered when the first
account tries and not before, and a Sift with every account paused would not notice a portal at all —
correctly, since it has nothing to do about it. It also cannot distinguish a portal from a
misconfigured or hostile network presenting a bad certificate, and treats both as offline-portal, which
is the safe direction.

**Contestable because:** conventional portal detection is a solved problem with well-known endpoints, and
declining to use them means Sift's detection is weaker on networks that pass TLS through and interfere
only with plain HTTP. The answer is that Sift makes no plain HTTP requests, so that class of portal is one
Sift cannot see and does not need to.

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

**What the counter counts, because a counter with no definition silently changes when a cap fires.** It
counts **bytes on the wire** — transport framing, encryption overhead and retransmissions included, taken
from the platform's own per-connection accounting where it offers one, and otherwise from the transport's
own byte counts rather than from application-layer payload sizes. The alternative, counting decoded
payload, under-reports by the fraction the user is actually paying for, and does it worst on the small
frequent requests idle sync is made of. The counters are durable installation policy covered by NFR-48,
so **changing what they count later silently changes when a user's cap fires** — which is why this is
fixed here rather than left to the first implementation.

**Traffic that belongs to no account is charged to the installation, never spread across accounts.**
FR-3's autoconfiguration discovery happens **before the account exists**, so there is nothing to charge it
to even in principle. Filter-list and [D-37](../rendering/sender-origin.md) infrastructure-list updates
were the other such traffic, serving every account and none, until
[D-111](../rendering/content-blocking.md) moved them into the binary; the rule is stated for the class, not
for them. All of it is accounted at installation scope, is visible to the user
alongside the per-account figures, and **does not count toward any account's cap** — a cap is a promise
about one mailbox's traffic, and letting installation traffic push an account over it would make the cap fire
for a reason the user cannot connect to that account. NFR-31's per-account steady-state figure reads the
same counter and excludes the same traffic, so the two cannot disagree about what they are measuring.

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
| **NFR-32** | **Zero speculative prefetch of any kind** — bodies, images, attachments — in Conservative or Minimal |
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

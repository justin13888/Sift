# Privacy

**Owns:** D-35, D-114, NFR-22.

## The posture

Sift is a client that reads a user's mail. The correct posture is that **nothing about the mail leaves the
machine except to the provider that already has it**.

## NFR-22 — Telemetry

**No telemetry may contain message content, addresses, subjects, or domains.** Crash reports MUST be
opt-in, and their content is constrained by D-35 below.

Domains are named explicitly because they are the tempting exception — a "which senders render badly"
metric sounds harmless and is a list of who the user corresponds with. Sender domains are correspondence
metadata; they are excluded on the same footing as subjects.

## Egress

The complete set of permitted outbound connections is:

| Destination | Purpose | Discloses |
|---|---|---|
| Configured mail providers | sync, fetch, mutation, server-side search | everything they already hold |
| ~~Filter-list sources~~ | **removed by [D-111](../rendering/content-blocking.md)** — every list ships in the binary and nothing fetches one | nothing — see below |
| ~~The bundled-list update source~~ | **removed by [D-111](../rendering/content-blocking.md)** — the [sender-infrastructure list](../rendering/sender-origin.md) and the bundled email filter list update with the binary | nothing |
| Autoconfiguration discovery | **only** during interactive account setup — see below | **the domain of an address the user is adding**, to a host that is not yet their provider |
| Remote content hosts | **only** for resources the user has explicitly allowed, and only through the [resource broker](../architecture/resource-broker.md) | that the message was opened, to the host the user allowed |
| ~~The crash-report endpoint~~ | **removed by [D-114](#d-114--a-crash-report-is-a-file-the-user-sends-themselves)** — a report is a local file the user reads and sends by their own means; Sift makes no connection for it | nothing |

Anything else is a defect. In particular there is **no analytics endpoint**, and there is no path by which
message content reaches any host other than the provider it came from.

**Autoconfiguration is the one row that discloses something to a party the user has not chosen**, and it
is listed because a table that claims to be complete has to carry its own exceptions. FR-3's discovery
tries a public provider database, which means the domain of the address being added reaches a third party
before the account exists. That is correspondence metadata under NFR-22's own definition — the clause that
names domains explicitly — so it cannot be waved through as configuration traffic. The requirement in
[accounts](../mail/accounts.md) is therefore that this step is **disclosed and skippable**, not that it is
silent and helpful. It is bounded in a way no other row is: it happens during setup, at the user's
instigation, once per account.

**The two list rows are struck rather than deleted, because what they disclosed is the reason they are
gone.** A list fetch carried the network address it was made from and the time it was made, to a host the
user did not choose. Because Sift is resident by design, those fetches would have recurred on a schedule
for as long as it stayed installed, and what accumulated at the other end was a coarse record of when this
machine is awake and roughly where it is — held by a third party, in a product whose stated posture is
that nothing about the mail leaves the machine. That disclosure was continuous where the autoconfiguration
row is once per account.

[D-111](../rendering/content-blocking.md) removes it rather than mitigating it: every list ships in the
binary and arrives through the platform channel with the rest of Sift, so no list traffic exists for a
table row to describe. The staleness that costs is the one NFR-43 already tolerates, and D-111 records it.
The same decision answers the integrity question this channel raised, which was
[tracked separately](../open-questions.md) as Q-11. It also removes the permanence Q-18 raised: under
[D-33](../product/platforms-and-distribution.md#d-33--platform-channels-only-sift-never-updates-itself) a
build keeps calling the address it shipped with for as long as it stays installed, so a list endpoint
would have frozen its address and payload at the first release. With no endpoint, nothing is frozen, and
the same section of D-33 states what a reinstated one would owe.

**The crash-report row is struck for the same reason, and it described less.** Unlike the list rows it
was never a design with a disclosure to remove: it named an endpoint with no address, no operator and no
retention, which [D-114](#d-114--a-crash-report-is-a-file-the-user-sends-themselves) settles by not having
one. Nothing about a crash crosses the network under Sift's own hand.

**There is no update endpoint.** [D-33](../product/platforms-and-distribution.md) removed self-update
entirely, so update traffic belongs to the platform's own channel and never to a Sift-initiated
connection. FR-26 in [process model](../architecture/process-model.md) is the requirement; this table is
where its absence is observable.

Remote content is blocked by default because every remote image in a message is a tracking beacon until
proven otherwise — see [content blocking](../rendering/content-blocking.md). Prefetching an allowed image
is itself the tracking event, which is why prefetch is gated on the same decision as display and is
disabled entirely under constrained network conditions — see
[network conditions](../runtime/network-conditions.md).

## D-35 — Crash reports carry no heap

**Chosen:** capture stack backtraces, per-subsystem counters and build metadata. Never capture heap
memory. Disable the operating system's own core dumps for the process, and let nothing leave the machine
except a report the user has read in full and chosen to send — which, under
[D-114](#d-114--a-crash-report-is-a-file-the-user-sends-themselves), they send themselves.
**Rejected:** conventional minidumps with a scrubbing pass; no crash reporting at all.

**Why.** A minidump of this process contains message bodies, addresses, subjects and — despite NFR-23's
intent — live credential material that has been read out of the OS store into memory to be used. Scrubbing
that is **unverifiable in principle**: you cannot demonstrate the absence of a token from a heap snapshot,
and NFR-23 states plainly that credentials must never be in a crash dump. A mechanism whose compliance
cannot be shown does not satisfy a requirement written that strongly.

Disabling the platform's own crash collectors matters as much as what Sift itself captures. Those
collectors write full dumps regardless of Sift's opt-in, so leaving them enabled would mean the guarantee
holds only for the path Sift controls.

**What it costs:** memory-corruption bugs and NFR-19 failures on hostile MIME are exactly the class that
backtraces alone diagnose poorly, and they are also the class least likely to be reproducible from a user
report. This decision accepts worse post-mortem debugging on the failures that matter most.

**Contestable because:** that cost is real and lands on the hostile-input path, which is the highest-risk
surface in the product. The mitigation is that the [fidelity corpus](../product/reference-environment.md)
and the fuzzing under NFR-40 are supposed to find these before users do — if they do not, this decision is
what made the difference.

## D-114 — A crash report is a file the user sends themselves

**Chosen:** a crash writes its D-35 report to a local file in Sift's own data directory. At the next launch
Sift says that a report exists, shows it in full, and offers to save a copy wherever the user chooses
through the platform's own save dialog. The user sends that copy by whatever means they like. Sift keeps
only the most recent report, so a new crash replaces an unsent one and the store is bounded by
construction; dismissing the notice deletes it. **Sift makes no connection for a crash report**, holds no
address for one, and ships none in any build. Where the project accepts reports is documented with the
release, not compiled into it.
**Rejected:** operating a crash-report endpoint; dropping crash reporting from the first release and
keeping D-35 as the seam.

**Why.** Under
[D-33](../product/platforms-and-distribution.md#d-33--platform-channels-only-sift-never-updates-itself)
every address a build calls is permanent, so an endpoint would have to be named, versioned and answered
for the life of the oldest installed build, by an operator with a retention policy — none of which exists,
and all of which would have to exist before the first release. It would also add the disclosure the list
rows were struck for: the network address and the time a report was sent, to a host the user did not
choose. A file needs none of it. It also makes D-35's condition literal rather than procedural: the user
cannot send a report without holding it, so reading it in full is the path, not a promise about the path.
Dropping reporting altogether would give up the one post-mortem signal D-35 kept, on the hostile-input
failures where it is already weakest.

The save dialog is chosen over revealing the file in place because it works the same way inside the macOS
sandboxes [D-45](../product/platform-baseline.md) requires and under Flatpak: the platform grants access
to the location the user picked, and Sift needs no broader file access to hand the report over. The
retained report holds what D-35 allows and nothing else, so it adds no correspondence metadata to a data
directory that is backed up and copied between machines.

**What it costs:** reports arrive only when a user bothers to send one, so they are far fewer, skewed
towards technical users, and cannot be counted, grouped or correlated by the project. A crash loop keeps
only its last report, which may not be its first cause. Nothing tells the project that a build is crashing
widely until someone says so.

**Contestable because:** a project with no crash volume learns about its worst failures late, and the
fuzzing under NFR-40 is carrying more of the load than D-35 already asked of it. Reinstating an endpoint
reopens this decision and D-33's permanence with it: its address and payload would have to be versioned
before the first build that calls it ships, and its row would return to the egress table.

## Queries are not retained

**Search queries MUST NOT be stored by default**, and neither policy scope in
[data model](../storage/data-model.md) carries a row for them.

A history of what someone searched their own mail for is correspondence metadata by this document's own
definition — the clause that names domains explicitly is drawn narrower than the queries would be, since a
query is often a person's name or an address typed in full. NFR-22 keeps that class of data out of
telemetry; a durable local record of it would be the same data, collected without being asked, in the
store that [encryption](../storage/encryption.md) notes is "backed up, synced, copied between machines,
and attached to bug reports".

This is the same judgement [D-41](../architecture/presentation-layer.md) makes in refusing to derive an
address book from observed mail: **Sift should not construct locally what it refuses to transmit.**
Recent queries within a session are a convenience and are not durable.

## The debug views are not telemetry

**Captive-portal detection adds no row, and that is a decision rather than an omission.**
[D-96](../runtime/network-conditions.md) detects a portal from the behaviour of connections Sift was
already making to the user's own providers, so there is no detection endpoint and no beacon. A
conventional 60-second connectivity check would have been a recurring third-party disclosure of when this
machine is awake and roughly where — the disclosure that removed the list-update rows from this table,
an order of magnitude more often — and permanent, for the reason
[D-33](../product/platforms-and-distribution.md#d-33--platform-channels-only-sift-never-updates-itself)
gives about addresses a build keeps calling for years.

The [debug panels](../runtime/observability.md) expose a great deal about a message and about Sift's
internals. None of it is transmitted. They are local introspection surfaces, and they exist partly so that
a user can verify these claims for themselves rather than take them on trust.

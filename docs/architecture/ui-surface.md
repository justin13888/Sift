# UI surface

What screens exist, what a window is, and how a user reaches everything.

**Owns:** D-97, D-98, D-101.

[UI shell](ui-shell.md) argues why the shells are native and states the requirements they must satisfy.
It describes no screen. The set implies more than twenty surfaces, names several of them inside
requirements — FR-3's disclosure, FR-33's ten sections, the blocked-content chrome — and says of none of
them whether it is a window, a sheet, or something reachable with no window at all.

That is not shell polish under [D-56](presentation-layer.md), because a state either has a rendering in
both shells or it has none: **the inventory is the list of places a state can be rendered**, and it has to
exist before either shell is written or the two will differ in what they can express.

## D-97 — Three window kinds, and everything else is presented within one

**Chosen:** exactly three kinds of top-level window — the **main window**, the **settings window**, and
the **standalone reader** — plus surfaces the always-on presence owns that require no window at all.
Everything else is presented within a window, and a surface that has no window available is either raised
by the always-on presence or deferred until one exists.
**Rejected:** a window per surface; a single window with everything inside it; leaving the question to
each shell.

**Why bound the kinds.** *"Windows are plural"* is normative in [UI shell](ui-shell.md), and
unconstrained that means every surface is a candidate window and the two shells will make different
choices — which is the drift [shell boundary](shell-boundary.md) calls a defect. Three kinds is the
smallest set that covers what the requirements demand: a place to triage, a place to change durable
settings, and the case where a user wants one message on screen beside another application.

**The main window** is the product. It holds an account and folder sidebar, the FR-6 message list, and
the reader — with the reader's body view under [D-90](../rendering/webview-isolation.md) being at most
one per window. Several may be open, each an independent view onto the same
[presentation layer](presentation-layer.md), which is what that document means by *"a window is a view
onto the presentation layer, never an instance of it"*.

**The settings window** is separate rather than a pane inside the main window, because
[D-72](lifecycle.md) restores main windows and their folders, and settings is not a place a user should
be restored into. It is also the surface that must be reachable when the main window is showing something
modal, which a pane cannot be.

**The standalone reader** shows one message with no list. It exists because
[FR-23](ui-shell.md)'s notification activation *"may mean opening a window on a process that has none"*,
and opening the full main window — restoring folders, loading the filter engine, painting a list — to
show one message a user asked for by name is the wrong response to that gesture.

### The surfaces, and where each lives

| Surface | Where | Notes |
|---|---|---|
| Account and folder sidebar, message list, reader | main window | FR-6; the thread reader is [D-54](../rendering/webview-isolation.md)'s native rows over one body view |
| Search, with FR-20 operators and FR-21 source labels | main window | Replaces the list's contents rather than opening anything |
| Attachment list, save, and warned open | within the reader | FR-10; the save destination is the platform's own chooser, and NFR-53's final path is shown |
| Blocked-content chrome, and the count and reason | reader chrome, never in the document | [Resource broker](resource-broker.md) requires every affordance be native, because a control inside the document is one a sender can counterfeit |
| Link confirmation, and FR-42's unsubscribe destination | a sheet on the window that raised it | [Link handling](../rendering/link-handling.md) |
| Undo, and FR-16 conflict notices | transient, within the window; and the always-on presence when none is open | [D-86](../mail/mutations.md) puts the record in the layer for exactly this reason |
| Permanent-delete confirmation | a sheet | [Mutations](../mail/mutations.md) makes this the one intent that is confirmed rather than optimistic |
| Add account, provider choice, FR-3's disclosure, manual configuration | a sheet on the window that started it; a window of its own on first run | See below |
| D-53 backfill progress | the sidebar, beside the account | It can run for a long time and must not be modal |
| Account settings, and installation settings | settings window | Enumerated in D-101 |
| FR-33 per-message debug view, FR-34 runtime panel | separate windows | Preference-gated and off by default; they are inspection surfaces, not part of the reading flow |
| The annunciator — D-49 conditions, D-71 process conditions | main window chrome, and the always-on presence | Reaches the user with no window open, which is D-49's own requirement |
| Re-authentication prompt, FR-26 restart prompt, quarantined intents | always-on presence, opening the relevant surface | The three [D-67](view-protocol.md) host callbacks that must work with no window |

**Nothing in this table is modal to the application.** A sheet attaches to the window that raised it and
leaves every other window usable, which matters because Sift is an application a user leaves open —
blocking the whole application on a confirmation in one window is a behaviour a resident application
cannot afford.

### Navigation, and what "reachable" means

**Every surface above is reachable from the main window's keyboard model**, which is
[FR-24](ui-shell.md)'s requirement and is what D-98 makes concrete. Surfaces raised by the always-on
presence are additionally reachable from it, because a process with no window has no keyboard model.

**Opening a surface never destroys state in another.** A sheet does not discard the list's selection, and
the settings window does not close main windows — which reads as obvious and is the thing a shell gets
wrong when settings is implemented as a mode of the main window.

**First run has no main window until an account exists.** [UI shell](ui-shell.md) rules that the
account-less state *"is the add-account flow"* rather than an empty inbox, so the add-account surface is
the first window, and the main window opens behind it once an account is added. Adding a *second* account
later is a sheet on an existing window, which is the same flow in a different frame rather than a second
implementation.

**What it costs:** three window kinds is three restoration paths under [D-72](lifecycle.md), three
keyboard scopes, and a rule about sheets that both toolkits express differently.

**Contestable because:** the standalone reader earns its place on one gesture — notification activation —
and a simpler design would open the main window and select the message. That is defensible, and it is
rejected because the gesture is the one a user makes most often when Sift has no window at all, which is
precisely when opening the full window is most expensive.

## D-98 — Actions are a register, and the register is what FR-24 and the test harness both use

**Chosen:** every user-initiated operation is a **named action** with a stable identifier, a scope, and
an enablement rule; the set is enumerated here; the command palette is a view of it; and the shell test
harness invokes actions by identifier. Default bindings are per platform and are **not** user-rebindable
in the first release.
**Rejected:** actions defined per shell; a palette with its own list; rebindable bindings now.

**Why a register.** [FR-24](ui-shell.md) requires every action be keyboard-reachable including a command
palette, and then says something stronger that nothing followed up on: *"it is also what makes the app
testable without UI automation."* That is a claim that a test can drive the application through its
action vocabulary — which is only true if the vocabulary is a real surface with stable names, reachable
across the boundary. **The action set is therefore an ABI surface**, not shell polish, and it is enumerated
here for the same reason [limits](../limits.md) is enumerated in one place: three consumers must agree on
it, and one of them is a test.

It is also what makes [provider model](../mail/provider-model.md)'s affordance rule mechanical. That
document requires that *"if an account declares no tag support, the tag affordance is absent for that
account"*, and a palette built from its own list would have to reimplement that check. A palette that is
a **view of the register filtered by enablement** gets it for free, in both shells, from one rule.

### What an action carries

| Field | Meaning |
|---|---|
| Identifier | Stable, never reused. It appears in tests, in the palette's own ordering, and in bindings |
| Scope | Application, window, list selection, or open message — which determines what must exist for it to apply |
| Enablement | A function of the current selection, the account's declared capabilities, and the account condition. An action that is not enabled is **absent from the palette**, not shown disabled |
| Default binding | Per platform, following that platform's idiom; the identifier is shared and the key is not |

**Enablement hides rather than disables**, which is the opposite of the usual convention and follows
[provider model](../mail/provider-model.md)'s rule that an unsupported affordance is *absent, not
approximated*. A greyed-out "add tag" on an account that has no tags tells the user their mail has a
feature they cannot reach; its absence tells them the truth.

### The actions

**Message and selection** — archive, delete to trash, permanently delete, move to folder, flag, mark
read, mark unread, add tag, remove tag, report junk, report not junk. These are exactly
[FR-13](../mail/mutations.md)'s intent set, and the correspondence is deliberate: **the action set MUST
NOT contain a mutation that is not an intent**, or the closed set FR-13 describes has a second door.

**Reading** — open message, open in standalone reader, next and previous message, next and previous
unread, expand and collapse thread, show plain text, show raw source, toggle the dark transform for this
message, allow remote content for this sender, find in message, reply, reply all, forward.

**Navigation** — next and previous folder, next and previous account, go to unified inbox, focus list,
focus reader, focus sidebar, focus search.

**Search** — begin search, clear search, narrow to this account, narrow to this folder.

**Application** — new main window, close window, quit, pause sync, resume sync, add account, open
settings, open the per-message debug view, open the runtime panel, and the palette itself.

**Undo** — undo the last reversible gesture, which acts over [D-85](../mail/mutations.md)'s undo group
rather than over a message, so a bulk operation reverses as the one gesture
[FR-17](../mail/mutations.md) promises.

**Reply, reply-all and forward are actions like any other**, and they hand off under
[FR-41](../product/scope.md). They are in this list rather than omitted because FR-41 makes the handoff a
requirement rather than a permission, and because an action set with no reply in it is the one a reviewer
would assume was an oversight.

### Bindings are fixed in the first release, and the register is the seam

**Bindings are not user-rebindable**, and this is a deferral rather than a refusal. A rebinding scheme is
durable policy with a storage entity, a conflict model, a reset path, and a permanent format — and once
users have rebound keys, changing any of that is a break. Shipping without it costs nothing that cannot be
added, because the register already gives the mechanism a rebinding surface would need: stable identifiers
to bind to. That is the [seam](../glossary.md) [scope](../product/scope.md) requires of anything deferred.

**The default set follows each platform's idiom rather than being shared.** A shared key map would be
wrong on at least one platform, and the thing that must be identical across shells is the action set, not
the keys. This is [D-17](shell-boundary.md)'s rule applied precisely: *what a shell can do* is the
boundary's business, and *how a user asks for it* is not.

**What it costs:** a register that must be kept in step with three consumers, and an enumeration that will
be incomplete the first time somebody adds a feature without adding its action.

**Contestable because:** hiding disabled actions makes the interface change shape between accounts, and a
user with a Gmail account and an IMAP account will find that the same keystroke does nothing in one of
them with no visible reason. Showing them disabled would explain it — at the cost of advertising
capabilities the account does not have, which provider-model chose against for the affordance and this
follows for the action.

## D-101 — Settings are enumerated with their defaults, and the scope split is the storage split

**Chosen:** the settings surface is enumerated here, every setting has a stated default, and it is
organized by the **scope** [data model](../storage/data-model.md) already stores it at — installation or
account — rather than by topic.
**Rejected:** organizing by topic; leaving defaults to each shell; a settings surface that accretes per
feature.

**Why enumerate at all.** Roughly a dozen settings are named in passing across this set — *"the budget is
user-configurable"*, *"the dwell is configurable, including off"*, *"list subscriptions and custom rules
MUST be user-manageable"* — and several have no value anywhere. Under [D-56](presentation-layer.md) the
surface is written twice, in Swift and in GTK, so a settings model that accretes per feature ends up
shaped differently in each, which is *"a capability that exists for one shell and not the other"* by
another route.

**Why organize by scope.** It is the split that already exists in storage and the split that decides
behaviour: an account setting disappears with [FR-4](../mail/accounts.md)'s removal and an installation
setting does not. Organizing by topic would put the cache budget beside the per-sender allowlist, which
look related and have opposite lifetimes — and the allowlist is the one that matters, because
[data model](../storage/data-model.md) says it *"is security state, not a preference, and MUST be treated
as such"*.

### Installation settings

| Setting | Default | Owner |
|---|---|---|
| Cache budget for bodies, attachments and blobs | 90 days or 2 GB, whichever binds first | [NFR-14](../storage/cache-and-blobs.md) |
| Envelope and index budget | L-20 | [NFR-52](../storage/cache-and-blobs.md) |
| Single-fetch ceiling before confirmation | L-13 | [NFR-39](../runtime/network-conditions.md) |
| Filter-list subscriptions and custom rules | the standard public blocking and privacy lists, plus the bundled email list, all enabled | [FR-27](../rendering/content-blocking.md) |
| Dark transform | **off** | [FR-31](../rendering/dark-mode.md), which makes it opt-in by name |
| Mark-read dwell | L-21, and it may be set to off | [D-52](../mail/mutations.md) |
| Data cap, and the accounting window | no cap; a 30-day rolling window | [FR-36](../runtime/network-conditions.md) |
| Per-network overrides | none; detection decides | [FR-35](../runtime/network-conditions.md) |
| Quiet mode, and notification defaults for new accounts | quiet off; notify on new mail in the inbox only | [FR-23](ui-shell.md) |
| Debug view and runtime panel | **off** | [FR-33, FR-34](../runtime/observability.md), preference-gated by their own decision |

### Account settings

| Setting | Default | Owner |
|---|---|---|
| Watched folder set | the inbox and the account's special-use folders | [FR-43](../runtime/scheduling.md) |
| Per-folder notification rules | inherited from the installation default at account creation | [FR-23](ui-shell.md) |
| Paused | not paused, including for an account added while others are paused | [D-95](../runtime/network-conditions.md) |
| Write authorisation | **off** — the account is watched and not written to | [D-110](../mail/mutations.md), and the control is beside the queue it releases rather than here |
| Per-sender remote-content allowlist | empty | [FR-8](../rendering/pipeline.md) — **security state**, and presented as such rather than as a preference list |
| Per-sender dark-mode choices | empty | [FR-31](../rendering/dark-mode.md) |

**The two per-sender lists are shown but are not edited like preferences.** They are records of decisions
the user made in context — allowing a sender's images, keeping a sender's own dark styling — and the
settings surface's job is to make them **reviewable and revocable**, not to invite bulk editing of a
security-relevant list in a screen away from any message.

**Every default above is a decision that ships**, which is why they are here rather than in each shell. A
default chosen independently by two shells is two products, and the ones that matter most are the ones
that look least like decisions: dark transform off, debug off, no cap, allowlist empty.

**What it costs:** a surface that must grow deliberately, and two tables that go stale the first time
somebody adds a preference without adding a row.

**Contestable because:** organizing by scope is a storage-shaped organization presented to users, who do
not know what an installation is. The defence is that the tables are the specification rather than the
layout, and a shell is free to group them for humans — but a shell that *invents* a setting, or ships a
different default, is the failure this exists to prevent.

## Related

- [UI shell](ui-shell.md) — D-1, FR-6, FR-22, FR-23, FR-24, and why the shells are native
- [Presentation layer](presentation-layer.md) — what the shells bind to rather than decide
- [State register](state-register.md) — the states these surfaces render
- [Lifecycle](lifecycle.md) — D-72, which restores some of these windows and deliberately not others

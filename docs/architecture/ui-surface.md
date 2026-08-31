# UI surface

What screens exist, what a window is, and how a user reaches everything.

**Owns:** D-97.

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

## Related

- [UI shell](ui-shell.md) — D-1, FR-6, FR-22, FR-23, FR-24, and why the shells are native
- [Presentation layer](presentation-layer.md) — what the shells bind to rather than decide
- [State register](state-register.md) — the states these surfaces render
- [Lifecycle](lifecycle.md) — D-72, which restores some of these windows and deliberately not others

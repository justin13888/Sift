# Glossary

Terms used with a specific meaning in this documentation set. Where a term has a looser everyday sense,
the definition here wins.

| Term | Meaning |
|---|---|
| **Core** | The always-resident Rust layers — sync, storage, index, mutation queue, brokers, and the rendering pipeline. Contains no web engine. See [overview](architecture/overview.md) |
| **Shell** | The native application interface — list, sidebar, reader chrome. AppKit on macOS, GTK4 on Linux. Not a web engine. Created with a window and destroyed with it; nothing in it is authoritative |
| **Subsystem** | The unit memory is attributed to, and one of the two axes wakeups are attributed to — the other is the account, because NFR-11 is stated per account and an account is not a subsystem. Named, enumerated, and reported on — see [observability](runtime/observability.md) |
| **Pressure governor** | The component that reads declared cache sizes, subscribes to the OS pressure signal, and drives the shed tiers. See [memory pressure](runtime/memory-pressure.md) |
| **Capability token** | The unguessable, per-view identifier under which a body view addresses its resources. Revoked wholesale on teardown — see [webview isolation](rendering/webview-isolation.md) |
| **Toolkit residue** | What a UI toolkit leaves resident after its window is destroyed. Sets the idle floor under [D-2](architecture/process-model.md) |
| **Body view** | The isolated web-engine document that renders one message body. No script, no network. See [webview isolation](rendering/webview-isolation.md) |
| **Presentation layer** | The shared Rust layer that owns windowing, selection, sort, formatting, and result assembly, so the shells only bind and lay out. See [presentation layer](architecture/presentation-layer.md) |
| **Resource broker** | The core component through which every byte the body view loads must pass. The place where blocking decisions are made. See [resource broker](architecture/resource-broker.md) |
| **Location** | *Where* a message is. Cardinality is one, or one-or-more, depending on the provider. See [provider model](mail/provider-model.md) |
| **Tag** | A many-to-many user label, distinct from Location. Called labels, categories, or keywords by different providers |
| **Intent** | A provider-agnostic mutation — *archive this thread* — resolved to wire operations inside an adapter. Never a wire operation itself. See [mutations](mail/mutations.md) |
| **Envelope** | The cheap per-message summary: sender, subject, snippet, date, flags, size, structure. Retained indefinitely; bodies are not |
| **Delta** | The authoritative change feed, resumed from a cursor. Answers *what changed* |
| **Push** | A notification that *something* changed. A doorbell, not a change feed. See [sync engine](mail/sync-engine.md) |
| **Cursor** | The provider-specific resumption token for a delta. Its invalidation is a first-class recovery case — NFR-18 |
| **Capability** | A declared property of a provider account that the sync engine, mutation queue, and UI plan against, in place of the provider's name |
| **Synthetic origin** | The first-party origin derived from authenticated sender identity, because an email has no document origin. See [sender origin](rendering/sender-origin.md) |
| **Shed tier** | One of the L0–L3 memory-pressure response levels. See [memory pressure](runtime/memory-pressure.md) |
| **Policy tier** | One of the Unrestricted / Conservative / Minimal / Offline network behaviour levels. See [network conditions](runtime/network-conditions.md) |
| **Footprint** | The memory metric Sift is measured by: `phys_footprint` on macOS, PSS on Linux. Never RSS. See [observability](runtime/observability.md) |
| **Fidelity corpus** | The real-world message set that rendering correctness is gated on. Distinct from the scale corpus used for performance. See [reference environment](product/reference-environment.md) |
| **Seam** | The named place a deferred feature would enter, recorded so that adding it later is an append rather than a redesign. Required of anything in [scope](product/scope.md)'s deferred list |
| **Attested origin** | A [synthetic origin](rendering/sender-origin.md) that resolved from a passing signature or sender-policy check, rather than from an unauthenticated header. Only an attested origin may widen what a message loads — see D-37 |
| **Invariant** | An asserted, tested property of the sanitizer (I1–I10) or of body isolation (N-1). Not an aspiration |

# Shell boundary

The contract between the core and the native shells.

**Owns:** D-17, NFR-24.

This document was previously an inter-process contract. [D-2](process-model.md) reversed the process
split, so what remains is a boundary between two layers of one process. The distinction it drew — that
the shells receive prepared view models and never touch the store — survives the reversal and is the
reason the boundary still exists at all.

## NFR-24 — Sift never listens

**Sift MUST NOT open a listening socket of any kind, for any purpose.** Not TCP, not a Unix domain
socket, not an abstract namespace socket.

Under the two-process design this requirement carried an exception it never acknowledged: the local IPC
socket itself, and separately the loopback listener that an OAuth redirect implies. Both are now gone —
the shell boundary is a function call, and authentication returns through a registered URI scheme handler
rather than a socket, see [credentials](../security/credentials.md). The requirement is therefore
absolute, with no carve-out, which is a far easier property to test and to audit than a bounded exception.

A listening socket on a mail client is a local privilege-escalation and cross-application attack surface.
Having no listener at all is the strongest available answer to "what can another process on this machine
reach", and it is now literally true.

## D-17 — A narrow C ABI between the core and the shells

**Chosen:** the presentation layer exposes a C ABI; the macOS shell binds it through generated Swift
declarations, and the GTK shell links the Rust crate directly.
**Rejected:** a serialized message protocol retained in-process; per-language bindings generated from a
schema.

**Why.** With one process, a wire format would mean serializing a view model into bytes and immediately
deserializing it in the same address space. That cost falls directly on NFR-2's 50 ms folder switch and
NFR-7's 16 ms optimistic feedback, and it buys nothing: there is no trust boundary here and no version
skew to detect, because both sides ship in one binary.

A C ABI is the narrowest waist that Swift and Rust both speak natively. The GTK shell needs no boundary
at all — it is Rust — so the ABI exists for exactly one consumer, and keeping it narrow is what stops the
macOS shell's assumptions leaking into a layer the Linux shell also uses. See
[presentation layer](presentation-layer.md), which owns that constraint.

**The ABI is the contract, not the macOS shell's copy of it.** The GTK shell links Rust directly and could
therefore reach presentation-layer API the ABI never exposes; it MUST NOT. Anything a shell is permitted
to use MUST be expressible across the C ABI, and a capability that exists for one shell and not the other
is a defect in this boundary rather than a Linux feature. The failure this forbids is quiet: the ABI
becomes the poorer of two interfaces, the two shells drift apart in what they can do, and the
[presentation layer](presentation-layer.md)'s whole argument — one UI codebase, two view layers — is lost
one convenience at a time. It is the same failure mode that document names for designing against a single
consumer, running in the opposite direction.

**What it costs:** every type crossing the boundary needs an explicit, stable representation, and the
ownership rules for anything passed across it must be written down rather than inferred. Memory-safety
bugs are possible at this boundary in a way they are not elsewhere in the core. The rule above adds a
second cost: the Linux shell pays for a narrow C-shaped interface it does not itself need.

**Contestable because:** a serialized protocol would have kept the two-process option open at low cost,
and would have made the boundary auditable by inspection rather than by reading unsafe code. If the ABI
surface grows past what one file can hold, that is the signal this was the wrong shape.

## What crosses the boundary

The contract itself — threading, reentrancy, cancellation, batch index space, ownership, identifier
stability and error representation — is in [view protocol](view-protocol.md), which owns D-48. This page
argues why the boundary exists and what shape it takes; that page specifies what an implementer has to
honour, which is the writing-down D-17 above requires and did not supply.

The interface is a **command and view-model protocol**, not a database-access protocol. The shells send
intents and requests and receive prepared view models. **The shells MUST NOT be given a path to the
store, the index, or a provider adapter.** This keeps the store single-writer, keeps the shell disposable,
and means the layer that can be destroyed on a window close holds nothing authoritative.

The shape of what crosses is defined in [presentation layer](presentation-layer.md).

## Message content on this path

Message bodies cross this boundary post-sanitization only. Raw provider payloads MUST NOT be handed to a
shell to process; parsing, sanitizing, blocking, and transforming all happen in the core. See
[rendering pipeline](../rendering/pipeline.md).

Resource loads requested by the body view do **not** cross this boundary. They arrive from the engine's
scheme handler and are answered by the resource broker directly — the body view has no other way to
obtain bytes, and the shell is not in that path. See
[webview isolation](../rendering/webview-isolation.md).

## Related

- [Threat model](../security/threat-model.md) — which boundaries are trust boundaries, and this one is not
- [Presentation layer](presentation-layer.md) — the view models themselves

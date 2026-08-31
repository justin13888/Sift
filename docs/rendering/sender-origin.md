# Synthetic sender origin

**Owns:** D-11, D-37, FR-28.

## The problem

Filter syntax — third-party matching, domain-scoped rules, hostname-scoped cosmetic filters — assumes a
**document origin**. A web page has one. **An email does not.**

Without a defined origin, third-party matching is meaningless and domain-scoped rules cannot apply. This
must be decided explicitly rather than defaulted into.

## D-11 — Synthesize the origin from authenticated sender identity

**Chosen:** derive a synthetic first-party origin at ingest, in strict priority order.
**Rejected:** using the From header alone; treating everything as same-origin; treating everything as
third-party.

| Priority | Source | Confidence |
|---|---|---|
| 1 | The signing domain of a **passing** cryptographic signature | attested |
| 2 | The envelope sender's domain, if the sender policy check passed | attested by policy |
| 3 | The From header's domain | **unauthenticated — marked low-confidence** |
| 4 | Nothing resolves → origin is **null**, meaning everything is third-party and the strictest rules apply | none |

**Why.** Only the first is cryptographically attested, and only an attested origin can safely widen what a
message is allowed to load. Deriving the origin from authentication also produces a genuinely good
property: **authentication results directly harden blocking**. A spoofed sender does not get the sender's
allowances.

Falling to null rather than to a permissive default is the correct failure direction: an unauthenticated
message is treated as maximally hostile.

**FR-28.** Message authentication MUST run at ingest, and its results — signing domain, sender-policy and
alignment outcomes — MUST be stored on the message. See [data model](../storage/data-model.md). They feed
both this origin and the [debug view](../runtime/observability.md).

## D-37 — Known mail infrastructure is first-party when the origin is attested

**Chosen:** a bundled, updatable list of known mail-service infrastructure. A resource host on that list is
treated as first-party when — and only when — the message's synthetic origin was **attested**, meaning it
resolved at priority 1 or 2 above.
**Rejected:** accepting the breakage; treating a service provider as first-party regardless of attestation.

**Why.** Mail service providers legitimately host images on their own infrastructure, so a sender's images
are frequently third-party under D-11. Strict third-party blocking therefore breaks a substantial amount of
legitimate mail *once the user has already chosen to enable remote content* — and mail that stays broken
after the user said "show me this sender's images" reads as Sift being broken, not as Sift being careful.

Gating on attestation is what keeps this from being a hole. An unauthenticated sender cannot reach the
widening at all, so a spoofed message gains nothing, which preserves the property D-11 exists for:
authentication results directly harden blocking.

**What it costs:** a list somebody has to curate, forever, and the coarseness of a flat list — any attested
sender is widened to any listed host, not merely to their own provider. The list is also a fingerprinting
surface in the sense that it encodes which providers Sift knows about, though it is bundled rather than
queried.

**Updating it is network traffic and is governed as such.** The list ships bundled so that a fresh install
is correct with no fetch, and updates arrive over the same path and the same policy tier as filter-list
updates, carried as a row in the [egress table](../security/privacy.md). It inherits NFR-43's rule
directly: a stale list is acceptable, an absent one is not. The failure direction is the safe one, since a
list that is behind widens *less* than it should and the cost is a broken image rather than an unintended
load — which is the same asymmetry D-11 chose when it made null the fallback origin.

**One case deserves naming.** Where a provider signs with its own domain rather than the brand's, priority
1 attests to the **provider**, not the sender. The origin is then the provider's domain and this rule is
largely redundant for that message — worth knowing before concluding the list is doing more work than it
is.

**Contestable because:** the tighter design is a map of attested-signer to permitted host, so that one
sender's provider does not widen another sender's allowances. That is the most correct answer and the most
work, and it is a strict refinement of this one — the list becomes a map, and nothing else changes. If the
flat list produces a real widening incident, that is the upgrade path, and the pairing MUST be surfaced in
the [debug view](../runtime/observability.md) either way.

## Consumption

The synthetic origin is consumed by [content blocking](content-blocking.md) for third-party and
domain-scoped rule evaluation, and by the per-sender allowlist that governs remote content in
[the pipeline](pipeline.md). A per-sender allowlist keyed on an *unauthenticated* From domain would be
trivially forgeable, so the allowlist MUST key on the synthetic origin, not on the displayed sender.

That rule has a consequence at priority 4 which is easier to meet in the interface than to discover in the
code: **where the origin is null there is nothing to key a durable allowance on**, so a persistent "always
show images from this sender" cannot exist for that message. [The pipeline](pipeline.md) states what is
offered instead, and why offering a one-time allowance is what keeps this from reading as a refusal.

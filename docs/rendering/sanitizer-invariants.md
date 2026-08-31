# Sanitizer invariants

**Owns:** D-26, I1–I10, NFR-40.

The sanitizer is asserted, not assumed. These invariants are testable claims, and they are tested — see
NFR-40 below.

Notation: *S* is the sanitizer, *P* the parser, *W* the serializer.

## The invariants

| ID | Invariant |
|---|---|
| **I1** | **No script.** The output contains no script element, no event-handler attribute, no script-bearing URL scheme in any position, no script embedded via foreign content, and no script-equivalent CSS construct |
| **I2** | **No implicit egress.** Every URL in a *fetching* position — image sources and source sets, poster attributes, CSS url references, font sources, imports — uses the internal scheme. No external-scheme URL survives in any fetching position, **including one a filter rule has already condemned** |
| **I3** | **No frames, plugins, or forms.** No frame, object, embed, applet, form, or form-control element |
| **I4** | **No document control.** No base element, no equivalent-header meta, no link element, no title element |
| **I5** | **Containment.** Structural, because the body renders in its own document; additionally, fixed positioning and viewport-unit escapes are rejected |
| **I6** | **Idempotence.** *S(S(x))* is equivalent to *S(x)* |
| **I7** | **Bounded.** *S* terminates within a time bound and produces bounded output for all inputs. Nesting depth, node count, and attribute count are capped |
| **I8** | **Parse stability.** *P(W(S(x)))* is DOM-isomorphic to *S(x)* |
| **I9** | **No content invention.** *S* introduces no visible text that was not present in *x*. Removal is permitted; addition is not |
| **I10** | **Encoding determinism.** Output is well-formed UTF-8 for any input bytes and any declared charset, including malformed and mutually contradictory declarations |

## D-26 — A spec-conformant tree builder, with Sift's own policy over it

**Chosen:** parse to a full DOM with a spec-conformant HTML5 tree builder, apply a purpose-written
allowlist policy to the tree, and serialize with the matching serializer.
**Rejected:** a streaming HTML rewriter; adopting an existing general-purpose sanitizer crate.

**Why.** I8 sets the bar. Asserting that *P(W(S(x)))* is DOM-isomorphic to *S(x)* is only meaningful if
*P* implements the same parsing algorithm the rendering engine does, because the failure this invariant
exists to catch is precisely the case where Sift's parser and the engine disagree. That rules out anything
short of a conformant tree builder. NFR-40's dual-parser divergence method needs the same property from
the other direction.

A streaming rewriter is faster and bounded in memory, which flatters NFR-41. It was rejected because three
separate consumers need a tree that a streaming pass never materializes: I8's round-trip assertion,
procedural cosmetic filters evaluated against the parsed DOM, and the colour graph the
[dark transform](dark-mode.md) builds.

An existing sanitizer crate brings a tested allowlist, which is genuinely valuable. It was rejected
because the policy Sift needs is not the policy those crates implement — they exist to make user-submitted
markup safe to embed in a page, and have no notion of rewriting **every fetching position** to an internal
scheme (I2), nor any CSS pipeline at all.

**What it costs:** the allowlist is Sift's to author and to maintain — elements, attributes, URL schemes
per position, and CSS properties — and it is the kind of artefact that is only ever as good as its last
review.

**Contestable because:** the tree is materialized for every message, including the large hostile ones I7
exists to bound, and NFR-41 is a shared 30 ms budget that the [full cascade](dark-mode.md) also draws on.
If that budget is missed, this is where the pressure lands first.

## I2 is asserted over the sanitizer's output, which is why the sanitizer rewrites

The rewrite to the internal scheme is the sanitizer's own work, in the same pass that enumerates fetching
positions. It is not a later stage, and [the pipeline](pipeline.md) says so in its stage list: an
invariant asserted over *S(x)* cannot be satisfied by something that happens to *S(x)* afterwards.

Two consequences are easy to get backwards.

**A condemned URL is rewritten too.** Rewriting is not a reward for surviving the blocker; it is how the
document is made incapable of naming an external host at all. Whether the address then yields bytes is
the [resource broker](../architecture/resource-broker.md)'s decision at request time, and it has to be,
because the per-sender allowlist and the network policy tier both change without the message changing.

**Rewritten is not the same as bound.** The address *S* emits is view-independent; the per-view
[capability token](webview-isolation.md) of D-28 is applied at the pipeline's bind stage, once a view
exists. I2 is satisfied at *S*, before any token is minted.

## I9 says what its name says

I9 was previously stated as a subset relation over visible text, and it was recorded as needing tightening
because it appeared to conflict with legitimate transformations. **The diagnosis was wrong, and it is worth
recording why.** A subset relation already permits removal, so stripping a tracking pixel's alternative
text never violated it; and the [dark transform](dark-mode.md) adds CSS, not text, so it never violated it
either. The stated conflicts did not exist.

What the invariant is actually for is **invention** — text appearing in the output that no sender wrote.
Stating it that way removes the false conflict, needs no carve-out for the later pipeline stages, and lets
I9 keep constraining all of them rather than shrinking to cover the sanitizer alone.

"Visible text" still needs an operational definition before I9 can be encoded as a test under NFR-40, and
that definition MUST be written against the parsed tree rather than against rendered output, since the
sanitizer has no renderer.

## Legacy word-processor markup is stripped, not transformed

Vendor conditional blocks and namespaced markup emitted by legacy word processors are **removed by the
sanitizer** and never reach the [dark transform](dark-mode.md). This was previously recorded as an
assumption; it is a consequence of the allowlist model rather than a special case:

- Conditional blocks parse as **comments** under the HTML5 algorithm, and comments are not retained.
- Vendor-namespaced elements are foreign content and are not on the element allowlist.
- Vendor-prefixed CSS properties fail the property allowlist in the same pass that enumerates fetching
  positions.

Because it falls out of three independent rules rather than one, it MUST be asserted directly as a
regression vector rather than left as an inference. A future allowlist change could reintroduce it
silently, and the transform's input contract depends on it not being there.

## Growing the allowlist is how I2 regresses

The allowlist is not frozen — new elements and CSS properties appear, and refusing to ever extend it
means rendering falls behind the mail people are actually sent. The rule is therefore about *how* it
grows, and it is normative:

**No addition to the element, attribute, or CSS-property allowlist may land without classifying whether
it introduces a fetching position, and adding an NFR-40 vector for it either way.**

I2 is stated over fetching positions, not over an enumerated element list, so it is silently falsifiable
by exactly one kind of change: allowing something that fetches, in a position nobody thought to rewrite.
Nothing else in this document catches that. The invariant still reads as satisfied, the tests still pass
because no vector exercises the new position, and the body view acquires a load that never reached the
broker.

The CSS half is where this is most likely, because [content blocking](content-blocking.md) already
enumerates fetching positions explicitly — background images, font sources, list-style images, cursors,
border images, masks, reflections — and an enumeration is a list that a new property is added *beside*
rather than *into*. A "harmless" decorative property that happens to accept a URL is the whole failure
mode.

The classification is cheap; noticing that it was needed is the expensive part, which is why the coupling
is written down rather than left to review.

## I8 is the one that will bite

Mutation-based cross-site scripting lives entirely in the serialize-then-reparse round trip: elements whose
content model changes when script is disabled, template contents, namespace confusion between HTML and
foreign content, and entity re-interpretation. **A sanitizer that satisfies I1 through I7 and fails I8 is
exploitable.** Prioritize it accordingly.

## What each invariant is backed by

Defence in depth changes how much each invariant must be sweated, and stating that explicitly tells a
reviewer where to spend attention.

| Invariant | Independent backstop |
|---|---|
| I1 | JavaScript disabled at the engine level — NFR-20 |
| I2 | The body view has no network capability — invariant N-1 |
| I5 | The body renders in its own document with its own data store — NFR-25 |

An I1 regression is therefore a defence-in-depth degradation, not an incident. I6 through I10 have **no
backstop** and are the ones that carry real risk alone.

See [webview isolation](webview-isolation.md) for the backstops themselves.

## NFR-40 — Verification

I1 through I10 MUST hold for every input, verified in CI by five complementary methods:

1. **Property tests with a grammar-based generator.** Nested elements, attribute soup, entity sequences,
   comments, character-data sections, foreign-content transitions, encoded-word headers, mixed charsets.
   Random-byte generation is supplementary only — it rarely reaches interesting states.
2. **Differential testing against the real engine.** *The highest-value test in the suite.* Load *S(x)*
   into an actual body view whose scheme handler records **every** attempted load and whose bridge records
   **any** execution; assert zero unexpected loads and zero execution. This catches "the sanitizer
   believes it is safe, the engine disagrees", which is the entire mutation-XSS class.
3. **Dual-parser divergence.** Parse *S(x)* with the Rust parser and with the engine; any structural
   divergence is a candidate I8 failure. Run continuously over the whole
   [fidelity corpus](../product/reference-environment.md).
4. **Fuzzing** of the MIME parser and the sanitizer, seeded with the real-world corpus.
5. **Fixed regression vectors** — every published mutation-XSS payload as a permanent test case.

Methods 2 and 3 share code with the [per-message debug view](../runtime/observability.md). Build it once,
use it twice.

## Where review attention belongs

I6 through I10 have no independent backstop, and of those I8 carries the most risk alone. I9's
reformulation above removes an ambiguity but does not add a backstop — it still rests entirely on the
sanitizer being correct.

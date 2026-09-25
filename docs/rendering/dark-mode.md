# Dark mode

**Owns:** D-27, FR-31, FR-32, NFR-47.

The application shell follows the system light or dark appearance natively — free in both toolkits, no
decision needed. **Message bodies are the hard part**, and this document is about them.

## The problem has no industry-standard answer

Most HTML email hardcodes light-mode colours and ignores the system preference. Rendering it unchanged
inside a dark application is honest and ugly; transforming it is attractive and sometimes destroys logos
and inline imagery. No correct answer exists industry-wide. Sift picks a compromise deliberately and
writes it down here.

## FR-31, FR-32 — The compromise

**FR-31.** An **opt-in, default-off** dark-mode transform for message bodies, with a per-message toggle
and per-sender persistence.

Default-off because the failure mode of transforming is worse than the failure mode of not transforming: a
mangled brand header is a visible defect, while a light message in a dark window is merely unpleasant.

**FR-32.** Where a sender declares their own dark-mode rules, **honour them and do not transform**. A
growing minority of senders do this correctly, and fighting them produces worse results than trusting them.

## Why this is tractable here

A browser extension doing this fights a live style model with script. Sift has neither: email CSS is
static, inline or in style blocks, and there is no script. The transform is therefore a **static Rust pass
over a parsed CSS tree**, which is cheaper and far more testable. It runs last in
[the pipeline](pipeline.md), after sanitization and cosmetic filtering, and is bounded by NFR-41 there.

## D-27 — Resolve the full cascade

**Chosen:** implement CSS parsing, selector matching, specificity, importance, shorthand expansion,
media-query evaluation and inheritance — a real cascade — over the parsed DOM.
**Rejected:** parsing declarations without resolving a cascade; resolving only the colour-bearing subset
of properties.

**Why.** Three consumers need computed style, not declarations, and they are not the same three people
would guess. This transform needs **inherited** colours to build step 3's colour graph at all — a
foreground colour set on a container and relied upon six levels down is the common case in email, not an
edge case. Procedural cosmetic filters in [content blocking](content-blocking.md) match on computed style
by definition. And enumerating CSS fetching positions correctly means knowing which declarations actually
apply, since a blocked background image that was overridden anyway is a false positive in the
[debug view](../runtime/observability.md).

Resolving only colour-bearing properties was the tempting middle path. It fails because specificity and
importance are properties of the cascade as a whole: to know which `color` declaration wins you must
evaluate the selectors that carry every competing declaration, at which point most of the machinery exists
regardless.

**What it costs — and this is the largest single commitment in the rendering pipeline.** A cascade is a
style system in miniature, it is the kind of component whose correctness is measured against a
specification thousands of pages long, and it runs inside NFR-41's 30 ms budget **shared with the
sanitizer and the blocker**. It is comparable in size to the sanitizer itself and should be budgeted as
such rather than as a step in this pass.

What makes it tractable at all is what makes this whole transform tractable: email CSS is static, there is
no script, and nothing recomputes. It is one resolution pass, not a live style model.

**"Nothing recomputes" holds for every input except the viewport, and that exception is not a small one.**
D-27 resolves media queries, and a media query keys on viewport width. So the declarations that apply, the
colour graph built from them, and every override this pass generates are correct at the width they were
computed at and at no other. Resizing the reader pane moves the sender's own colours underneath overrides
that did not move with them, and step 5's contrast repair was checked against the pair that no longer
holds. NFR-47's 95% gate is measured at a single width and cannot see this.

Media queries are ubiquitous in HTML email, so this is the ordinary case rather than a corner.

**Answered: the body view is pinned to a fixed layout width, and the cascade resolves at that same
width.** The width is one constant of the render rather than a property of the window. The body view
MUST lay every body out at exactly that width whatever the reader pane's size — a pane wider than it
shows the column with room around it, a narrower one scrolls it sideways — and the cascade MUST evaluate
media queries at the same value. Resizing the reader moves the column and never reflows the document, so
the sender's colours, the colour graph, and every override stay the ones computed together, and "nothing
recomputes" holds for every input without exception. Magnification that scales the rendered page without
laying it out again is compatible with this; anything that changes the layout width is not.

It was preferred over the other two answers because it is the only one that costs nothing elsewhere.
Recomputing stages 3 through 6 on a debounced resize puts NFR-41's 30 ms budget onto an interactive drag
that no requirement bounds. Dropping media-query evaluation from the cascade weakens D-27's own argument
that a colour-only subset fails because specificity is a property of the cascade as a whole. And the pin
is already [D-50](webview-isolation.md)'s stated retreat for content height, so one mechanism serves both.

**What it costs:** a sender who designed for a narrow viewport sees their wide layout in every pane, and
a reader pane narrower than the pinned width scrolls sideways rather than reflowing. Both are the
sender's layout shown faithfully at one width rather than a broken one; a reader who wants reflow is
asking for the recompute answer and its unbounded drag cost. The value of the width is a hypothesis like
every other number here, and the one property that MUST hold is that the body view and the cascade use
the same one.

**Contestable because:** if NFR-41 is missed, this is the first thing to cut, and the retreat is real —
declarations-only with inheritance approximated for a fixed property list would serve the transform
poorly, procedural filters not at all, and would be a visibly worse product rather than a slower one. A
reader who thinks the budget is unachievable should say so before the cascade is written, not after.

## The pass

1. **Parse all CSS** — style blocks and inline style attributes — into a real tree. Not regular
   expressions; the whole point is operating on structure.
2. **Honour the sender first**, per FR-32. If sender dark rules exist, stop here.
3. **Build a colour graph** — resolved foreground, background, gradient stops, borders, shadows, and
   outlines per element, including inherited values.
4. **Transform in a perceptual colour space.** Invert and compress lightness along a tuned curve, clamp
   chroma to avoid neon artefacts, preserve hue. A perceptual space is chosen over the naive
   hue-saturation-lightness inversion used by browser extensions specifically because that approach
   muddies mid-tones and shifts hues.
5. **Repair contrast.** After transforming, check every text-on-background pair against a perceptual
   contrast model and nudge lightness until it passes. **This step is what separates "usable" from
   "technically inverted".** The threshold is raised where the system's increased-contrast preference is
   set — see [UI shell](../architecture/ui-shell.md), which owns the rest of that request.
6. **Classify images, which is where Sift can do better than an extension.** An extension cannot reliably
   sample pixels. Sift decodes in the broker — only when a classification is needed, which is here and
   nowhere else under [D-29](content-blocking.md) — so it can compute a histogram and alpha
   coverage, classify the image — photograph, transparent logo, logo baked onto white, screenshot, pixel —
   and act accordingly: never invert photographs, consider inverting transparent logos, leave or
   light-chip logos baked onto white. The classification is keyed **by content hash** and recorded durably
   in the shared blob index, so a given image is classified once ever rather than once per shed cycle. See
   [cache and blobs](../storage/cache-and-blobs.md).
7. **Provide an escape hatch.** A one-key per-message toggle back to the original, plus per-sender
   persistence.

## NFR-47 — Contrast gate

Transform output MUST meet the perceptual contrast threshold for at least 95% of the
[fidelity corpus](../product/reference-environment.md). Failures MUST be surfaced in the
[debug view](../runtime/observability.md) rather than silently shipped.

## Gradients and CSS-drawn decoration

**A gradient is transformed only when every one of its stops is near-neutral** — below a defined chroma
threshold in the perceptual space of step 4. If any stop carries real chroma, the whole gradient is left
alone.

The two failure modes are not symmetric, which is what decides this. Transforming a brand gradient mangles
a header the sender designed, and that is a visible defect the user attributes to Sift. Leaving a neutral
gradient produces a light band in a dark layout, which is merely ugly. FR-31 already resolves that
asymmetry in favour of not transforming, and this rule is the same judgement applied one level down.

Mixed-stop gradients are excluded by construction rather than by a special case: the rule tests every
stop, so a gradient that is half neutral and half brand fails it. The same threshold governs the other
CSS-drawn decoration step 3 enumerates — borders, shadows, and outlines.

A gradient supplied as an **image** is not covered by this rule at all. It reaches step 6's classifier
instead, and is handled as whatever the classifier decides it is.

The threshold itself is a number to be validated against the
[fidelity corpus](../product/reference-environment.md) under NFR-47, not a constant to be asserted here.

Legacy word-processor conditional content does **not** reach this pass. It is removed by the sanitizer,
which [sanitizer invariants](sanitizer-invariants.md) now states directly and asserts as a regression
vector rather than leaving as an assumption.

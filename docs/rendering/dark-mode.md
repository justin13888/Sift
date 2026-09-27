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
asking for the recompute answer and its unbounded drag cost. A narrow pane also hides the body's own
vertical scroll bar: the body view owns its vertical scrolling ([D-50](webview-isolation.md)), so that
bar sits at the pinned column's trailing edge, beyond the visible pane, until the reader scrolls
sideways to it. Wheel, trackpad, and keyboard scrolling still reach the body anywhere it is visible, and
the horizontal bar is always reachable; what is lost is a draggable vertical position indicator. A
native bar that mirrored the body's position would have to read that position, and without script
nothing public exposes it. The value of the width is a hypothesis like
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

### The gate's metric, its thresholds, and how they were derived

*Amended to record the values [D-64](../build/verification.md) registered as outstanding. The gate now
blocks.*

**Metric.** The WCAG 2 contrast ratio between a text-bearing element's resolved foreground and
background, after step 4 has transformed both and step 5 has repaired the foreground. A message **meets**
the gate when the transform ran and left no such pair below the threshold. The share is taken over the
messages the transform ran on: a message whose sender declared their own dark mode is outside it, because
FR-32 stops the pass before it produces anything to measure, and counting it either way would score Sift
on the sender's palette. A message whose styles were refused counts as a failure — the reader asked for
dark and was given the light original. A corpus on which the transform ran on no message leaves the gate
**unperformed**, which fails rather than passing vacuously.

**Thresholds.** **4.5** ordinarily, and **7** where the system's increased-contrast preference is set.

**How they were derived.** Both are the body-text levels the metric's own definition publishes — the
minimum and the enhanced — so neither is a number chosen here. The alternative was to derive them from the
corpus, as the smallest ratio any corpus text pair has as its sender sent it, on the rule D-64 applies to
NFR-26: the worst thing anybody accepted. It is rejected because the population is wrong. NFR-26's pairs
are ones a person reviewed and agreed; a sender's footer is not an agreement, and low-contrast footers are
part of what step 5 exists to repair. A threshold read off senders' worst pairs would certify the defect.
The corpus's role for these two values is the gate itself — at them, at least 95% of it must pass.

**Measured.** Over the corpus as admitted when this was recorded — seven constructed messages, one per
category and two for CJK — the sender of one declared their own dark mode, the transform ran on the other
six, and all six met both thresholds. **That result discriminates nothing yet, and is recorded as such:**
none of the six resolves an element carrying both a foreground and a background, so step 5 was presented
with no pair to repair at either threshold. Elements keep no class names through sanitization, so
stylesheet rules keyed on them match nothing, and the corpus's declared backgrounds sit on the one message that honours its own dark
mode. The gate gains evidence as captured messages are admitted beside the constructed ones under
[D-115](../product/reference-environment.md); a failure then is resolved by repairing the transform, not
by moving either threshold, and never in the run that failed.

**What would reopen it.** A contrast metric other than the WCAG 2 ratio — a perceptual lightness-contrast
model is the likely candidate — replaces both numbers with that metric's own, by amendment here.

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

**The threshold is a chroma of 0.02 in the perceptual space of step 4**, amended from a stated starting
value of 0.04. It is one just-noticeable difference in that space, as CSS Color 4's gamut mapping takes
it: a colour within one such difference of the grey at its own lightness cannot be told from that grey, so
calling it neutral changes nothing a reader could recognise as a brand colour, and anything above it is
visibly tinted and falls to the asymmetry above, which favours leaving it. The move is a consequence of the
derivation, not of a measurement: 0.04 was twice the difference with no reason for the factor, and lowering
it errs in the direction this section already chose.

**The [fidelity corpus](../product/reference-environment.md) falsifies it rather than choosing it.** The
value MUST fall above the chroma of every neutral colour the corpus draws and below that of every brand
colour. When this was recorded, the most tinted neutral was a cool-grey table border at 0.012 and the least
tinted brand colour a green call-to-action ground at 0.089, so the corpus bounds the value without picking
it — 0.04 satisfied it too — and the just-noticeable difference picks it inside the bound. A corpus colour
on the wrong side reopens the derivation rather than moving the number. No gradient, shadow or outline in
the corpus survives sanitization to reach the transform, so the falsifier was performed over plain colours
and borders only; a gradient admitted later is measured against it on arrival.

Legacy word-processor conditional content does **not** reach this pass. It is removed by the sanitizer,
which [sanitizer invariants](sanitizer-invariants.md) now states directly and asserts as a regression
vector rather than leaving as an assumption.

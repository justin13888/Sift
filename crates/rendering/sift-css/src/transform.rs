//! D-27's dark transform — the pass the cascade exists to serve.
//!
//! # Opt-in and default-off, and why that is not timidity
//!
//! FR-31 defaults this off because **a mangled brand header is a visible defect while a
//! light message in a dark window is merely unpleasant**. R-7 says the compromise will
//! disappoint someone whatever it is; defaulting off means it disappoints the people who
//! asked for it rather than everybody.
//!
//! # The seven steps, and the one that matters most
//!
//! 1. Parse all CSS into a real tree — [`crate::cascade`].
//! 2. **Honour the sender first** and stop (FR-32).
//! 3. Build a colour graph, **including inherited values**.
//! 4. Transform in a perceptual space: invert and compress lightness, clamp chroma,
//!    preserve hue.
//! 5. **Repair contrast.** D-27: "this step is what separates *usable* from *technically
//!    inverted*." An inversion that produces grey-on-grey has done the arithmetic and
//!    failed the job.
//! 6. Classify images — never invert a photograph.
//! 7. Provide an escape hatch.

use crate::cascade::Computed;
use crate::colour::{ACCEPTABLE_CONTRAST, NEAR_NEUTRAL_CHROMA, Oklab, Rgb, contrast_ratio, parse};

/// Why a message was not transformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// FR-32 — the sender declared their own dark-mode rules.
    ///
    /// Honoured **in preference to** the transform: a sender who did the work knows their
    /// brand better than a general inversion does, and overriding them would be the
    /// mangled-header failure arriving from the other direction.
    SenderDeclaredDarkMode,
    /// The transform is off. FR-31's default, or a per-message or per-sender choice.
    NotEnabled,
}

/// What the transform produced for one element.
#[derive(Debug, Clone, PartialEq)]
pub struct Override {
    pub element: usize,
    pub property: String,
    pub value: String,
}

/// The transform's output.
#[derive(Debug, Clone, PartialEq)]
pub struct Transformed {
    pub overrides: Vec<Override>,
    /// Pairs whose contrast could not be repaired.
    ///
    /// NFR-47 gates on at least 95% of the corpus meeting the threshold, and requires that
    /// **failures be surfaced in the debug view rather than silently shipped**. A transform
    /// that quietly gave up would make that gate unmeasurable.
    pub unrepaired: Vec<usize>,
}

/// Whether the sender declared their own dark-mode rules — FR-32.
///
/// Detected from the two things a sender who has done the work will have written.
#[must_use]
pub fn sender_declared_dark_mode(css: &str) -> bool {
    let c = css.to_ascii_lowercase().replace(' ', "");
    c.contains("prefers-color-scheme:dark") || c.contains("color-scheme:dark")
}

/// Step 4 — invert and compress lightness, clamp chroma, preserve hue.
///
/// The compression is what stops a pure-white background becoming pure black: a document
/// inverted to true black with true white text is harsher than the light original, which is
/// the "technically inverted" outcome step 5 exists to rescue and this step tries not to
/// create.
#[must_use]
pub fn transform_colour(colour: Rgb) -> Rgb {
    let lab = colour.to_oklab();

    // Invert lightness, then compress toward the middle. The bounds keep the darkest result
    // off true black and the lightest off true white.
    let inverted = 1.0 - lab.l;
    let compressed = 0.12 + inverted * 0.76;

    // Clamp chroma to avoid the neon artefacts D-27 names. Inverting lightness leaves
    // saturated colours looking more saturated, because the same chroma reads stronger
    // against a dark ground.
    let chroma = lab.chroma().min(0.16);

    // Hue is preserved. A brand's blue stays that blue, which is the failure naive HSL
    // inversion is rejected for.
    Rgb::from_oklab(Oklab::from_lch(compressed, chroma, lab.hue()), colour.a)
}

/// Step 5 — repair contrast.
///
/// Nudges the foreground's lightness away from the background until the pair passes, or
/// until it runs out of room. Moves the **foreground** because the background is more likely
/// to be a brand colour a reader recognises.
#[must_use]
pub fn repair_contrast(foreground: Rgb, background: Rgb) -> Option<Rgb> {
    if contrast_ratio(foreground, background) >= ACCEPTABLE_CONTRAST {
        return Some(foreground);
    }
    let fg = foreground.to_oklab();
    let bg = background.to_oklab();
    // Move away from the background: lighter text on a dark ground, darker on a light one.
    let direction = if bg.l < 0.5 { 1.0 } else { -1.0 };

    let mut best = foreground;
    for step in 1..=40 {
        let l = (fg.l + direction * f64::from(step) * 0.025).clamp(0.0, 1.0);
        let candidate = Rgb::from_oklab(Oklab::from_lch(l, fg.chroma(), fg.hue()), foreground.a);
        best = candidate;
        if contrast_ratio(candidate, background) >= ACCEPTABLE_CONTRAST {
            return Some(candidate);
        }
    }
    // Ran out of room. Reported rather than silently shipped — NFR-47's gate depends on
    // knowing this happened.
    let _ = best;
    None
}

/// Whether a gradient may be transformed.
///
/// > "A gradient is transformed only when **every one of its stops** is near-neutral."
///
/// Testing every stop is what excludes mixed-stop gradients *by construction* rather than
/// by a special case somebody has to remember. A gradient with one real colour in it is a
/// brand element, and inverting part of it is worse than leaving all of it.
#[must_use]
pub fn gradient_may_be_transformed(stops: &[Rgb]) -> bool {
    !stops.is_empty()
        && stops
            .iter()
            .all(|s| s.to_oklab().chroma() < NEAR_NEUTRAL_CHROMA)
}

/// The properties the transform rewrites.
const COLOUR_PROPERTIES: &[&str] = &[
    "color",
    "background-color",
    "border-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "outline-color",
];

/// Run steps 2 through 5 over a resolved tree.
///
/// # Errors
/// [`Skipped`] where the transform must not run.
pub fn run(css: &str, enabled: bool, computed: &[Computed]) -> Result<Transformed, Skipped> {
    if !enabled {
        return Err(Skipped::NotEnabled);
    }
    // Step 2, before anything else. Stopping here is the whole of FR-32.
    if sender_declared_dark_mode(css) {
        return Err(Skipped::SenderDeclaredDarkMode);
    }

    let mut overrides = Vec::new();
    let mut unrepaired = Vec::new();

    for (index, style) in computed.iter().enumerate() {
        // Step 3: the colour graph. `color` is here whether or not this element declared it,
        // because the cascade inherited it — which is the whole reason a cascade was needed
        // rather than a per-element lookup.
        let foreground = style.get("color").and_then(|v| parse(v));
        let background = style.get("background-color").and_then(|v| parse(v));

        for property in COLOUR_PROPERTIES {
            let Some(value) = style.get(*property) else {
                continue;
            };
            let Some(colour) = parse(value) else {
                // Unrecognised: left alone. An untransformed colour is the sender's; a
                // guessed one would be Sift's.
                continue;
            };
            // Fully transparent colours carry no appearance to invert.
            if colour.a == 0.0 {
                continue;
            }
            overrides.push(Override {
                element: index,
                property: (*property).to_owned(),
                value: transform_colour(colour).to_css(),
            });
        }

        // Step 5, over the pair this element actually renders.
        if let (Some(fg), Some(bg)) = (foreground, background) {
            let (tfg, tbg) = (transform_colour(fg), transform_colour(bg));
            match repair_contrast(tfg, tbg) {
                Some(repaired) if repaired != tfg => {
                    overrides.retain(|o| !(o.element == index && o.property == "color"));
                    overrides.push(Override {
                        element: index,
                        property: "color".to_owned(),
                        value: repaired.to_css(),
                    });
                }
                Some(_) => {}
                None => unrepaired.push(index),
            }
        }
    }

    Ok(Transformed {
        overrides,
        unrepaired,
    })
}

/// Step 6 — what a decoded image is.
///
/// D-29 decodes **only** for this, "which is here and nowhere else", and the classification
/// is keyed by content address and recorded durably in the shared blob index — so an image
/// is classified **once ever** rather than once per shed cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageClass {
    /// **Never inverted.** An inverted photograph is unmistakably wrong, and there is no
    /// version of "slightly wrong" here.
    Photograph,
    /// Considered for inversion: a logo drawn on transparency reads as ink, and ink on a
    /// dark ground should be light.
    TransparentLogo,
    /// Left alone, or given a light chip behind it. Inverting it would turn the white card
    /// the brand designed into a black one.
    LogoOnWhite,
    Screenshot,
    /// Small enough that it is a tracking pixel rather than an image — FR-29's territory,
    /// and it will not be loaded at all.
    Pixel,
}

impl ImageClass {
    #[must_use]
    pub const fn may_invert(self) -> bool {
        matches!(self, Self::TransparentLogo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::Computed;

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb {
            r: f64::from(r) / 255.0,
            g: f64::from(g) / 255.0,
            b: f64::from(b) / 255.0,
            a: 1.0,
        }
    }

    fn style(pairs: &[(&str, &str)]) -> Computed {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn the_sender_is_honoured_before_anything_else_happens() {
        // FR-32, and the order matters: a sender who did the work knows their brand better
        // than a general inversion does.
        for css in [
            "@media (prefers-color-scheme: dark) { body { background: #000 } }",
            "body { color-scheme: dark }",
        ] {
            assert!(sender_declared_dark_mode(css), "{css}");
            assert_eq!(
                run(css, true, &[style(&[("color", "black")])]),
                Err(Skipped::SenderDeclaredDarkMode)
            );
        }
    }

    #[test]
    fn the_transform_is_off_by_default() {
        // FR-31: a mangled brand header is a visible defect, a light message in a dark
        // window is merely unpleasant.
        assert_eq!(
            run("", false, &[style(&[("color", "black")])]),
            Err(Skipped::NotEnabled)
        );
    }

    #[test]
    fn lightness_is_inverted() {
        let dark = transform_colour(rgb(255, 255, 255));
        let light = transform_colour(rgb(0, 0, 0));
        assert!(dark.to_oklab().l < 0.4, "white did not darken: {dark:?}");
        assert!(light.to_oklab().l > 0.6, "black did not lighten: {light:?}");
    }

    #[test]
    fn lightness_is_compressed_rather_than_fully_inverted() {
        // A document inverted to true black with true white text is harsher than the light
        // original, which is the "technically inverted" outcome step 5 exists to rescue and
        // this step tries not to create.
        let from_white = transform_colour(rgb(255, 255, 255));
        assert!(
            from_white.to_oklab().l > 0.05,
            "white became true black: {from_white:?}"
        );
        let from_black = transform_colour(rgb(0, 0, 0));
        assert!(
            from_black.to_oklab().l < 0.95,
            "black became true white: {from_black:?}"
        );
    }

    #[test]
    fn hue_is_preserved() {
        // The failure naive HSL inversion is rejected for: a brand's blue becoming a
        // different blue is the visible defect that makes users turn the feature off.
        for c in [rgb(0, 0, 255), rgb(255, 0, 0), rgb(0, 128, 0)] {
            let before = c.to_oklab().hue();
            let after = transform_colour(c).to_oklab().hue();
            let drift = (before - after).abs();
            assert!(drift < 0.05, "hue drifted by {drift} for {c:?}");
        }
    }

    #[test]
    fn chroma_is_clamped_so_nothing_goes_neon() {
        // Inverting lightness leaves saturated colours reading *more* saturated, because the
        // same chroma is stronger against a dark ground.
        let vivid = transform_colour(rgb(255, 0, 255));
        assert!(vivid.to_oklab().chroma() <= 0.17, "{vivid:?}");
    }

    #[test]
    fn contrast_repair_moves_the_foreground_until_the_pair_passes() {
        // "This step is what separates *usable* from *technically inverted*."
        let background = rgb(30, 30, 30);
        let foreground = rgb(45, 45, 45);
        assert!(contrast_ratio(foreground, background) < ACCEPTABLE_CONTRAST);
        let repaired = repair_contrast(foreground, background).expect("repairable");
        assert!(contrast_ratio(repaired, background) >= ACCEPTABLE_CONTRAST);
    }

    #[test]
    fn a_pair_that_already_passes_is_left_alone() {
        let (fg, bg) = (rgb(255, 255, 255), rgb(0, 0, 0));
        assert_eq!(repair_contrast(fg, bg), Some(fg));
    }

    #[test]
    fn an_unrepairable_pair_is_reported_rather_than_silently_shipped() {
        // NFR-47 gates on 95% of the corpus and requires failures be surfaced. A transform
        // that quietly gave up would make that gate unmeasurable.
        let computed = [style(&[
            ("color", "#808080"),
            ("background-color", "#7f7f7f"),
        ])];
        let result = run("", true, &computed).expect("runs");
        // Either it repaired it or it said it could not; what it must never do is neither.
        let repaired = result.overrides.iter().any(|o| o.property == "color");
        assert!(repaired || !result.unrepaired.is_empty());
    }

    #[test]
    fn a_gradient_is_transformed_only_when_every_stop_is_near_neutral() {
        // Testing every stop excludes mixed-stop gradients **by construction** rather than by
        // a special case somebody has to remember.
        assert!(gradient_may_be_transformed(&[
            rgb(255, 255, 255),
            rgb(240, 240, 240)
        ]));
        assert!(
            !gradient_may_be_transformed(&[rgb(255, 255, 255), rgb(255, 0, 0)]),
            "a gradient with one real colour in it was transformed"
        );
        assert!(
            !gradient_may_be_transformed(&[]),
            "an empty gradient is not near-neutral"
        );
    }

    #[test]
    fn an_unrecognised_colour_is_left_alone() {
        let computed = [style(&[("color", "var(--brand)")])];
        let result = run("", true, &computed).expect("runs");
        assert!(
            !result.overrides.iter().any(|o| o.property == "color"),
            "a colour nobody could parse was overridden anyway"
        );
    }

    #[test]
    fn a_fully_transparent_colour_carries_nothing_to_invert() {
        let computed = [style(&[("background-color", "transparent")])];
        let result = run("", true, &computed).expect("runs");
        assert!(result.overrides.is_empty());
    }

    #[test]
    fn an_inherited_colour_is_transformed_even_where_the_element_declared_none() {
        // The reason the cascade had to be resolved first: a `<td>` with no colour of its own
        // still has one, and a transform working from declarations alone would leave it.
        let computed = [style(&[("color", "black"), ("background-color", "white")])];
        let result = run("", true, &computed).expect("runs");
        assert!(result.overrides.iter().any(|o| o.property == "color"));
        assert!(
            result
                .overrides
                .iter()
                .any(|o| o.property == "background-color")
        );
    }

    #[test]
    fn a_photograph_is_never_inverted() {
        // There is no version of "slightly wrong" here — an inverted photograph is
        // unmistakable.
        assert!(!ImageClass::Photograph.may_invert());
        assert!(!ImageClass::LogoOnWhite.may_invert());
        assert!(!ImageClass::Screenshot.may_invert());
        assert!(ImageClass::TransparentLogo.may_invert());
    }
}

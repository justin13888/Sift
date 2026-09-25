//! The seven stages, composed — and the order is the whole design.
//!
//! ```text
//! 1 parse  →  2 select  →  3 SANITIZE  →  4 cosmetic filter  →  5 bind  →  6 transform  →  7 render
//! ```
//!
//! `docs/rendering/pipeline.md` makes the order normative and gives three reasons that are
//! each a defect if the order changes:
//!
//! - **Sanitization precedes filtering**, so the filter works on structure that can be
//!   trusted. A cosmetic rule matched against unsanitized markup is a rule matched against
//!   whatever the sender wanted it to match.
//! - **Rewriting happens inside sanitization**, not after it. I2 is asserted over the output
//!   of stage 3, so an external-scheme URL that survived the stage would falsify the
//!   invariant while every later pass still behaved.
//! - **The transform runs last**, so it cannot reintroduce what earlier stages removed. A
//!   dark-mode pass that could add a fetching position would be a hole in the broker's claim
//!   to see every one of them.
//!
//! **Any stage added later enters above stage 3.** D-39's decryption stage is the named
//! example: it goes between 1 and 2, so decrypted content passes through sanitization,
//! blocking and rewriting exactly as plaintext does and gains no exemption from I1–I10.
//!
//! # Where this crate sits
//!
//! In the rendering layer, which "may not reach the store, the network, or the adapters".
//! Nothing here fetches anything. The broker is the one component with an edge outward and
//! it expresses that edge as a decision, not as a request.
//!
//! # Failure is not an error
//!
//! Every way this can fail ends in the same place: FR-9's raw source view. NFR-19 already
//! requires parse failure to degrade there, so the path exists, is tested, and is the one a
//! user already meets on malformed mail. Nothing here truncates — `docs/limits.md` gives the
//! reason, and it is the reason I8 exists.

use sift_block::engine::{Authority, Decision};
use sift_block::origin::Origin;
use sift_broker::broker::{Broker, Position};
use sift_broker::token::Token;
use sift_sanitize::sanitize::{FetchingPosition, Removal, SanitizeError};

/// What stage 2 chose — the seam between "what the provider gave us" and "what gets
/// rendered".
///
/// It is a type rather than a step inside [`render`] because the two providers Sift has
/// reach it differently: one hands over the message's own bytes and stages 1 and 2 run here,
/// and one hands over a parsed structure whose parts are already separated, so stage 2 has
/// already happened by the time Sift sees it. Both arrive at the same value, and stage 3
/// cannot tell which produced it — which is what keeps the hostile-input claim true for both.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selected {
    pub html: Option<String>,
    pub text: Option<String>,
    /// Why this part rather than another, for FR-33.
    pub reason: Option<String>,
}

impl Selected {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.html.is_none() && self.text.is_none()
    }
}

/// Why a message fell back to the raw view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Stage 1 — the message did not parse inside `docs/limits.md`'s bounds.
    Parse(String),
    /// Stage 2 — nothing renderable.
    NothingToRender,
    /// Stage 3 — a bound in the limits register.
    Sanitize(SanitizeError),
}

impl core::fmt::Display for Failure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Parse(why) => write!(f, "the message did not parse: {why}"),
            Self::NothingToRender => write!(f, "the message carries no renderable part"),
            Self::Sanitize(e) => write!(f, "the document exceeded a bound: {e:?}"),
        }
    }
}

/// What the reader is given.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The document, with every fetching position rewritten to the internal scheme.
    pub html: String,
    /// D-90: minted **per document** and revoked at navigation — earlier and more often than
    /// teardown. Reuse of the view is safe only because script, storage and network are all
    /// absent; relax any one and this loses its footing.
    pub token: Token,
    /// Every fetching position, in document order — **including the ones a filter rule
    /// condemns**. I2 is asserted over this, and a condemned URL left unrewritten would
    /// falsify the invariant while every blocking test still passed.
    pub positions: Vec<FetchingPosition>,
    /// What the filter said about each, in step with `positions`.
    pub verdicts: Vec<Decision>,
    /// Navigation targets. **Not fetching positions**: a link is followed on an explicit
    /// confirmation and never in place.
    pub links: Vec<String>,
    /// What was removed, and under which rule — FR-33 item 4.
    pub removals: Vec<Removal>,
    /// Which stages ran, in order, for the debug view.
    pub stages: Vec<&'static str>,
    /// Whether the dark transform ran, and why not where it did not.
    pub transform: TransformOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransformOutcome {
    NotRequested,
    /// The sender declared their own dark mode. **Honoured first, and then stop** — a second
    /// transform on top of one the sender already applied is how a readable message becomes
    /// unreadable.
    SenderDeclaredTheirOwn,
    /// The CSS did not parse inside L-9. The message renders untransformed rather than
    /// half-transformed, which is the same rule the parse limits follow: a document
    /// transformed to the point the cap fell is one whose appearance the sender chose.
    CssRefused,
    Ran {
        /// How many declarations the transform replaced.
        overrides: usize,
        /// Pairs whose contrast could not be repaired. **Surfaced rather than silently
        /// shipped** — NFR-47 gates on 95% of a corpus, and a transform that quietly gave up
        /// would make that gate unmeasurable.
        unrepaired: usize,
    },
}

/// What the pipeline is run against.
#[derive(Debug)]
pub struct Context<'a> {
    /// D-11's synthetic origin. The per-sender allowlist keys on this and never on a
    /// displayed sender — which is why "always show images from this sender" **cannot
    /// exist** for a null-origin message.
    pub origin: Origin,
    /// D-10's authority. **An absent authority denies**: with no engine loaded every remote
    /// fetch is refused and the reason names the shed, rather than falling through to the
    /// backstop.
    pub blocker: Option<&'a Authority>,
    pub dark: bool,
    /// The system's increased-contrast preference. Raises the threshold the dark
    /// transform's contrast repair targets: a user who asked the system for more contrast
    /// has not asked for it everywhere except inside the message. Meaningless while `dark`
    /// is off, because the repair is a step of the transform.
    pub increased_contrast: bool,
    pub broker: &'a mut Broker,
}

/// Stages 1 and 2, for a provider that hands over the message's own bytes.
///
/// # Errors
/// [`Failure::Parse`] on anything outside L-1 through L-5, and
/// [`Failure::NothingToRender`] where no part is renderable.
pub fn select_from_mime(raw: &[u8]) -> Result<Selected, Failure> {
    let message = sift_mime::parse::parse(raw).map_err(|e| Failure::Parse(format!("{e:?}")))?;
    let selection = sift_mime::select::select(&message);
    let reason = Some(format!("{:?}", selection.reason));
    let chosen = selection.chosen.ok_or(Failure::NothingToRender)?;
    // A range rather than the bytes — "structure first" refuses to materialise every part,
    // so the chosen one is the only slice this takes.
    let slice = raw.get(chosen.body.clone()).unwrap_or_default();
    let body = String::from_utf8_lossy(slice).into_owned();
    let is_html = chosen.content_type().starts_with("text/html");
    Ok(Selected {
        html: is_html.then_some(body.clone()),
        text: (!is_html).then_some(body),
        reason,
    })
}

/// Stages 3 through 7.
///
/// # Errors
/// [`Failure::Sanitize`] on a bound in the limits register, and
/// [`Failure::NothingToRender`] where stage 2 chose nothing.
pub fn render(selected: &Selected, context: &mut Context<'_>) -> Result<Rendered, Failure> {
    let mut stages = Vec::new();

    // Plain text is escaped rather than parsed, and then travels the same road: a `text/plain`
    // part that reached the sanitizer as markup would be a way to author HTML in a part the
    // user was told was plain.
    let source = match (&selected.html, &selected.text) {
        (Some(html), _) => html.clone(),
        (None, Some(text)) => format!("<pre>{}</pre>", escape(text)),
        (None, None) => return Err(Failure::NothingToRender),
    };

    // ---- 3. Sanitize. Rewriting happens here, inside the stage I2 is asserted over.
    let sanitized = sift_sanitize::sanitize::sanitize(&source).map_err(Failure::Sanitize)?;
    stages.push("sanitize");

    // ---- 4. Cosmetic filter, on structure that can now be trusted.
    let source_domain = context.origin.domain().unwrap_or("").to_owned();
    let verdicts: Vec<Decision> = sanitized
        .positions
        .iter()
        .map(|p| decide(context.blocker, &p.original, &source_domain))
        .collect();
    stages.push("filter");

    // ---- 5. Bind. D-90: the token is minted per *document*.
    let positions: Vec<Position> = sanitized
        .positions
        .iter()
        .map(|p| Position {
            url: p.original.clone(),
            declared_length: None,
            declared_width: None,
            declared_height: None,
            declared_pixels: None,
            style: None,
            alt: None,
            in_zero_height_container: false,
            request_type: "image".to_owned(),
        })
        .collect();
    let token = context
        .broker
        .open_document(context.origin.clone(), positions);
    stages.push("bind");

    // ---- 6. Transform. Last, so it cannot reintroduce what earlier stages removed, and
    // over the *sanitized* document rather than the sender's — the cascade resolves against
    // what will actually be rendered.
    let document = sift_sanitize::document::read(&sanitized.html);
    let (transform, overrides) =
        dark_transform(&document, context.dark, context.increased_contrast);
    stages.push("transform");

    // ---- 7. Render. The overrides are appended as a last stylesheet rather than written
    // into the elements: an author-origin declaration Sift added would be indistinguishable
    // from one the sender wrote, and FR-33 could not then show what the transform did.
    let mut html = sanitized.html;
    if !overrides.is_empty() {
        html.push_str(&overrides);
    }
    stages.push("render");

    Ok(Rendered {
        html,
        token,
        positions: sanitized.positions,
        verdicts,
        links: sanitized.links,
        removals: sanitized.removals,
        stages,
        transform,
    })
}

/// D-10: the filter engine is the authority, and **an absent authority denies**.
///
/// With no engine loaded every remote fetch is refused and the reason names the shed. It does
/// not fall through to the backstop, and it does not reload forty megabytes in response to a
/// pressure signal — the two failures the rule exists to prevent, and the second is worse
/// because it is a memory spike triggered by memory pressure.
fn decide(authority: Option<&Authority>, url: &str, source_domain: &str) -> Decision {
    match authority {
        Some(engine) => engine.decide(url, source_domain, "image"),
        None => Decision::AbsentAuthority,
    }
}

/// D-27's steps 2 through 5, over the document that will be rendered.
///
/// The seven steps are the decision's, and the ones that live here are the composition:
/// **honour the sender first and stop** (step 2), build the colour graph from a resolved
/// cascade rather than from the declarations an element wrote (step 3, and the reason a
/// cascade was needed at all — `color` is inherited), transform in a perceptual space
/// (step 4) and repair contrast (step 5, "the step separating usable from
/// technically-inverted").
fn dark_transform(
    document: &sift_sanitize::document::Document,
    enabled: bool,
    increased_contrast: bool,
) -> (TransformOutcome, String) {
    if !enabled {
        return (TransformOutcome::NotRequested, String::new());
    }
    // Q-15's assumption: the body view is pinned to a fixed layout width, so the resolved
    // cascade does not depend on a viewport the transform cannot see. It is also D-50's
    // stated retreat if the non-script height mechanism fails, so one mechanism serves both.
    let viewport = sift_css::cascade::Viewport::default();
    // FR-32 is read off the **sanitized** stylesheet rather than the sender's. Honouring a
    // declaration that did not survive would leave the message untransformed for a reason
    // nothing in it explains.
    if sift_sanitize::transform_input_declares_dark_mode_in(&document.stylesheet) {
        return (TransformOutcome::SenderDeclaredTheirOwn, String::new());
    }
    let Ok(sheet) = sift_css::cascade::Stylesheet::parse(&document.stylesheet, viewport) else {
        return (TransformOutcome::CssRefused, String::new());
    };
    let elements: Vec<CascadeElement> = document
        .elements
        .iter()
        .map(|e| CascadeElement(e.clone()))
        .collect();
    let inline: Vec<Option<String>> = document
        .elements
        .iter()
        .map(|e| e.inline_style.clone())
        .collect();
    let computed = sift_css::cascade::resolve(&sheet, &inline, &elements, viewport);

    match sift_css::transform::run(&document.stylesheet, true, increased_contrast, &computed) {
        Err(sift_css::transform::Skipped::SenderDeclaredDarkMode) => {
            (TransformOutcome::SenderDeclaredTheirOwn, String::new())
        }
        Err(sift_css::transform::Skipped::NotEnabled) => {
            (TransformOutcome::NotRequested, String::new())
        }
        Ok(transformed) => {
            let css = as_stylesheet(&transformed.overrides, &document.elements);
            (
                TransformOutcome::Ran {
                    overrides: transformed.overrides.len(),
                    unrepaired: transformed.unrepaired.len(),
                },
                css,
            )
        }
    }
}

/// Turn the transform's per-element overrides into one appended stylesheet.
///
/// Each element is addressed by **the handle the sanitizer stamped on it**, not by a
/// selector guessed from its tag and classes. A guessed selector would apply to elements the
/// transform never examined, which is how a dark mode ends up inverting half a document.
///
/// An element with no handle is skipped rather than addressed approximately. In practice
/// that is the scaffolding a parser invents — `html`, `head`, `body` — which the sanitizer
/// unwrapped and which therefore has no appearance of the sender's to transform.
fn as_stylesheet(
    overrides: &[sift_css::transform::Override],
    elements: &[sift_sanitize::document::Element],
) -> String {
    let mut declarations = String::new();
    for over in overrides {
        let Some(handle) = elements
            .get(over.element)
            .and_then(|e| e.attribute(sift_sanitize::sanitize::ELEMENT_HANDLE))
        else {
            continue;
        };
        // The handle is an index this pipeline wrote, so it is digits. Checked rather than
        // trusted, because the alternative is a selector built from an attribute value that
        // reached here from a document.
        if !handle.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        declarations.push_str(&format!(
            "[{}=\"{handle}\"]{{{}:{} !important}}",
            sift_sanitize::sanitize::ELEMENT_HANDLE,
            over.property,
            over.value
        ));
    }
    if declarations.is_empty() {
        return String::new();
    }
    format!("<style>{declarations}</style>")
}

/// The cascade's element view over a read-back document.
struct CascadeElement(sift_sanitize::document::Element);

impl sift_css::selector::Element for CascadeElement {
    fn tag(&self) -> &str {
        &self.0.tag
    }
    fn id(&self) -> Option<&str> {
        self.0.id.as_deref()
    }
    fn classes(&self) -> &[String] {
        &self.0.classes
    }
    fn attribute(&self, name: &str) -> Option<&str> {
        self.0.attribute(name)
    }
    fn parent(&self) -> Option<usize> {
        self.0.parent
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The stage order, as a value, so a test can assert it rather than a reader trusting it.
pub const STAGES: &[&str] = &[
    "parse",
    "select",
    "sanitize",
    "filter",
    "bind",
    "transform",
    "render",
];

/// Where a stage added later must enter.
///
/// **Above stage 3.** D-39's decryption stage is the named example: between 1 and 2, so
/// decrypted content passes through sanitization, blocking and rewriting exactly as
/// plaintext does and gains no exemption from I1–I10.
#[must_use]
pub fn a_new_stage_enters_above(index: usize) -> bool {
    index < 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_block::origin::Authentication;

    fn origin() -> Origin {
        Origin::derive(&Authentication {
            signing_domain: Some("example.test".into()),
            envelope_domain: Some("example.test".into()),
            sender_policy_passed: true,
            from_domain: Some("example.test".into()),
        })
    }

    fn context(broker: &mut Broker) -> Context<'_> {
        Context {
            origin: origin(),
            blocker: None,
            dark: false,
            increased_contrast: false,
            broker,
        }
    }

    fn html(body: &str) -> Selected {
        Selected {
            html: Some(body.to_owned()),
            text: None,
            reason: None,
        }
    }

    #[test]
    fn the_stage_order_is_the_documented_one() {
        assert_eq!(
            STAGES,
            [
                "parse",
                "select",
                "sanitize",
                "filter",
                "bind",
                "transform",
                "render"
            ]
        );
    }

    #[test]
    fn a_new_stage_enters_above_the_sanitizer() {
        // D-39's decryption stage goes between 1 and 2. A stage below the sanitizer would be
        // one whose output never passed through it.
        assert!(a_new_stage_enters_above(1), "between parse and select");
        assert!(!a_new_stage_enters_above(3), "below the sanitizer");
    }

    #[test]
    fn a_rendered_document_records_the_stages_it_ran() {
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(&html("<p>hello</p>"), &mut ctx).unwrap();
        assert_eq!(
            out.stages,
            ["sanitize", "filter", "bind", "transform", "render"]
        );
    }

    #[test]
    fn filtering_happens_after_sanitization_and_binding_after_filtering() {
        // The order is the design. A cosmetic rule matched against unsanitized markup is a
        // rule matched against whatever the sender wanted it to match.
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(&html("<img src='https://tracker.test/x.gif'>"), &mut ctx).unwrap();
        let sanitize = out.stages.iter().position(|s| *s == "sanitize").unwrap();
        let filter = out.stages.iter().position(|s| *s == "filter").unwrap();
        let bind = out.stages.iter().position(|s| *s == "bind").unwrap();
        let transform = out.stages.iter().position(|s| *s == "transform").unwrap();
        assert!(sanitize < filter && filter < bind && bind < transform);
    }

    #[test]
    fn every_fetching_position_is_rewritten_before_anybody_decides_about_it() {
        // I2: no external-scheme URL survives stage 3, *including* one a filter rule
        // condemns. A condemned URL left unrewritten would falsify the invariant while every
        // blocking test still passed.
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(
            &html("<img src='https://tracker.test/pixel.gif'><img src='https://ok.test/a.png'>"),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(out.positions.len(), 2);
        assert!(!out.html.contains("https://"), "{}", out.html);
        assert!(
            out.html
                .contains(sift_foundation::identifiers::INTERNAL_SCHEME)
        );
    }

    #[test]
    fn with_no_filter_engine_loaded_every_position_is_refused_with_a_reason() {
        // D-10: an absent authority denies. It does not fall through to the backstop, and it
        // does not reload forty megabytes in response to a pressure signal.
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(&html("<img src='https://anything.test/a.png'>"), &mut ctx).unwrap();
        assert_eq!(out.verdicts[0], Decision::AbsentAuthority);
        assert!(
            !out.verdicts[0].permits_fetch(),
            "an absent authority allowed a fetch"
        );
    }

    #[test]
    fn a_document_gets_a_token_and_a_second_document_gets_a_different_one() {
        // D-90: minted per document, so revoking one says nothing about the other.
        let mut broker = Broker::new();
        let first = {
            let mut ctx = context(&mut broker);
            render(&html("<p>a</p>"), &mut ctx).unwrap().token
        };
        let mut ctx = context(&mut broker);
        let second = render(&html("<p>b</p>"), &mut ctx).unwrap().token;
        assert_ne!(first.as_str(), second.as_str());
    }

    #[test]
    fn a_plain_text_part_cannot_author_markup() {
        // A `text/plain` part that reached the sanitizer as markup would be a way to write
        // HTML into a part the user was told was plain.
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(
            &Selected {
                html: None,
                text: Some("<img src=x onerror=alert(1)>".into()),
                reason: None,
            },
            &mut ctx,
        )
        .unwrap();
        assert!(out.positions.is_empty(), "plain text produced a fetch");
        assert!(out.html.contains("&lt;img"), "{}", out.html);
    }

    #[test]
    fn a_sender_who_declared_their_own_dark_mode_is_honoured_and_then_left_alone() {
        // A second transform on top of one the sender already applied is how a readable
        // message becomes unreadable.
        let mut broker = Broker::new();
        let mut ctx = Context {
            dark: true,
            ..context(&mut broker)
        };
        let out = render(
            &html("<style>@media (prefers-color-scheme: dark) { body { color: #fff } }</style><p>x</p>"),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(out.transform, TransformOutcome::SenderDeclaredTheirOwn);
    }

    #[test]
    fn a_message_with_nothing_renderable_degrades_rather_than_rendering_nothing() {
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        assert_eq!(
            render(&Selected::default(), &mut ctx).err(),
            Some(Failure::NothingToRender)
        );
    }

    #[test]
    fn a_document_past_a_bound_is_refused_rather_than_truncated() {
        // docs/limits.md: a document cut mid-tree is one whose structure the sender chose by
        // choosing where the cap fell, which is the parse-differential primitive I8 exists
        // to close.
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let deep = "<div>".repeat(600);
        let out = render(&html(&deep), &mut ctx);
        assert!(matches!(out, Err(Failure::Sanitize(_))), "{out:?}");
    }

    #[test]
    fn a_link_is_not_a_fetching_position() {
        // A link is followed on an explicit confirmation and never in place, so it is not
        // something the broker resolves.
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(&html("<a href='https://example.test/x'>go</a>"), &mut ctx).unwrap();
        assert_eq!(out.links.len(), 1);
        assert!(out.positions.is_empty());
    }

    #[test]
    fn stages_one_and_two_choose_the_html_alternative_and_say_why() {
        let raw = b"From: a@x.test\r\n\
Content-Type: multipart/alternative; boundary=b\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
plain version\r\n\
--b\r\n\
Content-Type: text/html\r\n\
\r\n\
<p>html version</p>\r\n\
--b--\r\n";
        let selected = select_from_mime(raw).expect("the message did not parse");
        assert!(selected.html.as_deref().unwrap().contains("html version"));
        assert!(selected.text.is_none());
        assert!(selected.reason.is_some(), "FR-33 needs the reason");
    }

    #[test]
    fn a_message_that_does_not_parse_degrades_rather_than_panicking() {
        // NFR-19: parse failure goes to FR-9's raw view, which is the path a user already
        // meets on malformed mail.
        for hostile in [
            &b""[..],
            &[0xff; 8],
            b"Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\n",
        ] {
            let _ = select_from_mime(hostile);
        }
    }

    #[test]
    fn the_transform_addresses_elements_by_the_handle_the_sanitizer_stamped() {
        // A selector guessed from a tag and a class would apply to elements the transform
        // never examined, which is how a dark mode inverts half a document.
        let mut broker = Broker::new();
        let mut ctx = Context {
            dark: true,
            ..context(&mut broker)
        };
        let out = render(
            &html("<style>p{color:#111111;background-color:#ffffff}</style><p>x</p><div>y</div>"),
            &mut ctx,
        )
        .unwrap();
        let TransformOutcome::Ran { overrides, .. } = out.transform else {
            panic!("the transform did not run: {:?}", out.transform);
        };
        assert!(overrides > 0, "the cascade resolved nothing to transform");
        assert!(
            out.html.contains(sift_sanitize::sanitize::ELEMENT_HANDLE),
            "{}",
            out.html
        );
    }

    #[test]
    fn a_light_background_does_not_become_pure_black() {
        // The compression D-27's step 4 exists for. A document that inverted to pure black
        // would be the "technically inverted" failure its step 5 names.
        let mut broker = Broker::new();
        let mut ctx = Context {
            dark: true,
            ..context(&mut broker)
        };
        let out = render(
            &html("<style>p{color:#000000;background-color:#ffffff}</style><p>x</p>"),
            &mut ctx,
        )
        .unwrap();
        assert!(!out.html.contains("rgb(0, 0, 0)"), "{}", out.html);
    }

    #[test]
    fn the_increased_contrast_preference_reaches_the_repair_inside_the_message() {
        // The UI shell: a user who asked the system for more contrast has not asked for it
        // everywhere except inside the message. `#383838` on white transforms to a pair
        // that clears the ordinary threshold and not the raised one, so only the raised
        // render repairs the foreground — which changes the appended stylesheet.
        let body = "<style>p{color:#383838;background-color:#ffffff}</style><p>x</p>";
        let mut broker = Broker::new();
        let ordinary = render(
            &html(body),
            &mut Context {
                dark: true,
                ..context(&mut broker)
            },
        )
        .unwrap();
        let raised = render(
            &html(body),
            &mut Context {
                dark: true,
                increased_contrast: true,
                ..context(&mut broker)
            },
        )
        .unwrap();
        for out in [&ordinary, &raised] {
            assert!(
                matches!(out.transform, TransformOutcome::Ran { unrepaired: 0, .. }),
                "{:?}",
                out.transform
            );
        }
        assert_ne!(
            ordinary.html, raised.html,
            "the preference did not change what the transform produced"
        );
    }

    #[test]
    fn nothing_is_transformed_when_dark_mode_is_off() {
        let mut broker = Broker::new();
        let mut ctx = context(&mut broker);
        let out = render(&html("<style>p{color:#111}</style><p>x</p>"), &mut ctx).unwrap();
        assert_eq!(out.transform, TransformOutcome::NotRequested);
    }
}

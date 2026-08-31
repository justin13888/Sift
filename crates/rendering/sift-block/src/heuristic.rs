//! FR-29 — email-specific heuristics, independent of filter-list coverage.
//!
//! > "**Public filter lists target web advertising and cover email tracking poorly.**"
//!
//! So these run whether or not a list matched, and **every heuristic block logs its
//! reason** — which is FR-33 item 5's requirement and also the only way a user can tell a
//! heuristic from a rule when something they wanted did not load.

use crate::origin::Origin;

/// What a heuristic concluded, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub reason: Reason,
    /// The specific evidence, for FR-33. A reason with no evidence is an accusation.
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Declared or intrinsic dimensions of at most two pixels.
    PixelDimensions,
    /// Hidden by `display`, `visibility`, or zero opacity.
    HiddenByStyle,
    /// A path carrying a high-entropy per-recipient token.
    ///
    /// The same signal FR-42 names as evidence of tracking in an unsubscribe destination —
    /// which is why Sift shows that destination and never requests it.
    PerRecipientToken,
    /// No alternative text, inside a zero-height container.
    ///
    /// Two weak signals that are strong together: a legitimate decorative image has one or
    /// the other, and a tracker has both because it is not meant to be seen or announced.
    UnannouncedAndInvisible,
}

impl Reason {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PixelDimensions => "tracking-pixel dimensions",
            Self::HiddenByStyle => "hidden by style",
            Self::PerRecipientToken => "per-recipient token in the address",
            Self::UnannouncedAndInvisible => "no alternative text in a zero-height container",
        }
    }
}

/// What is known about one candidate image.
#[derive(Debug, Clone, Default)]
pub struct Candidate<'a> {
    pub url: &'a str,
    pub declared_width: Option<u32>,
    pub declared_height: Option<u32>,
    pub style: Option<&'a str>,
    pub alt: Option<&'a str>,
    /// Whether an ancestor collapses this to zero height.
    pub in_zero_height_container: bool,
}

/// Run the heuristics.
///
/// Returns every finding rather than the first, because the debug view shows what was
/// concluded and a single reason would hide the others.
#[must_use]
pub fn examine(candidate: &Candidate<'_>, origin: &Origin) -> Vec<Finding> {
    let mut findings = Vec::new();

    if let (Some(w), Some(h)) = (candidate.declared_width, candidate.declared_height)
        && w <= 2
        && h <= 2
    {
        findings.push(Finding {
            reason: Reason::PixelDimensions,
            evidence: format!("{w}x{h}"),
        });
    }

    if let Some(style) = candidate.style {
        let s = style.to_ascii_lowercase().replace(' ', "");
        for marker in ["display:none", "visibility:hidden", "opacity:0"] {
            if s.contains(marker) {
                findings.push(Finding {
                    reason: Reason::HiddenByStyle,
                    evidence: marker.to_owned(),
                });
                break;
            }
        }
    }

    // A first-party image from an attested sender is not exempt from the dimension checks —
    // a sender can track its own recipients — but the token heuristic is where a false
    // positive is most likely, because legitimate first-party addresses carry opaque
    // identifiers all the time.
    let first_party = origin.domain().is_some_and(|d| candidate.url.contains(d));
    if !first_party && let Some(token) = high_entropy_segment(candidate.url) {
        findings.push(Finding {
            reason: Reason::PerRecipientToken,
            evidence: token,
        });
    }

    if candidate.in_zero_height_container && candidate.alt.is_none_or(str::is_empty) {
        findings.push(Finding {
            reason: Reason::UnannouncedAndInvisible,
            evidence: "no alt text, zero-height container".to_owned(),
        });
    }

    findings
}

/// The longest path or query segment that looks like an opaque per-recipient identifier.
///
/// Deliberately conservative: length **and** a mix of character classes, because a long
/// lowercase word is a slug and a long mixed-case alphanumeric run is an identifier. Getting
/// this wrong blocks legitimate images, which is a visible failure — but the failure
/// direction is still the safe one, since a blocked image is reported and can be allowed.
fn high_entropy_segment(url: &str) -> Option<String> {
    const MINIMUM: usize = 20;
    url.split(['/', '?', '&', '=', '.', '-', '_'])
        .filter(|s| s.len() >= MINIMUM)
        .find(|s| {
            let alnum = s.chars().all(|c| c.is_ascii_alphanumeric());
            let has_digit = s.chars().any(|c| c.is_ascii_digit());
            let has_alpha = s.chars().any(|c| c.is_ascii_alphabetic());
            alnum && has_digit && has_alpha
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::Authentication;

    fn attested(domain: &str) -> Origin {
        Origin::derive(&Authentication {
            signing_domain: Some(domain.to_owned()),
            ..Authentication::default()
        })
    }

    #[test]
    fn a_one_by_one_image_is_a_tracking_pixel() {
        let c = Candidate {
            url: "https://t.test/p.gif",
            declared_width: Some(1),
            declared_height: Some(1),
            ..Candidate::default()
        };
        let f = examine(&c, &Origin::Null);
        assert!(f.iter().any(|f| f.reason == Reason::PixelDimensions));
    }

    #[test]
    fn an_ordinary_image_is_not() {
        let c = Candidate {
            url: "https://cdn.test/hero.png",
            declared_width: Some(600),
            declared_height: Some(400),
            alt: Some("the hero image"),
            ..Candidate::default()
        };
        assert!(examine(&c, &attested("cdn.test")).is_empty());
    }

    #[test]
    fn hidden_by_style_is_caught_however_it_is_hidden() {
        for style in ["display:none", "visibility: hidden", "opacity:0"] {
            let c = Candidate {
                url: "https://t.test/x",
                style: Some(style),
                ..Candidate::default()
            };
            let f = examine(&c, &Origin::Null);
            assert!(
                f.iter().any(|f| f.reason == Reason::HiddenByStyle),
                "{style} was not caught"
            );
        }
    }

    #[test]
    fn a_per_recipient_token_is_evidence() {
        let c = Candidate {
            url: "https://t.test/open/a8Fk29xQ7bLm3PzR6vT1/pixel.gif",
            ..Candidate::default()
        };
        let f = examine(&c, &Origin::Null);
        assert!(
            f.iter().any(|f| f.reason == Reason::PerRecipientToken),
            "{f:?}"
        );
    }

    #[test]
    fn a_readable_path_is_not_a_token() {
        // The false positive that matters: blocking a legitimate image is visible, and a
        // heuristic that fires on ordinary paths is one users turn off.
        for url in [
            "https://cdn.test/images/newsletter-header-october.png",
            "https://cdn.test/assets/logo.svg",
            "https://cdn.test/2026/quarterly/chart.png",
        ] {
            let c = Candidate {
                url,
                ..Candidate::default()
            };
            let f = examine(&c, &Origin::Null);
            assert!(
                !f.iter().any(|f| f.reason == Reason::PerRecipientToken),
                "{url} was called a token: {f:?}"
            );
        }
    }

    #[test]
    fn a_first_party_address_is_spared_the_token_heuristic() {
        // Legitimate first-party addresses carry opaque identifiers constantly. The
        // dimension checks still apply — a sender can track its own recipients.
        let c = Candidate {
            url: "https://bank.test/i/a8Fk29xQ7bLm3PzR6vT1.png",
            ..Candidate::default()
        };
        let f = examine(&c, &attested("bank.test"));
        assert!(!f.iter().any(|f| f.reason == Reason::PerRecipientToken));
    }

    #[test]
    fn two_weak_signals_are_strong_together() {
        // A legitimate decorative image has alt text or visible height. A tracker has
        // neither, because it is meant to be neither seen nor announced.
        let c = Candidate {
            url: "https://t.test/x.gif",
            in_zero_height_container: true,
            alt: None,
            ..Candidate::default()
        };
        let f = examine(&c, &Origin::Null);
        assert!(
            f.iter()
                .any(|f| f.reason == Reason::UnannouncedAndInvisible)
        );

        let decorative = Candidate {
            url: "https://t.test/x.gif",
            in_zero_height_container: true,
            alt: Some("a decorative rule"),
            ..Candidate::default()
        };
        assert!(
            !examine(&decorative, &Origin::Null)
                .iter()
                .any(|f| f.reason == Reason::UnannouncedAndInvisible)
        );
    }

    #[test]
    fn every_finding_carries_its_evidence() {
        // "Every heuristic block MUST log its reason." A reason with no evidence is an
        // accusation, and the debug view would have nothing to show.
        let c = Candidate {
            url: "https://t.test/x",
            declared_width: Some(1),
            declared_height: Some(1),
            style: Some("display:none"),
            ..Candidate::default()
        };
        let findings = examine(&c, &Origin::Null);
        assert!(
            findings.len() >= 2,
            "only one reason was reported: {findings:?}"
        );
        for f in &findings {
            assert!(!f.evidence.is_empty(), "{f:?}");
            assert!(!f.reason.name().is_empty());
        }
    }
}

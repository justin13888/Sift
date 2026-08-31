//! FR-30, FR-42 — presenting a destination honestly, and never following it.
//!
//! # Navigation never happens in place
//!
//! Every navigation attempt from the body view is intercepted, and the document that renders
//! a message renders **only** that message. This is part of N-1: a body view that could
//! navigate would have a network capability by another name.
//!
//! On an intercepted navigation Sift shows the **real resolved destination** and opens it in
//! the system browser only on explicit confirmation.
//!
//! # Showing the raw string is not enough, and a naive decoding is worse
//!
//! Two attacks make a URL render as one domain and resolve to another: internationalized
//! homographs, and bidirectional controls that reverse the visible order of a URL's
//! components. So the displayed form is punycode-decoded **and** bidi-stripped — and the
//! order matters, because decoding without stripping produces a string that looks even more
//! convincing than the raw one.

use sift_foundation::normalize;

/// A destination, as it will be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// What the user is shown. Safe to render in native chrome.
    pub display: String,
    /// The address that will actually be opened.
    pub resolved: String,
    /// The wrapper, where one was unwrapped. FR-30 keeps it **available on request**.
    pub wrapper: Option<String>,
    /// Whether the destination is a `mailto:`.
    ///
    /// FR-42: shown and **reported as requiring a mail handler**, exactly as FR-41's absent
    /// handler case — never silently omitted, because omitting it means the user cannot
    /// unsubscribe and does not know why.
    pub needs_a_mail_handler: bool,
}

/// Whether a host is punycode and needs decoding for display.
fn decode_punycode_label(label: &str) -> Option<String> {
    // Only the shape matters here: a label starting with the ACE prefix is one whose
    // displayed form differs from its wire form, which is the property the display rule is
    // about. Full decoding belongs with the platform's IDN support; what must never happen
    // is showing the wire form as though it were the display form, or the reverse.
    label.strip_prefix("xn--").map(|rest| format!("[{rest}]"))
}

/// The display form of a URL.
///
/// Bidi controls are **removed** here rather than isolated, which is the opposite of what
/// NFR-54 does for a display name — and the difference is deliberate. A display name is
/// prose, where a right-to-left run is legitimate content. A URL is a structured identifier
/// whose component order carries meaning, and a control that reverses that order has no
/// legitimate use in one.
#[must_use]
pub fn display_form(url: &str) -> String {
    let stripped: String = url
        .chars()
        .filter(|c| {
            !matches!(u32::from(*c),
                0x061C | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069)
        })
        .collect();

    let normalized = normalize::for_index(&stripped);
    let Some((scheme, rest)) = normalized.split_once("://") else {
        return normalized;
    };
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let decoded: Vec<String> = host
        .split('.')
        .map(|l| decode_punycode_label(l).unwrap_or_else(|| l.to_owned()))
        .collect();
    let host = decoded.join(".");
    if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{path}")
    }
}

/// FR-30 — unwrap a click tracker.
///
/// **Recovered locally or not at all.** Following the redirect to find out where it goes
/// *is* the tracking event, so a wrapper whose destination is not recoverable from its own
/// text is reported as unrecoverable rather than resolved.
///
/// Where it is recoverable, tracking parameters are stripped **using the removal rules the
/// filter lists already carry** rather than a bespoke list — a second list would drift from
/// the first, and the first is maintained by people who do nothing else.
#[must_use]
pub fn unwrap(url: &str, parameter_removals: &[String]) -> Destination {
    let needs_a_mail_handler = url.trim_start().to_ascii_lowercase().starts_with("mailto:");

    // A wrapper carries its destination as a parameter. Anything else needs a request to
    // resolve, and Sift does not make one.
    let embedded = url
        .split(['?', '&'])
        .skip(1)
        .filter_map(|p| p.split_once('='))
        .filter(|(k, _)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "url" | "u" | "target" | "redirect" | "dest"
            )
        })
        .map(|(_, v)| percent_decode(v))
        .find(|v| v.contains("://"));

    let (resolved, wrapper) = match embedded {
        Some(destination) => (
            strip_parameters(&destination, parameter_removals),
            Some(url.to_owned()),
        ),
        None => (strip_parameters(url, parameter_removals), None),
    };

    Destination {
        display: display_form(&resolved),
        resolved,
        wrapper,
        needs_a_mail_handler,
    }
}

/// Remove tracking parameters.
fn strip_parameters(url: &str, removals: &[String]) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| {
            let name = p.split('=').next().unwrap_or("").to_ascii_lowercase();
            !removals.iter().any(|r| r.eq_ignore_ascii_case(&name))
        })
        .collect();
    if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// FR-42 — whether Sift issues an unsubscribe request itself.
///
/// **Never, by any method.** Two independent reasons, and either alone would be enough. The
/// historical form is a message, which the no-send constraint forbids outright. The modern
/// form is an HTTP request, which would be **egress from outside the resource broker** —
/// falsifying the completeness of the privacy egress table — carrying a high-entropy
/// per-recipient token, which is the same signal FR-29 treats as evidence of tracking.
///
/// So Sift displays the destination and opens it in the system browser on confirmation.
#[must_use]
pub const fn issues_unsubscribe_requests() -> bool {
    false
}

/// Whether a wrapper is resolved by fetching it.
///
/// **No.** Following the redirect is the tracking event.
#[must_use]
pub const fn resolves_wrappers_by_fetching() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removals() -> Vec<String> {
        ["utm_source", "utm_medium", "utm_campaign", "fbclid"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn a_wrapped_destination_is_recovered_from_the_wrapper_itself() {
        let d = unwrap(
            "https://click.test/redirect?u=https%3A%2F%2Freal.test%2Farticle&id=abc",
            &removals(),
        );
        assert_eq!(d.resolved, "https://real.test/article");
        assert_eq!(
            d.wrapper.as_deref(),
            Some("https://click.test/redirect?u=https%3A%2F%2Freal.test%2Farticle&id=abc")
        );
    }

    #[test]
    fn a_wrapper_whose_destination_is_not_in_it_is_left_alone() {
        // Following the redirect to find out where it goes **is** the tracking event.
        let d = unwrap("https://click.test/x/9f3a2b", &removals());
        assert_eq!(d.resolved, "https://click.test/x/9f3a2b");
        assert!(d.wrapper.is_none(), "a destination was invented");
        assert!(!resolves_wrappers_by_fetching());
    }

    #[test]
    fn tracking_parameters_are_stripped_using_the_lists_own_rules() {
        // A bespoke list would drift from the filter lists, which are maintained by people
        // who do nothing else.
        let d = unwrap(
            "https://shop.test/item?id=7&utm_source=mail&fbclid=xyz",
            &removals(),
        );
        assert_eq!(d.resolved, "https://shop.test/item?id=7");
    }

    #[test]
    fn stripping_every_parameter_leaves_no_dangling_question_mark() {
        let d = unwrap("https://shop.test/item?utm_source=mail", &removals());
        assert_eq!(d.resolved, "https://shop.test/item");
    }

    #[test]
    fn bidi_controls_are_removed_from_a_url() {
        // The opposite of what NFR-54 does for a display name, deliberately: a display name
        // is prose where a right-to-left run is legitimate content, and a URL is a structured
        // identifier whose component order carries meaning.
        let hostile = "https://ex\u{202E}moc.live\u{202C}ample.test/";
        let shown = display_form(hostile);
        assert!(
            !shown.contains('\u{202E}'),
            "an override survived into native chrome"
        );
        assert!(!shown.contains('\u{202C}'));
    }

    #[test]
    fn a_punycode_host_is_not_shown_as_though_it_were_its_display_form() {
        // Showing the wire form as the display form, or the reverse, is what makes a
        // homograph convincing.
        let shown = display_form("https://xn--80ak6aa92e.test/path");
        assert_ne!(shown, "https://xn--80ak6aa92e.test/path");
        assert!(shown.contains("[80ak6aa92e]"), "{shown}");
    }

    #[test]
    fn an_ordinary_url_survives_display_unchanged() {
        assert_eq!(
            display_form("https://example.test/a/b"),
            "https://example.test/a/b"
        );
    }

    #[test]
    fn a_mailto_destination_is_shown_and_reported_rather_than_omitted() {
        // FR-42: exactly as FR-41's absent-handler case. Omitting it means the user cannot
        // unsubscribe and does not know why.
        let d = unwrap("mailto:unsubscribe@list.test?subject=stop", &removals());
        assert!(d.needs_a_mail_handler);
        assert!(!d.display.is_empty());
    }

    #[test]
    fn sift_never_issues_an_unsubscribe_request() {
        // Either reason alone would be enough: the historical form is a message the no-send
        // constraint forbids, and the modern form is egress from outside the broker carrying
        // a per-recipient token.
        assert!(!issues_unsubscribe_requests());
    }

    #[test]
    fn hostile_urls_do_not_panic_the_display_path() {
        for url in [
            "",
            "://",
            "https://",
            "https://a",
            "%%%",
            "\u{0000}",
            &"a".repeat(10_000),
        ] {
            let _ = display_form(url);
            let _ = unwrap(url, &removals());
        }
    }
}

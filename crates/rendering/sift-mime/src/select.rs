//! Stage 2 — part selection.
//!
//! Prefer HTML from a multipart alternative; fall back to plain text, linkified. FR-9
//! requires **both** views be available for *any* message, including one that rendered
//! successfully — so selection picks what to render by default, and never removes a choice.

use crate::parse::{Message, Part};
use sift_foundation::limits::L1_BODY_PART_BYTES;

/// One part of a message, described without its bytes.
///
/// **No bytes.** The whole point of asking a provider for a structure is that it costs
/// nothing to learn what a message contains, so a descriptor carries a size and never the
/// thing it sizes.
///
/// It lives here rather than beside the adapter contract because the choice made over these
/// is [`choose`] — the same choice [`select`] makes over a parsed tree, and it has to be the
/// same, because a message must render the same way whichever provider it arrived through.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartDescriptor {
    /// The identifier the adapter takes to fetch this part. Opaque above the adapter.
    pub id: String,
    /// `type/subtype`, lowercased.
    pub media_type: String,
    /// Present where the part is an attachment rather than body content.
    ///
    /// **A sender-supplied filename never becomes a path** — NFR-53 — so this is a value to
    /// display and to derive a name from, never one to write with.
    pub filename: Option<String>,
    /// What the provider says the part will cost. Advisory: L-1 and L-13 are enforced against
    /// what is actually transferred, because a sender controls both numbers.
    pub size: u64,
}

impl PartDescriptor {
    /// Whether this part is body content rather than an attachment.
    #[must_use]
    pub fn is_body(&self) -> bool {
        self.filename.is_none()
            && (self.media_type == "text/html" || self.media_type == "text/plain")
    }
}

/// Which part to ask for, and why — FR-33 item 2 requires the debug view show both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub part: String,
    pub is_html: bool,
    pub reason: Reason,
}

/// Stage 2, over a structure a provider described rather than a tree Sift parsed.
///
/// The rule is [`select`]'s: **prefer the HTML alternative, fall back to plain text, and take
/// plain text where the HTML exceeds L-1.** That last one rejects rather than truncating, per
/// `docs/limits.md` — and here the rejection has somewhere to land, which is what makes the
/// fallback honest rather than a partial render with nothing saying so.
///
/// Nothing is fetched. That is the point: the caller asks for exactly one part's bytes, and
/// the forty-megabyte attachment beside it costs nothing.
#[must_use]
pub fn choose(parts: &[PartDescriptor]) -> Option<Chosen> {
    let body: Vec<&PartDescriptor> = parts.iter().filter(|p| p.is_body()).collect();
    let html = body
        .iter()
        .find(|p| p.media_type == "text/html" && p.size <= L1_BODY_PART_BYTES);
    let oversized = body
        .iter()
        .any(|p| p.media_type == "text/html" && p.size > L1_BODY_PART_BYTES);
    let plain = body.iter().find(|p| p.media_type == "text/plain");

    match (html, plain, oversized) {
        (Some(part), _, _) => Some(Chosen {
            part: part.id.clone(),
            is_html: true,
            reason: Reason::HtmlPreferred,
        }),
        (None, Some(part), true) => Some(Chosen {
            part: part.id.clone(),
            is_html: false,
            reason: Reason::HtmlTooLarge,
        }),
        (None, Some(part), false) => Some(Chosen {
            part: part.id.clone(),
            is_html: false,
            reason: Reason::OnlyPlainText,
        }),
        (None, None, _) => None,
    }
}

/// What was chosen, and what else is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection<'a> {
    pub chosen: Option<&'a Part>,
    pub html: Option<&'a Part>,
    pub plain: Option<&'a Part>,
    /// Everything with a filename or a disposition of attachment.
    pub attachments: Vec<&'a Part>,
    /// Why the chosen part was chosen. FR-33 item 2 requires the debug view show "which
    /// alternative was chosen **and why**", so the reason is produced here rather than
    /// reconstructed later.
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// HTML was present and preferred.
    HtmlPreferred,
    /// No HTML alternative existed.
    OnlyPlainText,
    /// HTML existed and exceeded L-1, so the plain alternative was taken instead.
    ///
    /// Note this is **not** the reject-to-raw-view path: a message with an oversized HTML
    /// part and a legitimate plain part still has something honest to show, and showing it
    /// is better than refusing the message. The oversized part is simply not rendered.
    HtmlTooLarge,
    /// Nothing renderable at all.
    NothingRenderable,
}

/// Whether a part is an attachment rather than body content.
fn is_attachment(part: &Part) -> bool {
    let disposition = part
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-disposition"))
        .map(|h| h.value.to_ascii_lowercase());
    match disposition {
        Some(d) if d.starts_with("attachment") => true,
        // A part with a filename is an attachment even when it calls itself inline, because
        // the platform decides what to do with a saved file by its *extension* — NFR-53's
        // whole concern.
        _ => part.parameter("name").is_some() || part.parameter("filename").is_some(),
    }
}

fn body_len(part: &Part) -> u64 {
    (part.body.end.saturating_sub(part.body.start)) as u64
}

/// Choose what to render.
#[must_use]
pub fn select(message: &Message) -> Selection<'_> {
    let all = message.root.walk();

    let mut html = None;
    let mut plain = None;
    let mut attachments = Vec::new();

    for part in all {
        if !part.children.is_empty() {
            continue;
        }
        if is_attachment(part) {
            attachments.push(part);
            continue;
        }
        match (part.media_type.as_str(), part.media_subtype.as_str()) {
            ("text", "html") if html.is_none() => html = Some(part),
            ("text", "plain") if plain.is_none() => plain = Some(part),
            _ => attachments.push(part),
        }
    }

    // L-1 bounds what reaches the sanitizer, **after transfer decoding**. The range here is
    // the encoded length, which is an over-estimate of the decoded one — so a part this
    // rejects would certainly have been rejected later, and one it admits is checked again
    // where the decoded bytes exist.
    let html_fits = html.is_some_and(|p| body_len(p) <= L1_BODY_PART_BYTES);

    let (chosen, reason) = match (html, plain, html_fits) {
        (Some(h), _, true) => (Some(h), Reason::HtmlPreferred),
        (Some(_), Some(p), false) => (Some(p), Reason::HtmlTooLarge),
        (Some(_), None, false) => (None, Reason::NothingRenderable),
        (None, Some(p), _) => (Some(p), Reason::OnlyPlainText),
        (None, None, _) => (None, Reason::NothingRenderable),
    };

    Selection {
        chosen,
        html,
        plain,
        attachments,
        reason,
    }
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;

    fn part(id: &str, media_type: &str, size: u64) -> PartDescriptor {
        PartDescriptor {
            id: id.into(),
            media_type: media_type.into(),
            filename: None,
            size,
        }
    }

    #[test]
    fn the_html_alternative_is_preferred_and_the_reason_is_recorded() {
        let chosen = choose(&[part("0", "text/plain", 10), part("1", "text/html", 20)])
            .expect("nothing was chosen");
        assert_eq!(chosen.part, "1");
        assert!(chosen.is_html);
        assert_eq!(chosen.reason, Reason::HtmlPreferred);
    }

    #[test]
    fn an_html_part_over_the_bound_falls_back_to_plain_rather_than_being_cut() {
        // docs/limits.md: exceeding a parse limit rejects, it does not truncate. Here the
        // rejection has somewhere to land, which is what makes the fallback honest rather
        // than a partial render with nothing saying so.
        let huge = L1_BODY_PART_BYTES + 1;
        let chosen = choose(&[part("0", "text/plain", 10), part("1", "text/html", huge)])
            .expect("nothing was chosen");
        assert_eq!(chosen.part, "0");
        assert!(!chosen.is_html);
        assert_eq!(chosen.reason, Reason::HtmlTooLarge);
    }

    #[test]
    fn an_oversized_html_part_with_no_plain_alternative_chooses_nothing() {
        // And *that* is the reject-to-raw-view path, because there is nothing honest left.
        assert_eq!(
            choose(&[part("0", "text/html", L1_BODY_PART_BYTES + 1)]),
            None
        );
    }

    #[test]
    fn an_attachment_is_never_chosen_as_the_body() {
        let mut attachment = part("2", "text/html", 10);
        attachment.filename = Some("invoice.html".into());
        assert_eq!(choose(&[attachment.clone()]), None);
        let chosen = choose(&[attachment, part("0", "text/plain", 10)]).unwrap();
        assert_eq!(chosen.part, "0");
    }

    #[test]
    fn a_message_of_nothing_but_an_attachment_chooses_nothing() {
        let mut attachment = part("0", "application/pdf", 40_000_000);
        attachment.filename = Some("statement.pdf".into());
        assert_eq!(choose(&[attachment]), None);
    }

    #[test]
    fn the_choice_over_a_description_agrees_with_the_choice_over_a_tree() {
        // The property that matters: a message must render the same way whichever provider
        // it arrived through. One rule, two inputs.
        let described = choose(&[part("0", "text/plain", 10), part("1", "text/html", 20)]).unwrap();
        assert_eq!(described.reason, Reason::HtmlPreferred);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    #[test]
    fn html_is_preferred_over_plain() {
        let raw = b"Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
                    --b\r\nContent-Type: text/plain\r\n\r\nplain\r\n\
                    --b\r\nContent-Type: text/html\r\n\r\n<p>html</p>\r\n--b--\r\n";
        let m = parse(raw).expect("parses");
        let s = select(&m);
        assert_eq!(s.reason, Reason::HtmlPreferred);
        assert_eq!(
            s.chosen.map(crate::parse::Part::content_type).as_deref(),
            Some("text/html")
        );
    }

    #[test]
    fn both_views_stay_available_even_when_one_is_chosen() {
        // FR-9: plain text and raw source MUST be available for **any** message, including
        // one that rendered successfully. Selection picks a default; it never removes a
        // choice.
        let raw = b"Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
                    --b\r\nContent-Type: text/plain\r\n\r\nplain\r\n\
                    --b\r\nContent-Type: text/html\r\n\r\n<p>html</p>\r\n--b--\r\n";
        let m = parse(raw).expect("parses");
        let s = select(&m);
        assert!(s.html.is_some() && s.plain.is_some());
    }

    #[test]
    fn plain_text_is_chosen_when_there_is_no_html() {
        let m = parse(b"Content-Type: text/plain\r\n\r\nplain").expect("parses");
        assert_eq!(select(&m).reason, Reason::OnlyPlainText);
    }

    #[test]
    fn a_part_with_a_filename_is_an_attachment_even_if_it_says_inline() {
        // The platform decides what to do with a saved file by its **extension**, not by
        // what the message called the disposition — which is the whole of NFR-53's concern.
        let raw = b"Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
                    --b\r\nContent-Type: text/html; name=\"invoice.html\"\r\n\
                    Content-Disposition: inline\r\n\r\n<p>x</p>\r\n--b--\r\n";
        let m = parse(raw).expect("parses");
        let s = select(&m);
        assert_eq!(
            s.attachments.len(),
            1,
            "a named part was treated as body content"
        );
        assert!(s.html.is_none());
    }

    #[test]
    fn a_message_with_nothing_renderable_says_so() {
        let raw = b"Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
                    --b\r\nContent-Type: application/octet-stream\r\n\r\nbytes\r\n--b--\r\n";
        let m = parse(raw).expect("parses");
        let s = select(&m);
        assert_eq!(s.reason, Reason::NothingRenderable);
        assert!(s.chosen.is_none());
    }

    #[test]
    fn selection_records_why_so_the_debug_view_can_show_it() {
        // FR-33 item 2: "which alternative was chosen **and why**". Produced here rather
        // than reconstructed later, because reconstructing it means guessing.
        let m = parse(b"Content-Type: text/plain\r\n\r\nx").expect("parses");
        let _: Reason = select(&m).reason;
    }
}

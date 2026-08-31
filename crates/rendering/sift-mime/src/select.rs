//! Stage 2 — part selection.
//!
//! Prefer HTML from a multipart alternative; fall back to plain text, linkified. FR-9
//! requires **both** views be available for *any* message, including one that rendered
//! successfully — so selection picks what to render by default, and never removes a choice.

use crate::parse::{Message, Part};
use sift_foundation::limits::L1_BODY_PART_BYTES;

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

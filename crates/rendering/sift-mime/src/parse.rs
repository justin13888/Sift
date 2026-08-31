//! Stage 1 — parsing structure and headers, within the bounds.

use sift_foundation::limits::{
    L2_MIME_PARTS, L3_MIME_DEPTH, L4_HEADER_BLOCK_BYTES, L5_HEADER_FIELDS,
};

/// Why a message could not be parsed.
///
/// Every variant degrades to FR-9's raw source view. None of them is recoverable by
/// truncating, and none of them is a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// L-4. Applies **before any header is decoded**, so an encoded-word bomb is bounded
    /// before NFR-28's decoding runs on it.
    HeaderBlockTooLarge,
    /// L-5.
    TooManyHeaderFields,
    /// L-2, counted across the whole tree rather than per level.
    TooManyParts,
    /// L-3. "A multipart inside a message/rfc822 inside a multipart is ordinary; thirty-two
    /// levels is an attack."
    TooDeep,
    /// Structurally unusable — an unterminated boundary, a header with no colon in a
    /// position where one is required.
    Malformed,
}

/// A header field, decoded far enough to be useful and no further.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// One node of the MIME tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub media_type: String,
    pub media_subtype: String,
    pub parameters: Vec<(String, String)>,
    pub headers: Vec<Header>,
    /// Where this part's body sits in the original bytes. **A range rather than the bytes**,
    /// because materialising every part is what "structure first" refuses to do.
    pub body: core::ops::Range<usize>,
    pub children: Vec<Part>,
}

impl Part {
    /// `type/subtype`, lowercased.
    #[must_use]
    pub fn content_type(&self) -> String {
        format!("{}/{}", self.media_type, self.media_subtype)
    }

    #[must_use]
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Every part in the tree, depth-first.
    pub fn walk(&self) -> Vec<&Self> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }
}

/// A parsed message: the top-level headers and the part tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub headers: Vec<Header>,
    pub root: Part,
}

impl Message {
    /// A header's value, by name, case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
}

/// Parse a message.
///
/// # Errors
///
/// Every error degrades to the raw source view under FR-9. Nothing here truncates.
pub fn parse(raw: &[u8]) -> Result<Message, ParseError> {
    let (headers, body_start) = parse_header_block(raw, 0)?;
    let mut budget = Budget { parts: 0 };
    let root = parse_part(raw, &headers, body_start, raw.len(), 0, &mut budget)?;
    Ok(Message { headers, root })
}

struct Budget {
    parts: u64,
}

/// Where the header block ends: the first blank line. A message with no blank line is all
/// headers, which is legal and empty.
fn parse_header_block(raw: &[u8], from: usize) -> Result<(Vec<Header>, usize), ParseError> {
    let end = find_blank_line(raw, from).unwrap_or(raw.len());
    if end.saturating_sub(from) as u64 > L4_HEADER_BLOCK_BYTES {
        return Err(ParseError::HeaderBlockTooLarge);
    }
    let block = &raw[from..end];

    // Unfolding: a continuation line begins with whitespace and belongs to the field above.
    let mut fields: Vec<String> = Vec::new();
    for line in split_lines(block) {
        let text = String::from_utf8_lossy(line);
        if text.starts_with([' ', '\t']) {
            if let Some(last) = fields.last_mut() {
                last.push(' ');
                last.push_str(text.trim());
                continue;
            }
            // A continuation with nothing to continue. Ignored rather than fatal: it is
            // malformed in a way that costs nothing to survive.
            continue;
        }
        if fields.len() as u64 >= L5_HEADER_FIELDS {
            return Err(ParseError::TooManyHeaderFields);
        }
        fields.push(text.into_owned());
    }

    let headers = fields
        .into_iter()
        .filter_map(|f| {
            let (name, value) = f.split_once(':')?;
            Some(Header {
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect();

    // Step past the blank line itself.
    let body = skip_blank_line(raw, end);
    Ok((headers, body))
}

fn parse_part(
    raw: &[u8],
    headers: &[Header],
    body_start: usize,
    body_end: usize,
    depth: u64,
    budget: &mut Budget,
) -> Result<Part, ParseError> {
    if depth > L3_MIME_DEPTH {
        return Err(ParseError::TooDeep);
    }
    budget.parts += 1;
    if budget.parts > L2_MIME_PARTS {
        return Err(ParseError::TooManyParts);
    }

    let content_type = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.as_str())
        // The default when a message says nothing, from the internet message format.
        .unwrap_or("text/plain; charset=us-ascii");

    let (media, parameters) = parse_content_type(content_type);
    let (media_type, media_subtype) = media
        .split_once('/')
        .map_or((media.clone(), "plain".to_owned()), |(t, s)| {
            (t.to_owned(), s.to_owned())
        });

    let mut part = Part {
        media_type: media_type.to_ascii_lowercase(),
        media_subtype: media_subtype.to_ascii_lowercase(),
        parameters,
        headers: headers.to_vec(),
        body: body_start..body_end,
        children: Vec::new(),
    };

    if part.media_type == "multipart" {
        let Some(boundary) = part.parameter("boundary").map(str::to_owned) else {
            // A multipart with no boundary cannot be split. Treated as a leaf rather than
            // rejected: the bytes are still showable in the raw view, and rejecting the
            // whole message for one malformed container is harsher than the input warrants.
            return Ok(part);
        };
        for (s, e, child_headers) in split_multipart(raw, body_start, body_end, &boundary)? {
            part.children
                .push(parse_part(raw, &child_headers, s, e, depth + 1, budget)?);
        }
    } else if part.media_type == "message" && part.media_subtype == "rfc822" {
        // A nested message. This is the construction L-3 exists for: each level is ordinary
        // and thirty-two of them are not.
        let (inner_headers, inner_body) = parse_header_block(raw, body_start)?;
        part.children.push(parse_part(
            raw,
            &inner_headers,
            inner_body,
            body_end,
            depth + 1,
            budget,
        )?);
    }

    Ok(part)
}

/// `type/subtype; name=value; name="quoted value"`
fn parse_content_type(value: &str) -> (String, Vec<(String, String)>) {
    let mut it = value.split(';');
    let media = it
        .next()
        .unwrap_or("text/plain")
        .trim()
        .to_ascii_lowercase();
    let mut parameters = Vec::new();
    for p in it {
        if let Some((k, v)) = p.split_once('=') {
            let v = v.trim().trim_matches('"');
            parameters.push((k.trim().to_ascii_lowercase(), v.to_owned()));
        }
    }
    (media, parameters)
}

/// Split a multipart body, returning each child's body range and headers.
fn split_multipart(
    raw: &[u8],
    from: usize,
    to: usize,
    boundary: &str,
) -> Result<Vec<(usize, usize, Vec<Header>)>, ParseError> {
    let delimiter = format!("--{boundary}");
    let close = format!("--{boundary}--");
    let mut children = Vec::new();
    let mut cursor = from;
    let mut open: Option<usize> = None;

    while cursor < to {
        let line_end = find_line_end(raw, cursor, to);
        let line = String::from_utf8_lossy(&raw[cursor..line_end]);
        let trimmed = line.trim_end();

        if trimmed == close || trimmed == delimiter {
            if let Some(start) = open.take() {
                // The part ends before the CRLF that precedes this delimiter.
                let end = trim_trailing_newline(raw, start, cursor);
                let (headers, body) = parse_header_block(raw, start)?;
                children.push((body.min(end), end, headers));
            }
            if trimmed == close {
                return Ok(children);
            }
            open = Some(skip_newline(raw, line_end, to));
        }
        cursor = skip_newline(raw, line_end, to);
        if line_end >= to {
            break;
        }
    }

    // An unterminated multipart: no closing delimiter. Whatever was open is taken to run to
    // the end, because discarding a part the user can see in the raw view would be a lie of
    // omission rather than safety.
    if let Some(start) = open {
        let (headers, body) = parse_header_block(raw, start)?;
        children.push((body.min(to), to, headers));
    }
    Ok(children)
}

// --- byte helpers, all bounds-checked so hostile input cannot walk off the end ---

fn find_blank_line(raw: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < raw.len() {
        if raw[i] == b'\n' {
            let next = i + 1;
            if next < raw.len() && raw[next] == b'\n' {
                return Some(i);
            }
            if next + 1 < raw.len() && raw[next] == b'\r' && raw[next + 1] == b'\n' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn skip_blank_line(raw: &[u8], end: usize) -> usize {
    let mut i = end;
    let mut newlines = 0;
    while i < raw.len() && newlines < 2 {
        if raw[i] == b'\n' {
            newlines += 1;
        }
        i += 1;
    }
    i
}

fn split_lines(block: &[u8]) -> Vec<&[u8]> {
    block
        .split(|b| *b == b'\n')
        .map(|l| {
            if l.last() == Some(&b'\r') {
                &l[..l.len() - 1]
            } else {
                l
            }
        })
        .filter(|l| !l.is_empty())
        .collect()
}

fn find_line_end(raw: &[u8], from: usize, to: usize) -> usize {
    let mut i = from;
    while i < to && raw[i] != b'\n' {
        i += 1;
    }
    i
}

fn skip_newline(raw: &[u8], at: usize, to: usize) -> usize {
    if at < to && raw.get(at) == Some(&b'\n') {
        at + 1
    } else {
        at.max(to.min(at + 1))
    }
}

fn trim_trailing_newline(raw: &[u8], start: usize, end: usize) -> usize {
    let mut e = end;
    while e > start && matches!(raw.get(e - 1), Some(b'\n' | b'\r')) {
        e -= 1;
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple() -> &'static [u8] {
        b"From: a@example.test\r\nSubject: Hello\r\nContent-Type: text/plain\r\n\r\nbody text\r\n"
    }

    #[test]
    fn a_plain_message_parses() {
        let m = parse(simple()).expect("parses");
        assert_eq!(m.header("subject"), Some("Hello"));
        assert_eq!(
            m.header("SUBJECT"),
            Some("Hello"),
            "header lookup is case-insensitive"
        );
        assert_eq!(m.root.content_type(), "text/plain");
    }

    #[test]
    fn a_message_with_no_content_type_defaults_correctly() {
        let m = parse(b"Subject: x\r\n\r\nbody").expect("parses");
        assert_eq!(m.root.content_type(), "text/plain");
    }

    #[test]
    fn folded_headers_are_unfolded() {
        // NFR-28's territory: a subject split across lines is one value, and a parser that
        // treated the continuation as a new field would lose half of it.
        let raw = b"Subject: a very\r\n long subject\r\nFrom: x@y.test\r\n\r\nbody";
        let m = parse(raw).expect("parses");
        assert_eq!(m.header("subject"), Some("a very long subject"));
        assert_eq!(m.header("from"), Some("x@y.test"));
    }

    #[test]
    fn a_multipart_splits_into_its_children() {
        let raw = b"Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
                    --b\r\nContent-Type: text/plain\r\n\r\nplain\r\n\
                    --b\r\nContent-Type: text/html\r\n\r\n<p>html</p>\r\n\
                    --b--\r\n";
        let m = parse(raw).expect("parses");
        assert_eq!(m.root.children.len(), 2, "the multipart did not split");
        assert_eq!(m.root.children[0].content_type(), "text/plain");
        assert_eq!(m.root.children[1].content_type(), "text/html");
    }

    #[test]
    fn a_header_block_over_l4_is_rejected_rather_than_truncated() {
        // Applies *before* any header is decoded, so an encoded-word bomb is bounded before
        // NFR-28's decoding runs on it.
        let mut raw = Vec::new();
        while (raw.len() as u64) <= L4_HEADER_BLOCK_BYTES {
            raw.extend_from_slice(b"X-Filler: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        raw.extend_from_slice(b"\r\nbody");
        assert_eq!(parse(&raw), Err(ParseError::HeaderBlockTooLarge));
    }

    #[test]
    fn too_many_header_fields_is_rejected() {
        let mut raw = Vec::new();
        for i in 0..=L5_HEADER_FIELDS {
            raw.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\nbody");
        // Whichever bound is reached first, the message is refused rather than partly parsed.
        assert!(matches!(
            parse(&raw),
            Err(ParseError::TooManyHeaderFields | ParseError::HeaderBlockTooLarge)
        ));
    }

    #[test]
    fn deep_nesting_is_rejected() {
        // "A multipart inside a message/rfc822 inside a multipart is ordinary; thirty-two
        // levels is an attack."
        let mut raw = Vec::new();
        for _ in 0..=L3_MIME_DEPTH + 2 {
            raw.extend_from_slice(b"Content-Type: message/rfc822\r\n\r\n");
        }
        raw.extend_from_slice(b"Content-Type: text/plain\r\n\r\nbody");
        assert_eq!(parse(&raw), Err(ParseError::TooDeep));
    }

    #[test]
    fn a_multipart_with_no_boundary_is_a_leaf_rather_than_a_rejection() {
        // Rejecting a whole message for one malformed container is harsher than the input
        // warrants: the bytes are still showable in the raw view.
        let m = parse(b"Content-Type: multipart/mixed\r\n\r\nsomething").expect("parses");
        assert!(m.root.children.is_empty());
    }

    #[test]
    fn an_unterminated_multipart_keeps_what_it_had() {
        // Discarding a part the user can see in the raw view would be a lie of omission
        // rather than safety.
        let raw = b"Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
                    --b\r\nContent-Type: text/plain\r\n\r\nplain text";
        let m = parse(raw).expect("parses");
        assert_eq!(m.root.children.len(), 1);
    }

    #[test]
    fn hostile_input_never_panics() {
        // NFR-19. The process holds every account's sync state, the mutation queue, and
        // under D-2 the shell; a panic here is not a rendering failure, it is an outage.
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"\r\n\r\n".to_vec(),
            b":::::".to_vec(),
            b"Content-Type: multipart/mixed; boundary=\"\"\r\n\r\n----\r\n".to_vec(),
            b"Content-Type:\r\n\r\n".to_vec(),
            b"\x00\x01\x02\xff\xfe".to_vec(),
            b"Content-Type: multipart/x; boundary=b\r\n\r\n--b".to_vec(),
            b"Content-Type: multipart/x; boundary=b\r\n\r\n--b--".to_vec(),
            b"Subject: \xc3\x28 invalid utf8\r\n\r\nbody".to_vec(),
            vec![b'\n'; 10_000],
            vec![b'-'; 10_000],
        ];
        for case in cases {
            // The contract is "does not panic". Either outcome is acceptable.
            let _ = parse(&case);
        }
    }

    #[test]
    fn invalid_utf8_in_a_header_does_not_lose_the_message() {
        // NFR-28: non-UTF-8 charsets are real mail. Lossy decoding keeps the message
        // parseable; what it must not do is fail.
        let m = parse(b"Subject: caf\xe9\r\nContent-Type: text/plain\r\n\r\nbody").expect("parses");
        assert!(m.header("subject").is_some());
    }

    #[test]
    fn a_part_carries_a_range_rather_than_its_bytes() {
        // "Structure first": a message carrying a 40 MB attachment costs a few kilobytes
        // until the user asks for the attachment.
        let m = parse(simple()).expect("parses");
        assert!(m.root.body.end > m.root.body.start);
        assert!(m.root.body.end <= simple().len());
    }
}

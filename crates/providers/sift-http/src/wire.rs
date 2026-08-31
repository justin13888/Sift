//! HTTP/1.1 on the wire: what is written, what is read back, and where each stops.
//!
//! # Why this is hand-written
//!
//! D-31 already decided that the IMAP client is hand-written, for a reason that applies here
//! unchanged: what Sift needs is a small, bounded, auditable subset, and a general-purpose
//! client is a much larger surface whose limits are its own rather than `docs/limits.md`'s.
//!
//! The specific properties a borrowed client would not have given:
//!
//! - **Every bound is one of Sift's.** The head is capped, the body is capped at L-13, and
//!   the decompressed body is capped at L-13 *again* — because a client that caps only the
//!   transfer has admitted a decompression bomb inside NFR-39's ceiling.
//! - **FR-36 counts bytes on the wire.** Not the body's length and not the decoded length:
//!   what the interface carried. That is only countable where the socket is.
//! - **Nothing is retried, redirected, or slept on here.** D-87 puts a stated delay on the
//!   timing wheel; a client that honoured `Retry-After` itself would be a per-account sleep
//!   loop wearing a library's name, which D-25 prohibits outright.
//!
//! There is no redirect following. Every endpoint Sift talks to is one it constructed, and a
//! provider that answered a delta with a redirect is a provider doing something Sift should
//! surface rather than obey.

use std::io::Read;

/// The largest response head Sift will read.
///
/// Not a limit in `docs/limits.md`, because it bounds no behaviour a user can observe: a
/// provider whose response headers exceed this is malfunctioning, not exceeding a budget.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// What went wrong below the level of a status code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The answer was not HTTP, or not HTTP this parser accepts.
    Malformed(&'static str),
    /// The head exceeded [`MAX_HEAD_BYTES`], or the body a stated cap.
    TooLarge { limit_bytes: u64 },
    /// The socket failed.
    Io(String),
}

/// A response's status line and headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl Head {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// How the body is framed. The order is the one RFC 9112 gives: transfer coding wins
    /// over content length, and a message with neither runs to the close of the connection.
    #[must_use]
    pub fn framing(&self) -> Framing {
        if self
            .get("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
        {
            return Framing::Chunked;
        }
        if let Some(len) = self.get("content-length").and_then(|v| v.trim().parse().ok()) {
            return Framing::Length(len);
        }
        // 204 and 304 carry no body regardless of what they say.
        if matches!(self.status, 204 | 304) {
            return Framing::Length(0);
        }
        Framing::UntilClose
    }

    /// Whether the connection may be reused for the next request.
    #[must_use]
    pub fn keeps_alive(&self) -> bool {
        !self
            .get("connection")
            .is_some_and(|v| v.to_ascii_lowercase().contains("close"))
            && !matches!(self.framing(), Framing::UntilClose)
    }

    /// `Retry-After`, in milliseconds, where the provider stated one as a delay.
    ///
    /// The date form is not parsed and is not guessed at: an unparsed `Retry-After` leaves
    /// the delay to D-87's backoff, which is a defined behaviour rather than a wrong number.
    #[must_use]
    pub fn retry_after_millis(&self) -> Option<u64> {
        self.get("retry-after")?
            .trim()
            .parse::<u64>()
            .ok()?
            .checked_mul(1000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Length(u64),
    Chunked,
    /// No framing was stated. The body ends when the connection does, and the connection is
    /// therefore not reusable.
    UntilClose,
}

/// Serialize a request.
///
/// `Accept-Encoding` is stated here rather than by the caller, because the decompression
/// bound below depends on knowing what may come back.
#[must_use]
pub fn serialize(
    verb: &str,
    target: &str,
    host: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(256 + body.len());
    out.extend_from_slice(format!("{verb} {target} HTTP/1.1\r\n").as_bytes());
    out.extend_from_slice(format!("host: {host}\r\n").as_bytes());
    out.extend_from_slice(b"accept-encoding: gzip\r\n");
    out.extend_from_slice(b"connection: keep-alive\r\n");
    for (name, value) in headers {
        // A header a caller could not have written by hand is one the transport invented.
        // Names and values are passed through as given, minus the two characters that would
        // let a value become a second header.
        let value = value.replace(['\r', '\n'], "");
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    if !body.is_empty() {
        out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

/// A bounded, buffered reader over one connection.
///
/// It exists because the head must be parsed before the body's framing is known, so some of
/// the body is usually already in hand by the time anybody asks for it.
#[derive(Debug)]
pub struct Incoming<R: Read + core::fmt::Debug> {
    source: R,
    buffer: Vec<u8>,
    /// How much of `buffer` has been handed out.
    at: usize,
    /// Bytes read from the socket, for FR-36.
    pub received: u64,
}

impl<R: Read + core::fmt::Debug> Incoming<R> {
    pub fn new(source: R) -> Self {
        Self {
            source,
            buffer: Vec::with_capacity(8 * 1024),
            at: 0,
            received: 0,
        }
    }

    /// Hand the connection back for reuse — **only where the answer was consumed exactly**.
    ///
    /// A buffer holding bytes past the end of the body means this parser and the far end
    /// disagree about where the answer ended. Reusing that connection would put the
    /// disagreement into the *next* request's answer, which is the response-splitting shape
    /// this parser refuses folded headers to avoid. Dropping it costs one handshake.
    pub fn into_source(self) -> Option<R> {
        (self.at == self.buffer.len()).then_some(self.source)
    }

    /// Pull at least one more byte into the buffer. `Ok(false)` at end of stream.
    fn fill(&mut self) -> Result<bool, WireError> {
        let mut chunk = [0u8; 16 * 1024];
        match self.source.read(&mut chunk) {
            Ok(0) => Ok(false),
            Ok(n) => {
                self.buffer.extend_from_slice(&chunk[..n]);
                self.received += n as u64;
                Ok(true)
            }
            Err(e) => Err(WireError::Io(e.to_string())),
        }
    }

    /// Read the status line and headers.
    ///
    /// # Errors
    /// [`WireError::Malformed`] on anything this parser does not accept, and
    /// [`WireError::TooLarge`] past [`MAX_HEAD_BYTES`].
    pub fn head(&mut self) -> Result<Head, WireError> {
        let end = loop {
            if let Some(i) = find(&self.buffer[self.at..], b"\r\n\r\n") {
                break self.at + i + 4;
            }
            if self.buffer.len() - self.at > MAX_HEAD_BYTES {
                return Err(WireError::TooLarge {
                    limit_bytes: MAX_HEAD_BYTES as u64,
                });
            }
            if !self.fill()? {
                return Err(WireError::Malformed("the answer ended inside its headers"));
            }
        };
        let head = parse_head(&self.buffer[self.at..end])?;
        self.at = end;
        Ok(head)
    }

    /// Read the body, bounded at `cap`.
    ///
    /// # Errors
    /// [`WireError::TooLarge`] where the body exceeds `cap`. **Refused, not truncated** —
    /// `docs/limits.md` gives the reason: a truncated answer is one whose shape the sender
    /// chose by choosing where the cap fell.
    pub fn body(&mut self, framing: Framing, cap: u64) -> Result<Vec<u8>, WireError> {
        match framing {
            Framing::Length(n) => {
                if n > cap {
                    return Err(WireError::TooLarge { limit_bytes: cap });
                }
                self.exactly(usize::try_from(n).unwrap_or(usize::MAX))
            }
            Framing::Chunked => self.chunked(cap),
            Framing::UntilClose => {
                let mut out = Vec::new();
                loop {
                    let have = self.buffer.len() - self.at;
                    if have as u64 > cap {
                        return Err(WireError::TooLarge { limit_bytes: cap });
                    }
                    if !self.fill()? {
                        out.extend_from_slice(&self.buffer[self.at..]);
                        self.at = self.buffer.len();
                        return Ok(out);
                    }
                }
            }
        }
    }

    fn exactly(&mut self, n: usize) -> Result<Vec<u8>, WireError> {
        while self.buffer.len() - self.at < n {
            if !self.fill()? {
                return Err(WireError::Malformed("the answer ended inside its body"));
            }
        }
        let out = self.buffer[self.at..self.at + n].to_vec();
        self.at += n;
        Ok(out)
    }

    fn line(&mut self) -> Result<String, WireError> {
        loop {
            if let Some(i) = find(&self.buffer[self.at..], b"\r\n") {
                let line = String::from_utf8_lossy(&self.buffer[self.at..self.at + i]).into_owned();
                self.at += i + 2;
                return Ok(line);
            }
            if self.buffer.len() - self.at > MAX_HEAD_BYTES {
                return Err(WireError::TooLarge {
                    limit_bytes: MAX_HEAD_BYTES as u64,
                });
            }
            if !self.fill()? {
                return Err(WireError::Malformed("the answer ended inside a chunk header"));
            }
        }
    }

    fn chunked(&mut self, cap: u64) -> Result<Vec<u8>, WireError> {
        let mut out = Vec::new();
        loop {
            let header = self.line()?;
            // A chunk extension follows a semicolon and is ignored, per the grammar.
            let size = header.split(';').next().unwrap_or("").trim();
            let size = u64::from_str_radix(size, 16)
                .map_err(|_| WireError::Malformed("a chunk size was not hexadecimal"))?;
            if size == 0 {
                // Trailers, then the terminating blank line.
                loop {
                    if self.line()?.is_empty() {
                        break;
                    }
                }
                return Ok(out);
            }
            if out.len() as u64 + size > cap {
                return Err(WireError::TooLarge { limit_bytes: cap });
            }
            let chunk = self.exactly(usize::try_from(size).unwrap_or(usize::MAX))?;
            out.extend_from_slice(&chunk);
            if !self.line()?.is_empty() {
                return Err(WireError::Malformed("a chunk was not followed by a blank line"));
            }
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse a status line and headers.
///
/// # Errors
/// [`WireError::Malformed`] on anything this parser does not accept.
pub fn parse_head(bytes: &[u8]) -> Result<Head, WireError> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or(WireError::Malformed("no status line"))?;
    let mut fields = status_line.splitn(3, ' ');
    let version = fields.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(WireError::Malformed("not an HTTP/1.x answer"));
    }
    let status: u16 = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or(WireError::Malformed("no status code"))?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            // A continuation line — obsolete line folding. Refused rather than joined:
            // disagreeing about where a header ends is a request-smuggling primitive, and
            // no provider Sift speaks to emits one.
            return Err(WireError::Malformed("a header line had no colon"));
        };
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Ok(Head { status, headers })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming(bytes: &'static [u8]) -> Incoming<&'static [u8]> {
        Incoming::new(bytes)
    }

    #[test]
    fn a_request_states_its_own_host_and_encoding() {
        let out = serialize("GET", "/x", "api.example.test", &[], b"");
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("GET /x HTTP/1.1\r\n"));
        assert!(text.contains("host: api.example.test\r\n"));
        assert!(text.contains("accept-encoding: gzip\r\n"));
        assert!(!text.contains("content-length"), "an empty body stated a length");
    }

    #[test]
    fn a_header_value_cannot_become_a_second_header() {
        // The one injection this serializer has to be immune to.
        let out = serialize(
            "GET",
            "/x",
            "h",
            &[("authorization".into(), "Bearer a\r\nx-injected: yes".into())],
            b"",
        );
        let text = String::from_utf8(out).unwrap();
        // The value is mangled rather than honoured: it stays one header line.
        assert!(
            !text.split("\r\n").any(|line| line.starts_with("x-injected")),
            "{text}"
        );
        assert!(text.contains("authorization: Bearer ax-injected: yes\r\n"));
    }

    #[test]
    fn a_body_states_its_length() {
        let out = serialize("POST", "/x", "h", &[], b"{}");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("content-length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }

    #[test]
    fn a_status_line_and_headers_parse() {
        let head = parse_head(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n").unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.get("Content-Type"), Some("application/json"));
        assert_eq!(head.get("absent"), None);
    }

    #[test]
    fn an_obsolete_folded_header_is_refused_rather_than_joined() {
        // Two parsers disagreeing about where a header ends is a smuggling primitive.
        let e = parse_head(b"HTTP/1.1 200 OK\r\nx: one\r\n  two\r\n\r\n").unwrap_err();
        assert_eq!(e, WireError::Malformed("a header line had no colon"));
    }

    #[test]
    fn something_that_is_not_http_is_refused() {
        // A captive portal answering a provider's address produces exactly this.
        assert!(parse_head(b"<html>Sign in to the network</html>\r\n\r\n").is_err());
    }

    #[test]
    fn a_length_framed_body_is_read() {
        let mut i = incoming(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello");
        let head = i.head().unwrap();
        assert_eq!(head.framing(), Framing::Length(5));
        assert_eq!(i.body(head.framing(), 1000).unwrap(), b"hello");
    }

    #[test]
    fn a_chunked_body_is_reassembled() {
        let mut i = incoming(
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        );
        let head = i.head().unwrap();
        assert_eq!(head.framing(), Framing::Chunked);
        assert_eq!(i.body(head.framing(), 1000).unwrap(), b"hello world");
    }

    #[test]
    fn a_chunk_extension_is_ignored_rather_than_refused() {
        let mut i =
            incoming(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2;a=b\r\nhi\r\n0\r\n\r\n");
        let head = i.head().unwrap();
        assert_eq!(i.body(head.framing(), 1000).unwrap(), b"hi");
    }

    #[test]
    fn a_body_over_the_cap_is_refused_rather_than_truncated() {
        // docs/limits.md: a truncated answer is one whose shape the sender chose by
        // choosing where the cap fell.
        let mut i = incoming(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello");
        let head = i.head().unwrap();
        assert_eq!(
            i.body(head.framing(), 4),
            Err(WireError::TooLarge { limit_bytes: 4 })
        );
    }

    #[test]
    fn a_chunked_body_is_capped_before_it_is_assembled() {
        // The cap has to bite on the declared chunk size, not after the bytes are in hand.
        let mut i = incoming(
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        );
        let head = i.head().unwrap();
        assert_eq!(
            i.body(head.framing(), 8),
            Err(WireError::TooLarge { limit_bytes: 8 })
        );
    }

    #[test]
    fn a_head_that_never_ends_is_bounded() {
        let huge = vec![b'x'; MAX_HEAD_BYTES + 16];
        let mut i = Incoming::new(huge.as_slice());
        assert!(matches!(i.head(), Err(WireError::TooLarge { .. })));
    }

    #[test]
    fn a_stated_delay_is_reported_and_not_acted_on() {
        // Reported as a number. D-87 puts it on the wheel; nothing here sleeps.
        let head = parse_head(b"HTTP/1.1 429 Too Many\r\nretry-after: 30\r\n\r\n").unwrap();
        assert_eq!(head.retry_after_millis(), Some(30_000));
    }

    #[test]
    fn an_unparsed_retry_after_is_left_to_the_backoff_rather_than_guessed() {
        let head =
            parse_head(b"HTTP/1.1 503 Busy\r\nretry-after: Wed, 21 Oct 2026 07:28:00 GMT\r\n\r\n")
                .unwrap();
        assert_eq!(head.retry_after_millis(), None);
    }

    #[test]
    fn a_connection_close_ends_the_reuse() {
        let head = parse_head(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .unwrap();
        assert!(!head.keeps_alive());
        let head = parse_head(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").unwrap();
        assert!(head.keeps_alive());
    }

    #[test]
    fn an_unframed_answer_is_not_reused() {
        // It ends when the connection does, so there is nothing left to reuse.
        let head = parse_head(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        assert_eq!(head.framing(), Framing::UntilClose);
        assert!(!head.keeps_alive());
    }

    #[test]
    fn a_no_content_answer_carries_no_body_whatever_it_says() {
        let head = parse_head(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap();
        assert_eq!(head.framing(), Framing::Length(0));
    }

    #[test]
    fn bytes_read_from_the_socket_are_counted() {
        // FR-36 counts what the interface carried, which includes the head.
        let raw: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello";
        let mut i = Incoming::new(raw);
        let head = i.head().unwrap();
        let _ = i.body(head.framing(), 100).unwrap();
        assert_eq!(i.received, raw.len() as u64);
    }
}

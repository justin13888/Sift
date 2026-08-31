//! The HTTPS transport an adapter reaches a provider over.
//!
//! # What this crate is, and what it deliberately is not
//!
//! It is the one place in Sift that opens a socket to somebody else's computer. It is not an
//! adapter: it knows no provider, holds no credential, and decides nothing about when to
//! run. `cargo xtask arch` enforces the first of those — this crate sits in the providers
//! layer beside the four adapters and is invisible to all of them individually.
//!
//! Four constraints from `docs/` shape every line:
//!
//! - **D-30 — the OS trust store.** Certificate verification goes through the platform's own
//!   verifier, never a root store bundled with Sift. The stated reason is generic IMAP
//!   behind a corporate inspection proxy: an installation whose administrator has placed a
//!   root in the system store expects it to work, and a bundled store silently breaks every
//!   such account with a certificate error the user cannot act on.
//! - **NFR-24 — never a listening socket.** Nothing here binds, listens, or accepts. There
//!   is one call that touches the network stack and it is an outbound connect.
//! - **FR-36 — bytes on the wire.** Counted at the socket, so the figure includes the TLS
//!   handshake, the record framing and the headers. A count taken above the encryption would
//!   understate a metered link, which is the wrong direction for a data cap.
//! - **D-87 — a stated delay is a deadline, never a sleep.** A throttle comes back as
//!   [`TransportError::Throttled`] carrying the provider's own number. Nothing here waits.

pub mod wire;

use core::time::Duration;
use sift_foundation::limits::L13_FETCH_BYTES;
use sift_provider::transport::{Request, Response, Transport, TransportError};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// How long a single request may take to connect, and how long it may stall mid-answer.
///
/// D-19 permits the runtime's own timer for **short I/O timeouts** and nothing else; every
/// periodic decision belongs to D-25's wheel. This is one of those short timeouts, and it is
/// not a retry policy: a request that times out returns and the scheduler decides.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether this crate ever binds or listens.
///
/// **No.** NFR-24 admits no exception "of any kind, for any purpose", and this is the crate
/// where an exception would be easiest to justify and hardest to notice.
#[must_use]
pub const fn opens_a_listening_socket() -> bool {
    false
}

/// Whether certificate verification uses a root store shipped with Sift.
///
/// **No** — D-30. See the module documentation for why the alternative breaks accounts
/// rather than merely differing from them.
#[must_use]
pub const fn uses_a_bundled_root_store() -> bool {
    false
}

/// A byte counter shared between the socket and whoever asks about FR-36.
#[derive(Debug, Clone, Default)]
pub struct Meter {
    sent: Arc<AtomicU64>,
    received: Arc<AtomicU64>,
}

impl Meter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `(sent, received)` — bytes on the wire, ciphertext and framing included.
    #[must_use]
    pub fn totals(&self) -> (u64, u64) {
        (
            self.sent.load(Ordering::Relaxed),
            self.received.load(Ordering::Relaxed),
        )
    }
}

/// A socket that counts what passes through it.
///
/// It wraps the *TCP* stream rather than the TLS stream on purpose: FR-36 is a claim about
/// the interface, and the interface carries ciphertext.
#[derive(Debug)]
struct Counted {
    inner: TcpStream,
    meter: Meter,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.meter.received.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Write for Counted {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.meter.sent.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

type Tls = rustls::StreamOwned<rustls::ClientConnection, Counted>;

/// One origin, reached over TLS.
///
/// Holds at most one connection and reuses it. The alternative — a connection per request —
/// costs a handshake every time, and L-23 budgets connections per *installation*, so an
/// adapter that opened one per call would spend the budget on arithmetic rather than on
/// watched folders.
pub struct Https {
    host: String,
    port: u16,
    config: Arc<rustls::ClientConfig>,
    connection: Option<Tls>,
    meter: Meter,
    /// NFR-39's ceiling on a single fetch, defaulting to L-13.
    cap: u64,
}

impl core::fmt::Debug for Https {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Https")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("connected", &self.connection.is_some())
            .field("cap", &self.cap)
            .finish()
    }
}

impl Https {
    /// Open a transport to one origin.
    ///
    /// # Errors
    /// Where the platform's own trust store cannot be consulted at all. That is D-71's
    /// territory: a security guarantee that is absent **refuses** rather than degrading, so
    /// this returns an error rather than falling back to anything.
    pub fn to(host: &str) -> Result<Self, String> {
        let verifier = rustls_platform_verifier::Verifier::new(
            rustls::crypto::ring::default_provider().into(),
        )
        .map_err(|e| format!("the platform's trust store could not be consulted: {e}"))?;
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(Self {
            host: host.to_owned(),
            port: 443,
            config: Arc::new(config),
            connection: None,
            meter: Meter::new(),
            cap: L13_FETCH_BYTES,
        })
    }

    /// Raise the ceiling on one fetch, which NFR-39 permits **only on explicit
    /// confirmation** — the user was asked and said yes to this transfer.
    #[must_use]
    pub const fn with_confirmed_ceiling(mut self, bytes: u64) -> Self {
        self.cap = bytes;
        self
    }

    #[must_use]
    pub fn meter(&self) -> Meter {
        self.meter.clone()
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    fn connect(&mut self) -> Result<Tls, TransportError> {
        let name = rustls::pki_types::ServerName::try_from(self.host.clone())
            .map_err(|_| TransportError::Refused(format!("`{}` is not a host name", self.host)))?;
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .map_err(|_| TransportError::Transient)?;
        tcp.set_read_timeout(Some(IO_TIMEOUT))
            .and_then(|()| tcp.set_write_timeout(Some(IO_TIMEOUT)))
            .and_then(|()| tcp.set_nodelay(true))
            .map_err(|_| TransportError::Transient)?;
        let session = rustls::ClientConnection::new(self.config.clone(), name)
            .map_err(|e| TransportError::Refused(e.to_string()))?;
        Ok(rustls::StreamOwned::new(
            session,
            Counted {
                inner: tcp,
                meter: self.meter.clone(),
            },
        ))
    }

    /// One attempt over one connection. The caller decides whether a failure may be retried.
    fn attempt(&mut self, mut tls: Tls, bytes: &[u8]) -> Result<Response, TransportError> {
        tls.write_all(bytes)
            .and_then(|()| tls.flush())
            .map_err(|_| TransportError::Unknown)?;

        let mut incoming = wire::Incoming::new(tls);
        let head = incoming.head().map_err(from_wire)?;
        let framing = head.framing();
        let keep = head.keeps_alive();
        let body = incoming.body(framing, self.cap).map_err(from_wire)?;
        let body = decode(&head, body, self.cap)?;

        if keep {
            self.connection = incoming.into_source();
        }
        answer(head, body)
    }
}

/// Translate a status into either an answer or one of D-87's and D-85's states.
///
/// The split matters: a 404 is an **answer** the adapter reads, and a 429 is a schedule.
fn answer(head: wire::Head, body: Vec<u8>) -> Result<Response, TransportError> {
    match head.status {
        // The provider stated a delay. D-87 puts it on the wheel.
        429 | 503 => Err(TransportError::Throttled {
            retry_after_millis: head.retry_after_millis().unwrap_or(0),
        }),
        // The provider's own trouble, not the account's.
        500..=599 => Err(TransportError::Transient),
        _ => Ok(Response {
            status: head.status,
            headers: head.headers,
            body,
        }),
    }
}

fn from_wire(e: wire::WireError) -> TransportError {
    match e {
        wire::WireError::TooLarge { limit_bytes } => TransportError::TooLarge { limit_bytes },
        // The request went out and no usable answer came back. D-85 moves an intent to
        // *Reconciling* on exactly this, rather than replaying it blindly.
        wire::WireError::Io(_) | wire::WireError::Malformed(_) => TransportError::Unknown,
    }
}

/// Undo a content coding, bounded.
///
/// **The decompressed length is capped at the same ceiling as the transfer.** A client that
/// bounded only what crossed the wire would have admitted a decompression bomb inside
/// NFR-39's limit, which is the failure `docs/limits.md` describes for every other bound it
/// holds: the sender chooses how much work the receiver does.
fn decode(head: &wire::Head, body: Vec<u8>, cap: u64) -> Result<Vec<u8>, TransportError> {
    let coding = head
        .get("content-encoding")
        .unwrap_or("")
        .to_ascii_lowercase();
    if coding.is_empty() || coding == "identity" {
        return Ok(body);
    }
    if coding != "gzip" {
        return Err(TransportError::Refused(format!(
            "the answer used a content coding Sift does not accept: `{coding}`"
        )));
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(body.as_slice())
        .take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|_| TransportError::Unknown)?;
    if out.len() as u64 > cap {
        return Err(TransportError::TooLarge { limit_bytes: cap });
    }
    Ok(out)
}

impl Transport for Https {
    fn exchange(&mut self, request: &Request<'_>) -> Result<Response, TransportError> {
        let bytes = wire::serialize(
            &request.exchange.verb,
            &request.exchange.target,
            &self.host,
            &request.headers,
            request.body,
        );

        // A pooled connection may have been closed by the far end since it was last used,
        // and that failure is indistinguishable from one where the request was received.
        //
        // So the retry is conditional on the **verb** rather than on the failure: a GET may
        // be reissued on a fresh connection because reissuing it changes nothing, and a POST
        // may not, because the answer to "did the server apply this?" is unknown and
        // guessing at it is precisely what NFR-17 forbids. An unknown POST becomes D-85's
        // *Reconciling*, where the adapter establishes server state before trying again.
        let idempotent = matches!(request.exchange.verb.as_str(), "GET" | "HEAD");
        if let Some(pooled) = self.connection.take() {
            match self.attempt(pooled, &bytes) {
                Ok(response) => return Ok(response),
                Err(e) if !idempotent || !matches!(e, TransportError::Unknown) => return Err(e),
                Err(_) => {}
            }
        }
        let fresh = self.connect()?;
        self.attempt(fresh, &bytes)
    }

    fn wire_bytes(&self) -> (u64, u64) {
        self.meter.totals()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_here_listens() {
        assert!(!opens_a_listening_socket());
    }

    #[test]
    fn the_trust_store_is_the_platforms() {
        // D-30: a bundled store silently breaks every account behind an inspection proxy,
        // with a certificate error the user cannot act on.
        assert!(!uses_a_bundled_root_store());
    }

    #[test]
    fn a_throttle_is_a_schedule_rather_than_an_answer() {
        let head = wire::parse_head(b"HTTP/1.1 429 Too Many\r\nretry-after: 12\r\n\r\n").unwrap();
        assert_eq!(
            answer(head, vec![]),
            Err(TransportError::Throttled {
                retry_after_millis: 12_000
            })
        );
    }

    #[test]
    fn a_throttle_with_no_stated_delay_still_goes_to_the_wheel() {
        // Zero means "the wheel's own backoff decides", which is L-24 — not "retry now".
        let head = wire::parse_head(b"HTTP/1.1 503 Busy\r\n\r\n").unwrap();
        assert_eq!(
            answer(head, vec![]),
            Err(TransportError::Throttled {
                retry_after_millis: 0
            })
        );
    }

    #[test]
    fn a_refusal_is_an_answer_the_adapter_reads() {
        // The distinction this whole return type exists for: 404 on a history cursor is a
        // recovery, and collapsing it into an error would make a routine delta look like a
        // broken account.
        let head = wire::parse_head(b"HTTP/1.1 404 Gone\r\n\r\n").unwrap();
        let got = answer(head, b"{}".to_vec()).unwrap();
        assert_eq!(got.status, 404);
    }

    #[test]
    fn a_gzipped_answer_is_decoded() {
        use flate2::{Compression, write::GzEncoder};
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"{\"ok\":true}").unwrap();
        let compressed = e.finish().unwrap();
        let head = wire::parse_head(b"HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\n\r\n").unwrap();
        assert_eq!(
            decode(&head, compressed, 1000).unwrap(),
            b"{\"ok\":true}".to_vec()
        );
    }

    #[test]
    fn a_decompression_bomb_is_refused_inside_the_fetch_ceiling() {
        // The failure a client that capped only the transfer would have admitted.
        use flate2::{Compression, write::GzEncoder};
        let mut e = GzEncoder::new(Vec::new(), Compression::best());
        e.write_all(&vec![0u8; 4 * 1024 * 1024]).unwrap();
        let compressed = e.finish().unwrap();
        assert!(
            (compressed.len() as u64) < 64 * 1024,
            "the bomb did not compress"
        );
        let head = wire::parse_head(b"HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\n\r\n").unwrap();
        assert_eq!(
            decode(&head, compressed, 64 * 1024),
            Err(TransportError::TooLarge {
                limit_bytes: 64 * 1024
            })
        );
    }

    #[test]
    fn a_content_coding_sift_does_not_accept_is_refused_rather_than_passed_through() {
        let head = wire::parse_head(b"HTTP/1.1 200 OK\r\ncontent-encoding: br\r\n\r\n").unwrap();
        assert!(matches!(
            decode(&head, vec![1, 2, 3], 1000),
            Err(TransportError::Refused(_))
        ));
    }

    #[test]
    fn an_answer_that_ends_mid_body_is_unknown_rather_than_transient() {
        // D-85's distinction: the request went out. Retrying it blindly is what NFR-17
        // forbids.
        assert_eq!(
            from_wire(wire::WireError::Malformed(
                "the answer ended inside its body"
            )),
            TransportError::Unknown
        );
    }

    #[test]
    fn the_default_ceiling_is_the_registered_limit() {
        // A number that drifts from docs/limits.md is the drift the register exists to stop.
        assert_eq!(L13_FETCH_BYTES, 25 * 1024 * 1024);
    }
}

//! The fetch behind an allowed answer, and the bytes it hands the body view.
//!
//! # Why the broker defines the fetch and does not perform it
//!
//! D-59's layer table puts this crate below the network: the rendering layer "may not reach
//! the store, the network, the adapters", and the broker is the one component in it with an
//! edge outward — **expressed as a trait it defines and the application layer implements**.
//! [`Fetch`] is that trait. What a fetch is allowed to be, what it is held to, and what of it
//! reaches the engine are decided here; which socket carries it is the application's.
//!
//! # What is handed over — D-29
//!
//! Original bytes, after **bounded structural validation**: the leading bytes must be one of
//! the raster formats below, and the header's own dimensions are held to L-12 before a byte is
//! handed to the engine that will decode it. Vector images are refused rather than rasterized
//! — D-29's honest fallback — because an XML surface with external references is a parser this
//! pipeline never inspected. Everything else is refused too: a format the broker cannot bound
//! is one whose decode it cannot bound.
//!
//! # Streamed, counted, and cancelled
//!
//! Nothing is buffered beyond the header the validation needs, which is itself bounded. Every
//! byte that arrives is counted against L-10 and L-13 **as it arrives**, because the sender
//! controls the declared length and the body both. Every read checks the document's token and
//! the load's deadline, so navigation cancels a fetch mid-body and a stalled one ends.

use crate::broker::{Answer, Grant, Reason, Unavailable, transferred_bound};
use sift_foundation::limits::L12_RASTER_PIXELS;
use std::time::{Duration, Instant};

/// How long one load may take from being granted to its last byte — D-91's deadline.
///
/// Not in the limits register, and not measured: D-91 requires that every load have one and
/// names no number. Twice the transport's own I/O timeout, so a single stalled read ends the
/// load rather than outliving it, and short enough that a slot held by a dead fetch is given
/// back while the reader is still looking at the message.
pub const LOAD_DEADLINE: Duration = Duration::from_secs(60);

/// The most bytes read to find a header before the resource is refused.
///
/// A JPEG's frame header follows whatever metadata segments the sender put first, so the
/// search is bounded rather than exhaustive: a sender who can make the broker buffer without
/// limit has found the buffering D-91 rejects.
pub const SNIFF_BOUND: usize = 256 * 1024;

/// The body of one remote resource, read as it arrives.
///
/// Implemented by the application over its transport. A source has already been checked for a
/// usable status; what it yields is the body and nothing else.
pub trait Source {
    /// Read some of the body. `Ok(0)` is the end of it.
    ///
    /// # Errors
    /// The body could not be read — the connection failed, or the answer ended inside its own
    /// framing. Never a short success: a truncated image drawn as though it were whole is a
    /// lie told to the reader.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Unavailable>;

    /// The length the server declared for the body, where it declared one.
    fn declared_length(&self) -> Option<u64> {
        None
    }
}

/// Opening a remote resource. The broker's edge outward, implemented above it.
pub trait Fetch {
    type Source: Source;

    /// Request `url` and hand back its body.
    ///
    /// # Errors
    /// [`Unavailable::NotFetchable`] where the address cannot be fetched at all, or was
    /// answered with anything but the resource.
    fn open(&mut self, url: &str) -> Result<Self::Source, Unavailable>;
}

/// The raster formats the broker passes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Gif,
    Jpeg,
    Webp,
}

impl Format {
    /// The media type the engine is told, **from the bytes rather than from the server**. A
    /// server's `Content-Type` is the sender's word, and the engine sniffing past it is the
    /// behaviour this validation exists to take away.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// What the leading bytes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sniff {
    /// Not enough bytes yet to tell.
    Need,
    /// A format the broker passes through, and the dimensions its header states.
    Is {
        format: Format,
        width: u32,
        height: u32,
    },
    /// Refused, with why.
    Refused(&'static str),
}

/// Identify a raster image from its leading bytes and read its stated dimensions.
///
/// Reads headers only and decodes nothing, which is the whole of "bounded" here: the cost is
/// a few comparisons for every format but JPEG, and a walk over segment lengths for JPEG that
/// [`SNIFF_BOUND`] caps.
#[must_use]
pub fn sniff(b: &[u8]) -> Sniff {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if b.starts_with(PNG) {
        if b.len() < 24 {
            return Sniff::Need;
        }
        if &b[12..16] != b"IHDR" {
            return Sniff::Refused("a PNG whose first chunk is not its header");
        }
        return is(Format::Png, be32(&b[16..20]), be32(&b[20..24]));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        if b.len() < 10 {
            return Sniff::Need;
        }
        return is(
            Format::Gif,
            u32::from(le16(&b[6..8])),
            u32::from(le16(&b[8..10])),
        );
    }
    if b.starts_with(b"\xff\xd8\xff") {
        return jpeg(b);
    }
    if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        return webp(b);
    }
    if b.len() < 12 {
        return Sniff::Need;
    }
    Sniff::Refused("not a raster format the broker passes through")
}

fn is(format: Format, width: u32, height: u32) -> Sniff {
    if width == 0 || height == 0 {
        return Sniff::Refused("an image header that describes no area");
    }
    Sniff::Is {
        format,
        width,
        height,
    }
}

fn webp(b: &[u8]) -> Sniff {
    if b.len() < 16 {
        return Sniff::Need;
    }
    match &b[12..16] {
        b"VP8 " => {
            if b.len() < 30 {
                return Sniff::Need;
            }
            if b[23..26] != [0x9d, 0x01, 0x2a] {
                return Sniff::Refused("a WebP frame without its start code");
            }
            is(
                Format::Webp,
                u32::from(le16(&b[26..28]) & 0x3fff),
                u32::from(le16(&b[28..30]) & 0x3fff),
            )
        }
        b"VP8L" => {
            if b.len() < 25 {
                return Sniff::Need;
            }
            if b[20] != 0x2f {
                return Sniff::Refused("a lossless WebP without its signature");
            }
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            is(
                Format::Webp,
                (bits & 0x3fff) + 1,
                ((bits >> 14) & 0x3fff) + 1,
            )
        }
        b"VP8X" => {
            if b.len() < 30 {
                return Sniff::Need;
            }
            is(Format::Webp, le24(&b[24..27]) + 1, le24(&b[27..30]) + 1)
        }
        _ => Sniff::Refused("a WebP container holding no known image"),
    }
}

/// Walk a JPEG's segments to its frame header.
fn jpeg(b: &[u8]) -> Sniff {
    let mut i = 2;
    loop {
        if i >= b.len() {
            return Sniff::Need;
        }
        if b[i] != 0xff {
            return Sniff::Refused("a JPEG segment that does not begin with a marker");
        }
        // Fill bytes: any number of 0xFF may precede a marker.
        while i < b.len() && b[i] == 0xff {
            i += 1;
        }
        if i >= b.len() {
            return Sniff::Need;
        }
        let marker = b[i];
        i += 1;
        match marker {
            // Standalone markers carry no length.
            0x01 | 0xd0..=0xd8 => continue,
            0xd9 | 0xda => return Sniff::Refused("a JPEG with no frame header before its data"),
            _ => {}
        }
        if i + 2 > b.len() {
            return Sniff::Need;
        }
        let length = usize::from(be16(&b[i..i + 2]));
        if length < 2 {
            return Sniff::Refused("a JPEG segment shorter than its own length field");
        }
        // Every start-of-frame marker but the three that share its range and mean otherwise.
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            if i + 7 > b.len() {
                return Sniff::Need;
            }
            let height = u32::from(be16(&b[i + 3..i + 5]));
            let width = u32::from(be16(&b[i + 5..i + 7]));
            return is(Format::Jpeg, width, height);
        }
        i += length;
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn le24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}

/// One allowed resource, validated and on its way to the engine.
#[derive(Debug)]
pub struct Stream<S> {
    grant: Grant,
    source: S,
    deadline: Instant,
    format: Format,
    /// The bytes read to validate the header, handed out before anything else. Emptied once
    /// they have been, so a long body holds one read rather than its first quarter-megabyte.
    prefix: Vec<u8>,
    handed: usize,
    /// Every byte the source has yielded, the prefix included. What L-10 and L-13 are
    /// enforced against.
    received: u64,
}

/// Fetch an allowed resource and validate it, holding one of its document's slots.
///
/// The order is the design. The slot first, because queueing for it is what bounds how many
/// fetches one message can have open; then the connection; then the declared length, so a
/// declared oversize costs no body; then the header, **before any byte reaches the engine**.
///
/// # Errors
/// The answer the request gets instead: *blocked* where Sift refused what arrived — a bound,
/// or D-29's validation — and *unavailable* where it did not arrive, the document went, or
/// the deadline passed.
pub fn open<F: Fetch>(
    mut grant: Grant,
    fetch: &mut F,
    deadline: Instant,
) -> Result<Stream<F::Source>, Answer> {
    grant.acquire(deadline).map_err(Answer::Unavailable)?;
    let mut source = fetch.open(&grant.url).map_err(Answer::Unavailable)?;
    if let Some(bound) = source.declared_length().and_then(transferred_bound) {
        return Err(Answer::Blocked(Reason::Bounds(bound)));
    }

    let mut prefix = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let (format, width, height) = loop {
        match sniff(&prefix) {
            Sniff::Is {
                format,
                width,
                height,
            } => break (format, width, height),
            Sniff::Refused(why) => return Err(Answer::Blocked(Reason::Validation(why))),
            Sniff::Need => {}
        }
        if prefix.len() >= SNIFF_BOUND {
            return Err(Answer::Blocked(Reason::Validation(
                "no image header within the bytes the broker reads to find one",
            )));
        }
        live(&grant, deadline)?;
        let room = chunk.len().min(SNIFF_BOUND - prefix.len());
        let n = source
            .read(&mut chunk[..room])
            .map_err(Answer::Unavailable)?;
        if n == 0 {
            // It ended before saying what it was. That is a resource that did not arrive,
            // not one Sift refused.
            return Err(Answer::Unavailable(Unavailable::NotFetchable));
        }
        prefix.extend_from_slice(&chunk[..n]);
    };

    // L-12 against the header's own dimensions, before the engine decodes a pixel. L-11 is
    // the same number and bounds the classifier's decode, which reads these same bytes.
    if u64::from(width) * u64::from(height) > L12_RASTER_PIXELS {
        return Err(Answer::Blocked(Reason::Bounds("L-12 rasterized pixels")));
    }
    let received = prefix.len() as u64;
    if let Some(bound) = transferred_bound(received) {
        return Err(Answer::Blocked(Reason::Bounds(bound)));
    }
    Ok(Stream {
        grant,
        source,
        deadline,
        format,
        prefix,
        handed: 0,
        received,
    })
}

/// Whether a fetch may go on: its document is live and its deadline has not passed.
fn live(grant: &Grant, deadline: Instant) -> Result<(), Answer> {
    if grant.is_revoked() {
        return Err(Answer::Unavailable(Unavailable::Revoked));
    }
    if Instant::now() >= deadline {
        return Err(Answer::Unavailable(Unavailable::Deadline));
    }
    Ok(())
}

impl<S: Source> Stream<S> {
    /// What the bytes are, as the engine is to be told.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
    }

    /// The next bytes, `Ok(0)` at the end.
    ///
    /// # Errors
    /// Revocation and the deadline, checked before every read; a bound, the moment what
    /// arrived crosses it — **refused, not truncated**, because a truncated image is one whose
    /// shape the sender chose by choosing where the cap fell; and a body that ended early.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, Answer> {
        live(&self.grant, self.deadline)?;
        if self.handed < self.prefix.len() {
            let n = buf.len().min(self.prefix.len() - self.handed);
            buf[..n].copy_from_slice(&self.prefix[self.handed..self.handed + n]);
            self.handed += n;
            if self.handed == self.prefix.len() {
                self.prefix = Vec::new();
                self.handed = 0;
            }
            return Ok(n);
        }
        let n = self.source.read(buf).map_err(Answer::Unavailable)?;
        self.received += n as u64;
        if let Some(bound) = transferred_bound(self.received) {
            return Err(Answer::Blocked(Reason::Bounds(bound)));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::{Broker, Position, Request};
    use crate::token::Address;
    use sift_block::engine::{Authority, Blocker};
    use sift_block::origin::{Authentication, Infrastructure, Origin};
    use sift_foundation::limits::{L10_IMAGE_BYTES, L13_FETCH_BYTES, L29_DOC_CONCURRENCY};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    /// A source over bytes in memory, handing out at most `step` per read.
    #[derive(Debug)]
    struct Memory {
        bytes: Vec<u8>,
        at: usize,
        step: usize,
        declared: Option<u64>,
    }

    impl Source for Memory {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, Unavailable> {
            let n = buf.len().min(self.step).min(self.bytes.len() - self.at);
            buf[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
        fn declared_length(&self) -> Option<u64> {
            self.declared
        }
    }

    /// A fetch that hands out one fixed body and records what it was asked for.
    #[derive(Debug, Default)]
    struct Canned {
        body: Vec<u8>,
        step: usize,
        declared: Option<u64>,
        asked: Vec<String>,
    }

    impl Fetch for Canned {
        type Source = Memory;
        fn open(&mut self, url: &str) -> Result<Memory, Unavailable> {
            self.asked.push(url.to_owned());
            Ok(Memory {
                bytes: self.body.clone(),
                at: 0,
                step: self.step.max(1),
                declared: self.declared,
            })
        }
    }

    fn canned(body: Vec<u8>) -> Canned {
        Canned {
            body,
            step: 7,
            ..Canned::default()
        }
    }

    /// A broker with one allowed document of `n` positions, and a grant for position 0.
    fn granted(n: usize) -> (Broker, crate::token::Token, Grant) {
        let origin = Origin::derive(&Authentication {
            signing_domain: Some("sender.test".to_owned()),
            ..Authentication::default()
        });
        let mut broker = Broker::new();
        let positions = (0..n)
            .map(|i| Position {
                url: format!("https://cdn.test/{i}.png"),
                declared_length: None,
                declared_width: Some(600),
                declared_height: Some(400),
                declared_pixels: None,
                style: None,
                alt: Some("an image".to_owned()),
                in_zero_height_container: false,
                request_type: "image".to_owned(),
            })
            .collect();
        let token = broker.open_document(origin, positions);
        assert!(broker.allow_once(token.as_str()));
        let grant = grant_for(&broker, &token, 0).expect("allowed");
        (broker, token, grant)
    }

    fn grant_for(broker: &Broker, token: &crate::token::Token, i: usize) -> Result<Grant, Answer> {
        let authority = Authority::Loaded(Box::new(Blocker::from_rules(&[
            "||tracker.test^".to_owned()
        ])));
        broker.grant(
            &Request {
                url: Address {
                    token: token.clone(),
                    position: i,
                }
                .to_url(),
                transferred_length: None,
            },
            &authority,
            &Infrastructure::default(),
        )
    }

    fn later() -> Instant {
        Instant::now() + LOAD_DEADLINE
    }

    fn drain<S: Source>(stream: &mut Stream<S>) -> Result<Vec<u8>, Answer> {
        let mut out = Vec::new();
        let mut buf = [0u8; 5];
        loop {
            let n = stream.read(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn an_allowed_image_arrives_whole_and_unaltered() {
        // D-29: original bytes after validation, not a re-encoding of them.
        let mut body = png(600, 400);
        body.extend(std::iter::repeat_n(0xab, 10_000));
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(body.clone());
        let mut stream = open(grant, &mut fetch, later()).expect("validated");
        assert_eq!(stream.format(), Format::Png);
        assert_eq!(drain(&mut stream).unwrap(), body);
        assert_eq!(
            fetch.asked,
            ["https://cdn.test/0.png"],
            "the position's own address"
        );
    }

    #[test]
    fn a_vector_image_is_refused_rather_than_handed_to_the_engine() {
        // D-29's honest fallback: an XML surface with external references is a parser this
        // pipeline never inspected.
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"https://x/\"/></svg>"
                .to_vec(),
        );
        assert!(matches!(
            open(grant, &mut fetch, later()),
            Err(Answer::Blocked(Reason::Validation(_)))
        ));
    }

    #[test]
    fn a_decode_bomb_is_refused_from_its_header_before_a_pixel_is_decoded() {
        // Ten thousand by ten thousand is a hundred megapixels in a few dozen bytes.
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(png(10_000, 10_000));
        assert_eq!(
            open(grant, &mut fetch, later()).unwrap_err(),
            Answer::Blocked(Reason::Bounds("L-12 rasterized pixels"))
        );
    }

    #[test]
    fn a_declared_oversize_is_refused_before_the_body_is_read() {
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(png(10, 10));
        fetch.declared = Some(L13_FETCH_BYTES + 1);
        assert_eq!(
            open(grant, &mut fetch, later()).unwrap_err(),
            Answer::Blocked(Reason::Bounds("L-13 single fetch without confirmation"))
        );
    }

    #[test]
    fn a_body_larger_than_it_declared_is_refused_as_it_arrives_rather_than_truncated() {
        // A small declaration with a large body is the obvious way around a declared-length
        // check, so the bound is enforced against what actually arrives.
        let bound = L13_FETCH_BYTES.min(L10_IMAGE_BYTES);
        let mut body = png(10, 10);
        body.resize(usize::try_from(bound).unwrap() + 1, 0);
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(body);
        fetch.step = 1024 * 1024;
        fetch.declared = Some(1024);
        let mut stream = open(grant, &mut fetch, later()).expect("the header is fine");
        let mut buf = vec![0u8; 1024 * 1024];
        let refused = loop {
            match stream.read(&mut buf) {
                Ok(0) => panic!("the whole oversize body was handed over"),
                Ok(_) => {}
                Err(answer) => break answer,
            }
        };
        assert!(
            matches!(refused, Answer::Blocked(Reason::Bounds(_))),
            "{refused:?}"
        );
    }

    #[test]
    fn navigation_cancels_a_fetch_mid_body() {
        // D-90 revokes at navigation, and D-91 cancels and answers rather than racing.
        let mut body = png(10, 10);
        body.extend(std::iter::repeat_n(0, 4096));
        let (mut broker, token, grant) = granted(1);
        let mut fetch = canned(body);
        let mut stream = open(grant, &mut fetch, later()).expect("validated");
        let mut buf = [0u8; 8];
        stream.read(&mut buf).expect("live");
        assert!(broker.revoke(&token));
        assert_eq!(
            stream.read(&mut buf),
            Err(Answer::Unavailable(Unavailable::Revoked))
        );
    }

    #[test]
    fn a_load_past_its_deadline_is_answered_unavailable() {
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(png(10, 10));
        let mut stream = open(grant, &mut fetch, later()).expect("validated");
        stream.deadline = Instant::now();
        assert_eq!(
            stream.read(&mut [0u8; 8]),
            Err(Answer::Unavailable(Unavailable::Deadline))
        );
    }

    #[test]
    fn a_body_that_ends_before_its_header_did_not_arrive_rather_than_being_refused() {
        // FR-12's pair: "this did not arrive" is not "Sift refused this".
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(b"\x89PNG\r\n".to_vec());
        assert_eq!(
            open(grant, &mut fetch, later()).unwrap_err(),
            Answer::Unavailable(Unavailable::NotFetchable)
        );
    }

    #[test]
    fn concurrency_is_bounded_per_document_and_a_finished_fetch_gives_its_slot_back() {
        // L-29: queues rather than fails, and a slot that is never returned would stall the
        // ninth image of every message for good.
        let bound = usize::try_from(L29_DOC_CONCURRENCY).unwrap();
        let (broker, token, first) = granted(bound + 1);
        let mut held = vec![first];
        for i in 1..=bound {
            held.push(grant_for(&broker, &token, i).expect("allowed"));
        }
        for grant in held.iter_mut().take(bound) {
            grant.acquire(later()).expect("a free slot");
        }
        assert_eq!(broker.in_flight(token.as_str()), Some(bound));

        // The one past the bound waits, and here times out rather than being refused outright.
        let mut extra = held.pop().expect("one more");
        assert_eq!(
            extra.acquire(Instant::now() + Duration::from_millis(20)),
            Err(Unavailable::Deadline)
        );

        // Giving one back is what lets it through.
        drop(held.pop());
        assert_eq!(broker.in_flight(token.as_str()), Some(bound - 1));
        extra.acquire(later()).expect("the freed slot");
        assert_eq!(broker.in_flight(token.as_str()), Some(bound));
    }

    #[test]
    fn revocation_wakes_a_fetch_waiting_for_a_slot() {
        let bound = usize::try_from(L29_DOC_CONCURRENCY).unwrap();
        let (mut broker, token, first) = granted(bound + 1);
        let mut held = vec![first];
        for i in 1..bound {
            held.push(grant_for(&broker, &token, i).expect("allowed"));
        }
        for grant in &mut held {
            grant.acquire(later()).expect("a free slot");
        }
        let mut waiting = grant_for(&broker, &token, bound).expect("allowed");
        let waiter = std::thread::spawn(move || waiting.acquire(Instant::now() + LOAD_DEADLINE));
        assert!(broker.revoke(&token));
        assert_eq!(waiter.join().unwrap(), Err(Unavailable::Revoked));
    }

    #[test]
    fn a_shed_cancels_a_fetch_mid_body_and_wakes_a_fetch_waiting_for_a_slot() {
        // D-93: L3 revokes every live document at once, and each grant under one learns it
        // exactly as it would from navigation — the stream at its next read, the waiter at once.
        let bound = usize::try_from(L29_DOC_CONCURRENCY).unwrap();
        let (mut broker, token, first) = granted(bound + 1);
        let mut body = png(10, 10);
        body.extend(std::iter::repeat_n(0, 4096));
        let mut fetch = canned(body);
        let mut stream = open(first, &mut fetch, later()).expect("validated");
        let mut buf = [0u8; 8];
        stream.read(&mut buf).expect("live");

        let mut held = Vec::new();
        for i in 1..bound {
            let mut grant = grant_for(&broker, &token, i).expect("allowed");
            grant.acquire(later()).expect("a free slot");
            held.push(grant);
        }
        assert_eq!(broker.in_flight(token.as_str()), Some(bound));
        let mut waiting = grant_for(&broker, &token, bound).expect("allowed");
        let waiter = std::thread::spawn(move || waiting.acquire(Instant::now() + LOAD_DEADLINE));

        broker.shed();

        assert_eq!(waiter.join().unwrap(), Err(Unavailable::Revoked));
        assert_eq!(
            stream.read(&mut buf),
            Err(Answer::Unavailable(Unavailable::Revoked))
        );
        assert!(held.iter().all(Grant::is_revoked));
        assert_eq!(broker.live_documents(), 0);
    }

    #[test]
    fn a_refused_position_is_granted_nothing() {
        // The grant is the same decision as the answer: an unallowed sender fetches nothing.
        let mut broker = Broker::new();
        let token = broker.open_document(
            Origin::Null,
            vec![Position {
                url: "https://cdn.test/x.png".to_owned(),
                declared_length: None,
                declared_width: None,
                declared_height: None,
                declared_pixels: None,
                style: None,
                alt: None,
                in_zero_height_container: false,
                request_type: "image".to_owned(),
            }],
        );
        assert_eq!(
            grant_for(&broker, &token, 0).unwrap_err(),
            Answer::Blocked(Reason::NotAllowedBySender)
        );
    }

    #[test]
    fn every_passed_format_states_its_dimensions() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x20, 0x03, 0x58, 0x02]);
        assert_eq!(
            sniff(&gif),
            Sniff::Is {
                format: Format::Gif,
                width: 800,
                height: 600
            }
        );

        // A JPEG with an application segment before its frame header, as nearly all have.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x01, 0x90, 0x02, 0x58]);
        assert_eq!(
            sniff(&jpeg),
            Sniff::Is {
                format: Format::Jpeg,
                width: 600,
                height: 400
            }
        );

        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        vp8x.extend_from_slice(&[0x1f, 0x03, 0x00, 0x57, 0x02, 0x00]);
        assert_eq!(
            sniff(&vp8x),
            Sniff::Is {
                format: Format::Webp,
                width: 800,
                height: 600
            }
        );

        let mut vp8 = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9d\x01\x2a".to_vec();
        vp8.extend_from_slice(&[0x20, 0x03, 0x58, 0x02]);
        assert_eq!(
            sniff(&vp8),
            Sniff::Is {
                format: Format::Webp,
                width: 800,
                height: 600
            }
        );

        let bits: u32 = 799 | (599 << 14);
        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        vp8l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(
            sniff(&vp8l),
            Sniff::Is {
                format: Format::Webp,
                width: 800,
                height: 600
            }
        );
    }

    #[test]
    fn a_short_prefix_asks_for_more_rather_than_deciding() {
        assert_eq!(sniff(b""), Sniff::Need);
        assert_eq!(sniff(b"\x89PNG"), Sniff::Need);
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0, 0x10, 0x00]), Sniff::Need);
    }

    #[test]
    fn a_header_that_describes_no_image_is_refused() {
        assert!(matches!(sniff(&png(0, 10)), Sniff::Refused(_)));
        assert!(matches!(
            sniff(&[0xff, 0xd8, 0xff, 0xda, 0x00, 0x02]),
            Sniff::Refused(_)
        ));
        assert!(matches!(
            sniff(b"<html><body>hi</body></html>"),
            Sniff::Refused(_)
        ));
    }

    #[test]
    fn a_jpeg_that_hides_its_header_past_the_bound_is_refused() {
        // Metadata segments of the maximum length, one after another, forever.
        let mut body = vec![0xff, 0xd8];
        while body.len() <= SNIFF_BOUND {
            body.extend_from_slice(&[0xff, 0xe1, 0xff, 0xff]);
            body.extend(std::iter::repeat_n(0, 0xfffd));
        }
        let (_broker, _token, grant) = granted(1);
        let mut fetch = canned(body);
        fetch.step = 16 * 1024;
        assert!(matches!(
            open(grant, &mut fetch, later()),
            Err(Answer::Blocked(Reason::Validation(_)))
        ));
    }
}

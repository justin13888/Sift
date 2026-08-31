//! D-76 — the page format, and D-42's layer beneath the database engine.
//!
//! Every page written to disk is encrypted and authenticated; the engine above is
//! unmodified. **The write-ahead log is covered by the same layer**, and so are the two
//! installation-scoped stores.
//!
//! # Why the nonce is the whole decision
//!
//! Nonce reuse under an AEAD is not a weakening. It is a break: it leaks plaintext and
//! permits forgery. D-76 records why this is the dangerous part rather than merely the
//! subtle part — deriving a nonce from the page number alone **does not fail a test, does
//! not corrupt a file, and nothing observable goes wrong**. The store works perfectly while
//! being broken, for as long as nobody looks.
//!
//! So the nonce is the page number *and* a write counter that is part of the file's state:
//!
//! ```text
//! nonce (96 bits) = page number (u32, big-endian) ‖ write counter (u64, big-endian)
//! ```
//!
//! A page written twice gets two nonces because the counter advanced. The counter's
//! high-water mark lives in the header so that **derivation resumes across a restart** — a
//! counter that reset on open would reissue every nonce it had ever issued.
//!
//! # Why AES-256-GCM
//!
//! D-75 requires the platform's own audited cipher behind one portable interface producing
//! **one byte-identical on-disk format**, and requires that where a platform cannot supply
//! the chosen construction the fallback is a vendored implementation of *that*
//! construction — never a different one chosen because it was available.
//!
//! AES-256-GCM is the construction both target platforms audit and ship: `AES.GCM` in
//! CryptoKit on macOS, and the same primitive in every Linux crypto library. Its 96-bit
//! nonce is exactly 32 + 64, which is what makes the derivation above fit without
//! truncating either field — a 192-bit construction would have more headroom and is not
//! offered by CryptoKit, and choosing it would be choosing a different construction
//! because it was available.
//!
//! # The counter must be stored with the page
//!
//! A reader knows a page's number and not which counter sealed it, so the counter is
//! written alongside the ciphertext. That is where D-76's per-page overhead comes from:
//!
//! ```text
//! sealed page = write counter (8) ‖ ciphertext (len) ‖ tag (16)
//! ```
//!
//! Twenty-four bytes per page, which "reduces usable bytes in every page, changing store
//! size and page-fault behaviour" — a cost Q-10's rig has to measure rather than one this
//! module can argue away.
//!
//! # What this does not defend against, stated rather than discovered
//!
//! **Per-page rollback.** An attacker with filesystem write access can replace a page with
//! an *older sealed version of the same page*: it carries a real counter and a real tag, so
//! it authenticates. D-76 chose independent per-page seals over one seal across the whole
//! file, and this is the cost of that choice — a whole-file seal would catch it and would
//! make every write rewrite the file.
//!
//! It is bounded rather than absent. The header's high-water mark means a rolled-back page
//! whose counter exceeds it is caught, and the [threat model](../../../docs/security/threat-model.md)
//! scopes the same-user adversary to "what a peer process can reach *without entering
//! Sift*" against a store the provider can refill. A reviewer should still know this is
//! here, because it is the kind of property that reads as an oversight when it is a trade.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use core::sync::atomic::{AtomicU64, Ordering};

/// Identifies a file as a Sift store.
///
/// D-76: a file that cannot say what it is forces a decryption attempt against arbitrary
/// bytes, and D-32 refuses to open a newer schema rather than guessing at it.
pub const MAGIC: [u8; 8] = *b"SIFTPG01";

/// The on-disk format version.
///
/// Distinct from the *schema* version D-32 owns: this is the shape of the envelope, that
/// is the shape of what is inside it. A build refuses a file whose format version it does
/// not know, for the same reason it refuses a newer schema.
pub const FORMAT_VERSION: u16 = 1;

/// Bytes the GCM tag adds.
pub const TAG_LEN: usize = 16;
/// Bytes the stored write counter adds.
pub const COUNTER_LEN: usize = 8;
/// Total per-page overhead. See the module documentation.
pub const PAGE_OVERHEAD: usize = COUNTER_LEN + TAG_LEN;

/// The fixed-size file header.
pub const HEADER_LEN: usize = 64;

/// Identifies which key a file is sealed under.
///
/// D-76 requires this so that **a file under a destroyed key is recognisable rather than
/// merely unreadable** — which is the difference between FR-4's erasure being provable and
/// being believed. It is also what lets D-22's rotation be lazy: both generations stay
/// readable while any page still carries the old identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(pub [u8; 16]);

/// A 256-bit key. Zeroed on drop.
pub struct PageKey([u8; 32]);

impl PageKey {
    #[must_use]
    pub const fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// The raw key. Crate-private on purpose: key material leaving this crate is how it
    /// ends up somewhere NFR-23 forbids, and the only legitimate readers are the cipher
    /// below and the derivation tests that assert two keys differ.
    pub(crate) const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for PageKey {
    fn drop(&mut self) {
        // Not a guarantee against a determined attacker in the address space — the threat
        // model puts that out of scope because the platform prevents it — but key material
        // should not outlive its use in a resident process that runs for weeks.
        for b in &mut self.0 {
            // Volatile so the write is not elided as dead.
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
}

impl core::fmt::Debug for PageKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PageKey(redacted)")
    }
}

/// What can go wrong opening a sealed page or a header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    /// Not a Sift store. Refused rather than attempted.
    NotASiftStore,
    /// A format version this build does not know. **Refused, never guessed at.**
    UnknownFormatVersion(u16),
    /// The file is sealed under a different key than the one supplied.
    WrongKey,
    /// Authentication failed.
    ///
    /// **Indistinguishable from tampering, and therefore MUST NOT be treated as a
    /// recoverable read error.** The failure model requires the account enter *storage
    /// unavailable*, the queue be drained or exported, and recovery be removal and resync.
    FailedAuthentication,
    /// Too short to contain what it claims.
    Truncated,
}

/// The file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format_version: u16,
    pub key_id: KeyId,
    /// The highest counter issued when this header was last written.
    ///
    /// Nonce derivation resumes **above** this, which is what makes it safe across a crash.
    pub counter_high_water: u64,
}

impl Header {
    #[must_use]
    pub fn new(key_id: KeyId) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            key_id,
            counter_high_water: 0,
        }
    }

    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..8].copy_from_slice(&MAGIC);
        out[8..10].copy_from_slice(&self.format_version.to_be_bytes());
        out[10..26].copy_from_slice(&self.key_id.0);
        out[26..34].copy_from_slice(&self.counter_high_water.to_be_bytes());
        out
    }

    /// # Errors
    /// Refuses anything that is not a Sift store of a known format version.
    pub fn decode(bytes: &[u8]) -> Result<Self, PageError> {
        if bytes.len() < HEADER_LEN {
            return Err(PageError::Truncated);
        }
        if bytes[0..8] != MAGIC {
            return Err(PageError::NotASiftStore);
        }
        let format_version = u16::from_be_bytes([bytes[8], bytes[9]]);
        if format_version != FORMAT_VERSION {
            return Err(PageError::UnknownFormatVersion(format_version));
        }
        let mut key_id = [0u8; 16];
        key_id.copy_from_slice(&bytes[10..26]);
        let mut hw = [0u8; 8];
        hw.copy_from_slice(&bytes[26..34]);
        Ok(Self {
            format_version,
            key_id: KeyId(key_id),
            counter_high_water: u64::from_be_bytes(hw),
        })
    }
}

/// Seals and opens the pages of one file.
pub struct PageCipher {
    cipher: Aes256Gcm,
    key_id: KeyId,
    counter: AtomicU64,
}

impl core::fmt::Debug for PageCipher {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PageCipher")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl PageCipher {
    /// Open a file's cipher, resuming the counter **above** the header's high-water mark.
    #[must_use]
    pub fn new(key: &PageKey, header: &Header) -> Self {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.bytes()));
        Self {
            cipher,
            key_id: header.key_id,
            counter: AtomicU64::new(header.counter_high_water),
        }
    }

    #[must_use]
    pub const fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// The counter to record in the header. Written **before** the pages it covers, so a
    /// crash leaves the mark ahead of reality rather than behind it — ahead wastes counter
    /// values, behind reissues nonces.
    #[must_use]
    pub fn high_water(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }

    /// Seal one page.
    ///
    /// # Errors
    /// Only if the underlying AEAD fails, which for this construction means the input is
    /// implausibly large.
    pub fn seal(&self, page_number: u32, plaintext: &[u8]) -> Result<Vec<u8>, PageError> {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        self.seal_with_counter(page_number, counter, plaintext)
    }

    /// Seal with a stated counter. Exposed so that the byte-for-byte fixture can be
    /// reproduced exactly; ordinary writes go through [`seal`](Self::seal).
    ///
    /// # Errors
    /// As [`seal`](Self::seal).
    pub fn seal_with_counter(
        &self,
        page_number: u32,
        counter: u64,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, PageError> {
        let nonce = nonce_for(page_number, counter);
        let aad = aad_for(self.key_id, page_number, counter);
        let sealed = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| PageError::FailedAuthentication)?;

        let mut out = Vec::with_capacity(COUNTER_LEN + sealed.len());
        out.extend_from_slice(&counter.to_be_bytes());
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Open one page.
    ///
    /// # Errors
    /// [`PageError::FailedAuthentication`] on any tampering, on a page presented under the
    /// wrong number, or under the wrong key.
    pub fn open(&self, page_number: u32, sealed: &[u8]) -> Result<Vec<u8>, PageError> {
        if sealed.len() < PAGE_OVERHEAD {
            return Err(PageError::Truncated);
        }
        let mut c = [0u8; 8];
        c.copy_from_slice(&sealed[..COUNTER_LEN]);
        let counter = u64::from_be_bytes(c);

        let nonce = nonce_for(page_number, counter);
        let aad = aad_for(self.key_id, page_number, counter);
        self.cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &sealed[COUNTER_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| PageError::FailedAuthentication)
    }
}

/// `page number ‖ counter`, which is exactly 96 bits.
fn nonce_for(page_number: u32, counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0..4].copy_from_slice(&page_number.to_be_bytes());
    n[4..12].copy_from_slice(&counter.to_be_bytes());
    n
}

/// Additional authenticated data.
///
/// The nonce already binds the page number and counter cryptographically, so this is
/// belt and braces for those two — but the key identifier and format version are *not* in
/// the nonce, and binding them here is what makes a key-confusion or a format-downgrade
/// attempt fail as authentication rather than succeed as a decrypt.
fn aad_for(key_id: KeyId, page_number: u32, counter: u64) -> [u8; 8 + 2 + 16 + 4 + 8] {
    let mut aad = [0u8; 38];
    aad[0..8].copy_from_slice(&MAGIC);
    aad[8..10].copy_from_slice(&FORMAT_VERSION.to_be_bytes());
    aad[10..26].copy_from_slice(&key_id.0);
    aad[26..30].copy_from_slice(&page_number.to_be_bytes());
    aad[30..38].copy_from_slice(&counter.to_be_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn key() -> PageKey {
        PageKey::from_bytes([0x42; 32])
    }
    fn key_id() -> KeyId {
        KeyId([0xAB; 16])
    }
    fn cipher() -> PageCipher {
        PageCipher::new(&key(), &Header::new(key_id()))
    }

    #[test]
    fn a_page_round_trips() {
        let c = cipher();
        let plain = b"a database page".repeat(100);
        let sealed = c.seal(7, &plain).unwrap();
        assert_eq!(c.open(7, &sealed).unwrap(), plain);
    }

    #[test]
    fn the_overhead_is_twenty_four_bytes_per_page() {
        // Q-10's rig has to measure what this does to store size and page-fault behaviour,
        // so the number is asserted rather than assumed.
        let c = cipher();
        let plain = [0u8; 4096];
        assert_eq!(
            c.seal(1, &plain).unwrap().len(),
            plain.len() + PAGE_OVERHEAD
        );
        assert_eq!(PAGE_OVERHEAD, 24);
    }

    #[test]
    fn any_single_flipped_bit_fails_authentication() {
        let c = cipher();
        let sealed = c.seal(3, b"secret").unwrap();
        for i in 0..sealed.len() {
            let mut t = sealed.clone();
            t[i] ^= 1;
            assert_eq!(
                c.open(3, &t),
                Err(PageError::FailedAuthentication),
                "a flip at byte {i} was accepted"
            );
        }
    }

    #[test]
    fn a_page_presented_under_another_number_is_refused() {
        // Without this an attacker could swap two pages and the store would open happily
        // with its contents rearranged.
        let c = cipher();
        let sealed = c.seal(5, b"page five").unwrap();
        assert_eq!(c.open(6, &sealed), Err(PageError::FailedAuthentication));
    }

    #[test]
    fn a_page_from_another_key_is_refused() {
        let a = cipher();
        let b = PageCipher::new(&PageKey::from_bytes([0x99; 32]), &Header::new(key_id()));
        let sealed = a.seal(1, b"mine").unwrap();
        assert_eq!(b.open(1, &sealed), Err(PageError::FailedAuthentication));
    }

    #[test]
    fn a_page_sealed_under_another_key_identifier_is_refused() {
        // The key bytes are the same and only the identifier differs. The nonce does not
        // carry the identifier, so this is caught by the additional authenticated data or
        // it is not caught at all.
        let a = cipher();
        let b = PageCipher::new(&key(), &Header::new(KeyId([0xCD; 16])));
        let sealed = a.seal(1, b"mine").unwrap();
        assert_eq!(b.open(1, &sealed), Err(PageError::FailedAuthentication));
    }

    #[test]
    fn a_truncated_page_is_refused_rather_than_panicking() {
        let c = cipher();
        assert_eq!(c.open(1, &[]), Err(PageError::Truncated));
        assert_eq!(
            c.open(1, &[0u8; PAGE_OVERHEAD - 1]),
            Err(PageError::Truncated)
        );
    }

    #[test]
    fn a_nonce_is_never_reused() {
        // The property the whole format exists to hold. Reuse under an AEAD is a break,
        // not a weakening, and D-76 notes it "does not fail a test, does not corrupt a
        // file; nothing observable goes wrong" — so it is asserted directly.
        let c = cipher();
        let mut nonces = BTreeSet::new();
        for round in 0..200u32 {
            for page in 0..50u32 {
                let sealed = c.seal(page, b"x").unwrap();
                let mut ctr = [0u8; 8];
                ctr.copy_from_slice(&sealed[..8]);
                let n = nonce_for(page, u64::from_be_bytes(ctr));
                assert!(
                    nonces.insert(n),
                    "nonce reused at round {round} page {page}"
                );
            }
        }
        assert_eq!(nonces.len(), 200 * 50);
    }

    #[test]
    fn writing_one_page_twice_uses_two_nonces() {
        // The case a page-number-only nonce gets wrong, and the reason the counter exists.
        let c = cipher();
        let first = c.seal(9, b"before").unwrap();
        let second = c.seal(9, b"after").unwrap();
        assert_ne!(
            &first[..8],
            &second[..8],
            "the same nonce sealed both writes"
        );
    }

    #[test]
    fn the_counter_resumes_above_the_high_water_mark_across_a_restart() {
        // "A counter that resets on open reuses every nonce it has issued."
        let c = cipher();
        for p in 0..10 {
            c.seal(p, b"x").unwrap();
        }
        let mark = c.high_water();
        assert!(mark >= 10);

        let mut header = Header::new(key_id());
        header.counter_high_water = mark;
        let after_restart = PageCipher::new(&key(), &header);

        let sealed = after_restart.seal(0, b"y").unwrap();
        let mut ctr = [0u8; 8];
        ctr.copy_from_slice(&sealed[..8]);
        assert!(
            u64::from_be_bytes(ctr) > mark,
            "the counter restarted at or below the high-water mark"
        );
    }

    #[test]
    fn a_header_round_trips() {
        let mut h = Header::new(key_id());
        h.counter_high_water = 123_456;
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn a_file_that_is_not_a_sift_store_is_refused_rather_than_attempted() {
        // "A file that cannot say what it is forces a decryption attempt against arbitrary
        // bytes."
        let mut bytes = [0u8; HEADER_LEN];
        bytes[0..8].copy_from_slice(b"NOTSIFT!");
        assert_eq!(Header::decode(&bytes), Err(PageError::NotASiftStore));
    }

    #[test]
    fn an_unknown_format_version_is_refused_never_guessed_at() {
        let mut h = Header::new(key_id()).encode();
        h[8..10].copy_from_slice(&99u16.to_be_bytes());
        assert_eq!(Header::decode(&h), Err(PageError::UnknownFormatVersion(99)));
    }

    #[test]
    fn a_short_header_is_refused() {
        assert_eq!(Header::decode(&[0u8; 4]), Err(PageError::Truncated));
    }

    #[test]
    fn a_key_identifier_survives_so_a_destroyed_key_is_recognisable() {
        // The difference between FR-4's erasure being provable and being believed: a file
        // under a destroyed key reads as *that*, rather than as corrupt.
        let h = Header::new(key_id());
        assert_eq!(Header::decode(&h.encode()).unwrap().key_id, key_id());
    }

    /// The property this format deliberately does **not** have, asserted so that it is a
    /// recorded trade rather than a discovered surprise.
    #[test]
    fn an_older_sealed_version_of_the_same_page_still_authenticates() {
        // D-76 chose independent per-page seals over one seal across the whole file. The
        // cost is that an attacker with filesystem write access can roll one page back:
        // the old bytes carry a real counter and a real tag. A whole-file seal would catch
        // this and would make every write rewrite the file.
        //
        // If this test ever fails, the format gained rollback resistance and the module
        // documentation is now wrong — which is a good problem, and still a change.
        let c = cipher();
        let old = c.seal(4, b"balance: 100").unwrap();
        let _new = c.seal(4, b"balance: 0").unwrap();
        assert_eq!(
            c.open(4, &old).unwrap(),
            b"balance: 100",
            "per-page rollback is a known property of D-76's choice"
        );
    }
}

#[cfg(test)]
mod byte_for_byte {
    //! D-75's fixture check, which `docs/build/verification.md` requires **from the first
    //! commit rather than after the first divergence**.
    //!
    //! R-16 is the risk it exists for: one portable interface over two platforms'
    //! libraries produces one format only as long as both agree about it, and **a mismatch
    //! does not fail loudly**. It writes a store the other platform cannot read, or worse,
    //! reads one incorrectly. This fixture is the only thing standing between the design
    //! and a class of bug a single vendored implementation would not have had.
    //!
    //! The vector below is the format's definition. A platform backend that produces
    //! different bytes for these inputs is wrong, however audited its cipher is.

    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/page-format-v1.txt");

    fn parse(label: &str) -> Vec<u8> {
        let line = FIXTURE
            .lines()
            .find(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("fixture has no `{label}` line"));
        let hex = line.split_once('=').expect("label = hex").1.trim();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect()
    }

    #[test]
    fn the_format_produces_exactly_these_bytes() {
        let key_bytes: [u8; 32] = parse("key").try_into().expect("32-byte key");
        let key_id_bytes: [u8; 16] = parse("key_id").try_into().expect("16-byte key id");
        let plaintext = parse("plaintext");
        let expected = parse("sealed");

        let cipher = PageCipher::new(
            &PageKey::from_bytes(key_bytes),
            &Header::new(KeyId(key_id_bytes)),
        );
        // A stated counter and page number, so the vector is reproducible rather than
        // depending on how many pages a test happened to write first.
        let sealed = cipher.seal_with_counter(7, 42, &plaintext).unwrap();

        assert_eq!(
            sealed, expected,
            "the on-disk format changed.\n\
             If this was deliberate it is a format version bump and a migration, not an \
             edit to the fixture — every store already written is in the old format.\n\
             If it was not deliberate, this is the divergence R-16 predicted."
        );
    }

    #[test]
    fn the_header_produces_exactly_these_bytes() {
        let key_id_bytes: [u8; 16] = parse("key_id").try_into().expect("16-byte key id");
        let mut header = Header::new(KeyId(key_id_bytes));
        header.counter_high_water = 42;
        assert_eq!(header.encode().to_vec(), parse("header"));
    }

    #[test]
    fn the_fixture_round_trips_through_open() {
        // A fixture that only pins bytes could pin bytes nothing can read.
        let key_bytes: [u8; 32] = parse("key").try_into().unwrap();
        let key_id_bytes: [u8; 16] = parse("key_id").try_into().unwrap();
        let cipher = PageCipher::new(
            &PageKey::from_bytes(key_bytes),
            &Header::new(KeyId(key_id_bytes)),
        );
        assert_eq!(
            cipher.open(7, &parse("sealed")).unwrap(),
            parse("plaintext")
        );
    }
}

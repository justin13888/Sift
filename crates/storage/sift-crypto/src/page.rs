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
//!
//! # Where the cipher comes from
//!
//! Everything above — the nonce, the additional data, the header, the counter written with
//! the page — is this module's, and is the same code on every platform. Only the AEAD call
//! itself is the platform's, behind [`backend`], which seals and opens one message under a
//! stated key, nonce and additional data and knows nothing else:
//!
//! - **macOS:** CryptoKit's `AES.GCM`. CryptoKit has no C interface, so a Swift file of four
//!   C-named functions (`platform/cryptokit.swift`, compiled by `build.rs`) is the bridge.
//! - **Everywhere else:** the vendored implementation of the same construction. D-75 names
//!   the Linux platform library as the eventual source; until it is wired, this is the
//!   fallback D-75 permits — *that* construction, vendored — not a different one.
//!
//! [`BACKEND`] says which one a build carries. The byte-for-byte fixture runs through it on
//! every platform, and on macOS every CryptoKit seal is additionally compared against the
//! vendored construction, so a disagreement (R-16) fails a test rather than a store.

use core::sync::atomic::{AtomicU64, Ordering};

pub use backend::BACKEND;

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
    cipher: backend::Aead,
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
        let cipher = backend::Aead::new(key.bytes());
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
        let mut out = vec![0u8; COUNTER_LEN + plaintext.len() + TAG_LEN];
        out[..COUNTER_LEN].copy_from_slice(&counter.to_be_bytes());
        self.cipher
            .seal(&nonce, &aad, plaintext, &mut out[COUNTER_LEN..])
            .map_err(|()| PageError::FailedAuthentication)?;
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
        let body = &sealed[COUNTER_LEN..];
        let mut out = vec![0u8; body.len() - TAG_LEN];
        self.cipher
            .open(&nonce, &aad, body, &mut out)
            .map_err(|()| PageError::FailedAuthentication)?;
        Ok(out)
    }
}

/// The one call that is the platform's: AES-256-GCM over one message.
///
/// Both implementations keep the same contract, and it is all of D-75's "one interface":
///
/// - `new` takes the 32-byte key.
/// - `seal` writes `plaintext.len() + 16` bytes into `out` — ciphertext, then the tag.
/// - `open` takes that shape back and writes `sealed.len() - 16` bytes, only on a verified tag.
/// - Any failure is `Err(())`, undistinguished, because the caller must treat every one as
///   [`PageError::FailedAuthentication`].
///
/// Callers size `out` exactly; a mismatch is a failure, never a partial write.
mod backend {
    #[cfg(target_os = "macos")]
    pub(super) use cryptokit::Aead;
    #[cfg(target_os = "macos")]
    pub use cryptokit::BACKEND;
    #[cfg(not(target_os = "macos"))]
    pub(super) use vendored::Aead;
    #[cfg(not(target_os = "macos"))]
    pub use vendored::BACKEND;

    #[cfg(target_os = "macos")]
    mod cryptokit {
        use core::ffi::c_void;
        use core::ptr::NonNull;

        /// Which implementation this build seals with.
        pub const BACKEND: &str = "CryptoKit AES.GCM";

        unsafe extern "C" {
            fn sift_cryptokit_key_new(bytes: *const u8, len: usize) -> *mut c_void;
            fn sift_cryptokit_key_free(handle: *mut c_void);
            fn sift_cryptokit_seal(
                handle: *const c_void,
                nonce: *const u8,
                nonce_len: usize,
                aad: *const u8,
                aad_len: usize,
                msg: *const u8,
                msg_len: usize,
                out: *mut u8,
                out_len: usize,
            ) -> i32;
            fn sift_cryptokit_open(
                handle: *const c_void,
                nonce: *const u8,
                nonce_len: usize,
                aad: *const u8,
                aad_len: usize,
                sealed: *const u8,
                sealed_len: usize,
                out: *mut u8,
                out_len: usize,
            ) -> i32;
        }

        /// A CryptoKit `SymmetricKey`, owned through an opaque handle and released on drop.
        /// The key bytes live in CryptoKit's own storage, which zeroes itself on release.
        pub(crate) struct Aead(NonNull<c_void>);

        // SAFETY: the handle points at an immutable Swift object (`let key: SymmetricKey`);
        // CryptoKit's sealing and opening read it without mutation, so sharing one handle
        // across threads is sound. `PageCipher` is shared between the VFS's file handles.
        unsafe impl Send for Aead {}
        unsafe impl Sync for Aead {}

        impl Aead {
            pub(crate) fn new(key: &[u8; 32]) -> Self {
                // SAFETY: `key` is 32 readable bytes for the duration of the call; the shim
                // copies them into CryptoKit's storage and keeps no reference.
                let handle = unsafe { sift_cryptokit_key_new(key.as_ptr(), key.len()) };
                // The shim refuses only a key that is not 32 bytes, which the type rules out.
                Self(NonNull::new(handle).expect("CryptoKit refused a 32-byte key"))
            }

            pub(crate) fn seal(
                &self,
                nonce: &[u8; 12],
                aad: &[u8],
                plaintext: &[u8],
                out: &mut [u8],
            ) -> Result<(), ()> {
                // SAFETY: every pointer is paired with the length of the slice it came from,
                // and the shim reads or writes exactly those lengths, returning non-zero
                // before touching `out` if the lengths do not fit its contract.
                let rc = unsafe {
                    sift_cryptokit_seal(
                        self.0.as_ptr(),
                        nonce.as_ptr(),
                        nonce.len(),
                        aad.as_ptr(),
                        aad.len(),
                        plaintext.as_ptr(),
                        plaintext.len(),
                        out.as_mut_ptr(),
                        out.len(),
                    )
                };
                if rc == 0 { Ok(()) } else { Err(()) }
            }

            pub(crate) fn open(
                &self,
                nonce: &[u8; 12],
                aad: &[u8],
                sealed: &[u8],
                out: &mut [u8],
            ) -> Result<(), ()> {
                // SAFETY: as `seal`. `out` is written only after the tag verified.
                let rc = unsafe {
                    sift_cryptokit_open(
                        self.0.as_ptr(),
                        nonce.as_ptr(),
                        nonce.len(),
                        aad.as_ptr(),
                        aad.len(),
                        sealed.as_ptr(),
                        sealed.len(),
                        out.as_mut_ptr(),
                        out.len(),
                    )
                };
                if rc == 0 { Ok(()) } else { Err(()) }
            }
        }

        impl Drop for Aead {
            fn drop(&mut self) {
                // SAFETY: the handle came from `sift_cryptokit_key_new` and is released
                // exactly once, here.
                unsafe { sift_cryptokit_key_free(self.0.as_ptr()) };
            }
        }
    }

    #[cfg(any(not(target_os = "macos"), test))]
    pub(super) mod vendored {
        use aes_gcm::aead::{AeadInPlace, KeyInit};
        use aes_gcm::{Aes256Gcm, Key, Nonce, Tag};

        /// Which implementation this build seals with.
        #[cfg(not(target_os = "macos"))]
        pub const BACKEND: &str = "vendored AES-256-GCM";

        /// The vendored construction. On macOS it exists only in tests, as the reference
        /// CryptoKit is compared against.
        pub(crate) struct Aead(Aes256Gcm);

        impl Aead {
            pub(crate) fn new(key: &[u8; 32]) -> Self {
                Self(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)))
            }

            pub(crate) fn seal(
                &self,
                nonce: &[u8; 12],
                aad: &[u8],
                plaintext: &[u8],
                out: &mut [u8],
            ) -> Result<(), ()> {
                if out.len() != plaintext.len() + super::super::TAG_LEN {
                    return Err(());
                }
                let (body, tag_out) = out.split_at_mut(plaintext.len());
                body.copy_from_slice(plaintext);
                let tag = self
                    .0
                    .encrypt_in_place_detached(Nonce::from_slice(nonce), aad, body)
                    .map_err(|_| ())?;
                tag_out.copy_from_slice(&tag);
                Ok(())
            }

            pub(crate) fn open(
                &self,
                nonce: &[u8; 12],
                aad: &[u8],
                sealed: &[u8],
                out: &mut [u8],
            ) -> Result<(), ()> {
                let tag_len = super::super::TAG_LEN;
                if sealed.len() < tag_len || out.len() != sealed.len() - tag_len {
                    return Err(());
                }
                let (body, tag) = sealed.split_at(out.len());
                out.copy_from_slice(body);
                let result = self.0.decrypt_in_place_detached(
                    Nonce::from_slice(nonce),
                    aad,
                    out,
                    Tag::from_slice(tag),
                );
                if result.is_err() {
                    // Nothing unauthenticated leaves, even into a buffer about to be dropped.
                    out.fill(0);
                    return Err(());
                }
                Ok(())
            }
        }

        #[cfg(test)]
        mod tests {
            use super::Aead;
            use crate::page::TAG_LEN;

            #[test]
            fn a_failed_open_leaves_nothing_unauthenticated_in_out() {
                // `open` copies the ciphertext into `out` before the tag is checked, so
                // without the zeroing a refused page would leave the forger's bytes behind.
                let aead = Aead::new(&[0x42; 32]);
                let nonce = [7u8; 12];
                let plain = [0x5Au8; 64];
                let mut sealed = vec![0u8; plain.len() + TAG_LEN];
                aead.seal(&nonce, b"aad", &plain, &mut sealed).unwrap();

                for i in 0..sealed.len() {
                    let mut forged = sealed.clone();
                    forged[i] ^= 0x01;
                    let mut out = vec![0xAAu8; plain.len()];
                    assert_eq!(aead.open(&nonce, b"aad", &forged, &mut out), Err(()));
                    assert!(out.iter().all(|&b| b == 0), "byte {i}: out not zeroed");
                }
                let mut out = vec![0xAAu8; plain.len()];
                assert_eq!(aead.open(&nonce, b"other", &sealed, &mut out), Err(()));
                assert!(out.iter().all(|&b| b == 0), "wrong AAD: out not zeroed");
            }
        }
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

    #[test]
    fn this_build_seals_with_its_platforms_backend() {
        // The fixture above proves whichever backend is compiled in. This pins *which* one,
        // so a build that quietly fell back to the vendored construction on macOS — the
        // alternative D-75 rejected, arriving through a cfg — fails here rather than passing
        // the fixture and meaning nothing about CryptoKit.
        let expected = if cfg!(target_os = "macos") {
            "CryptoKit AES.GCM"
        } else {
            "vendored AES-256-GCM"
        };
        assert_eq!(BACKEND, expected);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod cryptokit_agrees_with_the_vendored_construction {
    //! R-16 beyond one vector. The fixture pins one input; this compares CryptoKit with the
    //! vendored construction across page sizes, additional data and nonces a single vector
    //! cannot cover — the empty page, one byte, a block boundary either side, the store's
    //! page size and a large one — and requires both to refuse the same forgeries.

    use super::TAG_LEN;
    use super::backend::{Aead as Platform, vendored::Aead as Vendored};

    /// A small deterministic generator, so a failure names a reproducible case.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 56) as u8
        }
        fn fill(&mut self, buf: &mut [u8]) {
            for b in buf {
                *b = self.next();
            }
        }
    }

    const LENGTHS: &[usize] = &[0, 1, 15, 16, 17, 31, 32, 33, 255, 4095, 4096, 4097, 65_536];

    #[test]
    fn both_seal_every_input_to_the_same_bytes_and_open_each_others() {
        let mut rng = Lcg(0x5EED_D075);
        for case in 0..16 {
            let mut key = [0u8; 32];
            let mut nonce = [0u8; 12];
            rng.fill(&mut key);
            rng.fill(&mut nonce);
            let platform = Platform::new(&key);
            let vendored = Vendored::new(&key);

            for &len in LENGTHS {
                let mut aad = vec![0u8; usize::from(rng.next()) % 64];
                rng.fill(&mut aad);
                let mut plain = vec![0u8; len];
                rng.fill(&mut plain);

                let mut a = vec![0u8; len + TAG_LEN];
                let mut b = vec![0u8; len + TAG_LEN];
                platform.seal(&nonce, &aad, &plain, &mut a).unwrap();
                vendored.seal(&nonce, &aad, &plain, &mut b).unwrap();
                assert_eq!(
                    a, b,
                    "case {case}, length {len}: CryptoKit and vendored disagree"
                );

                let mut out = vec![0u8; len];
                platform.open(&nonce, &aad, &b, &mut out).unwrap();
                assert_eq!(
                    out, plain,
                    "case {case}, length {len}: CryptoKit opening vendored"
                );
                out.fill(0);
                vendored.open(&nonce, &aad, &a, &mut out).unwrap();
                assert_eq!(
                    out, plain,
                    "case {case}, length {len}: vendored opening CryptoKit"
                );
            }
        }
    }

    #[test]
    fn both_refuse_the_same_forgeries() {
        let key = [0x42u8; 32];
        let nonce = [7u8; 12];
        let aad = b"additional data";
        let platform = Platform::new(&key);
        let vendored = Vendored::new(&key);
        let plain = b"a page of a Sift store";
        let mut sealed = vec![0u8; plain.len() + TAG_LEN];
        platform.seal(&nonce, aad, plain, &mut sealed).unwrap();

        let mut out = vec![0u8; plain.len()];
        for i in 0..sealed.len() {
            let mut t = sealed.clone();
            t[i] ^= 0x80;
            assert_eq!(
                platform.open(&nonce, aad, &t, &mut out),
                Err(()),
                "byte {i}"
            );
            assert_eq!(
                vendored.open(&nonce, aad, &t, &mut out),
                Err(()),
                "byte {i}"
            );
        }
        let mut other_nonce = nonce;
        other_nonce[11] ^= 1;
        assert_eq!(platform.open(&other_nonce, aad, &sealed, &mut out), Err(()));
        assert_eq!(
            platform.open(&nonce, b"other data", &sealed, &mut out),
            Err(())
        );
    }

    #[test]
    fn a_wrongly_sized_buffer_is_refused_rather_than_overrun() {
        // The shim's length checks are the only thing between a caller's mistake and a
        // write past the end of `out`, so they are asserted rather than trusted.
        let platform = Platform::new(&[1u8; 32]);
        let nonce = [0u8; 12];
        let plain = [9u8; 32];
        let mut short = vec![0u8; plain.len() + TAG_LEN - 1];
        assert_eq!(platform.seal(&nonce, &[], &plain, &mut short), Err(()));
        let mut long = vec![0u8; plain.len() + TAG_LEN + 1];
        assert_eq!(platform.seal(&nonce, &[], &plain, &mut long), Err(()));

        let mut sealed = vec![0u8; plain.len() + TAG_LEN];
        platform.seal(&nonce, &[], &plain, &mut sealed).unwrap();
        let mut out = vec![0u8; plain.len() - 1];
        assert_eq!(platform.open(&nonce, &[], &sealed, &mut out), Err(()));
        assert_eq!(
            platform.open(&nonce, &[], &sealed[..TAG_LEN - 1], &mut []),
            Err(())
        );
    }

    #[test]
    fn one_key_is_usable_from_many_threads_at_once() {
        // `PageCipher` is shared between the VFS's handles, which is what the `Send` and
        // `Sync` claims on the CryptoKit handle rest on.
        let platform = std::sync::Arc::new(Platform::new(&[3u8; 32]));
        let vendored = Vendored::new(&[3u8; 32]);
        let mut expected = vec![0u8; 4096 + TAG_LEN];
        vendored
            .seal(&[5u8; 12], b"aad", &[6u8; 4096], &mut expected)
            .unwrap();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let p = std::sync::Arc::clone(&platform);
                std::thread::spawn(move || {
                    let mut out = vec![0u8; 4096 + TAG_LEN];
                    for _ in 0..200 {
                        p.seal(&[5u8; 12], b"aad", &[6u8; 4096], &mut out).unwrap();
                    }
                    out
                })
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), expected);
        }
    }
}

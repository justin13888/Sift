//! D-23, D-43 and D-22 — content addressing, and the keys derived alongside it.
//!
//! # The two disclosures that must not be conflated
//!
//! Deduplicating across accounts means an observer can tell that two accounts hold the
//! same attachment. That is **inherent to dedup** and D-22 accepts it.
//!
//! The second is not inherent and is not accepted: with a bare hash of plaintext, an
//! observer holding a *candidate file* could hash it, derive its address, and check whether
//! the store contains it. That is **confirmation of a known file** — it turns the blob
//! store into an oracle answering "does this person have this document?" for any document
//! the asker already has.
//!
//! D-43 closes it by deriving both the address and the convergent key with **BLAKE3 in
//! keyed mode under a per-installation secret** held in the OS credential store. Without
//! the secret the address of a known file cannot be computed, so there is nothing to ask.
//!
//! **Deduplication is unaffected**, because convergence was only ever needed *within* one
//! installation. Cross-machine convergence was never a property this design had or wanted.
//!
//! # What it costs, recorded rather than discovered
//!
//! One secret whose compromise re-enables the confirmation attack across every account at
//! once, and which also unseals the installation-wide half of the allowlist. And an
//! orphaned store — one whose secret is gone — **MUST be discarded wholesale rather than
//! retained**: the shared blob index is encrypted under the same lost secret, so the
//! installation can neither enumerate it, collect from it, nor evict from it. It becomes
//! unbounded disk that NFR-14 can no longer reach.
//!
//! # Why the address is over plaintext
//!
//! D-22: "the content address is computed over plaintext, not ciphertext." Addressing
//! ciphertext would make dedup impossible, because two accounts' encryptions of the same
//! bytes differ.

use crate::page::PageKey;

/// Domain-separation contexts.
///
/// Each derivation from the installation secret gets its own, so that the address of a
/// blob and the key that encrypts it are **independent** — otherwise the address, which is
/// a filename on disk, would be a function of the key material, or worse, reveal it.
///
/// The strings carry the application identifier and a version. They are inputs to the key
/// schedule and **changing one re-derives everything under it**, which for the address
/// context means a discard-and-refill of the entire blob store.
mod context {
    pub(crate) const ADDRESS: &str = "net.justinchung.sift 2026-08 blob content address v1";
    pub(crate) const BLOB_KEY: &str = "net.justinchung.sift 2026-08 convergent blob key v1";
    pub(crate) const SHARED_INDEX: &str = "net.justinchung.sift 2026-08 shared blob index v1";
    pub(crate) const INSTALLATION_POLICY: &str =
        "net.justinchung.sift 2026-08 installation policy store v1";
}

/// The per-installation secret, held in the OS credential store and nowhere else.
///
/// "Installation" means **one user account on one machine** — a definition rather than a
/// decision, and re-scoping it would re-derive every content address in the store.
///
/// The credential item is *not* synchronizable, which would redefine an installation as
/// per-account-holder, and *not* device-only, which would turn every hardware upgrade into
/// a total cache loss.
pub struct InstallationSecret([u8; 32]);

impl InstallationSecret {
    #[must_use]
    pub const fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    fn subkey(&self, context: &str) -> [u8; 32] {
        blake3::derive_key(context, &self.0)
    }

    /// The content address of some plaintext: the blob's name on disk, and its key in the
    /// shared blob index.
    #[must_use]
    pub fn address(&self, plaintext: &[u8]) -> ContentAddress {
        let key = self.subkey(context::ADDRESS);
        ContentAddress(*blake3::keyed_hash(&key, plaintext).as_bytes())
    }

    /// D-22's convergent blob key: derived from the blob's own content, so two accounts
    /// holding the same bytes derive the same key and the blob is stored once.
    ///
    /// The key is then **wrapped per account**, which is what keeps an account's claim on a
    /// blob removable without touching anybody else's.
    #[must_use]
    pub fn blob_key(&self, plaintext: &[u8]) -> PageKey {
        let key = self.subkey(context::BLOB_KEY);
        PageKey::from_bytes(*blake3::keyed_hash(&key, plaintext).as_bytes())
    }

    /// The key the shared blob index is sealed under.
    #[must_use]
    pub fn shared_index_key(&self) -> PageKey {
        PageKey::from_bytes(self.subkey(context::SHARED_INDEX))
    }

    /// The key the installation policy store is sealed under.
    ///
    /// **Authentication is the point here rather than confidentiality.** An attacker who
    /// can rewrite an allowlist entry converts filesystem write access into a standing
    /// instruction to fetch remote content — which is egress the user never allowed, from
    /// a store that looks like their own decision.
    #[must_use]
    pub fn installation_policy_key(&self) -> PageKey {
        PageKey::from_bytes(self.subkey(context::INSTALLATION_POLICY))
    }
}

impl core::fmt::Debug for InstallationSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("InstallationSecret(redacted)")
    }
}

impl Drop for InstallationSecret {
    fn drop(&mut self) {
        for b in &mut self.0 {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
}

/// A blob's content address.
///
/// Under D-77 this **is the filename**, placed in a directory fan-out derived from it. That
/// is safe only because the address is a keyed hash: an unkeyed one would be a stable
/// global identifier, discoverable by anybody with the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentAddress([u8; 32]);

impl ContentAddress {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The path a blob lives at, relative to the blob store root.
    ///
    /// D-77 places the file in a fan-out derived from its address, and **the depth is fixed
    /// at first release** because the on-disk layout is published in the Cask uninstall
    /// stanza — changing it later breaks uninstall for users who already have one.
    ///
    /// Two levels of one byte each: 256 directories holding 256 directories, which keeps a
    /// directory's entry count reasonable for a store bounded by NFR-14's default of 2 GB
    /// without making the tree deep enough to cost an extra lookup on every read.
    #[must_use]
    pub fn path(&self) -> String {
        let h = self.hex();
        format!("{}/{}/{h}", &h[0..2], &h[2..4])
    }

    #[must_use]
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> InstallationSecret {
        InstallationSecret::from_bytes([7u8; 32])
    }

    #[test]
    fn the_same_content_addresses_the_same_way() {
        // Convergence is what makes dedup work at all.
        let s = secret();
        assert_eq!(s.address(b"an attachment"), s.address(b"an attachment"));
    }

    #[test]
    fn different_content_addresses_differently() {
        let s = secret();
        assert_ne!(s.address(b"a"), s.address(b"b"));
    }

    #[test]
    fn a_known_file_cannot_be_confirmed_without_the_secret() {
        // The attack D-43 exists to close. An observer holding the candidate file, and the
        // algorithm, and the source code, still cannot compute the address it would have in
        // *this* installation.
        let mine = secret();
        let theirs = InstallationSecret::from_bytes([9u8; 32]);
        let candidate = b"a document the observer already has";
        assert_ne!(
            mine.address(candidate),
            theirs.address(candidate),
            "the address does not depend on the installation secret"
        );
    }

    #[test]
    fn dedup_still_works_within_one_installation() {
        // "Convergence was only ever needed within one installation." Two accounts on one
        // machine share a secret, so they share an address and the blob is stored once.
        let s = secret();
        let account_a = s.address(b"the same PDF");
        let account_b = s.address(b"the same PDF");
        assert_eq!(account_a, account_b);
    }

    #[test]
    fn the_address_and_the_key_are_independent() {
        // The address is a filename on disk. If it were derivable from the key material, or
        // the key from it, the directory listing would be leaking the key schedule.
        let s = secret();
        let content = b"content";
        let address = s.address(content);
        let key = s.blob_key(content);
        assert_ne!(address.as_bytes(), key.bytes());
    }

    #[test]
    fn every_derived_key_is_distinct() {
        // Domain separation across all four contexts. Two of these sharing a value would
        // mean the shared blob index and the installation policy store were sealed under
        // one key, so compromising one would unseal the other.
        let s = secret();
        let keys = [
            key_bytes(&s.blob_key(b"x")),
            key_bytes(&s.shared_index_key()),
            key_bytes(&s.installation_policy_key()),
        ];
        for (i, a) in keys.iter().enumerate() {
            for (j, b) in keys.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "contexts {i} and {j} derive the same key");
                }
            }
        }
    }

    #[test]
    fn the_convergent_key_depends_on_the_content() {
        // "Each blob encrypted under a key derived from the blob's own content."
        let s = secret();
        assert_ne!(key_bytes(&s.blob_key(b"a")), key_bytes(&s.blob_key(b"b")));
        assert_eq!(key_bytes(&s.blob_key(b"a")), key_bytes(&s.blob_key(b"a")));
    }

    #[test]
    fn the_fan_out_is_two_levels_of_one_byte() {
        // Fixed at first release, because the layout is published in the Cask uninstall
        // stanza and a change breaks uninstall for existing users.
        let s = secret();
        let a = s.address(b"anything");
        let hex = a.hex();
        assert_eq!(a.path(), format!("{}/{}/{hex}", &hex[0..2], &hex[2..4]));
        assert_eq!(a.path().matches('/').count(), 2);
    }

    #[test]
    fn an_address_is_thirty_two_bytes() {
        assert_eq!(secret().address(b"x").as_bytes().len(), 32);
        assert_eq!(secret().address(b"x").hex().len(), 64);
    }

    #[test]
    fn the_secret_does_not_print_itself() {
        // It reaches a Debug formatter through any struct that holds it, and NFR-23 says
        // credentials never appear in logs or crash dumps.
        let rendered = format!("{:?}", secret());
        assert!(!rendered.contains('7'), "the secret rendered its bytes");
        assert!(rendered.contains("redacted"));
    }

    fn key_bytes(k: &PageKey) -> [u8; 32] {
        *k.bytes()
    }
}

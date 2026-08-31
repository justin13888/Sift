//! D-77 — a blob is a chunked, sealed file whose name is its address.

use sift_crypto::address::ContentAddress;
use sift_foundation::limits::L14_BLOB_BYTES;

/// The chunk size.
///
/// D-77 chunks rather than sealing the whole file because **three consumers need less than
/// the whole thing**: FR-10's preview hand-off, NFR-39's byte ceiling on a single fetch, and
/// NFR-19's rule that no stage may materialize a large part. A whole-file seal would force
/// every one of them to hold a 2 GB blob in memory to read its first page.
///
/// D-77 concedes the trade: chunking buys large-attachment behaviour in a client that does
/// not send mail and whose users mostly receive small ones.
pub const CHUNK_BYTES: usize = 256 * 1024;

/// Why a blob could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobError {
    /// L-14. Above NFR-14's default budget, so in practice that binds first — this exists so
    /// a single attachment cannot be the thing that makes the budget unenforceable.
    TooLarge,
    /// A chunk failed to authenticate.
    ///
    /// **The file is deleted, not repaired, and its reference count is left alone.** The next
    /// fetch re-downloads. Repairing would mean trusting bytes that already failed the only
    /// check there is.
    FailedAuthentication,
    /// The index names a blob that is not on disk.
    ///
    /// **Harmless, and MUST NOT be an error**: the row is dropped and the content is treated
    /// as not cached, which is an ordinary FR-12 state rather than a fault.
    Missing,
}

/// A blob being written.
///
/// **Staged outside the content-addressed store and entering it only when complete.** This
/// is forced rather than chosen: a blob's address is a hash of its *complete* content and is
/// not known until the last byte arrives, so there is no name to write it under until then.
/// Writing partial content under a provisional address was rejected — a provisional address
/// is one another reader can resolve.
#[derive(Debug)]
pub struct Staged {
    written: u64,
    /// What the transfer said it would be.
    ///
    /// D-102 requires this be checked, and the gap it closes is specific: L-10 bounds images
    /// and nothing bounded attachments, so an endless attachment met only L-14's 2 GB.
    declared_length: Option<u64>,
    chunks: Vec<Vec<u8>>,
    current: Vec<u8>,
}

impl Staged {
    #[must_use]
    pub fn new(declared_length: Option<u64>) -> Self {
        Self {
            written: 0,
            declared_length,
            chunks: Vec::new(),
            current: Vec::new(),
        }
    }

    /// Append transferred bytes.
    ///
    /// # Errors
    /// [`BlobError::TooLarge`] where the transfer exceeds what it declared, or L-14.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), BlobError> {
        self.written += bytes.len() as u64;
        if self.written > L14_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        if self.declared_length.is_some_and(|d| self.written > d) {
            // The transfer exceeded its own declaration. A sender controls both, so the
            // check is against whichever is smaller.
            return Err(BlobError::TooLarge);
        }
        for byte in bytes {
            self.current.push(*byte);
            if self.current.len() >= CHUNK_BYTES {
                self.chunks.push(core::mem::take(&mut self.current));
            }
        }
        Ok(())
    }

    /// Whether the transfer delivered what it promised.
    ///
    /// A short transfer is **not** complete: entering it into the store would give the
    /// address of a truncated file to content that is not that file.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.declared_length.is_none_or(|d| self.written == d)
    }

    #[must_use]
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Finish, yielding the chunks. `None` if the transfer was short.
    #[must_use]
    pub fn finish(mut self) -> Option<Vec<Vec<u8>>> {
        if !self.is_complete() {
            return None;
        }
        if !self.current.is_empty() {
            self.chunks.push(core::mem::take(&mut self.current));
        }
        Some(self.chunks)
    }
}

/// Where a blob lives, relative to the store root.
///
/// The filename **is** the content address, in a fan-out derived from it. That is safe only
/// because the address is a *keyed* hash: an unkeyed one would be a stable global identifier
/// discoverable by anyone holding the file, which is the confirmation attack D-43 closes.
///
/// The fan-out depth is **fixed at first release**, because the on-disk layout is published
/// in the Cask uninstall stanza and changing it breaks uninstall for existing users.
#[must_use]
pub fn path_of(address: &ContentAddress) -> String {
    address.path()
}

/// Two kinds of orphan, and only one of them matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orphan {
    /// A file the index does not name.
    ///
    /// **A leak, and invisible to eviction** — eviction walks the index, so a file it never
    /// hears about is never considered. The refcount rebuild required after abnormal
    /// termination must collect these in the same pass, on the same trigger.
    FileWithNoRow,
    /// An index row naming a file that is not there.
    ///
    /// **Harmless, and MUST NOT be an error.** The row is dropped and the content becomes an
    /// ordinary FR-12 "not cached" state.
    RowWithNoFile,
}

impl Orphan {
    #[must_use]
    pub const fn is_a_leak(self) -> bool {
        matches!(self, Self::FileWithNoRow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_crypto::address::InstallationSecret;

    #[test]
    fn a_short_transfer_never_enters_the_store() {
        // Entering it would give the address of a truncated file to content that is not that
        // file, and every future reader would resolve that address to the wrong bytes.
        let mut s = Staged::new(Some(100));
        s.write(&[0u8; 50]).expect("within bounds");
        assert!(!s.is_complete());
        assert!(s.finish().is_none());
    }

    #[test]
    fn a_transfer_that_exceeds_its_declaration_is_refused() {
        // The gap D-102 closes: L-10 bounds images and nothing bounded attachments, so an
        // endless attachment met only L-14's 2 GB.
        let mut s = Staged::new(Some(100));
        assert_eq!(s.write(&[0u8; 200]), Err(BlobError::TooLarge));
    }

    #[test]
    fn a_complete_transfer_chunks_and_finishes() {
        let mut s = Staged::new(Some(CHUNK_BYTES as u64 * 2 + 10));
        s.write(&vec![7u8; CHUNK_BYTES * 2 + 10])
            .expect("within bounds");
        assert!(s.is_complete());
        let chunks = s.finish().expect("complete");
        assert_eq!(chunks.len(), 3, "chunking did not split at the boundary");
        assert_eq!(chunks[0].len(), CHUNK_BYTES);
        assert_eq!(chunks[2].len(), 10);
    }

    #[test]
    fn a_transfer_with_no_declared_length_is_complete_when_it_stops() {
        // Not every provider declares one, and refusing those would refuse real mail.
        let mut s = Staged::new(None);
        s.write(b"bytes").expect("within bounds");
        assert!(s.is_complete());
        assert_eq!(s.finish().expect("complete").len(), 1);
    }

    #[test]
    fn nothing_may_exceed_l14() {
        let mut s = Staged::new(None);
        assert_eq!(s.write(&[0u8; 8]), Ok(()));
        let mut huge = Staged::new(None);
        huge.written = L14_BLOB_BYTES;
        assert_eq!(huge.write(&[0u8; 1]), Err(BlobError::TooLarge));
    }

    #[test]
    fn a_files_name_is_its_address_in_a_derived_fan_out() {
        let secret = InstallationSecret::from_bytes([3u8; 32]);
        let address = secret.address(b"an attachment");
        let path = path_of(&address);
        assert!(path.ends_with(&address.hex()));
        assert_eq!(path.matches('/').count(), 2, "the fan-out depth changed");
    }

    #[test]
    fn only_one_kind_of_orphan_is_a_leak() {
        // A file the index does not name is invisible to eviction, because eviction walks
        // the index. A row with no file is an FR-12 state rather than a fault.
        assert!(Orphan::FileWithNoRow.is_a_leak());
        assert!(!Orphan::RowWithNoFile.is_a_leak());
    }
}

//! The message flag word, in the one place every reader and writer of it must agree.
//!
//! It is a bit field rather than a column per state because D-51 makes the *base* state one
//! value that a pending overlay is read over: the overlay decides what a row shows, and the
//! thing it decides over has to be a single readable value rather than a set of columns that
//! could disagree with each other.
//!
//! **Read is stored as read, not as unread.** Some providers carry the state as the presence
//! of an unread marker, and the adapter is where that inversion is undone — because a store
//! whose meaning depended on which provider wrote the row is a store nothing above it can
//! query.

/// The message has been read.
pub const READ: i64 = 1 << 0;
/// The message is flagged, starred, or whatever the provider calls its single-bit
/// attention axis.
pub const FLAGGED: i64 = 1 << 1;
/// The message carries at least one part that is an attachment rather than body content.
pub const HAS_ATTACHMENT: i64 = 1 << 2;

/// Compose a flag word.
#[must_use]
pub const fn word(read: bool, flagged: bool) -> i64 {
    (if read { READ } else { 0 }) | (if flagged { FLAGGED } else { 0 })
}

#[must_use]
pub const fn is_read(flags: i64) -> bool {
    flags & READ != 0
}

#[must_use]
pub const fn is_flagged(flags: i64) -> bool {
    flags & FLAGGED != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bits_round_trip() {
        for read in [false, true] {
            for flagged in [false, true] {
                let w = word(read, flagged);
                assert_eq!(is_read(w), read);
                assert_eq!(is_flagged(w), flagged);
            }
        }
    }

    #[test]
    fn an_unset_word_is_unread_and_unflagged() {
        // The default a fresh row carries, and the one an arriving message should have.
        assert!(!is_read(0));
        assert!(!is_flagged(0));
    }

    #[test]
    fn the_bits_do_not_overlap() {
        assert_eq!(READ & FLAGGED, 0);
        assert_eq!(READ & HAS_ATTACHMENT, 0);
        assert_eq!(FLAGGED & HAS_ATTACHMENT, 0);
    }
}

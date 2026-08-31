//! D-78 — local identity, and D-89 — account identity.
//!
//! # Why local identity is not a row number
//!
//! D-55 orders the message list on the server's received time **with local identity as a
//! stable tiebreak**, and D-4's unified inbox assembles that order by merging per-account
//! result streams in memory. A merge needs a **total** order, and the same comparator has
//! to serve each account's own query and the in-memory merge. A per-database row
//! identifier is not comparable across accounts; a per-account counter collides across
//! them.
//!
//! A single installation-wide counter would be comparable, and is rejected for a
//! different reason: it puts a shared-store write on the **ingest path of every message**,
//! which serializes D-6's parallel writers and makes the installation policy store hot
//! during D-53's backfill — exactly when the most messages are arriving at once.
//!
//! So identity is generated per account with **no cross-account coordination at ingest**,
//! and is nonetheless unique across every account in the installation. The coordination
//! that remains happens once, when an account is added: it is handed an
//! [`AccountOrdinal`], and adding an account is a rare interactive act rather than a hot
//! path.
//!
//! # What D-78 requires, and where each property lives in the layout
//!
//! | Property | How |
//! |---|---|
//! | Fixed width | 128 bits, always |
//! | Time-ordered | a 48-bit millisecond timestamp in the **high** bytes |
//! | Unique across the installation | the account ordinal is part of the value |
//! | No coordination at ingest | the ordinal is assigned once, at account creation |
//! | Never reused | time is monotonic here even when the clock is not |
//! | Stable for the life of the message | nothing recomputes it |
//!
//! Bytes are big-endian throughout, so **lexicographic byte order is numeric order is
//! time order**. That matters because this value is an index key in every account
//! database and a field in every row crossing the C ABI.
//!
//! # What it costs
//!
//! A wider key in every index, every foreign reference, and every ABI row — paid for a
//! property that only the unified-inbox merge needs, and D-4 concedes the unified inbox
//! is the feature to cut. That trade is D-78's own recorded weakness.
//!
//! # The one exception to stability
//!
//! Under D-44, a move on a provider with unstable identifiers that does not corroborate
//! to exactly one candidate **presents as a delete plus an arrival**, and the arrival has
//! a new identity. That is a deliberate loss, not a bug: merging wrongly is data loss
//! wearing a display defect's clothes.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// D-89 — the identity Sift assigns an account when it is added.
///
/// Independent of the address, of the provider, and of any server-side identifier. It is
/// the one value the user cannot influence, and it names the account's two files, its
/// credential items, and its rows in the shared blob index.
///
/// Because it is Sift's own rather than the address, an item survives an address change —
/// and **FR-4's erasure becomes assertable by enumeration** rather than believed.
///
/// Adding the same mailbox twice is permitted and warned about; removing and re-adding
/// produces a **new account with no relationship to the old one**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(u128);

impl AccountId {
    #[must_use]
    pub const fn from_u128(v: u128) -> Self {
        Self(v)
    }

    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// The account's slot in this installation, assigned once when the account is added.
///
/// This is the whole of the coordination D-78 needs, and it is deliberately not on the
/// ingest path. Sixteen bits is far more than the five accounts the scale corpus
/// describes or the L-23 connection budget's arithmetic assumes, and an ordinal is never
/// reused within an installation so that a removed account cannot collide with a later
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountOrdinal(u16);

impl AccountOrdinal {
    #[must_use]
    pub const fn new(v: u16) -> Self {
        Self(v)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// D-78 — a message's local identity.
///
/// Sift's own, assigned on ingest. Remote identifiers are attributes, not keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId(u128);

/// Bit layout. Documented as constants rather than as magic numbers, because the layout
/// is on disk in every index and cannot be changed without a migration.
const TIMESTAMP_BITS: u32 = 48;
const ORDINAL_BITS: u32 = 16;
const SEQUENCE_BITS: u32 = 16;
const ENTROPY_BITS: u32 = 48;
const _: () = assert!(TIMESTAMP_BITS + ORDINAL_BITS + SEQUENCE_BITS + ENTROPY_BITS == 128);

const ORDINAL_SHIFT: u32 = SEQUENCE_BITS + ENTROPY_BITS;
const TIMESTAMP_SHIFT: u32 = ORDINAL_SHIFT + ORDINAL_BITS;
const SEQUENCE_SHIFT: u32 = ENTROPY_BITS;
const SEQUENCE_MAX: u64 = (1 << SEQUENCE_BITS) - 1;

impl LocalId {
    #[must_use]
    pub const fn from_u128(v: u128) -> Self {
        Self(v)
    }

    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// Big-endian bytes. Lexicographic order over these is the same total order
    /// [`Ord`] gives, which is what lets the value be an index key without a custom
    /// collation.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    #[must_use]
    pub const fn from_bytes(b: [u8; 16]) -> Self {
        Self(u128::from_be_bytes(b))
    }

    /// Milliseconds since the Unix epoch at which this identity was generated.
    #[must_use]
    pub const fn millis(self) -> u64 {
        (self.0 >> TIMESTAMP_SHIFT) as u64
    }

    /// The account this identity was generated for.
    #[must_use]
    pub const fn ordinal(self) -> AccountOrdinal {
        AccountOrdinal(((self.0 >> ORDINAL_SHIFT) & 0xFFFF) as u16)
    }
}

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Generates [`LocalId`] values for one account.
///
/// Lock-free: ingest is the hot path during D-53's backfill, and D-6 already permits
/// parallel writers across accounts. Nothing here reaches another account's generator, so
/// there is nothing to contend on.
#[derive(Debug)]
pub struct LocalIdGenerator {
    ordinal: AccountOrdinal,
    /// Packed `millis << SEQUENCE_BITS | sequence`, so the monotonic guarantee is one
    /// compare-and-swap rather than two fields that can be observed disagreeing.
    clock: AtomicU64,
    entropy: AtomicU64,
}

impl LocalIdGenerator {
    /// # Panics
    /// Never. A clock before the Unix epoch is treated as the epoch.
    #[must_use]
    pub fn new(ordinal: AccountOrdinal) -> Self {
        Self {
            ordinal,
            clock: AtomicU64::new(0),
            entropy: AtomicU64::new(seed()),
        }
    }

    /// Mint the next identity.
    ///
    /// Monotonic **even when the system clock is not**. A wall-clock correction that moves
    /// time backwards must not be able to mint an identity that already exists: a
    /// reissued identity would resolve a stale reference to a *different* message, which
    /// is the failure D-78's "never reused" exists to prevent. So a backwards clock keeps
    /// the last observed millisecond and advances the sequence instead.
    pub fn next(&self) -> LocalId {
        let now = now_millis().min((1 << TIMESTAMP_BITS) - 1);

        // `fetch_update` yields the value that was there *before* the update. The value
        // this identity must use is the one that replaced it, so it is recomputed from the
        // same inputs — `advance` is deterministic, so this is the value that was stored
        // rather than a guess at it.
        let previous = self
            .clock
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |prev| {
                Some(advance(prev, now))
            })
            .unwrap_or(0);
        let packed = advance(previous, now);

        let millis = u128::from(packed >> SEQUENCE_BITS);
        let sequence = u128::from(packed & SEQUENCE_MAX);
        let entropy = u128::from(self.next_entropy() & ((1 << ENTROPY_BITS) - 1));

        LocalId(
            (millis << TIMESTAMP_SHIFT)
                | (u128::from(self.ordinal.get()) << ORDINAL_SHIFT)
                | (sequence << SEQUENCE_SHIFT)
                | entropy,
        )
    }

    /// SplitMix64. **Not a security random** — the D-28 capability token is the value that
    /// must be unguessable, and it is generated elsewhere from the platform's own source.
    /// This field exists so that two identities minted in the same millisecond either side
    /// of a restart, where the sequence counter has reset, still differ.
    fn next_entropy(&self) -> u64 {
        let z = self
            .entropy
            .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The monotonic step, extracted so that the cases which cannot be provoked from a test
/// — a clock correction moving time backwards, and more than 65,536 identities in one
/// millisecond — can be asserted directly rather than hoped for.
const fn advance(prev: u64, now: u64) -> u64 {
    let prev_millis = prev >> SEQUENCE_BITS;
    let prev_seq = prev & SEQUENCE_MAX;
    if now > prev_millis {
        now << SEQUENCE_BITS
    } else if prev_seq < SEQUENCE_MAX {
        // Same millisecond, or the clock moved backwards. Either way, forward.
        prev + 1
    } else {
        // More than 65,536 in one millisecond: borrow from the next millisecond rather
        // than wrapping the sequence, which would repeat a value.
        (prev_millis + 1) << SEQUENCE_BITS
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    // Mixed with the address of a fresh allocation so that two generators created in the
    // same nanosecond, in the same process, do not share a stream.
    let boxed = Box::new(0u8);
    let addr = std::ptr::from_ref::<u8>(&*boxed) as usize as u64;
    u64::from(nanos).rotate_left(32) ^ addr ^ 0x2545_F491_4F6C_DD1D
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn generator(ordinal: u16) -> LocalIdGenerator {
        LocalIdGenerator::new(AccountOrdinal::new(ordinal))
    }

    #[test]
    fn identity_is_fixed_width() {
        // "Fixed-width" is not a detail: this value is a column in every index and a
        // field in every fixed-layout record crossing the C ABI under D-66.
        assert_eq!(size_of::<LocalId>(), 16);
        assert_eq!(generator(1).next().to_bytes().len(), 16);
    }

    #[test]
    fn identity_is_never_reused() {
        let g = generator(7);
        let n = 200_000;
        let seen: BTreeSet<LocalId> = (0..n).map(|_| g.next()).collect();
        assert_eq!(seen.len(), n, "an identity was minted twice");
    }

    #[test]
    fn identity_is_time_ordered() {
        let g = generator(3);
        let ids: Vec<LocalId> = (0..10_000).map(|_| g.next()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "generation order is not sort order");
    }

    #[test]
    fn byte_order_is_sort_order() {
        // The value is an index key. If lexicographic byte order diverged from Ord, every
        // range scan would need a custom collation and D-55's comparator would have two
        // implementations that could disagree.
        let g = generator(9);
        let ids: Vec<LocalId> = (0..2_000).map(|_| g.next()).collect();
        let mut by_value = ids.clone();
        by_value.sort_unstable();
        let mut by_bytes = ids;
        by_bytes.sort_unstable_by_key(|i| i.to_bytes());
        assert_eq!(by_value, by_bytes);
    }

    #[test]
    fn accounts_do_not_collide_without_coordinating() {
        // The property that lets ingest write in parallel across accounts: two generators
        // that never speak to each other still cannot mint the same value.
        let a = generator(1);
        let b = generator(2);
        let mut all = BTreeSet::new();
        for _ in 0..50_000 {
            assert!(all.insert(a.next()), "account 1 repeated");
            assert!(all.insert(b.next()), "account 2 repeated");
        }
        assert_eq!(all.len(), 100_000);
    }

    #[test]
    fn the_account_is_recoverable_from_the_identity() {
        let id = generator(4242).next();
        assert_eq!(id.ordinal(), AccountOrdinal::new(4242));
    }

    #[test]
    fn a_backwards_clock_cannot_reissue_an_identity() {
        // A wall-clock correction is the case that would silently reissue values. A
        // reissued identity resolves a stale reference to a *different* message, which is
        // exactly what "never reused" exists to prevent.
        let at_1000 = advance(0, 1000);
        let stepped_back = advance(at_1000, 500);
        assert!(
            stepped_back > at_1000,
            "time moved back and so did identity"
        );
        assert_eq!(
            stepped_back >> SEQUENCE_BITS,
            1000,
            "kept the last observed millisecond"
        );
        assert_eq!(
            stepped_back & SEQUENCE_MAX,
            1,
            "advanced the sequence instead"
        );
    }

    #[test]
    fn a_full_sequence_borrows_from_the_next_millisecond() {
        // Wrapping the sequence would repeat a value. Borrowing is the only answer that
        // stays monotonic; it costs at most a millisecond of drift ahead of the clock.
        let full = (1000 << SEQUENCE_BITS) | SEQUENCE_MAX;
        let next = advance(full, 1000);
        assert_eq!(next >> SEQUENCE_BITS, 1001);
        assert_eq!(next & SEQUENCE_MAX, 0);
        assert!(next > full);
    }

    #[test]
    fn advancing_is_strictly_monotonic_for_any_clock() {
        // Every path through advance() must move forward, whatever the clock does.
        let clocks = [0u64, 1, 999, 1000, 1001, u64::from(u32::MAX)];
        for &now in &clocks {
            for &then in &clocks {
                let a = advance(0, then);
                let b = advance(a, now);
                assert!(b > a, "advance({a}, {now}) did not move forward");
            }
        }
    }

    #[test]
    fn an_identity_carries_the_time_it_was_minted() {
        // The bug this exists for: `fetch_update` returns the *previous* value, so an
        // implementation that uses it directly gives every identity the timestamp of the one
        // before — and the very first identity from a fresh generator carries zero.
        //
        // Two accounts added in one session would then both mint a first message stamped at
        // the epoch, and D-55's cross-account merge would order them by ordinal rather than
        // by when they arrived.
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis() as u64;

        let first = generator(1).next();
        assert!(
            first.millis() >= before,
            "the first identity from a fresh generator is stamped {} rather than now",
            first.millis()
        );

        let g = generator(2);
        for _ in 0..100 {
            let id = g.next();
            assert!(
                id.millis() >= before,
                "an identity lagged behind its own generator"
            );
        }
    }

    #[test]
    fn two_fresh_generators_agree_about_the_present() {
        // The cross-account consequence: the unified-inbox merge orders on received time
        // with identity as the tiebreak, so identities that disagree about *now* put two
        // accounts' first messages in an order nothing chose.
        let a = generator(1).next();
        let b = generator(2).next();
        let gap = a.millis().abs_diff(b.millis());
        assert!(gap < 1_000, "two generators started {gap} ms apart");
    }

    #[test]
    fn bytes_round_trip() {
        let id = generator(11).next();
        assert_eq!(LocalId::from_bytes(id.to_bytes()), id);
        assert_eq!(LocalId::from_u128(id.as_u128()), id);
    }

    #[test]
    fn the_layout_fills_exactly_one_hundred_and_twenty_eight_bits() {
        // Asserted at compile time too, but stated here so the intent is greppable: the
        // layout is on disk in every index and cannot change without a migration.
        assert_eq!(
            TIMESTAMP_BITS + ORDINAL_BITS + SEQUENCE_BITS + ENTROPY_BITS,
            128
        );
    }
}

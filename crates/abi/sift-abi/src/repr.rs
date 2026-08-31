//! D-66 — the boundary's representation, stated once.
//!
//! One calling convention, one string representation, one aggregate shape, and build-time
//! exhaustiveness in place of runtime tolerance.
//!
//! D-17 concedes what this costs: every type crossing needs an explicit, stable
//! representation, ownership rules have to be written down rather than inferred, and
//! **memory-safety bugs are possible here in a way they are not elsewhere**. That is why
//! this crate is one of the four where `unsafe` is permitted, and why the surface is kept
//! small enough to read in one place. D-17's own tripwire: *if the ABI surface grows past
//! what one file can hold, that is the signal this was the wrong shape.*
//!
//! # Ownership, in one rule
//!
//! **Every value crossing the boundary is owned by the layer and borrowed by the shell for
//! the duration of the call or callback that delivered it.** A shell that needs a value
//! beyond that copies it. The layer frees nothing a shell might still hold; a shell frees
//! nothing the layer allocated.

use core::marker::PhantomData;
use core::slice;

/// The result of every entry point.
///
/// **No entry point encodes failure in its return value's domain.** Sentinel returns,
/// null-means-error, and a thread-local last-error are all excluded — the first two make
/// the failure case indistinguishable from a legal value at some point in the future, and
/// the third makes it depend on which thread asked.
///
/// The status distinguishes **three** things rather than two, and the third is the point:
/// a caught panic is its own value, everywhere. D-47 catches panics at each pipeline stage
/// and no unwind may cross this boundary, so the shell has to be able to tell "this failed
/// in a way the design anticipated" from "this failed in a way it did not". Collapsing
/// them would make every panic look like an ordinary parse failure, which D-47 forbids
/// explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SiftStatus {
    /// The call succeeded and any out-parameters have been written.
    Ok = 0,
    /// The call failed in an identified way. The identified state is delivered separately;
    /// **a failure crossing this boundary is a state, never a message**.
    Failed = 1,
    /// A panic was caught at this entry point. Out-parameters are not written.
    ///
    /// Counted against the subsystem whose tag was current, per D-24, and never absorbed
    /// as an ordinary failure.
    Panicked = 2,
}

/// A UTF-8 string, as a pointer and a length.
///
/// **Never NUL-terminated, and the length is authoritative** — nothing on this boundary
/// scans for a terminator. Two consequences the shells depend on:
///
/// - **Every truncation on this boundary is one the presentation layer performed
///   deliberately**, under a bound in `limits.md`. None is a property of the encoding. A
///   NUL-terminated representation would let an embedded NUL — which a sender can put in a
///   header — truncate a subject at a point nobody chose, which is precisely the
///   attacker-chooses-the-cut problem the limits register rejects for documents.
/// - **Validity is established once**, where normalization happens, and is not re-checked
///   by the shell.
///
/// # Safety
///
/// Valid only for the duration of the call or callback that delivered it.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SiftStr<'a> {
    ptr: *const u8,
    len: usize,
    _borrow: PhantomData<&'a str>,
}

impl<'a> SiftStr<'a> {
    #[must_use]
    pub const fn new(s: &'a str) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len(),
            _borrow: PhantomData,
        }
    }

    /// An absent value. Distinct from the empty string: a message with no subject and a
    /// message whose subject is "" are different facts, and a shell renders them
    /// differently.
    #[must_use]
    pub const fn null() -> Self {
        Self {
            ptr: core::ptr::null(),
            len: 0,
            _borrow: PhantomData,
        }
    }

    #[must_use]
    pub const fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Recover the string.
    ///
    /// # Safety
    ///
    /// The caller must guarantee the delivery that produced this value is still live, and
    /// that the bytes are the UTF-8 the layer validated. Both hold for a value received in
    /// a call or callback and not stored past it.
    #[must_use]
    pub unsafe fn as_str(&self) -> Option<&'a str> {
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the layer only ever constructs this from a live `&str`, so the bytes are
        // valid UTF-8 and the range is initialised. The caller's obligation above covers
        // liveness, which is the half this function cannot check.
        unsafe {
            let bytes = slice::from_raw_parts(self.ptr, self.len);
            Some(core::str::from_utf8_unchecked(bytes))
        }
    }
}

impl core::fmt::Debug for SiftStr<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_null() {
            f.write_str("SiftStr(null)")
        } else {
            // SAFETY: borrowed for the lifetime of `self`, which is what Debug has.
            let s = unsafe { self.as_str() };
            write!(f, "SiftStr({s:?})")
        }
    }
}

/// A contiguous, borrowed array of fixed-layout records.
///
/// The alternative — an opaque row handle with a per-field accessor — was excluded
/// arithmetically rather than on taste. FR-6's list row carries about ten fields, and
/// NFR-6 requires 60 fps with **zero dropped frames over a 10,000-row fling**. That is a
/// hundred thousand boundary crossings per fling, against one delivery.
///
/// # Safety
///
/// Borrowed for the duration of the delivery. Each record's text fields are [`SiftStr`]
/// pointing into layer-owned storage with the same lifetime.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SiftRows<'a, T> {
    ptr: *const T,
    len: usize,
    _borrow: PhantomData<&'a [T]>,
}

impl<'a, T> SiftRows<'a, T> {
    #[must_use]
    pub const fn new(rows: &'a [T]) -> Self {
        Self {
            ptr: rows.as_ptr(),
            len: rows.len(),
            _borrow: PhantomData,
        }
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: core::ptr::NonNull::dangling().as_ptr(),
            len: 0,
            _borrow: PhantomData,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// # Safety
    ///
    /// The caller must guarantee the delivery that produced this value is still live.
    #[must_use]
    pub unsafe fn as_slice(&self) -> &'a [T] {
        // SAFETY: constructed from a live slice; liveness is the caller's obligation.
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T> core::fmt::Debug for SiftRows<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SiftRows(len={})", self.len)
    }
}

/// The generation of an observation.
///
/// D-66's answer to a race D-48 would otherwise deadlock on. Cancellation is synchronous:
/// when it returns, no further callback for that observation will arrive, on any thread,
/// ever. But a delivery may already have been *posted* to the shell's main loop — and a
/// cancellation that waited for posted deliveries would be **waiting on the very loop that
/// called it**, which deadlocks deterministically rather than occasionally.
///
/// So cancellation rendezvous with worker-side work only, and advances the generation. A
/// stale delivery reaching the main loop is discarded by comparing generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Generation(pub u64);

impl Generation {
    pub const FIRST: Self = Self(0);

    /// Advance, which is what cancellation does.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Whether a delivery posted under `posted` should still be handed to the shell.
    #[must_use]
    pub const fn accepts(self, posted: Self) -> bool {
        posted.0 == self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_round_trips() {
        let s = "Ünïcode subject";
        let w = SiftStr::new(s);
        assert_eq!(w.len(), s.len(), "the length is in bytes, not characters");
        assert_eq!(unsafe { w.as_str() }, Some(s));
    }

    #[test]
    fn absent_and_empty_are_different_facts() {
        // A message with no subject and a message whose subject is "" are not the same
        // thing, and a shell renders them differently. A representation that could not
        // distinguish them would force the layer to invent one, which I9 forbids in the
        // body and honesty forbids here.
        let absent = SiftStr::null();
        let empty = SiftStr::new("");
        assert!(absent.is_null());
        assert!(!empty.is_null());
        assert!(absent.is_empty() && empty.is_empty());
        assert_eq!(unsafe { absent.as_str() }, None);
        assert_eq!(unsafe { empty.as_str() }, Some(""));
    }

    #[test]
    fn a_string_is_not_nul_terminated_and_the_length_is_authoritative() {
        // A sender can put a NUL in a header. Under a NUL-terminated representation that
        // would truncate a subject at a point nobody chose — the attacker choosing where
        // the cut falls, which is exactly what the limits register refuses for documents.
        let hostile = "Invoice\u{0000}.pdf.exe";
        let w = SiftStr::new(hostile);
        assert_eq!(w.len(), hostile.len());
        let recovered = unsafe { w.as_str() }.expect("present");
        assert_eq!(recovered, hostile, "the value was cut at the embedded NUL");
        assert!(recovered.contains('\u{0000}'));
    }

    #[test]
    fn rows_cross_as_one_contiguous_borrow() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        #[repr(C)]
        struct Row {
            id: u128,
            unread: u8,
        }
        let rows = [Row { id: 1, unread: 1 }, Row { id: 2, unread: 0 }];
        let w = SiftRows::new(&rows);
        assert_eq!(w.len(), 2);
        assert_eq!(unsafe { w.as_slice() }, &rows);
    }

    #[test]
    fn an_empty_row_array_is_valid_and_not_null() {
        // A dangling-but-aligned pointer rather than null: a shell that does pointer
        // arithmetic on an empty array must not be doing it on null.
        let w = SiftRows::<u8>::empty();
        assert!(w.is_empty());
        assert!(unsafe { w.as_slice() }.is_empty());
    }

    #[test]
    fn the_representation_is_two_words() {
        // Fixed layout is what makes a row array readable from Swift and from C without
        // either side describing it twice.
        assert_eq!(
            size_of::<SiftStr<'_>>(),
            size_of::<*const u8>() + size_of::<usize>()
        );
        assert_eq!(
            size_of::<SiftRows<'_, u8>>(),
            size_of::<*const u8>() + size_of::<usize>()
        );
    }

    #[test]
    fn the_three_statuses_are_distinct_and_success_is_zero() {
        assert_eq!(SiftStatus::Ok as i32, 0);
        assert_ne!(SiftStatus::Failed as i32, SiftStatus::Panicked as i32);
        assert_ne!(SiftStatus::Ok as i32, SiftStatus::Failed as i32);
    }

    #[test]
    fn a_delivery_posted_before_cancellation_is_discarded() {
        // The race D-48 would otherwise deadlock on. Cancellation cannot wait for posted
        // deliveries — it would be waiting on the main loop that called it — so it
        // advances the generation and the stale delivery is dropped on arrival.
        let live = Generation::FIRST;
        let posted = live;
        assert!(
            live.accepts(posted),
            "a delivery under the live generation is handed over"
        );

        let after_cancel = live.next();
        assert!(
            !after_cancel.accepts(posted),
            "a delivery posted before cancellation survived"
        );
    }

    #[test]
    fn generations_do_not_repeat() {
        let mut g = Generation::FIRST;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1000 {
            assert!(
                seen.insert(g),
                "a generation repeated, so a stale delivery would pass"
            );
            g = g.next();
        }
    }

    #[test]
    fn a_generation_is_one_word_on_the_wire() {
        assert_eq!(size_of::<Generation>(), size_of::<u64>());
    }
}

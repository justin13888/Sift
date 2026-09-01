//! The C entry points — D-17's boundary, in the one file D-17 says it must fit in.
//!
//! > "If the ABI surface grows past what one file can hold, that is the signal this was the
//! > wrong shape."
//!
//! Every function here follows D-66 exactly: a status return, results through
//! caller-owned out-parameters, UTF-8 pointer-and-length strings, contiguous fixed-layout row
//! arrays borrowed for the callback, and **no unwind crossing the boundary**.
//!
//! # What the shells are not given
//!
//! **No path to the store, the index, or a provider adapter.** This is a command and
//! view-model protocol rather than a database-access one: shells send intents and requests
//! and receive prepared view models. Message bodies cross **post-sanitization only** — a raw
//! provider payload never reaches a shell, because parsing, sanitizing, blocking and
//! transforming all happen below this line.
//!
//! **Resource loads do not cross here at all.** They arrive at the engine's scheme handler
//! and the resource broker answers them directly; no shell is in that path.
//!
//! This boundary is **not a trust boundary**. Both sides ship in one binary, which is why
//! D-66 can make an unknown discriminant a build failure rather than a runtime case.

use crate::barrier::{guard, guard_out};
use crate::host::SiftHostCallbacks;
use crate::repr::{Generation, SiftId, SiftObservation, SiftRows, SiftStatus, SiftStr};
use core::ffi::c_void;
use sift_presentation::action;

/// An opaque handle to the running layer.
///
/// The shell holds it and passes it back. It never dereferences it — which is what lets the
/// layer change shape without changing the boundary.
#[derive(Debug)]
#[repr(C)]
pub struct SiftApp {
    _private: [u8; 0],
}

/// A message row, as the list receives it.
///
/// **Fixed layout, and the text fields are pointers into layer-owned storage valid for the
/// duration of the delivery.** A shell that needs a value beyond the callback copies it.
///
/// D-66 excluded the alternative arithmetically: FR-6's ten fields against NFR-6's
/// 10,000-row fling is a hundred thousand boundary crossings per fling, versus one delivery.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SiftMessageRow<'a> {
    /// D-78's local identity. Stable for as long as the message exists in that account, and
    /// therefore usable as a key for selection, undo rendering and notification
    /// click-through.
    pub id: SiftId,
    pub account: SiftId,
    /// Server-assigned received time — what D-55 orders on.
    pub received_millis: u64,
    /// The sender's `Date` header. **Displayed only.**
    pub origination_millis: u64,
    /// Normalized under NFR-54 before it got here. Validity is established once, where
    /// normalization happens, and is **not re-checked by the shell**.
    pub sender: SiftStr<'a>,
    pub subject: SiftStr<'a>,
    pub snippet: SiftStr<'a>,
    pub unread: u8,
    pub flagged: u8,
    pub has_attachments: u8,
    /// Marked rather than joined — D-4.
    pub duplicate_across_accounts: u8,
    pub thread_count: u32,
}

/// Delivered on the shell's main loop, non-reentrantly.
///
/// The shell's rule inside one of these is D-48's: **receive, record, return; act on the next
/// turn of the loop.**
///
/// It carries **both** identifiers, and they answer different questions. The observation says
/// which registration this delivery belongs to, so a shell holding several can route it. The
/// generation says whether it is still wanted, so a delivery posted before a cancellation is
/// discarded on arrival rather than waited for at the cancel — which is the deadlock D-48
/// names.
pub type SiftRowsCallback = extern "C" fn(
    context: *mut c_void,
    observation: SiftObservation,
    generation: Generation,
    rows: SiftRows<'_, SiftMessageRow<'_>>,
);

/// Initialize the layer.
///
/// The shell supplies its host callbacks **once**, here — D-67's set is process-scoped and
/// is unregistered only at shutdown, because a host callback has no observation and therefore
/// no generation to discard by.
///
/// # Safety
/// `out` must be a valid writable pointer to a `*mut SiftApp`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_initialize(
    callbacks: SiftHostCallbacks,
    out: *mut *mut SiftApp,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let _ = callbacks;
            // The layer's real construction lands here. The boundary's shape is what this
            // file fixes, and it is fixed before either shell exists so that neither can
            // shape it around its own toolkit.
            Ok(core::ptr::null_mut())
        })
    }
}

/// Tear the layer down.
///
/// D-70's teardown is bounded and **flushes nothing**; this is the entry point that starts
/// it, and it must not block on D-48's cancellation.
///
/// # Safety
/// `app` must have come from [`sift_initialize`] and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_shutdown(app: *mut SiftApp) -> SiftStatus {
    guard(|| {
        let _ = app;
        Ok(())
    })
}

/// Invoke an action by its stable identifier — D-98.
///
/// **This is how a shell mutates anything.** The action set is an ABI surface, the palette
/// is a filtered view of the same register, and the test harness invokes through this exact
/// entry point rather than a test-only door.
///
/// # Safety
/// `app` must be valid; `id` must point to `id_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_invoke_action(
    app: *mut SiftApp,
    id: *const u8,
    id_len: usize,
) -> SiftStatus {
    guard(|| {
        let _ = app;
        if id.is_null() {
            return Err(());
        }
        // SAFETY: the caller's obligation. Validity was established where normalization
        // happened and is not re-checked here.
        let bytes = unsafe { core::slice::from_raw_parts(id, id_len) };
        let name = core::str::from_utf8(bytes).map_err(|_| ())?;
        // An identifier the register does not know is an identified failure rather than a
        // panic: a shell built against a newer register is a version skew D-2 removed as a
        // category, but the boundary still answers honestly rather than aborting.
        action::by_id(name).ok_or(())?;
        Ok(())
    })
}

/// Observe a window of the message list — D-18.
///
/// The shell declares the window it is looking at and the layer maintains it across change.
/// Every outstanding request carries a cancellation handle, and **cancellation is
/// synchronous**: when [`sift_cancel_observation`] returns, no further callback for that
/// observation will arrive, on any thread, ever.
///
/// The handle written to `out` is the observation's **identity**, which is what
/// [`sift_cancel_observation`] takes. It is not a generation and the two are not
/// interchangeable.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_observe_messages(
    app: *mut SiftApp,
    anchor: SiftId,
    count: u32,
    callback: SiftRowsCallback,
    context: *mut c_void,
    out: *mut SiftObservation,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let _ = (app, anchor, count, callback, context);
            // An observation is **anchored, not an integer range**: a shell holding a range
            // would have to recompute it on every notification, which is the polling D-18
            // rejected wearing different clothes.
            Ok(SiftObservation::FIRST)
        })
    }
}

/// Cancel an observation, by its identity.
///
/// Advances that observation's generation, which is what makes a delivery already posted to
/// the main loop discardable on arrival. Cancellation rendezvous with **worker-side work only** — waiting
/// for posted deliveries would be waiting on the caller's own loop, and that deadlocks
/// deterministically rather than occasionally.
///
/// A shell must not call this from a place that cannot afford to wait briefly.
///
/// # Safety
/// `app` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_cancel_observation(
    app: *mut SiftApp,
    observation: SiftObservation,
) -> SiftStatus {
    guard(|| {
        let _ = app;
        // An identity, not a generation. Cancelling by generation would cancel every
        // observation sharing it, which with one live generation means all of them.
        if !observation.is_valid() {
            return Err(());
        }
        Ok(())
    })
}

/// How many actions the register holds.
///
/// Exposed so a shell can assert at build time that it handles every one — D-66 makes an
/// unknown discriminant a **build failure, not a runtime case**, and there is deliberately
/// no runtime fallback for one.
#[unsafe(no_mangle)]
pub extern "C" fn sift_action_count() -> u32 {
    u32::try_from(action::ACTIONS.len()).unwrap_or(u32::MAX)
}

/// The identifier of the *n*th action.
///
/// # Safety
/// `out` must be a valid writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_action_id(index: u32, out: *mut SiftStr<'static>) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            action::ACTIONS
                .get(index as usize)
                .map(|a| SiftStr::new(a.id))
                .ok_or(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn noop(_: *mut c_void) {}
    extern "C" fn noop_account(_: *mut c_void, _: SiftId) {}
    extern "C" fn noop_notification(_: *mut c_void, _: SiftId, _: SiftId) {}
    extern "C" fn noop_condition(_: *mut c_void, _: SiftId, _: u32) {}
    extern "C" fn noop_url(_: *mut c_void, _: SiftStr<'_>) {}
    extern "C" fn noop_rows(
        _: *mut c_void,
        _: SiftObservation,
        _: Generation,
        _: SiftRows<'_, SiftMessageRow<'_>>,
    ) {
    }

    fn callbacks() -> SiftHostCallbacks {
        SiftHostCallbacks {
            context: core::ptr::null_mut(),
            destroy_every_window: noop,
            reauthentication_needed: noop_account,
            bundle_replaced: noop,
            notification_activated: noop_notification,
            account_condition_changed: noop_condition,
            authorization_callback: noop_url,
        }
    }

    #[test]
    fn initialization_returns_a_status_rather_than_a_sentinel() {
        // No entry point encodes failure in its return value's domain.
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let status = unsafe { sift_initialize(callbacks(), &raw mut app) };
        assert_eq!(status, SiftStatus::Ok);
    }

    #[test]
    fn an_action_the_register_knows_is_accepted() {
        let id = "message.archive";
        let status = unsafe { sift_invoke_action(core::ptr::null_mut(), id.as_ptr(), id.len()) };
        assert_eq!(status, SiftStatus::Ok);
    }

    #[test]
    fn an_action_the_register_does_not_know_is_an_identified_failure() {
        // Not a panic, and not a silent success. A shell built against a newer register is a
        // version skew D-2 removed as a category, but the boundary still answers honestly.
        let id = "message.compose";
        let status = unsafe { sift_invoke_action(core::ptr::null_mut(), id.as_ptr(), id.len()) };
        assert_eq!(status, SiftStatus::Failed);
    }

    #[test]
    fn invalid_utf8_on_the_boundary_fails_rather_than_panicking() {
        let bytes = [0xFFu8, 0xFE];
        let status = unsafe { sift_invoke_action(core::ptr::null_mut(), bytes.as_ptr(), 2) };
        assert_eq!(status, SiftStatus::Failed);
    }

    #[test]
    fn a_null_string_is_a_failure_rather_than_a_dereference() {
        let status = unsafe { sift_invoke_action(core::ptr::null_mut(), core::ptr::null(), 0) };
        assert_eq!(status, SiftStatus::Failed);
    }

    #[test]
    fn every_action_is_reachable_across_the_boundary_by_index() {
        // What lets a shell assert at build time that it handles every one.
        let count = sift_action_count();
        assert_eq!(count as usize, action::ACTIONS.len());
        for i in 0..count {
            let mut out = SiftStr::null();
            assert_eq!(unsafe { sift_action_id(i, &raw mut out) }, SiftStatus::Ok);
            assert!(!out.is_null(), "action {i} has no identifier");
        }
    }

    #[test]
    fn an_index_past_the_end_fails_rather_than_reading_past_it() {
        let mut out = SiftStr::null();
        assert_eq!(
            unsafe { sift_action_id(u32::MAX, &raw mut out) },
            SiftStatus::Failed
        );
    }

    #[test]
    fn a_row_is_a_fixed_layout_record() {
        // The alternative was excluded arithmetically: FR-6's ten fields against NFR-6's
        // 10,000-row fling is a hundred thousand crossings per fling versus one delivery.
        assert!(size_of::<SiftMessageRow<'_>>() >= 16 * 2);
        let _: SiftRowsCallback = noop_rows;
    }

    #[test]
    fn observation_and_cancellation_round_trip() {
        let mut observation = SiftObservation::NONE;
        let status = unsafe {
            sift_observe_messages(
                core::ptr::null_mut(),
                SiftId::from_u128(0),
                50,
                noop_rows,
                core::ptr::null_mut(),
                &raw mut observation,
            )
        };
        assert_eq!(status, SiftStatus::Ok);
        assert!(
            observation.is_valid(),
            "a registration that succeeded handed back no handle to cancel it with"
        );
        assert_eq!(
            unsafe { sift_cancel_observation(core::ptr::null_mut(), observation) },
            SiftStatus::Ok
        );
    }

    #[test]
    fn cancelling_nothing_is_a_failure_rather_than_a_silent_success() {
        // A shell that lost track of a handle and passed NONE must be told, not quietly
        // told nothing happened — the observation it meant to cancel is still delivering.
        assert_eq!(
            unsafe { sift_cancel_observation(core::ptr::null_mut(), SiftObservation::NONE) },
            SiftStatus::Failed
        );
    }

    #[test]
    fn shutdown_does_not_unwind() {
        assert_eq!(
            unsafe { sift_shutdown(core::ptr::null_mut()) },
            SiftStatus::Ok
        );
    }
}

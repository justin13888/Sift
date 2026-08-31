//! D-47 — no unwind crosses the C ABI.
//!
//! The release binary unwinds, deliberately: the rendering pipeline establishes a catch
//! boundary at each stage and degrades that message to FR-9's raw source view, which is a
//! guarantee abort-on-panic cannot offer. NFR-19 requires that hostile MIME never crash
//! the process, and under D-2 that process holds every account's sync state, the mutation
//! queue, and credential material — so aborting is not a conservative choice here, it is
//! the expensive one.
//!
//! But unwinding **across** an FFI boundary is undefined behaviour. So every exported
//! entry point terminates unwinding and returns a renderable failure. That is this module.
//!
//! # Why a caught panic is its own status
//!
//! D-47 requires a caught panic be **counted per subsystem** and **MUST NOT be silently
//! absorbed as an ordinary parse failure**. If [`SiftStatus`] had only success and
//! failure, every panic would arrive at the shell indistinguishable from a message that
//! was merely malformed — and the two need different responses: one is a message Sift
//! handled correctly, the other is a defect somebody has to see.

use crate::repr::SiftStatus;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Run an entry point's body, terminating any unwind at this boundary.
///
/// `body` returns `Ok(())` on success, or `Err(())` where the failure is one the design
/// anticipated — in which case the identified state is delivered separately, because **a
/// failure crossing this boundary is a state, never a message**.
///
/// # Out-parameters
///
/// A body that panics has not written its out-parameters, and the caller must not read
/// them. That is why out-parameters are written *last* in every entry point, after the
/// work that could fail: a partially written result under a `Panicked` status is a shell
/// reading half an answer.
pub fn guard<F>(body: F) -> SiftStatus
where
    F: FnOnce() -> Result<(), ()>,
{
    // AssertUnwindSafe: the boundary is not reentrant under D-48 and the layer owns every
    // value crossing it, so there is no shell-visible state a partially-completed body
    // could leave observably broken. What a panic *can* leave inconsistent is layer-owned
    // state, and that is the pipeline's problem rather than the boundary's — D-92 makes
    // each stage a pure function of its input precisely so a discarded stage discards
    // cleanly.
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => SiftStatus::Ok,
        Ok(Err(())) => SiftStatus::Failed,
        Err(_) => SiftStatus::Panicked,
    }
}

/// Run an entry point that produces a value, writing it through `out` only on success.
///
/// # Safety
///
/// `out` must be a valid, aligned, writable pointer to a `T` the caller owns, or null.
pub unsafe fn guard_out<T, F>(out: *mut T, body: F) -> SiftStatus
where
    F: FnOnce() -> Result<T, ()>,
{
    let mut produced = None;
    let status = guard(|| {
        produced = Some(body()?);
        Ok(())
    });
    if status == SiftStatus::Ok && !out.is_null() {
        if let Some(v) = produced {
            // SAFETY: the caller's obligation above. Written only after the body returned
            // successfully, so a panicking body leaves the caller's memory untouched.
            unsafe { out.write(v) };
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panicking on purpose prints a backtrace to stderr and makes a passing test look
    /// like a failing one. This silences the hook for the duration.
    fn quietly<R>(f: impl FnOnce() -> R) -> R {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = f();
        std::panic::set_hook(previous);
        r
    }

    #[test]
    fn success_is_ok() {
        assert_eq!(guard(|| Ok(())), SiftStatus::Ok);
    }

    #[test]
    fn an_anticipated_failure_is_failed() {
        assert_eq!(guard(|| Err(())), SiftStatus::Failed);
    }

    #[test]
    fn a_panic_becomes_a_status_rather_than_crossing() {
        // The whole of D-47's boundary obligation: this must not unwind out of `guard`.
        let status = quietly(|| guard(|| panic!("hostile input")));
        assert_eq!(status, SiftStatus::Panicked);
    }

    #[test]
    fn a_panic_is_distinguishable_from_an_ordinary_failure() {
        // D-47 forbids absorbing a caught panic as an ordinary parse failure. If these two
        // were the same value the shell could not tell a message Sift handled correctly
        // from a defect somebody has to see.
        let panicked = quietly(|| guard(|| panic!()));
        assert_ne!(panicked, guard(|| Err(())));
    }

    #[test]
    fn a_value_is_written_on_success() {
        let mut out: u32 = 0;
        let status = unsafe { guard_out(&raw mut out, || Ok(42u32)) };
        assert_eq!(status, SiftStatus::Ok);
        assert_eq!(out, 42);
    }

    #[test]
    fn a_panicking_body_leaves_the_out_parameter_untouched() {
        // A partially written result under a Panicked status is a shell reading half an
        // answer, so the write happens last or not at all.
        let mut out: u32 = 0xDEAD;
        let status =
            quietly(|| unsafe { guard_out(&raw mut out, || -> Result<u32, ()> { panic!() }) });
        assert_eq!(status, SiftStatus::Panicked);
        assert_eq!(
            out, 0xDEAD,
            "the caller's memory was written by a call that panicked"
        );
    }

    #[test]
    fn a_failing_body_leaves_the_out_parameter_untouched() {
        let mut out: u32 = 0xBEEF;
        let status = unsafe { guard_out(&raw mut out, || Err(())) };
        assert_eq!(status, SiftStatus::Failed);
        assert_eq!(out, 0xBEEF);
    }

    #[test]
    fn a_null_out_parameter_is_not_written() {
        let status = unsafe { guard_out(core::ptr::null_mut::<u32>(), || Ok(1u32)) };
        assert_eq!(status, SiftStatus::Ok);
    }

    #[test]
    fn a_panic_carrying_a_payload_still_does_not_cross() {
        // A panic payload that is not a string, and one that allocates, both have to be
        // absorbed rather than propagated.
        let s = quietly(|| guard(|| std::panic::panic_any(String::from("allocated"))));
        assert_eq!(s, SiftStatus::Panicked);
        let s = quietly(|| guard(|| std::panic::panic_any(7u64)));
        assert_eq!(s, SiftStatus::Panicked);
    }
}

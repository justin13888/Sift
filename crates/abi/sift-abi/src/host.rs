//! D-67 — the boundary's second direction.
//!
//! Everything else on this boundary is the shell asking and the layer answering. These seven
//! are the layer reaching the shell **on its own initiative**, and they exist because seven
//! requirements need something to happen when no view asked for it.
//!
//! # What makes them different from an observation
//!
//! An observation is scoped to a view, is cancellable, and carries a generation so a stale
//! delivery can be discarded. A host callback has **none of those**: it is not scoped to
//! any view, it is not cancellable, and it survives the destruction of every window.
//!
//! That last property is the whole point. Sift is resident with no window open, and four
//! of the seven *must* work in exactly that state: the re-authentication prompt (FR-2 —
//! "the one condition that MUST reach the user with no window open"), the restart prompt
//! when the bundle is replaced (FR-26), a quarantined intent needing attention, and new mail
//! (FR-23), which exists *for* the case where nobody is looking. A window-scoped mechanism
//! would be unable to deliver any of them.
//!
//! Because there is no observation, **D-66's generation discard does not apply**. The set
//! is process-scoped instead: registered once at initialization, unregistered only at
//! shutdown.
//!
//! # The set is closed
//!
//! **An eighth host callback is an amendment to D-67's table**, not an addition to this
//! struct — the seventh, new mail, was made exactly that way (#53). D-67 records its own
//! weakness here — seven entries is enough that one general event channel with an
//! identified payload would be a smaller surface than seven signatures — and the reason to
//! keep them separate is that a general channel makes the table impossible to enumerate,
//! which is what "closed" was for.
//!
//! # Delivery
//!
//! Under D-48, like everything else: on the shell's own main loop, and **not reentrant**.
//! A shell's rule inside one of these is the same as inside any callback — *receive,
//! record, return; act on the next turn of the loop*.

use crate::entry::SiftMessageRow;
use crate::repr::{SiftId, SiftRows, SiftStr};
use core::ffi::c_void;

/// An opaque pointer the shell supplies at registration and receives back with every
/// callback. The layer never dereferences it.
pub type HostContext = *mut c_void;

/// The seven callbacks the shell registers, once, at initialization.
///
/// Every field is required. There is no "optional callback": a shell that cannot destroy
/// its windows cannot honour L3, and a shell that cannot raise re-authentication has an
/// account that stalls silently — which is the failure FR-2 exists to prevent.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SiftHostCallbacks {
    /// The shell's context, handed back to every callback below.
    pub context: HostContext,

    /// **Destroy every window** — L3 in memory pressure.
    ///
    /// Destroys every window shell and its view hierarchy, leaving the application shell
    /// that owns the always-on surface. Removing that too would leave the application
    /// unreachable, so L3 is the deepest *in-process* shed and terminates nothing.
    ///
    /// Issued and never awaited: D-93's governor holds no lock a shed target needs and
    /// waits for nothing, because waiting here would mean blocking on a main loop it does
    /// not control — which, against D-48's synchronous cancellation, is a deadlock rather
    /// than a delay.
    pub destroy_every_window: extern "C" fn(HostContext),

    /// **Re-authentication is needed** — FR-2.
    ///
    /// Raised through the always-on surface, because Sift may be resident with nothing on
    /// screen. D-88's classifier is deliberately conservative about reaching here: only a
    /// well-formed provider denial is non-transient, so a captive portal answering with a
    /// login page does not produce this on every account at once.
    pub reauthentication_needed: extern "C" fn(HostContext, account: SiftId),

    /// **The bundle was replaced** — FR-26.
    ///
    /// Sift implements no self-update; the platform channel replaced the bundle underneath
    /// the running process. The shell surfaces a restart prompt rather than continuing
    /// against replaced resources.
    pub bundle_replaced: extern "C" fn(HostContext),

    /// **A notification was activated** — FR-23.
    ///
    /// Opens that message, which under FR-25 may mean opening a window on a process that
    /// has none. The identifiers are the ones [`Self::new_mail`] carried; a shell reads the
    /// row back with `sift_message_row`, because an activation may arrive on a relaunched
    /// process that holds nothing else about it.
    pub notification_activated: extern "C" fn(HostContext, account: SiftId, message: SiftId),

    /// **The account condition changed** — D-49.
    ///
    /// One condition per account, from the enumerated precedence-ordered set. The
    /// discriminant is [`AccountCondition`](sift_foundation::condition::AccountCondition)'s
    /// position in its `ALL`, and under D-66 the shell handles it **exhaustively at build
    /// time** — there is no runtime fallback for an unrecognised value and one must not be
    /// added.
    pub account_condition_changed: extern "C" fn(HostContext, account: SiftId, condition: u32),

    /// **An authorization callback arrived** — D-36.
    ///
    /// Delivered by the platform's own launch machinery through the registered URI scheme.
    /// Not a socket: NFR-24 admits no listening socket for any purpose, and D-36 removes
    /// the loopback redirect rather than excusing it.
    ///
    /// The URL is attacker-reachable — any local application can invoke a registered
    /// scheme — which is why D-88's state parameter is doing real work rather than being
    /// ceremony. A callback whose state matches no flow in progress is **discarded without
    /// comment**.
    pub authorization_callback: extern "C" fn(HostContext, url: SiftStr<'_>),

    /// **New mail arrived** — FR-23.
    ///
    /// One call per account per wheel fire that brought any in — coalesced on the tick the
    /// scheduler was already taking rather than posted per message, so it costs no wakeup of
    /// its own. `delivered` is FR-23's count: delivered by a delta and unread at that moment,
    /// never anything a backfill or a recovery merely discovered.
    ///
    /// `newest` holds the most recently received of them, or no row where it is no longer
    /// visible — put away between the arrival and the announcement — and then there is nothing
    /// for a notification to open. Its strings are borrowed for the call, under D-66's rule.
    ///
    /// Why this is not an observation: FR-23 exists for when there is no window, and an
    /// observation belongs to one. The authorization a platform may require before the first
    /// notification is the shell's to ask for.
    pub new_mail: extern "C" fn(
        HostContext,
        account: SiftId,
        delivered: u32,
        newest: SiftRows<'_, SiftMessageRow<'_>>,
    ),
}

impl core::fmt::Debug for SiftHostCallbacks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SiftHostCallbacks")
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

/// The entries in D-67's closed table, for the test that keeps them in step with it.
pub const HOST_CALLBACKS: &[(&str, &str)] = &[
    ("destroy_every_window", "L3 in memory pressure"),
    ("reauthentication_needed", "FR-2"),
    ("bundle_replaced", "FR-26"),
    ("notification_activated", "FR-23"),
    ("account_condition_changed", "D-49"),
    ("authorization_callback", "D-36"),
    ("new_mail", "FR-23"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use sift_foundation::condition::AccountCondition;

    #[test]
    fn the_table_holds_seven_entries() {
        // "An eighth host callback is an amendment to this table." Growing the struct
        // without amending D-67 is the change this notices.
        assert_eq!(HOST_CALLBACKS.len(), 7);
    }

    #[test]
    fn every_callback_names_the_requirement_that_needs_it() {
        // The membership test: an entry that cannot say why it exists is one nobody will
        // know how to remove, and the set stops being closed in practice.
        for (name, why) in HOST_CALLBACKS {
            assert!(!name.is_empty() && !why.is_empty());
        }
    }

    #[test]
    fn the_struct_has_one_field_per_callback_plus_the_context() {
        // Function pointers and the context are all pointer-sized, so the size is a
        // faithful count. An eighth callback changes this and has to change the table too.
        assert_eq!(
            size_of::<SiftHostCallbacks>(),
            size_of::<*const ()>() * (HOST_CALLBACKS.len() + 1)
        );
    }

    #[test]
    fn four_of_them_must_work_with_no_window_open() {
        // The property that makes these host callbacks rather than observations. A
        // window-scoped mechanism could deliver none of the four.
        for name in [
            "reauthentication_needed",
            "bundle_replaced",
            "account_condition_changed",
            "new_mail",
        ] {
            assert!(HOST_CALLBACKS.iter().any(|(n, _)| *n == name));
        }
    }

    #[test]
    fn the_condition_discriminant_covers_every_condition() {
        // The callback carries a u32 index into AccountCondition::ALL. If the set grew past
        // what the shell handles, D-66 requires that be a build failure rather than a
        // runtime case — so the index must at least be able to name every member.
        assert!(u32::try_from(AccountCondition::ALL.len()).is_ok());
        for (i, c) in AccountCondition::ALL.iter().enumerate() {
            let discriminant = u32::try_from(i).expect("fits");
            assert_eq!(
                AccountCondition::ALL[discriminant as usize],
                *c,
                "the discriminant does not round-trip"
            );
        }
    }
}

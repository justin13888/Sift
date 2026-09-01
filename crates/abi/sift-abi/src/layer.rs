//! The running layer behind [`SiftApp`](crate::entry::SiftApp), and the main-loop hop.
//!
//! # The hop nothing implemented
//!
//! D-48's first rule is that **"every observer callback is delivered on the shell's own main
//! loop. The layer performs the hop; the shell never does."** The reasoning is stated: both
//! toolkits require view mutation on their own loop, and D-19 chose a work-stealing runtime,
//! so a completion lands on an arbitrary worker. If the layer does not hop, both shells grow
//! a hand-rolled marshal and the two will not be the same marshal.
//!
//! Nothing implemented it. `SiftHostCallbacks` carries six callbacks and none of them is a
//! main-loop post, and there was no `pump`, `tick` or `drain` anywhere. A layer cannot hop
//! onto a loop it has no handle to, so the shell supplies one — **once, at initialization**,
//! beside the host callbacks it already supplies once.
//!
//! The shell's obligation is one line: when [`SiftSchedule`] is called, arrange for
//! [`sift_run_scheduled`] to be called with that ticket on the main loop, and return
//! immediately. On macOS that is `DispatchQueue.main.async`.
//!
//! # Why a ticket rather than a pointer
//!
//! The alternative is handing the shell a context pointer to call back with. A shell that
//! dropped one — a window closing between the post and the turn of the loop — would leak the
//! box behind it, and NFR-12 finds that in fourteen days. A ticket is an index into a slab
//! the layer owns, so a shutdown can reclaim every ticket the shell never ran, and a ticket
//! run twice or run after shutdown resolves to nothing instead of to freed memory.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use sift_session::Session;

use crate::host::SiftHostCallbacks;

/// Run a scheduled item. The shell calls this, on its main loop, with the ticket it was given.
pub type SiftRun = extern "C" fn(ticket: u64);

/// Arrange for `run(ticket)` to happen on the shell's main loop, and return immediately.
///
/// **It must not run it inline.** Running it inline would deliver a callback from inside the
/// call that produced it, which is the reentrancy D-48 forbids outright.
pub type SiftSchedule = extern "C" fn(context: *mut core::ffi::c_void, run: SiftRun, ticket: u64);

/// What the layer needs from the shell that the host callbacks do not carry.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SiftInit {
    /// The container the application's files live under — UTF-8, pointer and length.
    ///
    /// **The shell supplies it; the layer never computes one.** The container API is platform
    /// code, and D-1's economics rest on the core having no platform toolkit. Deriving a path
    /// from `$HOME` would also be wrong under both the macOS sandbox and Flatpak, and would
    /// give the test harness no way to ask for a scratch root.
    pub container_root: crate::repr::SiftStr<'static>,
    /// D-48's hop.
    pub schedule: SiftSchedule,
    pub schedule_context: *mut core::ffi::c_void,
    /// D-36 and D-71: whether the callback scheme is registered with the system.
    ///
    /// Checked **before** an authorization begins rather than after, because discovering it
    /// afterwards means the user has already been sent to a browser and returned to nothing.
    pub scheme_is_registered: u8,
}

/// The layer, as one process holds it.
pub(crate) struct Layer {
    pub(crate) session: Mutex<Session>,
    /// D-67's six, registered **once** and unregistered only at shutdown.
    ///
    /// Held from initialization rather than at first use because a host callback has no
    /// observation and therefore no generation to discard by — there is nothing to register
    /// it against later. They are invoked as the states they carry become computable; the
    /// set is closed and stored whole so that a shell cannot be asked for one it did not
    /// supply.
    #[allow(dead_code, reason = "D-67's set is stored whole at initialization")]
    pub(crate) host: SiftHostCallbacks,
    pub(crate) schedule: SiftSchedule,
    pub(crate) schedule_context: usize,
    /// Observation handle to the shell's callback for it.
    pub(crate) sinks: Mutex<BTreeMap<u64, Sink>>,
    /// Open documents, by token.
    ///
    /// A rendered body outlives the call that produced it: the shell holds the HTML while it
    /// renders and resolves resources against the token afterwards. So the strings live here,
    /// keyed on the token, and closing the document is the one gesture that both revokes the
    /// token and frees them — one lifetime rather than two that can disagree.
    pub(crate) documents: Mutex<BTreeMap<String, OpenDocument>>,
    /// Attachment listings, keyed on the message. Held for the same reason the document is:
    /// the rows are borrowed, so something has to own them past the call that returns them.
    pub(crate) attachments: Mutex<BTreeMap<u128, OpenAttachments>>,
    /// NFR-53's resolved save plans, by handle. A plan is *the* thing the user was shown, so
    /// writing takes the handle rather than a path — re-deriving a path at write time would
    /// let the written one differ from the shown one, which is the whole requirement.
    pub(crate) plans: Mutex<BTreeMap<u64, sift_app::attachment::SavePlan>>,
    pub(crate) next_plan: Mutex<u64>,
}

/// A message's attachment listing, and the rows borrowed from it.
#[derive(Debug)]
pub(crate) struct OpenAttachments {
    pub(crate) listed: Vec<sift_app::attachment::Attachment>,
    pub(crate) rows: Vec<crate::entry::SiftAttachment<'static>>,
}

/// The layer-owned text behind a [`SiftDocument`](crate::entry::SiftDocument).
///
/// The whole document rather than only its HTML, because the reader's chrome is drawn from
/// the parts *around* the body — what was withheld, where each link goes — and those are
/// borrowed row arrays under D-66. A row array has to point at something with a lifetime, and
/// the document's lifetime is the only one that is already correct: it ends at the revocation
/// that makes every address in it dead.
#[derive(Debug)]
pub(crate) struct OpenDocument {
    pub(crate) document: sift_app::document::Document,
    /// Row arrays are handed out as pointers into a contiguous slice, so the rows are built
    /// once when the document opens rather than per call — a per-call `Vec` would be freed
    /// before the shell read it.
    pub(crate) withheld: Vec<crate::entry::SiftWithheld<'static>>,
    pub(crate) links: Vec<crate::entry::SiftLink<'static>>,
}

/// Where one observation's batches go.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sink {
    pub(crate) callback: crate::entry::SiftRowsCallback,
    pub(crate) context: usize,
}

// SAFETY: every field is either owned outright or is an integer the layer never dereferences
// on its own. The context values are opaque to the layer — D-67 says so of the host context
// and the same is true of the schedule's — and the session is behind a mutex.
unsafe impl Send for Layer {}
unsafe impl Sync for Layer {}

/// The scheduled work a ticket names.
///
/// One variant today. It is an enum rather than a boxed closure because a closure would put
/// the shell's callback pointers behind a trait object whose lifetime is harder to reason
/// about than the thing it saves.
pub(crate) enum Task {
    /// Deliver whatever the session has to say.
    Deliver { layer: usize },
}

struct Slab {
    tasks: BTreeMap<u64, Task>,
    next: u64,
}

fn slab() -> &'static Mutex<Slab> {
    static SLAB: OnceLock<Mutex<Slab>> = OnceLock::new();
    SLAB.get_or_init(|| {
        Mutex::new(Slab {
            tasks: BTreeMap::new(),
            next: 1,
        })
    })
}

/// Put a task in the slab and hand the shell its ticket.
pub(crate) fn post(layer: &Layer, task: Task) {
    let ticket = {
        let Ok(mut slab) = slab().lock() else {
            // A poisoned slab means a previous delivery panicked while holding it. The
            // barrier turned that into a status rather than an unwind; dropping this
            // delivery is the honest consequence, and the observation stays registered so
            // the next signal tries again.
            return;
        };
        let ticket = slab.next;
        slab.next += 1;
        slab.tasks.insert(ticket, task);
        ticket
    };
    (layer.schedule)(
        layer.schedule_context as *mut core::ffi::c_void,
        crate::entry::sift_run_scheduled,
        ticket,
    );
}

/// Take a task out of the slab. A ticket run twice resolves to nothing the second time.
pub(crate) fn take(ticket: u64) -> Option<Task> {
    slab().lock().ok()?.tasks.remove(&ticket)
}

/// Discard every ticket the shell has not run.
///
/// D-70's teardown is bounded and waits for nothing, so a posted delivery that never came
/// back is reclaimed here rather than waited for.
pub(crate) fn abandon_all() {
    if let Ok(mut slab) = slab().lock() {
        slab.tasks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_resolves_once() {
        // A shell that ran a ticket twice — a double-posted block, a loop that drained
        // itself — must not get the delivery twice, and must not reach freed memory.
        let mut slab = slab().lock().expect("slab");
        let ticket = slab.next;
        slab.next += 1;
        slab.tasks.insert(ticket, Task::Deliver { layer: 0 });
        drop(slab);

        assert!(take(ticket).is_some());
        assert!(
            take(ticket).is_none(),
            "a ticket resolved twice, so a delivery would be handed over twice"
        );
    }

    #[test]
    fn a_ticket_that_was_never_issued_resolves_to_nothing() {
        assert!(take(u64::MAX).is_none());
    }
}

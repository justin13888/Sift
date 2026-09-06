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

/// Arm a coalescing platform timer, and call `run(ticket)` on the main loop when it fires.
///
/// **The leeway is the whole point.** D-25 rejects the async runtime's own timer precisely
/// because it cannot tell the kernel "this may fire late, batch it with something else", and
/// that hint is the entire mechanism by which wakeups coalesce. A shell that ignores
/// `leeway_millis` and arms an exact timer satisfies this signature and fails NFR-11.
///
/// On macOS this is a dispatch source timer with an explicit leeway; on Linux, an
/// absolute-mode timer file descriptor.
pub type SiftArmTimer = extern "C" fn(
    context: *mut core::ffi::c_void,
    run: SiftRun,
    ticket: u64,
    delay_millis: u64,
    leeway_millis: u64,
);

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
    /// D-25's platform timer, armed by the shell on the layer's behalf.
    ///
    /// It shares `schedule_context`: both are the same shell object, and a second context
    /// would be a second thing to keep alive for no gain. The layer computes *when* from its
    /// own wheel; the shell owns the one thing only it can do, which is asking the platform
    /// for a timer that is allowed to fire late.
    pub arm_timer: SiftArmTimer,
    /// The OAuth client this bundle was configured with — empty where there is none.
    ///
    /// **Configuration, not a secret.** A public client's identifier appears in every
    /// authorization URL it generates, which is why PKCE exists; D-88 forbids an embedded
    /// secret outright. It is stated once, here, rather than repeated at every call, because
    /// the layer needs it for three things a shell should not be answering separately.
    pub oauth_client_id: crate::repr::SiftStr<'static>,
    /// Every URI scheme this shell's bundle claims, separated by newlines — D-36 and D-109.
    ///
    /// **A fact, not a conclusion, and the difference is the bug this replaced.** The macOS
    /// shell used to pass a boolean saying the scheme was registered, hardcoded to true beside
    /// a comment asserting the Info.plist did it. A configuration shipped without the derived
    /// scheme, the layer was told otherwise, D-71's refusal could not fire, and the failure
    /// surfaced as a browser page after the user had granted consent.
    ///
    /// A bundle is what registers a scheme, so only a shell can report this. Deciding what it
    /// *means* — which scheme this client requires, and whether it is among them — is the
    /// layer's, where the derivation already lives and where one rule serves both shells.
    ///
    /// A URI scheme cannot contain a newline, so this needs no escaping.
    pub registered_schemes: crate::repr::SiftStr<'static>,
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
    pub(crate) host: SiftHostCallbacks,
    pub(crate) schedule: SiftSchedule,
    pub(crate) schedule_context: usize,
    /// D-25's platform timer. Shares `schedule_context`.
    pub(crate) arm_timer: SiftArmTimer,
    /// Whether a platform timer is outstanding.
    ///
    /// **One at a time, or an account added is a wakeup added.** Every path that changes what
    /// is armed — initialization, adding an account, opening a container — has to make sure a
    /// timer exists, and without this each of them would arm its own. Five accounts added in
    /// one session would then be five timers firing a second apart, which is precisely the
    /// per-account sleep loop the wheel exists to replace.
    pub(crate) timer_pending: std::sync::atomic::AtomicBool,
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
    /// The authorization in progress, if any.
    ///
    /// **The layer holds it, not the shell.** PKCE binds the exchange to the process that
    /// started the flow, and a shell holding the client identifier and the address would be a
    /// shell that could be asked to complete a flow it did not begin.
    pub(crate) flows: Mutex<Flow>,
    /// FR-34's tables. Held because their rows are borrowed, and replaced by each call rather
    /// than accumulated — a debug panel that grew its own memory would be measuring itself.
    pub(crate) queue_rows: Mutex<Vec<crate::entry::SiftQueued<'static>>>,
    pub(crate) memory_rows: Mutex<Vec<crate::entry::SiftSubsystemBytes<'static>>>,
    pub(crate) stage_rows: Mutex<Vec<crate::repr::SiftStr<'static>>>,
    pub(crate) setting_rows: Mutex<Vec<crate::entry::SiftSetting<'static>>>,
    /// The account list, and the text it borrows.
    ///
    /// **A shell cannot name an account without this.** Every other account-taking entry point
    /// takes a label, and until now nothing across the boundary said what the labels were —
    /// so a shell could add an account and then never sync, pause, authorize or flush it
    /// again, and the runtime panel asked the user to type one in.
    pub(crate) account_rows: Mutex<Vec<crate::entry::SiftAccount<'static>>>,
    /// Each account's label and kind, paired. Paired rather than concatenated so that an
    /// index cannot mean one account's name and another's kind.
    pub(crate) account_names: Mutex<Vec<(String, String)>>,
    /// The last account setting read back, held for the string handed out.
    pub(crate) account_setting_value: Mutex<String>,
    /// D-49's condition, per account, as the shell was last told it.
    ///
    /// **What makes the push a push.** The condition is computable at any moment and a shell
    /// can poll it, but FR-2's re-authentication has to reach a user with no window open —
    /// and a window-less process has nothing to poll from. So the layer compares against what
    /// it last said and calls the host callback on a change, which is also what keeps the
    /// callback from firing on every delivery with the same answer.
    pub(crate) conditions: Mutex<BTreeMap<u128, u32>>,
    /// The text the setting rows borrow. Held separately because the rows are `repr(C)` and
    /// cannot own a `String`.
    pub(crate) setting_values: Mutex<Vec<String>>,
    /// The last search. Held for the same reason every other table here is: the rows are
    /// borrowed, and something has to own them past the call that returns them.
    pub(crate) search: Mutex<SearchResult>,
}

/// One search's results, and the text they borrow.
#[derive(Debug, Default)]
pub(crate) struct SearchResult {
    /// The rows themselves, owned here. The `repr(C)` records below borrow from these, and
    /// something has to hold them past the call that returns them.
    pub(crate) owned: Vec<sift_app::rows::MessageRow>,
    pub(crate) rows: Vec<crate::entry::SiftMessageRow<'static>>,
    /// In step with `rows`. Kept beside them rather than in them so that a result row is
    /// byte-identical to a list row and a shell draws both with one code path.
    pub(crate) sources: Vec<u32>,
    pub(crate) interpretation: String,
    pub(crate) caveats: String,
}

/// One authorization in progress. Empty means none.
#[derive(Debug, Default)]
pub(crate) struct Flow {
    pub(crate) client_id: String,
    pub(crate) url: String,
    /// The callback scheme last asked for, kept alive for the string handed back.
    ///
    /// Separate from a flow's own lifetime on purpose: a shell asks for this *before* it
    /// begins anything, to find out whether it can receive a callback at all.
    pub(crate) scheme: String,
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
    /// A wheel fire came due. Do its work, then re-arm for the next one.
    Tick { layer: usize },
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

/// Arm the platform timer for the layer's next wheel fire.
///
/// **Nothing armed means no timer at all.** A resident process with no work outstanding must
/// take no wakeups, so an absent deadline arms nothing rather than arming a poll that would
/// wake the machine to discover there was nothing to do.
pub(crate) fn ensure_timer(layer: &Layer) {
    use std::sync::atomic::Ordering;
    // **Claimed with a compare-exchange, not a load then a store.** Two entry points can add
    // an account at once — nothing in this ABI confines callers to one thread, and `Layer` is
    // `Sync` — and a load-then-store lets both observe `false` and both arm. The doubling is
    // permanent, because each fire re-arms its own successor, so a race that happened once
    // spends a wakeup a minute forever against a budget of two.
    if layer
        .timer_pending
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // One is already outstanding, and the fire it belongs to re-arms from the wheel as it
        // finds it then. Arming a second here would add a wakeup to say nothing new.
        return;
    }
    let next = {
        match layer.session.lock() {
            Ok(session) => session.app().next_wake(),
            // Release the claim. Holding it would leave the flag true with no timer behind
            // it, and nothing would ever arm one again.
            Err(_) => {
                layer.timer_pending.store(false, Ordering::SeqCst);
                return;
            }
        }
    };
    match next {
        Some(delay) => post_timer(layer, delay),
        // Nothing to schedule. The claim is released so that the next account added can take
        // it — a resident process with no work takes no wakeups, and must still be able to
        // start taking them.
        None => layer.timer_pending.store(false, Ordering::SeqCst),
    }
}

/// Arm the platform timer. The caller has already claimed `timer_pending`.
pub(crate) fn post_timer(layer: &Layer, delay: core::time::Duration) {
    let ticket = {
        let Ok(mut slab) = slab().lock() else {
            layer
                .timer_pending
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        let ticket = slab.next;
        slab.next += 1;
        slab.tasks.insert(
            ticket,
            Task::Tick {
                layer: core::ptr::from_ref(layer) as usize,
            },
        );
        ticket
    };
    // A poisoned slab is the one path that reaches here with nothing armed, and it must not
    // leave the claim standing — see the release in `ensure_timer` for why.
    (layer.arm_timer)(
        layer.schedule_context as *mut core::ffi::c_void,
        crate::entry::sift_run_scheduled,
        ticket,
        u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
        u64::try_from(sift_foundation::limits::L31_WHEEL_SLACK.as_millis()).unwrap_or(u64::MAX),
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

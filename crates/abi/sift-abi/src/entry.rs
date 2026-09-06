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
use crate::layer::{Layer, OpenAttachments, OpenDocument, SiftInit, Sink, Task};
use crate::repr::{Generation, SiftId, SiftObservation, SiftRows, SiftStatus, SiftStr};
use core::ffi::c_void;
use sift_presentation::action;
use sift_session::{Session, Watching};

/// An opaque handle to the running layer.
///
/// The shell holds it and passes it back. It never dereferences it — which is what lets the
/// layer change shape without changing the boundary.
#[derive(Debug)]
#[repr(C)]
pub struct SiftApp {
    _private: [u8; 0],
}

/// Borrow the layer behind a handle the shell gave back.
///
/// # Safety
/// `app` must be a pointer this crate handed out from `sift_initialize` and has not since
/// torn down.
unsafe fn layer<'a>(app: *mut SiftApp) -> Option<&'a Layer> {
    if app.is_null() {
        return None;
    }
    // SAFETY: the caller's obligation, stated on every entry point that takes one.
    Some(unsafe { &*app.cast::<Layer>() })
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
    init: SiftInit,
    out: *mut *mut SiftApp,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            // The container is the shell's to name and is not optional. A layer that fell
            // back to a path of its own would be wrong under the sandbox and under Flatpak,
            // and would give the harness no way to ask for a scratch root.
            let root = init.container_root.as_str().ok_or(())?;
            if root.is_empty() {
                return Err(());
            }
            let mut app = sift_app::App::new();
            // The container, and everything the last run left in it: the accounts, sealed
            // under their own keys, with each queue rebuilt from its journal.
            //
            // A failure here is a **refused launch** rather than a degraded one. The
            // alternatives are all worse than not starting: an empty application over a
            // container that is there would look like the user's mail had gone, and a
            // scratch root beside it would quietly start a second installation. D-71's rule
            // is the same shape — a security guarantee that is absent refuses.
            //
            // The tests run without a credential store, which on a machine with one would
            // prompt. `SIFT_EPHEMERAL` is what they set; a shell never does.
            //
            // **It ignores the root it was given rather than sharing it.** An ephemeral
            // session writing into the real container collides with what is already there —
            // an identity that is taken, a file that is sealed under a key this session does
            // not have — and the collision surfaces as an account that silently fails to be
            // added and a first-run screen where a mailbox should be. Which is how it was
            // found.
            // D-36, D-71 and D-109 in one place, and deliberately *here* rather than in the
            // shell that reported the facts. The shell says which client it was configured
            // with and which schemes its bundle claims; which scheme this client requires is
            // the derivation in `sift-foundation`, and whether it is among them is arithmetic.
            // A shell has nothing left to assert, which is what stops the previous failure —
            // a hardcoded `true` beside a bundle that registered no such scheme — recurring.
            let client_id = init.oauth_client_id.as_str().unwrap_or("");
            app.oauth_client_id = client_id.to_owned();
            let required = sift_foundation::identifiers::callback_scheme_for(client_id);
            app.scheme_is_registered = !client_id.is_empty()
                && init
                    .registered_schemes
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .any(|claimed| claimed.trim() == required);
            if std::env::var_os("SIFT_EPHEMERAL").is_none() {
                app.open_container(std::path::Path::new(root))
                    .map_err(|_| ())?;
            } else {
                // A counter as well as the clock. `as_nanos` reports at whatever resolution
                // the platform has, and two initializations in one process can and do read
                // the same value — which gives two sessions one directory, two accounts one
                // set of files, and a failure about one run in ten. The same mistake was
                // made and fixed in the test scratch path; the clock is here for readable
                // names and the counter is what makes them unique.
                use std::sync::atomic::{AtomicU64, Ordering};
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let unique = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                let scratch = std::env::temp_dir().join(format!(
                    "sift-ephemeral-{}-{unique}-{n}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&scratch).map_err(|_| ())?;
                app.root = Some(scratch);
            }

            let layer = Box::new(Layer {
                session: std::sync::Mutex::new(Session::new(app)),
                host: callbacks,
                schedule: init.schedule,
                schedule_context: init.schedule_context as usize,
                sinks: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                documents: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                attachments: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                plans: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                next_plan: std::sync::Mutex::new(1),
                flows: std::sync::Mutex::new(crate::layer::Flow::default()),
                queue_rows: std::sync::Mutex::new(Vec::new()),
                memory_rows: std::sync::Mutex::new(Vec::new()),
                stage_rows: std::sync::Mutex::new(Vec::new()),
                setting_rows: std::sync::Mutex::new(Vec::new()),
                setting_values: std::sync::Mutex::new(Vec::new()),
                account_rows: std::sync::Mutex::new(Vec::new()),
                account_names: std::sync::Mutex::new(Vec::new()),
                account_setting_value: std::sync::Mutex::new(String::new()),
                conditions: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                search: std::sync::Mutex::new(crate::layer::SearchResult::default()),
            });
            Ok(Box::into_raw(layer).cast::<SiftApp>())
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
        if app.is_null() {
            // Tearing down nothing is not a failure. A shell that lost its handle during a
            // failed launch still has to be able to quit.
            return Ok(());
        }
        // Every ticket the shell never ran, reclaimed rather than waited for. D-70's
        // teardown is bounded and flushes nothing.
        crate::layer::abandon_all();
        // SAFETY: the caller's obligation — the pointer came from `sift_initialize` and is
        // not used again.
        drop(unsafe { Box::from_raw(app.cast::<Layer>()) });
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
    parameter: *const u8,
    parameter_len: usize,
    confirmed: u8,
    out: *mut SiftGesture,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let name = borrowed(id, id_len)?;
            // An identifier the register does not know is an identified failure rather than a
            // panic: a shell built against a newer register is a version skew D-2 removed as
            // a category, but the boundary still answers honestly rather than aborting.
            action::by_id(name).ok_or(())?;
            let parameter = if parameter.is_null() {
                None
            } else {
                Some(borrowed(parameter, parameter_len)?)
            };
            let layer = layer(app).ok_or(())?;

            let gesture = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session
                    .invoke(name, parameter, confirmed != 0, now_millis())
                    .map_err(|_| ())?
            };

            // Anything that could have changed a window is followed by a delivery, posted
            // rather than run: running it here would hand the shell a callback from inside
            // the call that caused it, which is the reentrancy D-48 forbids.
            crate::layer::post(
                layer,
                Task::Deliver {
                    layer: app as usize,
                },
            );
            Ok(SiftGesture {
                mutated: u8::from(gesture.mutates),
                enqueued: u32::try_from(gesture.enqueued.len()).unwrap_or(u32::MAX),
                skipped: u32::try_from(gesture.skipped.len()).unwrap_or(u32::MAX),
                optimistic: u8::from(gesture.optimistic),
            })
        })
    }
}

/// What one gesture did, as the shell needs to know it.
///
/// The undo affordance keys on `enqueued`: a gesture that enqueued nothing has nothing to take
/// back, and offering undo for it would be a control that does nothing.
#[derive(Debug)]
#[repr(C)]
pub struct SiftGesture {
    /// Zero for a navigation or a surface. **Not a failure** — reporting a navigation as
    /// "0 enqueued" would read as one.
    pub mutated: u8,
    pub enqueued: u32,
    /// Messages whose account is gone. The rest of a bulk gesture is still the user's, so
    /// these are skipped rather than fatal.
    pub skipped: u32,
    /// Whether the overlay hides it before any round trip — NFR-7's 16 ms.
    pub optimistic: u8,
}

/// What could be undone right now.
///
/// D-86 puts this record **in the layer**, and this is why it can be: a window shell is
/// destroyed when its window closes, in a product that runs with no window at all, so a
/// countdown owned by a view dies with the view. The always-on surface reads the same record
/// through the same entry point.
///
/// `timed` distinguishes FR-15's countdown from ordinary reversibility. Every intent but
/// permanent delete stays reversible for as long as the message exists; only intents that
/// remove the message from view get a *window*, because those are the ones where the user has
/// nothing left to click. A countdown on every message the reader marks read would make the
/// mechanism worthless by making it constant.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_undoable(app: *mut SiftApp, out: *mut SiftUndoable) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let session = layer.session.lock().map_err(|_| ())?;
            let record = session.undoable().ok_or(())?;
            let now = now_millis();
            Ok(SiftUndoable {
                messages: u32::try_from(record.messages).unwrap_or(u32::MAX),
                timed: u8::from(record.timed),
                remaining_millis: if record.timed {
                    record.remaining_millis(now)
                } else {
                    0
                },
                intent: SiftStr::new(record.intent),
            })
        })
    }
}

/// The gesture that undo would reverse.
#[derive(Debug)]
#[repr(C)]
pub struct SiftUndoable<'a> {
    pub messages: u32,
    /// Whether FR-15's countdown applies. Where this is zero the gesture is still reversible
    /// through the ordinary interface — there is simply no toast.
    pub timed: u8,
    /// What is left of L-22. Zero once the countdown has run out, which does not mean the
    /// gesture became irreversible.
    pub remaining_millis: u64,
    /// What was done, for the affordance's own words. A `'static` name from the register,
    /// so unlike every other borrowed string here it outlives any document.
    pub intent: SiftStr<'a>,
}

/// How many accounts this installation has — including the ones a previous run added.
///
/// **A shell asks rather than counting what it has seen.** The macOS shell kept a counter that
/// began at zero every launch, so every launch took the account-less branch and offered to add
/// an account the user had already added: the container had the mail, sealed, with its queue
/// rebuilt from its journal, and the first-run screen was drawn over it.
///
/// Zero is the genuine account-less state, and it is what makes the add-account flow the right
/// thing to show rather than an empty inbox.
///
/// # Safety
/// `app` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_account_count(app: *mut SiftApp) -> u32 {
    unsafe {
        let Some(layer) = layer(app) else { return 0 };
        let Ok(session) = layer.session.lock() else {
            return 0;
        };
        u32::try_from(session.app().accounts().count()).unwrap_or(u32::MAX)
    }
}

/// One account, as a shell needs to see it.
///
/// **The label is the handle.** Every account-taking entry point across this boundary names an
/// account by the label it was added under, and until this existed nothing said what those
/// labels were — so a shell could add an account and then never reach it again, and the
/// runtime panel had to ask the user to type one. D-89 makes the identity Sift's own; the
/// label is what a person calls it and what the container recorded.
#[derive(Debug)]
#[repr(C)]
pub struct SiftAccount<'a> {
    /// D-89's Sift-assigned identity — the anchor a message-list observation takes.
    pub id: SiftId,
    /// What the account was added as, and what every other entry point takes.
    pub name: SiftStr<'a>,
    /// What the container recorded so a later run knows how to reconnect it. **Not something
    /// to branch on**: the provider model plans against declared capabilities, and this is a
    /// name for a reconnection route rather than a provider a shell may reason about.
    pub kind: SiftStr<'a>,
    /// D-49's single condition for this account.
    pub condition: SiftCondition,
    /// Whether Sift may change this mailbox. An account is added watching and nothing else,
    /// and this is the flag that says so.
    pub writes_enabled: u8,
    /// Intents recorded and held because writes are not authorized. Zero once they are.
    pub held: u32,
}

/// Every account the container holds.
///
/// The rows are borrowed for the duration of the call, like every other row array here, and
/// the text behind them lives in the layer until the next call replaces it.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_accounts(
    app: *mut SiftApp,
    out: *mut SiftRows<'static, SiftAccount<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let mut session = layer.session.lock().map_err(|_| ())?;

            let names: Vec<String> = session
                .app()
                .accounts()
                .map(|(name, _)| name.clone())
                .collect();
            // One pass, and the condition first, because computing it needs the application
            // mutably and a borrow of the account cannot be alive across that call.
            let mut gathered: Vec<Gathered> = Vec::with_capacity(names.len());
            for name in &names {
                let condition = session
                    .app_mut()
                    .condition_of(name)
                    .map_or(SiftCondition::HEALTHY, SiftCondition::of);
                let app = session.app();
                let held = u32::try_from(app.held(name)).unwrap_or(u32::MAX);
                let Some((_, account)) = app.accounts().find(|(n, _)| *n == name) else {
                    continue;
                };
                gathered.push(Gathered {
                    id: account.id.as_u128(),
                    name: name.clone(),
                    kind: account.kind.clone(),
                    writes_enabled: account.writes_enabled,
                    held,
                    condition,
                });
            }
            drop(session);

            // The names and the kinds, in one vector the layer owns, because a `repr(C)` row
            // cannot own a `String` and the temporaries above die at the end of this call.
            //
            // **Pairs rather than two runs in one vector.** The kind of row *i* lived at
            // `names.len() + i` and was read at `gathered.len() + i`; the two agree only while
            // no account is skipped above, and a skip would have been silent — every index
            // stays in bounds, so each row would have been handed a name and a kind belonging
            // to different accounts. A pair cannot be indexed apart.
            let mut stored = layer.account_names.lock().map_err(|_| ())?;
            *stored = gathered
                .iter()
                .map(|g| (g.name.clone(), g.kind.clone()))
                .collect();
            let mut table = layer.account_rows.lock().map_err(|_| ())?;
            *table = gathered
                .iter()
                .zip(stored.iter())
                .map(|(g, (name, kind))| SiftAccount {
                    id: SiftId::from_u128(g.id),
                    name: SiftStr::new(extend(name)),
                    kind: SiftStr::new(extend(kind)),
                    condition: g.condition,
                    writes_enabled: u8::from(g.writes_enabled),
                    held: g.held,
                })
                .collect();
            Ok(SiftRows::new(extend_rows(&table)))
        })
    }
}

/// One account's facts, read while the session is held and used after it is released.
struct Gathered {
    id: u128,
    name: String,
    kind: String,
    writes_enabled: bool,
    held: u32,
    condition: SiftCondition,
}

/// Authorize, or withdraw authorization for, writes to one account.
///
/// **An account is added watching and nothing else**, and this is the only thing that changes
/// it. Until it existed, triage on a macOS account was journaled durably, applied
/// optimistically, and could never be issued — the posture was settable from the test harness
/// and from nowhere a person could reach.
///
/// Withdrawing takes effect immediately for anything not yet issued. Intents already on the
/// wire are not recalled: a request that has left cannot be unsent, and pretending otherwise
/// is the one lie a mutation queue must not tell.
///
/// # Safety
/// `app` must be valid; `label` must point to `label_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_set_writes_enabled(
    app: *mut SiftApp,
    label: *const u8,
    label_len: usize,
    enabled: u8,
) -> SiftStatus {
    guard(|| {
        // SAFETY: the caller's obligation.
        let name = unsafe { borrowed(label, label_len)? };
        // SAFETY: as above.
        let layer = (unsafe { layer(app) }).ok_or(())?;
        let mut session = layer.session.lock().map_err(|_| ())?;
        session
            .app_mut()
            .set_writes_enabled(name, enabled != 0)
            .map_err(|_| ())
    })
}

/// What one turn of the flush did.
///
/// **`authorized` is not a failure.** An account that is only being watched has a queue that
/// grows and sends nothing, and that is the state the user chose — so it crosses as a result
/// with a count in it rather than as an error, which is what lets a surface say *nothing has
/// been sent, and nothing will be until you say so* instead of drawing a fault.
#[derive(Debug)]
#[repr(C)]
pub struct SiftFlush {
    /// Zero where writes are not authorized for this account. Nothing was issued.
    pub authorized: u8,
    /// Intents held because writes are not authorized. Zero once they are.
    pub held: u32,
    pub issued: u32,
    pub applied: u32,
    /// The provider refused, in its own terms. Settled: retrying changes nothing.
    pub refused: u32,
    /// Left for the scheduler to try again.
    pub deferred: u32,
    /// The request went out and no answer came back — D-85's `Reconciling`.
    pub reconciling: u32,
    /// Held rather than executed: unrecognised, or its gating capability has gone away.
    pub quarantined: u32,
    /// What is still queued afterwards.
    pub queued: u32,
    /// Whether the flush ended in a stated failure. **The state, not the sentence** — D-56
    /// keeps prose on the shell's side of this boundary.
    pub failed: u8,
}

/// Send what is queued for one account, once.
///
/// **This blocks the calling thread**, which is a limitation rather than a design, and the same
/// one [`sift_sync_account`] carries: the work belongs on a worker under D-19, and moving it
/// there changes nothing a shell can see because every delivery already arrives through D-48's
/// hop rather than out of this call.
///
/// # Safety
/// `app` and `out` must be valid; `label` must point to `label_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_flush_account(
    app: *mut SiftApp,
    label: *const u8,
    label_len: usize,
    out: *mut SiftFlush,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let name = borrowed(label, label_len)?;
            let layer = layer(app).ok_or(())?;
            let flushed = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session.app_mut().flush(name).map_err(|_| ())?
            };
            // The list is an observation, and a settled intent removes the overlay row that was
            // hiding a message. Posted rather than run: running it here would hand the shell a
            // callback from inside the call that caused it, which is D-48's reentrancy.
            crate::layer::post(
                layer,
                Task::Deliver {
                    layer: app as usize,
                },
            );
            let report = &flushed.report;
            let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            Ok(SiftFlush {
                authorized: u8::from(flushed.authorized),
                held: count(flushed.held),
                issued: count(report.issued),
                applied: count(report.applied),
                refused: count(report.refused),
                deferred: count(report.deferred),
                reconciling: count(report.reconciling),
                quarantined: count(report.quarantined),
                queued: count(flushed.queued),
                failed: u8::from(flushed.error.is_some()),
            })
        })
    }
}

/// FR-15 — reverse the last reversible gesture.
///
/// **Not an action.** `undo.last-gesture` is in D-98's register and has no intent behind it, so
/// invoking it through [`sift_invoke_action`] returns success and does nothing — which is what
/// the undo toast was wired to. The reversal is a gesture of its own shape: it acts over
/// D-85's undo group rather than over a message, so a bulk operation reverses as the one
/// gesture FR-17 promises.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_undo_last(app: *mut SiftApp, out: *mut SiftGesture) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let gesture = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session.undo_last(now_millis()).map_err(|_| ())?
            };
            crate::layer::post(
                layer,
                Task::Deliver {
                    layer: app as usize,
                },
            );
            Ok(SiftGesture {
                mutated: u8::from(gesture.mutates),
                enqueued: u32::try_from(gesture.enqueued.len()).unwrap_or(u32::MAX),
                skipped: u32::try_from(gesture.skipped.len()).unwrap_or(u32::MAX),
                optimistic: u8::from(gesture.optimistic),
            })
        })
    }
}

/// The URI scheme this client's authorization callback comes back on — D-36 and D-109.
///
/// # Why a shell asks rather than derives
///
/// It is one rule, and it is not the obvious one: a provider that lets an application name its
/// own redirect gets Sift's scheme, and one that does not — Google's iOS/macOS client type is
/// the case that forced this — accepts exactly one, the client identifier reversed. D-17 exists
/// to stop two shells growing two answers to a question like that, so the derivation stays in
/// `sift-foundation` and this is how a shell reaches it.
///
/// It is derived from the client this installation was configured with, which the layer was
/// given at initialization — so a shell that asks this and a flow that declares a redirect
/// cannot answer differently.
///
/// A shell needs it to tell the platform which scheme a callback will arrive on. It is empty
/// where no client is configured, and a shell must not begin an authorization in that case.
///
/// The string lives in the layer until the next call that asks for one.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_callback_scheme(
    app: *mut SiftApp,
    out: *mut SiftStr<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let client_id = {
                let session = layer.session.lock().map_err(|_| ())?;
                session.app().oauth_client_id.clone()
            };
            let mut flows = layer.flows.lock().map_err(|_| ())?;
            flows.scheme = if client_id.is_empty() {
                String::new()
            } else {
                sift_foundation::identifiers::callback_scheme_for(&client_id)
            };
            Ok(SiftStr::new(extend(&flows.scheme)))
        })
    }
}

/// D-36 — begin an authorization, and hand back the address to open in a browser.
///
/// **The scheme registration is checked before the user goes anywhere.** Discovering it
/// afterwards means they have already granted consent and returned to nothing, and the
/// resulting page is a browser error rather than anything Sift can explain.
///
/// The address is held by the layer until the flow completes or another begins, because the
/// verifier behind it is: PKCE binds the exchange to the process that started it, and a shell
/// holding the state would be a shell that could be asked to complete a flow it did not begin.
///
/// The client is the one this installation was configured with, stated at initialization. It
/// is not a parameter because it was one: a shell repeating it at every call is a shell that
/// can disagree with the bundle it is running out of.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_begin_authorization(
    app: *mut SiftApp,
    out: *mut SiftStr<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let (client_id, url) = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                let app = session.app_mut();
                let client_id = app.oauth_client_id.clone();
                if client_id.is_empty() {
                    return Err(());
                }
                let registered = app.scheme_is_registered;
                let url = sift_app::authorize::begin(
                    &mut app.broker,
                    &client_id,
                    registered,
                    now_millis(),
                )
                .map_err(|_| ())?;
                (client_id, url)
            };
            let mut flows = layer.flows.lock().map_err(|_| ())?;
            flows.client_id = client_id;
            flows.url = url;
            Ok(SiftStr::new(extend(&flows.url)))
        })
    }
}

/// Finish an authorization from the address the system handed back, and add the account.
///
/// The callback arrives through the registered URI scheme — **not a socket**, because NFR-24
/// admits none for any purpose. It is also one of only two local attack surfaces Sift has, so
/// a callback whose state matches no flow in progress is discarded without comment: D-88's
/// state parameter is doing real work here rather than being ceremony.
///
/// # Safety
/// `app` and `out` must be valid; both strings must point to their lengths in UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_complete_authorization(
    app: *mut SiftApp,
    callback: *const u8,
    callback_len: usize,
    display_name: *const u8,
    display_name_len: usize,
    out: *mut SiftId,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let callback = borrowed(callback, callback_len)?;
            let display_name = borrowed(display_name, display_name_len)?;
            let layer = layer(app).ok_or(())?;
            let client_id = {
                let flows = layer.flows.lock().map_err(|_| ())?;
                if flows.client_id.is_empty() {
                    return Err(());
                }
                flows.client_id.clone()
            };

            let mut session = layer.session.lock().map_err(|_| ())?;
            let app_ref = session.app_mut();
            let identity = app_ref.reserve_identity();
            let adapter = sift_app::authorize::complete(
                &mut app_ref.broker,
                &client_id,
                identity,
                callback,
                now_millis(),
            )
            .map_err(|_| ())?;
            let id = app_ref
                .add_provider_account(display_name, adapter)
                .map_err(|_| ())?;
            // The flow is spent. A verifier that outlived its exchange would be one a second
            // callback could be replayed against.
            let mut flows = layer.flows.lock().map_err(|_| ())?;
            flows.client_id.clear();
            flows.url.clear();
            Ok(SiftId::from_u128(id.as_u128()))
        })
    }
}

/// FR-19, FR-20 and FR-21 — search, with the interpretation the user is shown.
///
/// `account` narrows it to one account, and zero is every account — the same anchor
/// [`sift_observe_messages`] takes, so a window that is looking at one mailbox can search the
/// one it is looking at. FR-20's *narrow to this account* is a scope rather than a query term:
/// spelling it as an operator would mean parsing, translating and explaining a word for
/// something the shell already knows.
///
/// **The interpretation crosses the boundary as a result, not as a debug aid.** A query that
/// found nothing and one that was misread look identical from the results alone, and
/// `form:alice` is a plausible typo for `from:alice`. So is the caveat list: empty results and
/// unsearched fields also look identical, and a person who searches `has:attachment`, gets
/// nothing, and concludes they have no attachments has been misled by a filter that was never
/// evaluated.
///
/// # Safety
/// `app` and `out` must be valid; the strings must point to their lengths in UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_search(
    app: *mut SiftApp,
    query: *const u8,
    query_len: usize,
    account: SiftId,
    limit: u32,
    out: *mut SiftSearch<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let query = borrowed(query, query_len)?;
            let layer = layer(app).ok_or(())?;
            let report = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                // The same anchor a message-list observation takes, and zero means the same
                // thing: D-4's unified stream rather than an account nobody has. FR-20's
                // narrowing is a scope rather than a query term — an operator would have to be
                // parsed, spelled and translated, and the account is a fact the shell already
                // holds.
                let named = if account == SiftId::from_u128(0) {
                    None
                } else {
                    session
                        .app()
                        .accounts()
                        .find(|(_, a)| a.id.as_u128() == account.to_u128())
                        .map(|(name, _)| name.clone())
                };
                session
                    .app_mut()
                    .search(query, named.as_deref(), limit)
                    .map_err(|_| ())?
            };

            let mut held = layer.search.lock().map_err(|_| ())?;
            held.interpretation = report.interpretation.join("; ");
            held.caveats = report.caveats.join("\n");
            held.sources = report
                .hits
                .iter()
                .map(|h| match h.source {
                    sift_app::search::Source::Local => SIFT_SOURCE_LOCAL,
                    sift_app::search::Source::Server => SIFT_SOURCE_SERVER,
                })
                .collect();
            held.owned = report.hits.into_iter().map(|h| h.row).collect();
            // SAFETY: the rows borrow from `held.owned`, which the layer owns and replaces
            // only on the next search — the call that also tells the shell to stop reading
            // the previous one.
            held.rows = held.owned.iter().map(|r| extend_row(row_of(r))).collect();
            Ok(SiftSearch {
                rows: SiftRows::new(extend_rows(&held.rows)),
                interpretation: SiftStr::new(extend(&held.interpretation)),
                caveats: SiftStr::new(extend(&held.caveats)),
                delegable_accounts: u32::try_from(report.delegable_accounts).unwrap_or(u32::MAX),
            })
        })
    }
}

/// Sift's own store.
pub const SIFT_SOURCE_LOCAL: u32 = 0;
/// The provider answered. Not reachable yet; the label exists because a merge that does not
/// distinguish the two is the mistake FR-21 is about.
pub const SIFT_SOURCE_SERVER: u32 = 1;

/// What a search found, and what it understood.
#[derive(Debug)]
#[repr(C)]
pub struct SiftSearch<'a> {
    /// The same fixed-layout row the list uses, so a shell draws results with the code it
    /// already has.
    pub rows: SiftRows<'a, SiftMessageRow<'a>>,
    /// How each term was read, joined by `; `. Shown to the user, not logged.
    pub interpretation: SiftStr<'a>,
    /// What this build could not answer about this query, one per line. Empty is the good
    /// case and means exactly that.
    pub caveats: SiftStr<'a>,
    /// How many accounts could have been asked to search server-side, and were not.
    pub delegable_accounts: u32,
}

/// D-101's settings: every one, with its scope, its default and what it currently holds.
///
/// **Enumerated across the boundary rather than known by each shell.** The surface is written
/// twice, in Swift and in GTK, and a default chosen independently by two shells is two
/// products — the ones that matter most being the ones that look least like decisions: the
/// dark transform is off, the debug surfaces are off, and there is no data cap.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_settings(
    app: *mut SiftApp,
    out: *mut SiftRows<'static, SiftSetting<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let session = layer.session.lock().map_err(|_| ())?;
            let held: Vec<(&'static sift_app::settings::Setting, String)> =
                sift_app::settings::SETTINGS
                    .iter()
                    .map(|s| {
                        let value = session
                            .app()
                            .setting(s.key)
                            .unwrap_or_else(|_| s.default.clone());
                        (s, value.as_text())
                    })
                    .collect();
            drop(session);

            let mut table = layer.setting_rows.lock().map_err(|_| ())?;
            let mut values = layer.setting_values.lock().map_err(|_| ())?;
            *values = held.iter().map(|(_, v)| v.clone()).collect();
            *table = held
                .iter()
                .zip(values.iter())
                .map(|((s, _), value)| SiftSetting {
                    key: SiftStr::new(s.key),
                    owner: SiftStr::new(s.owner),
                    default: SiftStr::new(match &s.default {
                        sift_app::settings::Value::Flag(true) => "true",
                        sift_app::settings::Value::Flag(false) => "false",
                        _ => "",
                    }),
                    value: SiftStr::new(extend(value)),
                    kind: match s.default {
                        sift_app::settings::Value::Flag(_) => SIFT_SETTING_FLAG,
                        sift_app::settings::Value::Number(_) => SIFT_SETTING_NUMBER,
                        sift_app::settings::Value::Text(_) => SIFT_SETTING_TEXT,
                    },
                    account_scoped: u8::from(s.scope == sift_app::settings::Scope::Account),
                    security_state: u8::from(s.is_security_state),
                })
                .collect();
            Ok(SiftRows::new(extend_rows(&table)))
        })
    }
}

/// Record a setting.
///
/// # Safety
/// `app` must be valid; both strings must point to their lengths in UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_set_setting(
    app: *mut SiftApp,
    key: *const u8,
    key_len: usize,
    value: *const u8,
    value_len: usize,
) -> SiftStatus {
    guard(|| {
        // SAFETY: the caller's obligation.
        let (key, value) = unsafe { (borrowed(key, key_len)?, borrowed(value, value_len)?) };
        // SAFETY: as above.
        let layer = (unsafe { layer(app) }).ok_or(())?;
        let mut session = layer.session.lock().map_err(|_| ())?;
        session.app_mut().set_setting(key, value).map_err(|_| ())
    })
}

/// Record an account setting — D-101's other table.
///
/// **Separate from [`sift_set_setting`] because the scope split is the storage split.** An
/// account setting goes with the account when it is removed and an installation setting does
/// not, and a single entry point taking a key would have to guess which table a key belongs to
/// from the key itself — which is exactly the ambiguity the two tables exist to remove.
///
/// The two security-state rows are refused here as they are there: the per-sender lists are
/// records of decisions the user made in context, shown and revoked where the decision was
/// made rather than bulk-edited in a screen away from any message.
///
/// # Safety
/// `app` must be valid; every string must point to its length in UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_set_account_setting(
    app: *mut SiftApp,
    label: *const u8,
    label_len: usize,
    key: *const u8,
    key_len: usize,
    value: *const u8,
    value_len: usize,
) -> SiftStatus {
    guard(|| {
        // SAFETY: the caller's obligation.
        let (name, key, value) = unsafe {
            (
                borrowed(label, label_len)?,
                borrowed(key, key_len)?,
                borrowed(value, value_len)?,
            )
        };
        // SAFETY: as above.
        let layer = (unsafe { layer(app) }).ok_or(())?;
        let mut session = layer.session.lock().map_err(|_| ())?;
        session
            .app_mut()
            .set_account_setting(name, key, value)
            .map_err(|_| ())
    })
}

/// What an account setting currently holds.
///
/// The text lives in the layer until the next call replaces it, like every other borrowed
/// string here.
///
/// # Safety
/// `app` and `out` must be valid; both strings must point to their lengths in UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_account_setting(
    app: *mut SiftApp,
    label: *const u8,
    label_len: usize,
    key: *const u8,
    key_len: usize,
    out: *mut SiftStr<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let (name, key) = (borrowed(label, label_len)?, borrowed(key, key_len)?);
            let layer = layer(app).ok_or(())?;
            let text = {
                let session = layer.session.lock().map_err(|_| ())?;
                session.app().account_setting(name, key).map_err(|_| ())?
            };
            let mut held = layer.account_setting_value.lock().map_err(|_| ())?;
            *held = text.as_text();
            Ok(SiftStr::new(extend(&held)))
        })
    }
}

/// A boolean.
pub const SIFT_SETTING_FLAG: u32 = 0;
/// A count, a byte budget, or a duration in milliseconds. The unit is the setting's.
pub const SIFT_SETTING_NUMBER: u32 = 1;
pub const SIFT_SETTING_TEXT: u32 = 2;

/// One setting, as D-101 enumerates it.
#[derive(Debug)]
#[repr(C)]
pub struct SiftSetting<'a> {
    /// Stable, and never renumbered.
    pub key: SiftStr<'a>,
    /// Which requirement or decision owns it, so a settings screen can say *why* a thing is
    /// there and a reviewer can find the argument rather than the value.
    pub owner: SiftStr<'a>,
    /// The shipped default, for flags. Empty for the other kinds, whose defaults are numbers
    /// and lists a screen shows differently anyway.
    pub default: SiftStr<'a>,
    /// What it currently holds.
    pub value: SiftStr<'a>,
    pub kind: u32,
    /// Whether it goes with the account under FR-4, rather than surviving every removal.
    pub account_scoped: u8,
    /// **Security state rather than a preference.** A record of decisions the user made in
    /// context. A shell shows these and revokes from them; it does not offer bulk editing of
    /// them in a screen away from any message.
    pub security_state: u8,
}

/// FR-34's queue: what is durably enqueued, per account, by state.
///
/// **This is the surface a person uses to check that nothing was sent.** An account that is
/// watched but not written to accumulates intents here, and being able to look at them — and
/// see that every one is `Pending` — is what makes the read-only posture something a user can
/// verify rather than something they are told.
///
/// The rows borrow from the layer and are replaced by the next call.
///
/// # Safety
/// `app` and `out` must be valid; `account` must point to `account_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_queue(
    app: *mut SiftApp,
    account: *const u8,
    account_len: usize,
    out: *mut SiftRows<'static, SiftQueued<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let name = borrowed(account, account_len)?;
            let layer = layer(app).ok_or(())?;
            let held: Vec<(SiftId, &'static str, &'static str, u32, u64)> = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                let account = session.app_mut().account(name).map_err(|_| ())?;
                account
                    .queue
                    .entries()
                    .iter()
                    .map(|e| {
                        (
                            SiftId::from_u128(e.message.as_u128()),
                            e.intent.name(),
                            e.state.name(),
                            e.attempts,
                            e.sequence,
                        )
                    })
                    .collect()
            };
            let mut table = layer.queue_rows.lock().map_err(|_| ())?;
            *table = held
                .into_iter()
                .map(|(message, intent, state, attempts, sequence)| SiftQueued {
                    message,
                    // Both are `'static` names from the register rather than borrowed text,
                    // so unlike every other row here these outlive the table they sit in.
                    intent: SiftStr::new(intent),
                    state: SiftStr::new(state),
                    attempts,
                    sequence,
                })
                .collect();
            Ok(SiftRows::new(extend_rows(&table)))
        })
    }
}

/// One durably enqueued intent, as FR-34 shows it.
#[derive(Debug)]
#[repr(C)]
pub struct SiftQueued<'a> {
    pub message: SiftId,
    /// FR-13's closed set, by name.
    pub intent: SiftStr<'a>,
    /// D-85's six states. `Pending` for everything on an account that is watched and not
    /// written to — which is the whole point of being able to read this.
    pub state: SiftStr<'a>,
    pub attempts: u32,
    /// Intents against one message apply strictly in this order. Always, including through
    /// batching, retry and a restart.
    pub sequence: u64,
}

/// FR-34's per-subsystem live bytes, and the residual nothing claimed.
///
/// The residual is reported rather than distributed. D-24's attribution is a tagging
/// allocator, and a number that added up perfectly would mean the tagging was being papered
/// over — an unattributed remainder is what an honest measurement of it looks like.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_memory(
    app: *mut SiftApp,
    out: *mut SiftRows<'static, SiftSubsystemBytes<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let mut table = layer.memory_rows.lock().map_err(|_| ())?;
            *table = sift_subsystem::Subsystem::ALL
                .iter()
                .map(|s| SiftSubsystemBytes {
                    name: SiftStr::new(s.name()),
                    // Signed on the way across, because it is signed underneath: a subsystem
                    // that frees in one task what another allocated reads negative, and
                    // clamping it to zero would hide the one number that says the tagging is
                    // wrong.
                    live_bytes: sift_alloc::live_bytes(*s),
                })
                .collect();
            table.push(SiftSubsystemBytes {
                name: SiftStr::new("attributed"),
                live_bytes: sift_alloc::total_attributed(),
            });
            Ok(SiftRows::new(extend_rows(&table)))
        })
    }
}

/// One subsystem's live bytes.
#[derive(Debug)]
#[repr(C)]
pub struct SiftSubsystemBytes<'a> {
    pub name: SiftStr<'a>,
    /// **Signed.** A subsystem that frees in one task what another allocated reads negative,
    /// and clamping that to zero would hide the one number that says the tagging is wrong.
    pub live_bytes: i64,
}

/// FR-33's debug view: which stages ran, over a document already open.
///
/// Available in release builds behind a preference, per FR-33 — the gating is the shell's,
/// because the preference is the shell's.
///
/// # Safety
/// `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_document_stages(
    app: *mut SiftApp,
    token: *const u8,
    token_len: usize,
    out: *mut SiftRows<'static, SiftStr<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let name = borrowed(token, token_len)?;
            let layer = layer(app).ok_or(())?;
            let open = layer.documents.lock().map_err(|_| ())?;
            let held = open.get(name).ok_or(())?;
            let mut table = layer.stage_rows.lock().map_err(|_| ())?;
            *table = held
                .document
                .stages
                .iter()
                .map(|s| SiftStr::new(extend(s)))
                .collect();
            Ok(SiftRows::new(extend_rows(&table)))
        })
    }
}

/// D-49's annunciator: the one condition worth drawing, across every account.
///
/// **One badge, not a list.** `AccountCondition`'s ordering is the precedence, so this is a
/// `min` over what applies — and `Healthy` renders nothing at all, because a badge that is
/// always present is a badge nobody reads.
///
/// This is a poll rather than a push on purpose. The push exists too: D-67's
/// `account_condition_changed` host callback is what wakes a shell with no window, because
/// `NeedsAuthentication` is the one condition that MUST reach the user with nothing on screen.
/// A shell that had only the callback could not draw the badge when a window opens.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_annunciator(
    app: *mut SiftApp,
    out: *mut SiftAnnunciator,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let mut session = layer.session.lock().map_err(|_| ())?;
            let (condition, accounts) = session.app_mut().annunciator();
            Ok(SiftAnnunciator {
                condition: SiftCondition::of(condition),
                accounts: u32::try_from(accounts).unwrap_or(u32::MAX),
                asks_something_of_the_user: u8::from(condition.asks_something_of_the_user()),
                reaches_the_user_without_a_window: u8::from(
                    condition.reaches_the_user_without_a_window(),
                ),
            })
        })
    }
}

/// D-49's eight conditions, in precedence order. Lower is worse.
///
/// A `#[repr(transparent)]` newtype with constants rather than a C enum, because cbindgen
/// emits an enum as both a tagged type and a typedef and Swift then sees the name twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct SiftCondition(pub u32);

impl SiftCondition {
    /// FR-2. **The one condition that must reach the user with no window open.**
    pub const NEEDS_AUTHENTICATION: Self = Self(0);
    /// Also what a full disk produces: mutations stop rather than being applied optimistically
    /// to a store Sift cannot write.
    pub const STORAGE_UNAVAILABLE: Self = Self(1);
    /// FR-22. Never resumes on its own, which is why it is not the same as the next one.
    pub const PAUSED_BY_USER: Self = Self(2);
    /// FR-36. Resumes when the accounting period rolls over.
    pub const PAUSED_BY_DATA_CAP: Self = Self(3);
    /// **Progress, not a fault.** The user action is nothing, and presenting it as a fault
    /// would be dishonest.
    pub const RECOVERING: Self = Self(4);
    /// NFR-29. A capability went away, or resynchronization has no efficient path.
    pub const DEGRADED: Self = Self(5);
    /// A quarantined intent, or triage held on an account that is watched but not written to.
    pub const ATTENTION: Self = Self(6);
    /// Draws nothing.
    pub const HEALTHY: Self = Self(7);

    const fn of(condition: sift_foundation::condition::AccountCondition) -> Self {
        use sift_foundation::condition::AccountCondition as C;
        match condition {
            C::NeedsAuthentication => Self::NEEDS_AUTHENTICATION,
            C::StorageUnavailable => Self::STORAGE_UNAVAILABLE,
            C::PausedByUser => Self::PAUSED_BY_USER,
            C::PausedByDataCap => Self::PAUSED_BY_DATA_CAP,
            C::Recovering => Self::RECOVERING,
            C::Degraded => Self::DEGRADED,
            C::Attention => Self::ATTENTION,
            C::Healthy => Self::HEALTHY,
        }
    }
}

/// What the badge draws.
#[derive(Debug)]
#[repr(C)]
pub struct SiftAnnunciator {
    pub condition: SiftCondition,
    /// How many accounts are in it. One account in trouble and five is a different sentence.
    pub accounts: u32,
    /// Whether the user has something to do. `RECOVERING` is the interesting zero.
    pub asks_something_of_the_user: u8,
    pub reaches_the_user_without_a_window: u8,
}

/// Whether an action is available right now.
///
/// D-98 makes an unavailable action **absent rather than disabled**, so this is what decides
/// whether a shell draws the menu item at all. A greyed item tells a user the action exists
/// and they cannot have it; an absent one tells them nothing, which is the trade D-98 takes
/// deliberately and records the cost of.
///
/// # Safety
/// `app` and `out` must be valid; `id` must point to `id_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_action_available(
    app: *mut SiftApp,
    id: *const u8,
    id_len: usize,
    out: *mut u8,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let name = borrowed(id, id_len)?;
            action::by_id(name).ok_or(())?;
            let layer = layer(app).ok_or(())?;
            let session = layer.session.lock().map_err(|_| ())?;
            Ok(u8::from(session.action_available(name)))
        })
    }
}

/// D-99's selection, set from the shell.
///
/// Keyed on **identity**, never on index: a row that moves under a selection is the same
/// message, and a selection that followed the index would silently retarget the gesture.
///
/// # Safety
/// `app` must be valid; `ids` must point to `count` identifiers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_select(
    app: *mut SiftApp,
    ids: *const SiftId,
    count: usize,
) -> SiftStatus {
    guard(|| {
        if ids.is_null() && count > 0 {
            return Err(());
        }
        // SAFETY: the caller's obligation.
        let selected = if count == 0 {
            &[][..]
        } else {
            unsafe { core::slice::from_raw_parts(ids, count) }
        };
        // SAFETY: the caller's obligation.
        let layer = (unsafe { layer(app) }).ok_or(())?;
        let mut session = layer.session.lock().map_err(|_| ())?;
        session.app_mut().selection = selected
            .iter()
            .map(|id| sift_foundation::identity::LocalId::from_u128(id.to_u128()))
            .collect();
        Ok(())
    })
}

/// Tell the layer whether a window exists.
///
/// FR-25 makes closing a window and quitting different acts, so "a window exists" is a fact
/// the shell owns and the layer is told — every `Window`-scoped action in the register turns
/// on it, and a layer that assumed one would offer a menu to nobody.
///
/// # Safety
/// `app` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_set_window_present(app: *mut SiftApp, present: u8) -> SiftStatus {
    guard(|| {
        // SAFETY: the caller's obligation.
        let layer = (unsafe { layer(app) }).ok_or(())?;
        let mut session = layer.session.lock().map_err(|_| ())?;
        session.app_mut().has_window = present != 0;
        Ok(())
    })
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
            // SAFETY: the caller's obligation.
            let Some(layer) = layer(app) else {
                return Err(());
            };
            // An observation is **anchored, not an integer range**: a shell holding a range
            // would have to recompute it on every notification, which is the polling D-18
            // rejected wearing different clothes.
            //
            // A zero anchor is the unified inbox — D-4 — rather than an account nobody has.
            let account = if anchor == SiftId::from_u128(0) {
                None
            } else {
                Some(sift_foundation::identity::AccountId::from_u128(
                    anchor.to_u128(),
                ))
            };
            let id = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session.observe(Watching::Messages {
                    account,
                    limit: count,
                })
            };
            layer.sinks.lock().map_err(|_| ())?.insert(
                id.0,
                Sink {
                    callback,
                    context: context as usize,
                },
            );
            // The first delivery fills a list the shell has never drawn.
            crate::layer::post(
                layer,
                Task::Deliver {
                    layer: app as usize,
                },
            );
            Ok(SiftObservation(id.0))
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
        // An identity, not a generation. Cancelling by generation would cancel every
        // observation sharing it, which with one live generation means all of them.
        if !observation.is_valid() {
            return Err(());
        }
        // SAFETY: the caller's obligation.
        let Some(layer) = (unsafe { layer(app) }) else {
            return Err(());
        };
        // The sink goes first. After this returns no callback for this observation can be
        // reached, which is D-48's synchronous-cancellation guarantee — a delivery already
        // posted to the shell's loop finds nothing to call and is discarded on arrival
        // rather than being something this call had to wait for.
        layer.sinks.lock().map_err(|_| ())?.remove(&observation.0);
        let live = layer
            .session
            .lock()
            .map_err(|_| ())?
            .cancel(sift_session::ObservationId(observation.0));
        if live { Ok(()) } else { Err(()) }
    })
}

/// Run a scheduled delivery. **The shell calls this, on its main loop, and nowhere else.**
///
/// This is the far side of D-48's hop: the layer asked the shell to arrange for a ticket to
/// be run on its loop, and this is what running it means. Every observer callback the shell
/// receives is invoked from inside this call, which is what makes "delivered on the shell's
/// own main loop" true rather than hoped for.
///
/// A ticket that was already run, or that belonged to a layer since torn down, resolves to
/// nothing. That is not a defect to report: a window closing between the post and the turn
/// of the loop is ordinary, and the guarantee cancellation makes is precisely that the
/// delivery finds nothing to call.
///
/// # Safety
/// Called from the shell's main loop, with a ticket the layer issued.
#[unsafe(no_mangle)]
pub extern "C" fn sift_run_scheduled(ticket: u64) {
    // A panic here is on the shell's own loop, so it must not unwind into it.
    let _ = crate::barrier::guard(|| {
        let Some(task) = crate::layer::take(ticket) else {
            return Ok(());
        };
        match task {
            crate::layer::Task::Deliver { layer } => {
                if layer == 0 {
                    return Ok(());
                }
                // SAFETY: the pointer was live when the ticket was posted, and `sift_shutdown`
                // abandons every outstanding ticket before it frees the layer — so a ticket
                // that resolves at all names a layer that still exists.
                let layer = unsafe { &*(layer as *const Layer) };
                deliver(layer);
                Ok(())
            }
        }
    });
}

/// Tell the shell about a condition that has changed since it was last told.
///
/// **D-67's callbacks, finally invoked.** The set was registered at initialization and stored
/// whole, and nothing on this side ever called one — so FR-2's requirement that
/// re-authentication reach the user *with no window open* could not be met by any shell,
/// because the only thing that could have woken one never fired.
///
/// Called from the main-loop hop, which is where D-48 requires every callback to arrive. The
/// session lock is released before any of them, because a shell is permitted to call back into
/// the layer on the next turn of its loop and holding it here would make that a deadlock
/// waiting for a schedule.
fn announce_conditions(layer: &Layer) {
    let Ok(mut session) = layer.session.lock() else {
        return;
    };
    let names: Vec<String> = session
        .app()
        .accounts()
        .map(|(name, _)| name.clone())
        .collect();
    let mut now = Vec::with_capacity(names.len());
    for name in &names {
        let Ok(condition) = session.app_mut().condition_of(name) else {
            continue;
        };
        let Some((_, account)) = session.app().accounts().find(|(n, _)| *n == name) else {
            continue;
        };
        now.push((account.id.as_u128(), SiftCondition::of(condition).0));
    }
    drop(session);

    let Ok(mut last) = layer.conditions.lock() else {
        return;
    };
    let mut changed = Vec::new();
    for (id, condition) in now {
        if last.insert(id, condition) != Some(condition) {
            changed.push((id, condition));
        }
    }
    drop(last);

    for (id, condition) in changed {
        let account = SiftId::from_u128(id);
        (layer.host.account_condition_changed)(layer.host.context, account, condition);
        // FR-2 is the one that must arrive with nothing on screen, so it gets its own
        // callback rather than being inferred from the condition by a shell that may have no
        // window to infer it in.
        if condition == SiftCondition::NEEDS_AUTHENTICATION.0 {
            (layer.host.reauthentication_needed)(layer.host.context, account);
        }
    }
}

/// Compute what changed and hand each batch to the observation that asked for it.
fn deliver(layer: &Layer) {
    announce_conditions(layer);
    let Ok(mut session) = layer.session.lock() else {
        return;
    };
    let Ok(deliveries) = session.poll() else {
        // A store that could not be read is a delivery that did not happen, not a window
        // that was emptied. The registration keeps the window it had and the next signal
        // tries again — a list that blanked itself on a locked database would be worse.
        return;
    };
    drop(session);

    // **Resolved under the lock, called outside it.** A shell is inside its own main loop when
    // one of these runs, and D-48 lets it call back into the layer on the next turn — but the
    // list it draws can also reach `observe` or `cancel` synchronously from a selection change,
    // and both take this mutex. Holding it across the callback makes that a self-deadlock one
    // call away, in a shell that has just grown several new selection-driven paths.
    let targets: Vec<(usize, Sink)> = {
        let Ok(sinks) = layer.sinks.lock() else {
            return;
        };
        deliveries
            .iter()
            .enumerate()
            // A sink removed by cancellation is the guarantee doing its job: the delivery was
            // computed before the cancel and finds nothing to call.
            .filter_map(|(i, d)| sinks.get(&d.observation.0).map(|sink| (i, *sink)))
            .collect()
    };
    for (index, sink) in targets {
        let d = &deliveries[index];
        // **The window, not the rows entering it.** A shell given only the arrivals has no way
        // to express a removal, so it appends — and a sync that drops a message leaves the row
        // on screen pointing at something the store no longer holds. D-18's change vocabulary
        // is the specified answer and `SiftChange` is written and tested; nothing carries it
        // across this boundary yet, and until something does, the whole window is the honest
        // delivery.
        let rows: Vec<SiftMessageRow<'_>> = d.window.iter().map(row_of).collect();
        (sink.callback)(
            sink.context as *mut c_void,
            SiftObservation(d.observation.0),
            Generation(d.generation.0),
            SiftRows::new(&rows),
        );
    }
}

/// One application row, as the boundary carries it.
///
/// Every string points into the row it came from and is valid for the delivery only, which
/// is D-66's rule: a shell that needs a value beyond the callback copies it.
/// # Safety
/// The row must borrow from layer-owned storage freed only by the call that tells the shell
/// to stop reading it.
const unsafe fn extend_row(r: SiftMessageRow<'_>) -> SiftMessageRow<'static> {
    // SAFETY: the caller's obligation. The lifetime is the only thing that changes.
    unsafe { core::mem::transmute::<SiftMessageRow<'_>, SiftMessageRow<'static>>(r) }
}

fn row_of(r: &sift_app::rows::MessageRow) -> SiftMessageRow<'_> {
    SiftMessageRow {
        id: SiftId::from_u128(r.id.as_u128()),
        account: SiftId::from_u128(r.account.as_u128()),
        received_millis: r.received_millis,
        origination_millis: r.origination_millis,
        sender: SiftStr::new(&r.sender),
        subject: SiftStr::new(&r.subject),
        snippet: SiftStr::new(&r.snippet),
        unread: u8::from(r.unread),
        flagged: u8::from(r.flagged),
        has_attachments: u8::from(r.has_attachments),
        duplicate_across_accounts: 0,
        thread_count: r.thread_count,
    }
}

/// Add an account backed by D-65's recorded corpus rather than by a socket.
///
/// **This is the fixture path, and it is deliberately part of the boundary rather than a
/// test-only door.** D-98 says the shell test harness invokes through the same entry points a
/// shell does; a second door would mean the thing under test is not the thing that ships.
/// What it adds is a real adapter over recorded exchanges — no network, no credential, no
/// account belonging to anybody — which is what lets a shell be driven, and looked at, before
/// a real mailbox is ever connected.
///
/// # Safety
/// `app` must be valid; `label` must point to `label_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_add_replayed_account(
    app: *mut SiftApp,
    label: *const u8,
    label_len: usize,
    out: *mut SiftId,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            if label.is_null() {
                return Err(());
            }
            // SAFETY: the caller's obligation — already inside this entry point's own
            // `unsafe` block, so no second one.
            let bytes = core::slice::from_raw_parts(label, label_len);
            let name = core::str::from_utf8(bytes).map_err(|_| ())?;
            let Some(layer) = layer(app) else {
                return Err(());
            };
            let id = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session
                    .app_mut()
                    .add_replayed_account(name)
                    .map_err(|_| ())?
            };
            crate::layer::post(
                layer,
                Task::Deliver {
                    layer: app as usize,
                },
            );
            Ok(SiftId::from_u128(id.as_u128()))
        })
    }
}

/// Discover an account's folders and walk its delta.
///
/// Folders first, because a delta needs somewhere to put what it finds and D-83 assigns local
/// identity on discovery rather than on first use.
///
/// **This blocks the calling thread**, which is a limitation rather than a design: the work
/// belongs on a worker under D-19, and moving it there changes nothing a shell can see
/// because every delivery already arrives through D-48's hop rather than from this call.
///
/// # Safety
/// `app` must be valid; `label` must point to `label_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_sync_account(
    app: *mut SiftApp,
    label: *const u8,
    label_len: usize,
) -> SiftStatus {
    guard(|| {
        if label.is_null() {
            return Err(());
        }
        // SAFETY: the caller's obligation.
        let bytes = unsafe { core::slice::from_raw_parts(label, label_len) };
        let name = core::str::from_utf8(bytes).map_err(|_| ())?;
        // SAFETY: the caller's obligation.
        let Some(layer) = (unsafe { layer(app) }) else {
            return Err(());
        };
        {
            let mut session = layer.session.lock().map_err(|_| ())?;
            session.app_mut().sync(name, 20).map_err(|_| ())?;
        }
        crate::layer::post(
            layer,
            Task::Deliver {
                layer: app as usize,
            },
        );
        Ok(())
    })
}

/// A rendered message body, as the reader receives it.
///
/// The strings point into layer-owned storage that lives until the document is closed, which
/// is longer than a delivery: the body view holds the HTML while it renders, and resolves
/// resources against the token afterwards.
#[derive(Debug)]
#[repr(C)]
pub struct SiftDocument<'a> {
    /// **Post-sanitization.** A raw provider payload never reaches a shell.
    pub html: SiftStr<'a>,
    /// D-28's per-document capability token. Every address in `html` is under it.
    pub token: SiftStr<'a>,
    /// Every position in the document that would fetch something, refused or not.
    ///
    /// Beside the refusal count rather than inferred from it: "three images, all three
    /// withheld" and "three images, one withheld" are different sentences, and only the pair
    /// distinguishes them.
    pub fetching_positions: u32,
    /// How many were refused. For the reader's **native** chrome — a count drawn inside the
    /// document is one a sender can counterfeit, and so is a control drawn inside it.
    pub blocked: u32,
    /// How many navigation targets the body carries.
    pub links: u32,
    /// Whether "always load from this sender" has anything to key a durable allowance on.
    /// Where this is zero the control is **absent** rather than disabled: an allowance keyed
    /// on nothing applies to everyone, which is the opposite of what the control says.
    pub may_always_allow: u8,
    /// Whether FR-42's destination exists. Read it with [`sift_document_unsubscribe`].
    pub has_unsubscribe: u8,
}

/// One refused fetching position, and the rule that refused it.
///
/// The strings borrow from the open document and die with it.
#[derive(Debug)]
#[repr(C)]
pub struct SiftWithheld<'a> {
    /// Where it appeared, so the disclosure says *what* was lost rather than only how much.
    pub element: SiftStr<'a>,
    pub attribute: SiftStr<'a>,
    /// The address, decoded and bidi-stripped. Safe to render in native chrome.
    pub displayed: SiftStr<'a>,
    /// Why. Where no filter list is loaded this says so, rather than naming a rule that did
    /// not run — a user who believes a rule matched believes in a protection that is absent.
    pub rule: SiftStr<'a>,
}

impl SiftWithheld<'static> {
    /// # Safety
    /// The borrowed document must outlive every use of the result.
    unsafe fn of(w: &sift_app::document::Withheld) -> Self {
        unsafe {
            Self {
                element: SiftStr::new(extend(&w.element)),
                attribute: SiftStr::new(extend(&w.attribute)),
                displayed: SiftStr::new(extend(&w.displayed)),
                rule: SiftStr::new(extend(&w.rule)),
            }
        }
    }
}

/// One navigation target, as FR-30 requires it be shown.
#[derive(Debug)]
#[repr(C)]
pub struct SiftLink<'a> {
    /// Punycode-decoded and bidi-**stripped** — the opposite of what NFR-54 does to a display
    /// name, because a URL's component order carries meaning and prose's does not.
    pub displayed: SiftStr<'a>,
    /// What will actually be opened.
    pub target: SiftStr<'a>,
    /// The wrapper this was recovered from, or null where there was none. Never followed to
    /// find out where it goes — following it *is* the tracking event.
    pub wrapper: SiftStr<'a>,
    /// A `mailto:`, shown and reported as needing a mail handler rather than omitted.
    pub needs_a_mail_handler: u8,
}

impl SiftLink<'static> {
    /// # Safety
    /// The borrowed document must outlive every use of the result.
    unsafe fn of(l: &sift_app::document::Link) -> Self {
        unsafe {
            Self {
                displayed: SiftStr::new(extend(&l.displayed)),
                target: SiftStr::new(extend(&l.target)),
                wrapper: l
                    .wrapper
                    .as_deref()
                    .map_or_else(SiftStr::null, |w| SiftStr::new(extend(w))),
                needs_a_mail_handler: u8::from(l.needs_a_mail_handler),
            }
        }
    }
}

/// One attachment, as FR-10 lists it. Nothing here has been downloaded.
#[derive(Debug)]
#[repr(C)]
pub struct SiftAttachment<'a> {
    /// The identifier a save takes. Opaque above the adapter.
    pub part: SiftStr<'a>,
    /// What the sender declared. Advisory, and one of three sources.
    pub media_type: SiftStr<'a>,
    /// The sender's name, normalized for chrome under NFR-54.
    pub display_name: SiftStr<'a>,
    /// The name a save would derive under NFR-53. Shown beside the sender's where they
    /// differ, because a name that changed silently is one the user did not agree to.
    pub file_name: SiftStr<'a>,
    /// What the provider says it costs. Advisory: L-13 bounds what is transferred.
    pub declared_size: u64,
    /// [`SIFT_WARN_DECLARED`], [`SIFT_WARN_EXTENSION`] and [`SIFT_WARN_DISAGREES`], or-ed.
    /// Non-zero means FR-10's explicit warning is required before opening.
    pub warning: u32,
}

/// The declared media type is an executable one.
pub const SIFT_WARN_DECLARED: u32 = 1;
/// The name ends in an extension the platform will execute. **This is the source that decides
/// what actually happens**, and the one a type check never sees.
pub const SIFT_WARN_EXTENSION: u32 = 2;
/// The sources disagree, which FR-10 makes suspicious in its own right: a sender who labels
/// an executable as a document has said something about their intent.
pub const SIFT_WARN_DISAGREES: u32 = 4;

const fn warning_bits(w: sift_app::attachment::Warning) -> u32 {
    (if w.declared { SIFT_WARN_DECLARED } else { 0 })
        | (if w.extension { SIFT_WARN_EXTENSION } else { 0 })
        | (if w.disagrees { SIFT_WARN_DISAGREES } else { 0 })
}

impl SiftAttachment<'static> {
    /// # Safety
    /// The borrowed listing must outlive every use of the result.
    unsafe fn of(a: &sift_app::attachment::Attachment) -> Self {
        unsafe {
            Self {
                part: SiftStr::new(extend(&a.part)),
                media_type: SiftStr::new(extend(&a.media_type)),
                display_name: SiftStr::new(extend(a.display_name.as_str())),
                file_name: SiftStr::new(extend(&a.file_name)),
                declared_size: a.declared_size,
                warning: warning_bits(a.warning),
            }
        }
    }
}

/// Open a message's body: fetch its chosen part and run the seven stages over it.
///
/// The document stays open until [`sift_close_document`] revokes its token, because the body
/// view asks for resources after the HTML has been handed over.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_open_document(
    app: *mut SiftApp,
    message: SiftId,
    dark: u8,
    out: *mut SiftDocument<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let Some(layer) = layer(app) else {
                return Err(());
            };
            let id = sift_foundation::identity::LocalId::from_u128(message.to_u128());
            let document = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                let document = session
                    .app_mut()
                    .open_document(id, dark != 0)
                    .map_err(|_| ())?;
                // **What makes D-98's `OpenMessage` scope reachable at all.** The field was
                // set to `None` at initialization and assigned nowhere, so every action scoped
                // to an open message — the dark transform, FR-41's three handoffs, FR-33's
                // debug view — was absent from every menu and every palette, permanently, and
                // the register's own reconciliation could not see it because both sides agreed
                // the identifiers existed.
                session.app_mut().open_message = Some(id);
                document
            };
            let blocked = u32::try_from(document.blocked).unwrap_or(u32::MAX);
            let positions = u32::try_from(document.fetching_positions).unwrap_or(u32::MAX);
            let links = u32::try_from(document.links.len()).unwrap_or(u32::MAX);
            let may_always_allow = u8::from(document.may_always_allow);
            let has_unsubscribe = u8::from(document.unsubscribe.is_some());
            let token = document.token.clone();

            // The strings outlive the call, so they are held by the layer and keyed on the
            // token the shell is about to be given. Closing the document is what frees them,
            // which is the same gesture that revokes the token — one lifetime, not two.
            let mut open = layer.documents.lock().map_err(|_| ())?;
            let entry = open.entry(token.clone()).or_insert_with(|| {
                let mut held = OpenDocument {
                    document,
                    withheld: Vec::new(),
                    links: Vec::new(),
                };
                // SAFETY: the rows borrow from `held.document`, which is moved into the map
                // in this same expression and removed only by `sift_close_document` — the
                // call that also tells the shell to stop using them.
                held.withheld = held
                    .document
                    .withheld
                    .iter()
                    .map(|w| SiftWithheld::of(w))
                    .collect();
                held.links = held
                    .document
                    .links
                    .iter()
                    .map(|l| SiftLink::of(l))
                    .collect();
                held
            });
            Ok(SiftDocument {
                html: SiftStr::new(extend(&entry.document.html)),
                token: SiftStr::new(extend(&entry.document.token)),
                fetching_positions: positions,
                blocked,
                links,
                may_always_allow,
                has_unsubscribe,
            })
        })
    }
}

/// FR-29's disclosure: every refused position in an open document, and the rule behind it.
///
/// The rows borrow from the document and are valid until [`sift_close_document`]. That is one
/// lifetime rather than two that can disagree — the revocation that kills the addresses is the
/// same call that frees the rows describing them.
///
/// # Safety
/// `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_document_withheld(
    app: *mut SiftApp,
    token: *const u8,
    token_len: usize,
    out: *mut SiftRows<'static, SiftWithheld<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let name = borrowed(token, token_len)?;
            let open = layer.documents.lock().map_err(|_| ())?;
            let held = open.get(name).ok_or(())?;
            Ok(SiftRows::new(extend_rows(&held.withheld)))
        })
    }
}

/// FR-8 — the user allows this message's remote content, once or for this sender.
///
/// **The two controls are different things and this is where the difference lives.**
/// `durable` zero lets the document that is open fetch, and dies with its token, so
/// re-opening the same message asks again. `durable` non-zero writes the sender into the
/// allowance list and survives. A single flag serving both would silently make a transient
/// choice permanent, which is the failure a user can neither see nor undo.
///
/// The shell holds a token, never an origin. Which sender a durable allowance keys on is the
/// layer's to resolve, from what authenticated the message — so a shell cannot name a sender
/// it was not given, and cannot key an allowance on one that authenticated nothing. Where
/// there is nothing to key on this **fails**, and the interface must not have offered the
/// control: `SiftDocument::may_always_allow` is what says so before it is pressed.
///
/// # Safety
/// `app` must be valid; `token` must point to `token_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_allow_remote_content(
    app: *mut SiftApp,
    token: *const u8,
    token_len: usize,
    durable: u8,
) -> SiftStatus {
    unsafe {
        guard(|| {
            let layer = layer(app).ok_or(())?;
            let name = borrowed(token, token_len)?;
            let mut session = layer.session.lock().map_err(|_| ())?;
            let broker = &mut session.app_mut().resources;
            if durable == 0 {
                return broker.allow_once(name).then_some(()).ok_or(());
            }
            let origin = broker.origin_of(name).ok_or(())?;
            // The durable allowance and the transient one, because the message in front of
            // the user must load now as well as the next one from this sender. Ordering them
            // the other way would leave the open document blocked by the very click that
            // allowed its sender.
            let allowed = broker.allow_origin(&origin);
            broker.allow_once(name);
            allowed.then_some(()).ok_or(())
        })
    }
}

/// FR-30: every navigation target, with the destination the confirmation sheet must show.
///
/// # Safety
/// `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_document_links(
    app: *mut SiftApp,
    token: *const u8,
    token_len: usize,
    out: *mut SiftRows<'static, SiftLink<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let name = borrowed(token, token_len)?;
            let open = layer.documents.lock().map_err(|_| ())?;
            let held = open.get(name).ok_or(())?;
            Ok(SiftRows::new(extend_rows(&held.links)))
        })
    }
}

/// FR-42: the unsubscribe destination, where the message declares one.
///
/// Fails where there is none, which the shell already knows from `has_unsubscribe` — the two
/// agree by construction because both read the same field.
///
/// **Sift never issues the request.** The historical form is a message, which the no-send
/// constraint forbids outright; the modern form is an HTTP request to an address carrying a
/// per-recipient token, which is precisely what FR-29 treats as evidence that a resource is
/// tracking the reader. This hands the shell a destination to open in a browser, and nothing
/// on this boundary can be made to fetch it.
///
/// # Safety
/// `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_document_unsubscribe(
    app: *mut SiftApp,
    token: *const u8,
    token_len: usize,
    out: *mut SiftLink<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let name = borrowed(token, token_len)?;
            let open = layer.documents.lock().map_err(|_| ())?;
            let held = open.get(name).ok_or(())?;
            let link = held.document.unsubscribe.as_ref().ok_or(())?;
            Ok(SiftLink::of(link))
        })
    }
}

/// FR-10's list. **Nothing is downloaded** — this reads the structure the sync already has.
///
/// The rows are held per message and replaced on each call, so a shell that lists twice sees
/// the second listing rather than two.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_message_attachments(
    app: *mut SiftApp,
    message: SiftId,
    out: *mut SiftRows<'static, SiftAttachment<'static>>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let listed = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session
                    .app_mut()
                    .attachments(sift_foundation::identity::LocalId::from_u128(
                        message.to_u128(),
                    ))
                    .map_err(|_| ())?
            };
            let mut open = layer.attachments.lock().map_err(|_| ())?;
            let entry = open.entry(message.to_u128()).or_insert(OpenAttachments {
                listed: Vec::new(),
                rows: Vec::new(),
            });
            entry.listed = listed;
            entry.rows.clear();
            // SAFETY: the rows borrow from `entry.listed`, which the layer owns and replaces
            // only here — and a replacement is a new listing the shell asked for.
            entry.rows = entry.listed.iter().map(|a| SiftAttachment::of(a)).collect();
            Ok(SiftRows::new(extend_rows(&entry.rows)))
        })
    }
}

/// NFR-53's plan: where an attachment would be written, resolved and shown before the write.
#[derive(Debug)]
#[repr(C)]
pub struct SiftSavePlan<'a> {
    /// Names the plan. [`sift_write_attachment`] takes this rather than a path, so the path
    /// that is written is the one that was shown — re-deriving at write time is exactly how
    /// those two come apart.
    pub plan: u64,
    /// The **exact** final path, including any disambiguating suffix.
    pub final_path: SiftStr<'a>,
    /// Whether the derived name differs from the sender's, which is worth saying out loud.
    pub renamed: u8,
    pub declared_size: u64,
}

/// Resolve where an attachment would be written. **Writes nothing.**
///
/// Two calls rather than one, because the requirement is that the exact path be shown
/// *before* the write. One call that saved and then reported would satisfy every test and
/// none of the requirement.
///
/// The directory is the user's and comes from the platform's own chooser; Sift decides only
/// the name, and decides it under NFR-53.
///
/// # Safety
/// `app` and `out` must be valid; `part` and `directory` must point to their lengths in UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_plan_attachment_save(
    app: *mut SiftApp,
    message: SiftId,
    part: *const u8,
    part_len: usize,
    directory: *const u8,
    directory_len: usize,
    out: *mut SiftSavePlan<'static>,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let part = borrowed(part, part_len)?;
            let directory = borrowed(directory, directory_len)?;
            let plan = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session
                    .app_mut()
                    .plan_attachment_save(
                        sift_foundation::identity::LocalId::from_u128(message.to_u128()),
                        part,
                        std::path::Path::new(directory),
                    )
                    .map_err(|_| ())?
            };
            let handle = {
                let mut next = layer.next_plan.lock().map_err(|_| ())?;
                let handle = *next;
                *next += 1;
                handle
            };
            let renamed = u8::from(plan.renamed);
            let declared_size = plan.declared_size;
            let mut plans = layer.plans.lock().map_err(|_| ())?;
            let held = plans.entry(handle).or_insert(plan);
            // The path is not valid UTF-8 on every platform; a path that cannot be shown is a
            // path that cannot be written under a requirement to show it first.
            let shown = held.final_path.to_str().ok_or(())?;
            Ok(SiftSavePlan {
                plan: handle,
                final_path: SiftStr::new(extend(shown)),
                renamed,
                declared_size,
            })
        })
    }
}

/// What a completed save wrote, and what the bytes turned out to be.
#[derive(Debug)]
#[repr(C)]
pub struct SiftSaveOutcome {
    pub written: u64,
    /// The warning bits again, now including the source that only exists once the content
    /// does. A `.pdf` whose bytes begin `MZ` is the case FR-10 wrote the rule for.
    pub warning: u32,
}

/// Fetch the part and write it to the planned path.
///
/// **Nothing is overwritten**, and that is held at the syscall rather than by a check — a
/// check before a write is a race, and the file that appears between the two is the one
/// somebody cared about.
///
/// The plan is consumed, so one plan writes one file. A shell that wants a second copy asks
/// for a second plan, which resolves a second path.
///
/// # Safety
/// `app` and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_write_attachment(
    app: *mut SiftApp,
    plan: u64,
    out: *mut SiftSaveOutcome,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            let layer = layer(app).ok_or(())?;
            let plan = layer
                .plans
                .lock()
                .map_err(|_| ())?
                .remove(&plan)
                .ok_or(())?;
            let mut session = layer.session.lock().map_err(|_| ())?;
            let (written, warning) = session.app_mut().write_attachment(&plan).map_err(|_| ())?;
            Ok(SiftSaveOutcome {
                written,
                warning: warning_bits(warning),
            })
        })
    }
}

/// Close a document: revoke its token and release what was held for it.
///
/// D-90 revokes at **navigation**, which is earlier and more often than teardown. Message A's
/// addresses are dead before message B's document exists, whether or not the view survives —
/// which is what keeps "two messages share no address space" true across a reused view.
///
/// # Safety
/// `app` must be valid; `token` must point to `token_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_close_document(
    app: *mut SiftApp,
    token: *const u8,
    token_len: usize,
) -> SiftStatus {
    guard(|| {
        if token.is_null() {
            return Err(());
        }
        // SAFETY: the caller's obligation.
        let bytes = unsafe { core::slice::from_raw_parts(token, token_len) };
        let name = core::str::from_utf8(bytes).map_err(|_| ())?;
        // SAFETY: the caller's obligation.
        let Some(layer) = (unsafe { layer(app) }) else {
            return Err(());
        };
        let remaining = {
            let mut documents = layer.documents.lock().map_err(|_| ())?;
            documents.remove(name);
            documents.len()
        };
        // **Only when the last one goes.** A reader that closed its message and left the menu
        // offering to reply to it would be offering a gesture over nothing — but the reader
        // opens the next document *before* revoking the previous token, under D-90, so
        // clearing on every close would clear the message that had just been opened.
        if remaining == 0 {
            layer.session.lock().map_err(|_| ())?.app_mut().open_message = None;
        }
        let revoked = layer
            .session
            .lock()
            .map_err(|_| ())?
            .app_mut()
            .close_document(name);
        if revoked { Ok(()) } else { Err(()) }
    })
}

/// What the broker said about one resource load.
///
/// Three answers, kept apart. "Sift refused this" and "this did not arrive" are different
/// facts, and FR-12 insists such pairs stay distinct — a reader that showed one as the other
/// would tell a person their mail was being censored, or that it was fine when it was not.
/// A transparent newtype rather than a C enum, deliberately.
///
/// cbindgen emits an enum as *both* a tagged `enum` and a `typedef`, and a Swift importer sees
/// two things with one name — which is ambiguous at the use site and cannot be disambiguated
/// without naming the module. A transparent wrapper with associated constants emits one
/// typedef and a set of `#define`s, which is unambiguous in every consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct SiftResourceAnswer(pub u32);

impl SiftResourceAnswer {
    /// The bytes are available.
    pub const BYTES: Self = Self(0);
    /// Deterministically refused, with a reason the reader can render.
    pub const BLOCKED: Self = Self(1);
    /// Could not be produced — a revoked token, a missing blob, a fabricated address.
    ///
    /// **Distinct from blocked, and the distinction is load-bearing.** "Sift refused this" and
    /// "this did not arrive" are different facts, and a fabricated or stale address resolving
    /// here is a defect being caught rather than a resource being refused.
    pub const UNAVAILABLE: Self = Self(2);
}

/// Resolve one address under the internal scheme.
///
/// **This is the body view's only channel out**, and it is a decision function rather than an
/// interception: N-1 leaves the view no network capability at all, so there is nothing to
/// intercept. A fabricated or stale address resolves to `Unavailable` rather than to nothing,
/// because a defect being caught and a resource being refused are different facts.
///
/// # Safety
/// `app` and `out` must be valid; `url` must point to `url_len` bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sift_resolve_resource(
    app: *mut SiftApp,
    url: *const u8,
    url_len: usize,
    out: *mut SiftResourceAnswer,
) -> SiftStatus {
    unsafe {
        guard_out(out, || {
            if url.is_null() {
                return Err(());
            }
            let bytes = core::slice::from_raw_parts(url, url_len);
            let address = core::str::from_utf8(bytes).map_err(|_| ())?;
            let Some(layer) = layer(app) else {
                return Err(());
            };
            let answer = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session.app_mut().resolve_resource(address, None)
            };
            Ok(match answer {
                sift_broker::broker::Answer::Bytes { .. } => SiftResourceAnswer::BYTES,
                sift_broker::broker::Answer::Blocked(_) => SiftResourceAnswer::BLOCKED,
                sift_broker::broker::Answer::Unavailable(_) => SiftResourceAnswer::UNAVAILABLE,
            })
        })
    }
}

/// Borrow layer-owned text for as long as the document that owns it is open.
///
/// # Safety
/// The caller must not let the returned reference outlive the entry in `Layer::documents`
/// that owns it — which is what `sift_close_document` is for.
/// Borrow a caller's string, which is a pointer and a length and never NUL-terminated.
///
/// # Safety
/// `p` must point to `len` bytes, or be null.
unsafe fn borrowed<'a>(p: *const u8, len: usize) -> Result<&'a str, ()> {
    if p.is_null() {
        return Err(());
    }
    // SAFETY: the caller's obligation.
    let bytes = unsafe { core::slice::from_raw_parts(p, len) };
    core::str::from_utf8(bytes).map_err(|_| ())
}

/// # Safety
/// The rows must live in layer-owned storage freed only by the call that tells the shell to
/// stop reading them.
unsafe fn extend_rows<T>(rows: &[T]) -> &'static [T] {
    // SAFETY: the caller's obligation.
    unsafe { &*(std::ptr::from_ref::<[T]>(rows)) }
}

unsafe fn extend(s: &str) -> &'static str {
    // SAFETY: the string lives in the layer's document table and is removed only by
    // `sift_close_document`, which is also what tells the shell to stop using it.
    unsafe { &*(std::ptr::from_ref::<str>(s)) }
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

    /// A schedule that runs the ticket immediately.
    ///
    /// **Only a test may do this.** A shell that ran a delivery inline would hand a callback
    /// back from inside the call that caused it, which is the reentrancy D-48 forbids. A test
    /// has no loop to post to, and running it inline is what makes the delivery observable.
    extern "C" fn run_inline(_: *mut c_void, run: crate::layer::SiftRun, ticket: u64) {
        run(ticket);
    }

    /// A schedule that drops the ticket on the floor, as a shell whose window closed does.
    extern "C" fn drop_it(_: *mut c_void, _: crate::layer::SiftRun, _: u64) {}

    /// A scratch container, leaked so it can be a `'static` string the way a bundle's own
    /// path is. Only a test needs this; a shell's container path outlives the process.
    fn scratch_str() -> &'static str {
        let d = scratch();
        Box::leak(d.to_str().expect("utf-8").to_owned().into_boxed_str())
    }

    /// The tests drive the boundary without a credential store: opening a real container
    /// would reach the login keychain and prompt. A shell never sets this.
    fn ephemeral() {
        // SAFETY: single-threaded at this point in every test, and the value is only ever set.
        unsafe { std::env::set_var("SIFT_EPHEMERAL", "1") };
    }

    fn scratch() -> std::path::PathBuf {
        // A clock is **not** a unique identifier. `as_nanos` reports at whatever resolution the
        // platform has, and two of these tests running in parallel on the same machine can and
        // do read the same value — which gives two layers one container, two accounts one set
        // of files, and a failure that appears about once in five runs. The counter is what
        // makes it unique; the clock is only there to keep the names readable.
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("sift-abi-{}-{unique}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).expect("scratch");
        d
    }

    fn start(schedule: crate::layer::SiftSchedule, root: &'static str) -> *mut SiftApp {
        ephemeral();
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let init = SiftInit {
            container_root: SiftStr::new(root),
            schedule,
            schedule_context: core::ptr::null_mut(),
            oauth_client_id: SiftStr::new(""),
            registered_schemes: SiftStr::new(""),
        };
        let status = unsafe { sift_initialize(callbacks(), init, &raw mut app) };
        assert_eq!(status, SiftStatus::Ok);
        assert!(!app.is_null(), "initialization handed back no layer");
        app
    }

    /// Invoke by identifier, with no parameter and no confirmation — what a menu item does.
    fn do_action(app: *mut SiftApp, id: &str) -> SiftStatus {
        let mut gesture = SiftGesture {
            mutated: 0,
            enqueued: 0,
            skipped: 0,
            optimistic: 0,
        };
        unsafe {
            sift_invoke_action(
                app,
                id.as_ptr(),
                id.len(),
                core::ptr::null(),
                0,
                0,
                &raw mut gesture,
            )
        }
    }

    /// A layer configured the way a bundle configures one: a client, and the schemes that
    /// bundle claims.
    fn start_configured(client: &'static str, schemes: &'static str) -> *mut SiftApp {
        ephemeral();
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let init = SiftInit {
            container_root: SiftStr::new(scratch_str()),
            schedule: drop_it,
            schedule_context: core::ptr::null_mut(),
            oauth_client_id: SiftStr::new(client),
            registered_schemes: SiftStr::new(schemes),
        };
        let status = unsafe { sift_initialize(callbacks(), init, &raw mut app) };
        assert_eq!(status, SiftStatus::Ok);
        app
    }

    fn scheme_of(app: *mut SiftApp) -> String {
        let mut out = SiftStr::new("");
        assert_eq!(
            unsafe { sift_callback_scheme(app, &raw mut out) },
            SiftStatus::Ok
        );
        // SAFETY: the layer holds the string until the next call that asks for one, and this
        // test makes no such call before reading it.
        unsafe { out.as_str() }
            .expect("the scheme is UTF-8")
            .to_owned()
    }

    #[test]
    fn the_callback_scheme_a_shell_is_given_is_the_one_the_flow_declares() {
        // D-17: one derivation. A shell that computed this itself would be a second answer to
        // the question of which scheme a provider accepts, and the two would drift the first
        // time a provider changed its mind — silently, because the symptom is a browser page
        // the user reaches *after* granting consent.
        for client in [
            "123456-abcdef.apps.googleusercontent.com",
            "a-client-that-names-its-own-redirect",
        ] {
            let app = start_configured(client, "");
            let scheme = scheme_of(app);
            assert_eq!(
                scheme,
                sift_foundation::identifiers::callback_scheme_for(client)
            );
            // And it is the prefix of the redirect the authorization URL actually carries, so
            // a shell registering this scheme registers the address the provider will use.
            assert!(
                sift_app::authorize::redirect_uri(client).starts_with(&format!("{scheme}:")),
                "{scheme} is not what the redirect is built from"
            );
            assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
        }
    }

    #[test]
    fn a_bundle_that_does_not_claim_the_scheme_cannot_begin_an_authorization() {
        // **This is the bug.** The shell used to pass a hardcoded `true` here, so a bundle
        // shipped without the derived scheme still opened a browser, and the user granted
        // consent and came back to a page saying no application would open the address.
        let client = "123456-abcdef.apps.googleusercontent.com";
        let app = start_configured(client, "net.justinchung.sift");
        let mut url = SiftStr::new("");
        assert_eq!(
            unsafe { sift_begin_authorization(app, &raw mut url) },
            SiftStatus::Failed,
            "an authorization began against a scheme the bundle does not claim"
        );
        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn the_same_bundle_claiming_the_derived_scheme_may_begin_one() {
        // The other half, so the refusal above is the check working rather than the flow
        // being broken for some unrelated reason.
        let client = "123456-abcdef.apps.googleusercontent.com";
        let app = start_configured(
            client,
            "net.justinchung.sift\ncom.googleusercontent.apps.123456-abcdef",
        );
        let mut url = SiftStr::new("");
        assert_eq!(
            unsafe { sift_begin_authorization(app, &raw mut url) },
            SiftStatus::Ok
        );
        // SAFETY: the layer holds the URL until the flow ends or another begins.
        let url = unsafe { url.as_str() }.expect("the URL is UTF-8");
        assert!(
            url.contains("code_challenge") && !url.contains("client_secret"),
            "{url}"
        );
        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn a_build_with_no_client_configured_refuses_rather_than_asking_for_nothing() {
        // A build with no client runs against the recorded corpus and says so. Beginning an
        // authorization with an empty client identifier would produce an authorization URL a
        // provider rejects, which is a worse way to say the same thing.
        let app = start_configured("", "net.justinchung.sift");
        assert_eq!(scheme_of(app), "");
        let mut url = SiftStr::new("");
        assert_eq!(
            unsafe { sift_begin_authorization(app, &raw mut url) },
            SiftStatus::Failed
        );
        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn initialization_returns_a_status_rather_than_a_sentinel() {
        // No entry point encodes failure in its return value's domain.
        let app = start(drop_it, scratch_str());
        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn a_layer_with_no_container_refuses_rather_than_choosing_one() {
        // The container is the shell's to name. A layer that fell back to a path of its own
        // would be wrong under the sandbox and under Flatpak, and would put a user's mail
        // somewhere nobody chose.
        ephemeral();
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let init = SiftInit {
            container_root: SiftStr::new(""),
            schedule: drop_it,
            schedule_context: core::ptr::null_mut(),
            oauth_client_id: SiftStr::new(""),
            registered_schemes: SiftStr::new(""),
        };
        assert_eq!(
            unsafe { sift_initialize(callbacks(), init, &raw mut app) },
            SiftStatus::Failed
        );
        assert!(app.is_null(), "a refused initialization wrote a handle");
    }

    /// A selection-scoped action with nothing selected is **not available**, and invoking it
    /// is an identified failure.
    ///
    /// This test used to assert `Ok`, and it passed for the wrong reason: the entry point
    /// validated the identifier and did nothing at all. An action that never runs is
    /// unfalsifiably "accepted".
    #[test]
    fn a_selection_action_with_nothing_selected_is_refused_rather_than_accepted() {
        let app = start(drop_it, scratch_str());
        assert_eq!(do_action(app, "message.archive"), SiftStatus::Failed);
        let _ = unsafe { sift_shutdown(app) };
    }

    /// The gesture, end to end across the boundary: select, archive, and see it durably
    /// enqueued and applied optimistically before any round trip.
    #[test]
    fn invoking_an_action_enqueues_a_gesture_and_hides_the_row_before_any_round_trip() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        assert_eq!(
            unsafe { sift_select(app, &raw const message, 1) },
            SiftStatus::Ok
        );

        let mut available = 0u8;
        let id = "message.archive";
        assert_eq!(
            unsafe { sift_action_available(app, id.as_ptr(), id.len(), &raw mut available) },
            SiftStatus::Ok
        );
        assert_eq!(available, 1, "a selected message with a provider behind it");

        let mut gesture = SiftGesture {
            mutated: 0,
            enqueued: 0,
            skipped: 0,
            optimistic: 0,
        };
        assert_eq!(
            unsafe {
                sift_invoke_action(
                    app,
                    id.as_ptr(),
                    id.len(),
                    core::ptr::null(),
                    0,
                    0,
                    &raw mut gesture,
                )
            },
            SiftStatus::Ok
        );
        assert_eq!(gesture.mutated, 1);
        assert_eq!(gesture.enqueued, 1);
        assert_eq!(gesture.skipped, 0);
        assert_eq!(gesture.optimistic, 1, "NFR-7: reflected before any network");

        // D-51: everything the user sees reads *through* the overlay, so the archived row is
        // absent from the projection rather than marked in it.
        let mut oldest = Oldest {
            at: u64::MAX,
            id: 0,
        };
        let mut observation = SiftObservation::NONE;
        let _ = unsafe {
            sift_observe_messages(
                app,
                SiftId::from_u128(0),
                50,
                keep_oldest,
                (&raw mut oldest).cast::<c_void>(),
                &raw mut observation,
            )
        };
        assert_ne!(
            oldest.id,
            message.to_u128(),
            "the archived message is still the oldest row visible"
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-14's single exception, on the boundary. Permanent deletion is confirmed **before**
    /// it is issued rather than undone after, because there is nothing to undo once it has
    /// happened — so an unconfirmed invocation must fail rather than enqueue.
    #[test]
    fn permanent_deletion_is_refused_without_confirmation() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let _ = unsafe { sift_select(app, &raw const message, 1) };
        // The replayed provider declares it cannot permanently delete at all, which is the
        // stronger of the two refusals and the one that fires first. Either way the gesture
        // does not reach the queue, which is the property.
        assert_eq!(
            do_action(app, "message.permanently-delete"),
            SiftStatus::Failed
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    /// A `Window`-scoped action is unavailable until the shell says a window exists. FR-25
    /// makes closing a window and quitting different acts, so this is a fact the shell owns.
    #[test]
    fn a_window_scoped_action_waits_for_the_shell_to_say_there_is_a_window() {
        let app = start(drop_it, scratch_str());
        let id = "navigate.unified-inbox";
        let mut available = 1u8;
        let _ = unsafe { sift_action_available(app, id.as_ptr(), id.len(), &raw mut available) };
        assert_eq!(available, 0, "no window has been declared");

        assert_eq!(unsafe { sift_set_window_present(app, 1) }, SiftStatus::Ok);
        let _ = unsafe { sift_action_available(app, id.as_ptr(), id.len(), &raw mut available) };
        assert_eq!(available, 1);
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn an_action_invoked_without_a_layer_is_a_failure_rather_than_a_crash() {
        let id = "message.archive";
        assert_eq!(do_action(core::ptr::null_mut(), id), SiftStatus::Failed);
    }

    #[test]
    fn shutting_down_reclaims_a_delivery_the_shell_never_ran() {
        // D-70's teardown is bounded and waits for nothing. A shell whose window closed
        // between the post and the turn of its loop leaves a ticket behind, and NFR-12
        // finds a leak of those in fourteen days.
        let app = start(drop_it, scratch_str());
        let mut observation = SiftObservation::NONE;
        let status = unsafe {
            sift_observe_messages(
                app,
                SiftId::from_u128(0),
                50,
                noop_rows,
                core::ptr::null_mut(),
                &raw mut observation,
            )
        };
        assert_eq!(status, SiftStatus::Ok);
        assert!(observation.is_valid());
        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn cancelling_an_observation_the_layer_does_not_have_is_a_failure() {
        let app = start(drop_it, scratch_str());
        assert_eq!(
            unsafe { sift_cancel_observation(app, SiftObservation(9_999)) },
            SiftStatus::Failed
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn a_delivery_reaches_the_shells_callback_and_carries_its_observation() {
        // The whole boundary, end to end: register, let the hop run, and see the callback
        // arrive with the handle that identifies which observation it belongs to.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEEN: AtomicU64 = AtomicU64::new(0);

        extern "C" fn record(
            _: *mut c_void,
            observation: SiftObservation,
            _: Generation,
            _: SiftRows<'_, SiftMessageRow<'_>>,
        ) {
            SEEN.store(observation.0, Ordering::SeqCst);
        }

        SEEN.store(0, Ordering::SeqCst);
        let app = start(run_inline, scratch_str());
        let mut observation = SiftObservation::NONE;
        let _ = unsafe {
            sift_observe_messages(
                app,
                SiftId::from_u128(0),
                50,
                record,
                core::ptr::null_mut(),
                &raw mut observation,
            )
        };
        // With no accounts there is nothing to deliver, so the callback must NOT have fired:
        // a delivery is a change, and an empty application has none.
        assert_eq!(
            SEEN.load(Ordering::SeqCst),
            0,
            "an empty application delivered a batch"
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn an_action_the_register_does_not_know_is_an_identified_failure() {
        // Not a panic, and not a silent success. A shell built against a newer register is a
        // version skew D-2 removed as a category, but the boundary still answers honestly.
        let id = "message.compose";
        let status = do_action(core::ptr::null_mut(), id);
        assert_eq!(status, SiftStatus::Failed);
    }

    #[test]
    fn invalid_utf8_on_the_boundary_fails_rather_than_panicking() {
        let bytes = [0xFFu8, 0xFE];
        let mut gesture = SiftGesture {
            mutated: 0,
            enqueued: 0,
            skipped: 0,
            optimistic: 0,
        };
        let status = unsafe {
            sift_invoke_action(
                core::ptr::null_mut(),
                bytes.as_ptr(),
                2,
                core::ptr::null(),
                0,
                0,
                &raw mut gesture,
            )
        };
        assert_eq!(status, SiftStatus::Failed);
    }

    #[test]
    fn a_null_string_is_a_failure_rather_than_a_dereference() {
        let mut gesture = SiftGesture {
            mutated: 0,
            enqueued: 0,
            skipped: 0,
            optimistic: 0,
        };
        let status = unsafe {
            sift_invoke_action(
                core::ptr::null_mut(),
                core::ptr::null(),
                0,
                core::ptr::null(),
                0,
                0,
                &raw mut gesture,
            )
        };
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
        let app = start(drop_it, scratch_str());
        let mut observation = SiftObservation::NONE;
        let status = unsafe {
            sift_observe_messages(
                app,
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
            unsafe { sift_cancel_observation(app, observation) },
            SiftStatus::Ok
        );
        // Cancelling the same observation twice is a failure, not a second success: the
        // shell's handle is stale and it has to be told rather than reassured.
        assert_eq!(
            unsafe { sift_cancel_observation(app, observation) },
            SiftStatus::Failed
        );
        let _ = unsafe { sift_shutdown(app) };
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

    // -----------------------------------------------------------------------------------
    // The reader's chrome, across the boundary a shell actually calls.
    //
    // The AppKit code that draws these is not testable here; what *is* testable is that the
    // boundary hands it the right answers, which is where a mistake would be silent. The
    // replayed corpus carries a deliberately hostile message — a click wrapper, a homograph
    // host, a `mailto:` unsubscribe, and an attachment declared `application/pdf`, named with
    // U+202E so it renders `invoice.pdf`, carrying a PE header.
    // -----------------------------------------------------------------------------------

    /// Where [`keep_oldest`] accumulates. Travels as the callback's context.
    struct Oldest {
        at: u64,
        id: u128,
    }

    extern "C" fn keep_oldest(
        context: *mut c_void,
        _: SiftObservation,
        _: Generation,
        rows: SiftRows<'_, SiftMessageRow<'_>>,
    ) {
        // SAFETY: the context is a live `Oldest` for the duration of the call that posted
        // this delivery, and the rows are live for the duration of the callback.
        let oldest = unsafe { &mut *context.cast::<Oldest>() };
        for row in unsafe { rows.as_slice() } {
            if row.received_millis < oldest.at {
                oldest.at = row.received_millis;
                oldest.id = u128::from_be_bytes(row.id.bytes);
            }
        }
    }

    /// Add the replayed account, sync it, and hand back the hostile message's identity.
    ///
    /// The layer must have been started with [`run_inline`], because the delivery this reads
    /// arrives through D-48's hop and a schedule that drops the ticket delivers nothing.
    fn hostile_message(app: *mut SiftApp) -> SiftId {
        let name = "mail";
        let mut account = SiftId::from_u128(0);
        assert_eq!(
            unsafe { sift_add_replayed_account(app, name.as_ptr(), name.len(), &raw mut account) },
            SiftStatus::Ok
        );
        assert_eq!(
            unsafe { sift_sync_account(app, name.as_ptr(), name.len()) },
            SiftStatus::Ok
        );

        // The oldest of the three, which is the hostile one. Read through the same row
        // projection the list draws from rather than through a test-only path — the point of
        // driving the boundary is that nothing here is a second way in.
        //
        // The accumulator travels as the callback's own context rather than as a static,
        // because these tests run in parallel and a static would make them one test with a
        // race in it.
        let mut oldest = Oldest {
            at: u64::MAX,
            id: 0,
        };
        let mut observation = SiftObservation::NONE;
        assert_eq!(
            unsafe {
                sift_observe_messages(
                    app,
                    SiftId::from_u128(0),
                    50,
                    keep_oldest,
                    (&raw mut oldest).cast::<c_void>(),
                    &raw mut observation,
                )
            },
            SiftStatus::Ok
        );
        assert_ne!(oldest.at, u64::MAX, "the observation delivered no rows");
        let _ = unsafe { sift_cancel_observation(app, observation) };
        SiftId::from_u128(oldest.id)
    }

    fn open(app: *mut SiftApp, message: SiftId) -> SiftDocument<'static> {
        let mut document = SiftDocument {
            html: SiftStr::null(),
            token: SiftStr::null(),
            fetching_positions: 0,
            blocked: 0,
            links: 0,
            may_always_allow: 0,
            has_unsubscribe: 0,
        };
        assert_eq!(
            unsafe { sift_open_document(app, message, 0, &raw mut document) },
            SiftStatus::Ok
        );
        document
    }

    fn text(s: SiftStr<'_>) -> String {
        // SAFETY: every call site reads a value whose document or listing is still open,
        // which is the liveness half the accessor cannot check for itself.
        unsafe { s.as_str() }.unwrap_or_default().to_owned()
    }

    #[test]
    fn a_documents_counts_distinguish_all_withheld_from_some_withheld() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let document = open(app, message);

        assert!(
            document.fetching_positions > 0,
            "there is something to refuse"
        );
        assert_eq!(
            document.fetching_positions, document.blocked,
            "with no filter list loaded, an absent authority denies every one"
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn the_withheld_disclosure_names_the_shed_rather_than_a_rule_that_never_ran() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let document = open(app, message);
        let token = text(document.token);

        let mut rows = SiftRows::empty();
        assert_eq!(
            unsafe { sift_document_withheld(app, token.as_ptr(), token.len(), &raw mut rows) },
            SiftStatus::Ok
        );
        assert_eq!(rows.len(), document.blocked as usize);

        // SAFETY: the document is open.
        let row = &unsafe { rows.as_slice() }[0];
        assert_eq!(text(row.element), "img");
        assert_eq!(text(row.attribute), "src");
        assert!(text(row.displayed).contains("beacon.tracker.test"));
        assert!(
            text(row.rule).contains("no filter list is loaded"),
            "a user who believes a rule matched believes in a protection that is absent"
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    /// The control is **absent** rather than disabled. An allowance keyed on nothing applies
    /// to every sender, which is the opposite of what the button says it does.
    #[test]
    fn the_durable_allowance_is_not_offered_when_there_is_nothing_to_key_it_on() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        assert_eq!(open(app, message).may_always_allow, 0);
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-8's *load once*, on the boundary.
    ///
    /// The bar's two buttons were assigned to nothing in the macOS shell — `onLoadOnce` and
    /// `onAlwaysAllow` were declared and never set — so a user could read that content was
    /// withheld and had no way to accept it. There was also no entry point to assign them
    /// to: the broker could allow an origin and nothing across the boundary could ask it to.
    #[test]
    fn loading_once_is_scoped_to_the_open_document() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let document = open(app, message);
        let token = text(document.token);

        assert_eq!(
            unsafe { sift_allow_remote_content(app, token.as_ptr(), token.len(), 0) },
            SiftStatus::Ok,
            "the open document could not be allowed to load"
        );

        // The same message, opened again, is a new token and a new decision. A load-once that
        // outlived its document would be a durable allowance the user never asked for and
        // cannot find to revoke.
        let again = open(app, message);
        let fresh = text(again.token);
        assert_ne!(fresh, token, "re-opening reused the token");
        assert!(
            again.blocked > 0,
            "content stayed allowed across a re-open, so `once` did not mean once"
        );

        let _ = unsafe { sift_shutdown(app) };
    }

    /// A durable allowance keyed on nothing would apply to every sender, so it must refuse
    /// rather than key on a placeholder. The interface is told this before the button is
    /// drawn — `may_always_allow` — and this is the boundary refusing anyway, because a shell
    /// that offered it regardless must not be able to make it happen.
    #[test]
    fn a_durable_allowance_is_refused_where_nothing_authenticated_the_sender() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let document = open(app, message);
        assert_eq!(document.may_always_allow, 0, "the fixture changed");
        let token = text(document.token);

        assert_eq!(
            unsafe { sift_allow_remote_content(app, token.as_ptr(), token.len(), 1) },
            SiftStatus::Failed,
            "an allowance was keyed on a sender nothing authenticated"
        );

        let _ = unsafe { sift_shutdown(app) };
    }

    /// A token the layer never minted, or one already revoked, is a failure rather than a
    /// silent success — the shell would otherwise report that content was allowed and show a
    /// document that still blocks it.
    #[test]
    fn allowing_an_unknown_token_fails() {
        let app = start(run_inline, scratch_str());
        let bogus = "not-a-token";
        assert_eq!(
            unsafe { sift_allow_remote_content(app, bogus.as_ptr(), bogus.len(), 0) },
            SiftStatus::Failed
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn a_link_crosses_with_its_wrapper_and_its_real_destination() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let document = open(app, message);
        let token = text(document.token);

        let mut rows = SiftRows::empty();
        assert_eq!(
            unsafe { sift_document_links(app, token.as_ptr(), token.len(), &raw mut rows) },
            SiftStatus::Ok
        );
        assert_eq!(rows.len(), document.links as usize);
        // SAFETY: the document is open.
        let links = unsafe { rows.as_slice() };

        let wrapped = links
            .iter()
            .find(|l| !l.wrapper.is_null())
            .expect("the wrapped link");
        assert_eq!(text(wrapped.target), "https://example.test/offer");
        assert!(text(wrapped.wrapper).contains("click.tracker.test"));

        // The falsifier: a wrapper carrying nothing recoverable stays unresolved. A Sift that
        // resolved wrappers by *fetching* them would resolve this one too.
        let opaque = links
            .iter()
            .find(|l| text(l.target).contains("/x/9f2c41"))
            .expect("the opaque wrapper");
        assert!(
            opaque.wrapper.is_null(),
            "it claims to have unwrapped nothing"
        );

        // A punycode label is marked rather than rendered: a user cannot compare two strings
        // they are only shown one of.
        let homograph = links
            .iter()
            .find(|l| text(l.target).contains("xn--"))
            .expect("the homograph host");
        assert!(text(homograph.displayed).contains("[80ak6aa92e]"));
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-42. Shown, reported as needing a mail handler, and never sent — nothing on this
    /// boundary can be made to issue the request.
    #[test]
    fn the_unsubscribe_destination_crosses_and_says_it_needs_a_mail_handler() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let document = open(app, message);
        assert_eq!(document.has_unsubscribe, 1);
        let token = text(document.token);

        let mut link = SiftLink {
            displayed: SiftStr::null(),
            target: SiftStr::null(),
            wrapper: SiftStr::null(),
            needs_a_mail_handler: 0,
        };
        assert_eq!(
            unsafe { sift_document_unsubscribe(app, token.as_ptr(), token.len(), &raw mut link) },
            SiftStatus::Ok
        );
        assert!(text(link.target).starts_with("mailto:"));
        assert_eq!(link.needs_a_mail_handler, 1);
        let _ = unsafe { sift_shutdown(app) };
    }

    /// The rows borrow from the document, and closing it is what tells the shell to stop
    /// reading them. A read after the close must fail rather than hand back a dangling slice.
    #[test]
    fn closing_a_document_takes_its_rows_with_it() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let token = text(open(app, message).token);

        assert_eq!(
            unsafe { sift_close_document(app, token.as_ptr(), token.len()) },
            SiftStatus::Ok
        );
        let mut rows = SiftRows::empty();
        assert_eq!(
            unsafe { sift_document_withheld(app, token.as_ptr(), token.len(), &raw mut rows) },
            SiftStatus::Failed
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-10's three sources, and the one a type check never sees. The declared type is
    /// `application/pdf`, the name renders `invoice.pdf`, and the extension the platform acts
    /// on is `.exe`.
    #[test]
    fn an_attachment_crosses_with_the_warning_a_type_check_would_have_missed() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);

        let mut rows = SiftRows::empty();
        assert_eq!(
            unsafe { sift_message_attachments(app, message, &raw mut rows) },
            SiftStatus::Ok
        );
        assert_eq!(rows.len(), 1);
        // SAFETY: the listing is held by the layer until the next call for this message.
        let row = &unsafe { rows.as_slice() }[0];

        assert_eq!(text(row.media_type), "application/pdf");
        assert_eq!(text(row.file_name), "invoicefdp.exe");
        assert!(
            !text(row.file_name).contains('\u{202E}'),
            "the override is gone from the name a save would use"
        );
        assert!(
            text(row.display_name).contains('\u{202E}'),
            "and is *kept* in the display name, which NFR-54 isolates rather than strips"
        );
        assert_ne!(row.warning & SIFT_WARN_EXTENSION, 0);
        assert_ne!(row.warning & SIFT_WARN_DISAGREES, 0);
        let _ = unsafe { sift_shutdown(app) };
    }

    /// NFR-53: the exact final path is resolved and shown **before** the write, and the write
    /// takes the plan rather than a path — so what is written is what was shown.
    #[test]
    fn planning_a_save_writes_nothing_and_naming_the_plan_is_what_writes() {
        let directory = scratch();
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let part = "2";
        let path = directory.to_str().expect("utf-8");

        let mut plan = SiftSavePlan {
            plan: 0,
            final_path: SiftStr::null(),
            renamed: 0,
            declared_size: 0,
        };
        assert_eq!(
            unsafe {
                sift_plan_attachment_save(
                    app,
                    message,
                    part.as_ptr(),
                    part.len(),
                    path.as_ptr(),
                    path.len(),
                    &raw mut plan,
                )
            },
            SiftStatus::Ok
        );
        let shown = text(plan.final_path);
        assert!(shown.ends_with("invoicefdp.exe"), "{shown}");
        assert_eq!(plan.renamed, 1, "the name was derived, and it says so");
        assert_eq!(
            std::fs::read_dir(&directory).expect("readable").count(),
            0,
            "planning wrote a file"
        );

        let mut outcome = SiftSaveOutcome {
            written: 0,
            warning: 0,
        };
        assert_eq!(
            unsafe { sift_write_attachment(app, plan.plan, &raw mut outcome) },
            SiftStatus::Ok
        );
        assert_eq!(
            std::fs::read(std::path::Path::new(&shown))
                .expect("written")
                .get(..2),
            Some(b"MZ".as_slice()),
            "written to exactly the path that was shown"
        );
        // The content is the fourth source, and it only exists once the bytes are here.
        assert_ne!(outcome.warning & SIFT_WARN_DECLARED, 0);

        // The plan is consumed: one plan writes one file, so a repeat cannot overwrite it.
        assert_eq!(
            unsafe { sift_write_attachment(app, plan.plan, &raw mut outcome) },
            SiftStatus::Failed
        );
        let _ = unsafe { sift_shutdown(app) };
        std::fs::remove_dir_all(&directory).ok();
    }

    /// FR-34's queue, across the boundary — and the sentence a person opens the runtime panel
    /// to read: everything is `Pending`, so nothing has been sent.
    #[test]
    fn the_queue_shows_a_watched_accounts_triage_as_recorded_and_unsent() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let _ = unsafe { sift_select(app, &raw const message, 1) };
        assert_eq!(do_action(app, "message.archive"), SiftStatus::Ok);

        let name = "mail";
        let mut rows = SiftRows::empty();
        assert_eq!(
            unsafe { sift_queue(app, name.as_ptr(), name.len(), &raw mut rows) },
            SiftStatus::Ok
        );
        assert_eq!(rows.len(), 1);
        // SAFETY: the table is held by the layer until the next call.
        let row = &unsafe { rows.as_slice() }[0];
        assert_eq!(text(row.intent), "archive");
        assert_eq!(
            text(row.state),
            "Pending",
            "an account that is only watched must show nothing as issued"
        );
        assert_eq!(row.message, message);
        let _ = unsafe { sift_shutdown(app) };
    }

    /// D-24's attribution is a tagging allocator, so the numbers do not add up to the process
    /// total — and reporting them as though they did would paper over exactly the gap the
    /// tagging exists to measure.
    #[test]
    fn memory_is_reported_per_subsystem_and_the_total_is_reported_beside_it() {
        let app = start(drop_it, scratch_str());
        let mut rows = SiftRows::empty();
        assert_eq!(unsafe { sift_memory(app, &raw mut rows) }, SiftStatus::Ok);
        // SAFETY: the table is held by the layer.
        let listed = unsafe { rows.as_slice() };
        assert_eq!(listed.len(), sift_subsystem::Subsystem::ALL.len() + 1);
        assert_eq!(text(listed[listed.len() - 1].name), "attributed");
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-33 item 1: which stages ran, for a message that is open.
    #[test]
    fn the_debug_view_can_read_which_stages_ran_over_an_open_document() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        let token = text(open(app, message).token);

        let mut rows = SiftRows::empty();
        assert_eq!(
            unsafe { sift_document_stages(app, token.as_ptr(), token.len(), &raw mut rows) },
            SiftStatus::Ok
        );
        // SAFETY: the document is open.
        let stages: Vec<String> = unsafe { rows.as_slice() }
            .iter()
            .map(|s| text(*s))
            .collect();
        assert!(stages.contains(&"sanitize".to_owned()), "{stages:?}");
        assert!(stages.contains(&"filter".to_owned()), "{stages:?}");
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-20's operators, executed, with the two things a shell must show beside the results.
    #[test]
    fn a_search_carries_how_it_was_read_and_what_it_could_not_answer() {
        let app = start(run_inline, scratch_str());
        let _ = hostile_message(app);

        let query = "from:someone has:attachment";
        let mut found = SiftSearch {
            rows: SiftRows::empty(),
            interpretation: SiftStr::null(),
            caveats: SiftStr::null(),
            delegable_accounts: 0,
        };
        assert_eq!(
            unsafe {
                sift_search(
                    app,
                    query.as_ptr(),
                    query.len(),
                    SiftId::from_u128(0),
                    50,
                    &raw mut found,
                )
            },
            SiftStatus::Ok
        );

        let read = text(found.interpretation);
        assert!(read.contains("from: someone"), "{read}");
        assert!(read.contains("with an attachment"), "{read}");

        // The caveat is the point. Without it, "no results" and "that filter was never
        // evaluated" are the same sentence to a user.
        let caveats = text(found.caveats);
        assert!(
            caveats.contains("has:attachment"),
            "an unevaluated filter said nothing about itself: {caveats:?}"
        );
        let _ = unsafe { sift_shutdown(app) };
    }

    /// A typo that looks like an operator is read as text, and **says so**. `form:alice` is a
    /// plausible mistake for `from:alice`, and the two produce very different result sets.
    #[test]
    fn an_operator_this_build_does_not_know_is_read_as_text_and_reported_as_such() {
        let app = start(run_inline, scratch_str());
        let _ = hostile_message(app);
        let query = "form:alice";
        let mut found = SiftSearch {
            rows: SiftRows::empty(),
            interpretation: SiftStr::null(),
            caveats: SiftStr::null(),
            delegable_accounts: 0,
        };
        let _ = unsafe {
            sift_search(
                app,
                query.as_ptr(),
                query.len(),
                SiftId::from_u128(0),
                50,
                &raw mut found,
            )
        };
        let read = text(found.interpretation);
        assert!(read.contains("read as text, not as an operator"), "{read}");
        let _ = unsafe { sift_shutdown(app) };
    }

    /// FR-21's label. Nothing delegates yet, and the count says so rather than the absence of
    /// server results implying it.
    #[test]
    fn a_search_says_that_nothing_was_asked_of_a_provider() {
        let app = start(run_inline, scratch_str());
        let _ = hostile_message(app);
        let query = "receipt";
        let mut found = SiftSearch {
            rows: SiftRows::empty(),
            interpretation: SiftStr::null(),
            caveats: SiftStr::null(),
            delegable_accounts: 0,
        };
        let _ = unsafe {
            sift_search(
                app,
                query.as_ptr(),
                query.len(),
                SiftId::from_u128(0),
                50,
                &raw mut found,
            )
        };
        assert_eq!(found.delegable_accounts, 0);
        assert!(!found.rows.is_empty(), "the local search found nothing");
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn the_account_list_names_what_a_shell_must_name_to_reach_it() {
        let app = start(drop_it, scratch_str());
        let name = "mail";
        let mut id = SiftId::from_u128(0);
        assert_eq!(
            unsafe { sift_add_replayed_account(app, name.as_ptr(), name.len(), &raw mut id) },
            SiftStatus::Ok
        );

        let mut rows = SiftRows::<SiftAccount<'static>>::empty();
        assert_eq!(unsafe { sift_accounts(app, &raw mut rows) }, SiftStatus::Ok);
        let listed = unsafe { rows.as_slice() };
        assert_eq!(
            listed.len(),
            1,
            "one account was added and one should be listed"
        );

        let row = &listed[0];
        assert_eq!(unsafe { row.name.as_str() }, Some(name));
        assert_eq!(
            row.id.bytes, id.bytes,
            "the listed identity must be the one adding it handed back, or an observation \
             anchored on it would watch a different account"
        );
        assert_eq!(
            row.writes_enabled, 0,
            "an account is added watching and nothing else"
        );
        assert_eq!(row.held, 0, "nothing has been queued yet");

        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn an_account_that_was_never_added_is_not_listed() {
        let app = start(drop_it, scratch_str());
        let mut rows = SiftRows::<SiftAccount<'static>>::empty();
        assert_eq!(unsafe { sift_accounts(app, &raw mut rows) }, SiftStatus::Ok);
        assert_eq!(
            unsafe { rows.as_slice() }.len(),
            0,
            "the account-less state is what makes the add-account flow the right first screen"
        );
        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    /// A flush against the recorded corpus, before and after the posture is changed.
    ///
    /// The corpus is a real adapter over recorded exchanges, so this drives the same path a
    /// real mailbox does — including the one step an account that is only watched skips.
    #[test]
    fn a_watched_account_issues_nothing_and_says_how_much_it_is_holding() {
        let app = start(run_inline, scratch_str());
        let name = "mail";
        let message = hostile_message(app);

        assert_eq!(
            unsafe { sift_select(app, &raw const message, 1) },
            SiftStatus::Ok
        );
        assert_eq!(do_action(app, "message.archive"), SiftStatus::Ok);

        let mut flushed = SiftFlush {
            authorized: 1,
            held: 0,
            issued: 0,
            applied: 0,
            refused: 0,
            deferred: 0,
            reconciling: 0,
            quarantined: 0,
            queued: 0,
            failed: 0,
        };
        assert_eq!(
            unsafe { sift_flush_account(app, name.as_ptr(), name.len(), &raw mut flushed) },
            SiftStatus::Ok
        );
        assert_eq!(
            flushed.authorized, 0,
            "an account is added watching, so the one step that cannot be taken back is the \
             one step it does not take"
        );
        assert_eq!(
            flushed.issued, 0,
            "nothing may leave an unauthorized account"
        );
        assert_eq!(
            flushed.held, 1,
            "the intent is durably recorded and held, and the count is what makes that \
             checkable rather than something a user is told"
        );

        assert_eq!(
            unsafe { sift_set_writes_enabled(app, name.as_ptr(), name.len(), 1) },
            SiftStatus::Ok
        );
        let mut rows = SiftRows::<SiftAccount<'static>>::empty();
        assert_eq!(unsafe { sift_accounts(app, &raw mut rows) }, SiftStatus::Ok);
        assert_eq!(
            unsafe { rows.as_slice() }[0].writes_enabled,
            1,
            "the posture the list reports is the one that was just set"
        );

        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn undo_reverses_the_gesture_the_register_could_only_report() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);
        assert_eq!(
            unsafe { sift_select(app, &raw const message, 1) },
            SiftStatus::Ok
        );
        assert_eq!(do_action(app, "message.archive"), SiftStatus::Ok);

        let mut undoable = SiftUndoable {
            messages: 0,
            timed: 0,
            remaining_millis: 0,
            intent: SiftStr::new(""),
        };
        assert_eq!(
            unsafe { sift_undoable(app, &raw mut undoable) },
            SiftStatus::Ok
        );
        assert_eq!(
            undoable.messages, 1,
            "an archive over one message is reversible"
        );

        let mut gesture = SiftGesture {
            mutated: 0,
            enqueued: 0,
            skipped: 0,
            optimistic: 0,
        };
        assert_eq!(
            unsafe { sift_undo_last(app, &raw mut gesture) },
            SiftStatus::Ok
        );
        assert_eq!(
            gesture.enqueued, 1,
            "the reversal is its own gesture with its own intent — invoking the register \
             entry returned success and enqueued nothing"
        );

        // Nothing to undo is an identified failure rather than a zero row: the affordance
        // is absent, and `SiftUndoable` has no way to say "none" that a shell could not
        // mistake for a gesture over no messages.
        assert_eq!(
            unsafe { sift_undoable(app, &raw mut undoable) },
            SiftStatus::Failed,
            "the record is spent once it has been used"
        );

        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    /// D-98's `OpenMessage` scope, which nothing ever satisfied.
    ///
    /// The field behind it was initialized to `None` and assigned nowhere, so every action
    /// scoped to an open message was absent from every menu and every palette for the life of
    /// the process — and the register's reconciliation could not see it, because both sides
    /// agreed the identifiers existed. Only availability disagreed, with nobody.
    #[test]
    fn opening_a_message_is_what_makes_the_open_message_scope_true() {
        let app = start(run_inline, scratch_str());
        let message = hostile_message(app);

        let mut available: u8 = 1;
        let id = "read.toggle-dark-transform";
        assert_eq!(
            unsafe { sift_action_available(app, id.as_ptr(), id.len(), &raw mut available) },
            SiftStatus::Ok
        );
        assert_eq!(
            available, 0,
            "nothing is open yet, so an action over an open message must be absent"
        );

        let mut document = SiftDocument {
            html: SiftStr::new(""),
            token: SiftStr::new(""),
            fetching_positions: 0,
            blocked: 0,
            links: 0,
            may_always_allow: 0,
            has_unsubscribe: 0,
        };
        assert_eq!(
            unsafe { sift_open_document(app, message, 0, &raw mut document) },
            SiftStatus::Ok
        );
        let token = unsafe { document.token.as_str() }
            .expect("a token")
            .to_owned();

        assert_eq!(
            unsafe { sift_action_available(app, id.as_ptr(), id.len(), &raw mut available) },
            SiftStatus::Ok
        );
        assert_eq!(
            available, 1,
            "a message is open, so the actions over one are reachable"
        );

        assert_eq!(
            unsafe { sift_close_document(app, token.as_ptr(), token.len()) },
            SiftStatus::Ok
        );
        assert_eq!(
            unsafe { sift_action_available(app, id.as_ptr(), id.len(), &raw mut available) },
            SiftStatus::Ok
        );
        assert_eq!(
            available, 0,
            "a reader that closed its message must not go on offering gestures over it"
        );

        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    /// D-67's callbacks were registered and never invoked.
    ///
    /// The set was stored whole at initialization behind an `allow(dead_code)`, and nothing on
    /// the layer's side ever called one — so FR-2's requirement that re-authentication reach
    /// the user *with no window open* could not be met by any shell, because the only thing
    /// that could have woken one never fired.
    #[test]
    fn a_condition_that_changes_reaches_the_shell_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEEN: AtomicU32 = AtomicU32::new(0);
        static CONDITION: AtomicU32 = AtomicU32::new(u32::MAX);

        extern "C" fn note(_: *mut c_void, _: SiftId, condition: u32) {
            SEEN.fetch_add(1, Ordering::Relaxed);
            CONDITION.store(condition, Ordering::Relaxed);
        }

        ephemeral();
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let mut callbacks = callbacks();
        callbacks.account_condition_changed = note;
        let init = SiftInit {
            container_root: SiftStr::new(scratch_str()),
            schedule: run_inline,
            schedule_context: core::ptr::null_mut(),
            oauth_client_id: SiftStr::new(""),
            registered_schemes: SiftStr::new(""),
        };
        assert_eq!(
            unsafe { sift_initialize(callbacks, init, &raw mut app) },
            SiftStatus::Ok
        );

        let message = hostile_message(app);
        assert!(
            SEEN.load(Ordering::Relaxed) > 0,
            "an account that appeared is a condition the shell has never been told"
        );
        assert_eq!(
            CONDITION.load(Ordering::Relaxed),
            SiftCondition::HEALTHY.0,
            "a freshly synced account with nothing queued is healthy"
        );

        // Triage on an account that is only being watched is a change, and it is the one a
        // person sees first: the queue grows, nothing leaves, and that is the state they
        // chose — which they have to be able to see they chose.
        assert_eq!(
            unsafe { sift_select(app, &raw const message, 1) },
            SiftStatus::Ok
        );
        assert_eq!(do_action(app, "message.archive"), SiftStatus::Ok);
        assert_eq!(
            CONDITION.load(Ordering::Relaxed),
            SiftCondition::ATTENTION.0,
            "the condition the shell was told is not the one that is now true"
        );

        // An unchanged condition is not re-announced. A callback that fires on every delivery
        // with the same answer is a wakeup NFR-11 counts and a badge that redraws for nothing.
        let before = SEEN.load(Ordering::Relaxed);
        let name = "mail";
        assert_eq!(
            unsafe { sift_sync_account(app, name.as_ptr(), name.len()) },
            SiftStatus::Ok
        );
        assert_eq!(
            SEEN.load(Ordering::Relaxed),
            before,
            "the same condition was announced twice"
        );

        assert_eq!(unsafe { sift_shutdown(app) }, SiftStatus::Ok);
    }

    #[test]
    fn shutdown_does_not_unwind() {
        assert_eq!(
            unsafe { sift_shutdown(core::ptr::null_mut()) },
            SiftStatus::Ok
        );
    }
}

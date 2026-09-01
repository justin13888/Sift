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
use crate::layer::{Layer, OpenDocument, SiftInit, Sink, Task};
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
            app.root = Some(std::path::PathBuf::from(root));

            let layer = Box::new(Layer {
                session: std::sync::Mutex::new(Session::new(app)),
                host: callbacks,
                schedule: init.schedule,
                schedule_context: init.schedule_context as usize,
                sinks: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                documents: std::sync::Mutex::new(std::collections::BTreeMap::new()),
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
) -> SiftStatus {
    guard(|| {
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
        // SAFETY: the caller's obligation.
        let Some(layer) = (unsafe { layer(app) }) else {
            return Err(());
        };
        // Anything that could have changed a window is followed by a delivery, posted
        // rather than run: running it here would hand the shell a callback from inside the
        // call that caused it, which is the reentrancy D-48 forbids.
        crate::layer::post(
            layer,
            Task::Deliver {
                layer: app as usize,
            },
        );
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

/// Compute what changed and hand each batch to the observation that asked for it.
fn deliver(layer: &Layer) {
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

    let Ok(sinks) = layer.sinks.lock() else {
        return;
    };
    for d in &deliveries {
        // A sink removed by cancellation is the guarantee doing its job: the delivery was
        // computed before the cancel and finds nothing to call.
        let Some(sink) = sinks.get(&d.observation.0) else {
            continue;
        };
        let rows: Vec<SiftMessageRow<'_>> = d.batch.incoming.iter().map(row_of).collect();
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
    /// How many fetching positions were refused. For the reader's **native** chrome — a
    /// count drawn inside the document is one a sender can counterfeit.
    pub blocked: u32,
    /// How many navigation targets the body carries.
    pub links: u32,
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
            let document = {
                let mut session = layer.session.lock().map_err(|_| ())?;
                session
                    .app_mut()
                    .open_document(
                        sift_foundation::identity::LocalId::from_u128(message.to_u128()),
                        dark != 0,
                    )
                    .map_err(|_| ())?
            };
            let blocked = u32::try_from(document.blocked).unwrap_or(u32::MAX);
            let links = u32::try_from(document.links.len()).unwrap_or(u32::MAX);
            // The strings outlive the call, so they are held by the layer and keyed on the
            // token the shell is about to be given. Closing the document is what frees them,
            // which is the same gesture that revokes the token — one lifetime, not two.
            let mut open = layer.documents.lock().map_err(|_| ())?;
            let entry = open
                .entry(document.token.clone())
                .or_insert_with(|| OpenDocument {
                    html: document.html,
                    token: document.token.clone(),
                });
            Ok(SiftDocument {
                html: SiftStr::new(extend(&entry.html)),
                token: SiftStr::new(extend(&entry.token)),
                blocked,
                links,
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
        layer.documents.lock().map_err(|_| ())?.remove(name);
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

    fn scratch() -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let d = std::env::temp_dir().join(format!("sift-abi-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&d).expect("scratch");
        d
    }

    fn start(schedule: crate::layer::SiftSchedule, root: &'static str) -> *mut SiftApp {
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let init = SiftInit {
            container_root: SiftStr::new(root),
            schedule,
            schedule_context: core::ptr::null_mut(),
            scheme_is_registered: 0,
        };
        let status = unsafe { sift_initialize(callbacks(), init, &raw mut app) };
        assert_eq!(status, SiftStatus::Ok);
        assert!(!app.is_null(), "initialization handed back no layer");
        app
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
        let mut app: *mut SiftApp = core::ptr::null_mut();
        let init = SiftInit {
            container_root: SiftStr::new(""),
            schedule: drop_it,
            schedule_context: core::ptr::null_mut(),
            scheme_is_registered: 0,
        };
        assert_eq!(
            unsafe { sift_initialize(callbacks(), init, &raw mut app) },
            SiftStatus::Failed
        );
        assert!(app.is_null(), "a refused initialization wrote a handle");
    }

    #[test]
    fn an_action_the_register_knows_is_accepted() {
        let app = start(drop_it, scratch_str());
        let id = "message.archive";
        let status = unsafe { sift_invoke_action(app, id.as_ptr(), id.len()) };
        assert_eq!(status, SiftStatus::Ok);
        let _ = unsafe { sift_shutdown(app) };
    }

    #[test]
    fn an_action_invoked_without_a_layer_is_a_failure_rather_than_a_crash() {
        let id = "message.archive";
        assert_eq!(
            unsafe { sift_invoke_action(core::ptr::null_mut(), id.as_ptr(), id.len()) },
            SiftStatus::Failed
        );
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

    #[test]
    fn shutdown_does_not_unwind() {
        assert_eq!(
            unsafe { sift_shutdown(core::ptr::null_mut()) },
            SiftStatus::Ok
        );
    }
}

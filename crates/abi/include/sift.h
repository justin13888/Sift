/*
 * Sift's C ABI — D-17's boundary.
 *
 * GENERATED FROM crates/abi/sift-abi. Do not edit.
 *
 * This file is committed deliberately. Committing generated output is widely disliked, and
 * D-60 accepts the dislike for two reasons: it keeps the Swift target independent of the
 * Rust toolchain in day-to-day shell work, and it makes every addition to the boundary
 * visible in review — which is what lets D-17's tripwire actually trip.
 *
 * Regenerating in CI and finding a difference fails the build.
 */


#ifndef SIFT_ABI_H
#define SIFT_ABI_H



#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * The result of every entry point.
 *
 * **No entry point encodes failure in its return value's domain.** Sentinel returns,
 * null-means-error, and a thread-local last-error are all excluded — the first two make
 * the failure case indistinguishable from a legal value at some point in the future, and
 * the third makes it depend on which thread asked.
 *
 * The status distinguishes **three** things rather than two, and the third is the point:
 * a caught panic is its own value, everywhere. D-47 catches panics at each pipeline stage
 * and no unwind may cross this boundary, so the shell has to be able to tell "this failed
 * in a way the design anticipated" from "this failed in a way it did not". Collapsing
 * them would make every panic look like an ordinary parse failure, which D-47 forbids
 * explicitly.
 */
typedef enum {
  /**
   * The call succeeded and any out-parameters have been written.
   */
  Ok = 0,
  /**
   * The call failed in an identified way. The identified state is delivered separately;
   * **a failure crossing this boundary is a state, never a message**.
   */
  Failed = 1,
  /**
   * A panic was caught at this entry point. Out-parameters are not written.
   *
   * Counted against the subsystem whose tag was current, per D-24, and never absorbed
   * as an ordinary failure.
   */
  Panicked = 2,
} SiftStatus;

/**
 * An opaque pointer the shell supplies at registration and receives back with every
 * callback. The layer never dereferences it.
 */
typedef void *HostContext;

/**
 * An identifier, as sixteen big-endian bytes.
 *
 * D-78's local identity is 128 bits, and **C has no portable 128-bit integer type** — so it
 * crosses as bytes. That is not a workaround: the identity was already defined big-endian
 * *so that lexicographic byte order is numeric order is time order*, because the value is an
 * index key in every account database. The same property is what lets a shell compare two of
 * these with `memcmp` and get D-55's tiebreak.
 *
 * D-78 records the cost this is part of: the width is paid in every index, every foreign
 * reference, **and every row crossing this boundary** — for a property only the
 * unified-inbox merge needs, and D-4 concedes that is the feature to cut.
 */
typedef struct {
  uint8_t bytes[16];
} SiftId;

/**
 * A UTF-8 string, as a pointer and a length.
 *
 * **Never NUL-terminated, and the length is authoritative** — nothing on this boundary
 * scans for a terminator. Two consequences the shells depend on:
 *
 * - **Every truncation on this boundary is one the presentation layer performed
 *   deliberately**, under a bound in `limits.md`. None is a property of the encoding. A
 *   NUL-terminated representation would let an embedded NUL — which a sender can put in a
 *   header — truncate a subject at a point nobody chose, which is precisely the
 *   attacker-chooses-the-cut problem the limits register rejects for documents.
 * - **Validity is established once**, where normalization happens, and is not re-checked
 *   by the shell.
 *
 * # Safety
 *
 * Valid only for the duration of the call or callback that delivered it.
 */
typedef struct {
  const uint8_t *ptr;
  size_t len;
} SiftStr;

/**
 * The six callbacks the shell registers, once, at initialization.
 *
 * Every field is required. There is no "optional callback": a shell that cannot destroy
 * its windows cannot honour L3, and a shell that cannot raise re-authentication has an
 * account that stalls silently — which is the failure FR-2 exists to prevent.
 */
typedef struct {
  /**
   * The shell's context, handed back to every callback below.
   */
  HostContext context;
  /**
   * **Destroy every window** — L3 in memory pressure.
   *
   * Destroys every window shell and its view hierarchy, leaving the application shell
   * that owns the always-on surface. Removing that too would leave the application
   * unreachable, so L3 is the deepest *in-process* shed and terminates nothing.
   *
   * Issued and never awaited: D-93's governor holds no lock a shed target needs and
   * waits for nothing, because waiting here would mean blocking on a main loop it does
   * not control — which, against D-48's synchronous cancellation, is a deadlock rather
   * than a delay.
   */
  void (*destroy_every_window)(HostContext);
  /**
   * **Re-authentication is needed** — FR-2.
   *
   * Raised through the always-on surface, because Sift may be resident with nothing on
   * screen. D-88's classifier is deliberately conservative about reaching here: only a
   * well-formed provider denial is non-transient, so a captive portal answering with a
   * login page does not produce this on every account at once.
   */
  void (*reauthentication_needed)(HostContext, SiftId account);
  /**
   * **The bundle was replaced** — FR-26.
   *
   * Sift implements no self-update; the platform channel replaced the bundle underneath
   * the running process. The shell surfaces a restart prompt rather than continuing
   * against replaced resources.
   */
  void (*bundle_replaced)(HostContext);
  /**
   * **A notification was activated** — FR-23.
   *
   * Opens that message, which under FR-25 may mean opening a window on a process that
   * has none.
   */
  void (*notification_activated)(HostContext, SiftId account, SiftId message);
  /**
   * **The account condition changed** — D-49.
   *
   * One condition per account, from the enumerated precedence-ordered set. The
   * discriminant is [`AccountCondition`](sift_foundation::condition::AccountCondition)'s
   * position in its `ALL`, and under D-66 the shell handles it **exhaustively at build
   * time** — there is no runtime fallback for an unrecognised value and one must not be
   * added.
   */
  void (*account_condition_changed)(HostContext, SiftId account, uint32_t condition);
  /**
   * **An authorization callback arrived** — D-36.
   *
   * Delivered by the platform's own launch machinery through the registered URI scheme.
   * Not a socket: NFR-24 admits no listening socket for any purpose, and D-36 removes
   * the loopback redirect rather than excusing it.
   *
   * The URL is attacker-reachable — any local application can invoke a registered
   * scheme — which is why D-88's state parameter is doing real work rather than being
   * ceremony. A callback whose state matches no flow in progress is **discarded without
   * comment**.
   */
  void (*authorization_callback)(HostContext, SiftStr url);
} SiftHostCallbacks;

/**
 * Run a scheduled item. The shell calls this, on its main loop, with the ticket it was given.
 */
typedef void (*SiftRun)(uint64_t ticket);

/**
 * Arrange for `run(ticket)` to happen on the shell's main loop, and return immediately.
 *
 * **It must not run it inline.** Running it inline would deliver a callback from inside the
 * call that produced it, which is the reentrancy D-48 forbids outright.
 */
typedef void (*SiftSchedule)(void *context, SiftRun run, uint64_t ticket);

/**
 * What the layer needs from the shell that the host callbacks do not carry.
 */
typedef struct {
  /**
   * The container the application's files live under — UTF-8, pointer and length.
   *
   * **The shell supplies it; the layer never computes one.** The container API is platform
   * code, and D-1's economics rest on the core having no platform toolkit. Deriving a path
   * from `$HOME` would also be wrong under both the macOS sandbox and Flatpak, and would
   * give the test harness no way to ask for a scratch root.
   */
  SiftStr container_root;
  /**
   * D-48's hop.
   */
  SiftSchedule schedule;
  void *schedule_context;
  /**
   * D-36 and D-71: whether the callback scheme is registered with the system.
   *
   * Checked **before** an authorization begins rather than after, because discovering it
   * afterwards means the user has already been sent to a browser and returned to nothing.
   */
  uint8_t scheme_is_registered;
} SiftInit;

/**
 * An opaque handle to the running layer.
 *
 * The shell holds it and passes it back. It never dereferences it — which is what lets the
 * layer change shape without changing the boundary.
 */
typedef struct {
  uint8_t _private[0];
} SiftApp;

/**
 * Which observation a delivery belongs to.
 *
 * **Distinct from [`Generation`], and the two cannot be one value.** A generation is a
 * per-delivery staleness stamp: it advances on cancellation so that a delivery already
 * posted to the main loop is discarded on arrival. An observation handle is an identity: it
 * says *which* registration a callback is for, and it must not change while that
 * registration lives.
 *
 * Collapsing them was a defect rather than a simplification. With one value, two live
 * observations over different sets are indistinguishable — a shell watching a folder list
 * and a message list holds the same handle for both — and cancelling one either cancels
 * every observation or none of them. The shell has no way to tell which it got.
 *
 * A handle is never reused within a process, for the same reason a generation is not: a
 * reused handle would let a delivery for a dead observation be accepted by a live one that
 * happened to inherit its number.
 */
typedef uint64_t SiftObservation;
/**
 * Not a valid observation. What an out-parameter holds if a registration fails.
 */
#define SiftObservation_NONE 0
/**
 * The first handle a process issues. Deliberately not zero.
 */
#define SiftObservation_FIRST 1

/**
 * The generation of an observation.
 *
 * D-66's answer to a race D-48 would otherwise deadlock on. Cancellation is synchronous:
 * when it returns, no further callback for that observation will arrive, on any thread,
 * ever. But a delivery may already have been *posted* to the shell's main loop — and a
 * cancellation that waited for posted deliveries would be **waiting on the very loop that
 * called it**, which deadlocks deterministically rather than occasionally.
 *
 * So cancellation rendezvous with worker-side work only, and advances the generation. A
 * stale delivery reaching the main loop is discarded by comparing generations.
 */
typedef uint64_t Generation;
#define Generation_FIRST 0

/**
 * A message row, as the list receives it.
 *
 * **Fixed layout, and the text fields are pointers into layer-owned storage valid for the
 * duration of the delivery.** A shell that needs a value beyond the callback copies it.
 *
 * D-66 excluded the alternative arithmetically: FR-6's ten fields against NFR-6's
 * 10,000-row fling is a hundred thousand boundary crossings per fling, versus one delivery.
 */
typedef struct {
  /**
   * D-78's local identity. Stable for as long as the message exists in that account, and
   * therefore usable as a key for selection, undo rendering and notification
   * click-through.
   */
  SiftId id;
  SiftId account;
  /**
   * Server-assigned received time — what D-55 orders on.
   */
  uint64_t received_millis;
  /**
   * The sender's `Date` header. **Displayed only.**
   */
  uint64_t origination_millis;
  /**
   * Normalized under NFR-54 before it got here. Validity is established once, where
   * normalization happens, and is **not re-checked by the shell**.
   */
  SiftStr sender;
  SiftStr subject;
  SiftStr snippet;
  uint8_t unread;
  uint8_t flagged;
  uint8_t has_attachments;
  /**
   * Marked rather than joined — D-4.
   */
  uint8_t duplicate_across_accounts;
  uint32_t thread_count;
} SiftMessageRow;

/**
 * A contiguous, borrowed array of fixed-layout records.
 *
 * The alternative — an opaque row handle with a per-field accessor — was excluded
 * arithmetically rather than on taste. FR-6's list row carries about ten fields, and
 * NFR-6 requires 60 fps with **zero dropped frames over a 10,000-row fling**. That is a
 * hundred thousand boundary crossings per fling, against one delivery.
 *
 * # Safety
 *
 * Borrowed for the duration of the delivery. Each record's text fields are [`SiftStr`]
 * pointing into layer-owned storage with the same lifetime.
 */
typedef struct {
  const SiftMessageRow *ptr;
  size_t len;
} SiftRows_SiftMessageRow;

/**
 * Delivered on the shell's main loop, non-reentrantly.
 *
 * The shell's rule inside one of these is D-48's: **receive, record, return; act on the next
 * turn of the loop.**
 *
 * It carries **both** identifiers, and they answer different questions. The observation says
 * which registration this delivery belongs to, so a shell holding several can route it. The
 * generation says whether it is still wanted, so a delivery posted before a cancellation is
 * discarded on arrival rather than waited for at the cancel — which is the deadlock D-48
 * names.
 */
typedef void (*SiftRowsCallback)(void *context,
                                 SiftObservation observation,
                                 Generation generation,
                                 SiftRows_SiftMessageRow rows);

/**
 * A rendered message body, as the reader receives it.
 *
 * The strings point into layer-owned storage that lives until the document is closed, which
 * is longer than a delivery: the body view holds the HTML while it renders, and resolves
 * resources against the token afterwards.
 */
typedef struct {
  /**
   * **Post-sanitization.** A raw provider payload never reaches a shell.
   */
  SiftStr html;
  /**
   * D-28's per-document capability token. Every address in `html` is under it.
   */
  SiftStr token;
  /**
   * How many fetching positions were refused. For the reader's **native** chrome — a
   * count drawn inside the document is one a sender can counterfeit.
   */
  uint32_t blocked;
  /**
   * How many navigation targets the body carries.
   */
  uint32_t links;
} SiftDocument;

/**
 * What the broker said about one resource load.
 *
 * Three answers, kept apart. "Sift refused this" and "this did not arrive" are different
 * facts, and FR-12 insists such pairs stay distinct — a reader that showed one as the other
 * would tell a person their mail was being censored, or that it was fine when it was not.
 * A transparent newtype rather than a C enum, deliberately.
 *
 * cbindgen emits an enum as *both* a tagged `enum` and a `typedef`, and a Swift importer sees
 * two things with one name — which is ambiguous at the use site and cannot be disambiguated
 * without naming the module. A transparent wrapper with associated constants emits one
 * typedef and a set of `#define`s, which is unambiguous in every consumer.
 */
typedef uint32_t SiftResourceAnswer;
/**
 * The bytes are available.
 */
#define SiftResourceAnswer_BYTES 0
/**
 * Deterministically refused, with a reason the reader can render.
 */
#define SiftResourceAnswer_BLOCKED 1
/**
 * Could not be produced — a revoked token, a missing blob, a fabricated address.
 *
 * **Distinct from blocked, and the distinction is load-bearing.** "Sift refused this" and
 * "this did not arrive" are different facts, and a fabricated or stale address resolving
 * here is a defect being caught rather than a resource being refused.
 */
#define SiftResourceAnswer_UNAVAILABLE 2

/**
 * Initialize the layer.
 *
 * The shell supplies its host callbacks **once**, here — D-67's set is process-scoped and
 * is unregistered only at shutdown, because a host callback has no observation and therefore
 * no generation to discard by.
 *
 * # Safety
 * `out` must be a valid writable pointer to a `*mut SiftApp`.
 */
SiftStatus sift_initialize(SiftHostCallbacks callbacks, SiftInit init, SiftApp **out);

/**
 * Tear the layer down.
 *
 * D-70's teardown is bounded and **flushes nothing**; this is the entry point that starts
 * it, and it must not block on D-48's cancellation.
 *
 * # Safety
 * `app` must have come from [`sift_initialize`] and must not be used afterwards.
 */
SiftStatus sift_shutdown(SiftApp *app);

/**
 * Invoke an action by its stable identifier — D-98.
 *
 * **This is how a shell mutates anything.** The action set is an ABI surface, the palette
 * is a filtered view of the same register, and the test harness invokes through this exact
 * entry point rather than a test-only door.
 *
 * # Safety
 * `app` must be valid; `id` must point to `id_len` bytes of UTF-8.
 */
SiftStatus sift_invoke_action(SiftApp *app, const uint8_t *id, size_t id_len);

/**
 * Observe a window of the message list — D-18.
 *
 * The shell declares the window it is looking at and the layer maintains it across change.
 * Every outstanding request carries a cancellation handle, and **cancellation is
 * synchronous**: when [`sift_cancel_observation`] returns, no further callback for that
 * observation will arrive, on any thread, ever.
 *
 * The handle written to `out` is the observation's **identity**, which is what
 * [`sift_cancel_observation`] takes. It is not a generation and the two are not
 * interchangeable.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_observe_messages(SiftApp *app,
                                 SiftId anchor,
                                 uint32_t count,
                                 SiftRowsCallback callback,
                                 void *context,
                                 SiftObservation *out);

/**
 * Cancel an observation, by its identity.
 *
 * Advances that observation's generation, which is what makes a delivery already posted to
 * the main loop discardable on arrival. Cancellation rendezvous with **worker-side work only** — waiting
 * for posted deliveries would be waiting on the caller's own loop, and that deadlocks
 * deterministically rather than occasionally.
 *
 * A shell must not call this from a place that cannot afford to wait briefly.
 *
 * # Safety
 * `app` must be valid.
 */
SiftStatus sift_cancel_observation(SiftApp *app,
                                   SiftObservation observation);

/**
 * Run a scheduled delivery. **The shell calls this, on its main loop, and nowhere else.**
 *
 * This is the far side of D-48's hop: the layer asked the shell to arrange for a ticket to
 * be run on its loop, and this is what running it means. Every observer callback the shell
 * receives is invoked from inside this call, which is what makes "delivered on the shell's
 * own main loop" true rather than hoped for.
 *
 * A ticket that was already run, or that belonged to a layer since torn down, resolves to
 * nothing. That is not a defect to report: a window closing between the post and the turn
 * of the loop is ordinary, and the guarantee cancellation makes is precisely that the
 * delivery finds nothing to call.
 *
 * # Safety
 * Called from the shell's main loop, with a ticket the layer issued.
 */
void sift_run_scheduled(uint64_t ticket);

/**
 * Add an account backed by D-65's recorded corpus rather than by a socket.
 *
 * **This is the fixture path, and it is deliberately part of the boundary rather than a
 * test-only door.** D-98 says the shell test harness invokes through the same entry points a
 * shell does; a second door would mean the thing under test is not the thing that ships.
 * What it adds is a real adapter over recorded exchanges — no network, no credential, no
 * account belonging to anybody — which is what lets a shell be driven, and looked at, before
 * a real mailbox is ever connected.
 *
 * # Safety
 * `app` must be valid; `label` must point to `label_len` bytes of UTF-8.
 */
SiftStatus sift_add_replayed_account(SiftApp *app,
                                     const uint8_t *label,
                                     size_t label_len,
                                     SiftId *out);

/**
 * Discover an account's folders and walk its delta.
 *
 * Folders first, because a delta needs somewhere to put what it finds and D-83 assigns local
 * identity on discovery rather than on first use.
 *
 * **This blocks the calling thread**, which is a limitation rather than a design: the work
 * belongs on a worker under D-19, and moving it there changes nothing a shell can see
 * because every delivery already arrives through D-48's hop rather than from this call.
 *
 * # Safety
 * `app` must be valid; `label` must point to `label_len` bytes of UTF-8.
 */
SiftStatus sift_sync_account(SiftApp *app, const uint8_t *label, size_t label_len);

/**
 * Open a message's body: fetch its chosen part and run the seven stages over it.
 *
 * The document stays open until [`sift_close_document`] revokes its token, because the body
 * view asks for resources after the HTML has been handed over.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_open_document(SiftApp *app, SiftId message, uint8_t dark, SiftDocument *out);

/**
 * Close a document: revoke its token and release what was held for it.
 *
 * D-90 revokes at **navigation**, which is earlier and more often than teardown. Message A's
 * addresses are dead before message B's document exists, whether or not the view survives —
 * which is what keeps "two messages share no address space" true across a reused view.
 *
 * # Safety
 * `app` must be valid; `token` must point to `token_len` bytes of UTF-8.
 */
SiftStatus sift_close_document(SiftApp *app, const uint8_t *token, size_t token_len);

/**
 * Resolve one address under the internal scheme.
 *
 * **This is the body view's only channel out**, and it is a decision function rather than an
 * interception: N-1 leaves the view no network capability at all, so there is nothing to
 * intercept. A fabricated or stale address resolves to `Unavailable` rather than to nothing,
 * because a defect being caught and a resource being refused are different facts.
 *
 * # Safety
 * `app` and `out` must be valid; `url` must point to `url_len` bytes of UTF-8.
 */
SiftStatus sift_resolve_resource(SiftApp *app,
                                 const uint8_t *url,
                                 size_t url_len,
                                 SiftResourceAnswer *out);

/**
 * How many actions the register holds.
 *
 * Exposed so a shell can assert at build time that it handles every one — D-66 makes an
 * unknown discriminant a **build failure, not a runtime case**, and there is deliberately
 * no runtime fallback for one.
 */
uint32_t sift_action_count(void);

/**
 * The identifier of the *n*th action.
 *
 * # Safety
 * `out` must be a valid writable pointer.
 */
SiftStatus sift_action_id(uint32_t index, SiftStr *out);

#endif  /* SIFT_ABI_H */

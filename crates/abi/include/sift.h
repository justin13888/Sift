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
 * The declared media type is an executable one.
 */
#define SIFT_WARN_DECLARED 1

/**
 * The name ends in an extension the platform will execute. **This is the source that decides
 * what actually happens**, and the one a type check never sees.
 */
#define SIFT_WARN_EXTENSION 2

/**
 * The sources disagree, which FR-10 makes suspicious in its own right: a sender who labels
 * an executable as a document has said something about their intent.
 */
#define SIFT_WARN_DISAGREES 4

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
 * What one gesture did, as the shell needs to know it.
 *
 * The undo affordance keys on `enqueued`: a gesture that enqueued nothing has nothing to take
 * back, and offering undo for it would be a control that does nothing.
 */
typedef struct {
  /**
   * Zero for a navigation or a surface. **Not a failure** — reporting a navigation as
   * "0 enqueued" would read as one.
   */
  uint8_t mutated;
  uint32_t enqueued;
  /**
   * Messages whose account is gone. The rest of a bulk gesture is still the user's, so
   * these are skipped rather than fatal.
   */
  uint32_t skipped;
  /**
   * Whether the overlay hides it before any round trip — NFR-7's 16 ms.
   */
  uint8_t optimistic;
} SiftGesture;

/**
 * The gesture that undo would reverse.
 */
typedef struct {
  uint32_t messages;
  /**
   * Whether FR-15's countdown applies. Where this is zero the gesture is still reversible
   * through the ordinary interface — there is simply no toast.
   */
  uint8_t timed;
  /**
   * What is left of L-22. Zero once the countdown has run out, which does not mean the
   * gesture became irreversible.
   */
  uint64_t remaining_millis;
  /**
   * What was done, for the affordance's own words. A `'static` name from the register,
   * so unlike every other borrowed string here it outlives any document.
   */
  SiftStr intent;
} SiftUndoable;

/**
 * D-49's eight conditions, in precedence order. Lower is worse.
 *
 * A `#[repr(transparent)]` newtype with constants rather than a C enum, because cbindgen
 * emits an enum as both a tagged type and a typedef and Swift then sees the name twice.
 */
typedef uint32_t SiftCondition;
/**
 * FR-2. **The one condition that must reach the user with no window open.**
 */
#define SiftCondition_NEEDS_AUTHENTICATION 0
/**
 * Also what a full disk produces: mutations stop rather than being applied optimistically
 * to a store Sift cannot write.
 */
#define SiftCondition_STORAGE_UNAVAILABLE 1
/**
 * FR-22. Never resumes on its own, which is why it is not the same as the next one.
 */
#define SiftCondition_PAUSED_BY_USER 2
/**
 * FR-36. Resumes when the accounting period rolls over.
 */
#define SiftCondition_PAUSED_BY_DATA_CAP 3
/**
 * **Progress, not a fault.** The user action is nothing, and presenting it as a fault
 * would be dishonest.
 */
#define SiftCondition_RECOVERING 4
/**
 * NFR-29. A capability went away, or resynchronization has no efficient path.
 */
#define SiftCondition_DEGRADED 5
/**
 * A quarantined intent, or triage held on an account that is watched but not written to.
 */
#define SiftCondition_ATTENTION 6
/**
 * Draws nothing.
 */
#define SiftCondition_HEALTHY 7

/**
 * What the badge draws.
 */
typedef struct {
  SiftCondition condition;
  /**
   * How many accounts are in it. One account in trouble and five is a different sentence.
   */
  uint32_t accounts;
  /**
   * Whether the user has something to do. `RECOVERING` is the interesting zero.
   */
  uint8_t asks_something_of_the_user;
  uint8_t reaches_the_user_without_a_window;
} SiftAnnunciator;

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
   * Every position in the document that would fetch something, refused or not.
   *
   * Beside the refusal count rather than inferred from it: "three images, all three
   * withheld" and "three images, one withheld" are different sentences, and only the pair
   * distinguishes them.
   */
  uint32_t fetching_positions;
  /**
   * How many were refused. For the reader's **native** chrome — a count drawn inside the
   * document is one a sender can counterfeit, and so is a control drawn inside it.
   */
  uint32_t blocked;
  /**
   * How many navigation targets the body carries.
   */
  uint32_t links;
  /**
   * Whether "always load from this sender" has anything to key a durable allowance on.
   * Where this is zero the control is **absent** rather than disabled: an allowance keyed
   * on nothing applies to everyone, which is the opposite of what the control says.
   */
  uint8_t may_always_allow;
  /**
   * Whether FR-42's destination exists. Read it with [`sift_document_unsubscribe`].
   */
  uint8_t has_unsubscribe;
} SiftDocument;

/**
 * One refused fetching position, and the rule that refused it.
 *
 * The strings borrow from the open document and die with it.
 */
typedef struct {
  /**
   * Where it appeared, so the disclosure says *what* was lost rather than only how much.
   */
  SiftStr element;
  SiftStr attribute;
  /**
   * The address, decoded and bidi-stripped. Safe to render in native chrome.
   */
  SiftStr displayed;
  /**
   * Why. Where no filter list is loaded this says so, rather than naming a rule that did
   * not run — a user who believes a rule matched believes in a protection that is absent.
   */
  SiftStr rule;
} SiftWithheld;

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
  const SiftWithheld *ptr;
  size_t len;
} SiftRows_SiftWithheld;

/**
 * One navigation target, as FR-30 requires it be shown.
 */
typedef struct {
  /**
   * Punycode-decoded and bidi-**stripped** — the opposite of what NFR-54 does to a display
   * name, because a URL's component order carries meaning and prose's does not.
   */
  SiftStr displayed;
  /**
   * What will actually be opened.
   */
  SiftStr target;
  /**
   * The wrapper this was recovered from, or null where there was none. Never followed to
   * find out where it goes — following it *is* the tracking event.
   */
  SiftStr wrapper;
  /**
   * A `mailto:`, shown and reported as needing a mail handler rather than omitted.
   */
  uint8_t needs_a_mail_handler;
} SiftLink;

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
  const SiftLink *ptr;
  size_t len;
} SiftRows_SiftLink;

/**
 * One attachment, as FR-10 lists it. Nothing here has been downloaded.
 */
typedef struct {
  /**
   * The identifier a save takes. Opaque above the adapter.
   */
  SiftStr part;
  /**
   * What the sender declared. Advisory, and one of three sources.
   */
  SiftStr media_type;
  /**
   * The sender's name, normalized for chrome under NFR-54.
   */
  SiftStr display_name;
  /**
   * The name a save would derive under NFR-53. Shown beside the sender's where they
   * differ, because a name that changed silently is one the user did not agree to.
   */
  SiftStr file_name;
  /**
   * What the provider says it costs. Advisory: L-13 bounds what is transferred.
   */
  uint64_t declared_size;
  /**
   * [`SIFT_WARN_DECLARED`], [`SIFT_WARN_EXTENSION`] and [`SIFT_WARN_DISAGREES`], or-ed.
   * Non-zero means FR-10's explicit warning is required before opening.
   */
  uint32_t warning;
} SiftAttachment;

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
  const SiftAttachment *ptr;
  size_t len;
} SiftRows_SiftAttachment;

/**
 * NFR-53's plan: where an attachment would be written, resolved and shown before the write.
 */
typedef struct {
  /**
   * Names the plan. [`sift_write_attachment`] takes this rather than a path, so the path
   * that is written is the one that was shown — re-deriving at write time is exactly how
   * those two come apart.
   */
  uint64_t plan;
  /**
   * The **exact** final path, including any disambiguating suffix.
   */
  SiftStr final_path;
  /**
   * Whether the derived name differs from the sender's, which is worth saying out loud.
   */
  uint8_t renamed;
  uint64_t declared_size;
} SiftSavePlan;

/**
 * What a completed save wrote, and what the bytes turned out to be.
 */
typedef struct {
  uint64_t written;
  /**
   * The warning bits again, now including the source that only exists once the content
   * does. A `.pdf` whose bytes begin `MZ` is the case FR-10 wrote the rule for.
   */
  uint32_t warning;
} SiftSaveOutcome;

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
SiftStatus sift_invoke_action(SiftApp *app,
                              const uint8_t *id,
                              size_t id_len,
                              const uint8_t *parameter,
                              size_t parameter_len,
                              uint8_t confirmed,
                              SiftGesture *out);

/**
 * What could be undone right now.
 *
 * D-86 puts this record **in the layer**, and this is why it can be: a window shell is
 * destroyed when its window closes, in a product that runs with no window at all, so a
 * countdown owned by a view dies with the view. The always-on surface reads the same record
 * through the same entry point.
 *
 * `timed` distinguishes FR-15's countdown from ordinary reversibility. Every intent but
 * permanent delete stays reversible for as long as the message exists; only intents that
 * remove the message from view get a *window*, because those are the ones where the user has
 * nothing left to click. A countdown on every message the reader marks read would make the
 * mechanism worthless by making it constant.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_undoable(SiftApp *app, SiftUndoable *out);

/**
 * D-49's annunciator: the one condition worth drawing, across every account.
 *
 * **One badge, not a list.** `AccountCondition`'s ordering is the precedence, so this is a
 * `min` over what applies — and `Healthy` renders nothing at all, because a badge that is
 * always present is a badge nobody reads.
 *
 * This is a poll rather than a push on purpose. The push exists too: D-67's
 * `account_condition_changed` host callback is what wakes a shell with no window, because
 * `NeedsAuthentication` is the one condition that MUST reach the user with nothing on screen.
 * A shell that had only the callback could not draw the badge when a window opens.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_annunciator(SiftApp *app, SiftAnnunciator *out);

/**
 * Whether an action is available right now.
 *
 * D-98 makes an unavailable action **absent rather than disabled**, so this is what decides
 * whether a shell draws the menu item at all. A greyed item tells a user the action exists
 * and they cannot have it; an absent one tells them nothing, which is the trade D-98 takes
 * deliberately and records the cost of.
 *
 * # Safety
 * `app` and `out` must be valid; `id` must point to `id_len` bytes of UTF-8.
 */
SiftStatus sift_action_available(SiftApp *app, const uint8_t *id, size_t id_len, uint8_t *out);

/**
 * D-99's selection, set from the shell.
 *
 * Keyed on **identity**, never on index: a row that moves under a selection is the same
 * message, and a selection that followed the index would silently retarget the gesture.
 *
 * # Safety
 * `app` must be valid; `ids` must point to `count` identifiers.
 */
SiftStatus sift_select(SiftApp *app, const SiftId *ids, size_t count);

/**
 * Tell the layer whether a window exists.
 *
 * FR-25 makes closing a window and quitting different acts, so "a window exists" is a fact
 * the shell owns and the layer is told — every `Window`-scoped action in the register turns
 * on it, and a layer that assumed one would offer a menu to nobody.
 *
 * # Safety
 * `app` must be valid.
 */
SiftStatus sift_set_window_present(SiftApp *app, uint8_t present);

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
 * FR-29's disclosure: every refused position in an open document, and the rule behind it.
 *
 * The rows borrow from the document and are valid until [`sift_close_document`]. That is one
 * lifetime rather than two that can disagree — the revocation that kills the addresses is the
 * same call that frees the rows describing them.
 *
 * # Safety
 * `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
 */
SiftStatus sift_document_withheld(SiftApp *app,
                                  const uint8_t *token,
                                  size_t token_len,
                                  SiftRows_SiftWithheld *out);

/**
 * FR-30: every navigation target, with the destination the confirmation sheet must show.
 *
 * # Safety
 * `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
 */
SiftStatus sift_document_links(SiftApp *app,
                               const uint8_t *token,
                               size_t token_len,
                               SiftRows_SiftLink *out);

/**
 * FR-42: the unsubscribe destination, where the message declares one.
 *
 * Fails where there is none, which the shell already knows from `has_unsubscribe` — the two
 * agree by construction because both read the same field.
 *
 * **Sift never issues the request.** The historical form is a message, which the no-send
 * constraint forbids outright; the modern form is an HTTP request to an address carrying a
 * per-recipient token, which is precisely what FR-29 treats as evidence that a resource is
 * tracking the reader. This hands the shell a destination to open in a browser, and nothing
 * on this boundary can be made to fetch it.
 *
 * # Safety
 * `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
 */
SiftStatus sift_document_unsubscribe(SiftApp *app,
                                     const uint8_t *token,
                                     size_t token_len,
                                     SiftLink *out);

/**
 * FR-10's list. **Nothing is downloaded** — this reads the structure the sync already has.
 *
 * The rows are held per message and replaced on each call, so a shell that lists twice sees
 * the second listing rather than two.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_message_attachments(SiftApp *app, SiftId message, SiftRows_SiftAttachment *out);

/**
 * Resolve where an attachment would be written. **Writes nothing.**
 *
 * Two calls rather than one, because the requirement is that the exact path be shown
 * *before* the write. One call that saved and then reported would satisfy every test and
 * none of the requirement.
 *
 * The directory is the user's and comes from the platform's own chooser; Sift decides only
 * the name, and decides it under NFR-53.
 *
 * # Safety
 * `app` and `out` must be valid; `part` and `directory` must point to their lengths in UTF-8.
 */
SiftStatus sift_plan_attachment_save(SiftApp *app,
                                     SiftId message,
                                     const uint8_t *part,
                                     size_t part_len,
                                     const uint8_t *directory,
                                     size_t directory_len,
                                     SiftSavePlan *out);

/**
 * Fetch the part and write it to the planned path.
 *
 * **Nothing is overwritten**, and that is held at the syscall rather than by a check — a
 * check before a write is a race, and the file that appears between the two is the one
 * somebody cared about.
 *
 * The plan is consumed, so one plan writes one file. A shell that wants a second copy asks
 * for a second plan, which resolves a second path.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_write_attachment(SiftApp *app, uint64_t plan, SiftSaveOutcome *out);

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

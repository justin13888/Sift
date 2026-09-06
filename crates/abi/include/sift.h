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
 * Sift's own store.
 */
#define SIFT_SOURCE_LOCAL 0

/**
 * The provider answered. Not reachable yet; the label exists because a merge that does not
 * distinguish the two is the mistake FR-21 is about.
 */
#define SIFT_SOURCE_SERVER 1

/**
 * A boolean.
 */
#define SIFT_SETTING_FLAG 0

/**
 * A count, a byte budget, or a duration in milliseconds. The unit is the setting's.
 */
#define SIFT_SETTING_NUMBER 1

#define SIFT_SETTING_TEXT 2

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
 * Arm a coalescing platform timer, and call `run(ticket)` on the main loop when it fires.
 *
 * **The leeway is the whole point.** D-25 rejects the async runtime's own timer precisely
 * because it cannot tell the kernel "this may fire late, batch it with something else", and
 * that hint is the entire mechanism by which wakeups coalesce. A shell that ignores
 * `leeway_millis` and arms an exact timer satisfies this signature and fails NFR-11.
 *
 * On macOS this is a dispatch source timer with an explicit leeway; on Linux, an
 * absolute-mode timer file descriptor.
 */
typedef void (*SiftArmTimer)(void *context,
                             SiftRun run,
                             uint64_t ticket,
                             uint64_t delay_millis,
                             uint64_t leeway_millis);

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
   * D-25's platform timer, armed by the shell on the layer's behalf.
   *
   * It shares `schedule_context`: both are the same shell object, and a second context
   * would be a second thing to keep alive for no gain. The layer computes *when* from its
   * own wheel; the shell owns the one thing only it can do, which is asking the platform
   * for a timer that is allowed to fire late.
   */
  SiftArmTimer arm_timer;
  /**
   * The OAuth client this bundle was configured with — empty where there is none.
   *
   * **Configuration, not a secret.** A public client's identifier appears in every
   * authorization URL it generates, which is why PKCE exists; D-88 forbids an embedded
   * secret outright. It is stated once, here, rather than repeated at every call, because
   * the layer needs it for three things a shell should not be answering separately.
   */
  SiftStr oauth_client_id;
  /**
   * Every URI scheme this shell's bundle claims, separated by newlines — D-36 and D-109.
   *
   * **A fact, not a conclusion, and the difference is the bug this replaced.** The macOS
   * shell used to pass a boolean saying the scheme was registered, hardcoded to true beside
   * a comment asserting the Info.plist did it. A configuration shipped without the derived
   * scheme, the layer was told otherwise, D-71's refusal could not fire, and the failure
   * surfaced as a browser page after the user had granted consent.
   *
   * A bundle is what registers a scheme, so only a shell can report this. Deciding what it
   * *means* — which scheme this client requires, and whether it is among them — is the
   * layer's, where the derivation already lives and where one rule serves both shells.
   *
   * A URI scheme cannot contain a newline, so this needs no escaping.
   */
  SiftStr registered_schemes;
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
 * One account, as a shell needs to see it.
 *
 * **The label is the handle.** Every account-taking entry point across this boundary names an
 * account by the label it was added under, and until this existed nothing said what those
 * labels were — so a shell could add an account and then never reach it again, and the
 * runtime panel had to ask the user to type one. D-89 makes the identity Sift's own; the
 * label is what a person calls it and what the container recorded.
 */
typedef struct {
  /**
   * D-89's Sift-assigned identity — the anchor a message-list observation takes.
   */
  SiftId id;
  /**
   * What the account was added as, and what every other entry point takes.
   */
  SiftStr name;
  /**
   * What the container recorded so a later run knows how to reconnect it. **Not something
   * to branch on**: the provider model plans against declared capabilities, and this is a
   * name for a reconnection route rather than a provider a shell may reason about.
   */
  SiftStr kind;
  /**
   * D-49's single condition for this account.
   */
  SiftCondition condition;
  /**
   * Whether Sift may change this mailbox. An account is added watching and nothing else,
   * and this is the flag that says so.
   */
  uint8_t writes_enabled;
  /**
   * Intents recorded and held because writes are not authorized. Zero once they are.
   */
  uint32_t held;
} SiftAccount;

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
  const SiftAccount *ptr;
  size_t len;
} SiftRows_SiftAccount;

/**
 * What one turn of the flush did.
 *
 * **`authorized` is not a failure.** An account that is only being watched has a queue that
 * grows and sends nothing, and that is the state the user chose — so it crosses as a result
 * with a count in it rather than as an error, which is what lets a surface say *nothing has
 * been sent, and nothing will be until you say so* instead of drawing a fault.
 */
typedef struct {
  /**
   * Zero where writes are not authorized for this account. Nothing was issued.
   */
  uint8_t authorized;
  /**
   * Intents held because writes are not authorized. Zero once they are.
   */
  uint32_t held;
  uint32_t issued;
  uint32_t applied;
  /**
   * The provider refused, in its own terms. Settled: retrying changes nothing.
   */
  uint32_t refused;
  /**
   * Left for the scheduler to try again.
   */
  uint32_t deferred;
  /**
   * The request went out and no answer came back — D-85's `Reconciling`.
   */
  uint32_t reconciling;
  /**
   * Held rather than executed: unrecognised, or its gating capability has gone away.
   */
  uint32_t quarantined;
  /**
   * What is still queued afterwards.
   */
  uint32_t queued;
  /**
   * Whether the flush ended in a stated failure. **The state, not the sentence** — D-56
   * keeps prose on the shell's side of this boundary.
   */
  uint8_t failed;
} SiftFlush;

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
 * What a search found, and what it understood.
 */
typedef struct {
  /**
   * The same fixed-layout row the list uses, so a shell draws results with the code it
   * already has.
   */
  SiftRows_SiftMessageRow rows;
  /**
   * How each term was read, joined by `; `. Shown to the user, not logged.
   */
  SiftStr interpretation;
  /**
   * What this build could not answer about this query, one per line. Empty is the good
   * case and means exactly that.
   */
  SiftStr caveats;
  /**
   * How many accounts could have been asked to search server-side, and were not.
   */
  uint32_t delegable_accounts;
} SiftSearch;

/**
 * One setting, as D-101 enumerates it.
 */
typedef struct {
  /**
   * Stable, and never renumbered.
   */
  SiftStr key;
  /**
   * Which requirement or decision owns it, so a settings screen can say *why* a thing is
   * there and a reviewer can find the argument rather than the value.
   */
  SiftStr owner;
  /**
   * The shipped default, for flags. Empty for the other kinds, whose defaults are numbers
   * and lists a screen shows differently anyway.
   */
  SiftStr default_;
  /**
   * What it currently holds.
   */
  SiftStr value;
  uint32_t kind;
  /**
   * Whether it goes with the account under FR-4, rather than surviving every removal.
   */
  uint8_t account_scoped;
  /**
   * **Security state rather than a preference.** A record of decisions the user made in
   * context. A shell shows these and revokes from them; it does not offer bulk editing of
   * them in a screen away from any message.
   */
  uint8_t security_state;
} SiftSetting;

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
  const SiftSetting *ptr;
  size_t len;
} SiftRows_SiftSetting;

/**
 * One durably enqueued intent, as FR-34 shows it.
 */
typedef struct {
  SiftId message;
  /**
   * FR-13's closed set, by name.
   */
  SiftStr intent;
  /**
   * D-85's six states. `Pending` for everything on an account that is watched and not
   * written to — which is the whole point of being able to read this.
   */
  SiftStr state;
  uint32_t attempts;
  /**
   * Intents against one message apply strictly in this order. Always, including through
   * batching, retry and a restart.
   */
  uint64_t sequence;
} SiftQueued;

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
  const SiftQueued *ptr;
  size_t len;
} SiftRows_SiftQueued;

/**
 * One subsystem's live bytes.
 */
typedef struct {
  SiftStr name;
  /**
   * **Signed.** A subsystem that frees in one task what another allocated reads negative,
   * and clamping that to zero would hide the one number that says the tagging is wrong.
   */
  int64_t live_bytes;
} SiftSubsystemBytes;

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
  const SiftSubsystemBytes *ptr;
  size_t len;
} SiftRows_SiftSubsystemBytes;

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
  const SiftStr *ptr;
  size_t len;
} SiftRows_SiftStr;

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
 * How many accounts this installation has — including the ones a previous run added.
 *
 * **A shell asks rather than counting what it has seen.** The macOS shell kept a counter that
 * began at zero every launch, so every launch took the account-less branch and offered to add
 * an account the user had already added: the container had the mail, sealed, with its queue
 * rebuilt from its journal, and the first-run screen was drawn over it.
 *
 * Zero is the genuine account-less state, and it is what makes the add-account flow the right
 * thing to show rather than an empty inbox.
 *
 * # Safety
 * `app` must be valid.
 */
uint32_t sift_account_count(SiftApp *app);

/**
 * Every account the container holds.
 *
 * The rows are borrowed for the duration of the call, like every other row array here, and
 * the text behind them lives in the layer until the next call replaces it.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_accounts(SiftApp *app, SiftRows_SiftAccount *out);

/**
 * Authorize, or withdraw authorization for, writes to one account.
 *
 * **An account is added watching and nothing else**, and this is the only thing that changes
 * it. Until it existed, triage on a macOS account was journaled durably, applied
 * optimistically, and could never be issued — the posture was settable from the test harness
 * and from nowhere a person could reach.
 *
 * Withdrawing takes effect immediately for anything not yet issued. Intents already on the
 * wire are not recalled: a request that has left cannot be unsent, and pretending otherwise
 * is the one lie a mutation queue must not tell.
 *
 * # Safety
 * `app` must be valid; `label` must point to `label_len` bytes of UTF-8.
 */
SiftStatus sift_set_writes_enabled(SiftApp *app,
                                   const uint8_t *label,
                                   size_t label_len,
                                   uint8_t enabled);

/**
 * Send what is queued for one account, once.
 *
 * **This blocks the calling thread**, which is a limitation rather than a design, and the same
 * one [`sift_sync_account`] carries: the work belongs on a worker under D-19, and moving it
 * there changes nothing a shell can see because every delivery already arrives through D-48's
 * hop rather than out of this call.
 *
 * # Safety
 * `app` and `out` must be valid; `label` must point to `label_len` bytes of UTF-8.
 */
SiftStatus sift_flush_account(SiftApp *app, const uint8_t *label, size_t label_len, SiftFlush *out);

/**
 * FR-15 — reverse the last reversible gesture.
 *
 * **Not an action.** `undo.last-gesture` is in D-98's register and has no intent behind it, so
 * invoking it through [`sift_invoke_action`] returns success and does nothing — which is what
 * the undo toast was wired to. The reversal is a gesture of its own shape: it acts over
 * D-85's undo group rather than over a message, so a bulk operation reverses as the one
 * gesture FR-17 promises.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_undo_last(SiftApp *app, SiftGesture *out);

/**
 * The URI scheme this client's authorization callback comes back on — D-36 and D-109.
 *
 * # Why a shell asks rather than derives
 *
 * It is one rule, and it is not the obvious one: a provider that lets an application name its
 * own redirect gets Sift's scheme, and one that does not — Google's iOS/macOS client type is
 * the case that forced this — accepts exactly one, the client identifier reversed. D-17 exists
 * to stop two shells growing two answers to a question like that, so the derivation stays in
 * `sift-foundation` and this is how a shell reaches it.
 *
 * It is derived from the client this installation was configured with, which the layer was
 * given at initialization — so a shell that asks this and a flow that declares a redirect
 * cannot answer differently.
 *
 * A shell needs it to tell the platform which scheme a callback will arrive on. It is empty
 * where no client is configured, and a shell must not begin an authorization in that case.
 *
 * The string lives in the layer until the next call that asks for one.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_callback_scheme(SiftApp *app, SiftStr *out);

/**
 * D-36 — begin an authorization, and hand back the address to open in a browser.
 *
 * **The scheme registration is checked before the user goes anywhere.** Discovering it
 * afterwards means they have already granted consent and returned to nothing, and the
 * resulting page is a browser error rather than anything Sift can explain.
 *
 * The address is held by the layer until the flow completes or another begins, because the
 * verifier behind it is: PKCE binds the exchange to the process that started it, and a shell
 * holding the state would be a shell that could be asked to complete a flow it did not begin.
 *
 * The client is the one this installation was configured with, stated at initialization. It
 * is not a parameter because it was one: a shell repeating it at every call is a shell that
 * can disagree with the bundle it is running out of.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_begin_authorization(SiftApp *app, SiftStr *out);

/**
 * Finish an authorization from the address the system handed back, and add the account.
 *
 * The callback arrives through the registered URI scheme — **not a socket**, because NFR-24
 * admits none for any purpose. It is also one of only two local attack surfaces Sift has, so
 * a callback whose state matches no flow in progress is discarded without comment: D-88's
 * state parameter is doing real work here rather than being ceremony.
 *
 * # Safety
 * `app` and `out` must be valid; both strings must point to their lengths in UTF-8.
 */
SiftStatus sift_complete_authorization(SiftApp *app,
                                       const uint8_t *callback,
                                       size_t callback_len,
                                       const uint8_t *display_name,
                                       size_t display_name_len,
                                       SiftId *out);

/**
 * FR-19, FR-20 and FR-21 — search, with the interpretation the user is shown.
 *
 * `account` narrows it to one account, and zero is every account — the same anchor
 * [`sift_observe_messages`] takes, so a window that is looking at one mailbox can search the
 * one it is looking at. FR-20's *narrow to this account* is a scope rather than a query term:
 * spelling it as an operator would mean parsing, translating and explaining a word for
 * something the shell already knows.
 *
 * **The interpretation crosses the boundary as a result, not as a debug aid.** A query that
 * found nothing and one that was misread look identical from the results alone, and
 * `form:alice` is a plausible typo for `from:alice`. So is the caveat list: empty results and
 * unsearched fields also look identical, and a person who searches `has:attachment`, gets
 * nothing, and concludes they have no attachments has been misled by a filter that was never
 * evaluated.
 *
 * # Safety
 * `app` and `out` must be valid; the strings must point to their lengths in UTF-8.
 */
SiftStatus sift_search(SiftApp *app,
                       const uint8_t *query,
                       size_t query_len,
                       SiftId account,
                       uint32_t limit,
                       SiftSearch *out);

/**
 * D-101's settings: every one, with its scope, its default and what it currently holds.
 *
 * **Enumerated across the boundary rather than known by each shell.** The surface is written
 * twice, in Swift and in GTK, and a default chosen independently by two shells is two
 * products — the ones that matter most being the ones that look least like decisions: the
 * dark transform is off, the debug surfaces are off, and there is no data cap.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_settings(SiftApp *app, SiftRows_SiftSetting *out);

/**
 * Record a setting.
 *
 * # Safety
 * `app` must be valid; both strings must point to their lengths in UTF-8.
 */
SiftStatus sift_set_setting(SiftApp *app,
                            const uint8_t *key,
                            size_t key_len,
                            const uint8_t *value,
                            size_t value_len);

/**
 * Record an account setting — D-101's other table.
 *
 * **Separate from [`sift_set_setting`] because the scope split is the storage split.** An
 * account setting goes with the account when it is removed and an installation setting does
 * not, and a single entry point taking a key would have to guess which table a key belongs to
 * from the key itself — which is exactly the ambiguity the two tables exist to remove.
 *
 * The two security-state rows are refused here as they are there: the per-sender lists are
 * records of decisions the user made in context, shown and revoked where the decision was
 * made rather than bulk-edited in a screen away from any message.
 *
 * # Safety
 * `app` must be valid; every string must point to its length in UTF-8.
 */
SiftStatus sift_set_account_setting(SiftApp *app,
                                    const uint8_t *label,
                                    size_t label_len,
                                    const uint8_t *key,
                                    size_t key_len,
                                    const uint8_t *value,
                                    size_t value_len);

/**
 * What an account setting currently holds.
 *
 * The text lives in the layer until the next call replaces it, like every other borrowed
 * string here.
 *
 * # Safety
 * `app` and `out` must be valid; both strings must point to their lengths in UTF-8.
 */
SiftStatus sift_account_setting(SiftApp *app,
                                const uint8_t *label,
                                size_t label_len,
                                const uint8_t *key,
                                size_t key_len,
                                SiftStr *out);

/**
 * FR-34's queue: what is durably enqueued, per account, by state.
 *
 * **This is the surface a person uses to check that nothing was sent.** An account that is
 * watched but not written to accumulates intents here, and being able to look at them — and
 * see that every one is `Pending` — is what makes the read-only posture something a user can
 * verify rather than something they are told.
 *
 * The rows borrow from the layer and are replaced by the next call.
 *
 * # Safety
 * `app` and `out` must be valid; `account` must point to `account_len` bytes of UTF-8.
 */
SiftStatus sift_queue(SiftApp *app,
                      const uint8_t *account,
                      size_t account_len,
                      SiftRows_SiftQueued *out);

/**
 * FR-34's per-subsystem live bytes, and the residual nothing claimed.
 *
 * The residual is reported rather than distributed. D-24's attribution is a tagging
 * allocator, and a number that added up perfectly would mean the tagging was being papered
 * over — an unattributed remainder is what an honest measurement of it looks like.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_memory(SiftApp *app, SiftRows_SiftSubsystemBytes *out);

/**
 * FR-33's debug view: which stages ran, over a document already open.
 *
 * Available in release builds behind a preference, per FR-33 — the gating is the shell's,
 * because the preference is the shell's.
 *
 * # Safety
 * `app` and `out` must be valid; `token` must point to `token_len` bytes of UTF-8.
 */
SiftStatus sift_document_stages(SiftApp *app,
                                const uint8_t *token,
                                size_t token_len,
                                SiftRows_SiftStr *out);

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
 * FR-8 — the user allows this message's remote content, once or for this sender.
 *
 * **The two controls are different things and this is where the difference lives.**
 * `durable` zero lets the document that is open fetch, and dies with its token, so
 * re-opening the same message asks again. `durable` non-zero writes the sender into the
 * allowance list and survives. A single flag serving both would silently make a transient
 * choice permanent, which is the failure a user can neither see nor undo.
 *
 * The shell holds a token, never an origin. Which sender a durable allowance keys on is the
 * layer's to resolve, from what authenticated the message — so a shell cannot name a sender
 * it was not given, and cannot key an allowance on one that authenticated nothing. Where
 * there is nothing to key on this **fails**, and the interface must not have offered the
 * control: `SiftDocument::may_always_allow` is what says so before it is pressed.
 *
 * # Safety
 * `app` must be valid; `token` must point to `token_len` bytes of UTF-8.
 */
SiftStatus sift_allow_remote_content(SiftApp *app,
                                     const uint8_t *token,
                                     size_t token_len,
                                     uint8_t durable);

/**
 * D-93 — the operating system says memory is under pressure.
 *
 * `pressure` is 0 normal, 1 warning, 2 critical, and anything else is treated as critical:
 * an unrecognised level from a platform source is not an argument for doing less.
 *
 * **Subscribed to, never polled.** Polling free memory is both a wakeup counted against
 * NFR-11 and a worse signal than the one the system already computes, so a shell arms a
 * platform pressure source and calls this from it.
 *
 * Returns the tier the governor now holds, so a shell can show it. The sheds are *issued*
 * here and never awaited — NFR-13's deadline is an issue deadline, and at L3 window
 * destruction is a host callback on the shell's own loop, so waiting on it from inside this
 * call would be a deadlock rather than a delay.
 *
 * # Safety
 * `app` and `out` must be valid.
 */
SiftStatus sift_memory_pressure(SiftApp *app, uint32_t pressure, uint32_t *out);

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

//! D-49 — the account condition. D-71 — the process condition.
//!
//! # Why there is one condition rather than a flag per subsystem
//!
//! Ten documents require that something be surfaced to the user — a degraded IMAP server,
//! a quarantined intent, a blocker disagreement, a replaced bundle, a contrast failure, a
//! failed token refresh, a cursor recovery, a conflict notice, a partially applied thread
//! mutation, an image withheld because L1 dropped the blocking authority — and no
//! document owned the surface they surface *to*. Eight distinct things wrong with an
//! account had the same shape.
//!
//! A flag per failing subsystem pushes adjudication into two shells, which is the drift
//! the presentation layer exists to prevent, and makes the tray a function of whichever
//! flag was set last. A single generic error state produces a badge that means
//! "something". So: **every account has exactly one condition at a time**, computed from
//! the independent facts that produce it.
//!
//! **What this costs, stated rather than discovered:** a condition computed from several
//! facts can hide a second behind the first. An account both over its data cap and on a
//! degraded server has two problems and shows one. FR-34's debug panel is where all the
//! underlying facts stay visible, and if the pairing proves common the answer is a
//! primary condition plus an explicit secondary list — **not a return to flags**.

use core::fmt;

/// The condition an account is in. Exactly one at a time.
///
/// **Declaration order is precedence order**, highest first: the first that applies is
/// the account's condition. [`Ord`] follows that order, so `min()` over the conditions
/// that apply selects correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountCondition {
    /// A token refresh failed non-transiently, under FR-2.
    ///
    /// **The one condition that MUST reach the user with no window open.** Sift is
    /// resident and may have nothing on screen, so this is raised through the always-on
    /// surface — which is why FR-22's tray presence ships in P1 rather than as late
    /// polish.
    ///
    /// Cleared by successful interactive re-authentication. Note D-88's classifier is
    /// deliberately conservative: only a well-formed provider denial is non-transient, so
    /// a hotel router answering with a login page does not produce this on every account.
    NeedsAuthentication,
    /// The credential store or the account database is unreadable.
    ///
    /// Also the condition a full disk produces: Sift stops accepting mutations rather
    /// than applying them optimistically to a store it cannot write.
    StorageUnavailable,
    /// The user paused this account through the always-on surface, under FR-22.
    ///
    /// Distinct from [`PausedByDataCap`](Self::PausedByDataCap) because **a capped
    /// account resumes on its own and a user-paused one never does** — D-58 shares the
    /// policy tier between them but deliberately does not collapse the reasons.
    PausedByUser,
    /// Cumulative usage reached FR-36's cap. Clears when the accounting period rolls over
    /// or the cap is raised.
    PausedByDataCap,
    /// A cursor invalidation is being recovered under NFR-18, or an initial backfill is
    /// still running under D-53.
    ///
    /// **This is progress, not a fault**, and the user action is nothing.
    Recovering,
    /// A probed capability went away, or the server has no efficient resynchronization —
    /// NFR-29. Degradation is surfaced rather than hidden.
    ///
    /// Provider throttling reaches here only once it stops progress; until then D-87 makes
    /// it transient degradation, which is a notice rather than a condition.
    Degraded,
    /// A quarantined intent under NFR-48, or a mutation whose failure FR-16 could not
    /// resolve unambiguously. The user has something to act on.
    Attention,
    /// Nothing is wrong.
    Healthy,
}

impl AccountCondition {
    /// Every condition, in precedence order.
    pub const ALL: &'static [Self] = &[
        Self::NeedsAuthentication,
        Self::StorageUnavailable,
        Self::PausedByUser,
        Self::PausedByDataCap,
        Self::Recovering,
        Self::Degraded,
        Self::Attention,
        Self::Healthy,
    ];

    /// Resolve the independent facts that hold into the one condition the account shows.
    ///
    /// D-49's rule, applied: "the first that applies is the account's condition".
    #[must_use]
    pub fn resolve(applicable: &[Self]) -> Self {
        applicable.iter().copied().min().unwrap_or(Self::Healthy)
    }

    /// Whether this condition must reach the user even with no window open.
    ///
    /// Only one does. The others are visible when a window is; this one leaves the account
    /// silently stalled, which FR-2 exists to prevent.
    #[must_use]
    pub const fn reaches_the_user_without_a_window(self) -> bool {
        matches!(self, Self::NeedsAuthentication)
    }

    /// Whether the user has something to do about it.
    ///
    /// `Recovering` is the interesting `false`: it is the only non-healthy condition that
    /// asks nothing of the user, and presenting it as a fault would be dishonest.
    #[must_use]
    pub const fn asks_something_of_the_user(self) -> bool {
        !matches!(self, Self::Recovering | Self::Healthy)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NeedsAuthentication => "needs-authentication",
            Self::StorageUnavailable => "storage-unavailable",
            Self::PausedByUser => "paused-by-user",
            Self::PausedByDataCap => "paused-by-data-cap",
            Self::Recovering => "recovering",
            Self::Degraded => "degraded",
            Self::Attention => "attention",
            Self::Healthy => "healthy",
        }
    }
}

impl fmt::Display for AccountCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What kind of guarantee a missing subsystem protects, which decides what happens when
/// it is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuaranteeKind {
    /// A security guarantee made unenforceable. Sift **refuses**.
    Security,
    /// A feature guarantee. Sift **degrades**, visibly.
    Feature,
    /// A resource guarantee. Sift **degrades and says so**.
    Resource,
}

/// What Sift does when a subsystem never started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    /// Refuse the operation the absent subsystem would have protected.
    Refuse,
    /// Continue without it, visibly.
    Degrade,
}

/// D-71 — a process-scoped condition, beside D-49's account-scoped ones.
///
/// **The set is closed.** Two scopes invite a third, and "process condition" could
/// otherwise become where anything awkward is filed — so an entry that cannot name the
/// guarantee it protects does not belong here.
///
/// The rule the table encodes: **a security guarantee made unenforceable refuses; a
/// feature or resource guarantee degrades visibly.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProcessCondition {
    /// The OS credential store is unavailable.
    ///
    /// Every account enters [`StorageUnavailable`](AccountCondition::StorageUnavailable).
    /// Sift **MUST NOT retry in a loop** — futile, and a wakeup source counted against
    /// NFR-11 — and raises the request through the always-on surface.
    CredentialStore,
    /// The filter engine's lists could not be loaded.
    ///
    /// **An absent authority denies**: every remote fetch is refused, and the reason
    /// recorded for FR-33 names the shed rather than a rule. Sift MUST NOT fall through to
    /// D-10's compiled backstop, and MUST NOT reload the engine on demand — a 40 MB
    /// allocation in response to a pressure signal is the shed undoing itself.
    FilterLists,
    /// The body view could not be created. Degrades to FR-9's plain-text and raw views.
    BodyView,
    /// No memory-pressure signal source. Runs with no governor: loses the shed tiers,
    /// keeps the per-cache budgets.
    PressureSignal,
    /// The scheduler's timing wheel did not start.
    ///
    /// **Refuses**, because the alternative is a per-account sleep loop and D-25 prohibits
    /// that outright rather than tolerating it as a fallback.
    TimingWheel,
    /// The URI scheme did not register.
    ///
    /// Sift **MUST NOT begin an authorization whose callback has nowhere to arrive**, and
    /// D-36 requires this be checked *before* a flow starts rather than discovered after
    /// one.
    UriSchemeRegistration,
}

impl ProcessCondition {
    pub const ALL: &'static [Self] = &[
        Self::CredentialStore,
        Self::FilterLists,
        Self::BodyView,
        Self::PressureSignal,
        Self::TimingWheel,
        Self::UriSchemeRegistration,
    ];

    /// The guarantee this subsystem's absence makes unenforceable.
    #[must_use]
    pub const fn guarantee(self) -> GuaranteeKind {
        match self {
            Self::CredentialStore | Self::FilterLists | Self::UriSchemeRegistration => {
                GuaranteeKind::Security
            }
            Self::BodyView => GuaranteeKind::Feature,
            Self::PressureSignal | Self::TimingWheel => GuaranteeKind::Resource,
        }
    }

    /// What Sift does. Derived from [`guarantee`](Self::guarantee) wherever the rule
    /// allows, and stated where it does not.
    #[must_use]
    pub const fn response(self) -> Response {
        match self {
            // Security guarantees refuse — with one stated exception below.
            Self::CredentialStore | Self::UriSchemeRegistration => Response::Refuse,
            // A security guarantee that degrades *safely*: an absent authority denies, so
            // the failure direction is a message with missing images rather than a message
            // that quietly fetched something. Refusing to run at all would be a worse
            // answer to the same risk.
            Self::FilterLists => Response::Degrade,
            Self::BodyView | Self::PressureSignal => Response::Degrade,
            // A resource guarantee that refuses, because the fallback D-25 forbids is the
            // only other way to do the work at all.
            Self::TimingWheel => Response::Refuse,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CredentialStore => "credential-store",
            Self::FilterLists => "filter-lists",
            Self::BodyView => "body-view",
            Self::PressureSignal => "pressure-signal",
            Self::TimingWheel => "timing-wheel",
            Self::UriSchemeRegistration => "uri-scheme-registration",
        }
    }
}

impl fmt::Display for ProcessCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn there_are_eight_account_conditions() {
        assert_eq!(AccountCondition::ALL.len(), 8);
        let names: BTreeSet<&str> = AccountCondition::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 8, "two conditions share a name");
    }

    #[test]
    fn declaration_order_is_precedence_order() {
        // Ord is what resolve() uses, so if the two ever disagreed an account would show
        // the wrong problem — silently, and only in the cases where two facts hold.
        let mut sorted = AccountCondition::ALL.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, AccountCondition::ALL);
    }

    #[test]
    fn the_first_that_applies_wins() {
        use AccountCondition::{Attention, Degraded, NeedsAuthentication, PausedByDataCap};
        assert_eq!(
            AccountCondition::resolve(&[Degraded, NeedsAuthentication, Attention]),
            NeedsAuthentication
        );
        // The pairing D-49 admits it hides: over the cap *and* on a degraded server.
        assert_eq!(
            AccountCondition::resolve(&[Degraded, PausedByDataCap]),
            PausedByDataCap
        );
    }

    #[test]
    fn nothing_applying_is_healthy() {
        assert_eq!(AccountCondition::resolve(&[]), AccountCondition::Healthy);
    }

    #[test]
    fn exactly_one_condition_must_reach_a_user_with_no_window() {
        let reaching: Vec<_> = AccountCondition::ALL
            .iter()
            .filter(|c| c.reaches_the_user_without_a_window())
            .collect();
        assert_eq!(reaching, vec![&AccountCondition::NeedsAuthentication]);
    }

    #[test]
    fn recovering_asks_nothing_of_the_user() {
        // It is progress, not a fault. Presenting it as one would be the dishonesty FR-12
        // rejects elsewhere.
        assert!(!AccountCondition::Recovering.asks_something_of_the_user());
        assert!(!AccountCondition::Healthy.asks_something_of_the_user());
        for c in AccountCondition::ALL {
            if !matches!(c, AccountCondition::Recovering | AccountCondition::Healthy) {
                assert!(
                    c.asks_something_of_the_user(),
                    "{c} leaves the user nothing to do"
                );
            }
        }
    }

    #[test]
    fn the_three_pauses_are_three_conditions() {
        // User pause, cap pause, and the absence of a path are not one flag. A capped
        // account resumes on its own; a user-paused one never does; offline is a property
        // of the network shared by every account and is deliberately absent from the set.
        assert_ne!(
            AccountCondition::PausedByUser,
            AccountCondition::PausedByDataCap
        );
        assert!(
            !AccountCondition::ALL
                .iter()
                .any(|c| c.name().contains("offline")),
            "offline is a policy tier, not an account condition"
        );
    }

    #[test]
    fn the_process_condition_set_is_closed_and_each_names_its_guarantee() {
        assert_eq!(ProcessCondition::ALL.len(), 6);
        for c in ProcessCondition::ALL {
            // The membership test D-71 states: an entry that cannot name the guarantee it
            // protects does not belong.
            let _ = c.guarantee();
            let _ = c.response();
        }
    }

    #[test]
    fn a_security_guarantee_refuses_unless_it_can_degrade_safely() {
        for c in ProcessCondition::ALL {
            if c.guarantee() == GuaranteeKind::Security && c.response() == Response::Degrade {
                // Only one may take this path, and only because an absent authority denies.
                assert_eq!(*c, ProcessCondition::FilterLists);
            }
        }
    }

    #[test]
    fn a_feature_absence_never_refuses() {
        for c in ProcessCondition::ALL {
            if c.guarantee() == GuaranteeKind::Feature {
                assert_eq!(
                    c.response(),
                    Response::Degrade,
                    "{c} refuses over a feature"
                );
            }
        }
    }

    #[test]
    fn losing_the_wheel_refuses_rather_than_falling_back_to_sleeping() {
        // The fallback is the thing D-25 prohibits outright, so there is no fallback.
        assert_eq!(ProcessCondition::TimingWheel.response(), Response::Refuse);
    }
}

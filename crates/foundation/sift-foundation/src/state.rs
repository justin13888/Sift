//! D-68 — the state register. Every state the presentation layer can raise, with its
//! parameters.
//!
//! # The rule, and why it is a type rather than a convention
//!
//! D-56: **no user-visible string crosses the shell boundary.** The layer emits
//! identified, parameterized states; each shell renders and translates them. That is what
//! makes NFR-51's locale awareness and the second shell possible without the layer
//! knowing a language.
//!
//! NFR-54 pulls the other way: display names, subjects, snippets, folder and tag names
//! and attachment names **must** cross, because they are the message. D-68 reconciles the
//! two by **authorship**, and the test is who wrote the words:
//!
//! - A **content value** came from the user's mail or from a provider. It crosses, as
//!   data. It is normalized under NFR-54 first, and it is **never translated**. Being a
//!   parameter does not launder it — it remains attacker-controlled.
//! - **Chrome prose** is a sentence Sift says to the user. It never crosses. It is a
//!   state identifier plus parameters, and the shell supplies every word around it.
//!
//! So [`Parameter`] has no prose variant. Not "should not"; cannot. A state that needs to
//! say something new needs a new identifier here, which is a change both shells see.
//!
//! # Adding a state
//!
//! A new state arrives here, **with its parameters, in the same change that raises it**.
//! Every state must have a rendering in both shells or in neither, and under D-66 the
//! check is at build time: exhaustiveness over [`StateId`] is enforced when the project is
//! built, so a state added here without a rendering does not compile.

use crate::condition::AccountCondition;
use crate::identity::{AccountId, LocalId};
use core::fmt;
use core::time::Duration;

/// Text that came from the user's mail or from a provider.
///
/// Crossing the boundary as data is the whole point; **it is not thereby trustworthy**.
/// It is still attacker-controlled after it becomes a parameter, and a shell renders it
/// as a value inside its own sentence rather than as a sentence.
///
/// Construct through [`ContentValue::normalized`], which is the NFR-54 obligation made
/// into a step somebody has to take: bidi controls isolated rather than stripped, other
/// control characters removed, and length bounded at L-25 after normalization, at a
/// grapheme boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentValue(String);

impl ContentValue {
    /// Wrap a string that has **already** been normalized under NFR-54.
    ///
    /// Named to be conspicuous at the call site. The only caller that should exist is the
    /// normalizer itself; anything else is asserting a property it did not establish.
    #[must_use]
    pub fn assume_normalized(s: String) -> Self {
        Self(s)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A state's parameter.
///
/// Deliberately without a prose variant — see the module documentation. The variants are
/// exactly what D-68 permits: "a content value, a number, a duration, a timestamp, an
/// identifier, or another state".
#[derive(Debug, Clone, PartialEq)]
pub enum Parameter {
    /// Attacker-controlled text, normalized, never translated.
    Content(ContentValue),
    Number(i64),
    Count(u64),
    Duration(Duration),
    /// Milliseconds since the Unix epoch.
    Timestamp(u64),
    Account(AccountId),
    Message(LocalId),
    /// "…or another state." A state may carry a state, which is how a condition reaches
    /// the register without the register restating D-49's set.
    Condition(AccountCondition),
    /// An identifier Sift assigned to something enumerable — an intent, a rule, a stage.
    /// A *stable identifier*, not a name to show; the shell supplies the words.
    Identified(&'static str),
}

/// The stable discriminant of a state, and the thing both shells must handle
/// exhaustively.
///
/// Under D-66 an unknown discriminant is a **build failure, not a runtime case**, and a
/// runtime fallback must not be added. That is deliberately the opposite of the provider
/// model's "an unrecognised capability is ignored, not fatal" — and it is justified
/// because both sides of this boundary ship in one binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateId {
    AccountCondition,
    ReconciliationNotice,
    DegradationExplanation,
    NotCached,
    NotAvailable,
    ResourceBlocked,
    ResourceUnavailable,
    ContentWithheldByShed,
    BlockerDisagreement,
    NoSpecialUseFolderResolved,
    NoMailHandlerConfigured,
    MessageIsEncrypted,
    ContrastRepairFailed,
    AttachmentWritten,
    IntentQuarantined,
    IntentExpired,
    StageFailedOnCaughtPanic,
}

impl StateId {
    pub const ALL: &'static [Self] = &[
        Self::AccountCondition,
        Self::ReconciliationNotice,
        Self::DegradationExplanation,
        Self::NotCached,
        Self::NotAvailable,
        Self::ResourceBlocked,
        Self::ResourceUnavailable,
        Self::ContentWithheldByShed,
        Self::BlockerDisagreement,
        Self::NoSpecialUseFolderResolved,
        Self::NoMailHandlerConfigured,
        Self::MessageIsEncrypted,
        Self::ContrastRepairFailed,
        Self::AttachmentWritten,
        Self::IntentQuarantined,
        Self::IntentExpired,
        Self::StageFailedOnCaughtPanic,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AccountCondition => "account-condition",
            Self::ReconciliationNotice => "reconciliation-notice",
            Self::DegradationExplanation => "degradation-explanation",
            Self::NotCached => "not-cached",
            Self::NotAvailable => "not-available",
            Self::ResourceBlocked => "resource-blocked",
            Self::ResourceUnavailable => "resource-unavailable",
            Self::ContentWithheldByShed => "content-withheld-by-a-shed",
            Self::BlockerDisagreement => "blocker-disagreement",
            Self::NoSpecialUseFolderResolved => "no-special-use-folder-resolved",
            Self::NoMailHandlerConfigured => "no-mail-handler-configured",
            Self::MessageIsEncrypted => "message-is-encrypted-and-unreadable",
            Self::ContrastRepairFailed => "contrast-repair-failed",
            Self::AttachmentWritten => "attachment-written",
            Self::IntentQuarantined => "intent-quarantined",
            Self::IntentExpired => "intent-expired",
            Self::StageFailedOnCaughtPanic => "stage-failed-on-a-caught-panic",
        }
    }

    /// Which document requires this state be surfaced. Kept next to the state because
    /// D-49 exists precisely because ten documents demanded a surface and none owned it.
    #[must_use]
    pub const fn raised_by(self) -> &'static str {
        match self {
            Self::AccountCondition => "D-49",
            Self::ReconciliationNotice => "D-38",
            Self::DegradationExplanation => "NFR-29",
            Self::NotCached | Self::NotAvailable => "FR-12",
            Self::ResourceBlocked => "FR-33",
            Self::ResourceUnavailable => "D-91",
            Self::ContentWithheldByShed => "L1 in memory pressure",
            Self::BlockerDisagreement => "D-10",
            Self::NoSpecialUseFolderResolved => "FR-5",
            Self::NoMailHandlerConfigured => "FR-41",
            Self::MessageIsEncrypted => "D-39",
            Self::ContrastRepairFailed => "NFR-47",
            Self::AttachmentWritten => "NFR-53",
            Self::IntentQuarantined | Self::IntentExpired => "mutations",
            Self::StageFailedOnCaughtPanic => "D-47",
        }
    }
}

impl fmt::Display for StateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One raised state: its identifier and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    id: StateId,
    parameters: Vec<Parameter>,
}

impl State {
    #[must_use]
    pub fn new(id: StateId, parameters: Vec<Parameter>) -> Self {
        Self { id, parameters }
    }

    #[must_use]
    pub const fn id(&self) -> StateId {
        self.id
    }

    #[must_use]
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    // ---- Constructors for the states whose parameter lists are easy to get wrong. ----

    /// FR-12. Distinct from [`not_available`](Self::not_available), and the distinction is
    /// the requirement: "not cached" is fetchable when the network returns; "not
    /// available" means the server no longer has it. Collapsing them is the dishonesty
    /// FR-12 names.
    #[must_use]
    pub fn not_cached(message: LocalId) -> Self {
        Self::new(StateId::NotCached, vec![Parameter::Message(message)])
    }

    /// FR-12, with the reason it cannot be fetched now.
    #[must_use]
    pub fn not_available(message: LocalId, reason: &'static str) -> Self {
        Self::new(
            StateId::NotAvailable,
            vec![Parameter::Message(message), Parameter::Identified(reason)],
        )
    }

    /// L1 in memory pressure. **The shed must be named, not a rule.**
    ///
    /// An image withheld because the pressure governor dropped the filter engine did not
    /// match a filter rule, and saying it did would be a lie the user could act on — they
    /// would go looking for the rule. L1 is not a rare event on the reference rig, so this
    /// is a state users will meet.
    #[must_use]
    pub fn content_withheld_by_a_shed(count: u64) -> Self {
        Self::new(
            StateId::ContentWithheldByShed,
            vec![Parameter::Count(count)],
        )
    }

    /// D-10. The filter engine is the authority and the compiled engine rules are the
    /// backstop; **if the two disagree that is a bug**, and it is surfaced rather than
    /// silently resolved in either direction.
    #[must_use]
    pub fn blocker_disagreement(resource: ContentValue, authority: bool, backstop: bool) -> Self {
        Self::new(
            StateId::BlockerDisagreement,
            vec![
                Parameter::Content(resource),
                Parameter::Identified(if authority {
                    "authority-allowed"
                } else {
                    "authority-blocked"
                }),
                Parameter::Identified(if backstop {
                    "backstop-allowed"
                } else {
                    "backstop-blocked"
                }),
            ],
        )
    }

    /// NFR-53. The **final path, as written** — shown to the user before the write, never
    /// the sender's filename.
    #[must_use]
    pub fn attachment_written(final_path: ContentValue) -> Self {
        Self::new(
            StateId::AttachmentWritten,
            vec![Parameter::Content(final_path)],
        )
    }

    /// D-47. A caught panic produces a state rather than only a degradation: the user sees
    /// FR-9's raw view, and the fact that a stage failed is recorded rather than absorbed
    /// as an ordinary parse failure.
    #[must_use]
    pub fn stage_failed_on_caught_panic(message: LocalId, stage: &'static str) -> Self {
        Self::new(
            StateId::StageFailedOnCaughtPanic,
            vec![Parameter::Message(message), Parameter::Identified(stage)],
        )
    }

    /// FR-41. Carries nothing: the shell says what it means, in its own language.
    #[must_use]
    pub fn no_mail_handler_configured() -> Self {
        Self::new(StateId::NoMailHandlerConfigured, Vec::new())
    }

    /// D-49, carrying the condition rather than restating the set.
    #[must_use]
    pub fn account_condition(account: AccountId, condition: AccountCondition) -> Self {
        Self::new(
            StateId::AccountCondition,
            vec![Parameter::Account(account), Parameter::Condition(condition)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_state_has_a_unique_identifier() {
        let names: BTreeSet<&str> = StateId::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names.len(), StateId::ALL.len(), "two states share a name");
    }

    #[test]
    fn every_state_names_what_requires_it() {
        // D-49 exists because ten documents demanded a surface and no document owned it.
        // A state that cannot say what demanded it is one nobody will know how to remove.
        for s in StateId::ALL {
            assert!(
                !s.raised_by().is_empty(),
                "{s} names nothing that raises it"
            );
        }
    }

    #[test]
    fn not_cached_and_not_available_are_two_states() {
        // "The UI MUST clearly distinguish 'not cached' from 'not available'." One is
        // fetchable when the network returns; the other means the server no longer has it.
        // Collapsing them is the specific dishonesty FR-12 was written against.
        assert_ne!(StateId::NotCached, StateId::NotAvailable);
        let cached = State::not_cached(LocalId::from_u128(1));
        let unavailable = State::not_available(LocalId::from_u128(1), "server-no-longer-has-it");
        assert_ne!(cached.id(), unavailable.id());
        assert!(
            unavailable.parameters().len() > cached.parameters().len(),
            "not-available carries the reason; not-cached has none to carry"
        );
    }

    #[test]
    fn a_shed_names_the_shed_rather_than_a_rule() {
        // An image withheld because L1 dropped the filter engine did not match a rule, and
        // saying it did sends the user looking for one.
        let s = State::content_withheld_by_a_shed(3);
        assert_eq!(s.id(), StateId::ContentWithheldByShed);
        assert_eq!(s.parameters(), &[Parameter::Count(3)]);
    }

    #[test]
    fn a_state_with_no_parameters_is_allowed() {
        // FR-41's absent mail handler carries nothing: the shell supplies every word.
        let s = State::no_mail_handler_configured();
        assert!(s.parameters().is_empty());
    }

    #[test]
    fn a_condition_crosses_as_a_state_rather_than_as_words() {
        let s = State::account_condition(
            AccountId::from_u128(9),
            AccountCondition::NeedsAuthentication,
        );
        assert!(matches!(s.parameters()[1], Parameter::Condition(_)));
    }

    #[test]
    fn a_content_value_is_conspicuous_to_construct() {
        // The only thing that should call this is the NFR-54 normalizer. Anything else is
        // asserting a property it did not establish, and the name is there to make that
        // visible at the call site rather than in a doc comment nobody opens.
        let v = ContentValue::assume_normalized("Ünïcode subject".to_owned());
        assert_eq!(v.as_str(), "Ünïcode subject");
    }
}

#[cfg(test)]
mod agrees_with_the_specification {
    //! The register and `docs/architecture/state-register.md` are two statements of the
    //! same seventeen states. D-68 requires a new state arrive in the document "with its
    //! parameters, in the same change that raises it"; this is what notices when it did
    //! not.

    use super::*;
    use std::collections::BTreeSet;

    const DOC: &str = include_str!("../../../../docs/architecture/state-register.md");

    /// The register's own table rows, kebab-cased to match [`StateId::name`].
    fn states_in_doc() -> BTreeSet<String> {
        DOC.lines()
            .filter(|l| l.starts_with("| ") && l.matches('|').count() >= 4)
            .filter_map(|l| l.split('|').nth(1))
            .map(str::trim)
            // Skip the header and the content-value/chrome-prose table above it.
            .filter(|n| {
                !n.is_empty()
                    && !n.starts_with("**")
                    && !n.starts_with("---")
                    && *n != "State"
                    && *n != "Category"
            })
            .map(|n| n.to_lowercase().replace(' ', "-"))
            .collect()
    }

    #[test]
    fn the_register_and_the_document_hold_the_same_states() {
        let doc = states_in_doc();
        assert!(
            !doc.is_empty(),
            "no states parsed — has the table format changed?"
        );
        let code: BTreeSet<String> = StateId::ALL.iter().map(|s| s.name().to_owned()).collect();

        let missing: Vec<_> = doc.difference(&code).collect();
        assert!(
            missing.is_empty(),
            "in docs/architecture/state-register.md and not here: {missing:?}\n\
             Under D-66 a state with no rendering does not compile — but a state with no \
             identifier cannot be raised at all."
        );

        let extra: Vec<_> = code.difference(&doc).collect();
        assert!(
            extra.is_empty(),
            "here and not in docs/architecture/state-register.md: {extra:?}\n\
             D-68: a new state arrives in the register, with its parameters, in the same \
             change that raises it."
        );
    }

    #[test]
    fn the_document_still_forbids_prose_as_a_parameter() {
        // The rule Parameter's shape encodes. If the document ever softens it, the type
        // stops being justified and should be revisited rather than left as an accident.
        assert!(
            DOC.contains("prose is the only thing that never crosses"),
            "D-68's heading changed; the no-prose rule Parameter encodes may have moved"
        );
        assert!(
            DOC.contains("Why a parameter may not be prose"),
            "state-register.md no longer argues the rule Parameter's shape enforces"
        );
    }
}

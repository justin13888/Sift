//! D-92 — the stage boundary, where three concerns meet at one point.
//!
//! > "An implementer who introduces a fourth boundary for any of them has made the other two
//! > harder to reason about."
//!
//! The three are **attribution** (D-24's tag changes here, because the subsystem partition
//! splits the pipeline across Parse and Sanitize), **panic containment** (D-47's catch sits
//! here, because a stage is the granularity at which state is discardable), and
//! **cancellation** (observed here and nowhere else — nothing checks cancellation inside the
//! sanitizer's tree walk).
//!
//! What makes one point serve all three is that **each stage is a pure function of its
//! input**. That is the property D-47 relies on: a discarded stage discards cleanly, so a
//! caught panic leaves nothing half-applied.

use sift_alloc::tagged;
use sift_subsystem::Subsystem;

/// The pipeline's stages, in the order the rendering document fixes.
///
/// Sanitization precedes cosmetic filtering so the filter works on a trustable structure;
/// rewriting happens **inside** sanitization so no external-scheme URL survives the stage I2
/// is asserted over; and the transform runs last so it cannot reintroduce what earlier stages
/// removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    MimeParse,
    PartSelection,
    Sanitize,
    CosmeticFilter,
    Bind,
    Transform,
    Render,
}

impl Stage {
    pub const IN_ORDER: &'static [Self] = &[
        Self::MimeParse,
        Self::PartSelection,
        Self::Sanitize,
        Self::CosmeticFilter,
        Self::Bind,
        Self::Transform,
        Self::Render,
    ];

    /// Which subsystem this stage's allocations belong to.
    ///
    /// The partition splits the pipeline across two rows, so **the tag changes mid-run** — at
    /// a stage boundary, which is the same point cancellation and panic containment use.
    #[must_use]
    pub const fn subsystem(self) -> Subsystem {
        match self {
            Self::MimeParse | Self::PartSelection => Subsystem::Parse,
            Self::Sanitize | Self::CosmeticFilter | Self::Transform => Subsystem::Sanitize,
            Self::Bind | Self::Render => Subsystem::Bodyview,
        }
    }

    /// Where a stage added later must enter.
    ///
    /// D-39's decryption seam sits between stages 1 and 2 — **above sanitization** — so
    /// decrypted content passes through sanitization, blocking and rewriting exactly as
    /// plaintext does and gains no exemption from I1 through I10. **Any stage added later
    /// must enter above stage 3 for the same reason.**
    #[must_use]
    pub const fn may_be_inserted_before(self) -> bool {
        matches!(self, Self::MimeParse | Self::PartSelection | Self::Sanitize)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MimeParse => "mime-parse",
            Self::PartSelection => "part-selection",
            Self::Sanitize => "sanitize",
            Self::CosmeticFilter => "cosmetic-filter",
            Self::Bind => "bind",
            Self::Transform => "transform",
            Self::Render => "render",
        }
    }
}

/// What a stage produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    Produced(T),
    /// The message was superseded. Output discarded, **nothing half-applied**.
    Cancelled,
    /// A panic was caught here.
    ///
    /// Produces **a state** rather than only a degradation — the state register carries "stage
    /// failed on a caught panic" with the message and the stage — and is **counted against
    /// the subsystem whose tag was current**, never silently absorbed as an ordinary parse
    /// failure.
    Panicked(Stage),
}

/// Run one stage.
///
/// The single point for all three concerns. `is_cancelled` is consulted **here and nowhere
/// else**: nothing checks cancellation inside the sanitizer's tree walk, because a stage is a
/// pure function of its input and a half-walked tree is not a value anybody wants.
pub fn run_stage<T, F>(stage: Stage, is_cancelled: impl Fn() -> bool, body: F) -> Outcome<T>
where
    F: FnOnce() -> T,
{
    if is_cancelled() {
        return Outcome::Cancelled;
    }
    // Attribution: the tag is re-established here rather than once per message, because the
    // partition splits this pipeline across two subsystems.
    tagged(stage.subsystem(), || {
        // Panic containment, at the same point. AssertUnwindSafe holds because a stage is a
        // pure function of its input: there is no shared state a partially-completed stage
        // could leave observably broken.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
            Ok(value) => Outcome::Produced(value),
            Err(_) => Outcome::Panicked(stage),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quietly<R>(f: impl FnOnce() -> R) -> R {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = f();
        std::panic::set_hook(previous);
        r
    }

    #[test]
    fn the_stages_are_in_the_order_the_pipeline_fixes() {
        // Sanitize before cosmetic filtering, and transform last. Getting this backwards is
        // the failure that "falsifies I2 while every test still passes".
        let mut sorted = Stage::IN_ORDER.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, Stage::IN_ORDER);
        let sanitize = Stage::IN_ORDER.iter().position(|s| *s == Stage::Sanitize);
        let filter = Stage::IN_ORDER
            .iter()
            .position(|s| *s == Stage::CosmeticFilter);
        let transform = Stage::IN_ORDER.iter().position(|s| *s == Stage::Transform);
        assert!(sanitize < filter);
        assert!(filter < transform);
    }

    #[test]
    fn the_tag_changes_partway_through_the_pipeline() {
        // The partition splits it across Parse and Sanitize, which is why attribution is
        // re-established at a stage boundary rather than once per message.
        assert_eq!(Stage::MimeParse.subsystem(), Subsystem::Parse);
        assert_eq!(Stage::Sanitize.subsystem(), Subsystem::Sanitize);
        assert_ne!(Stage::MimeParse.subsystem(), Stage::Sanitize.subsystem());
    }

    #[test]
    fn a_stage_runs_under_its_own_subsystem_tag() {
        let observed = run_stage(Stage::Sanitize, || false, sift_alloc::current);
        assert_eq!(observed, Outcome::Produced(Subsystem::Sanitize));
    }

    #[test]
    fn cancellation_is_observed_at_the_boundary_and_not_inside() {
        // Nothing checks cancellation inside the sanitizer's tree walk: a stage is a pure
        // function of its input, and a half-walked tree is not a value anybody wants.
        let outcome: Outcome<u32> = run_stage(Stage::Sanitize, || true, || panic!("never runs"));
        assert_eq!(outcome, Outcome::Cancelled);
    }

    #[test]
    fn a_caught_panic_names_the_stage_it_happened_in() {
        // It produces a state rather than only a degradation, and the state carries the
        // message and the stage.
        let outcome: Outcome<u32> =
            quietly(|| run_stage(Stage::Transform, || false, || panic!("hostile")));
        assert_eq!(outcome, Outcome::Panicked(Stage::Transform));
    }

    #[test]
    fn a_caught_panic_does_not_leave_the_tag_set() {
        // D-47 catches at exactly this point, so a tag left set would misattribute everything
        // that ran next on this thread.
        let before = sift_alloc::current();
        let _: Outcome<u32> = quietly(|| run_stage(Stage::Sanitize, || false, || panic!()));
        assert_eq!(sift_alloc::current(), before);
    }

    #[test]
    fn a_panic_is_distinguishable_from_a_cancellation() {
        // One is a message Sift handled correctly; the other is a defect somebody has to see.
        let cancelled: Outcome<u32> = run_stage(Stage::Sanitize, || true, || 1);
        let panicked: Outcome<u32> = quietly(|| run_stage(Stage::Sanitize, || false, || panic!()));
        assert_ne!(cancelled, panicked);
    }

    #[test]
    fn a_new_stage_must_enter_above_sanitization() {
        // D-39's decryption seam sits between stages 1 and 2 so decrypted content gains no
        // exemption from I1 through I10, and every later addition inherits the rule.
        assert!(Stage::MimeParse.may_be_inserted_before());
        assert!(Stage::Sanitize.may_be_inserted_before());
        assert!(
            !Stage::Transform.may_be_inserted_before(),
            "a stage could enter below the sanitizer"
        );
        assert!(!Stage::Render.may_be_inserted_before());
    }
}

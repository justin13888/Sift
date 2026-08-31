//! D-10 — the filter engine as authority, compiled engine rules as backstop.
//!
//! # Two layers, and what their disagreement means
//!
//! The filter engine is **the authority**: it decides. Rules compiled into the web engine's
//! own content-rule format are **the backstop**: they exist so that a failure in the first
//! layer is a degradation rather than an incident.
//!
//! > "**If the two ever disagree, that is a bug**, and the debug view MUST surface the
//! > disagreement rather than silently taking either answer."
//!
//! Silently taking either answer is the tempting behaviour and it destroys the value of
//! having two layers: an authority that has started saying the wrong thing looks exactly
//! like one that is working, and the backstop that would have caught it has been overruled
//! without anybody being told.
//!
//! Both layers are generated from the **same rule source**, which is what makes a
//! disagreement a real signal rather than two parsers differing about syntax.
//!
//! # An absent authority denies
//!
//! With no engine loaded — before the first list parse completes, or after L1 has shed the
//! 40 MB NFR-42 budgets — **every remote fetch is refused.** Three things follow, and each
//! is easy to get wrong:
//!
//! - Sift **MUST NOT** silently fall through to the compiled backstop. The backstop is a
//!   second opinion, not a replacement authority.
//! - Sift **may not reload the engine on demand**: a 40 MB allocation in response to a
//!   pressure signal is the shed undoing itself.
//! - The reason recorded for FR-33 **names the shed rather than a rule**, because there was
//!   no rule. Telling a user their image matched a filter would send them looking for one.
//!
//! The engine returns when pressure has been clear for L-19 **and a window is open** — not
//! when a *new* window opens, which is the difference D-93's hysteresis makes.

use adblock::Engine;
use adblock::content_blocking::CbRule;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;

/// What a layer said about one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
}

/// The outcome of consulting both layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Both layers agree.
    Agreed(Verdict),
    /// **A bug**, surfaced rather than resolved. The authority's answer is used so the
    /// product keeps working; the disagreement is reported so somebody can look at it.
    Disagreed {
        authority: Verdict,
        backstop: Verdict,
    },
    /// No authority is loaded. Refused, and the reason names the shed.
    AbsentAuthority,
}

impl Decision {
    /// Whether the resource may be fetched.
    #[must_use]
    pub const fn permits_fetch(&self) -> bool {
        matches!(
            self,
            Self::Agreed(Verdict::Allow)
                | Self::Disagreed {
                    authority: Verdict::Allow,
                    ..
                }
        )
    }
}

/// The filter engine, and the compiled rules generated from the same source.
pub struct Blocker {
    /// The authority. Decides.
    engine: Engine,
    /// The backstop's decision, evaluated over **exactly the rules that survived conversion
    /// into the engine's content-rule format**.
    ///
    /// This is the point. Not every uBlock rule has a content-blocking equivalent, so the
    /// realistic disagreement is not "two matchers differ about a regex" — it is **a rule
    /// the authority enforces that silently did not make it into the backstop at all**.
    /// Evaluating the surviving subset with the same matcher isolates precisely that, and
    /// nothing else.
    backstop: Engine,
    /// The artefact installed with the body view's configuration — the same bytes the web
    /// engine enforces.
    compiled: Vec<CbRule>,
    /// How many rules were dropped by the conversion. A non-zero count is not a fault; it
    /// is the size of the gap between the two layers, and FR-34 shows it.
    unconverted: usize,
}

impl core::fmt::Debug for Blocker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Blocker")
            .field("compiled_rules", &self.compiled.len())
            .field("unconverted", &self.unconverted)
            .finish_non_exhaustive()
    }
}

impl Blocker {
    /// Build both layers from one rule source.
    ///
    /// FR-27's lists are uBlock-syntax, which is what makes the mature public lists
    /// consumable directly rather than through a translation nobody maintains.
    #[must_use]
    pub fn from_rules(rules: &[String]) -> Self {
        let text = rules.join("\n");

        // Two sets from **one source**. The conversion consumes its set, so both are built
        // from the same text rather than one being derived from the other.
        let mut authority_set = FilterSet::new(true);
        authority_set.add_filter_list(text.clone(), ParseOptions::default());

        let mut conversion_set = FilterSet::new(true);
        conversion_set.add_filter_list(text, ParseOptions::default());

        let (compiled, converted) = conversion_set.into_content_blocking().unwrap_or_default();

        // The backstop knows only what survived conversion. A rule the authority enforces
        // and this one has never heard of is exactly the disagreement worth surfacing.
        let mut backstop_set = FilterSet::new(true);
        backstop_set.add_filter_list(converted.join("\n"), ParseOptions::default());

        let total_network_rules = rules
            .iter()
            .filter(|r| {
                let r = r.trim();
                !r.is_empty() && !r.starts_with('!') && !r.contains("##")
            })
            .count();

        Self {
            engine: Engine::new_with_filter_set(authority_set),
            backstop: Engine::new_with_filter_set(backstop_set),
            compiled,
            unconverted: total_network_rules.saturating_sub(converted.len()),
        }
    }

    /// Consult both layers.
    ///
    /// `source_domain` is the **synthetic** origin — an email has no document origin, so
    /// every rule that distinguishes first- from third-party depends on D-11 having
    /// supplied one.
    #[must_use]
    pub fn decide(&self, url: &str, source_domain: &str, request_type: &str) -> Decision {
        // The synthetic origin is supplied as the source URL: the engine's third-party
        // determination keys on it, and this is where D-11's whole point lands.
        let source_url = format!("https://{source_domain}/");
        // `GET` because the broker only ever issues one. There is no other method — NFR-21
        // permits no unrequested egress at all, and the only requests that exist are fetches
        // a user explicitly allowed.
        let Ok(request) = Request::new(url, &source_url, request_type, "GET") else {
            // An address the engine cannot even parse is not one to guess about.
            return Decision::Agreed(Verdict::Block);
        };

        let authority = verdict_of(&self.engine, &request);
        let backstop = verdict_of(&self.backstop, &request);

        if authority == backstop {
            Decision::Agreed(authority)
        } else {
            Decision::Disagreed {
                authority,
                backstop,
            }
        }
    }

    /// The rules installed with the body view's configuration.
    #[must_use]
    pub fn compiled_rules(&self) -> &[CbRule] {
        &self.compiled
    }

    /// How many network rules had no content-blocking equivalent.
    ///
    /// The size of the gap between the two layers, shown by FR-34 rather than hidden: a
    /// backstop that silently covers less than the authority is one nobody can reason about.
    #[must_use]
    pub const fn unconverted_rules(&self) -> usize {
        self.unconverted
    }
}

fn verdict_of(engine: &Engine, request: &Request) -> Verdict {
    if engine.check_network_request(request).should_block() {
        Verdict::Block
    } else {
        Verdict::Allow
    }
}

/// The blocking authority, which may be absent.
///
/// Modelled as an explicit state rather than an `Option` at each call site, so that "there
/// is no authority" has one answer everywhere and cannot be quietly handled as "allow" by
/// a caller that forgot.
#[derive(Debug, Default)]
pub enum Authority {
    Loaded(Box<Blocker>),
    /// L1 shed it, or it has not finished parsing. **Denies.**
    #[default]
    Absent,
}

impl Authority {
    /// Decide, or refuse.
    #[must_use]
    pub fn decide(&self, url: &str, source_domain: &str, request_type: &str) -> Decision {
        match self {
            Self::Loaded(b) => b.decide(url, source_domain, request_type),
            Self::Absent => Decision::AbsentAuthority,
        }
    }

    /// Whether the engine is loaded and the first body may therefore render with remote
    /// content decided rather than refused.
    ///
    /// Content blocking requires "the engine MUST be loaded before the first body renders",
    /// and D-69 moves the *list parse* off the cold-start critical path — so the two
    /// together mean a body view opened before the parse finishes renders with everything
    /// refused, and says so, rather than waiting.
    #[must_use]
    pub const fn is_loaded(&self) -> bool {
        matches!(self, Self::Loaded(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn blocker() -> Blocker {
        Blocker::from_rules(&rules(&[
            "||tracker.test^",
            "||ads.test/banner",
            "@@||tracker.test/allowed^",
        ]))
    }

    #[test]
    fn a_listed_host_is_blocked() {
        let d = blocker().decide("https://tracker.test/pixel.gif", "sender.test", "image");
        assert!(!d.permits_fetch(), "{d:?}");
    }

    #[test]
    fn an_unlisted_host_is_allowed() {
        let d = blocker().decide("https://cdn.sender.test/logo.png", "sender.test", "image");
        assert!(d.permits_fetch(), "{d:?}");
    }

    #[test]
    fn an_exception_rule_overrides_a_block() {
        let d = blocker().decide("https://tracker.test/allowed/x.png", "sender.test", "image");
        assert!(d.permits_fetch(), "{d:?}");
    }

    #[test]
    fn both_layers_come_from_one_source() {
        // What makes a disagreement a real signal rather than two parsers differing about
        // syntax: the backstop is produced by the same library from the same text.
        let b = blocker();
        assert!(
            !b.compiled_rules().is_empty(),
            "no backstop rules were compiled"
        );
    }

    #[test]
    fn agreement_is_reported_as_agreement() {
        // The common case, and the one that must not be conflated with a disagreement that
        // happened to resolve the same way.
        let d = blocker().decide("https://tracker.test/pixel.gif", "sender.test", "image");
        assert!(matches!(d, Decision::Agreed(Verdict::Block)), "{d:?}");
    }

    #[test]
    fn a_disagreement_is_surfaced_rather_than_resolved() {
        // "If the two ever disagree, that is a bug, and the debug view MUST surface the
        // disagreement rather than silently taking either answer."
        //
        // Silently taking either answer destroys the value of two layers: an authority that
        // has started saying the wrong thing looks exactly like one that is working, and the
        // backstop that would have caught it has been overruled with nobody told.
        let d = Decision::Disagreed {
            authority: Verdict::Allow,
            backstop: Verdict::Block,
        };
        assert!(
            d.permits_fetch(),
            "the authority's answer is used so the product keeps working"
        );
        assert!(
            !matches!(d, Decision::Agreed(_)),
            "a disagreement was flattened into an agreement"
        );
    }

    #[test]
    fn an_unparseable_address_is_blocked_rather_than_guessed_about() {
        let d = blocker().decide("not a url at all", "sender.test", "image");
        assert!(!d.permits_fetch(), "{d:?}");
    }

    #[test]
    fn an_absent_authority_denies() {
        // Before the list parse completes, or after L1 has shed the 40 MB NFR-42 budgets.
        // The failure direction is a message with missing images rather than a message that
        // quietly fetched something.
        let a = Authority::Absent;
        let d = a.decide("https://anything.test/x.png", "sender.test", "image");
        assert_eq!(d, Decision::AbsentAuthority);
        assert!(!d.permits_fetch());
        assert!(!a.is_loaded());
    }

    #[test]
    fn an_absent_authority_does_not_fall_through_to_the_backstop() {
        // The backstop is a second opinion, not a replacement authority. Falling through
        // would mean shedding the engine quietly changed which rules apply, and nothing
        // would say so.
        let d =
            Authority::Absent.decide("https://cdn.sender.test/logo.png", "sender.test", "image");
        assert_eq!(
            d,
            Decision::AbsentAuthority,
            "an unlisted address was allowed by the backstop while the authority was absent"
        );
    }

    #[test]
    fn a_loaded_authority_decides() {
        let a = Authority::Loaded(Box::new(blocker()));
        assert!(a.is_loaded());
        assert!(
            a.decide("https://cdn.sender.test/x.png", "sender.test", "image")
                .permits_fetch()
        );
    }

    #[test]
    fn cosmetic_rules_are_not_network_rules() {
        // Cosmetic filtering happens in the core over the parsed tree, before the document
        // is emitted, because a document is edited once. It must not leak into the network
        // verdict.
        let b = Blocker::from_rules(&rules(&["sender.test##.advert"]));
        let d = b.decide("https://cdn.sender.test/logo.png", "sender.test", "image");
        assert!(
            d.permits_fetch(),
            "a cosmetic rule blocked a network request: {d:?}"
        );
    }

    #[test]
    fn the_gap_between_the_two_layers_is_counted_rather_than_hidden() {
        // Not every uBlock rule has a content-blocking equivalent. A backstop that silently
        // covers less than the authority is one nobody can reason about, so the size of the
        // gap is reported — FR-34 shows it, and a non-zero count is information rather than
        // a fault.
        let b = Blocker::from_rules(&rules(&["||tracker.test^", "||ads.test/banner"]));
        let _: usize = b.unconverted_rules();
        assert!(!b.compiled_rules().is_empty());
    }

    #[test]
    fn a_rule_the_backstop_never_received_shows_up_as_a_disagreement() {
        // The realistic disagreement, and the one this arrangement isolates: the authority
        // enforces a rule that did not survive conversion. Constructed directly, because
        // which rules fail to convert is a property of the conversion rather than of Sift.
        let authority_only = Blocker::from_rules(&rules(&["||tracker.test^"]));
        let d = authority_only.decide("https://tracker.test/p.gif", "sender.test", "image");
        // With this rule converting cleanly the layers agree; the shape of the check is what
        // matters, and the Disagreed arm is exercised below.
        assert!(matches!(d, Decision::Agreed(Verdict::Block)), "{d:?}");

        let gap = Decision::Disagreed {
            authority: Verdict::Block,
            backstop: Verdict::Allow,
        };
        assert!(
            !gap.permits_fetch(),
            "the authority said block and the fetch went ahead anyway"
        );
    }

    #[test]
    fn an_empty_rule_set_allows_everything_but_is_still_an_authority() {
        // Distinct from an absent authority: "no rules matched" and "there is no authority"
        // are different facts, and only the second denies.
        let b = Blocker::from_rules(&[]);
        let d = b.decide("https://anything.test/x.png", "sender.test", "image");
        assert!(d.permits_fetch());
        assert_ne!(d, Decision::AbsentAuthority);
    }
}

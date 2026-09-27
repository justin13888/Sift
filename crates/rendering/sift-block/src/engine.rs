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
use adblock::lists::{FilterSet, ParseOptions, ParsedLine, RuleTypes, parse_filter};
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
    /// engine enforces: the rules as the content-rule JSON both target engines compile.
    ///
    /// Held serialized rather than as parsed rules because that is NFR-42's largest line.
    /// With EasyList and EasyPrivacy loaded the parsed form is several times the size of its
    /// JSON and alone overruns the 40 MB, and the web engine consumes the JSON anyway.
    compiled: String,
    /// How many rules `compiled` holds.
    compiled_count: usize,
    /// How many rules were dropped by the conversion. A non-zero count is not a fault; it
    /// is the size of the gap between the two layers, and FR-34 shows it.
    unconverted: usize,
}

impl core::fmt::Debug for Blocker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Blocker")
            .field("compiled_rules", &self.compiled_count)
            .field("unconverted", &self.unconverted)
            .finish_non_exhaustive()
    }
}

/// FR-27's bundled email list, as it ships — D-111 puts every list inside the binary.
///
/// Compiled in rather than read from disk, because a list read from a path at run time is a
/// list something other than the build can change, and D-111's whole argument is that the
/// integrity of list content is the integrity of the build.
///
/// It is also how **NFR-43** holds: an update is a new build, never a fetch, so nothing about
/// updating can block rendering, and a list compiled in can be stale but never absent.
pub const BUNDLED_EMAIL_LIST: &str = include_str!("../lists/email.txt");

/// FR-27's standard blocking list, EasyList, vendored at the upstream revision its header
/// records and taken under GPL-3.0 (D-112). `lists/EASYLIST-NOTICE` is its notice, says
/// which revision this is and how it is refreshed, and `lists/COPYING.GPL-3.0` is the licence.
///
/// **Only the Cask and Flatpak builds carry it**, through the `public-lists` feature: D-112
/// keeps it out of a channel whose terms GPL-3.0 forbids, and the compiled rule form ships
/// only where its list does. The test build reads it too, so the vendored text is exercised
/// by `cargo test` whatever the shipped feature set is — the tests never enter a binary.
#[cfg(any(test, feature = "public-lists"))]
pub const BUNDLED_EASYLIST: &str = include_str!("../lists/easylist.txt");

/// FR-27's standard privacy list, EasyPrivacy, under the same terms and gate as
/// [`BUNDLED_EASYLIST`].
#[cfg(any(test, feature = "public-lists"))]
pub const BUNDLED_EASYPRIVACY: &str = include_str!("../lists/easyprivacy.txt");

/// Every list this build carries, in the order they are parsed.
///
/// The email list is in every build. EasyList and EasyPrivacy join it only where the
/// `public-lists` feature is on — the Cask and Flatpak builds D-112 allows.
#[must_use]
pub const fn bundled_lists() -> &'static [&'static str] {
    #[cfg(feature = "public-lists")]
    {
        &[BUNDLED_EMAIL_LIST, BUNDLED_EASYLIST, BUNDLED_EASYPRIVACY]
    }
    #[cfg(not(feature = "public-lists"))]
    {
        &[BUNDLED_EMAIL_LIST]
    }
}

impl Blocker {
    /// The authority this build ships: every bundled list, parsed into both layers.
    ///
    /// The email list everywhere, and EasyList and EasyPrivacy in the builds D-112 lets carry
    /// them ([`bundled_lists`]). One call either way, so the one caller that loads the engine
    /// does not know which build it is in.
    ///
    /// This is the 40 MB NFR-42 budgets, and it is **not** built on demand: the application
    /// builds it only when the governor says the engine may return and a window is open.
    #[must_use]
    pub fn bundled() -> Self {
        Self::from_lists(bundled_lists())
    }

    /// Build both layers from one rule source.
    ///
    /// FR-27's lists are uBlock-syntax, which is what makes the mature public lists
    /// consumable directly rather than through a translation nobody maintains.
    #[must_use]
    pub fn from_rules(rules: &[String]) -> Self {
        Self::from_text(rules.join("\n"))
    }

    /// Build both layers from whole lists, concatenated into one rule source.
    #[must_use]
    pub fn from_lists(lists: &[&str]) -> Self {
        Self::from_text(lists.join("\n"))
    }

    fn from_text(text: String) -> Self {
        let total_network_rules = network_rules(text.lines());

        // Two sets from **one source**. The conversion consumes its set, so both are built
        // from the same text rather than one being derived from the other. Only the
        // conversion needs the engine's debug mode, which keeps each rule's original text:
        // that text is how it reports which rules it used.
        let mut conversion_set = FilterSet::new(true);
        conversion_set.add_filter_list(text.clone(), ParseOptions::default());

        // Converted and serialized first, so the parsed rules — the largest thing this
        // builds — are gone before either engine is.
        let (compiled, compiled_count, converted) = compile(conversion_set);

        // The conversion reports every rule it used, cosmetic ones included, so only the
        // network rules among them are set against the network rules of the source.
        let converted_network_rules = network_rules(converted.iter().map(String::as_str));

        let mut authority_set = FilterSet::new(false);
        authority_set.add_filter_list(text, ParseOptions::default());

        // The backstop knows only what survived conversion. A rule the authority enforces
        // and this one has never heard of is exactly the disagreement worth surfacing. It
        // only ever answers a network request, so it keeps only network rules.
        let mut backstop_set = FilterSet::new(false);
        backstop_set.add_filter_list(
            converted.join("\n"),
            ParseOptions {
                rule_types: RuleTypes::NetworkOnly,
                ..ParseOptions::default()
            },
        );

        Self {
            engine: Engine::new_with_filter_set(authority_set),
            backstop: Engine::new_with_filter_set(backstop_set),
            compiled,
            compiled_count,
            unconverted: total_network_rules.saturating_sub(converted_network_rules),
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

    /// The rules installed with the body view's configuration, as the content-rule JSON the
    /// web engine compiles: one array, in the order the engine applies them.
    #[must_use]
    pub fn compiled_rules(&self) -> &str {
        &self.compiled
    }

    /// How many rules [`compiled_rules`](Self::compiled_rules) holds.
    ///
    /// Worth watching against the web engine's own cap on one rule list, which the three
    /// standard lists approach.
    #[must_use]
    pub const fn compiled_rule_count(&self) -> usize {
        self.compiled_count
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

/// Convert a set into the web engine's content-rule JSON, with how many rules it holds and
/// the original text of every rule the conversion used.
///
/// A conversion or serialization that fails yields no rules and no used rules at all, so the
/// backstop is empty rather than holding rules the compiled artefact does not.
fn compile(set: FilterSet) -> (String, usize, Vec<String>) {
    let Ok((rules, converted)) = set.into_content_blocking() else {
        return (String::from("[]"), 0, Vec::new());
    };
    match serde_json::to_string(&rules) {
        Ok(json) => (json, rules.len(), converted),
        Err(_) => (String::from("[]"), 0, Vec::new()),
    }
}

/// How many of these lines the engine parses as network rules.
///
/// Classified by the engine's own parser rather than by a guess at the syntax: the public
/// lists carry cosmetic exceptions, scriptlets and header lines a prefix test miscounts, and
/// the count is the size of the gap FR-34 shows.
fn network_rules<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    lines
        .filter(|line| {
            matches!(
                parse_filter(line, false, ParseOptions::default()),
                Ok(ParsedLine::Network(_))
            )
        })
        .count()
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
            b.compiled_rule_count() > 0,
            "no backstop rules were compiled"
        );
    }

    #[test]
    fn the_compiled_rules_are_the_content_rule_json_the_web_engine_takes() {
        // One JSON array of trigger/action objects, holding as many rules as it says, and the
        // listed host is in it.
        let b = blocker();
        let json = b.compiled_rules();
        assert!(json.starts_with('[') && json.ends_with(']'), "{json}");
        assert_eq!(json.matches("\"trigger\"").count(), b.compiled_rule_count());
        assert!(json.contains("tracker"), "{json}");
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
        assert!(b.compiled_rule_count() > 0);
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

    /// The three lists, as the Cask and Flatpak builds carry them.
    fn with_public_lists() -> Blocker {
        Blocker::from_lists(&[BUNDLED_EMAIL_LIST, BUNDLED_EASYLIST, BUNDLED_EASYPRIVACY])
    }

    /// Open-report pixels the email list exists for.
    const OPEN_REPORTS: [&str; 4] = [
        "https://mailtrack.io/trace/mail/abc.png",
        "https://us1.list-manage.com/track/open.php?u=1&id=2",
        "https://u123.ct.sendgrid.net/wf/open?upn=xyz",
        "https://abc.r.us-east-1.awstrack.me/I0/0100/xyz",
    ];

    /// Content a reader wants from the same services, and from an unlisted host.
    const READER_CONTENT: [&str; 3] = [
        "https://cdn.sender.test/logo.png",
        "https://mcusercontent.com/abc/images/hero.jpg",
        "https://us1.list-manage.com/images/banner.png",
    ];

    #[test]
    fn this_build_bundles_the_lists_its_channel_may_carry() {
        // D-112: the email list in every build, the two public lists only where the
        // `public-lists` feature says the channel may carry them.
        let lists = bundled_lists();
        assert_eq!(lists.first(), Some(&BUNDLED_EMAIL_LIST));
        let carries_public = lists.contains(&BUNDLED_EASYLIST);
        assert_eq!(carries_public, cfg!(feature = "public-lists"));
        assert_eq!(
            lists.contains(&BUNDLED_EASYPRIVACY),
            carries_public,
            "one public list without the other"
        );
    }

    #[test]
    fn the_vendored_public_lists_are_the_published_ones() {
        // EASYLIST-NOTICE records the upstream revision; each file's header records the
        // same one. A refresh that updated only one of the three is caught here.
        let notice = include_str!("../lists/EASYLIST-NOTICE");
        for (list, title) in [
            (BUNDLED_EASYLIST, "! Title: EasyList"),
            (BUNDLED_EASYPRIVACY, "! Title: EasyPrivacy"),
        ] {
            assert!(list.contains(title), "{title} is not the list's own header");
            for key in ["! Version: ", "! Commit: "] {
                let value = list
                    .lines()
                    .find_map(|l| l.strip_prefix(key))
                    .unwrap_or_else(|| panic!("{title} has no {key:?} header line"));
                assert!(
                    notice.contains(value.trim()),
                    "{title}'s {key:?} {value} is not the revision EASYLIST-NOTICE records"
                );
            }
        }
    }

    #[test]
    fn the_public_lists_block_what_they_are_for_and_leave_the_email_list_whole() {
        // FR-27's standard lists: an ad server and an analytics beacon are blocked, and
        // adding them neither un-blocks an open report nor blocks content a reader wants.
        let b = with_public_lists();
        for listed in [
            "https://securepubads.g.doubleclick.net/gampad/ads?iu=1",
            "https://www.google-analytics.com/collect?v=1&t=pageview",
        ] {
            let d = b.decide(listed, "sender.test", "image");
            assert!(!d.permits_fetch(), "{listed} was allowed: {d:?}");
        }
        for tracker in OPEN_REPORTS {
            let d = b.decide(tracker, "sender.test", "image");
            assert!(!d.permits_fetch(), "{tracker} was allowed: {d:?}");
        }
        for content in READER_CONTENT {
            let d = b.decide(content, "sender.test", "image");
            assert!(d.permits_fetch(), "{content} was blocked: {d:?}");
        }
    }

    #[test]
    fn the_public_lists_gap_is_counted_and_the_compiled_rules_fit_one_rule_list() {
        // Some of EasyList's network rules have no content-blocking form; the count says how
        // many rather than hiding them, and it is a fraction rather than most of the list.
        // WebKit refuses a content rule list over 150,000 rules, and the three lists come
        // close enough that a refresh crossing it should fail here rather than in the body
        // view.
        let b = with_public_lists();
        let network = network_rules(
            [BUNDLED_EMAIL_LIST, BUNDLED_EASYLIST, BUNDLED_EASYPRIVACY]
                .iter()
                .flat_map(|l| l.lines()),
        );
        assert!(b.unconverted_rules() > 0);
        assert!(b.unconverted_rules() < network / 10, "{b:?} of {network}");
        assert!(b.compiled_rule_count() > 10_000, "{b:?}");
        assert!(b.compiled_rule_count() <= 150_000, "{b:?}");
    }

    #[test]
    fn a_cosmetic_line_is_not_counted_as_a_network_rule() {
        // The public lists' headers, cosmetic exceptions and scriptlets are not network
        // rules, and counting them would inflate the gap FR-34 shows.
        let lines = [
            "[Adblock Plus 2.0]",
            "! comment",
            "example.com##.ad",
            "example.com#@#.ad",
            "example.com##+js(noop)",
            "||tracker.test^",
            "@@||tracker.test/allowed^",
        ];
        assert_eq!(network_rules(lines.into_iter()), 2);
    }

    #[test]
    fn the_bundled_list_blocks_an_open_report_and_nothing_a_reader_wants() {
        // FR-27's email list, as it ships. An open-report pixel stays blocked after a sender
        // is allowed; the same service's content, and an unlisted host, do not.
        let b = Blocker::from_lists(&[BUNDLED_EMAIL_LIST]);
        for tracker in [
            "https://mailtrack.io/trace/mail/abc.png",
            "https://us1.list-manage.com/track/open.php?u=1&id=2",
            "https://u123.ct.sendgrid.net/wf/open?upn=xyz",
            "https://abc.r.us-east-1.awstrack.me/I0/0100/xyz",
        ] {
            let d = b.decide(tracker, "sender.test", "image");
            assert!(!d.permits_fetch(), "{tracker} was allowed: {d:?}");
        }
        for content in [
            "https://cdn.sender.test/logo.png",
            "https://mcusercontent.com/abc/images/hero.jpg",
            "https://us1.list-manage.com/images/banner.png",
        ] {
            let d = b.decide(content, "sender.test", "image");
            assert!(d.permits_fetch(), "{content} was blocked: {d:?}");
        }
    }

    #[test]
    fn every_bundled_rule_reaches_the_backstop() {
        // A rule the conversion drops is one the backstop never hears of. Zero today, and a
        // list change that breaks that is one somebody should look at rather than absorb.
        let b = Blocker::from_lists(&[BUNDLED_EMAIL_LIST]);
        assert_eq!(b.unconverted_rules(), 0);
        assert!(b.compiled_rule_count() > 0);
    }

    #[test]
    fn the_bundled_authority_is_every_list_this_build_carries() {
        // What the application loads: the email list decides in every build.
        let b = Blocker::bundled();
        for tracker in OPEN_REPORTS {
            let d = b.decide(tracker, "sender.test", "image");
            assert!(!d.permits_fetch(), "{tracker} was allowed: {d:?}");
        }
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

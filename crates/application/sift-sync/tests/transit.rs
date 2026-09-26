//! R-5 — D-44's normalization tuple, measured against a header-transit corpus.
//!
//! D-44 corroborates a join with a digest over originator address, origination date,
//! normalized subject and reference chain, "chosen because those are what a compliant relay
//! carries unchanged". R-5 is that this was a hypothesis with no corpus behind it. The corpus
//! is `fixtures/transit.txt`: each group is one message seen at several points in transit, or
//! two distinct messages one scope could propose together, and each records which tuple
//! elements agree and whether the digest does. This test recomputes both under the current
//! rule and fails on any difference, so the recorded measurement cannot drift from the rule.
//!
//! The headers become an [`Envelope`] the way an adapter hands one over: unfolded, the
//! address out of `From`, `Date` parsed to the instant, `References` then `In-Reply-To`.
//! The address and date parsing are the shared ones every adapter uses; nothing here names
//! a provider, because D-12 forbids it at this layer and because the question is about the
//! rule, not about anybody's protocol.

use sift_provider::adapter::Envelope;
use sift_provider::rfc5322;
use sift_sync::join::{self, Tuple};
use std::collections::BTreeSet;

const CORPUS: &str = include_str!("fixtures/transit.txt");
const ELEMENTS: [&str; 4] = ["from", "date", "subject", "references"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Same,
    Distinct,
}

#[derive(Debug)]
struct Group {
    id: String,
    kind: Kind,
    provenance: String,
    source: String,
    equal: BTreeSet<String>,
    agrees: bool,
    observations: Vec<(String, Vec<String>)>,
}

/// `\u{HEX}` to the character, so a fixture can hold what an editor would normalize away.
fn unescape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("\\u{") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 3..];
        let close = after.find('}').expect("an unterminated \\u{ escape");
        let code = u32::from_str_radix(&after[..close], 16).expect("a hexadecimal escape");
        out.push(char::from_u32(code).expect("an escape that is a character"));
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn parse(corpus: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for line in corpus.lines() {
        if let Some(id) = line.strip_prefix("== ") {
            groups.push(Group {
                id: id.trim().to_owned(),
                kind: Kind::Same,
                provenance: String::new(),
                source: String::new(),
                equal: BTreeSet::new(),
                agrees: false,
                observations: Vec::new(),
            });
            continue;
        }
        if line.starts_with('#') && groups.is_empty() {
            continue;
        }
        let Some(group) = groups.last_mut() else {
            assert!(
                line.trim().is_empty(),
                "text before the first group: {line}"
            );
            continue;
        };
        if let Some(label) = line.strip_prefix("-- ") {
            group
                .observations
                .push((label.trim().to_owned(), Vec::new()));
        } else if let Some((_, lines)) = group.observations.last_mut() {
            if !line.is_empty() {
                lines.push(unescape(line));
            }
        } else if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            match key {
                "kind" => {
                    group.kind = match value {
                        "same" => Kind::Same,
                        "distinct" => Kind::Distinct,
                        other => panic!("{}: unknown kind {other}", group.id),
                    }
                }
                "provenance" => group.provenance = value.to_owned(),
                "source" => group.source = value.to_owned(),
                "equal" => {
                    group.equal = value
                        .split(',')
                        .map(|e| e.trim().to_owned())
                        .filter(|e| !e.is_empty() && e != "none")
                        .collect();
                }
                "digest" => {
                    group.agrees = match value {
                        "agrees" => true,
                        "diverges" => false,
                        other => panic!("{}: unknown digest outcome {other}", group.id),
                    }
                }
                other => panic!("{}: unknown key {other}", group.id),
            }
        } else {
            assert!(line.trim().is_empty(), "{}: stray line {line}", group.id);
        }
    }
    groups
}

/// The value of a header, unfolded, with the whitespace after the colon dropped.
fn header(lines: &[String], name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    let mut inside = false;
    for line in lines {
        if line.starts_with([' ', '\t']) {
            // RFC 5322 s2.2.3: unfolding removes the line break and nothing else.
            if inside && let Some(value) = found.as_mut() {
                value.push_str(line);
            }
            continue;
        }
        inside = false;
        if found.is_some() {
            continue;
        }
        if let Some((field, value)) = line.split_once(':')
            && field.eq_ignore_ascii_case(name)
        {
            found = Some(value.trim_start_matches([' ', '\t']).to_owned());
            inside = true;
        }
    }
    found
}

fn envelope(lines: &[String]) -> Envelope {
    // The reference chain as every adapter composes it: `References` in order, then
    // `In-Reply-To` where it is not already there.
    let mut references: Vec<String> = header(lines, "References")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if let Some(parent) = header(lines, "In-Reply-To")
        && let Some(parent) = parent.split_whitespace().next()
        && !references.iter().any(|r| r == parent)
    {
        references.push(parent.to_owned());
    }
    Envelope {
        internet_message_id: header(lines, "Message-ID"),
        references,
        subject: header(lines, "Subject"),
        from: header(lines, "From")
            .as_deref()
            .and_then(rfc5322::address_of),
        origination_date_millis: header(lines, "Date")
            .as_deref()
            .and_then(rfc5322::parse_date_millis)
            .and_then(|m| u64::try_from(m).ok()),
        ..Envelope::default()
    }
}

/// The elements every tuple in the set agrees on.
fn equal_elements(tuples: &[Tuple]) -> BTreeSet<String> {
    let first = &tuples[0];
    let all = |same: &dyn Fn(&Tuple) -> bool| tuples.iter().all(same);
    let mut equal = BTreeSet::new();
    if all(&|t| t.from == first.from) {
        equal.insert("from".to_owned());
    }
    if all(&|t| t.origination_date_millis == first.origination_date_millis) {
        equal.insert("date".to_owned());
    }
    if all(&|t| t.subject == first.subject) {
        equal.insert("subject".to_owned());
    }
    if all(&|t| t.references == first.references) {
        equal.insert("references".to_owned());
    }
    equal
}

fn corpus() -> Vec<Group> {
    let groups = parse(CORPUS);
    let mut ids = BTreeSet::new();
    for g in &groups {
        assert!(
            ids.insert(g.id.clone()),
            "{}: a group id is used twice",
            g.id
        );
        // D-115: nothing enters without a provenance, and a constructed group names what
        // it was constructed after.
        assert!(
            matches!(g.provenance.as_str(), "constructed" | "captured"),
            "{}: no provenance",
            g.id
        );
        assert!(!g.source.is_empty(), "{}: no source", g.id);
        assert!(
            g.equal.iter().all(|e| ELEMENTS.contains(&e.as_str())),
            "{}: an unknown element in {:?}",
            g.id,
            g.equal
        );
        match g.kind {
            Kind::Same => assert!(g.observations.len() >= 2, "{}: one observation", g.id),
            Kind::Distinct => assert_eq!(g.observations.len(), 2, "{}: not a pair", g.id),
        }
    }
    groups
}

#[test]
fn every_group_measures_what_it_records() {
    let mut wrong = Vec::new();
    for g in corpus() {
        let envelopes: Vec<Envelope> = g.observations.iter().map(|(_, l)| envelope(l)).collect();
        let tuples: Vec<Tuple> = envelopes.iter().map(join::tuple).collect();
        let digests: Vec<[u8; 32]> = envelopes.iter().map(join::digest).collect();
        let equal = equal_elements(&tuples);
        let agrees = digests.iter().all(|d| *d == digests[0]);
        if equal != g.equal || agrees != g.agrees {
            wrong.push(format!(
                "{}: recorded equal {:?} digest {}, measured equal {:?} digest {}\n  {:#?}",
                g.id,
                g.equal,
                if g.agrees { "agrees" } else { "diverges" },
                equal,
                if agrees { "agrees" } else { "diverges" },
                tuples,
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_digest_agrees_exactly_when_every_element_does() {
    // The digest is a function of the tuple and nothing else, and an injective one over the
    // corpus: it must neither add agreement the elements do not have nor lose agreement they
    // do. Anything else means the digest is hashing something the tuple does not show.
    for g in corpus() {
        assert_eq!(
            g.agrees,
            g.equal.len() == ELEMENTS.len(),
            "{}: the recorded digest outcome contradicts the recorded elements",
            g.id
        );
    }
}

#[test]
fn every_collision_is_one_the_corpus_names() {
    // The measurement that decides D-44's retreat. A collision is two distinct messages the
    // digest cannot tell apart; each one here is recorded, with the sender behaviour that
    // produces it. A new one fails this test until it is recorded, so a rule change cannot
    // add a collision silently.
    let groups = corpus();
    let collisions: Vec<&str> = groups
        .iter()
        .filter(|g| g.kind == Kind::Distinct && g.agrees)
        .map(|g| g.id.as_str())
        .collect();
    assert_eq!(
        collisions,
        ["duplicate-id-device-same-second", "automated-no-date"],
        "the collisions recorded against R-5 changed"
    );
    // And every one shares the whole tuple with no date to tell the two apart at a
    // resolution finer than the second: the only discriminator left for automated mail.
    for g in groups
        .iter()
        .filter(|g| collisions.contains(&g.id.as_str()))
    {
        assert!(g.equal.contains("date"), "{}", g.id);
    }
}

/// The summary `docs/open-questions.md` records against R-5, under the current rule.
///
/// A count over a constructed sample is a statement about the transformations the corpus
/// chose, not a frequency in real mail, and the documents say so. It is asserted so that the
/// figures they cite cannot drift from the corpus without this test saying so.
const RECORDED: &str = "\
digest rule version 2
from survives 14/16 transit groups
date survives 16/16 transit groups
subject survives 14/16 transit groups
references survives 16/16 transit groups
digest survives 13/16 transit groups
digest collides 2/7 distinct pairs
";

#[test]
fn the_summary_is_the_one_recorded_against_r5() {
    let groups = corpus();
    let same: Vec<&Group> = groups.iter().filter(|g| g.kind == Kind::Same).collect();
    let distinct: Vec<&Group> = groups.iter().filter(|g| g.kind == Kind::Distinct).collect();
    let mut summary = format!("digest rule version {}\n", join::DIGEST_RULE_VERSION);
    for element in ELEMENTS {
        let survived = same.iter().filter(|g| g.equal.contains(element)).count();
        summary += &format!(
            "{element} survives {survived}/{} transit groups\n",
            same.len()
        );
    }
    let survived = same.iter().filter(|g| g.agrees).count();
    summary += &format!("digest survives {survived}/{} transit groups\n", same.len());
    let collided = distinct.iter().filter(|g| g.agrees).count();
    summary += &format!(
        "digest collides {collided}/{} distinct pairs\n",
        distinct.len()
    );
    assert_eq!(summary, RECORDED);
}

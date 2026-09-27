//! D-44 — the one identity rule, and the digest that corroborates a join it never keys.
//!
//! # Scoped before it is keyed
//!
//! Every join is performed **within a scope** the provider already established: the
//! conversation identifier, the reference chain, the sibling folder. Inside that scope the
//! `Message-ID` header **narrows** the candidates, and a stored digest **corroborates**.
//! Nothing keys on either.
//!
//! The asymmetry is the whole decision: **failing to merge is a display defect, and merging
//! wrongly is data loss wearing a display defect's clothes.** An archive applied to the
//! wrong message is gone from the user's inbox and they will not know why. So where a join
//! does not resolve to exactly one candidate, the answer is **distinct messages**, and the
//! near miss is recorded for the FR-33 debug view rather than resolved by preference.
//!
//! # The rule version travels with the digest — D-104
//!
//! A digest computed under one rule version and one computed under another are **not
//! comparable**, and a comparison across versions counts as *no corroboration* rather than
//! as a mismatch. Otherwise a rule change would silently un-merge every thread in the store
//! on the day it shipped.

use sift_foundation::normalize;
use sift_provider::adapter::Envelope;

/// The version of the rule below. **Stored beside every digest**, per D-104.
///
/// It changes when any input to [`digest`] changes — including how one is normalized, which
/// is the change most likely to be made without noticing it is one.
///
/// Version 2 made the subject's whitespace insignificant — R-5's measurement found that a
/// relay's refold, an inserted fold space or stripped trailing spaces diverged the digest
/// on mail that was otherwise unchanged.
pub const DIGEST_RULE_VERSION: u32 = 2;

/// The domain this hash is used in. Present so that a digest can never be confused with a
/// content address, which is the same primitive over the same store.
const CONTEXT: &str = "sift 2026 message join digest v1";

/// D-44's normalization tuple: the four inputs to [`digest`], exactly as it hashes them.
///
/// Public so that R-5's measurement compares the **same** normalized values the digest
/// is taken over, element by element. A measurement that re-derived them would be
/// measuring its own copy of the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tuple {
    /// The originator address, as the adapter recovered it.
    pub from: Option<String>,
    /// The sender's `Date` header, in milliseconds. Second precision is all the header has.
    pub origination_date_millis: Option<u64>,
    /// The subject, normalized under NFR-54.
    pub subject: Option<String>,
    /// `References` then `In-Reply-To`, in order.
    pub references: Vec<String>,
}

/// The normalized tuple for one envelope, under [`DIGEST_RULE_VERSION`].
#[must_use]
pub fn tuple(envelope: &Envelope) -> Tuple {
    Tuple {
        from: envelope.from.clone(),
        origination_date_millis: envelope.origination_date_millis,
        subject: envelope.subject.as_deref().map(subject_key),
        references: envelope.references.clone(),
    }
}

/// The subject as the digest compares it.
///
/// Normalized under NFR-54, so that two spellings of one subject agree — which is the whole
/// reason the normalizer lives in the foundation rather than in the presentation layer.
///
/// Whitespace is then insignificant: every run is one space and the ends are trimmed.
/// Folding a long field is permitted at any whitespace and unfolding restores the fold's own
/// character, so a relay that folds with a tab hands back a tab where the sender wrote a
/// space; non-compliant folders insert a space, and gateways strip trailing ones. None of
/// those is a different message. Whitespace becomes a space **before** the normalizer runs,
/// because the normalizer deletes a tab as a control character and would join two words.
fn subject_key(raw: &str) -> String {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    normalize::for_index(&spaced)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// D-44's corroborating digest.
///
/// Over the originator address, the origination date, the **normalized** subject and the
/// reference chain — the four things D-44 in `docs/storage/data-model.md` names. See
/// [`tuple`].
///
/// A field that is absent contributes its absence rather than an empty string, because
/// "no subject" and "a subject that is empty" are different messages and a digest that
/// could not tell them apart would corroborate a join between them.
#[must_use]
pub fn digest(envelope: &Envelope) -> [u8; 32] {
    digest_of(&tuple(envelope))
}

fn digest_of(tuple: &Tuple) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(CONTEXT);
    let mut field = |tag: u8, value: Option<&str>| {
        hasher.update(&[tag]);
        match value {
            Some(v) => {
                hasher.update(&[1]);
                hasher.update(&(v.len() as u64).to_be_bytes());
                hasher.update(v.as_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
    };
    field(1, tuple.from.as_deref());
    field(
        2,
        tuple
            .origination_date_millis
            .map(|m| m.to_string())
            .as_deref(),
    );
    field(3, tuple.subject.as_deref());
    for reference in &tuple.references {
        field(4, Some(reference));
    }
    *hasher.finalize().as_bytes()
}

/// What a digest comparison is worth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corroboration {
    /// The same rule version, and the digests agree.
    Agrees,
    /// The same rule version, and they do not.
    Disagrees,
    /// D-104: **not evidence of anything.** A rule change must not silently un-merge every
    /// thread in the store on the day it ships.
    NotComparable,
}

/// Compare two stored digests.
#[must_use]
pub fn corroborates(
    (left, left_version): (&[u8], u32),
    (right, right_version): (&[u8], u32),
) -> Corroboration {
    if left_version != right_version {
        return Corroboration::NotComparable;
    }
    if left == right {
        Corroboration::Agrees
    } else {
        Corroboration::Disagrees
    }
}

/// What the scope proposed, and what the corroboration did to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one candidate, corroborated. The join is made.
    One(usize),
    /// **Distinct messages.** Either nothing was proposed, or more than one was, or the
    /// corroboration did not hold. The near miss is worth recording.
    Distinct { near_misses: usize },
}

/// Resolve a join within a scope.
///
/// `candidates` are what the scope proposed, each with its stored digest and rule version.
#[must_use]
pub fn resolve(incoming: (&[u8; 32], u32), candidates: &[(&[u8], u32)]) -> Resolution {
    let mut agreeing = Vec::new();
    let mut near = 0usize;
    for (index, candidate) in candidates.iter().enumerate() {
        match corroborates((incoming.0, incoming.1), *candidate) {
            Corroboration::Agrees => agreeing.push(index),
            // A candidate the scope proposed and the digest did not confirm is exactly the
            // near miss FR-33 item 5 shows: it is what a wrong merge would have looked like
            // a moment before it happened.
            Corroboration::Disagrees | Corroboration::NotComparable => near += 1,
        }
    }
    match agreeing.as_slice() {
        [only] => Resolution::One(*only),
        // Nothing, or more than one. Ambiguity resolves to distinct messages, always —
        // merging wrongly is data loss and failing to merge is a display defect.
        _ => Resolution::Distinct {
            near_misses: near + agreeing.len(),
        },
    }
}

/// One side of a move, as the store holds it: what D-44 scopes, narrows and corroborates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side<'a> {
    /// The provider's conversation identifier — D-44's scope. `None` is no scope at all.
    pub conversation: Option<&'a str>,
    /// `Message-ID`. Narrows, never keys.
    pub internet_message_id: Option<&'a str>,
    /// The stored digest.
    pub digest: &'a [u8],
    /// The rule version stored beside it — D-104.
    pub rule_version: u32,
}

/// Rejoin the messages that arrived in a window to the messages that departed from it —
/// D-44's move join, for an account whose identifiers do not survive a move.
///
/// One [`Resolution`] per arrival, in order. `One(d)` names the index in `departed` whose
/// identity the arrival takes.
///
/// 1. **Scope.** Only a departure in the arrival's own conversation is a candidate. An
///    arrival with no conversation has no scope, and a join with no scope is a scan.
/// 2. **Narrow.** Where both carry a `Message-ID`, a different one is a different message.
///    Where either lacks one, nothing is narrowed: absence is neither a match nor a mismatch.
/// 3. **Corroborate, then resolve** — [`resolve`], so ambiguity is distinct messages.
/// 4. **A departure claimed twice is claimed by nobody.** Two arrivals each resolving to the
///    same departure is the same ambiguity seen from the other side, and choosing between them
///    would be the guess D-44 forbids.
#[must_use]
pub fn rejoin(arrivals: &[Side<'_>], departed: &[Side<'_>]) -> Vec<Resolution> {
    let mut resolved: Vec<(Resolution, usize)> = arrivals
        .iter()
        .map(|arrival| {
            let Some(scope) = arrival.conversation else {
                return (Resolution::Distinct { near_misses: 0 }, 0);
            };
            let in_scope: Vec<usize> = departed
                .iter()
                .enumerate()
                .filter(|(_, d)| d.conversation == Some(scope))
                .filter(
                    |(_, d)| match (arrival.internet_message_id, d.internet_message_id) {
                        (Some(arriving), Some(left)) => arriving == left,
                        _ => true,
                    },
                )
                .map(|(index, _)| index)
                .collect();
            // A digest of the wrong width is not a digest this rule wrote, and corroborates
            // nothing.
            let Ok(digest) = <&[u8; 32]>::try_from(arrival.digest) else {
                return (
                    Resolution::Distinct {
                        near_misses: in_scope.len(),
                    },
                    in_scope.len(),
                );
            };
            let candidates: Vec<(&[u8], u32)> = in_scope
                .iter()
                .map(|&i| (departed[i].digest, departed[i].rule_version))
                .collect();
            let resolution = match resolve((digest, arrival.rule_version), &candidates) {
                Resolution::One(k) => Resolution::One(in_scope[k]),
                distinct @ Resolution::Distinct { .. } => distinct,
            };
            (resolution, in_scope.len())
        })
        .collect();

    let mut claims = vec![0usize; departed.len()];
    for (resolution, _) in &resolved {
        if let Resolution::One(d) = resolution {
            claims[*d] += 1;
        }
    }
    for (resolution, in_scope) in &mut resolved {
        if let Resolution::One(d) = resolution
            && claims[*d] > 1
        {
            *resolution = Resolution::Distinct {
                near_misses: *in_scope,
            };
        }
    }
    resolved.into_iter().map(|(r, _)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side<'a>(
        conversation: Option<&'a str>,
        message_id: Option<&'a str>,
        digest: &'a [u8],
    ) -> Side<'a> {
        Side {
            conversation,
            internet_message_id: message_id,
            digest,
            rule_version: DIGEST_RULE_VERSION,
        }
    }

    #[test]
    fn an_unambiguous_move_rejoins_the_departure_it_corroborates() {
        let d = digest(&envelope());
        let other = [9u8; 32];
        let departed = [
            side(Some("c1"), Some("<a@x>"), &d),
            // Same conversation, different message: narrowed out.
            side(Some("c1"), Some("<b@x>"), &d),
            // Same message identifier, different conversation: out of scope, never scanned.
            side(Some("c2"), Some("<a@x>"), &d),
            // In scope, and the digest refuses it.
            side(Some("c1"), None, &other),
        ];
        assert_eq!(
            rejoin(&[side(Some("c1"), Some("<a@x>"), &d)], &departed),
            vec![Resolution::One(0)]
        );
    }

    #[test]
    fn a_duplicated_message_in_one_conversation_resolves_to_distinct_messages() {
        // Two departures both match and both corroborate: choosing one is a guess.
        let d = digest(&envelope());
        let departed = [
            side(Some("c1"), Some("<a@x>"), &d),
            side(Some("c1"), Some("<a@x>"), &d),
        ];
        let arrival = side(Some("c1"), Some("<a@x>"), &d);
        assert_eq!(
            rejoin(&[arrival, arrival], &departed),
            vec![
                Resolution::Distinct { near_misses: 2 },
                Resolution::Distinct { near_misses: 2 }
            ]
        );
    }

    #[test]
    fn a_departure_two_arrivals_claim_is_claimed_by_neither() {
        // The same ambiguity from the other side: one message left, two identical ones came.
        let d = digest(&envelope());
        let departed = [side(Some("c1"), Some("<a@x>"), &d)];
        let arrival = side(Some("c1"), Some("<a@x>"), &d);
        assert_eq!(
            rejoin(&[arrival, arrival], &departed),
            vec![
                Resolution::Distinct { near_misses: 1 },
                Resolution::Distinct { near_misses: 1 }
            ]
        );
    }

    #[test]
    fn an_arrival_with_no_conversation_is_never_rejoined() {
        let d = digest(&envelope());
        let departed = [side(Some("c1"), Some("<a@x>"), &d)];
        assert_eq!(
            rejoin(&[side(None, Some("<a@x>"), &d)], &departed),
            vec![Resolution::Distinct { near_misses: 0 }]
        );
    }

    #[test]
    fn a_departure_under_another_rule_version_is_a_near_miss_not_a_join() {
        // D-104: not comparable is not evidence.
        let d = digest(&envelope());
        let mut old = side(Some("c1"), Some("<a@x>"), &d);
        old.rule_version = DIGEST_RULE_VERSION - 1;
        assert_eq!(
            rejoin(&[side(Some("c1"), Some("<a@x>"), &d)], &[old]),
            vec![Resolution::Distinct { near_misses: 1 }]
        );
    }

    #[test]
    fn a_missing_message_identifier_narrows_nothing() {
        let d = digest(&envelope());
        let departed = [side(Some("c1"), None, &d)];
        assert_eq!(
            rejoin(&[side(Some("c1"), Some("<a@x>"), &d)], &departed),
            vec![Resolution::One(0)]
        );
    }

    #[test]
    fn a_digest_of_the_wrong_width_corroborates_nothing() {
        let d = digest(&envelope());
        let departed = [side(Some("c1"), None, &d)];
        assert_eq!(
            rejoin(&[side(Some("c1"), None, &d[..16])], &departed),
            vec![Resolution::Distinct { near_misses: 1 }]
        );
    }

    fn envelope() -> Envelope {
        Envelope {
            from: Some("someone@example.test".into()),
            origination_date_millis: Some(1_788_179_696_000),
            subject: Some("A subject".into()),
            references: vec!["<one@x.test>".into()],
            ..Envelope::default()
        }
    }

    #[test]
    fn the_same_message_digests_the_same_way_twice() {
        assert_eq!(digest(&envelope()), digest(&envelope()));
    }

    #[test]
    fn the_remote_identifier_is_not_an_input() {
        // It cannot be: D-44 exists because identifiers are not stable across a move on
        // every provider, and a digest that included one would corroborate nothing there.
        let mut other = envelope();
        other.id = sift_provider::adapter::RemoteMessageId("different".into());
        assert_eq!(digest(&envelope()), digest(&other));
    }

    #[test]
    fn each_of_the_four_inputs_changes_the_answer() {
        let base = digest(&envelope());
        let mut from = envelope();
        from.from = Some("other@example.test".into());
        let mut date = envelope();
        date.origination_date_millis = Some(1);
        let mut subject = envelope();
        subject.subject = Some("Another subject".into());
        let mut chain = envelope();
        chain.references.push("<two@x.test>".into());
        for changed in [&from, &date, &subject, &chain] {
            assert_ne!(base, digest(changed));
        }
    }

    #[test]
    fn an_absent_field_is_not_an_empty_one() {
        // "No subject" and "a subject that is empty" are different messages, and a digest
        // that could not tell them apart would corroborate a join between them.
        let mut absent = envelope();
        absent.subject = None;
        let mut empty = envelope();
        empty.subject = Some(String::new());
        assert_ne!(digest(&absent), digest(&empty));
    }

    #[test]
    fn two_spellings_of_one_subject_agree() {
        // Why the normalizer is on this path at all rather than the raw header: the same
        // subject written two ways is one subject, and a digest over the raw bytes would
        // refuse to corroborate a join between them.
        let mut composed = envelope();
        composed.subject = Some("Re: caf\u{e9}".into());
        let mut decomposed = envelope();
        decomposed.subject = Some("Re: cafe\u{301}".into());
        assert_eq!(
            digest(&composed),
            digest(&decomposed),
            "two spellings of one subject digested differently"
        );
    }

    #[test]
    fn a_control_character_in_a_subject_does_not_change_the_digest() {
        // A sender who could change the digest by inserting an invisible character could
        // stop a message joining the thread it belongs to.
        let mut plain = envelope();
        plain.subject = Some("A subject".into());
        let mut sneaked = envelope();
        sneaked.subject = Some("A sub\u{200b}ject".into());
        assert_eq!(digest(&plain), digest(&sneaked));
    }

    #[test]
    fn whitespace_in_a_subject_is_insignificant_but_words_are_not_joined() {
        // R-5: a refold with a tab, an inserted fold space and stripped trailing spaces
        // each diverged the version-1 digest on mail that was otherwise unchanged.
        let mut plain = envelope();
        plain.subject = Some("A long subject".into());
        for variant in [
            "A long\tsubject",
            "A long  subject",
            "A long subject  ",
            "  A long subject",
            "A long\u{3000}subject",
        ] {
            let mut other = envelope();
            other.subject = Some(variant.into());
            assert_eq!(digest(&plain), digest(&other), "{variant:?}");
        }
        // The tab becomes a space before the normalizer deletes it as a control, so it
        // separates two words rather than joining them.
        let mut joined = envelope();
        joined.subject = Some("A longsubject".into());
        assert_ne!(digest(&plain), digest(&joined));
    }

    #[test]
    fn a_whitespace_only_subject_is_empty_and_still_not_absent() {
        let mut blank = envelope();
        blank.subject = Some("   ".into());
        let mut empty = envelope();
        empty.subject = Some(String::new());
        let mut absent = envelope();
        absent.subject = None;
        assert_eq!(digest(&blank), digest(&empty));
        assert_ne!(digest(&blank), digest(&absent));
    }

    #[test]
    fn a_field_boundary_cannot_be_moved_by_a_sender() {
        // The classic: two different messages hashing the same because one field's tail
        // reads as the next field's head. Each field states its own length.
        let mut a = envelope();
        a.from = Some("ab@x.test".into());
        a.subject = Some("cd".into());
        let mut b = envelope();
        b.from = Some("ab@x.testcd".into());
        b.subject = Some(String::new());
        assert_ne!(digest(&a), digest(&b));
    }

    #[test]
    fn a_digest_from_another_rule_version_corroborates_nothing() {
        // D-104. The alternative silently un-merges every thread in the store on the day a
        // rule changes.
        let d = digest(&envelope());
        assert_eq!(corroborates((&d, 1), (&d, 2)), Corroboration::NotComparable);
        assert_eq!(corroborates((&d, 1), (&d, 1)), Corroboration::Agrees);
    }

    #[test]
    fn exactly_one_corroborated_candidate_joins() {
        let d = digest(&envelope());
        let other = [9u8; 32];
        assert_eq!(
            resolve((&d, 1), &[(&other, 1), (&d, 1)]),
            Resolution::One(1)
        );
    }

    #[test]
    fn two_corroborated_candidates_resolve_to_distinct_messages() {
        // Ambiguity resolves to distinct. Always. Merging wrongly is data loss wearing a
        // display defect's clothes.
        let d = digest(&envelope());
        assert!(matches!(
            resolve((&d, 1), &[(&d, 1), (&d, 1)]),
            Resolution::Distinct { .. }
        ));
    }

    #[test]
    fn a_scope_that_proposed_nothing_is_a_distinct_message_and_not_an_error() {
        assert_eq!(
            resolve((&digest(&envelope()), 1), &[]),
            Resolution::Distinct { near_misses: 0 }
        );
    }

    #[test]
    fn a_candidate_the_digest_refuses_is_recorded_as_a_near_miss() {
        // What a wrong merge would have looked like a moment before it happened — which is
        // what FR-33 item 5 shows.
        let d = digest(&envelope());
        let other = [9u8; 32];
        assert_eq!(
            resolve((&d, 1), &[(&other, 1)]),
            Resolution::Distinct { near_misses: 1 }
        );
    }
}

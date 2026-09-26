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
pub const DIGEST_RULE_VERSION: u32 = 1;

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
        // Normalized under NFR-54, so that two spellings of one subject agree — which is
        // the whole reason the normalizer lives in the foundation rather than in the
        // presentation layer.
        subject: envelope
            .subject
            .as_deref()
            .map(|s| normalize::for_index(s).as_str().to_owned()),
        references: envelope.references.clone(),
    }
}

/// D-44's corroborating digest.
///
/// Over the originator address, the origination date, the **normalized** subject and the
/// reference chain — the four things `docs/mail/identity.md` names. See [`tuple`].
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

#[cfg(test)]
mod tests {
    use super::*;

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

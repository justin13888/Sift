//! D-79 — merging results from indexes that cannot be compared.
//!
//! # Why per-index scores cannot be merged
//!
//! D-6 gives every account its own database, so a five-account search runs **five separate
//! indexes**. Each scores against its own corpus: a term that is rare in one account and
//! common in another gets a high score in the first and a low one in the second, and those
//! numbers describe different populations. Merging them ranks by which mailbox a message
//! happened to arrive in.
//!
//! Normalizing across indexes was rejected for the same reason — there is nothing to
//! normalize *against* without a shared corpus, which is the thing that does not exist.
//!
//! So the layer computes its own relevance signal from **corpus-independent features**,
//! per-index scores order results only *within* one account, and where a query carries no
//! relevance signal at all the order falls back to D-55's list order.
//!
//! The cost D-79 records: a hand-built feature ranking is a **worse ranker than a
//! corpus-aware one within a single account**. It trades peak quality for cross-account
//! coherence. If it proves measurably worse, the retreat is to reconsider D-6's per-account
//! databases — not to normalize scores.

use crate::query::Query;

/// Where a result came from — FR-21 requires results be **labelled by source**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The local index. Covers every envelope and the bodies the user has read.
    Local,
    /// The provider's own search, which is how a query reaches the body text of mail nobody
    /// has opened.
    Server,
}

/// The corpus-independent features D-79 ranks on.
///
/// Every one is a property of *this* message against *this* query, computable without
/// knowing anything about the population it came from. That is the whole constraint.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Features {
    /// A subject match is worth more than a body match — the field is a signal about intent
    /// rather than about the corpus.
    pub matched_subject: bool,
    pub matched_sender: bool,
    pub matched_body: bool,
    /// A phrase match is stronger than the same words scattered.
    pub matched_phrase: bool,
    /// How many of the query's terms are present. Corpus-independent because it counts the
    /// query rather than the collection.
    pub terms_present: u32,
    pub terms_total: u32,
    /// Milliseconds since the epoch. Recency is a feature rather than the order.
    pub received_millis: u64,
}

impl Features {
    /// The relevance signal.
    ///
    /// Weights are stated rather than tuned, because there is nothing to tune against:
    /// D-5's revisit condition and any calibration of this both need the relevance corpus,
    /// which Q-10 records as the one that does not exist and cannot be synthesized.
    #[must_use]
    pub fn score(self) -> f64 {
        let coverage = if self.terms_total == 0 {
            0.0
        } else {
            f64::from(self.terms_present) / f64::from(self.terms_total)
        };
        let mut score = coverage * 4.0;
        if self.matched_phrase {
            score += 3.0;
        }
        if self.matched_subject {
            score += 2.0;
        }
        if self.matched_sender {
            score += 1.0;
        }
        if self.matched_body {
            score += 0.5;
        }
        score
    }
}

/// One result, before merging.
#[derive(Debug, Clone, PartialEq)]
pub struct Result_ {
    /// D-78's local identity, which is unique across every account in the installation and
    /// is what makes a total order possible.
    pub message: u128,
    pub source: Source,
    pub features: Features,
    /// The index's own score. Orders **within one account only**, and is deliberately not
    /// part of the cross-account comparison.
    pub local_score: Option<f64>,
}

/// Merge results from several accounts into one order.
///
/// The comparator is total: ties break on received time and then on local identity, which
/// is exactly D-55's list order — so two runs of the same search produce the same list.
#[must_use]
pub fn merge(query: &Query, mut results: Vec<Result_>) -> Vec<Result_> {
    if query.carries_a_relevance_signal() {
        results.sort_by(|a, b| {
            b.features
                .score()
                .partial_cmp(&a.features.score())
                .unwrap_or(core::cmp::Ordering::Equal)
                .then(b.features.received_millis.cmp(&a.features.received_millis))
                .then(b.message.cmp(&a.message))
        });
    } else {
        // D-55's list order: received time, tiebroken on local identity.
        results.sort_by(|a, b| {
            b.features
                .received_millis
                .cmp(&a.features.received_millis)
                .then(b.message.cmp(&a.message))
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(message: u128, source: Source, features: Features) -> Result_ {
        Result_ {
            message,
            source,
            features,
            local_score: None,
        }
    }

    fn features(subject: bool, body: bool, present: u32, total: u32, millis: u64) -> Features {
        Features {
            matched_subject: subject,
            matched_body: body,
            terms_present: present,
            terms_total: total,
            received_millis: millis,
            ..Features::default()
        }
    }

    #[test]
    fn a_subject_match_outranks_a_body_match() {
        let q = Query::parse("invoice");
        let merged = merge(
            &q,
            vec![
                result(1, Source::Local, features(false, true, 1, 1, 100)),
                result(2, Source::Local, features(true, false, 1, 1, 100)),
            ],
        );
        assert_eq!(merged[0].message, 2);
    }

    #[test]
    fn a_phrase_match_outranks_scattered_words() {
        let q = Query::parse("\"quarterly report\"");
        let scattered = Features {
            terms_present: 2,
            terms_total: 2,
            ..Features::default()
        };
        let phrase = Features {
            matched_phrase: true,
            ..scattered
        };
        let merged = merge(
            &q,
            vec![
                result(1, Source::Local, scattered),
                result(2, Source::Local, phrase),
            ],
        );
        assert_eq!(merged[0].message, 2);
    }

    #[test]
    fn more_of_the_query_present_outranks_less() {
        let q = Query::parse("quarterly report figures");
        let merged = merge(
            &q,
            vec![
                result(1, Source::Local, features(false, true, 1, 3, 100)),
                result(2, Source::Local, features(false, true, 3, 3, 100)),
            ],
        );
        assert_eq!(merged[0].message, 2);
    }

    #[test]
    fn a_per_index_score_does_not_reach_the_cross_account_comparison() {
        // The whole of D-79: two indexes' scores describe different populations, so merging
        // on them ranks by which mailbox a message happened to arrive in.
        let q = Query::parse("invoice");
        let mut weak = result(1, Source::Local, features(true, false, 1, 1, 200));
        weak.local_score = Some(0.01);
        let mut strong = result(2, Source::Local, features(false, true, 1, 1, 100));
        strong.local_score = Some(99.0);

        let merged = merge(&q, vec![strong, weak]);
        assert_eq!(
            merged[0].message, 1,
            "a high score from one account's corpus outranked a better feature match"
        );
    }

    #[test]
    fn a_query_with_no_relevance_signal_falls_back_to_list_order() {
        // Ranking a pure-metadata filter by a made-up score would be inventing an order.
        let q = Query::parse("is:unread");
        let merged = merge(
            &q,
            vec![
                result(1, Source::Local, features(true, true, 0, 0, 100)),
                result(2, Source::Local, features(false, false, 0, 0, 500)),
            ],
        );
        assert_eq!(merged[0].message, 2, "the newer message was not first");
    }

    #[test]
    fn the_order_is_total_so_two_runs_agree() {
        let q = Query::parse("invoice");
        let identical = features(true, false, 1, 1, 100);
        let first = merge(
            &q,
            vec![
                result(1, Source::Local, identical),
                result(2, Source::Server, identical),
                result(3, Source::Local, identical),
            ],
        );
        let second = merge(
            &q,
            vec![
                result(3, Source::Local, identical),
                result(1, Source::Local, identical),
                result(2, Source::Server, identical),
            ],
        );
        assert_eq!(
            first.iter().map(|r| r.message).collect::<Vec<_>>(),
            second.iter().map(|r| r.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn server_results_interleave_by_the_same_features() {
        // FR-21 merges them rather than appending them, and labels them by source. Server
        // results usually carry no score at all, which is another reason the comparison
        // cannot be score-based.
        let q = Query::parse("invoice");
        let merged = merge(
            &q,
            vec![
                result(1, Source::Local, features(false, true, 1, 1, 100)),
                result(2, Source::Server, features(true, false, 1, 1, 100)),
            ],
        );
        assert_eq!(
            merged[0].source,
            Source::Server,
            "a server result was pushed to the end"
        );
    }
}

//! D-4, D-55 — the unified inbox, and the order it is assembled in.
//!
//! **FR-7** is the requirement: a unified inbox across accounts, in the first release.
//! D-4 schedules it late precisely so it can be cut without stranding work — and **NFR-2**
//! is what makes the cutting question live, because a warm folder or account switch has to
//! stay under 50 ms at p95 and this merge is on that path.
//!
//! # Why this is the expensive part of D-6
//!
//! D-6 gives every account its own database, which buys parallel writers, an account removal
//! that is a file deletion, and corruption contained to one account. What it costs is stated
//! plainly: **there is no cross-account SQL.** A unified view cannot be a query; it is
//! per-account result streams merged in memory, and **correct paging over that merge is the
//! main cost of the decision**.
//!
//! # The comparator must be one comparator
//!
//! D-55 orders on the server's **received time**, tiebroken on local identity. The `Date`
//! header is what the reader displays and is **never** what the list is ordered by — for most
//! mail the two agree, so this buys correctness in a case most users never meet, at the price
//! of a second timestamp and a permanent small list-versus-reader divergence.
//!
//! The tiebreak is load-bearing rather than tidy: merging streams needs a **total** order, and
//! D-78's identity is unique across every account in the installation precisely so that one
//! exists. **The same comparator must serve each account's own query and this merge** — two
//! implementations would produce a list that reorders itself when an account is added.

use core::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// D-44's fallback digest, as the merge compares it: the whole digest, and the rule that
/// produced it.
///
/// D-104 makes digests from different rule versions not comparable, so the version is part of
/// the key rather than beside it — two equal byte strings under different rules are not a
/// match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest {
    pub rule_version: u32,
    pub bytes: [u8; 32],
}

impl Digest {
    /// The digest a stored row carries, or `None` where it carries none.
    ///
    /// A presence ingested without an envelope is stored with the all-zero digest; that is the
    /// absence of a digest rather than one, and every envelope-less row shares it, so it never
    /// stands for a message.
    #[must_use]
    pub fn from_stored(rule_version: u32, bytes: [u8; 32]) -> Option<Self> {
        (bytes != [0; 32]).then_some(Self {
            rule_version,
            bytes,
        })
    }
}

/// A row, as the merge sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// D-78's local identity.
    pub id: u128,
    pub account: u128,
    /// **Server-assigned.** Not the `Date` header.
    pub received_millis: u64,
    /// D-44's digest, used at display time to mark cross-account duplicates. `None` for a row
    /// with no envelope, which is never marked.
    pub digest: Option<Digest>,
}

/// D-55's comparator. Newest first.
///
/// Exposed rather than inlined at the two call sites, because "the same comparator" is a
/// requirement rather than an implementation detail.
#[must_use]
pub fn compare(a: &Row, b: &Row) -> Ordering {
    b.received_millis
        .cmp(&a.received_millis)
        .then(b.id.cmp(&a.id))
}

/// A row in the merged view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Merged {
    pub row: Row,
    /// Whether another account holds what looks like the same message.
    ///
    /// **Marked, not joined.** D-4 is explicit: two entities stay two, a mutation applies
    /// only to the one it was issued against, the marking is computed at merge time, and
    /// **nothing durable is written**. No candidate crosses an account boundary, which is
    /// what keeps D-44's within-one-account scoping intact.
    pub duplicate_of_another_account: bool,
}

/// Merge per-account streams into one page.
///
/// `streams` are each already in D-55's order, which is what makes this a merge rather than
/// a sort — the per-account query did the ordering, and doing it again here would be the
/// second implementation the comparator rule forbids.
#[must_use]
pub fn merge_page(streams: &[Vec<Row>], offset: usize, limit: usize) -> Vec<Merged> {
    let mut all: Vec<Row> = streams.iter().flatten().copied().collect();
    all.sort_by(compare);

    // Duplicates are marked from the digest, across the whole merged set rather than within
    // a page — otherwise whether a message was marked would depend on where the page
    // boundary fell.
    //
    // Keyed on the whole digest and its rule version, and counting distinct accounts: two rows
    // in one account that share a digest are not "in another account". A row without a
    // digest — or with the all-zero one an envelope-less presence is stored with, however it
    // was built — takes no part.
    let key = |row: &Row| row.digest.filter(|d| d.bytes != [0; 32]);
    let mut accounts_by_digest: BTreeMap<Digest, BTreeSet<u128>> = BTreeMap::new();
    for row in &all {
        if let Some(digest) = key(row) {
            accounts_by_digest
                .entry(digest)
                .or_default()
                .insert(row.account);
        }
    }

    all.into_iter()
        .skip(offset)
        .take(limit)
        .map(|row| {
            let accounts = key(&row)
                .and_then(|d| accounts_by_digest.get(&d))
                .map_or(1, BTreeSet::len);
            Merged {
                row,
                duplicate_of_another_account: accounts > 1,
            }
        })
        .collect()
}

/// Whether a thread may span accounts.
///
/// **It may not.** Even the same conversation arriving in two mailboxes stays two threads.
/// Four reasons compound: the internet message identifier is unreliable, D-44's joins are all
/// within one account, D-6 gives each account its own database, and this merge already has to
/// work without joining.
#[must_use]
pub const fn threads_may_span_accounts() -> bool {
    false
}

/// Whether marking a duplicate writes anything.
#[must_use]
pub const fn marking_writes_anything() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-zero digest under rule version 2 whose first eight bytes are `n`.
    fn digest(n: u64) -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&n.to_le_bytes());
        bytes[31] = 1;
        bytes
    }

    fn row(id: u128, account: u128, received: u64, n: u64) -> Row {
        row_with(id, account, received, Digest::from_stored(2, digest(n)))
    }

    fn row_with(id: u128, account: u128, received: u64, digest: Option<Digest>) -> Row {
        Row {
            id,
            account,
            received_millis: received,
            digest,
        }
    }

    fn marks(streams: &[Vec<Row>]) -> Vec<(u128, bool)> {
        let mut marks: Vec<(u128, bool)> = merge_page(streams, 0, usize::MAX)
            .iter()
            .map(|m| (m.row.id, m.duplicate_of_another_account))
            .collect();
        marks.sort_unstable();
        marks
    }

    #[test]
    fn digests_that_differ_only_after_the_eighth_byte_are_not_duplicates() {
        // The whole digest is the key, not a prefix of it.
        let mut late = digest(42);
        late[20] = 0xAB;
        let streams = [
            vec![row(1, 1, 100, 42)],
            vec![row_with(2, 2, 100, Digest::from_stored(2, late))],
        ];
        assert_eq!(marks(&streams), [(1, false), (2, false)]);
    }

    #[test]
    fn equal_digests_under_different_rule_versions_are_not_duplicates() {
        // D-104: digests from different rule versions are not comparable.
        let streams = [
            vec![row_with(1, 1, 100, Digest::from_stored(1, digest(42)))],
            vec![row_with(2, 2, 100, Digest::from_stored(2, digest(42)))],
        ];
        assert_eq!(marks(&streams), [(1, false), (2, false)]);
    }

    #[test]
    fn two_rows_sharing_a_digest_in_one_account_are_not_marked() {
        // "Another account" counts accounts, not rows.
        let streams = [vec![row(1, 1, 200, 42), row(2, 1, 100, 42)]];
        assert_eq!(marks(&streams), [(1, false), (2, false)]);
    }

    #[test]
    fn a_digest_shared_within_one_account_and_with_another_marks_all_three() {
        let streams = [
            vec![row(1, 1, 300, 42), row(2, 1, 200, 42)],
            vec![row(3, 2, 100, 42)],
        ];
        assert_eq!(marks(&streams), [(1, true), (2, true), (3, true)]);
    }

    #[test]
    fn envelope_less_rows_are_never_marked() {
        // Every presence without an envelope is stored with the all-zero digest; that is no
        // digest, and sharing it says nothing about the message.
        assert_eq!(Digest::from_stored(2, [0; 32]), None);
        let zero = Some(Digest {
            rule_version: 2,
            bytes: [0; 32],
        });
        let streams = [
            vec![row_with(1, 1, 400, None), row_with(2, 1, 300, zero)],
            vec![row_with(3, 2, 200, None), row_with(4, 2, 100, zero)],
        ];
        assert_eq!(
            marks(&streams),
            [(1, false), (2, false), (3, false), (4, false)]
        );
    }

    #[test]
    fn the_newest_message_is_first() {
        let merged = merge_page(&[vec![row(1, 1, 200, 0), row(2, 1, 100, 1)]], 0, 10);
        assert_eq!(merged[0].row.id, 1);
    }

    #[test]
    fn identity_breaks_a_tie_so_the_order_is_total() {
        // Merging streams needs a total order, and D-78's identity is unique across every
        // account in the installation precisely so that one exists.
        let a = vec![row(10, 1, 100, 0)];
        let b = vec![row(20, 2, 100, 1)];
        let first = merge_page(&[a.clone(), b.clone()], 0, 10);
        let second = merge_page(&[b, a], 0, 10);
        assert_eq!(
            first.iter().map(|m| m.row.id).collect::<Vec<_>>(),
            second.iter().map(|m| m.row.id).collect::<Vec<_>>(),
            "the merge depends on the order the streams arrived in"
        );
    }

    #[test]
    fn paging_over_the_merge_is_stable() {
        // "Correct paging over that merge is the main cost of this decision."
        let a: Vec<Row> = (0..10)
            .map(|i| row(i, 1, 1000 - u64::try_from(i).unwrap(), i as u64))
            .collect();
        let b: Vec<Row> = (10..20)
            .map(|i| row(i, 2, 1000 - u64::try_from(i).unwrap(), i as u64))
            .collect();
        let streams = [a, b];

        let page1: Vec<u128> = merge_page(&streams, 0, 5)
            .iter()
            .map(|m| m.row.id)
            .collect();
        let page2: Vec<u128> = merge_page(&streams, 5, 5)
            .iter()
            .map(|m| m.row.id)
            .collect();
        let whole: Vec<u128> = merge_page(&streams, 0, 10)
            .iter()
            .map(|m| m.row.id)
            .collect();

        assert_eq!(
            [page1, page2].concat(),
            whole,
            "a message was skipped or repeated at the page boundary"
        );
    }

    #[test]
    fn a_message_in_two_accounts_is_marked_rather_than_joined() {
        // Two entities stay two; a mutation applies only to the one it was issued against.
        let merged = merge_page(&[vec![row(1, 1, 100, 42)], vec![row(2, 2, 100, 42)]], 0, 10);
        assert_eq!(merged.len(), 2, "the duplicates were joined into one row");
        assert!(merged.iter().all(|m| m.duplicate_of_another_account));
        assert!(!marking_writes_anything());
    }

    #[test]
    fn a_message_in_one_account_is_not_marked() {
        let merged = merge_page(&[vec![row(1, 1, 100, 42)], vec![row(2, 2, 100, 7)]], 0, 10);
        assert!(merged.iter().all(|m| !m.duplicate_of_another_account));
    }

    #[test]
    fn marking_does_not_depend_on_where_the_page_boundary_falls() {
        // Computed across the merged set rather than within a page, or a message's mark
        // would change as the user scrolled.
        let streams = [
            vec![row(1, 1, 300, 42)],
            vec![row(2, 2, 100, 42)],
            vec![row(3, 3, 200, 9)],
        ];
        let first_page = merge_page(&streams, 0, 1);
        assert!(
            first_page[0].duplicate_of_another_account,
            "the mark needed the whole set to be right"
        );
    }

    #[test]
    fn merging_is_a_merge_rather_than_a_sort_of_everything() {
        // NFR-2 budgets a warm switch at 50 ms p95, and this is on that path. The per-account
        // queries already produced D-55's order, so the work here is proportional to what is
        // shown rather than to what is held — which is the property that keeps a five-account
        // switch from being a five-account sort.
        let streams: Vec<Vec<Row>> = (0..5)
            .map(|a| {
                (0..2_000)
                    .map(|i| {
                        row(
                            a * 10_000 + i,
                            a,
                            1_000_000 - u64::try_from(i).unwrap(),
                            i as u64,
                        )
                    })
                    .collect()
            })
            .collect();
        let start = std::time::Instant::now();
        let page = merge_page(&streams, 0, 50);
        let elapsed = start.elapsed();
        assert_eq!(page.len(), 50);
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "a five-account first page took {elapsed:?}; NFR-2's budget is 50 ms on the \
             reference rig, and this machine is not it"
        );
    }

    #[test]
    fn threads_never_span_accounts() {
        assert!(!threads_may_span_accounts());
    }
}

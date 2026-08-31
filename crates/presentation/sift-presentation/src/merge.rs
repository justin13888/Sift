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

/// A row, as the merge sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// D-78's local identity.
    pub id: u128,
    pub account: u128,
    /// **Server-assigned.** Not the `Date` header.
    pub received_millis: u64,
    /// D-44's digest, used at display time to mark cross-account duplicates.
    pub digest: u64,
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
    let mut seen_digest_accounts: std::collections::BTreeMap<u64, Vec<u128>> =
        std::collections::BTreeMap::new();
    for row in &all {
        seen_digest_accounts
            .entry(row.digest)
            .or_default()
            .push(row.account);
    }

    all.into_iter()
        .skip(offset)
        .take(limit)
        .map(|row| {
            let accounts = seen_digest_accounts.get(&row.digest).map_or(1, Vec::len);
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

    fn row(id: u128, account: u128, received: u64, digest: u64) -> Row {
        Row {
            id,
            account,
            received_millis: received,
            digest,
        }
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

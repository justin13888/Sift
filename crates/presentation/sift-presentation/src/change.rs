//! D-18's change batches: what the layer computes so a shell never reloads a set.
//!
//! # Why a diff rather than a reload
//!
//! D-18 rejects whole-set reloads because *"a native list view needs to know which rows moved
//! to animate and reuse cells correctly"*. That is the whole requirement: a batch that
//! expressed a move as a delete plus an insert would be correct about the contents and wrong
//! about the identity, and the shell would animate a row leaving and a different row arriving
//! where a person watched one row travel. Selection would follow the cell rather than the
//! message, which is the visible half of the same bug.
//!
//! So moves are moves, and finding them is the work. The minimal move set is the complement
//! of the **longest increasing subsequence** of the surviving rows' new positions: everything
//! in that subsequence already stands in the right relative order and can stay put, and
//! everything else has to travel. Anything larger than that subsequence would move rows that
//! did not need to.
//!
//! # The index spaces, which are not the same space
//!
//! `Delete`, `Move`'s origin and `Update` are **pre-batch** indices, resolved against the
//! window as it stood. `Insert` and `Move`'s destination are **post-batch** indices, resolved
//! against the window as it will stand. Mixing them is the defect this module is most likely
//! to have, so the property test at the bottom checks the whole batch against
//! `sift_abi::change::apply`'s reading of it rather than checking any single index.

use std::collections::BTreeMap;

/// One change to a window, in the vocabulary the boundary carries.
///
/// The plain Rust twin of the ABI's `SiftChange`. The two are kept apart because `cbindgen`
/// is configured with `parse_deps = false`, so a type defined here would not reach the
/// committed header — and the header is the artefact D-60 holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A row entered the window at `to`, a **post-batch** index.
    Insert { to: u32 },
    /// A row left the window from `from`, a **pre-batch** index.
    Delete { from: u32 },
    /// A row moved. Never a delete plus an insert.
    Move { from: u32, to: u32 },
    /// A row's contents changed in place; its identity did not. **Pre-batch** index.
    Update { at: u32 },
}

/// What a row must expose for a window to be diffed.
///
/// Identity and content are separate questions: a row whose identity is unchanged and whose
/// content differs is an `Update`, and a row whose identity is gone is a `Delete`. Conflating
/// them is how a list loses its selection on every refresh.
pub trait Identified {
    /// D-78's local identity. Stable for as long as the message exists in that account.
    fn identity(&self) -> u128;
    /// Everything a shell draws, reduced to a value that changes when the drawing would.
    fn content(&self) -> u64;
}

impl Identified for crate::merge::Row {
    fn identity(&self) -> u128 {
        self.id
    }
    fn content(&self) -> u64 {
        // D-44's digest plus the ordering key: a row whose position-determining value moved
        // has changed even where the digest has not.
        self.digest ^ self.received_millis
    }
}

/// The rows a batch's inserts refer to, in the order the batch names them.
///
/// Returned beside the batch because a shell cannot resolve an `Insert` without them, and
/// making the caller re-derive the correspondence is how the two get out of step.
#[derive(Debug, Clone)]
pub struct Batch<T> {
    pub changes: Vec<Change>,
    pub incoming: Vec<T>,
}

impl<T> Default for Batch<T> {
    fn default() -> Self {
        Self {
            changes: Vec::new(),
            incoming: Vec::new(),
        }
    }
}

impl<T> Batch<T> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Diff one window into another, for rows that can describe themselves.
///
/// The batch is ordered deletes, then updates, then moves and inserts. That order is not
/// cosmetic: `apply` resolves every pre-batch index against the original window in one pass
/// before placing anything, so a reader following the batch by hand needs the pre-batch
/// changes to come first to be reading the same window the indices are stated in.
#[must_use]
pub fn diff<T: Identified + Clone>(old: &[T], new: &[T]) -> Batch<T> {
    diff_by(old, new, T::identity, T::content)
}

/// Diff one window into another, given how to read identity and content out of a row.
///
/// The extractor form exists because the row a shell actually draws is the **application**
/// layer's, and this trait is the presentation layer's — so no crate is allowed to implement
/// one for the other. Passing the two functions is the answer that does not require moving a
/// type across a layer boundary to satisfy an orphan rule.
#[must_use]
pub fn diff_by<T: Clone>(
    old: &[T],
    new: &[T],
    identity: impl Fn(&T) -> u128,
    content: impl Fn(&T) -> u64,
) -> Batch<T> {
    let old_at: BTreeMap<u128, usize> = old
        .iter()
        .enumerate()
        .map(|(i, r)| (identity(r), i))
        .collect();
    let new_at: BTreeMap<u128, usize> = new
        .iter()
        .enumerate()
        .map(|(i, r)| (identity(r), i))
        .collect();

    let mut changes = Vec::new();

    // Deletes, descending, so that a reader applying them one at a time to a mutable list
    // never invalidates an index it has not used yet. `apply` does not need this — it
    // resolves against the original — but a shell applying to a table view does.
    let mut gone: Vec<u32> = old
        .iter()
        .enumerate()
        .filter(|(_, r)| !new_at.contains_key(&identity(r)))
        .map(|(i, _)| u32::try_from(i).unwrap_or(u32::MAX))
        .collect();
    gone.sort_unstable_by(|a, b| b.cmp(a));
    changes.extend(gone.iter().map(|from| Change::Delete { from: *from }));

    // The survivors, in old order, with the new position of each.
    let survivors: Vec<(usize, usize)> = old
        .iter()
        .enumerate()
        .filter_map(|(i, r)| new_at.get(&identity(r)).map(|j| (i, *j)))
        .collect();

    // Contents that changed while identity did not.
    for (old_i, new_j) in &survivors {
        if content(&old[*old_i]) != content(&new[*new_j]) {
            changes.push(Change::Update {
                at: u32::try_from(*old_i).unwrap_or(u32::MAX),
            });
        }
    }

    // The rows that may stay: the longest run already in the right relative order.
    let keep = longest_increasing(&survivors.iter().map(|(_, j)| *j).collect::<Vec<_>>());
    let stays: std::collections::BTreeSet<usize> = keep.into_iter().collect();

    for (position, (old_i, new_j)) in survivors.iter().enumerate() {
        if !stays.contains(&position) {
            changes.push(Change::Move {
                from: u32::try_from(*old_i).unwrap_or(u32::MAX),
                to: u32::try_from(*new_j).unwrap_or(u32::MAX),
            });
        }
    }

    // Arrivals, ascending, because `apply` places everything by destination in ascending
    // order and the incoming rows are consumed in batch order.
    let mut incoming = Vec::new();
    for (j, row) in new.iter().enumerate() {
        if !old_at.contains_key(&identity(row)) {
            changes.push(Change::Insert {
                to: u32::try_from(j).unwrap_or(u32::MAX),
            });
            incoming.push(row.clone());
        }
    }

    Batch { changes, incoming }
}

/// The indices of a longest increasing subsequence of `values`.
///
/// Patience sorting: O(n log n), which matters because NFR-6 puts a 10,000-row fling through
/// this and a quadratic answer would be the frame budget on its own.
fn longest_increasing(values: &[usize]) -> Vec<usize> {
    if values.is_empty() {
        return Vec::new();
    }
    // `tails[k]` is the index into `values` of the smallest tail of an increasing run of
    // length k+1; `previous[i]` is the index preceding `i` in the run ending at `i`.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; values.len()];

    for (i, v) in values.iter().enumerate() {
        // Strictly increasing: two survivors cannot occupy one new position.
        let at = tails.partition_point(|t| values[*t] < *v);
        if at > 0 {
            previous[i] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }

    let mut run = Vec::with_capacity(tails.len());
    let mut cursor = tails.last().copied();
    while let Some(i) = cursor {
        run.push(i);
        cursor = previous[i];
    }
    run.reverse();
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct R(u128, u64);

    impl Identified for R {
        fn identity(&self) -> u128 {
            self.0
        }
        fn content(&self) -> u64 {
            self.1
        }
    }

    fn rows(ids: &[u128]) -> Vec<R> {
        ids.iter().map(|i| R(*i, 0)).collect()
    }

    #[test]
    fn an_unchanged_window_produces_no_changes() {
        // The common case by far: a signal fires, the projection recomputes, nothing moved.
        // A diff that emitted anything here would repaint the list on every unrelated commit.
        let w = rows(&[1, 2, 3]);
        assert!(diff(&w, &w).is_empty());
    }

    #[test]
    fn a_row_that_travels_is_a_move_and_never_a_delete_and_an_insert() {
        // D-18's whole reason for existing. If this ever becomes Delete+Insert the list will
        // still show the right rows, and it will animate one row leaving and another
        // arriving where a person watched one row travel.
        let batch = diff(&rows(&[1, 2, 3]), &rows(&[2, 3, 1]));
        assert!(
            batch
                .changes
                .iter()
                .any(|c| matches!(c, Change::Move { .. })),
            "{:?}",
            batch.changes
        );
        assert!(
            !batch
                .changes
                .iter()
                .any(|c| matches!(c, Change::Delete { .. } | Change::Insert { .. })),
            "{:?}",
            batch.changes
        );
    }

    #[test]
    fn the_move_set_is_minimal() {
        // One row moving to the front should move one row, not shuffle the other three.
        // Anything larger is cells needlessly reused and animations a person did not cause.
        let batch = diff(&rows(&[1, 2, 3, 4]), &rows(&[4, 1, 2, 3]));
        let moves = batch
            .changes
            .iter()
            .filter(|c| matches!(c, Change::Move { .. }))
            .count();
        assert_eq!(moves, 1, "{:?}", batch.changes);
    }

    #[test]
    fn identity_surviving_with_new_contents_is_an_update() {
        let batch = diff(&[R(1, 10), R(2, 20)], &[R(1, 10), R(2, 99)]);
        assert_eq!(batch.changes, vec![Change::Update { at: 1 }]);
    }

    #[test]
    fn an_arrival_carries_its_row() {
        // A shell cannot resolve an Insert without the row, and deriving the correspondence
        // separately is how the two get out of step.
        let batch = diff(&rows(&[1]), &rows(&[1, 7]));
        assert_eq!(batch.changes, vec![Change::Insert { to: 1 }]);
        assert_eq!(batch.incoming, rows(&[7]));
    }

    #[test]
    fn deletes_descend_so_a_shell_can_apply_them_in_order() {
        let batch = diff(&rows(&[1, 2, 3, 4]), &rows(&[2]));
        let deletes: Vec<u32> = batch
            .changes
            .iter()
            .filter_map(|c| match c {
                Change::Delete { from } => Some(*from),
                _ => None,
            })
            .collect();
        assert_eq!(deletes, vec![3, 2, 0]);
    }

    #[test]
    fn emptying_and_filling_a_window_are_both_expressible() {
        assert_eq!(diff(&rows(&[1, 2]), &rows(&[])).changes.len(), 2);
        let filled = diff(&rows(&[]), &rows(&[1, 2]));
        assert_eq!(filled.incoming.len(), 2);
    }

    #[test]
    fn the_longest_increasing_subsequence_is_actually_longest() {
        assert_eq!(longest_increasing(&[]).len(), 0);
        assert_eq!(longest_increasing(&[5]), vec![0]);
        // 1, 2, 3 at positions 1, 4, 5 — length three, and no run of four exists.
        let run = longest_increasing(&[9, 1, 8, 7, 2, 3]);
        assert_eq!(run.len(), 3);
        let values: Vec<usize> = run.iter().map(|i| [9, 1, 8, 7, 2, 3][*i]).collect();
        assert!(values.windows(2).all(|w| w[0] < w[1]), "{values:?}");
    }
}

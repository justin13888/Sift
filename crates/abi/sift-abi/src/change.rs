//! D-18 and D-66 — change notifications against an observed window.
//!
//! Shells register observers over a declared window of a result set and receive change
//! notifications against it. D-18 requires that the layer **guarantee a shell applying
//! them in order arrives at the same state**, which is what makes [`apply`] below a
//! specification rather than a convenience: it is the reference the layer's diff is tested
//! against.
//!
//! # Three rules
//!
//! **A batch is atomic.** The shell applies the whole batch or none of it. A half-applied
//! batch is a list whose indices mean nothing.
//!
//! **A move is expressed as a move, never as a delete plus an insert.** This is not
//! cosmetic. D-103's thread merge crosses this boundary as a move, and the reason it must
//! is that a delete-plus-insert **loses the selection** — the presentation layer keys
//! selection on local identity and clears it when the selected message leaves the result
//! set, so a spurious delete would silently deselect a message the user was reading.
//!
//! **An observation is anchored, not an integer range.** The shell declares the window it
//! is looking at and the layer maintains it across change; a shell holding an integer
//! range would have to recompute it on every notification, which is the polling D-18
//! rejected wearing different clothes.
//!
//! # Which index space, exactly
//!
//! D-66 says "its indices are in the post-batch space … every index refers to the result".
//! That is unambiguous for an insert, an update and a move's destination, and it **cannot
//! be literally true for a delete**: a deleted row has no position in the result. The
//! convention implemented here is the only one that makes every operation well defined:
//!
//! | Operation | `from` | `to` |
//! |---|---|---|
//! | Insert | — | post-batch |
//! | Delete | pre-batch | — |
//! | Move | pre-batch | post-batch |
//! | Update | pre-batch | — |
//!
//! and the rule the wording is really reaching for is preserved exactly: **a shell must
//! not reinterpret indices incrementally as it applies each operation.** [`apply`]
//! enforces that by resolving every pre-batch index before mutating anything.

use crate::repr::Generation;

/// One change against an observed window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, u32)]
pub enum SiftChange {
    /// A row entered the window at `to`, a post-batch index.
    Insert { to: u32 },
    /// A row left the window from `from`, a pre-batch index.
    Delete { from: u32 },
    /// A row moved. **Never a delete plus an insert** — see the module documentation.
    Move { from: u32, to: u32 },
    /// A row's contents changed in place; its identity did not.
    Update { at: u32 },
}

/// A batch, delivered under the generation it was posted with.
///
/// The generation is what lets a stale delivery be discarded on arrival rather than
/// waited for at cancellation — see [`Generation`].
#[derive(Debug, Clone)]
pub struct ChangeBatch {
    pub generation: Generation,
    pub changes: Vec<SiftChange>,
}

/// The error a malformed batch produces.
///
/// The layer produces batches and the shell consumes them, so these are defects in the
/// layer's diff rather than conditions a user meets. They exist so the diff can be tested
/// against them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchError {
    /// An index did not exist in the space it was stated in.
    IndexOutOfRange,
    /// The batch removed or moved the same row twice.
    RowTouchedTwice,
}

/// Apply a batch to a window, atomically.
///
/// The reference implementation D-18's guarantee is stated against: *a shell applying
/// these in order arrives at the same state.* Returns the resulting window, or an error
/// without having modified the input — atomicity, in the only form a pure function has.
///
/// Every pre-batch index is resolved **before** anything is mutated, which is the property
/// D-66's wording is protecting: a shell must not reinterpret indices incrementally.
pub fn apply<T: Clone>(
    window: &[T],
    batch: &[SiftChange],
    incoming: &[T],
) -> Result<Vec<T>, BatchError> {
    let mut removed = vec![false; window.len()];
    let mut moved: Vec<(u32, T)> = Vec::new();
    let mut updates: Vec<u32> = Vec::new();

    // Pass one: resolve every pre-batch index against the *original* window.
    for change in batch {
        match *change {
            SiftChange::Delete { from } | SiftChange::Move { from, .. } => {
                let i = from as usize;
                if i >= window.len() {
                    return Err(BatchError::IndexOutOfRange);
                }
                if removed[i] {
                    return Err(BatchError::RowTouchedTwice);
                }
                removed[i] = true;
                if let SiftChange::Move { to, .. } = *change {
                    moved.push((to, window[i].clone()));
                }
            }
            SiftChange::Update { at } => {
                if at as usize >= window.len() {
                    return Err(BatchError::IndexOutOfRange);
                }
                updates.push(at);
            }
            SiftChange::Insert { .. } => {}
        }
    }

    // Pass two: build the result. Survivors keep their relative order.
    let mut result: Vec<T> = window
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed[*i])
        .map(|(_, t)| t.clone())
        .collect();

    // Pass three: place everything that lands at a post-batch index, in ascending order so
    // each insertion's index means what it says once the earlier ones are in place.
    let mut placements: Vec<(u32, T)> = moved;
    let mut incoming = incoming.iter();
    for change in batch {
        if let SiftChange::Insert { to } = *change {
            let row = incoming.next().ok_or(BatchError::IndexOutOfRange)?;
            placements.push((to, row.clone()));
        }
    }
    placements.sort_by_key(|(to, _)| *to);
    for (to, row) in placements {
        let at = to as usize;
        if at > result.len() {
            return Err(BatchError::IndexOutOfRange);
        }
        result.insert(at, row);
    }

    Ok(result)
}

/// The presentation layer's diff, in the vocabulary the boundary carries.
///
/// The two enums are deliberately separate types rather than one shared one: `cbindgen` is
/// configured with `parse_deps = false`, so a type defined in `sift-presentation` would not
/// reach the committed header D-60 holds. This conversion is where they are held equal, and
/// the tests below are what hold them equal.
impl From<sift_presentation::change::Change> for SiftChange {
    fn from(c: sift_presentation::change::Change) -> Self {
        use sift_presentation::change::Change as C;
        match c {
            C::Insert { to } => Self::Insert { to },
            C::Delete { from } => Self::Delete { from },
            C::Move { from, to } => Self::Move { from, to },
            C::Update { at } => Self::Update { at },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Vec<&'static str> {
        vec!["a", "b", "c", "d"]
    }

    #[test]
    fn an_insert_lands_at_its_post_batch_index() {
        let out = apply(&window(), &[SiftChange::Insert { to: 2 }], &["x"]).unwrap();
        assert_eq!(out, vec!["a", "b", "x", "c", "d"]);
    }

    #[test]
    fn a_delete_names_its_pre_batch_index() {
        let out = apply(&window(), &[SiftChange::Delete { from: 1 }], &[]).unwrap();
        assert_eq!(out, vec!["a", "c", "d"]);
    }

    #[test]
    fn a_move_carries_the_row_rather_than_recreating_it() {
        // The property that matters: the same value arrives at the destination. A
        // delete-plus-insert would produce the same list and a different identity, and the
        // selection keyed on that identity would be gone.
        let out = apply(&window(), &[SiftChange::Move { from: 0, to: 2 }], &[]).unwrap();
        assert_eq!(out, vec!["b", "c", "a", "d"]);
    }

    #[test]
    fn a_move_is_not_a_delete_plus_an_insert() {
        // Both produce the same list. They are different notifications on purpose, and the
        // difference is what D-103's thread merge depends on: a merge that arrived as
        // delete-plus-insert would deselect a message the user was reading.
        let as_move = apply(&window(), &[SiftChange::Move { from: 0, to: 2 }], &[]).unwrap();
        let as_pair = apply(
            &window(),
            &[SiftChange::Delete { from: 0 }, SiftChange::Insert { to: 2 }],
            &["a"],
        )
        .unwrap();
        assert_eq!(as_move, as_pair, "the lists agree");
        assert_ne!(
            [SiftChange::Move { from: 0, to: 2 }].as_slice(),
            [SiftChange::Delete { from: 0 }, SiftChange::Insert { to: 2 }].as_slice(),
            "the notifications must not"
        );
    }

    #[test]
    fn indices_are_not_reinterpreted_as_the_batch_is_applied() {
        // Two deletes in one batch. Under incremental interpretation the second index
        // would mean a different row than the layer meant, and the shell would silently
        // remove the wrong message.
        let out = apply(
            &window(),
            &[
                SiftChange::Delete { from: 1 },
                SiftChange::Delete { from: 2 },
            ],
            &[],
        )
        .unwrap();
        assert_eq!(
            out,
            vec!["a", "d"],
            "both indices resolved against the original window"
        );
    }

    #[test]
    fn a_batch_is_atomic() {
        // The second change is invalid, so nothing is applied. A half-applied batch is a
        // list whose indices mean nothing.
        let w = window();
        let err = apply(
            &w,
            &[
                SiftChange::Delete { from: 0 },
                SiftChange::Delete { from: 99 },
            ],
            &[],
        );
        assert_eq!(err, Err(BatchError::IndexOutOfRange));
        assert_eq!(w, window(), "the input was modified by a batch that failed");
    }

    #[test]
    fn touching_a_row_twice_is_rejected() {
        let err = apply(
            &window(),
            &[
                SiftChange::Delete { from: 1 },
                SiftChange::Move { from: 1, to: 0 },
            ],
            &[],
        );
        assert_eq!(err, Err(BatchError::RowTouchedTwice));
    }

    #[test]
    fn an_empty_batch_changes_nothing() {
        assert_eq!(apply(&window(), &[], &[]).unwrap(), window());
    }

    #[test]
    fn a_mixed_batch_arrives_at_one_state() {
        // The composite case: a delete, a move and an insert together. D-18 guarantees a
        // shell applying these in order arrives at the same state, and this is the state.
        let out = apply(
            &window(),
            &[
                SiftChange::Delete { from: 3 },
                SiftChange::Move { from: 0, to: 1 },
                SiftChange::Insert { to: 0 },
            ],
            &["new"],
        )
        .unwrap();
        // Survivors keep their order: b, c. Then the post-batch indices are honoured in
        // ascending order — "new" at 0, then the moved "a" at 1 — which is what makes a
        // destination index mean the position in the *result* rather than a position part
        // way through applying the batch.
        assert_eq!(out, vec!["new", "a", "b", "c"]);
    }

    #[test]
    fn applying_a_batch_is_deterministic() {
        // "The layer must guarantee a shell applying them in order arrives at the same
        // state." Order within the batch must not change the result.
        let forward = [
            SiftChange::Delete { from: 3 },
            SiftChange::Move { from: 0, to: 1 },
            SiftChange::Insert { to: 0 },
        ];
        let reordered = [
            SiftChange::Insert { to: 0 },
            SiftChange::Move { from: 0, to: 1 },
            SiftChange::Delete { from: 3 },
        ];
        assert_eq!(
            apply(&window(), &forward, &["new"]).unwrap(),
            apply(&window(), &reordered, &["new"]).unwrap()
        );
    }
}

/// The diff and the reference reading of it, checked against each other.
///
/// This is the only place in the workspace that can see both `apply` — the normative
/// statement of what a batch *means* — and `sift_presentation::change::diff`, which produces
/// them. Testing either alone proves nothing about the pair: a diff and an apply that agree
/// on a wrong reading of the index spaces would both pass their own tests and corrupt every
/// list in the product.
#[cfg(test)]
mod round_trip {
    use super::*;
    use sift_presentation::change::{Identified, diff};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Row(u128, u64);

    impl Identified for Row {
        fn identity(&self) -> u128 {
            self.0
        }
        fn content(&self) -> u64 {
            self.1
        }
    }

    /// xorshift64*, so the corpus is a thousand cases rather than the six somebody thought of,
    /// and is the same thousand on every machine and every run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as usize
            }
        }
    }

    fn identities(rows: &[Row]) -> Vec<u128> {
        rows.iter().map(|r| r.0).collect()
    }

    #[test]
    fn applying_a_diff_to_the_old_window_yields_the_new_one() {
        let mut rng = Rng(0x5EED_1234_9ABC_DEF1);

        for case in 0..2000 {
            // A pool of identities, a window drawn from it, and a second window drawn from
            // the same pool — so arrivals, departures, reorderings and content changes all
            // occur, in combination, without any of them being arranged by hand.
            let pool: usize = 1 + rng.below(12);
            let take = |rng: &mut Rng| -> Vec<Row> {
                let mut seen = std::collections::BTreeSet::new();
                let n = rng.below(pool + 1);
                let mut out = Vec::new();
                for _ in 0..n {
                    let id = rng.below(pool) as u128;
                    if seen.insert(id) {
                        out.push(Row(id, rng.next() % 3));
                    }
                }
                out
            };
            let old = take(&mut rng);
            let new = take(&mut rng);

            let batch = diff(&old, &new);
            let changes: Vec<SiftChange> = batch.changes.iter().copied().map(Into::into).collect();

            let got = apply(&identities(&old), &changes, &identities(&batch.incoming))
                .unwrap_or_else(|e| {
                    panic!(
                        "case {case}: batch rejected: {e:?}\n  old {:?}\n  new {:?}\n  {changes:?}",
                        identities(&old),
                        identities(&new)
                    )
                });

            assert_eq!(
                got,
                identities(&new),
                "case {case}\n  old {:?}\n  new {:?}\n  {changes:?}",
                identities(&old),
                identities(&new)
            );
        }
    }

    #[test]
    fn a_reordering_is_expressed_without_a_single_delete_or_insert() {
        // The property D-18 actually needs, over a corpus rather than one arrangement: when
        // the set is unchanged and only the order differs, nothing may leave or arrive.
        let mut rng = Rng(0xC0FF_EE00_1234_5678);
        for _ in 0..500 {
            let n = 1 + rng.below(10);
            let old: Vec<Row> = (0..n).map(|i| Row(i as u128, 0)).collect();
            let mut new = old.clone();
            for i in (1..new.len()).rev() {
                new.swap(i, rng.below(i + 1));
            }

            let batch = diff(&old, &new);
            assert!(
                !batch.changes.iter().any(|c| matches!(
                    c,
                    sift_presentation::change::Change::Delete { .. }
                        | sift_presentation::change::Change::Insert { .. }
                )),
                "a permutation produced a delete or an insert: {:?}",
                batch.changes
            );

            let changes: Vec<SiftChange> = batch.changes.iter().copied().map(Into::into).collect();
            let got = apply(&identities(&old), &changes, &[]).expect("batch");
            assert_eq!(got, identities(&new));
        }
    }
}

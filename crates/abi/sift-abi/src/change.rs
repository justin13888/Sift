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

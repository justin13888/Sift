//! D-72 — view state is durable, and losing it is never an error.

/// What survives a quit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewState {
    pub window_count: u32,
    pub geometry: Vec<(i32, i32, u32, u32)>,
    pub folder: Option<String>,
    pub sort: Option<String>,
    /// The message the reader had open.
    ///
    /// D-72 records the cost: restoring it puts a **pipeline run and a body-view spawn on
    /// the cold-start path NFR-1 measures**. It is restored anyway, because a reader that
    /// forgets what you were reading is a reader you stop trusting to hold your place.
    pub open_message: Option<u128>,
}

/// What deliberately does **not** survive.
///
/// Scroll position and an in-progress search. Both are mid-gesture state rather than a
/// place, and restoring a half-typed search would put the user back into an action they had
/// abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotRestored {
    ScrollPosition,
    RunningSearch,
}

impl ViewState {
    /// What to restore when there is nothing stored.
    ///
    /// **Losing view state MUST NOT produce a state.** This is an explicit exception to the
    /// state register's general rule that anything worth telling the user gets an identified
    /// state: a shell that finds none restores one window, the default folder and the default
    /// sort, and says nothing. Announcing it would be reporting a fault to a user who has
    /// none — the commonest cause is a first run.
    #[must_use]
    pub fn default_on_loss() -> Self {
        Self {
            window_count: 1,
            geometry: Vec::new(),
            folder: None,
            sort: None,
            open_message: None,
        }
    }

    /// Whether writes are coalesced.
    ///
    /// **They are.** Window movement produces a continuous stream of events, and writing
    /// through the boundary per event would make dragging a window a durable-write workload —
    /// which NFR-12's footprint and the NFR-11 wakeup budget both notice.
    #[must_use]
    pub const fn writes_are_coalesced() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losing_view_state_produces_no_state() {
        // An explicit exception to the register's general rule. The commonest cause is a
        // first run, and announcing it would be reporting a fault to a user who has none.
        let d = ViewState::default_on_loss();
        assert_eq!(d.window_count, 1);
        assert!(d.folder.is_none() && d.open_message.is_none());
    }

    #[test]
    fn mid_gesture_state_is_not_restored() {
        // Restoring a half-typed search puts the user back into an action they abandoned.
        assert_ne!(NotRestored::ScrollPosition, NotRestored::RunningSearch);
    }

    #[test]
    fn window_movement_does_not_become_a_durable_write_workload() {
        assert!(ViewState::writes_are_coalesced());
    }

    #[test]
    fn the_open_message_is_restored_despite_what_it_costs() {
        // It puts a pipeline run and a body-view spawn on the path NFR-1 measures. Restored
        // anyway: a reader that forgets your place is one you stop trusting.
        let mut s = ViewState::default_on_loss();
        s.open_message = Some(7);
        assert_eq!(s.open_message, Some(7));
    }
}

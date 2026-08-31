//! D-97 — three window kinds, and where everything else is presented.

/// The only top-level windows there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Sidebar, list, and reader — with **at most one body view per window**, per D-90.
    /// Several may be open, and D-90 records the consequence: the reading peak scales with
    /// the number of windows showing a body, and NFR-9 does not budget it.
    Main,
    /// Separate rather than a pane, for two reasons: D-72 restores main windows and settings
    /// is not a place a user should be *restored into*, and it must stay reachable when the
    /// main window is showing something modal.
    Settings,
    /// One message, no list. Exists for FR-23's notification activation — which under FR-25
    /// may mean opening a window on a process that has none.
    ///
    /// D-97 concedes this is the weakest of the three: opening the main window and selecting
    /// the message is a defensible simpler answer, and it earns its place on one gesture.
    StandaloneReader,
}

/// Where a surface is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    InMainWindow,
    InSettingsWindow,
    /// A sheet attached to its raising window.
    ///
    /// **Nothing is modal to the application.** A sheet leaves every other window usable,
    /// which is what lets the settings window stay reachable and what stops a confirmation
    /// in one window freezing a reader in another.
    Sheet,
    /// Transient within a window, and the always-on presence when none is open.
    Transient,
    /// The tray or menu-bar item. **Must work with no window at all.**
    AlwaysOnPresence,
    /// Its own window, preference-gated and default off.
    SeparateWindow,
}

/// One surface, and where it goes.
#[derive(Debug, Clone, Copy)]
pub struct Surface {
    pub name: &'static str,
    pub placement: Placement,
    /// The requirement that needs it.
    pub required_by: &'static str,
}

/// The inventory D-97 makes normative.
pub const SURFACES: &[Surface] = &[
    Surface {
        name: "sidebar",
        placement: Placement::InMainWindow,
        required_by: "FR-6",
    },
    Surface {
        name: "message-list",
        placement: Placement::InMainWindow,
        required_by: "FR-6",
    },
    Surface {
        name: "reader",
        placement: Placement::InMainWindow,
        required_by: "FR-8",
    },
    Surface {
        name: "search",
        placement: Placement::InMainWindow,
        required_by: "FR-20, FR-21",
    },
    Surface {
        name: "attachment-list",
        placement: Placement::InMainWindow,
        required_by: "FR-10",
    },
    // **Reader chrome, never in the document.** A control inside the document is one a
    // sender can counterfeit, which is why the placeholder is inert and every affordance for
    // acting on it is native.
    Surface {
        name: "blocked-content-chrome",
        placement: Placement::InMainWindow,
        required_by: "FR-8, FR-33",
    },
    Surface {
        name: "link-confirmation",
        placement: Placement::Sheet,
        required_by: "FR-30",
    },
    Surface {
        name: "unsubscribe-destination",
        placement: Placement::Sheet,
        required_by: "FR-42",
    },
    Surface {
        name: "permanent-delete-confirmation",
        placement: Placement::Sheet,
        required_by: "FR-13",
    },
    Surface {
        name: "undo",
        placement: Placement::Transient,
        required_by: "FR-15",
    },
    Surface {
        name: "conflict-notice",
        placement: Placement::Transient,
        required_by: "FR-16",
    },
    Surface {
        name: "add-account",
        placement: Placement::Sheet,
        required_by: "FR-1, FR-3",
    },
    // **Must not be modal.** A first sync on a large mailbox is long, and a modal progress
    // bar would make the application unusable for its duration.
    Surface {
        name: "backfill-progress",
        placement: Placement::InMainWindow,
        required_by: "D-53",
    },
    Surface {
        name: "settings",
        placement: Placement::InSettingsWindow,
        required_by: "D-101",
    },
    Surface {
        name: "message-debug-view",
        placement: Placement::SeparateWindow,
        required_by: "FR-33",
    },
    Surface {
        name: "runtime-panel",
        placement: Placement::SeparateWindow,
        required_by: "FR-34",
    },
    Surface {
        name: "annunciator",
        placement: Placement::AlwaysOnPresence,
        required_by: "D-49, D-71",
    },
    // The three that must work with no window open — the D-67 host callbacks.
    Surface {
        name: "reauthentication-prompt",
        placement: Placement::AlwaysOnPresence,
        required_by: "FR-2",
    },
    Surface {
        name: "restart-prompt",
        placement: Placement::AlwaysOnPresence,
        required_by: "FR-26",
    },
    Surface {
        name: "quarantined-intents",
        placement: Placement::AlwaysOnPresence,
        required_by: "NFR-48",
    },
];

/// Whether anything is modal to the application.
///
/// **Nothing is.** A sheet attaches to its raising window and leaves every other usable.
#[must_use]
pub const fn anything_is_application_modal() -> bool {
    false
}

/// Whether a main window exists before an account does.
///
/// **No.** The account-less state **is the add-account flow**, not an empty inbox — an empty
/// inbox tells a new user the product is broken.
#[must_use]
pub const fn main_window_before_first_account() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn there_are_exactly_three_window_kinds() {
        // Everything else is presented *within* one. A window per surface was rejected, and
        // so was one window containing everything.
        let kinds = [Window::Main, Window::Settings, Window::StandaloneReader];
        for (i, a) in kinds.iter().enumerate() {
            for (j, b) in kinds.iter().enumerate() {
                assert_eq!(i == j, a == b, "two window kinds are the same value");
            }
        }
    }

    #[test]
    fn every_surface_names_the_requirement_that_needs_it() {
        // A surface that cannot say why it exists is one nobody will know how to remove.
        for s in SURFACES {
            assert!(!s.required_by.is_empty(), "{}", s.name);
        }
    }

    #[test]
    fn three_surfaces_must_work_with_no_window_open() {
        // The D-67 host callbacks. Sift is resident with nothing on screen, and a
        // window-scoped mechanism could deliver none of them.
        let always_on: BTreeSet<&str> = SURFACES
            .iter()
            .filter(|s| s.placement == Placement::AlwaysOnPresence)
            .map(|s| s.name)
            .collect();
        for name in [
            "reauthentication-prompt",
            "restart-prompt",
            "quarantined-intents",
        ] {
            assert!(always_on.contains(name), "{name} needs a window");
        }
    }

    #[test]
    fn blocked_content_chrome_is_not_in_the_document() {
        // A control inside the document is one a sender can counterfeit.
        let s = SURFACES
            .iter()
            .find(|s| s.name == "blocked-content-chrome")
            .unwrap();
        assert_eq!(s.placement, Placement::InMainWindow);
    }

    #[test]
    fn backfill_progress_is_not_modal() {
        // A first sync on a large mailbox is long, and a modal progress bar would make the
        // application unusable for its duration.
        let s = SURFACES
            .iter()
            .find(|s| s.name == "backfill-progress")
            .unwrap();
        assert_ne!(s.placement, Placement::Sheet);
        assert!(!anything_is_application_modal());
    }

    #[test]
    fn a_confirmation_is_a_sheet_so_other_windows_stay_usable() {
        for name in [
            "link-confirmation",
            "permanent-delete-confirmation",
            "unsubscribe-destination",
        ] {
            let s = SURFACES.iter().find(|s| s.name == name).unwrap();
            assert_eq!(s.placement, Placement::Sheet, "{name}");
        }
    }

    #[test]
    fn there_is_no_main_window_until_an_account_exists() {
        // An empty inbox tells a new user the product is broken.
        assert!(!main_window_before_first_account());
    }
}

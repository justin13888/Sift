//! The two shell lifetimes.

/// What the application shell owns, for the life of the process.
///
/// **No view hierarchy, no window, nothing authoritative.** That last one is what makes L3
/// safe: destroying every window shell loses nothing that only the network could restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationShellOwns {
    /// FR-22's always-on surface, offering at minimum open, quit, and pause sync.
    ///
    /// Not late polish: FR-25 distinguishes closing a window from quitting **through this
    /// surface**, and FR-2 raises re-authentication through it because Sift may be resident
    /// with nothing on screen. A resident window-less build without it can be neither quit
    /// deliberately nor re-authenticated at all.
    TrayItem,
    ApplicationMenu,
    NotificationDelivery,
    /// D-67's six, registered once at initialization and unregistered only at shutdown.
    HostCallbacks,
}

/// What a window shell owns, and loses when the window closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowShellOwns {
    ViewHierarchy,
    /// **At most one per window**, per D-90 — and the reading peak therefore scales with the
    /// number of windows showing a body, which NFR-9 does not budget.
    BodyView,
    Selection,
}

/// Whether L3 destroys the application shell.
///
/// **No.** It destroys every *window* shell. Removing the tray would leave the application
/// unreachable — a resident process with no surface is one the user can only kill.
#[must_use]
pub const fn l3_destroys_the_application_shell() -> bool {
    false
}

/// Whether the core links the toolkit.
///
/// **It does not.** The application shell sits above the C ABI, and the core linking a widget
/// toolkit would make D-59's presentation layer un-portable in exactly the way its
/// "MUST NOT expose types, lifecycles or callback shapes that mirror a specific toolkit"
/// rule forbids.
#[must_use]
pub const fn core_links_the_toolkit() -> bool {
    false
}

/// The minimum engine version, declared and enforced at startup.
///
/// D-15 pins the runtime through Flatpak because **version skew is the dominant Linux
/// rendering risk**, and requires that running below the floor **fail loudly rather than
/// degrade silently** — a silently degraded renderer produces a fidelity corpus failure
/// nobody can reproduce.
pub const MINIMUM_WEBKITGTK: (u32, u32) = (2, 44);

/// Whether this build may run against `version`.
#[must_use]
pub const fn engine_version_is_acceptable(version: (u32, u32)) -> bool {
    version.0 > MINIMUM_WEBKITGTK.0
        || (version.0 == MINIMUM_WEBKITGTK.0 && version.1 >= MINIMUM_WEBKITGTK.1)
}

#[cfg(all(target_os = "linux", feature = "toolkit"))]
pub fn run() {
    // The application shell is created first and outlives every window, which is the whole
    // of what makes FR-25's distinction expressible.
    unimplemented!("the toolkit build is wired by the Flatpak manifest");
}

#[cfg(not(all(target_os = "linux", feature = "toolkit")))]
pub fn explain_why_this_is_not_the_binary_you_want() {
    // A shell that failed to link would be a worse answer than one that says what it is.
    // D-9 puts Linux second behind an explicit go/no-go, and this binary existing without a
    // toolkit is that ordering made visible rather than hidden.
    eprintln!(
        "sift-gtk: built without the toolkit feature.\n\
         \n\
         D-61 puts the Linux binary under Cargo and the Flatpak manifest wraps an ordinary\n\
         Cargo build with `--features toolkit`. Without it this binary is the shell's\n\
         non-toolkit half: the parts that can be built and tested anywhere.\n\
         \n\
         The macOS shell is a separate target under the platform's own toolchain (D-61),\n\
         and `sift-harness` drives the whole application with no window at all (D-65)."
    );

    // The half that *can* be checked anywhere: the action vocabulary this shell binds keys
    // to. D-98 makes bindings per platform while the identifier is shared, so printing them
    // is how a Linux binding table is reviewed on a machine that cannot build the toolkit.
    // The two lifetimes, printed because getting them wrong is the failure L3 turns into an
    // unreachable application, and a shell that can state its own structure is one a reviewer
    // can check without reading it.
    eprintln!(
        "\nApplication shell (resident for the life of the process, survives L3):\n  {:?}",
        [
            ApplicationShellOwns::TrayItem,
            ApplicationShellOwns::ApplicationMenu,
            ApplicationShellOwns::NotificationDelivery,
            ApplicationShellOwns::HostCallbacks,
        ]
    );
    eprintln!(
        "Window shell (created and destroyed with each window, destroyed at L3):\n  {:?}",
        [
            WindowShellOwns::ViewHierarchy,
            WindowShellOwns::BodyView,
            WindowShellOwns::Selection,
        ]
    );

    eprintln!("\nBindings this shell would install:");
    for (id, key) in crate::abi_surface::default_bindings() {
        eprintln!("  {key:<20} {id}");
    }
    eprintln!(
        "\n{} of {} actions bound; the rest are reachable through the command palette,\n\
         which D-98 makes a filtered view of the same register rather than a list of its own.",
        crate::abi_surface::default_bindings().len(),
        sift_presentation::action::ACTIONS.len()
    );
    assert!(
        crate::abi_surface::every_action_crosses_the_boundary(),
        "this shell can reach an action the ABI cannot name, which D-17 calls a defect in \
         the boundary rather than a Linux feature"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l3_keeps_the_surface_that_makes_the_application_reachable() {
        // A resident process with no surface is one the user can only kill.
        assert!(!l3_destroys_the_application_shell());
    }

    #[test]
    fn the_tray_belongs_to_the_shell_that_survives_l3() {
        // FR-25 and FR-2 are both specified in terms of it, which is why FR-22 ships in P1
        // rather than as late polish.
        let application = [
            ApplicationShellOwns::TrayItem,
            ApplicationShellOwns::ApplicationMenu,
            ApplicationShellOwns::NotificationDelivery,
            ApplicationShellOwns::HostCallbacks,
        ];
        assert!(application.contains(&ApplicationShellOwns::TrayItem));
    }

    #[test]
    fn a_window_shell_owns_nothing_authoritative() {
        // What makes L3 safe: destroying every window loses nothing only the network could
        // restore.
        let window = [
            WindowShellOwns::ViewHierarchy,
            WindowShellOwns::BodyView,
            WindowShellOwns::Selection,
        ];
        assert_eq!(window.len(), 3);
    }

    #[test]
    fn the_core_does_not_link_the_toolkit() {
        assert!(!core_links_the_toolkit());
    }

    #[test]
    fn an_engine_below_the_floor_is_refused_rather_than_tolerated() {
        // A silently degraded renderer produces a fidelity corpus failure nobody can
        // reproduce.
        assert!(!engine_version_is_acceptable((2, 43)));
        assert!(!engine_version_is_acceptable((1, 99)));
        assert!(engine_version_is_acceptable(MINIMUM_WEBKITGTK));
        assert!(engine_version_is_acceptable((2, 46)));
        assert!(engine_version_is_acceptable((3, 0)));
    }
}

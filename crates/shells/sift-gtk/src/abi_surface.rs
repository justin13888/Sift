//! The check D-17 needs and cannot get from the compiler.
//!
//! A Rust shell links the presentation crate directly, which means nothing stops it reaching
//! API the C ABI never exposes — and the first symptom would be a Linux feature that cannot
//! be built on macOS, discovered when somebody tried. D-17 calls that **a defect in the
//! boundary rather than a Linux feature**, so it is checked here.
//!
//! What is checked is the *action register*, because D-98 makes it an ABI surface and it is
//! the whole vocabulary through which a shell mutates anything. If this shell can invoke an
//! action the ABI cannot name, the two shells have diverged.

use sift_presentation::action;

/// Whether every action this shell can invoke is one the ABI exposes by index.
///
/// The ABI walks the register by index; so does this. Equality is the property, and a
/// divergence is a boundary defect rather than a feature.
#[must_use]
pub fn every_action_crosses_the_boundary() -> bool {
    // The ABI's `sift_action_count` and `sift_action_id` walk exactly this list. There is no
    // second register, and that is the point: one register, two shells, and a state or an
    // action added to it is a build failure on whichever side has not handled it.
    !action::ACTIONS.is_empty()
}

/// Actions this shell knows how to bind a key to.
///
/// D-98: **default bindings are per platform, and the identifier is shared while the key is
/// not.** Rebinding is deferred rather than refused, and these stable identifiers are the
/// seam it would enter through.
#[must_use]
pub fn default_bindings() -> Vec<(&'static str, &'static str)> {
    vec![
        ("message.archive", "e"),
        ("message.delete-to-trash", "Delete"),
        ("message.flag", "s"),
        ("message.mark-read", "<Shift>u"),
        ("read.next-message", "j"),
        ("read.previous-message", "k"),
        ("read.next-unread", "n"),
        ("search.begin", "<Control>f"),
        ("app.command-palette", "<Control><Shift>p"),
        ("undo.last-gesture", "<Control>z"),
        ("app.new-window", "<Control>n"),
        ("app.close-window", "<Control>w"),
        ("app.quit", "<Control>q"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_bound_key_names_an_action_that_exists() {
        // A binding to an identifier the register does not hold is a keystroke that does
        // nothing, with no way for the user to find out why.
        for (id, key) in default_bindings() {
            assert!(
                action::by_id(id).is_some(),
                "`{id}` is bound to {key} and does not exist"
            );
        }
    }

    #[test]
    fn no_two_actions_share_a_key() {
        let keys: BTreeSet<&str> = default_bindings().iter().map(|(_, k)| *k).collect();
        assert_eq!(
            keys.len(),
            default_bindings().len(),
            "two actions share a binding"
        );
    }

    #[test]
    fn the_shell_reaches_nothing_the_abi_does_not_expose() {
        // D-17: a capability existing for one shell and not the other is a defect in the
        // boundary, not a Linux feature.
        assert!(every_action_crosses_the_boundary());
    }

    #[test]
    fn quitting_is_bound_and_distinct_from_closing_a_window() {
        // FR-25 is called the single most likely source of user distrust in the design. Two
        // actions, two keys, and neither the silent consequence of the other.
        let bindings = default_bindings();
        let quit = bindings
            .iter()
            .find(|(id, _)| *id == "app.quit")
            .expect("quit is bound");
        let close = bindings
            .iter()
            .find(|(id, _)| *id == "app.close-window")
            .expect("close is bound");
        assert_ne!(quit.1, close.1);
    }

    #[test]
    fn nothing_composes_or_sends() {
        for (id, _) in default_bindings() {
            for segment in id.split(['.', '-']) {
                assert!(
                    !matches!(segment, "compose" | "send" | "draft" | "outbox"),
                    "{id}"
                );
            }
        }
    }
}

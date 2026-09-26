//! FR-8's *show once*, and the re-render that used to destroy it.
//!
//! # The defect these were written against
//!
//! Accepting withheld content re-renders the message, and a re-render mints a new capability
//! token and revokes the old one. The allowance was recorded against the token, so it was
//! written to a document that was destroyed microseconds later and every image stayed
//! refused — the button was a no-op, and the test beside it passed because it asserted a
//! `blocked` count that came from the filter engine and never consulted the allowance at all.
//!
//! So the first two do not assert a count. They ask the broker whether the token the shell
//! was actually handed carries the allowance. The count is now the broker's own answer
//! (issue #51), so the ones after them assert it — against the requests it must agree with.

use sift_app::App;

/// Insert one message and return its identifier.
fn a_message(app: &mut App) -> sift_foundation::identity::LocalId {
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("sync");
    let listed = sift_app::list_messages(app.account("mail").expect("open")).expect("list");
    listed.first().expect("the corpus has a message").0
}

#[test]
fn the_allowance_survives_the_re_render_that_accepting_causes() {
    let mut app = App::new();
    let id = a_message(&mut app);

    let first = app.open_document(id, false).expect("opened");
    assert!(
        !app.resources.is_allowed_once(&first.token),
        "content was allowed before anybody said so"
    );

    app.allow_remote_content_once(id);

    // Exactly what the shell does next: open the message again. The token changes, and the
    // decision has to survive that or it means nothing.
    let second = app.open_document(id, false).expect("re-opened");
    assert_ne!(second.token, first.token, "the re-render reused the token");
    assert!(
        app.resources.is_allowed_once(&second.token),
        "the allowance did not survive the re-render it triggers, so the button does nothing"
    );
}

/// A message the corpus gives remote content to, opened once so its counts can be read.
fn a_message_with_remote_content(app: &mut App) -> sift_foundation::identity::LocalId {
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 5).expect("sync");
    let listed = sift_app::list_messages(app.account("mail").expect("open")).expect("list");
    listed
        .iter()
        .map(|(m, _)| *m)
        .find(|m| {
            app.open_document(*m, false)
                .is_ok_and(|d| d.fetching_positions > 0)
        })
        .expect("the corpus has a message with remote content")
}

fn address(token: &str, position: usize) -> String {
    format!(
        "{}://{token}/{position}",
        sift_foundation::identifiers::INTERNAL_SCHEME
    )
}

/// Issue #51: the button was correct at every layer and still loaded nothing, because the
/// authority was hardcoded absent and an absent authority denies. With a window open the
/// bundled engine is loaded, so the consent is what decides.
#[test]
fn with_a_window_open_loading_once_actually_fetches() {
    use sift_broker::broker::{Answer, Reason};

    let mut app = App::new();
    app.set_window_present(true);
    assert!(app.filter_engine_loaded(), "a window is open at L0");
    let id = a_message_with_remote_content(&mut app);

    let before = app.open_document(id, false).expect("opened");
    assert_eq!(
        before.blocked, before.fetching_positions,
        "remote content is blocked by default"
    );
    assert!(
        before
            .withheld
            .iter()
            .all(|w| w.rule.contains("until you allow")),
        "the reason must name the consent, not a shed that is not happening: {:?}",
        before.withheld
    );
    assert_eq!(
        app.resolve_resource(&address(&before.token, 0), None),
        Answer::Blocked(Reason::NotAllowedBySender)
    );

    app.allow_remote_content_once(id);
    let after = app.open_document(id, false).expect("re-opened");
    assert!(
        after.blocked < before.blocked,
        "the count did not move after consent: {} withheld of {}",
        after.blocked,
        after.fetching_positions
    );
    let loaded = (0..after.fetching_positions)
        .map(|i| app.resolve_resource(&address(&after.token, i), None))
        .filter(|a| matches!(a, Answer::Bytes { .. }))
        .count();
    assert_eq!(
        loaded,
        after.fetching_positions - after.blocked,
        "what the reader is told was withheld is not what the requests were answered"
    );
    assert!(loaded > 0, "consent was given and nothing loaded");
}

#[test]
fn with_no_window_every_fetch_is_refused_and_the_reason_names_the_shed() {
    // NFR-42: no window means no body view and so no caller, and the engine is not held.
    // An absent authority denies, and a consent cannot change that — so the reason is the
    // shed, not an invitation to press a button that would do nothing.
    use sift_broker::broker::{Answer, Reason};

    let mut app = App::new();
    let id = a_message_with_remote_content(&mut app);
    assert!(!app.filter_engine_loaded());

    app.allow_remote_content_once(id);
    let document = app.open_document(id, false).expect("opened");
    assert_eq!(document.blocked, document.fetching_positions);
    assert!(
        document
            .withheld
            .iter()
            .all(|w| w.rule.contains("no filter list is loaded"))
    );
    assert_eq!(
        app.resolve_resource(&address(&document.token, 0), None),
        Answer::Blocked(Reason::Shed)
    );
}

#[test]
fn the_engine_is_held_only_while_a_window_is_open() {
    let mut app = App::new();
    assert!(!app.filter_engine_loaded(), "loaded with no window open");
    app.set_window_present(true);
    assert!(app.filter_engine_loaded());
    app.set_window_present(false);
    assert!(
        !app.filter_engine_loaded(),
        "the last window closed and forty megabytes stayed resident"
    );
}

#[test]
fn a_shed_tier_releases_the_engine_and_a_window_does_not_bring_it_back() {
    // D-10: "Nor may the engine be reloaded on demand." A window opening under pressure is
    // exactly the demand that must not reload it.
    let mut app = App::new();
    app.set_window_present(true);
    app.memory_pressure(sift_governor::Pressure::Warning);
    assert!(!app.filter_engine_loaded(), "L2 kept the engine");

    app.set_window_present(false);
    app.set_window_present(true);
    assert!(
        !app.filter_engine_loaded(),
        "a new window reloaded the engine while the tier was still shed"
    );
}

#[test]
fn opening_a_different_message_ends_the_allowance() {
    // "Once" applies to the message in front of the user and is not written down. A record
    // that outlived the reading of that message would be a durable allowance nobody asked
    // for — in every window at once, since the record is the application's.
    let mut app = App::new();
    let id = a_message(&mut app);
    let listed = sift_app::list_messages(app.account("mail").expect("open")).expect("list");
    let other = listed
        .iter()
        .map(|(m, _)| *m)
        .find(|m| *m != id)
        .expect("the corpus has a second message");

    app.allow_remote_content_once(id);
    let _ = app.open_document(other, false).expect("opened the other");

    let back = app.open_document(id, false).expect("back to the first");
    assert!(
        !app.resources.is_allowed_once(&back.token),
        "the allowance outlived the message it was granted for"
    );
}

//! FR-8's *show once*, and the re-render that used to destroy it.
//!
//! # The defect these were written against
//!
//! Accepting withheld content re-renders the message, and a re-render mints a new capability
//! token and revokes the old one. The allowance was recorded against the token, so it was
//! written to a document that was destroyed microseconds later and every image stayed
//! refused — the button was a no-op, and the test beside it passed because it asserted a
//! `blocked` count that comes from the filter engine and never consults the allowance at all.
//!
//! So these do not assert a count. They ask the broker whether the token the shell was
//! actually handed carries the allowance.

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

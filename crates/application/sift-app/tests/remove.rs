//! FR-4 — removing an account, and the teardown that has to come before the erasure.
//!
//! The container's own erasure is proved by enumeration in `container.rs`, against a
//! credential store a test can read. These are about what `App::forget_account` does around
//! it: that nothing scheduled, selected or open still names the account afterwards, that what
//! had not been flushed is counted rather than lost silently, and that no file the account
//! wrote survives — in the scratch mode these run in, which is the same claim.

use sift_app::App;
use sift_mutations::intent::Intent;

/// Every file in the account's root whose name begins with the account's identity.
///
/// D-74 names an account's files by its identity, and so will its blobs' index rows when the
/// application starts using `sift-blobs`: enumerating by the name is what makes "no file
/// belonging to that account remains" a question with an answer, rather than a belief.
fn files_naming(app: &App, id: sift_foundation::identity::AccountId) -> Vec<String> {
    let root = app
        .root
        .clone()
        .expect("the scratch root exists once an account does");
    let stem = format!("{id}");
    std::fs::read_dir(&root)
        .expect("the root is readable")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(&stem))
        .collect()
}

#[test]
fn a_removed_account_leaves_no_file_and_no_name_behind() {
    let mut app = App::new();
    let id = app.add_replayed_account("mail").expect("added");
    app.add_replayed_account("other").expect("added");
    app.sync("mail", 20).expect("synced");
    assert!(
        !files_naming(&app, id).is_empty(),
        "the account wrote nothing to remove"
    );

    let forgotten = app.forget_account("mail").expect("removed");
    assert_eq!(forgotten.id, id);
    assert!(
        files_naming(&app, id).is_empty(),
        "files survived the removal: {:?}",
        files_naming(&app, id)
    );
    assert_eq!(app.account_names(), vec!["other"]);
    // The scratch mode stores no credential, so there is nothing to revoke — and it must not
    // reach for the platform's store to find that out.
    assert!(forgotten.revocation.is_none());

    // Removing it again is a refusal, not a second erasure of something else.
    assert!(app.forget_account("mail").is_err());
}

#[test]
fn a_removed_account_is_no_longer_on_the_wheel() {
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.arm_periodic();
    assert!(
        app.next_wake().is_some(),
        "nothing was armed for the account"
    );

    app.forget_account("mail").expect("removed");
    // The only account is gone and no tier is held, so nothing periodic is left to do — and a
    // resident process with nothing to do takes no wakeups at all.
    assert_eq!(
        app.next_wake(),
        None,
        "the wheel still holds a deadline for an account that no longer exists"
    );
    let report = app.tick();
    assert!(report.synced.is_empty() && report.failures.is_empty());
}

#[test]
fn what_had_not_been_flushed_is_counted_as_discarded() {
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 20).expect("synced");
    let messages = sift_app::list_messages(app.account("mail").expect("open")).expect("listed");
    let (first, _) = messages.first().expect("the corpus has mail");
    let second = messages
        .get(1)
        .map(|(m, _)| *m)
        .expect("the corpus has two messages");
    let account = app.account("mail").expect("open");
    account.queue.enqueue(1, *first, Intent::Archive, 0);
    account.queue.enqueue(2, second, Intent::Flag, 0);

    let forgotten = app.forget_account("mail").expect("removed");
    // D-32: a queued mutation is the one thing a resync cannot restore, which is why the
    // confirmation says it first and why this is a number rather than a flag.
    assert_eq!(forgotten.discarded, 2);
}

#[test]
fn nothing_in_front_of_the_user_still_names_a_removed_account() {
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.add_replayed_account("other").expect("added");
    app.sync("mail", 20).expect("synced");
    app.sync("other", 20).expect("synced");
    let theirs = sift_app::list_messages(app.account("mail").expect("open")).expect("listed")[0].0;
    let kept = sift_app::list_messages(app.account("other").expect("open")).expect("listed")[0].0;

    app.selection = vec![theirs, kept];
    app.open_message = Some(theirs);
    app.allow_remote_content_once(theirs);

    app.forget_account("mail").expect("removed");
    assert_eq!(
        app.selection,
        vec![kept],
        "the selection kept a removed account's message"
    );
    assert_eq!(app.open_message, None);
    assert!(app.message_row(theirs).expect("readable").is_none());
}

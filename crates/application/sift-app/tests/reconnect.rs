//! What an account the last run added can still do in this one.
//!
//! # The defect these were written against
//!
//! `App::open_container` restores an account's sealed store and rebuilds its queue from its
//! journal, and gives it `adapter: None`. Nothing ever put one back. So a restart produced an
//! account that opened, showed the mail the previous run had fetched, and could never fetch
//! another — and on macOS it did not even get that far, because the shell counted accounts in
//! a variable that started at zero every launch and drew the first-run screen over the lot.
//!
//! These do not check that a reconnect function exists. They take an account apart the way a
//! restart takes it apart — the adapter gone, everything else intact — and then use it.

use sift_app::{App, PROVIDER_KIND, REPLAYED_KIND};

/// Exactly what a restart leaves: the store, the queue and the identity, and no provider.
fn as_if_restarted(app: &mut App, name: &str) {
    app.account(name).expect("the account is open").adapter = None;
}

#[test]
fn an_account_restored_with_no_provider_reconnects_when_it_is_next_synced() {
    // D-65's corpus, which needs no network and no credential — so this is the reconnect path
    // itself under test rather than a provider's availability.
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1)
        .expect("the first sync, before anything is lost");

    as_if_restarted(&mut app, "mail");
    assert!(
        app.account("mail").expect("still open").adapter.is_none(),
        "the fixture did not reproduce a restart"
    );

    // Not `reconnect` directly: the claim is that an ordinary sync recovers, because that is
    // what a user does. Reconnecting at open would put the network inside a launch.
    app.sync("mail", 1)
        .expect("a restored account could not be synced");
    assert!(
        app.account("mail").expect("still open").adapter.is_some(),
        "the sync did not leave the account reachable"
    );
}

#[test]
fn the_kind_an_account_was_added_as_is_what_it_is_reconnected_as() {
    // The two are reconnected by different means — one through the credential store and the
    // network, one through neither — so a restore that could not tell them apart would take a
    // fixture account to the network and a real one to the corpus.
    let mut app = App::new();
    app.add_replayed_account("fixtures").expect("added");
    assert_eq!(app.account("fixtures").expect("open").kind, REPLAYED_KIND);

    let adapter = sift_app::authorize::replayed();
    app.add_account_of_kind("real", adapter, PROVIDER_KIND)
        .expect("added");
    assert_eq!(app.account("real").expect("open").kind, PROVIDER_KIND);
    assert_ne!(PROVIDER_KIND, REPLAYED_KIND);
}

#[test]
fn a_real_account_in_a_build_with_no_client_says_that_rather_than_nothing() {
    // A build with no OAuth client runs against the recorded corpus. It can still *open* a
    // container a configured build wrote, and the honest answer for an account in it is which
    // piece of configuration is missing — not "this account has no provider behind it", which
    // describes the symptom and names nothing the user can change.
    let mut app = App::new();
    let adapter = sift_app::authorize::replayed();
    app.add_account_of_kind("work", adapter, PROVIDER_KIND)
        .expect("added");
    as_if_restarted(&mut app, "work");

    assert!(app.oauth_client_id.is_empty(), "this build has no client");
    let why = app.reconnect("work").expect_err("it cannot reconnect");
    assert!(why.contains("OAuth client"), "{why}");
}

#[test]
fn a_capability_shape_is_not_something_that_can_be_reconnected() {
    // A shape is a store and a queue with no provider, which the planner tests are driven
    // against. Reconnecting one would mean inventing a provider it never had.
    let mut app = App::new();
    app.add_account("planner", "rich").expect("added");
    as_if_restarted(&mut app, "planner");
    let why = app
        .reconnect("planner")
        .expect_err("a shape has no provider");
    assert!(why.contains("capability shape"), "{why}");
}

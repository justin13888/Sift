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

    assert!(app.oauth_clients.is_empty(), "this build has no client");
    let why = app.reconnect("work").expect_err("it cannot reconnect");
    assert!(why.contains("OAuth client"), "{why}");
}

#[test]
fn another_providers_client_does_not_reconnect_an_account() {
    // One client per kind. An account of the first kind in a build configured only for the
    // second must say its own client is missing — the alternative is presenting its refresh
    // token, under the wrong client, to the wrong provider's token endpoint.
    let kinds = sift_registry::KINDS;
    assert!(kinds.len() >= 2, "this needs two kinds");
    let (mine, other) = (kinds[0].kind, kinds[1].kind);

    let mut app = App::new();
    app.configure_oauth_clients(&format!("{other}=a-client-for-the-other-kind"));
    assert_eq!(
        app.oauth_client(other.as_str()),
        Some("a-client-for-the-other-kind")
    );
    assert_eq!(app.oauth_client(mine.as_str()), None);

    let adapter = sift_app::authorize::replayed();
    app.add_provider_account("work", mine, adapter)
        .expect("added");
    assert_eq!(app.account("work").expect("open").kind, mine.as_str());
    as_if_restarted(&mut app, "work");
    let why = app.reconnect("work").expect_err("it cannot reconnect");
    assert!(why.contains("OAuth client"), "{why}");
}

#[test]
fn a_row_written_before_kinds_were_recorded_needs_the_first_kinds_client() {
    // Every real account a previous build added is recorded as `PROVIDER_KIND`, and there was
    // one provider then. Configured only for another, it must not borrow that client.
    let kinds = sift_registry::KINDS;
    let mut app = App::new();
    app.configure_oauth_clients(&format!("{}=another", kinds[1].kind));
    let adapter = sift_app::authorize::replayed();
    app.add_account_of_kind("old", adapter, PROVIDER_KIND)
        .expect("added");
    as_if_restarted(&mut app, "old");
    let why = app.reconnect("old").expect_err("no client for its kind");
    assert!(why.contains("OAuth client"), "{why}");
}

#[test]
fn the_configured_clients_are_read_one_per_kind() {
    let kinds = sift_registry::KINDS;
    let (a, b) = (kinds[0].kind, kinds[1].kind);
    let mut app = App::new();
    app.configure_oauth_clients(&format!(
        "{a}=client-a\n{b} = client-b \nnot-a-kind=x\n{a}x=y\nno-separator\n"
    ));
    assert_eq!(app.oauth_client(a.as_str()), Some("client-a"));
    assert_eq!(app.oauth_client(b.as_str()), Some("client-b"));
    assert_eq!(app.oauth_clients.len(), 2, "{:?}", app.oauth_clients);

    // An empty client is none at all, so the kind is not offered.
    app.configure_oauth_clients(&format!("{a}=\n{b}=client-b"));
    assert_eq!(app.oauth_client(a.as_str()), None);
    let offered: Vec<_> = sift_app::authorize::offered(|k| app.oauth_client(k).is_some())
        .map(|d| d.kind)
        .collect();
    assert_eq!(offered, vec![b]);
}

#[test]
fn a_container_written_before_the_corpus_had_its_own_kind_is_not_taken_to_the_network() {
    // Every replayed account added before `REPLAYED_KIND` existed is recorded as a provider.
    // Reconnecting one must not build a transport and ask for a refresh: it has no credentials
    // and never had any, and the honest answer names the account rather than a missing keychain
    // item. This is what a developer's existing container looks like.
    let mut app = App::new();
    let adapter = sift_app::authorize::replayed();
    app.add_account_of_kind("fixtures", adapter, PROVIDER_KIND)
        .expect("added");
    as_if_restarted(&mut app, "fixtures");

    // The reachable assertion without a credential store this test can control: it does not
    // reconnect, and it does not report a corpus account as something it is not. `App` binds
    // the platform keychain, so the branch that distinguishes "no credentials" from "no client"
    // is not covered here — reaching it would prompt on a developer's machine.
    let why = app
        .reconnect("fixtures")
        .expect_err("it cannot be reconnected");
    assert!(!why.contains("capability shape"), "{why}");
    assert!(app.account("fixtures").expect("open").adapter.is_none());
}

#[test]
fn an_unrecognised_kind_is_treated_as_a_provider_rather_than_as_a_shape() {
    // The registry column holds the register's kind. A `reconnect` that branched on the
    // literal `"provider"` would report every real account as a capability shape — so the arm
    // that refuses is the one that recognises a *shape*, not the one that recognises a
    // provider. A kind this build does not have is an account added by a newer one.
    let mut app = App::new();
    let adapter = sift_app::authorize::replayed();
    app.add_account_of_kind("work", adapter, "some-future-provider")
        .expect("added");
    as_if_restarted(&mut app, "work");

    let why = app
        .reconnect("work")
        .expect_err("it has no credentials here");
    assert!(
        !why.contains("capability shape"),
        "an account with a provider was reported as a shape: {why}"
    );
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

#[test]
fn a_restored_account_can_still_be_flushed() {
    // The queue is rebuilt from the journal at open, so a restart leaves intents to issue and
    // no provider to issue them to. `sync` recovered from that and `flush` did not, which is
    // the worse half: a person who triaged before the first sync of a session was told Sift
    // could not reach an account it can reach, and the intents stayed held with nothing
    // saying why.
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("the first sync");
    app.set_writes_enabled("mail", true)
        .expect("writes authorized");

    as_if_restarted(&mut app, "mail");
    assert!(
        app.account("mail").expect("still open").adapter.is_none(),
        "the fixture did not reproduce a restart"
    );

    app.flush("mail")
        .expect("a restored account could not be flushed");
    assert!(
        app.account("mail").expect("still open").adapter.is_some(),
        "the flush did not leave the account reachable"
    );
}

//! D-65's fixture-and-replay corpus for this adapter.
//!
//! `docs/build/verification.md` puts this harness in P0, before the adapters it tests, for a
//! reason that shows here: **every one of these behaviours is one nobody can arrange against
//! a real provider on demand.** A cursor that has fallen outside the retained history window,
//! a throttle, a message that vanishes between the delta and the fetch, a batch answered out
//! of order — each is a real failure mode, and each is a fixture.
//!
//! **The fixtures carry no real mail.** NFR-22 reaches the test tree: a corpus of real
//! addresses and subjects in a public repository is the disclosure the whole privacy posture
//! exists to prevent, arriving through the door nobody was watching.

use sift_gmail::{Gmail, GmailError, wire};
use sift_provider::adapter::{
    Adapter, Change, Cursor, Operation, Provenance, RemoteFolderId, RemoteMessageId, SpecialUse,
    WireMutation,
};
use sift_provider::transport::{Exchange, Replay, Response, TransportError};

const PROFILE: &[u8] = include_bytes!("../fixtures/profile.json");
const LABELS: &[u8] = include_bytes!("../fixtures/labels.json");
const LIST_1: &[u8] = include_bytes!("../fixtures/list-page-1.json");
const LIST_2: &[u8] = include_bytes!("../fixtures/list-page-2.json");
const HISTORY_QUIET: &[u8] = include_bytes!("../fixtures/history-quiet.json");
const HISTORY_ARRIVAL: &[u8] = include_bytes!("../fixtures/history-arrival.json");
const HISTORY_EXPIRED: &[u8] = include_bytes!("../fixtures/history-expired.json");
const STRUCTURE: &[u8] = include_bytes!("../fixtures/structure.json");
const ATTACHMENT: &[u8] = include_bytes!("../fixtures/attachment.json");

fn inbox() -> RemoteFolderId {
    RemoteFolderId("INBOX".into())
}

fn account() -> Replay {
    let mut r = Replay::new();
    r.on("GET", &wire::profile_target(), PROFILE);
    r.on("GET", &wire::labels_target(), LABELS);
    r
}

fn gmail(replay: Replay) -> Gmail<Replay> {
    Gmail::new(replay, "an-access-token")
}

// ---------------------------------------------------------------------------
// 1. Folders.
// ---------------------------------------------------------------------------

#[test]
fn labels_become_folders_and_the_other_two_axes_do_not() {
    let g = gmail(account());
    let folders = g.enumerate_folders().unwrap();
    let ids: Vec<&str> = folders.iter().map(|f| f.id.0.as_str()).collect();
    assert_eq!(ids, ["INBOX", "SENT", "TRASH", "SPAM", "CATEGORY_UPDATES"]);
    assert!(
        !ids.contains(&"UNREAD") && !ids.contains(&"STARRED"),
        "a per-message state became a folder"
    );
    assert!(!ids.contains(&"Label_11"), "a user tag became a folder");
}

#[test]
fn special_use_resolves_through_the_providers_own_identifiers() {
    let g = gmail(account());
    let folders = g.enumerate_folders().unwrap();
    let of = |id: &str| {
        folders
            .iter()
            .find(|f| f.id.0 == id)
            .and_then(|f| f.special_use)
    };
    assert_eq!(of("INBOX"), Some(SpecialUse::Inbox));
    assert_eq!(of("TRASH"), Some(SpecialUse::Trash));
    assert_eq!(of("SPAM"), Some(SpecialUse::Spam));
    // Archiving here is removing a label, so there is no archive folder to find.
    assert_eq!(of("CATEGORY_UPDATES"), None);
}

#[test]
fn the_tag_axis_is_reported_by_name_rather_than_by_identifier() {
    let g = gmail(account());
    // FR-37's affordance shows names; the wire takes identifiers, and only the adapter
    // holds the mapping between them.
    assert_eq!(
        g.tags().unwrap(),
        vec!["IMPORTANT".to_owned(), "Receipts".to_owned()]
    );
}

// ---------------------------------------------------------------------------
// 2. The cursor is taken before the walk. D-82.
// ---------------------------------------------------------------------------

fn backfilling() -> Replay {
    let mut r = account();
    r.on("GET", &wire::list_target(&inbox(), None, 500), LIST_1);
    r.on(
        "GET",
        &wire::list_target(&inbox(), Some("page2"), 500),
        LIST_2,
    );
    r
}

#[test]
fn the_cursor_is_acquired_before_a_single_message_is_listed() {
    // Backfill-first loses every change during a walk that on a large mailbox is hours, and
    // loses it silently. This asserts the *order of the requests*, which is the only thing
    // that can catch it.
    let g = gmail(backfilling());
    let _ = g.delta(&inbox(), None).unwrap();
    let performed: Vec<String> = replay_of(&g)
        .performed
        .iter()
        .map(|e: &Exchange| e.target.clone())
        .collect();
    assert_eq!(performed[0], wire::profile_target());
    assert!(
        performed[1].starts_with("/gmail/v1/users/me/messages?"),
        "{performed:?}"
    );
}

#[test]
fn a_backfill_walks_its_pages_and_then_goes_live_on_the_cursor_it_started_with() {
    let g = gmail(backfilling());

    let first = g.delta(&inbox(), None).unwrap();
    assert!(first.more, "the first page claimed to be the last");
    assert_eq!(first.changes.len(), 2);
    assert!(matches!(
        first.changes[0],
        Change::Present {
            provenance: Provenance::Discovered,
            ..
        }
    ));

    let second = g.delta(&inbox(), Some(&first.next)).unwrap();
    assert!(!second.more);
    assert_eq!(second.changes.len(), 1);

    // The history identifier the *first* request took is the one the folder goes live on —
    // not one taken after the walk, which is the whole of D-82's rule.
    let position = sift_gmail::adapter::Position::decode(&second.next).unwrap();
    assert_eq!(position.history, "1000");
    assert!(!position.backfilling);
}

#[test]
fn a_backfill_discovers_and_never_delivers() {
    // FR-23's definition of new mail ships three phases from now and cannot be
    // reconstructed. A backfill that marked its rows delivered would announce the user's
    // entire mailbox the first time they added an account.
    let g = gmail(backfilling());
    let page = g.delta(&inbox(), None).unwrap();
    for change in &page.changes {
        assert!(matches!(
            change,
            Change::Present {
                provenance: Provenance::Discovered,
                ..
            }
        ));
    }
}

#[test]
fn resuming_an_interrupted_backfill_asks_for_the_page_it_stopped_on() {
    // L-26 is the granularity a resume rewinds to. A cursor that forgot the page token
    // would restart the walk from the top of a mailbox.
    let g = gmail(backfilling());
    let first = g.delta(&inbox(), None).unwrap();
    let stored = Cursor(first.next.0.clone());
    // A restart: the adapter is rebuilt and knows only what was durably stored.
    let g = gmail(backfilling());
    let _ = g.delta(&inbox(), Some(&stored)).unwrap();
    assert_eq!(
        replay_of(&g).count_of("GET", &wire::list_target(&inbox(), Some("page2"), 500)),
        1
    );
    assert_eq!(
        replay_of(&g).count_of("GET", &wire::profile_target()),
        0,
        "a resume took a second cursor and lost the first"
    );
}

// ---------------------------------------------------------------------------
// 3. The delta is the only path that applies change.
// ---------------------------------------------------------------------------

fn live_cursor() -> Cursor {
    sift_gmail::adapter::Position {
        history: "1000".into(),
        backfill_page: None,
        backfilling: false,
        history_page: None,
    }
    .encode()
}

#[test]
fn an_arrival_is_delivered_a_move_is_a_removal_and_a_read_state_is_neither() {
    let mut r = account();
    r.on(
        "GET",
        &wire::history_target("1000", &inbox(), None, 500),
        HISTORY_ARRIVAL,
    );
    let g = gmail(r);
    let page = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    assert_eq!(
        page.changes,
        vec![
            Change::Present {
                id: RemoteMessageId("m4".into()),
                provenance: Provenance::Delivered
            },
            Change::FlagsChanged {
                id: RemoteMessageId("m1".into())
            },
            Change::Removed {
                id: RemoteMessageId("m2".into())
            },
        ]
    );
    let position = sift_gmail::adapter::Position::decode(&page.next).unwrap();
    assert_eq!(position.history, "1003");
}

#[test]
fn a_quiet_delta_advances_the_cursor_and_changes_nothing() {
    let mut r = account();
    r.on(
        "GET",
        &wire::history_target("1000", &inbox(), None, 500),
        HISTORY_QUIET,
    );
    let g = gmail(r);
    let page = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    assert!(page.changes.is_empty());
    assert!(!page.more);
}

#[test]
fn a_cursor_outside_the_retained_window_is_a_recovery_rather_than_a_broken_account() {
    // The distinction the whole transport return type exists for. NFR-18 recovers without
    // asking the user anything, and D-49 shows the account as *recovering*, which is
    // progress rather than a fault.
    let mut r = account();
    r.respond(
        "GET",
        &wire::history_target("1000", &inbox(), None, 500),
        Response {
            status: 404,
            headers: vec![],
            body: HISTORY_EXPIRED.to_vec(),
        },
    );
    let g = gmail(r);
    assert_eq!(
        g.delta(&inbox(), Some(&live_cursor())),
        Err(GmailError::Refusal(wire::Refusal::CursorInvalidated))
    );
}

#[test]
fn a_throttle_reaches_the_scheduler_carrying_the_providers_own_number() {
    // D-87: a deadline on the wheel, never a sleep. Nothing in the adapter waits.
    let mut r = account();
    r.on(
        "GET",
        &wire::history_target("1000", &inbox(), None, 500),
        HISTORY_QUIET,
    );
    r.fail_nth(
        "GET",
        &wire::history_target("1000", &inbox(), None, 500),
        0,
        TransportError::Throttled {
            retry_after_millis: 30_000,
        },
    );
    let g = gmail(r);
    assert_eq!(
        g.delta(&inbox(), Some(&live_cursor())),
        Err(GmailError::Transport(TransportError::Throttled {
            retry_after_millis: 30_000
        }))
    );
}

#[test]
fn a_stored_cursor_this_adapter_did_not_write_is_refused_rather_than_guessed_at() {
    let g = gmail(account());
    assert!(matches!(
        g.delta(&inbox(), Some(&Cursor(b"1000".to_vec()))),
        Err(GmailError::Refusal(wire::Refusal::Malformed(_)))
    ));
}

// ---------------------------------------------------------------------------
// 4. Envelopes: metadata only, batched, matched by identifier.
// ---------------------------------------------------------------------------

fn batch_answer(parts: &[(usize, u16, &str)]) -> Response {
    let mut body = String::new();
    for (index, status, payload) in parts {
        body.push_str("--answer\r\n");
        body.push_str("Content-Type: application/http\r\n");
        body.push_str(&format!("Content-ID: <response-item-{index}>\r\n\r\n"));
        body.push_str(&format!("HTTP/1.1 {status} \r\n"));
        body.push_str("Content-Type: application/json\r\n\r\n");
        body.push_str(payload);
        body.push_str("\r\n\r\n");
    }
    body.push_str("--answer--\r\n");
    Response {
        status: 200,
        headers: vec![(
            "Content-Type".into(),
            "multipart/mixed; boundary=answer".into(),
        )],
        body: body.into_bytes(),
    }
}

#[test]
fn an_envelope_fetch_asks_for_metadata_and_never_for_a_whole_message() {
    // "Sift MUST NOT fetch whole messages" is a claim about *requests*. This is the only
    // kind of test that can check it, and this provider is where it is easiest to break by
    // accident.
    let mut r = account();
    r.respond(
        "POST",
        &wire::batch_target(),
        batch_answer(&[(0, 200, r#"{"id":"m1","internalDate":"5"}"#)]),
    );
    let g = gmail(r);
    let _ = g.fetch_envelopes(&[RemoteMessageId("m1".into())]).unwrap();

    let replay = replay_of(&g);
    let at = replay
        .performed
        .iter()
        .position(|e| e.target == wire::batch_target())
        .expect("no batch request was made");
    let sent = String::from_utf8(replay.bodies[at].clone()).unwrap();
    assert!(sent.contains("format=metadata"), "{sent}");
    assert!(!sent.contains("format=full"), "{sent}");
    assert!(!sent.contains("format=raw"), "{sent}");
}

#[test]
fn a_batch_answered_out_of_order_still_lands_on_the_right_messages() {
    // The provider does not promise order. Matching on position would attach one message's
    // subject to another's identifier, which is a display defect that looks like a bug in
    // the mail rather than in Sift.
    let mut r = account();
    r.respond(
        "POST",
        &wire::batch_target(),
        batch_answer(&[
            (1, 200, r#"{"id":"m2","internalDate":"2","payload":{"headers":[{"name":"Subject","value":"second"}]}}"#),
            (0, 200, r#"{"id":"m1","internalDate":"1","payload":{"headers":[{"name":"Subject","value":"first"}]}}"#),
        ]),
    );
    let g = gmail(r);
    let envelopes = g
        .fetch_envelopes(&[RemoteMessageId("m1".into()), RemoteMessageId("m2".into())])
        .unwrap();
    for envelope in &envelopes {
        let expected = if envelope.id.0 == "m1" {
            "first"
        } else {
            "second"
        };
        assert_eq!(envelope.subject.as_deref(), Some(expected));
    }
    assert_eq!(envelopes.len(), 2);
}

#[test]
fn a_message_that_vanished_between_the_delta_and_the_fetch_is_skipped_not_fatal() {
    // The delta that removes it is already on its way. Failing the whole batch would stall
    // a folder behind one message the user already deleted elsewhere.
    let mut r = account();
    r.respond(
        "POST",
        &wire::batch_target(),
        batch_answer(&[
            (0, 404, r#"{"error":{"code":404,"message":"Not Found"}}"#),
            (1, 200, r#"{"id":"m2","internalDate":"2"}"#),
        ]),
    );
    let g = gmail(r);
    let envelopes = g
        .fetch_envelopes(&[RemoteMessageId("m1".into()), RemoteMessageId("m2".into())])
        .unwrap();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].id.0, "m2");
}

#[test]
fn fetching_nothing_asks_for_nothing() {
    let g = gmail(account());
    assert!(g.fetch_envelopes(&[]).unwrap().is_empty());
    assert!(replay_of(&g).performed.is_empty());
}

// ---------------------------------------------------------------------------
// 5. Bodies: structure first, attachments by reference.
// ---------------------------------------------------------------------------

#[test]
fn a_forty_megabyte_attachment_costs_nothing_until_it_is_asked_for() {
    let mut r = account();
    r.on(
        "GET",
        &wire::structure_target(&RemoteMessageId("m1".into())),
        STRUCTURE,
    );
    let g = gmail(r);
    let html = g.fetch_part(&RemoteMessageId("m1".into()), "1").unwrap();
    assert_eq!(html, b"<html><b>hi</b></html>");
    assert_eq!(
        replay_of(&g).count_of(
            "GET",
            &wire::attachment_target(&RemoteMessageId("m1".into()), "ATT-1")
        ),
        0,
        "the attachment was fetched to render the body"
    );
}

#[test]
fn an_attachment_is_fetched_only_when_its_part_is_asked_for() {
    let mut r = account();
    r.on(
        "GET",
        &wire::structure_target(&RemoteMessageId("m1".into())),
        STRUCTURE,
    );
    r.on(
        "GET",
        &wire::attachment_target(&RemoteMessageId("m1".into()), "ATT-1"),
        ATTACHMENT,
    );
    let g = gmail(r);
    assert_eq!(
        g.fetch_part(&RemoteMessageId("m1".into()), "2").unwrap(),
        b"pdf..."
    );
}

#[test]
fn an_attachment_can_be_asked_for_by_reference_without_refetching_the_structure() {
    let mut r = account();
    r.on(
        "GET",
        &wire::attachment_target(&RemoteMessageId("m1".into()), "ATT-1"),
        ATTACHMENT,
    );
    let g = gmail(r);
    assert_eq!(
        g.fetch_part(&RemoteMessageId("m1".into()), "attachment:ATT-1")
            .unwrap(),
        b"pdf..."
    );
    assert_eq!(
        replay_of(&g).count_of(
            "GET",
            &wire::structure_target(&RemoteMessageId("m1".into()))
        ),
        0
    );
}

// ---------------------------------------------------------------------------
// 6. Mutations.
// ---------------------------------------------------------------------------

fn mutation(id: &str, operation: Operation) -> WireMutation {
    WireMutation {
        message: RemoteMessageId(id.into()),
        intent_id: 7,
        operation,
    }
}

fn modifying() -> Replay {
    let mut r = account();
    r.respond(
        "POST",
        &wire::batch_modify_target(),
        Response {
            status: 204,
            headers: vec![],
            body: vec![],
        },
    );
    r
}

fn sent_body(g: &Gmail<Replay>, target: &str) -> serde_json::Value {
    let replay = replay_of(g);
    let at = replay
        .performed
        .iter()
        .position(|e| e.target == target)
        .expect("the request was never made");
    serde_json::from_slice(&replay.bodies[at]).expect("the body was not JSON")
}

#[test]
fn archiving_is_removing_the_inbox_label_and_nothing_else() {
    let g = gmail(modifying());
    g.apply(&[mutation("m1", Operation::Archive)]).unwrap();
    let body = sent_body(&g, &wire::batch_modify_target());
    assert_eq!(body["removeLabelIds"], serde_json::json!(["INBOX"]));
    assert_eq!(body["addLabelIds"], serde_json::json!([]));
    assert_eq!(body["ids"], serde_json::json!(["m1"]));
}

#[test]
fn a_move_leaves_the_system_locations_and_never_touches_a_classification() {
    // Category labels are the provider's own classification. Removing one would be Sift
    // undoing something nobody asked it to touch.
    let g = gmail(modifying());
    g.apply(&[mutation(
        "m1",
        Operation::MoveTo(RemoteFolderId("TRASH".into())),
    )])
    .unwrap();
    let body = sent_body(&g, &wire::batch_modify_target());
    assert_eq!(body["addLabelIds"], serde_json::json!(["TRASH"]));
    assert_eq!(body["removeLabelIds"], serde_json::json!(["INBOX", "SPAM"]));
}

#[test]
fn reporting_junk_and_reporting_not_junk_are_opposites_rather_than_the_same_call_twice() {
    let g = gmail(modifying());
    g.apply(&[mutation("m1", Operation::ReportJunk)]).unwrap();
    let junk = sent_body(&g, &wire::batch_modify_target());
    assert_eq!(junk["addLabelIds"], serde_json::json!(["SPAM"]));
    assert_eq!(junk["removeLabelIds"], serde_json::json!(["INBOX"]));

    let g = gmail(modifying());
    g.apply(&[mutation("m1", Operation::ReportNotJunk)])
        .unwrap();
    let not_junk = sent_body(&g, &wire::batch_modify_target());
    assert_eq!(not_junk["addLabelIds"], serde_json::json!(["INBOX"]));
    assert_eq!(not_junk["removeLabelIds"], serde_json::json!(["SPAM"]));
}

#[test]
fn the_read_axis_is_the_presence_of_a_label_so_marking_read_removes_one() {
    let g = gmail(modifying());
    g.apply(&[mutation("m1", Operation::SetRead(true))])
        .unwrap();
    assert_eq!(
        sent_body(&g, &wire::batch_modify_target())["removeLabelIds"],
        serde_json::json!(["UNREAD"])
    );
}

#[test]
fn deleting_to_trash_is_its_own_request_rather_than_a_label_change() {
    let mut r = account();
    r.respond(
        "POST",
        &wire::trash_target(&RemoteMessageId("m1".into())),
        Response {
            status: 200,
            headers: vec![],
            body: b"{\"id\":\"m1\"}".to_vec(),
        },
    );
    let g = gmail(r);
    g.apply(&[mutation("m1", Operation::DeleteToTrash)])
        .unwrap();
    assert_eq!(
        replay_of(&g).count_of("POST", &wire::trash_target(&RemoteMessageId("m1".into()))),
        1
    );
}

#[test]
fn permanent_delete_is_refused_and_no_request_leaves_at_all() {
    // The scope that permits it also authorizes sending. Reaching here means a caller
    // ignored the capability table, and the answer is a refusal rather than a substitute.
    let g = gmail(account());
    let outcome = g
        .apply(&[mutation("m1", Operation::PermanentlyDelete)])
        .unwrap();
    assert_eq!(
        outcome,
        vec![sift_provider::adapter::MutationOutcome::Refused]
    );
    assert!(
        replay_of(&g).performed.is_empty(),
        "a request went out for an operation this provider will not do"
    );
}

#[test]
fn adding_a_tag_that_does_not_exist_yet_creates_it_once() {
    let mut r = modifying();
    r.respond(
        "POST",
        &format!("{}/labels", wire::USER),
        Response {
            status: 200,
            headers: vec![],
            body: br#"{"id":"Label_42","name":"Invoices","type":"user"}"#.to_vec(),
        },
    );
    let g = gmail(r);
    g.apply(&[
        mutation("m1", Operation::AddTag("Invoices".into())),
        mutation("m2", Operation::AddTag("Invoices".into())),
    ])
    .unwrap();
    assert_eq!(
        replay_of(&g).count_of("POST", &format!("{}/labels", wire::USER)),
        1,
        "the label was created twice"
    );
    assert_eq!(
        sent_body(&g, &wire::batch_modify_target())["addLabelIds"],
        serde_json::json!(["Label_42"])
    );
}

#[test]
fn removing_a_tag_the_message_never_had_succeeds_without_asking_the_server() {
    // Exactly-once *observable*: the state the intent wanted is the state that holds.
    let g = gmail(account());
    let outcome = g
        .apply(&[mutation("m1", Operation::RemoveTag("Nonexistent".into()))])
        .unwrap();
    assert_eq!(
        outcome,
        vec![sift_provider::adapter::MutationOutcome::Applied]
    );
    assert_eq!(
        replay_of(&g).count_of("POST", &wire::batch_modify_target()),
        0
    );
}

#[test]
fn a_mutation_against_a_message_that_is_gone_is_settled_rather_than_retried_forever() {
    let mut r = account();
    r.respond(
        "POST",
        &wire::batch_modify_target(),
        Response {
            status: 404,
            headers: vec![],
            body: b"{}".to_vec(),
        },
    );
    let g = gmail(r);
    assert_eq!(
        g.apply(&[mutation("m1", Operation::Archive)]).unwrap(),
        vec![sift_provider::adapter::MutationOutcome::Refused]
    );
}

// ---------------------------------------------------------------------------
// 7. Authorization, and the doorbell that is not there.
// ---------------------------------------------------------------------------

#[test]
fn every_request_carries_the_bearer_and_the_adapter_never_decides_to_refresh_it() {
    let g = gmail(account());
    let _ = g.enumerate_folders().unwrap();
    assert_eq!(
        replay_of(&g).header_of(0, "authorization"),
        Some("Bearer an-access-token")
    );
}

#[test]
fn a_rejected_token_is_reported_rather_than_confused_with_a_denied_grant() {
    // Only the refresh can conclude that the grant is gone. D-88's classifier lives in the
    // credential broker, and an adapter that decided this itself would prompt the user to
    // re-authenticate every time a token expired.
    let mut r = Replay::new();
    r.respond(
        "GET",
        &wire::labels_target(),
        Response {
            status: 401,
            headers: vec![],
            body: br#"{"error":{"code":401,"message":"Invalid Credentials"}}"#.to_vec(),
        },
    );
    let g = gmail(r);
    assert_eq!(g.enumerate_folders(), Err(GmailError::TokenRejected));
}

#[test]
fn a_replaced_token_is_the_one_the_next_request_carries() {
    let g = gmail(account());
    let _ = g.enumerate_folders().unwrap();
    g.set_access_token("a-refreshed-token");
    let _ = g.enumerate_folders().unwrap();
    assert_eq!(
        replay_of(&g).header_of(1, "authorization"),
        Some("Bearer a-refreshed-token")
    );
}

#[test]
fn asking_for_a_doorbell_refuses_rather_than_quietly_succeeding() {
    // A watch that returned Ok and watched nothing is the worst of the three answers: the
    // scheduler would stop polling and the account would silently go stale.
    let g = gmail(account());
    assert!(matches!(
        g.watch(&[inbox()]),
        Err(GmailError::Unsupported(_))
    ));
}

#[test]
fn the_replay_harness_is_not_on_anybodys_data_plan() {
    let g = gmail(account());
    let _ = g.enumerate_folders().unwrap();
    assert_eq!(g.wire_bytes(), (0, 0));
}

/// The transport, for assertions about what was asked rather than what came back.
fn replay_of(g: &Gmail<Replay>) -> core::cell::Ref<'_, Replay> {
    g.transport()
}

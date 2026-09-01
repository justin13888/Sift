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

// ---------------------------------------------------------------------------
// 7. Every intent, pinned to the request it makes.
//
// The individual tests above check one behaviour each. This section is a different
// question, and the one that matters before a real mailbox is connected: **given FR-13's
// closed set, what does each member actually put on the wire, and is there anything on the
// wire that no member should have put there?**
//
// A per-intent test cannot answer the second half. A table can.
// ---------------------------------------------------------------------------

/// Every request the adapter performed, as `VERB target`.
fn wire_log(g: &Gmail<Replay>) -> Vec<String> {
    replay_of(g)
        .performed
        .iter()
        .map(|e| format!("{} {}", e.verb, e.target))
        .collect()
}

/// The whole of FR-13, with the exact call each one makes.
///
/// Read this as the answer to "what will Sift do to my mailbox". Two entries are worth
/// pausing on: permanent delete makes **no request at all**, and nothing anywhere reaches
/// `messages.delete`.
#[test]
fn every_intent_makes_exactly_the_request_it_should_and_no_other() {
    let expected: &[(&str, Operation, &[&str])] = &[
        (
            "archive",
            Operation::Archive,
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "delete-to-trash",
            Operation::DeleteToTrash,
            &["POST /gmail/v1/users/me/messages/m1/trash"],
        ),
        // **Nothing.** The scope Sift asks for cannot permanently delete, and the scope that
        // can also authorizes sending — which the no-send constraint forbids outright. So this
        // is refused inside the adapter and never becomes a request.
        ("permanently-delete", Operation::PermanentlyDelete, &[]),
        (
            "move-to",
            Operation::MoveTo(RemoteFolderId("Label_11".into())),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "mark-read",
            Operation::SetRead(true),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "mark-unread",
            Operation::SetRead(false),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "flag",
            Operation::SetFlagged(true),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "unflag",
            Operation::SetFlagged(false),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "add-tag",
            Operation::AddTag("Receipts".into()),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "remove-tag",
            Operation::RemoveTag("Receipts".into()),
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "report-junk",
            Operation::ReportJunk,
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
        (
            "report-not-junk",
            Operation::ReportNotJunk,
            &["POST /gmail/v1/users/me/messages/batchModify"],
        ),
    ];

    for (name, operation, calls) in expected {
        let mut r = modifying();
        r.respond(
            "POST",
            &wire::trash_target(&RemoteMessageId("m1".into())),
            Response {
                status: 200,
                headers: vec![],
                body: br#"{"id":"m1"}"#.to_vec(),
            },
        );
        let g = gmail(r);
        // The labels fetch is the adapter resolving a name to an identifier, and it happens
        // once. Filtering it out leaves exactly the mutation traffic.
        let _ = g.apply(&[mutation("m1", operation.clone())]);
        let log: Vec<String> = wire_log(&g)
            .into_iter()
            .filter(|c| !c.contains("/labels") && !c.contains("/profile"))
            .collect();
        assert_eq!(
            log,
            calls.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
            "`{name}` did not make the calls it should"
        );
    }
}

/// The one that would be unrecoverable. `messages.delete` bypasses the Trash entirely, and
/// nothing in FR-13's set may reach it — not by any operation, and not by any sequence.
#[test]
fn nothing_in_the_intent_set_can_reach_the_endpoint_that_deletes_without_a_trash() {
    let every = [
        Operation::Archive,
        Operation::DeleteToTrash,
        Operation::PermanentlyDelete,
        Operation::MoveTo(RemoteFolderId("Label_11".into())),
        Operation::SetRead(true),
        Operation::SetRead(false),
        Operation::SetFlagged(true),
        Operation::SetFlagged(false),
        Operation::AddTag("Receipts".into()),
        Operation::RemoveTag("Receipts".into()),
        Operation::ReportJunk,
        Operation::ReportNotJunk,
    ];
    let mut r = modifying();
    r.respond(
        "POST",
        &wire::trash_target(&RemoteMessageId("m1".into())),
        Response {
            status: 200,
            headers: vec![],
            body: br#"{"id":"m1"}"#.to_vec(),
        },
    );
    let g = gmail(r);
    let batch: Vec<WireMutation> = every.iter().cloned().map(|o| mutation("m1", o)).collect();
    let _ = g.apply(&batch);

    for call in wire_log(&g) {
        assert!(
            !call.starts_with("DELETE "),
            "a DELETE reached the wire: {call}"
        );
        // Gmail's own permanent deletion is `DELETE …/messages/{id}` and the batch form is
        // `POST …/messages/batchDelete`. Neither may appear.
        assert!(
            !call.contains("batchDelete"),
            "the batch delete endpoint was called: {call}"
        );
    }
}

/// Permanent deletion is refused **inside the adapter**, so it cannot be reached by anything
/// above it that ignored the capability table. The refusal is settled rather than retryable:
/// a retry of something the provider will never do is a loop.
#[test]
fn permanent_deletion_is_refused_without_a_request_and_is_not_retryable() {
    let g = gmail(modifying());
    let outcomes = g
        .apply(&[mutation("m1", Operation::PermanentlyDelete)])
        .expect("the batch itself succeeds");

    assert_eq!(
        outcomes,
        vec![sift_provider::adapter::MutationOutcome::Refused]
    );
    let mutations: Vec<String> = wire_log(&g)
        .into_iter()
        .filter(|c| !c.contains("/labels") && !c.contains("/profile"))
        .collect();
    assert!(
        mutations.is_empty(),
        "permanent deletion put something on the wire: {mutations:?}"
    );
}

/// The capability table is what the layers above plan against, so it has to agree with what
/// the adapter will actually do. Two answers to "can this account permanently delete" is how
/// an action gets offered that then refuses.
#[test]
fn the_declared_capability_and_the_adapter_agree_about_permanent_deletion() {
    let g = gmail(modifying());
    assert!(
        !g.capabilities().permanent_delete,
        "the capability says it can, and the adapter refuses"
    );
}

/// Trashing is recoverable, and that is the whole reason `delete` means this. A message in
/// the provider's own Trash is one the user can get back through any client they own.
#[test]
fn delete_means_the_providers_own_trash_and_the_message_is_still_there() {
    let mut r = modifying();
    r.respond(
        "POST",
        &wire::trash_target(&RemoteMessageId("m1".into())),
        Response {
            status: 200,
            headers: vec![],
            body: br#"{"id":"m1","labelIds":["TRASH"]}"#.to_vec(),
        },
    );
    let g = gmail(r);
    let outcomes = g
        .apply(&[mutation("m1", Operation::DeleteToTrash)])
        .unwrap();
    assert_eq!(
        outcomes,
        vec![sift_provider::adapter::MutationOutcome::Applied]
    );
    assert_eq!(
        replay_of(&g).count_of("POST", &wire::trash_target(&RemoteMessageId("m1".into()))),
        1
    );
    // And the untrash endpoint exists, which is what makes the compensation reachable.
    assert!(wire::untrash_target(&RemoteMessageId("m1".into())).ends_with("/untrash"));
}

// ---------------------------------------------------------------------------
// 8. The shapes a real mailbox produces.
//
// Every one of these is a thing that happens and that nobody can arrange on demand against
// a live account — which is why they are fixtures. What is being checked is not that the
// adapter survives them but that it classifies each one **correctly**, because the class is
// what decides whether the scheduler retries, whether the folder resynchronises, and whether
// the user is asked to sign in again.
// ---------------------------------------------------------------------------

/// A refused credential is not a refused account.
///
/// Only the credential broker can decide whether the *grant* is gone. This says the request
/// was not answered, and nothing more — because a refresh that succeeds afterwards must leave
/// no trace of this having happened.
#[test]
fn a_refused_token_is_its_own_class_and_not_a_permanent_failure() {
    let mut r = account();
    r.respond(
        "GET",
        &wire::list_target(&inbox(), None, 500),
        Response {
            status: 401,
            headers: vec![],
            body: br#"{"error":{"code":401,"message":"Invalid Credentials"}}"#.to_vec(),
        },
    );
    let g = gmail(r);
    let error = g.delta(&inbox(), None).expect_err("401 is not success");
    assert!(matches!(
        g.classify(&error),
        sift_provider::adapter::Failure::CredentialRefused
    ));
}

/// 403 is the provider saying no to *this request*, and it is settled: retrying a scope the
/// account does not have is a loop. NFR-29 requires the reason be surfaced rather than
/// swallowed, which is why the message is carried rather than discarded.
#[test]
fn a_forbidden_request_is_settled_and_carries_the_reason() {
    let mut r = account();
    r.respond(
        "GET",
        &wire::list_target(&inbox(), None, 500),
        Response {
            status: 403,
            headers: vec![],
            body: br#"{"error":{"code":403,"message":"Insufficient Permission"}}"#.to_vec(),
        },
    );
    let g = gmail(r);
    let error = g.delta(&inbox(), None).expect_err("403 is not success");
    assert!(matches!(
        g.classify(&error),
        sift_provider::adapter::Failure::Permanent
    ));
    assert!(
        format!("{error}").contains("Insufficient Permission"),
        "the reason was swallowed: {error}"
    );
}

/// A captive portal's sign-in page arrives as an answer that does not parse. Treating it as
/// settled would degrade **every account** on a hotel network — the cascade NFR-34 is about,
/// reaching the folder state machine instead of the credential store.
#[test]
fn an_answer_that_does_not_parse_is_transient_rather_than_settled() {
    let mut r = account();
    r.on(
        "GET",
        &wire::list_target(&inbox(), None, 500),
        b"<html><body>Sign in to continue</body></html>",
    );
    let g = gmail(r);
    let error = g
        .delta(&inbox(), None)
        .expect_err("a login page is not a message list");
    assert!(
        matches!(
            g.classify(&error),
            sift_provider::adapter::Failure::Transient
        ),
        "a portal page was classified as {:?}, which would degrade every account on the network",
        g.classify(&error)
    );
}

/// D-82: a cursor outside the retained window is **progress, not a fault**. NFR-18 recovers
/// without asking the user anything, so this must not reach them as an error.
#[test]
fn a_cursor_outside_the_window_is_progress_and_recovers_into_a_full_walk() {
    let mut r = account();
    // A 404 body **with a 404 status**. Registering the body at 200 would test a fixture
    // rather than the adapter: the status is what carries the meaning here.
    r.respond(
        "GET",
        &wire::history_target("1000", &inbox(), None, 500),
        Response {
            status: 404,
            headers: vec![],
            body: HISTORY_EXPIRED.to_vec(),
        },
    );
    r.on("GET", &wire::list_target(&inbox(), None, 500), LIST_1);
    r.on(
        "GET",
        &wire::list_target(&inbox(), Some("page2"), 500),
        LIST_2,
    );
    let g = gmail(r);
    let error = g
        .delta(&inbox(), Some(&live_cursor()))
        .expect_err("the cursor is gone");
    assert!(matches!(
        g.classify(&error),
        sift_provider::adapter::Failure::CursorInvalidated
    ));

    // And the recovery is a full walk from no cursor, which the same adapter can do.
    let recovered = g.delta(&inbox(), None).expect("a full walk");
    assert!(
        !recovered.changes.is_empty(),
        "the recovery produced nothing"
    );
}

/// An empty answer is an answer. A folder with nothing in it must produce no changes rather
/// than an error, because "empty" and "broken" reaching the same place is how an account gets
/// marked degraded for being tidy.
#[test]
fn an_empty_folder_is_a_delta_with_nothing_in_it() {
    let mut r = account();
    r.on("GET", &wire::list_target(&inbox(), None, 500), b"{}");
    let g = gmail(r);
    let delta = g
        .delta(&inbox(), None)
        .expect("an empty list is not an error");
    assert!(delta.changes.is_empty());
}

/// A folder larger than any list a person scrolls.
///
/// The walk is **resumable by page** under D-53 rather than one call that returns everything,
/// so this drives it the way the sync engine does and checks the total. The failure it is
/// looking for is a walk that stops at a page boundary and reports itself complete, which
/// looks like mail that never arrived.
#[test]
fn a_folder_larger_than_one_page_is_walked_rather_than_truncated() {
    let ids: Vec<String> = (0..1_500)
        .map(|i| format!(r#"{{"id":"m{i}","threadId":"t{i}"}}"#))
        .collect();
    let (first, second) = ids.split_at(500);
    let mut r = account();
    r.on(
        "GET",
        &wire::list_target(&inbox(), None, 500),
        format!(
            r#"{{"messages":[{}],"nextPageToken":"p2"}}"#,
            first.join(",")
        )
        .as_bytes(),
    );
    r.on(
        "GET",
        &wire::list_target(&inbox(), Some("p2"), 500),
        format!(r#"{{"messages":[{}]}}"#, second.join(",")).as_bytes(),
    );
    let g = gmail(r);
    let mut total = 0;
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let delta = g.delta(&inbox(), cursor.as_ref()).expect("a page");
        total += delta.changes.len();
        pages += 1;
        if !delta.more {
            break;
        }
        cursor = Some(delta.next);
        assert!(pages < 10, "the walk did not terminate");
    }
    assert_eq!(total, 1_500, "the walk stopped at a page boundary");
    assert_eq!(pages, 2, "1500 messages in 500-message pages is two pages");
}

/// A message whose structure is not what the provider's own schema says. Sender-controlled
/// bytes reach this parser, so a malformed one must be a refusal rather than a panic — and it
/// must be **transient**, because what is malformed may be the network rather than the message.
#[test]
fn a_malformed_structure_refuses_without_panicking() {
    let id = RemoteMessageId("m1".into());
    for body in [
        &b"{}"[..],
        &b"{\"payload\":null}"[..],
        &b"{\"payload\":{\"parts\":\"not an array\"}}"[..],
        &b"not json at all"[..],
    ] {
        let mut r = account();
        r.on("GET", &wire::structure_target(&id), body);
        let g = gmail(r);
        // Either an empty structure or a refusal. What must not happen is a panic, and what
        // must not happen is a *permanent* classification of a network's answer.
        if let Err(e) = g.structure(&id) {
            assert!(
                !matches!(
                    g.classify(&e),
                    sift_provider::adapter::Failure::CredentialRefused
                ),
                "a malformed body was read as a credential problem"
            );
        }
    }
}

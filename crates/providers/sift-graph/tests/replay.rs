//! D-65's fixture-and-replay corpus for this adapter.
//!
//! Every behaviour here is one nobody can arrange against a real provider on demand: a delta
//! link that has expired, a throttle on one element of a batch, a batch answered out of order,
//! a message that vanishes between the delta and the fetch — and the one this adapter exists
//! to get right, **a move whose rejoin is ambiguous**.
//!
//! **The fixtures carry no real mail.** NFR-22 reaches the test tree: every address is under
//! `example.invalid`, every subject and preview is invented, and every identifier is shaped
//! like the provider's without being one.

use sift_graph::adapter::Position;
use sift_graph::{Departed, Graph, MoveOutcome, rejoin, wire};
use sift_provider::adapter::{
    Adapter, Change, Cursor, Envelope, Failure, MutationOutcome, Operation, Provenance,
    RemoteFolderId, RemoteMessageId, SpecialUse, WireMutation,
};
use sift_provider::transport::{Replay, Response, TransportError};

const FOLDERS_1: &[u8] = include_bytes!("../fixtures/folders-page-1.json");
const FOLDERS_2: &[u8] = include_bytes!("../fixtures/folders-page-2.json");
const CHILD_FOLDERS: &[u8] = include_bytes!("../fixtures/child-folders.json");
const WELL_KNOWN: &[u8] = include_bytes!("../fixtures/well-known.json");
const DELTA_1: &[u8] = include_bytes!("../fixtures/delta-backfill-1.json");
const DELTA_2: &[u8] = include_bytes!("../fixtures/delta-backfill-2.json");
const DELTA_QUIET: &[u8] = include_bytes!("../fixtures/delta-quiet.json");
const DELTA_LIVE: &[u8] = include_bytes!("../fixtures/delta-live.json");
const DELTA_EXPIRED: &[u8] = include_bytes!("../fixtures/delta-expired.json");
const ENVELOPES_1: &[u8] = include_bytes!("../fixtures/envelopes-backfill-1.json");
const ENVELOPES_2: &[u8] = include_bytes!("../fixtures/envelopes-backfill-2.json");
const ENVELOPES_LIVE: &[u8] = include_bytes!("../fixtures/envelopes-live.json");
const ENVELOPES_BEFORE_MOVE: &[u8] = include_bytes!("../fixtures/envelopes-before-move.json");
const ENVELOPES_AFTER_MOVE: &[u8] = include_bytes!("../fixtures/envelopes-after-move.json");
const INBOX_MOVED_OUT: &[u8] = include_bytes!("../fixtures/delta-inbox-moved-out.json");
const ARCHIVE_MOVED_IN: &[u8] = include_bytes!("../fixtures/delta-archive-moved-in.json");
const ATTACHMENTS: &[u8] = include_bytes!("../fixtures/attachments.json");
const BODY: &[u8] = include_bytes!("../fixtures/body.json");

const BATCH: &str = "/v1.0/$batch";
const DELTA_BASE: &str = "/v1.0/me/mailFolders/AAMk-inbox/messages/delta";

fn inbox() -> RemoteFolderId {
    RemoteFolderId("AAMk-inbox".into())
}

fn archive() -> RemoteFolderId {
    RemoteFolderId("AAMk-archive".into())
}

fn id(s: &str) -> RemoteMessageId {
    RemoteMessageId(s.into())
}

fn graph(replay: Replay) -> Graph<Replay> {
    Graph::new(replay, "an-access-token")
}

fn start(folder: &RemoteFolderId) -> String {
    wire::rooted(&wire::delta_target(folder))
}

fn status(code: u16, body: &[u8]) -> Response {
    Response {
        status: code,
        headers: vec![],
        body: body.to_vec(),
    }
}

/// A batch answer built inline, for the shapes too small to be worth a file.
fn batch_answer(items: &[(usize, u16, serde_json::Value)]) -> Vec<u8> {
    let responses: Vec<serde_json::Value> = items
        .iter()
        .map(|(index, code, body)| {
            serde_json::json!({ "id": index.to_string(), "status": code, "body": body })
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({ "responses": responses })).unwrap()
}

/// The requests inside every batch the adapter sent, as `(method, url, body)`.
fn batches_sent(g: &Graph<Replay>) -> Vec<Vec<(String, String, serde_json::Value)>> {
    let r = g.transport();
    r.performed
        .iter()
        .zip(r.bodies.iter())
        .filter(|(e, _)| e.verb == "POST" && e.target == BATCH)
        .map(|(_, body)| {
            let value: serde_json::Value = serde_json::from_slice(body).unwrap();
            value["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|q| {
                    (
                        q["method"].as_str().unwrap().to_owned(),
                        q["url"].as_str().unwrap().to_owned(),
                        q.get("body").cloned().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .collect()
}

/// Every request the adapter performed, as `VERB target`.
fn wire_log(g: &Graph<Replay>) -> Vec<String> {
    g.transport()
        .performed
        .iter()
        .map(|e| format!("{} {}", e.verb, e.target))
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Folders, and FR-5's special use through well-known names.
// ---------------------------------------------------------------------------

fn with_folders(r: &mut Replay) {
    r.on("GET", &wire::rooted(&wire::folders_target()), FOLDERS_1);
    r.on(
        "GET",
        "/v1.0/me/mailFolders?$select=id,displayName,parentFolderId,childFolderCount&$top=100&$skip=4",
        FOLDERS_2,
    );
    r.on(
        "GET",
        &wire::rooted(&wire::child_folders_target("AAMk-projects")),
        CHILD_FOLDERS,
    );
}

#[test]
fn every_folder_is_walked_across_pages_and_down_into_children() {
    let mut r = Replay::new();
    with_folders(&mut r);
    r.on("POST", BATCH, WELL_KNOWN);
    let g = graph(r);
    let folders = g.enumerate_folders().unwrap();
    let ids: Vec<&str> = folders.iter().map(|f| f.id.0.as_str()).collect();
    assert_eq!(
        ids,
        [
            "AAMk-inbox",
            "AAMk-archive",
            "AAMk-sent",
            "AAMk-deleted",
            "AAMk-junk",
            "AAMk-drafts",
            "AAMk-projects",
            "AAMk-user-archive",
            "AAMk-projects-2026",
        ]
    );
}

#[test]
fn special_use_resolves_through_well_known_names_and_never_through_a_display_name() {
    let mut r = Replay::new();
    with_folders(&mut r);
    r.on("POST", BATCH, WELL_KNOWN);
    let g = graph(r);
    let folders = g.enumerate_folders().unwrap();
    let of = |id: &str| {
        folders
            .iter()
            .find(|f| f.id.0 == id)
            .and_then(|f| f.special_use)
    };
    // Localized names, resolved without a locale table.
    assert_eq!(of("AAMk-inbox"), Some(SpecialUse::Inbox));
    assert_eq!(of("AAMk-archive"), Some(SpecialUse::Archive));
    assert_eq!(of("AAMk-deleted"), Some(SpecialUse::Trash));
    assert_eq!(of("AAMk-junk"), Some(SpecialUse::Spam));
    assert_eq!(of("AAMk-sent"), Some(SpecialUse::Sent));
    assert_eq!(of("AAMk-drafts"), Some(SpecialUse::Drafts));
    // A user's own folder named "Archive" is not the archive.
    assert_eq!(of("AAMk-user-archive"), None);
    assert_eq!(of("AAMk-projects-2026"), None);

    // One batch, one element per well-known name, answered out of order and still matched.
    let sent = batches_sent(&g);
    assert_eq!(sent.len(), 1);
    let urls: Vec<&str> = sent[0].iter().map(|(_, u, _)| u.as_str()).collect();
    assert_eq!(
        urls,
        [
            "/me/mailFolders/inbox?$select=id",
            "/me/mailFolders/archive?$select=id",
            "/me/mailFolders/sentitems?$select=id",
            "/me/mailFolders/deleteditems?$select=id",
            "/me/mailFolders/junkemail?$select=id",
            "/me/mailFolders/drafts?$select=id",
        ]
    );
}

#[test]
fn a_well_known_name_the_account_lacks_resolves_to_nothing_rather_than_a_guess() {
    let mut r = Replay::new();
    with_folders(&mut r);
    let not_found = serde_json::json!({"error":{"code":"ErrorItemNotFound","message":"x"}});
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (0, 200, serde_json::json!({"id":"AAMk-inbox"})),
            (1, 404, not_found),
            (2, 200, serde_json::json!({"id":"AAMk-sent"})),
            (3, 200, serde_json::json!({"id":"AAMk-deleted"})),
            (4, 200, serde_json::json!({"id":"AAMk-junk"})),
            (5, 200, serde_json::json!({"id":"AAMk-drafts"})),
        ]),
    );
    let g = graph(r);
    let folders = g.enumerate_folders().unwrap();
    assert!(
        folders
            .iter()
            .all(|f| f.special_use != Some(SpecialUse::Archive)),
        "an archive was guessed at"
    );
}

#[test]
fn a_paging_link_that_loops_back_is_not_followed_forever() {
    let mut r = Replay::new();
    let looping = serde_json::to_vec(&serde_json::json!({
        "value": [{"id":"AAMk-inbox","displayName":"Inbox","childFolderCount":0}],
        "@odata.nextLink": format!("https://graph.microsoft.com{}", wire::rooted(&wire::folders_target())),
    }))
    .unwrap();
    r.on("GET", &wire::rooted(&wire::folders_target()), &looping);
    r.on("POST", BATCH, WELL_KNOWN);
    let g = graph(r);
    assert_eq!(g.enumerate_folders().unwrap().len(), 1);
    assert_eq!(
        g.transport()
            .count_of("GET", &wire::rooted(&wire::folders_target())),
        1
    );
}

// ---------------------------------------------------------------------------
// 2. Backfill: the first round, with the link as the cursor. D-82.
// ---------------------------------------------------------------------------

fn backfilling() -> Replay {
    let mut r = Replay::new();
    r.on("GET", &start(&inbox()), DELTA_1);
    r.on("GET", &format!("{DELTA_BASE}?$skiptoken=page2"), DELTA_2);
    r
}

#[test]
fn the_first_request_is_the_round_itself_and_asks_for_identifiers_only() {
    let g = graph(backfilling());
    let _ = g.delta(&inbox(), None).unwrap();
    let log = wire_log(&g);
    assert_eq!(
        log,
        ["GET /v1.0/me/mailFolders/AAMk-inbox/messages/delta?$select=id"]
    );
    // L-26 as the provider's page ceiling, stated on the request.
    assert_eq!(
        g.transport().header_of(0, "prefer"),
        Some("odata.maxpagesize=500")
    );
}

#[test]
fn a_backfill_walks_its_pages_discovers_and_then_goes_live_on_the_delta_link() {
    let g = graph(backfilling());
    let first = g.delta(&inbox(), None).unwrap();
    assert!(first.more);
    assert_eq!(first.changes.len(), 2);
    for change in &first.changes {
        // FR-23's new mail is delivered mail. A first round that delivered would announce the
        // user's whole mailbox the day they added the account.
        assert!(matches!(
            change,
            Change::Present {
                provenance: Provenance::Discovered,
                ..
            }
        ));
    }
    let second = g.delta(&inbox(), Some(&first.next)).unwrap();
    assert!(!second.more);
    assert_eq!(second.changes.len(), 1);
    let position = Position::decode(&second.next).unwrap();
    assert!(position.live);
    assert_eq!(position.link, format!("{DELTA_BASE}?$deltatoken=round1"));
}

#[test]
fn resuming_an_interrupted_backfill_asks_for_the_page_it_stopped_on() {
    let g = graph(backfilling());
    let first = g.delta(&inbox(), None).unwrap();
    let stored = Cursor(first.next.0.clone());
    // A restart: the adapter is rebuilt and knows only what was durably stored.
    let g = graph(backfilling());
    let _ = g.delta(&inbox(), Some(&stored)).unwrap();
    assert_eq!(
        wire_log(&g),
        [format!("GET {DELTA_BASE}?$skiptoken=page2")],
        "a resume restarted the round and walked the mailbox again"
    );
}

#[test]
fn a_folder_larger_than_one_page_is_walked_rather_than_truncated() {
    let items = |from: usize, to: usize| -> Vec<serde_json::Value> {
        (from..to)
            .map(|i| serde_json::json!({ "id": format!("m{i}") }))
            .collect()
    };
    let mut r = Replay::new();
    r.on(
        "GET",
        &start(&inbox()),
        &serde_json::to_vec(&serde_json::json!({
            "value": items(0, 500),
            "@odata.nextLink": format!("https://graph.microsoft.com{DELTA_BASE}?$skiptoken=p2"),
        }))
        .unwrap(),
    );
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$skiptoken=p2"),
        &serde_json::to_vec(&serde_json::json!({
            "value": items(500, 1_500),
            "@odata.deltaLink": format!("https://graph.microsoft.com{DELTA_BASE}?$deltatoken=d"),
        }))
        .unwrap(),
    );
    let g = graph(r);
    let (mut total, mut pages, mut cursor) = (0, 0, None);
    loop {
        let delta = g.delta(&inbox(), cursor.as_ref()).unwrap();
        total += delta.changes.len();
        pages += 1;
        cursor = Some(delta.next);
        if !delta.more {
            break;
        }
        assert!(pages < 10, "the walk did not terminate");
    }
    assert_eq!(total, 1_500, "the walk stopped at a page boundary");
    assert_eq!(pages, 2);
}

// ---------------------------------------------------------------------------
// 3. Live: the delta is the only path that applies change.
// ---------------------------------------------------------------------------

fn live_cursor() -> Cursor {
    Position {
        live: true,
        link: format!("{DELTA_BASE}?$deltatoken=round1"),
    }
    .encode()
}

#[test]
fn a_live_round_delivers_arrivals_refetches_changes_and_removes_departures() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        DELTA_LIVE,
    );
    let g = graph(r);
    let delta = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    assert_eq!(
        delta.changes,
        vec![
            Change::Present {
                id: id("AAMk-m4"),
                provenance: Provenance::Delivered
            },
            // The provider does not say whether this is an arrival or a change to a message
            // already held, so both are reported and the envelope is fetched either way.
            Change::FlagsChanged { id: id("AAMk-m4") },
            Change::Removed { id: id("AAMk-m1") },
        ]
    );
    assert!(!delta.more);
    assert_eq!(
        Position::decode(&delta.next).unwrap().link,
        format!("{DELTA_BASE}?$deltatoken=round2")
    );
}

#[test]
fn a_quiet_round_changes_nothing_and_keeps_its_place() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        DELTA_QUIET,
    );
    let g = graph(r);
    let delta = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    assert!(delta.changes.is_empty());
    assert_eq!(Position::decode(&delta.next).unwrap(), {
        Position {
            live: true,
            link: format!("{DELTA_BASE}?$deltatoken=round1"),
        }
    });
}

#[test]
fn an_expired_delta_link_is_a_recovery_rather_than_a_broken_account() {
    for code in [410, 400] {
        let mut r = Replay::new();
        // The status carries the meaning. Registering the body at 200 would test a fixture
        // rather than the adapter.
        r.respond(
            "GET",
            &format!("{DELTA_BASE}?$deltatoken=round1"),
            status(code, DELTA_EXPIRED),
        );
        r.on("GET", &start(&inbox()), DELTA_1);
        let g = graph(r);
        let error = g
            .delta(&inbox(), Some(&live_cursor()))
            .expect_err("the link is gone");
        assert_eq!(g.classify(&error), Failure::CursorInvalidated, "{code}");
        // And the recovery is a new first round from no cursor, which the same adapter can do.
        let recovered = g.delta(&inbox(), None).unwrap();
        assert!(!recovered.changes.is_empty());
    }
}

#[test]
fn a_folder_that_is_gone_is_not_mistaken_for_an_expired_link() {
    let mut r = Replay::new();
    r.respond(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        status(
            404,
            br#"{"error":{"code":"ErrorItemNotFound","message":"The specified object was not found in the store."}}"#,
        ),
    );
    let g = graph(r);
    let error = g.delta(&inbox(), Some(&live_cursor())).unwrap_err();
    // A resync of a folder that no longer exists would be a loop. D-83 retires it instead.
    assert_eq!(g.classify(&error), Failure::Permanent);
    assert!(format!("{error}").contains("not found in the store"));
}

#[test]
fn a_throttle_reaches_the_scheduler_carrying_the_providers_own_number() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        DELTA_QUIET,
    );
    r.fail_nth(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        0,
        TransportError::Throttled {
            retry_after_millis: 30_000,
        },
    );
    let g = graph(r);
    let error = g.delta(&inbox(), Some(&live_cursor())).unwrap_err();
    assert_eq!(
        g.classify(&error),
        Failure::Throttled {
            retry_after_millis: 30_000
        }
    );
    // The adapter did not wait and try again: the next attempt is the scheduler's.
    assert_eq!(g.transport().performed.len(), 1);
}

#[test]
fn a_captive_portals_page_is_transient_rather_than_settled() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &start(&inbox()),
        b"<html><body>Sign in to continue</body></html>",
    );
    let g = graph(r);
    let error = g.delta(&inbox(), None).unwrap_err();
    assert_eq!(g.classify(&error), Failure::Transient);
}

#[test]
fn a_link_to_another_host_is_never_followed_and_the_bearer_never_leaves() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &start(&inbox()),
        br#"{"value":[{"id":"m"}],"@odata.nextLink":"https://collector.example.invalid/v1.0/steal"}"#,
    );
    let g = graph(r);
    assert!(g.delta(&inbox(), None).is_err());
    // And a stored cursor pointing anywhere else is refused before a request is made.
    let hostile =
        Cursor(br#"{"phase":"live","link":"https://collector.example.invalid/x"}"#.to_vec());
    assert!(g.delta(&inbox(), Some(&hostile)).is_err());
    assert_eq!(g.transport().performed.len(), 1);
}

#[test]
fn a_stored_cursor_this_adapter_did_not_write_is_refused_rather_than_guessed_at() {
    let g = graph(Replay::new());
    let error = g
        .delta(&inbox(), Some(&Cursor(b"12345".to_vec())))
        .unwrap_err();
    assert!(matches!(
        error,
        sift_graph::GraphError::Refusal(wire::Refusal::Malformed(_))
    ));
    assert!(g.transport().performed.is_empty());
}

// ---------------------------------------------------------------------------
// 4. Envelopes, through the batch endpoint, fields named.
// ---------------------------------------------------------------------------

#[test]
fn an_envelope_fetch_is_one_batch_of_named_fields_and_never_a_whole_message() {
    let mut r = Replay::new();
    r.on("POST", BATCH, ENVELOPES_1);
    let g = graph(r);
    let envelopes = g.fetch_envelopes(&[id("AAMk-m1"), id("AAMk-m2")]).unwrap();
    assert_eq!(envelopes.len(), 2);
    let sent = batches_sent(&g);
    assert_eq!(sent.len(), 1);
    for (method, url, _) in &sent[0] {
        assert_eq!(method, "GET");
        assert!(url.contains("$select="), "{url}");
        assert!(!url.contains("$expand"), "{url}");
        assert!(!url.contains("$value"), "{url}");
        // The provider's own preview is a field of the envelope; the body is not.
        let fields = sift_graph::schema::selected(url);
        for whole in [
            "body",
            "uniqueBody",
            "internetMessageHeaders",
            "attachments",
        ] {
            assert!(
                !fields.iter().any(|f| f == whole),
                "an envelope fetch asked for `{whole}`: {url}"
            );
        }
    }
}

#[test]
fn a_batch_answered_out_of_order_still_lands_on_the_right_messages() {
    let mut r = Replay::new();
    r.on("POST", BATCH, ENVELOPES_1);
    let g = graph(r);
    let envelopes = g.fetch_envelopes(&[id("AAMk-m1"), id("AAMk-m2")]).unwrap();
    let m2 = envelopes.iter().find(|e| e.id == id("AAMk-m2")).unwrap();
    assert!(m2.flagged && !m2.read);
    assert_eq!(m2.tags, vec!["Receipts".to_owned()]);
    assert_eq!(m2.thread_id.as_deref(), Some("conv-1"));
    let m1 = envelopes.iter().find(|e| e.id == id("AAMk-m1")).unwrap();
    assert!(m1.read && !m1.flagged);
    // D-55's sort key is the server's time, never the sender's.
    assert!(m1.received_at_millis > m1.origination_date_millis.unwrap());
}

#[test]
fn a_message_that_vanished_between_the_delta_and_the_fetch_is_skipped_not_fatal() {
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (
                0,
                404,
                serde_json::json!({"error":{"code":"ErrorItemNotFound","message":"x"}}),
            ),
            (1, 200, serde_json::json!({"id":"AAMk-m2","subject":"s"})),
        ]),
    );
    let g = graph(r);
    let envelopes = g.fetch_envelopes(&[id("AAMk-m1"), id("AAMk-m2")]).unwrap();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].id, id("AAMk-m2"));
}

#[test]
fn a_throttled_element_fails_the_page_with_its_own_delay_rather_than_leaving_a_hole() {
    let mut r = Replay::new();
    let throttled = serde_json::to_vec(&serde_json::json!({"responses":[
        {"id":"0","status":200,"body":{"id":"AAMk-m1"}},
        {"id":"1","status":429,"headers":{"Retry-After":"12"},"body":{}},
    ]}))
    .unwrap();
    r.on("POST", BATCH, &throttled);
    let g = graph(r);
    let error = g
        .fetch_envelopes(&[id("AAMk-m1"), id("AAMk-m2")])
        .unwrap_err();
    assert_eq!(
        g.classify(&error),
        Failure::Throttled {
            retry_after_millis: 12_000
        }
    );
}

#[test]
fn an_element_left_unanswered_or_failing_on_the_server_fails_the_page_as_transient() {
    let mut r = Replay::new();
    // Element 1 is not answered at all.
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[(0, 200, serde_json::json!({"id":"AAMk-m1"}))]),
    );
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (0, 200, serde_json::json!({"id":"AAMk-m1"})),
            (1, 503, serde_json::json!({})),
        ]),
    );
    let g = graph(r);
    for _ in 0..2 {
        let error = g
            .fetch_envelopes(&[id("AAMk-m1"), id("AAMk-m2")])
            .unwrap_err();
        assert_eq!(g.classify(&error), Failure::Transient);
    }
}

#[test]
fn an_envelope_nobody_asked_for_is_not_believed() {
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[(0, 200, serde_json::json!({"id":"AAMk-someone-else"}))]),
    );
    let g = graph(r);
    assert!(g.fetch_envelopes(&[id("AAMk-m1")]).unwrap().is_empty());
}

#[test]
fn more_envelopes_than_one_batch_holds_are_asked_for_in_several() {
    let ids: Vec<RemoteMessageId> = (0..25).map(|i| id(&format!("m{i}"))).collect();
    let first: Vec<(usize, u16, serde_json::Value)> = (0..20)
        .map(|i| (i, 200, serde_json::json!({ "id": format!("m{i}") })))
        .collect();
    let second: Vec<(usize, u16, serde_json::Value)> = (0..5)
        .map(|i| (i, 200, serde_json::json!({ "id": format!("m{}", i + 20) })))
        .collect();
    let mut r = Replay::new();
    r.on("POST", BATCH, &batch_answer(&first));
    r.on("POST", BATCH, &batch_answer(&second));
    let g = graph(r);
    let envelopes = g.fetch_envelopes(&ids).unwrap();
    assert_eq!(envelopes.len(), 25);
    let sizes: Vec<usize> = batches_sent(&g).iter().map(Vec::len).collect();
    assert_eq!(sizes, [20, 5], "a batch exceeded the provider's cap");
}

#[test]
fn fetching_nothing_asks_for_nothing() {
    let g = graph(Replay::new());
    assert!(g.fetch_envelopes(&[]).unwrap().is_empty());
    assert!(g.transport().performed.is_empty());
}

// ---------------------------------------------------------------------------
// 5. Structure first, then only the part decided for display.
// ---------------------------------------------------------------------------

#[test]
fn a_forty_megabyte_attachment_costs_one_listing_until_it_is_asked_for() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &wire::rooted(&wire::attachments_target(&id("AAMk-m1"))),
        ATTACHMENTS,
    );
    let g = graph(r);
    let parts = g.structure(&id("AAMk-m1")).unwrap();
    let pdf = parts
        .iter()
        .find(|p| p.id == "attachment:AAMk-att-1")
        .unwrap();
    assert_eq!(pdf.size, 41_943_040);
    assert_eq!(pdf.filename.as_deref(), Some("report.pdf"));
    assert!(!pdf.is_body());
    // Both body representations are described, so stage 2 chooses between them without a
    // byte of either having been fetched.
    let bodies: Vec<(&str, &str)> = parts
        .iter()
        .filter(|p| p.is_body())
        .map(|p| (p.id.as_str(), p.media_type.as_str()))
        .collect();
    assert_eq!(
        bodies,
        [
            (wire::BODY_HTML, "text/html"),
            (wire::BODY_TEXT, "text/plain")
        ]
    );
    // An inline image is an attachment with a name, never mistaken for body content.
    assert!(
        !parts
            .iter()
            .find(|p| p.id == "attachment:AAMk-att-2")
            .unwrap()
            .is_body()
    );
    assert_eq!(wire_log(&g).len(), 1);
    assert!(!wire_log(&g)[0].contains("$value"));
}

#[test]
fn the_body_is_fetched_alone_in_the_representation_stage_2_chose() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &wire::rooted(&wire::body_target(&id("AAMk-m1"))),
        BODY,
    );
    let g = graph(r);
    let bytes = g.fetch_part(&id("AAMk-m1"), wire::BODY_HTML).unwrap();
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("A scrubbed body.")
    );
    assert_eq!(
        g.transport().header_of(0, "prefer"),
        Some("outlook.body-content-type=\"html\"")
    );
    let _ = g.fetch_part(&id("AAMk-m1"), wire::BODY_TEXT).unwrap();
    assert_eq!(
        g.transport().header_of(1, "prefer"),
        Some("outlook.body-content-type=\"text\"")
    );
}

#[test]
fn a_body_past_l1_is_refused_rather_than_handed_to_the_sanitizer() {
    let limit = usize::try_from(sift_foundation::limits::L1_BODY_PART_BYTES).unwrap();
    let at = |len: usize| {
        serde_json::to_vec(&serde_json::json!({
            "body": { "contentType": "html", "content": "x".repeat(len) }
        }))
        .unwrap()
    };
    let target = wire::rooted(&wire::body_target(&id("AAMk-m1")));
    let mut r = Replay::new();
    r.on("GET", &target, &at(limit));
    r.on("GET", &target, &at(limit + 1));
    let g = graph(r);
    // Exactly at the bound is within it.
    assert_eq!(
        g.fetch_part(&id("AAMk-m1"), wire::BODY_HTML).unwrap().len(),
        limit
    );
    // One byte past it is refused whole, and refused as settled: asking again returns the
    // same body.
    let error = g.fetch_part(&id("AAMk-m1"), wire::BODY_HTML).unwrap_err();
    assert_eq!(
        error,
        sift_graph::adapter::GraphError::Transport(TransportError::TooLarge {
            limit_bytes: sift_foundation::limits::L1_BODY_PART_BYTES
        })
    );
    assert_eq!(g.classify(&error), Failure::Permanent);
}

#[test]
fn a_message_with_more_parts_than_l2_allows_is_refused_rather_than_truncated() {
    let bound = usize::try_from(sift_foundation::limits::L2_MIME_PARTS).unwrap();
    // The two body representations count toward the bound, so this many attachments fills
    // it exactly, and one more exceeds it.
    let listing = |count: usize| {
        let value: Vec<serde_json::Value> = (0..count)
            .map(|i| {
                serde_json::json!({
                    "@odata.type": "#microsoft.graph.fileAttachment",
                    "id": format!("AAMk-att-{i}"),
                    "name": format!("{i}.bin"),
                    "contentType": "application/octet-stream",
                    "size": 1,
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap()
    };
    let mut r = Replay::new();
    r.on(
        "GET",
        &wire::rooted(&wire::attachments_target(&id("AAMk-m1"))),
        &listing(bound - 2),
    );
    r.on(
        "GET",
        &wire::rooted(&wire::attachments_target(&id("AAMk-m2"))),
        &listing(bound - 1),
    );
    let g = graph(r);
    assert_eq!(g.structure(&id("AAMk-m1")).unwrap().len(), bound);
    let error = g.structure(&id("AAMk-m2")).unwrap_err();
    assert!(
        matches!(
            error,
            sift_graph::adapter::GraphError::Refusal(wire::Refusal::Malformed(_))
        ),
        "{error:?}"
    );
}

#[test]
fn an_attachment_is_fetched_raw_and_only_when_its_part_is_asked_for() {
    let mut r = Replay::new();
    let target = wire::rooted(&wire::attachment_value_target(&id("AAMk-m1"), "AAMk-att-2"));
    r.on("GET", &target, b"\x89PNG bytes");
    let g = graph(r);
    let bytes = g
        .fetch_part(&id("AAMk-m1"), "attachment:AAMk-att-2")
        .unwrap();
    assert_eq!(bytes, b"\x89PNG bytes");
    assert_eq!(wire_log(&g), [format!("GET {target}")]);
    assert!(g.fetch_part(&id("AAMk-m1"), "no-such-part").is_err());
}

// ---------------------------------------------------------------------------
// 6. Mutations: every intent pinned to the element it sends, outcomes per element.
// ---------------------------------------------------------------------------

fn mutation(message: &str, operation: Operation) -> WireMutation {
    WireMutation {
        message: id(message),
        intent_id: 7,
        operation,
    }
}

/// Answer every element of the next batch with the same status.
fn all(r: &mut Replay, count: usize, code: u16) {
    let items: Vec<(usize, u16, serde_json::Value)> = (0..count)
        .map(|i| (i, code, serde_json::json!({})))
        .collect();
    r.on("POST", BATCH, &batch_answer(&items));
}

/// The whole of FR-13, with the exact element each one sends.
///
/// Read this as the answer to "what will Sift do to my mailbox". Two entries are worth pausing
/// on: junk reporting makes **no request at all**, because the stable API has no report call
/// and a move is not one; and nothing anywhere reaches a send, reply or forward.
#[test]
fn every_intent_makes_exactly_the_request_it_should_and_no_other() {
    type Expected = Option<(&'static str, &'static str, serde_json::Value)>;
    let expected: Vec<(&str, Operation, Expected)> = vec![
        (
            "archive",
            Operation::Archive,
            Some((
                "POST",
                "/me/messages/m1/move",
                serde_json::json!({"destinationId":"archive"}),
            )),
        ),
        (
            "delete-to-trash",
            Operation::DeleteToTrash,
            Some((
                "POST",
                "/me/messages/m1/move",
                serde_json::json!({"destinationId":"deleteditems"}),
            )),
        ),
        (
            "permanently-delete",
            Operation::PermanentlyDelete,
            Some((
                "POST",
                "/me/messages/m1/permanentDelete",
                serde_json::Value::Null,
            )),
        ),
        (
            "move-to",
            Operation::MoveTo(RemoteFolderId("AAMk-projects".into())),
            Some((
                "POST",
                "/me/messages/m1/move",
                serde_json::json!({"destinationId":"AAMk-projects"}),
            )),
        ),
        (
            "mark-read",
            Operation::SetRead(true),
            Some((
                "PATCH",
                "/me/messages/m1",
                serde_json::json!({"isRead":true}),
            )),
        ),
        (
            "mark-unread",
            Operation::SetRead(false),
            Some((
                "PATCH",
                "/me/messages/m1",
                serde_json::json!({"isRead":false}),
            )),
        ),
        (
            "flag",
            Operation::SetFlagged(true),
            Some((
                "PATCH",
                "/me/messages/m1",
                serde_json::json!({"flag":{"flagStatus":"flagged"}}),
            )),
        ),
        (
            "unflag",
            Operation::SetFlagged(false),
            Some((
                "PATCH",
                "/me/messages/m1",
                serde_json::json!({"flag":{"flagStatus":"notFlagged"}}),
            )),
        ),
        ("report-junk", Operation::ReportJunk, None),
        ("report-not-junk", Operation::ReportNotJunk, None),
    ];
    for (name, operation, call) in expected {
        let mut r = Replay::new();
        all(&mut r, 1, 200);
        let g = graph(r);
        let outcomes = g.apply(&[mutation("m1", operation)]).unwrap();
        let sent = batches_sent(&g);
        match call {
            Some((method, url, body)) => {
                assert_eq!(outcomes, [MutationOutcome::Applied], "`{name}`");
                assert_eq!(sent.len(), 1, "`{name}` sent {} batches", sent.len());
                assert_eq!(
                    sent[0],
                    vec![(method.to_owned(), url.to_owned(), body)],
                    "`{name}` did not send what it should"
                );
            }
            None => {
                assert_eq!(outcomes, [MutationOutcome::Refused], "`{name}`");
                assert!(sent.is_empty(), "`{name}` put something on the wire");
            }
        }
    }
}

#[test]
fn a_tag_change_reads_the_categories_and_restates_them_whole() {
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (0, 200, serde_json::json!({"categories":["Receipts"]})),
            (
                1,
                200,
                serde_json::json!({"categories":["Receipts","Travel"]}),
            ),
        ]),
    );
    all(&mut r, 2, 200);
    let g = graph(r);
    let outcomes = g
        .apply(&[
            mutation("m1", Operation::AddTag("Travel".into())),
            mutation("m2", Operation::RemoveTag("Receipts".into())),
        ])
        .unwrap();
    assert_eq!(
        outcomes,
        [MutationOutcome::Applied, MutationOutcome::Applied]
    );
    let sent = batches_sent(&g);
    assert_eq!(sent.len(), 2, "the read and the write are one batch each");
    assert_eq!(sent[0][0].1, "/me/messages/m1?$select=categories");
    assert_eq!(
        sent[1][0],
        (
            "PATCH".to_owned(),
            "/me/messages/m1".to_owned(),
            serde_json::json!({"categories":["Receipts","Travel"]})
        )
    );
    assert_eq!(
        sent[1][1].2,
        serde_json::json!({"categories":["Travel"]}),
        "removing one category dropped another"
    );
}

#[test]
fn a_tag_already_in_the_state_asked_for_succeeds_without_a_write() {
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (0, 200, serde_json::json!({"categories":["Receipts"]})),
            (1, 200, serde_json::json!({"categories":[]})),
        ]),
    );
    let g = graph(r);
    let outcomes = g
        .apply(&[
            mutation("m1", Operation::AddTag("Receipts".into())),
            mutation("m2", Operation::RemoveTag("Receipts".into())),
        ])
        .unwrap();
    assert_eq!(
        outcomes,
        [MutationOutcome::Applied, MutationOutcome::Applied]
    );
    assert_eq!(batches_sent(&g).len(), 1, "a no-op was written");
}

#[test]
fn a_tag_whose_categories_could_not_be_read_is_settled_without_a_write() {
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (
                0,
                404,
                serde_json::json!({"error":{"code":"ErrorItemNotFound","message":"x"}}),
            ),
            (1, 503, serde_json::json!({})),
            // Element 2 is not answered at all.
        ]),
    );
    let g = graph(r);
    let outcomes = g
        .apply(&[
            mutation("m1", Operation::AddTag("Travel".into())),
            mutation("m2", Operation::RemoveTag("Receipts".into())),
            mutation("m3", Operation::AddTag("Travel".into())),
        ])
        .unwrap();
    assert_eq!(
        outcomes,
        [
            MutationOutcome::Refused,
            MutationOutcome::Transient,
            MutationOutcome::Transient,
        ]
    );
    // Restating a category list that was never read would overwrite what is there.
    assert_eq!(batches_sent(&g).len(), 1, "a tag was written unread");
}

#[test]
fn a_tag_name_past_l15_is_refused_without_asking() {
    let g = graph(Replay::new());
    let long = "x".repeat(10_000);
    let outcomes = g.apply(&[mutation("m1", Operation::AddTag(long))]).unwrap();
    assert_eq!(outcomes, [MutationOutcome::Refused]);
    assert!(g.transport().performed.is_empty());
}

#[test]
fn a_batch_that_half_succeeds_is_resolved_per_element() {
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (3, 503, serde_json::json!({})),
            (0, 201, serde_json::json!({"id":"new-id"})),
            (
                1,
                404,
                serde_json::json!({"error":{"code":"ErrorItemNotFound","message":"x"}}),
            ),
            (2, 429, serde_json::json!({})),
            // Element 4 is not answered at all.
        ]),
    );
    let g = graph(r);
    let outcomes = g
        .apply(&[
            mutation("m1", Operation::Archive),
            mutation("m2", Operation::Archive),
            mutation("m3", Operation::SetRead(true)),
            mutation("m4", Operation::SetFlagged(true)),
            mutation("m5", Operation::DeleteToTrash),
        ])
        .unwrap();
    assert_eq!(
        outcomes,
        [
            MutationOutcome::Applied,
            // Gone from under this identifier — deleted, or already moved and renamed.
            MutationOutcome::Refused,
            MutationOutcome::Transient,
            MutationOutcome::Transient,
            // Sent and never answered: D-85's Reconciling, not a blind replay.
            MutationOutcome::Unknown,
        ]
    );
}

#[test]
fn a_batch_whose_answer_never_came_back_is_unknown_rather_than_failed() {
    let mut r = Replay::new();
    r.fail_nth("POST", BATCH, 0, TransportError::Unknown);
    let g = graph(r);
    let error = g.apply(&[mutation("m1", Operation::Archive)]).unwrap_err();
    assert_eq!(g.classify(&error), Failure::Unknown);
}

#[test]
fn a_later_chunk_that_fails_does_not_discard_the_outcomes_of_one_already_applied() {
    let batch: Vec<WireMutation> = (0..25)
        .map(|i| mutation(&format!("m{i}"), Operation::Archive))
        .collect();
    let mut r = Replay::new();
    all(&mut r, 20, 201);
    r.fail_nth("POST", BATCH, 1, TransportError::Unknown);
    let g = graph(r);
    let outcomes = g.apply(&batch).unwrap();
    assert!(
        outcomes[..20]
            .iter()
            .all(|o| *o == MutationOutcome::Applied)
    );
    assert!(
        outcomes[20..]
            .iter()
            .all(|o| *o == MutationOutcome::Unknown)
    );
}

#[test]
fn nothing_in_the_intent_set_can_reach_a_send_reply_or_forward() {
    let every = [
        Operation::Archive,
        Operation::DeleteToTrash,
        Operation::PermanentlyDelete,
        Operation::MoveTo(RemoteFolderId("AAMk-projects".into())),
        Operation::SetRead(true),
        Operation::SetFlagged(true),
        Operation::AddTag("Travel".into()),
        Operation::RemoveTag("Receipts".into()),
        Operation::ReportJunk,
        Operation::ReportNotJunk,
    ];
    let batch: Vec<WireMutation> = every
        .iter()
        .enumerate()
        .map(|(i, o)| mutation(&format!("m{i}"), o.clone()))
        .collect();
    let mut r = Replay::new();
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (0, 200, serde_json::json!({"categories":[]})),
            (1, 200, serde_json::json!({"categories":["Receipts"]})),
        ]),
    );
    all(&mut r, 8, 200);
    let g = graph(r);
    let _ = g.apply(&batch).unwrap();
    let mut urls: Vec<String> = wire_log(&g);
    urls.extend(batches_sent(&g).into_iter().flatten().map(|(_, u, _)| u));
    for url in urls {
        let lower = url.to_ascii_lowercase();
        for word in ["send", "reply", "forward", "createreply", "createforward"] {
            assert!(!lower.contains(word), "`{url}` reached the wire");
        }
    }
}

// ---------------------------------------------------------------------------
// 7. Credentials and the doorbell.
// ---------------------------------------------------------------------------

#[test]
fn every_request_carries_the_bearer_and_a_replaced_token_is_the_one_sent_next() {
    let mut r = Replay::new();
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        DELTA_QUIET,
    );
    let g = graph(r);
    let _ = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    g.present_credential("a-refreshed-token");
    let _ = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    assert_eq!(
        g.transport().header_of(0, "authorization"),
        Some("Bearer an-access-token")
    );
    assert_eq!(
        g.transport().header_of(1, "authorization"),
        Some("Bearer a-refreshed-token")
    );
}

#[test]
fn a_refused_token_is_its_own_class_and_not_a_permanent_failure() {
    let mut r = Replay::new();
    r.respond(
        "GET",
        &start(&inbox()),
        status(
            401,
            br#"{"error":{"code":"InvalidAuthenticationToken","message":"x"}}"#,
        ),
    );
    let g = graph(r);
    let error = g.delta(&inbox(), None).unwrap_err();
    assert_eq!(g.classify(&error), Failure::CredentialRefused);
}

#[test]
fn asking_for_a_doorbell_refuses_rather_than_quietly_succeeding() {
    let g = graph(Replay::new());
    assert!(g.watch(&[inbox()]).is_err());
    assert!(g.transport().performed.is_empty());
}

// ---------------------------------------------------------------------------
// 8. The move — D-44, and the case where merging wrongly is data loss.
// ---------------------------------------------------------------------------

/// D-44's corroboration, as the store makes it: over the originator, the origination date,
/// the subject and the reference chain. Written out here because the digest itself lives with
/// the store, which this crate may not depend on — what is under test is that the adapter
/// hands the join everything it needs, and that the rule resolves ambiguity to distinct
/// messages.
fn corroborates(a: &Envelope, b: &Envelope) -> bool {
    a.from == b.from
        && a.origination_date_millis == b.origination_date_millis
        && a.subject == b.subject
        && a.references == b.references
}

/// A local message, as the store would hold it: its identity, and the envelope it last had.
#[derive(Debug, Clone)]
struct Held {
    local: u128,
    remote: RemoteMessageId,
    envelope: Envelope,
}

#[test]
fn an_ambiguous_move_leaves_two_messages_rather_than_guessing() {
    let mut r = Replay::new();
    // Before: two deliveries of one message in one conversation — a list and a direct copy,
    // same `Message-ID`, same sender, same date, same subject — and one ordinary message.
    r.on("POST", BATCH, ENVELOPES_BEFORE_MOVE);
    // The user moves all three to the archive. The inbox reports three departures; the archive
    // reports three arrivals under identifiers nobody has seen.
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        INBOX_MOVED_OUT,
    );
    r.on(
        "GET",
        "/v1.0/me/mailFolders/AAMk-archive/messages/delta?$deltatoken=round1",
        ARCHIVE_MOVED_IN,
    );
    r.on("POST", BATCH, ENVELOPES_AFTER_MOVE);
    let g = graph(r);

    let before = g
        .fetch_envelopes(&[id("AAMk-d1"), id("AAMk-d2"), id("AAMk-u1")])
        .unwrap();
    let mut held: Vec<Held> = before
        .into_iter()
        .enumerate()
        .map(|(i, envelope)| Held {
            local: 100 + i as u128,
            remote: envelope.id.clone(),
            envelope,
        })
        .collect();
    let identities_before: Vec<u128> = held.iter().map(|h| h.local).collect();

    let out_of_inbox = g.delta(&inbox(), Some(&live_cursor())).unwrap();
    let archive_cursor = Position {
        live: true,
        link: "/v1.0/me/mailFolders/AAMk-archive/messages/delta?$deltatoken=round1".into(),
    }
    .encode();
    let into_archive = g.delta(&archive(), Some(&archive_cursor)).unwrap();

    let departed_ids: Vec<RemoteMessageId> = out_of_inbox
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Removed { id } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(departed_ids.len(), 3);
    let mut arrived_ids: Vec<RemoteMessageId> = into_archive
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Present { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    arrived_ids.dedup();
    let arrivals = g.fetch_envelopes(&arrived_ids).unwrap();
    assert_eq!(arrivals.len(), 3);

    let departed: Vec<Held> = held
        .iter()
        .filter(|h| departed_ids.contains(&h.remote))
        .cloned()
        .collect();
    held.retain(|h| !departed_ids.contains(&h.remote));

    let mut next_local = 200u128;
    let mut outcomes = Vec::new();
    for arrival in &arrivals {
        let candidates: Vec<Departed<'_>> = departed
            .iter()
            .map(|d| Departed {
                message: d.local,
                conversation: d.envelope.thread_id.as_deref(),
                internet_message_id: d.envelope.internet_message_id.as_deref(),
                corroborated: corroborates(&d.envelope, arrival),
            })
            .collect();
        let outcome = rejoin(arrival, &candidates);
        let local = match outcome {
            MoveOutcome::Rejoined { message } => message,
            MoveOutcome::DeleteAndArrival => {
                next_local += 1;
                next_local
            }
        };
        outcomes.push((arrival.id.clone(), outcome));
        held.push(Held {
            local,
            remote: arrival.id.clone(),
            envelope: arrival.clone(),
        });
    }

    // The unambiguous move keeps its identity — the shell's selection survives it.
    let invoice = held.iter().find(|h| h.remote == id("AAMk-n-u1")).unwrap();
    assert_eq!(invoice.local, identities_before[2]);

    // The duplicates were each corroborated by **both** departures. Choosing one would be a
    // guess, and a wrong guess is an archive applied to the wrong message.
    let duplicates: Vec<&Held> = held
        .iter()
        .filter(|h| h.envelope.thread_id.as_deref() == Some("conv-dup"))
        .collect();
    assert_eq!(duplicates.len(), 2, "an ambiguous move merged two messages");
    assert_ne!(duplicates[0].local, duplicates[1].local);
    for duplicate in &duplicates {
        assert!(
            !identities_before.contains(&duplicate.local),
            "an ambiguous arrival inherited an identity it could not prove was its own"
        );
    }
    for (remote, outcome) in &outcomes {
        if remote.0.starts_with("AAMk-n-d") {
            assert_eq!(*outcome, MoveOutcome::DeleteAndArrival);
        }
    }
    assert_eq!(held.len(), 3, "a message was lost or invented by the move");
}

// ---------------------------------------------------------------------------
// 9. The whole of it: add, backfill, go live, triage, flush.
// ---------------------------------------------------------------------------

#[test]
fn an_account_is_added_backfilled_kept_live_and_triaged_through_the_harness() {
    let mut r = Replay::new();
    with_folders(&mut r);
    // The batch endpoint answers in the order the session asks: well-known names, the two
    // backfill pages' envelopes, the live page's envelope, then the flush.
    r.on("POST", BATCH, WELL_KNOWN);
    r.on("POST", BATCH, ENVELOPES_1);
    r.on("POST", BATCH, ENVELOPES_2);
    r.on("POST", BATCH, ENVELOPES_LIVE);
    r.on(
        "POST",
        BATCH,
        &batch_answer(&[
            (0, 201, serde_json::json!({"id":"AAMk-m2-archived"})),
            (1, 200, serde_json::json!({})),
        ]),
    );
    r.on("GET", &start(&inbox()), DELTA_1);
    r.on("GET", &format!("{DELTA_BASE}?$skiptoken=page2"), DELTA_2);
    r.on(
        "GET",
        &format!("{DELTA_BASE}?$deltatoken=round1"),
        DELTA_LIVE,
    );
    let g = graph(r);

    // Add: the folders, and which of them is the inbox — found by name, not by display name.
    let folders = g.enumerate_folders().unwrap();
    let inbox_folder = folders
        .iter()
        .find(|f| f.special_use == Some(SpecialUse::Inbox))
        .unwrap();
    assert_eq!(inbox_folder.id, inbox());

    // Backfill, page by page, fetching only what each page names.
    let mut held: Vec<Envelope> = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let page = g.delta(&inbox_folder.id, cursor.as_ref()).unwrap();
        let wanted: Vec<RemoteMessageId> = page
            .changes
            .iter()
            .filter_map(|c| match c {
                Change::Present { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        held.extend(g.fetch_envelopes(&wanted).unwrap());
        cursor = Some(page.next);
        if !page.more {
            break;
        }
    }
    assert_eq!(held.len(), 3);

    // Live: one arrival, one departure.
    let live = g.delta(&inbox_folder.id, cursor.as_ref()).unwrap();
    let mut arrived: Vec<RemoteMessageId> = Vec::new();
    for change in &live.changes {
        match change {
            Change::Present { id, provenance } => {
                assert_eq!(*provenance, Provenance::Delivered);
                arrived.push(id.clone());
            }
            Change::Removed { id } => held.retain(|e| e.id != *id),
            Change::FlagsChanged { .. } => {}
        }
    }
    arrived.dedup();
    held.extend(g.fetch_envelopes(&arrived).unwrap());
    let mut remaining: Vec<&str> = held.iter().map(|e| e.id.0.as_str()).collect();
    remaining.sort_unstable();
    assert_eq!(remaining, ["AAMk-m2", "AAMk-m3", "AAMk-m4"]);

    // Triage, then flush: archive one, mark another read, in one batch.
    let outcomes = g
        .apply(&[
            mutation("AAMk-m2", Operation::Archive),
            mutation("AAMk-m4", Operation::SetRead(true)),
        ])
        .unwrap();
    assert_eq!(
        outcomes,
        [MutationOutcome::Applied, MutationOutcome::Applied]
    );
    let flush = batches_sent(&g).pop().unwrap();
    assert_eq!(
        flush,
        vec![
            (
                "POST".to_owned(),
                "/me/messages/AAMk-m2/move".to_owned(),
                serde_json::json!({"destinationId":"archive"})
            ),
            (
                "PATCH".to_owned(),
                "/me/messages/AAMk-m4".to_owned(),
                serde_json::json!({"isRead":true})
            ),
        ]
    );
    // Nothing on the replay harness costs anybody's data plan.
    assert_eq!(g.wire_bytes(), (0, 0));
}

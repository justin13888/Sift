//! D-65's recorded exchanges, replayed instead of reached.
//!
//! **Scrubbed** — there is no real address, subject or body here. NFR-22 reaches the test
//! tree: a corpus of real addresses and subjects in a public repository is the disclosure the
//! whole privacy posture exists to prevent, arriving through the door nobody was watching.
//!
//! This lives beside the register rather than in a shell because it is what a shell is handed,
//! not something a shell builds. `docs/build/verification.md` puts the fixture harness in P0
//! **before the adapters it tests**.

use sift_provider::transport::{Replay, Response};

/// The Gmail corpus.
#[must_use]
pub fn gmail() -> Replay {
    use sift_gmail::wire;
    use sift_provider::adapter::{RemoteFolderId, RemoteMessageId};

    let inbox = RemoteFolderId("INBOX".into());
    let mut r = Replay::new();
    r.on(
        "GET",
        &wire::profile_target(),
        br#"{"emailAddress":"someone@example.test","historyId":"1000"}"#,
    );
    r.on(
        "GET",
        &wire::labels_target(),
        br#"{"labels":[
            {"id":"INBOX","name":"Inbox","type":"system"},
            {"id":"SENT","name":"Sent","type":"system"},
            {"id":"TRASH","name":"Trash","type":"system"},
            {"id":"SPAM","name":"Spam","type":"system"},
            {"id":"UNREAD","name":"UNREAD","type":"system"},
            {"id":"STARRED","name":"STARRED","type":"system"},
            {"id":"Label_11","name":"Receipts","type":"user"}
        ]}"#,
    );
    // Two pages, so a resumed backfill is drivable.
    r.on(
        "GET",
        &wire::list_target(&inbox, None, 500),
        br#"{"messages":[{"id":"m1","threadId":"t1"},{"id":"m2","threadId":"t1"}],"nextPageToken":"p2"}"#,
    );
    r.on(
        "GET",
        &wire::list_target(&inbox, Some("p2"), 500),
        br#"{"messages":[{"id":"m3","threadId":"t2"}]}"#,
    );
    // The delta: one arrival, one read-state change, one departure.
    r.on(
        "GET",
        &wire::history_target("1000", &inbox, None, 500),
        br#"{"history":[
            {"id":"1001","messagesAdded":[{"message":{"id":"m4","threadId":"t3","labelIds":["INBOX","UNREAD"]}}]},
            {"id":"1002","labelsRemoved":[{"message":{"id":"m1","threadId":"t1"},"labelIds":["UNREAD"]}]},
            {"id":"1003","labelsRemoved":[{"message":{"id":"m2","threadId":"t1"},"labelIds":["INBOX"]}]}
        ],"historyId":"1003"}"#,
    );
    // And then quiet, so a second sync is a no-op rather than a repeat.
    r.on(
        "GET",
        &wire::history_target("1003", &inbox, None, 500),
        br#"{"historyId":"1003"}"#,
    );

    let envelopes: &[(&str, &str, &str, u64, bool)] = &[
        ("m1", "t1", "A receipt", 1_700_000_001_000, false),
        ("m2", "t1", "Re: A receipt", 1_700_000_002_000, true),
        ("m3", "t2", "A newsletter", 1_700_000_003_000, true),
        ("m4", "t3", "Something new", 1_700_000_004_000, false),
    ];
    for (id, thread, subject, received, read) in envelopes {
        r.on(
            "GET",
            &wire::structure_target(&RemoteMessageId((*id).into())),
            structure(id).as_bytes(),
        );
        let _ = (thread, subject, received, read);
    }
    // The batch endpoint answers with a multipart document, **out of order on purpose**: the
    // provider does not promise order, and matching on position would attach one message's
    // subject to another's identifier.
    //
    // It answers with every envelope regardless of what was asked for, which is the second
    // thing this fixture is checking. An adapter that believed an answer it did not request
    // would insert messages the delta never listed, with a provenance nothing assigned them.
    r.respond("POST", &wire::batch_target(), batch(envelopes));
    // Mutations. A label change answers 204, and trashing answers with the message.
    r.respond(
        "POST",
        &wire::batch_modify_target(),
        Response {
            status: 204,
            headers: vec![],
            body: vec![],
        },
    );
    for id in ["m1", "m2", "m3", "m4"] {
        r.respond(
            "POST",
            &wire::trash_target(&RemoteMessageId(id.into())),
            Response {
                status: 200,
                headers: vec![],
                body: format!("{{\"id\":\"{id}\"}}").into_bytes(),
            },
        );
    }
    r
}

/// A message's MIME structure: a plain alternative, an HTML body, and an attachment that is
/// **not** fetched to render it.
fn structure(id: &str) -> String {
    let html = "<style>p{color:#111111;background-color:#ffffff}</style>\
                <p>Hello from a fixture. <img src=\"https://tracker.test/pixel.gif\" width=\"1\" height=\"1\"> \
                <a href=\"https://example.test/read\">read more</a></p>";
    let encoded = sift_provider::oauth::base64url(html.as_bytes());
    format!(
        r#"{{"id":"{id}","payload":{{"partId":"","mimeType":"multipart/mixed","body":{{"size":0}},
        "parts":[
          {{"partId":"0","mimeType":"text/plain","body":{{"size":5,"data":"aGVsbG8"}}}},
          {{"partId":"1","mimeType":"text/html","body":{{"size":{},"data":"{encoded}"}}}},
          {{"partId":"2","mimeType":"application/pdf","filename":"statement.pdf",
            "body":{{"size":41943040,"attachmentId":"ATT-{id}"}}}}
        ]}}}}"#,
        html.len()
    )
}

fn batch(envelopes: &[(&str, &str, &str, u64, bool)]) -> Response {
    let mut body = String::new();
    // Deliberately answered back to front.
    for (position, (id, thread, subject, received, read)) in envelopes.iter().enumerate().rev() {
        let labels = if *read {
            r#""INBOX""#
        } else {
            r#""INBOX","UNREAD""#
        };
        let payload = format!(
            r#"{{"id":"{id}","threadId":"{thread}","labelIds":[{labels}],
                 "snippet":"a scrubbed snippet","internalDate":"{received}","sizeEstimate":4096,
                 "payload":{{"headers":[
                   {{"name":"Subject","value":"{subject}"}},
                   {{"name":"From","value":"Someone <someone@example.test>"}},
                   {{"name":"To","value":"me@example.test"}},
                   {{"name":"Date","value":"Mon, 31 Aug 2026 12:34:56 +0000"}},
                   {{"name":"Message-ID","value":"<{id}@example.test>"}}
                 ]}}}}"#
        );
        body.push_str("--answer\r\nContent-Type: application/http\r\n");
        body.push_str(&format!("Content-ID: <response-item-{position}>\r\n\r\n"));
        body.push_str("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n");
        body.push_str(&payload);
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

#[cfg(test)]
mod tests {
    use super::*;
    use sift_provider::transport::Transport as _;

    #[test]
    fn the_fixture_corpus_carries_no_real_mail() {
        // NFR-22 reaches the test tree.
        let mut replay = gmail();
        let answer = replay
            .exchange(&sift_provider::transport::Request::new(
                "GET",
                &sift_gmail::wire::profile_target(),
            ))
            .unwrap();
        let text = String::from_utf8_lossy(&answer.body).into_owned();
        assert!(text.contains("example.test"), "{text}");
    }
}

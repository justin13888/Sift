//! Adding an account, and everything a live one needs to work.
//!
//! # A shell is where a provider gets named
//!
//! Everywhere above the adapter layer plans against declared capabilities, and
//! `cargo xtask invariants` enforces it — but the rule stops below the shells, because "add a
//! Gmail account" is a thing a person clicks. The register of what a user can add is the
//! shell's, and the moment after the account exists, nothing here knows which provider it is
//! again: every command below this one goes through [`Adapter`] and
//! [`Capabilities`](sift_provider::capability::Capabilities).

use sift_credentials::oauth::{AuthError, Broker, Registration};
use sift_credentials::store::CredentialStore;
use sift_foundation::identifiers::CALLBACK_SCHEME;
use sift_foundation::identity::AccountId;
use sift_gmail::Gmail;
use sift_provider::adapter::Adapter;
use sift_provider::transport::{Replay, Response};

/// The adapter an account is reached through.
///
/// `dyn` rather than an enum of the four, and that is the capability model working: this
/// shell asks which provider an account is exactly **once**, when the user adds it. After
/// that the type is gone and every command plans against what the account declares.
pub type Live = Box<dyn Adapter<Error = sift_gmail::GmailError>>;

/// Where the callback comes back to — D-36's registered URI scheme.
///
/// **Not a loopback address.** NFR-24 admits no listening socket for any purpose, and D-36
/// removes the loopback redirect rather than excusing it.
#[must_use]
pub fn redirect_uri() -> String {
    format!("{CALLBACK_SCHEME}:/oauth2/callback")
}

/// The authorization this shell can offer.
#[must_use]
pub fn registration(client_id: &str) -> Registration {
    Registration {
        profile: sift_gmail::oauth::profile(),
        client_id: client_id.to_owned(),
        redirect_uri: redirect_uri(),
    }
}

/// Begin an authorization and hand back the address to open.
///
/// # Errors
/// See [`AuthError`]. In particular it refuses to begin where the callback scheme is not
/// registered with the system — checked before the user goes anywhere, because discovering
/// it afterwards means they have already been sent to a browser and returned to nothing.
pub fn begin<S: CredentialStore>(
    broker: &mut Broker<S>,
    client_id: &str,
    scheme_is_registered: bool,
    now_millis: u64,
) -> Result<String, AuthError> {
    broker.begin(&registration(client_id), scheme_is_registered, now_millis)
}

/// Complete an authorization from the address the system handed back, and build the adapter.
///
/// # Errors
/// See [`AuthError`].
pub fn complete<S: CredentialStore>(
    broker: &mut Broker<S>,
    client_id: &str,
    account: AccountId,
    callback: &str,
    now_millis: u64,
) -> Result<Live, AuthError> {
    let registration = registration(client_id);
    let mut token_transport = sift_http::Https::to(&registration.profile.token.host)
        .map_err(|why| AuthError::Store(sift_credentials::store::StoreError::Unavailable(why)))?;
    let pair = broker.complete(
        &mut token_transport,
        &registration,
        account,
        callback,
        now_millis,
    )?;
    let api = sift_http::Https::to(sift_gmail::oauth::API_HOST)
        .map_err(|why| AuthError::Store(sift_credentials::store::StoreError::Unavailable(why)))?;
    Ok(Box::new(Gmail::new(api, &pair.access)))
}

/// An account backed by the fixture corpus rather than by a socket — D-65.
///
/// This is not a convenience. `docs/build/verification.md` puts the fixture harness in P0
/// **before the adapters it tests**, because it is what makes a provider's behaviour
/// assertable "against servers nobody has": a cursor outside the retained history window, a
/// throttle, a message that vanishes between the delta and the fetch. None of those can be
/// arranged against a real account on demand, and a shell that could only be driven against
/// one would leave every one of them untested end to end.
#[must_use]
pub fn replayed() -> Live {
    Box::new(Gmail::new(corpus(), "a-fixture-token"))
}

/// The recorded exchanges. **Scrubbed** — there is no real address, subject or body here.
fn corpus() -> Replay {
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

/// Whether the callback returns through a socket.
///
/// **No.** NFR-24 admits no listening socket for any purpose, and this shell is where a
/// loopback redirect would be easiest to reach for.
#[must_use]
pub const fn callback_arrives_on_a_socket() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_provider::transport::Transport as _;

    #[test]
    fn the_callback_returns_through_a_registered_scheme_and_not_a_socket() {
        assert!(!callback_arrives_on_a_socket());
        let uri = redirect_uri();
        assert!(uri.starts_with(CALLBACK_SCHEME), "{uri}");
        assert!(
            !uri.contains("localhost") && !uri.contains("127.0.0.1"),
            "{uri}"
        );
    }

    #[test]
    fn the_registration_asks_for_nothing_that_can_send() {
        assert!(!registration("c").profile.authorizes_sending());
    }

    #[test]
    fn the_fixture_corpus_carries_no_real_mail() {
        // NFR-22 reaches the test tree. A corpus of real addresses and subjects in a public
        // repository is the disclosure the whole privacy posture exists to prevent, arriving
        // through the door nobody was watching.
        let mut replay = corpus();
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

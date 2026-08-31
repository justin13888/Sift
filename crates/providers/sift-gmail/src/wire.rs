//! What Sift asks this provider for, and how it reads the answers.
//!
//! Everything here is a pure function over bytes. That is deliberate and it is what makes
//! D-65's fixture corpus worth having: the request builders can be asserted on without a
//! socket, and "Sift MUST NOT fetch whole messages" is a claim about a *target string* that
//! a test can read.
//!
//! # Three rules from `docs/mail/providers/gmail.md`, made mechanical
//!
//! 1. **Envelope fetches request metadata only, through the batch endpoint.** The
//!    full-message endpoint is the convenient one, which is why this is the provider where
//!    the rule is easiest to break by accident. [`envelope_target`] cannot express it.
//! 2. **The history feed is the only path by which change is applied.** Nothing else in this
//!    module produces a [`Change`].
//! 3. **A cursor outside the retained window is a recovery, not an error.** It arrives as a
//!    404 from the history endpoint and leaves here as [`Refusal::CursorInvalidated`].

use base64::Engine as _;
use sift_provider::adapter::{Change, Envelope, Provenance, RemoteFolderId, RemoteMessageId};
use sift_provider::rfc5322;

use crate::label::Label;

/// The version prefix every path shares. `me` rather than an address: the token identifies
/// the account, and putting the address in the path would mean the adapter had to know it.
pub const USER: &str = "/gmail/v1/users/me";

/// The headers an envelope fetch asks for, and the whole of it.
///
/// Each one is needed by something named: the first four by the list row, and the last three
/// by D-44 — whose corroborating digest is over the originator address, the origination
/// date, the normalized subject and the reference chain.
pub const ENVELOPE_HEADERS: &[&str] = &[
    "Subject",
    "From",
    "To",
    "Date",
    "Message-ID",
    "References",
    "In-Reply-To",
];

/// What went wrong that is not a transport failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The answer did not parse as this provider's own JSON.
    Malformed(&'static str),
    /// The stored cursor is outside the provider's retained history window.
    ///
    /// **Not an error.** D-82 moves the folder to `Invalidated` and NFR-18's recovery
    /// follows without asking the user anything.
    CursorInvalidated,
    /// The provider refused in its own terms, with its own message.
    Denied { status: u16, message: String },
}

// ---------------------------------------------------------------------------
// Targets. Every request Sift makes of this provider is one of these.
// ---------------------------------------------------------------------------

#[must_use]
pub fn profile_target() -> String {
    format!("{USER}/profile")
}

#[must_use]
pub fn labels_target() -> String {
    format!("{USER}/labels")
}

/// One page of a folder's backfill.
///
/// `includeSpamTrash` is left at its default of false for every folder **except** the two it
/// would otherwise hide, because a mailbox whose trash cannot be listed is one where FR-13's
/// delete has nowhere visible to put anything.
#[must_use]
pub fn list_target(folder: &RemoteFolderId, page: Option<&str>, page_size: u32) -> String {
    let mut target = format!(
        "{USER}/messages?maxResults={page_size}&labelIds={}",
        encode(&folder.0)
    );
    if matches!(folder.0.as_str(), crate::label::TRASH | crate::label::SPAM) {
        target.push_str("&includeSpamTrash=true");
    }
    if let Some(page) = page {
        target.push_str(&format!("&pageToken={}", encode(page)));
    }
    target
}

/// One page of the change feed, from a history identifier.
#[must_use]
pub fn history_target(
    from: &str,
    folder: &RemoteFolderId,
    page: Option<&str>,
    page_size: u32,
) -> String {
    let mut target = format!(
        "{USER}/history?startHistoryId={}&maxResults={page_size}&labelId={}",
        encode(from),
        encode(&folder.0)
    );
    if let Some(page) = page {
        target.push_str(&format!("&pageToken={}", encode(page)));
    }
    target
}

/// One envelope. **Metadata only** — the rule this module exists to keep.
#[must_use]
pub fn envelope_target(id: &RemoteMessageId) -> String {
    let headers: Vec<String> = ENVELOPE_HEADERS
        .iter()
        .map(|h| format!("&metadataHeaders={h}"))
        .collect();
    format!(
        "{USER}/messages/{}?format=metadata{}",
        encode(&id.0),
        headers.concat()
    )
}

/// A message's MIME structure, with small parts inline and attachments **by reference**.
///
/// This is the structure-first fetch `docs/mail/sync-engine.md` requires: an attachment
/// arrives as an identifier rather than as bytes, so a message carrying 40 MB costs a few
/// kilobytes until somebody asks for the attachment.
#[must_use]
pub fn structure_target(id: &RemoteMessageId) -> String {
    format!("{USER}/messages/{}?format=full", encode(&id.0))
}

#[must_use]
pub fn attachment_target(id: &RemoteMessageId, attachment: &str) -> String {
    format!(
        "{USER}/messages/{}/attachments/{}",
        encode(&id.0),
        encode(attachment)
    )
}

#[must_use]
pub fn batch_target() -> String {
    "/batch/gmail/v1".to_owned()
}

#[must_use]
pub fn batch_modify_target() -> String {
    format!("{USER}/messages/batchModify")
}

#[must_use]
pub fn trash_target(id: &RemoteMessageId) -> String {
    format!("{USER}/messages/{}/trash", encode(&id.0))
}

#[must_use]
pub fn untrash_target(id: &RemoteMessageId) -> String {
    format!("{USER}/messages/{}/untrash", encode(&id.0))
}

/// Percent-encode a path or query component.
///
/// Identifiers here are the provider's, not a user's — but a label name is a *user's*, and
/// it reaches [`list_target`] as a folder identifier on some paths. An identifier that could
/// carry a `&` into a query string is a request the caller did not write.
#[must_use]
pub fn encode(value: &str) -> String {
    sift_provider::oauth::form_encode(value)
}

// ---------------------------------------------------------------------------
// Answers.
// ---------------------------------------------------------------------------

fn json(bytes: &[u8]) -> Result<serde_json::Value, Refusal> {
    serde_json::from_slice(bytes).map_err(|_| Refusal::Malformed("the answer was not JSON"))
}

/// Read the provider's own error document.
///
/// D-88's classifier turns on this distinction: a **well-formed** provider error is
/// authoritative, and anything that does not parse as one is evidence about the network
/// rather than about the account. A captive portal's sign-in page arrives here.
#[must_use]
pub fn refusal(status: u16, body: &[u8]) -> Refusal {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| "the provider refused and did not say why".to_owned());
    Refusal::Denied { status, message }
}

/// The account's current history identifier — the cursor a backfill takes **before** it
/// walks, per D-82.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer is not this provider's profile document.
pub fn parse_history_id(body: &[u8]) -> Result<String, Refusal> {
    json(body)?
        .get("historyId")
        .and_then(scalar)
        .ok_or(Refusal::Malformed(
            "the profile carried no history identifier",
        ))
}

/// # Errors
/// [`Refusal::Malformed`] where the answer is not a label list.
pub fn parse_labels(body: &[u8]) -> Result<Vec<Label>, Refusal> {
    let value = json(body)?;
    let labels = value
        .get("labels")
        .and_then(|l| l.as_array())
        .ok_or(Refusal::Malformed("the answer carried no labels"))?;
    Ok(labels
        .iter()
        .filter_map(|l| {
            let id = l.get("id")?.as_str()?.to_owned();
            let name = l
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or(&id)
                .to_owned();
            let system = l.get("type").and_then(|t| t.as_str()) == Some("system");
            Some(Label { id, name, system })
        })
        .collect())
}

/// One page of a message list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub ids: Vec<RemoteMessageId>,
    pub next_page: Option<String>,
}

/// # Errors
/// [`Refusal::Malformed`] where the answer is not a message list.
pub fn parse_list(body: &[u8]) -> Result<Page, Refusal> {
    let value = json(body)?;
    // An empty folder answers with the object and no `messages` key at all, which is a page
    // of nothing rather than a malformed answer.
    let ids = value
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|m| Some(RemoteMessageId(m.get("id")?.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    Ok(Page {
        ids,
        next_page: value
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .map(str::to_owned),
    })
}

/// One page of the change feed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryPage {
    pub changes: Vec<Change>,
    pub next_page: Option<String>,
    /// The identifier to resume from once this run of pages is applied.
    pub history_id: Option<String>,
}

/// Read one page of history, in the order the provider reported it.
///
/// **Order is preserved rather than deduplicated.** A message that arrived and was then
/// archived inside one page is two changes, and collapsing them would leave the store
/// holding whichever the collapse happened to keep.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer is not a history page.
pub fn parse_history(body: &[u8], folder: &RemoteFolderId) -> Result<HistoryPage, Refusal> {
    let value = json(body)?;
    let mut changes = Vec::new();

    for record in value
        .get("history")
        .and_then(|h| h.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        // An arrival. This is the *only* place `Delivered` is produced, which is what FR-23
        // will need three phases from now and cannot reconstruct afterwards.
        for added in array(record, "messagesAdded") {
            if let Some(id) = message_id(added) {
                changes.push(Change::Present {
                    id,
                    provenance: Provenance::Delivered,
                });
            }
        }
        for removed in array(record, "messagesDeleted") {
            if let Some(id) = message_id(removed) {
                changes.push(Change::Removed { id });
            }
        }
        // A label change is one of three things depending on *which* label moved: into this
        // folder, out of it, or a state on another axis.
        for touched in array(record, "labelsAdded") {
            let Some(id) = message_id(touched) else {
                continue;
            };
            changes.push(if mentions(touched, &folder.0) {
                Change::Present {
                    id,
                    provenance: Provenance::Discovered,
                }
            } else {
                Change::FlagsChanged { id }
            });
        }
        for touched in array(record, "labelsRemoved") {
            let Some(id) = message_id(touched) else {
                continue;
            };
            changes.push(if mentions(touched, &folder.0) {
                Change::Removed { id }
            } else {
                Change::FlagsChanged { id }
            });
        }
    }

    Ok(HistoryPage {
        changes,
        next_page: value
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .map(str::to_owned),
        history_id: value.get("historyId").and_then(scalar),
    })
}

fn array<'a>(value: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn message_id(record: &serde_json::Value) -> Option<RemoteMessageId> {
    Some(RemoteMessageId(
        record.get("message")?.get("id")?.as_str()?.to_owned(),
    ))
}

fn mentions(record: &serde_json::Value, label: &str) -> bool {
    array(record, "labelIds")
        .iter()
        .any(|l| l.as_str() == Some(label))
}

/// A history identifier is a number the provider writes as a string in some places and as a
/// number in others. Both are read; neither is converted.
fn scalar(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}

/// Read a message resource into an envelope.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer is not a message resource.
pub fn parse_envelope(body: &[u8]) -> Result<Envelope, Refusal> {
    let value = json(body)?;
    envelope_from(&value)
}

fn envelope_from(value: &serde_json::Value) -> Result<Envelope, Refusal> {
    let id = value
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or(Refusal::Malformed("a message carried no identifier"))?;

    let labels: Vec<&str> = array(value, "labelIds")
        .iter()
        .filter_map(|l| l.as_str())
        .collect();

    let header = |name: &str| -> Option<String> {
        value
            .get("payload")?
            .get("headers")?
            .as_array()?
            .iter()
            .find(|h| {
                h.get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })?
            .get("value")?
            .as_str()
            .map(str::to_owned)
    };

    // D-44's reference chain: `References` in order, with `In-Reply-To` appended where it
    // is not already there. The chain narrows a join; it never keys one.
    let mut references: Vec<String> = header("References")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if let Some(parent) = header("In-Reply-To")
        && let Some(parent) = parent.split_whitespace().next()
        && !references.iter().any(|r| r == parent)
    {
        references.push(parent.to_owned());
    }

    Ok(Envelope {
        id: RemoteMessageId(id.to_owned()),
        thread_id: value
            .get("threadId")
            .and_then(|t| t.as_str())
            .map(str::to_owned),
        internet_message_id: header("Message-ID"),
        references,
        subject: header("Subject"),
        from: header("From").as_deref().and_then(rfc5322::address_of),
        to: header("To")
            .as_deref()
            .map(rfc5322::addresses_of)
            .unwrap_or_default(),
        // The server's own received time. D-55 orders on this and never on the `Date`
        // header, which is the sender's and can say anything at all.
        received_at_millis: value
            .get("internalDate")
            .and_then(scalar)
            .and_then(|d| d.parse().ok())
            .unwrap_or(0),
        origination_date_millis: header("Date")
            .as_deref()
            .and_then(rfc5322::parse_date_millis)
            .and_then(|m| u64::try_from(m).ok()),
        snippet: value
            .get("snippet")
            .and_then(|s| s.as_str())
            .map(str::to_owned),
        folders: labels
            .iter()
            .filter(|l| crate::label::is_system_location(l))
            .map(|l| RemoteFolderId((*l).to_owned()))
            .collect(),
        // Everything that is neither a place nor an axis another field already owns. These
        // are provider identifiers rather than names: FR-37's affordance shows names, and
        // resolving one to the other needs the label set the adapter holds.
        tags: labels
            .iter()
            .filter(|l| {
                !crate::label::is_system_location(l)
                    && !matches!(
                        crate::label::system_axis(l),
                        crate::label::Axis::ClaimedElsewhere
                    )
            })
            .map(|l| (*l).to_owned())
            .collect(),
        // The *presence* of the unread label means unread, so read is its absence.
        read: !labels.contains(&crate::label::UNREAD),
        flagged: labels.contains(&crate::label::STARRED),
        size_estimate: value
            .get("sizeEstimate")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

/// One part of a message's MIME structure, as the provider describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The provider's own part identifier, which is what [`crate::Gmail::fetch_part`] takes.
    pub id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    /// Present where the bytes came inline.
    pub inline: Option<Vec<u8>>,
    /// Present where they did not, and must be asked for separately.
    pub attachment: Option<String>,
    pub size: u64,
}

/// Flatten a message's payload into its parts.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer carries no payload.
pub fn parse_structure(body: &[u8]) -> Result<Vec<Part>, Refusal> {
    let value = json(body)?;
    let payload = value
        .get("payload")
        .ok_or(Refusal::Malformed("the message carried no structure"))?;
    let mut out = Vec::new();
    flatten(payload, &mut out);
    Ok(out)
}

fn flatten(node: &serde_json::Value, out: &mut Vec<Part>) {
    let id = node
        .get("partId")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_owned();
    let body = node.get("body");
    out.push(Part {
        id,
        mime_type: node
            .get("mimeType")
            .and_then(|m| m.as_str())
            .unwrap_or("application/octet-stream")
            .to_owned(),
        filename: node
            .get("filename")
            .and_then(|f| f.as_str())
            .filter(|f| !f.is_empty())
            .map(str::to_owned),
        inline: body
            .and_then(|b| b.get("data"))
            .and_then(|d| d.as_str())
            .and_then(|d| decode_base64url(d).ok()),
        attachment: body
            .and_then(|b| b.get("attachmentId"))
            .and_then(|a| a.as_str())
            .map(str::to_owned),
        size: body
            .and_then(|b| b.get("size"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    });
    for child in array(node, "parts") {
        flatten(child, out);
    }
}

/// Decode the URL-safe, unpadded base64 the provider returns bodies in.
///
/// # Errors
/// [`Refusal::Malformed`] on anything that is not that encoding.
pub fn decode_base64url(value: &str) -> Result<Vec<u8>, Refusal> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .map_err(|_| Refusal::Malformed("a body was not URL-safe base64"))
}

/// The bytes of an attachment, out of the attachment resource.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer is not an attachment resource.
pub fn parse_attachment(body: &[u8]) -> Result<Vec<u8>, Refusal> {
    let value = json(body)?;
    let data = value
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or(Refusal::Malformed("the attachment carried no data"))?;
    decode_base64url(data)
}

// ---------------------------------------------------------------------------
// The batch endpoint.
// ---------------------------------------------------------------------------

/// The boundary Sift writes on a batch request.
///
/// Fixed rather than random: it is not a security boundary, and a fixed one makes a fixture
/// byte-comparable. Chosen so that no JSON, header or identifier can contain it.
pub const BATCH_BOUNDARY: &str = "sift-batch-Zm9vYmFy";

/// Build a batch of `GET`s.
#[must_use]
pub fn batch_request_body(targets: &[String]) -> Vec<u8> {
    let mut out = String::new();
    for (index, target) in targets.iter().enumerate() {
        out.push_str(&format!("--{BATCH_BOUNDARY}\r\n"));
        out.push_str("Content-Type: application/http\r\n");
        out.push_str(&format!("Content-ID: <item-{index}>\r\n\r\n"));
        out.push_str(&format!("GET {target}\r\n\r\n"));
    }
    out.push_str(&format!("--{BATCH_BOUNDARY}--\r\n"));
    out.into_bytes()
}

#[must_use]
pub fn batch_content_type() -> String {
    format!("multipart/mixed; boundary={BATCH_BOUNDARY}")
}

/// One answer inside a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    /// The index of the request this answers, read from its `Content-ID`.
    ///
    /// **Read rather than assumed**: the provider does not promise batch parts come back in
    /// the order they were sent, and matching on position would silently attach one
    /// message's envelope to another's identifier.
    pub index: Option<usize>,
    pub status: u16,
    pub body: Vec<u8>,
}

/// The boundary out of a `Content-Type`.
#[must_use]
pub fn boundary_of(content_type: &str) -> Option<String> {
    let at = content_type.to_ascii_lowercase().find("boundary=")?;
    let rest = content_type[at + "boundary=".len()..].trim();
    let rest = rest.split(';').next()?.trim();
    Some(rest.trim_matches('"').to_owned())
}

/// Split a multipart batch answer into its parts.
#[must_use]
pub fn parse_batch(body: &[u8], boundary: &str) -> Vec<BatchItem> {
    let text = String::from_utf8_lossy(body);
    let separator = format!("--{boundary}");
    let mut out = Vec::new();
    for segment in text.split(&separator).skip(1) {
        let segment = segment.trim_start_matches("\r\n").trim_start_matches('\n');
        if segment.starts_with("--") {
            break;
        }
        let Some((part_headers, embedded)) = split_head(segment) else {
            continue;
        };
        let index = part_headers
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-id:"))
            .and_then(|l| {
                let value = l.split_once(':')?.1.trim();
                let digits: String = value.chars().filter(char::is_ascii_digit).collect();
                digits.parse().ok()
            });
        let Some((embedded_head, embedded_body)) = split_head(embedded) else {
            continue;
        };
        let status = embedded_head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        out.push(BatchItem {
            index,
            status,
            body: embedded_body.trim_end().as_bytes().to_vec(),
        });
    }
    out
}

/// Split at the first blank line, tolerating both line endings.
fn split_head(text: &str) -> Option<(&str, &str)> {
    let (at, width) = match (text.find("\r\n\r\n"), text.find("\n\n")) {
        (Some(a), Some(b)) if b < a => (b, 2),
        (Some(a), _) => (a, 4),
        (None, Some(b)) => (b, 2),
        (None, None) => return None,
    };
    Some((&text[..at], &text[at + width..]))
}

/// The body of a batched label change.
#[must_use]
pub fn batch_modify_body(ids: &[RemoteMessageId], add: &[String], remove: &[String]) -> Vec<u8> {
    let value = serde_json::json!({
        "ids": ids.iter().map(|i| i.0.clone()).collect::<Vec<_>>(),
        "addLabelIds": add,
        "removeLabelIds": remove,
    });
    serde_json::to_vec(&value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_fetch_cannot_ask_for_a_whole_message() {
        // The rule this module exists to keep, checked against the string that carries it.
        let target = envelope_target(&RemoteMessageId("abc".into()));
        assert!(target.contains("format=metadata"), "{target}");
        assert!(!target.contains("format=full"), "{target}");
        assert!(!target.contains("format=raw"), "{target}");
        for header in ENVELOPE_HEADERS {
            assert!(
                target.contains(&format!("metadataHeaders={header}")),
                "{header}"
            );
        }
    }

    #[test]
    fn the_structure_fetch_leaves_attachments_behind() {
        // format=full returns small parts inline and attachments by identifier, which is
        // what "structure first" means on this provider.
        let target = structure_target(&RemoteMessageId("abc".into()));
        assert!(target.ends_with("?format=full"));
        assert!(
            !target.contains("format=raw"),
            "a whole message was requested"
        );
    }

    #[test]
    fn an_identifier_cannot_carry_a_second_query_parameter() {
        let target = list_target(&RemoteFolderId("INBOX&maxResults=99999".into()), None, 500);
        assert!(
            target.contains("labelIds=INBOX%26maxResults%3D99999"),
            "{target}"
        );
        assert_eq!(target.matches("maxResults=").count(), 1, "{target}");
    }

    #[test]
    fn the_two_folders_that_are_otherwise_hidden_ask_to_see_themselves() {
        assert!(
            list_target(&RemoteFolderId(crate::label::TRASH.into()), None, 500)
                .contains("includeSpamTrash=true")
        );
        assert!(
            list_target(&RemoteFolderId(crate::label::SPAM.into()), None, 500)
                .contains("includeSpamTrash=true")
        );
        assert!(
            !list_target(&RemoteFolderId("INBOX".into()), None, 500).contains("includeSpamTrash")
        );
    }

    #[test]
    fn a_page_token_continues_the_same_walk() {
        let target = list_target(&RemoteFolderId("INBOX".into()), Some("tok+en/="), 500);
        assert!(target.contains("pageToken=tok%2Ben%2F%3D"), "{target}");
    }

    #[test]
    fn a_history_identifier_is_read_whether_it_is_written_as_a_string_or_a_number() {
        assert_eq!(
            parse_history_id(br#"{"historyId":"12345"}"#).unwrap(),
            "12345"
        );
        assert_eq!(
            parse_history_id(br#"{"historyId":12345}"#).unwrap(),
            "12345"
        );
    }

    #[test]
    fn a_captive_portals_sign_in_page_is_malformed_rather_than_authoritative() {
        // D-88's classifier turns on exactly this: anything that does not parse as the
        // provider's own document is evidence about the network, not about the account.
        assert_eq!(
            parse_history_id(b"<html>Sign in to continue</html>"),
            Err(Refusal::Malformed("the answer was not JSON"))
        );
    }

    #[test]
    fn a_well_formed_denial_carries_the_providers_own_words() {
        let body = br#"{"error":{"code":403,"message":"Insufficient Permission"}}"#;
        assert_eq!(
            refusal(403, body),
            Refusal::Denied {
                status: 403,
                message: "Insufficient Permission".into()
            }
        );
    }

    #[test]
    fn labels_split_into_the_axes_they_belong_to() {
        let body = br#"{"labels":[
            {"id":"INBOX","name":"INBOX","type":"system"},
            {"id":"UNREAD","name":"UNREAD","type":"system"},
            {"id":"Label_9","name":"Receipts","type":"user"}
        ]}"#;
        let labels = parse_labels(body).unwrap();
        assert_eq!(labels.len(), 3);
        let folders: Vec<_> = labels.iter().filter_map(Label::as_folder).collect();
        assert_eq!(folders.len(), 1, "the read state or a tag became a folder");
        assert_eq!(folders[0].id.0, "INBOX");
    }

    #[test]
    fn an_empty_folder_is_a_page_of_nothing_rather_than_a_malformed_answer() {
        let page = parse_list(br#"{"resultSizeEstimate":0}"#).unwrap();
        assert!(page.ids.is_empty());
        assert_eq!(page.next_page, None);
    }

    #[test]
    fn a_list_page_carries_its_continuation() {
        let page =
            parse_list(br#"{"messages":[{"id":"a"},{"id":"b"}],"nextPageToken":"next"}"#).unwrap();
        assert_eq!(page.ids.len(), 2);
        assert_eq!(page.next_page.as_deref(), Some("next"));
    }

    fn inbox() -> RemoteFolderId {
        RemoteFolderId("INBOX".into())
    }

    #[test]
    fn an_arrival_is_delivered_and_a_label_move_is_only_discovered() {
        // The distinction FR-23 needs three phases from now and cannot reconstruct.
        let body = br#"{"history":[
            {"id":"1","messagesAdded":[{"message":{"id":"new"}}]},
            {"id":"2","labelsAdded":[{"message":{"id":"moved"},"labelIds":["INBOX"]}]}
        ],"historyId":"2"}"#;
        let page = parse_history(body, &inbox()).unwrap();
        assert_eq!(
            page.changes,
            vec![
                Change::Present {
                    id: RemoteMessageId("new".into()),
                    provenance: Provenance::Delivered
                },
                Change::Present {
                    id: RemoteMessageId("moved".into()),
                    provenance: Provenance::Discovered
                },
            ]
        );
        assert_eq!(page.history_id.as_deref(), Some("2"));
    }

    #[test]
    fn a_read_state_change_is_not_mistaken_for_a_move() {
        let body = br#"{"history":[
            {"id":"1","labelsRemoved":[{"message":{"id":"m"},"labelIds":["UNREAD"]}]}
        ]}"#;
        let page = parse_history(body, &inbox()).unwrap();
        assert_eq!(
            page.changes,
            vec![Change::FlagsChanged {
                id: RemoteMessageId("m".into())
            }]
        );
    }

    #[test]
    fn leaving_this_folder_is_a_removal() {
        let body = br#"{"history":[
            {"id":"1","labelsRemoved":[{"message":{"id":"m"},"labelIds":["INBOX"]}]}
        ]}"#;
        assert_eq!(
            parse_history(body, &inbox()).unwrap().changes,
            vec![Change::Removed {
                id: RemoteMessageId("m".into())
            }]
        );
    }

    #[test]
    fn two_changes_to_one_message_stay_two_changes_in_order() {
        // Collapsing them would leave the store holding whichever the collapse kept.
        let body = br#"{"history":[
            {"id":"1","messagesAdded":[{"message":{"id":"m"}}]},
            {"id":"2","labelsRemoved":[{"message":{"id":"m"},"labelIds":["INBOX"]}]}
        ]}"#;
        let changes = parse_history(body, &inbox()).unwrap().changes;
        assert_eq!(changes.len(), 2);
        assert!(matches!(changes[0], Change::Present { .. }));
        assert!(matches!(changes[1], Change::Removed { .. }));
    }

    #[test]
    fn a_history_page_with_no_change_at_all_is_ordinary() {
        let page = parse_history(br#"{"historyId":"77"}"#, &inbox()).unwrap();
        assert!(page.changes.is_empty());
        assert_eq!(page.history_id.as_deref(), Some("77"));
    }

    const MESSAGE: &[u8] = br#"{
        "id":"18f",
        "threadId":"18e",
        "labelIds":["INBOX","UNREAD","STARRED","Label_9","CATEGORY_UPDATES"],
        "snippet":"a snippet",
        "internalDate":"1788179696000",
        "sizeEstimate":4096,
        "payload":{"headers":[
            {"name":"Subject","value":"A subject"},
            {"name":"From","value":"Someone <SOMEONE@Example.TEST>"},
            {"name":"To","value":"a@x.test, b@y.test"},
            {"name":"Date","value":"Mon, 31 Aug 2026 12:34:56 +0000"},
            {"name":"Message-ID","value":"<abc@example.test>"},
            {"name":"References","value":"<one@x.test> <two@x.test>"},
            {"name":"In-Reply-To","value":"<two@x.test>"}
        ]}
    }"#;

    #[test]
    fn a_message_resource_becomes_an_envelope() {
        let e = parse_envelope(MESSAGE).unwrap();
        assert_eq!(e.id.0, "18f");
        assert_eq!(e.thread_id.as_deref(), Some("18e"));
        assert_eq!(e.subject.as_deref(), Some("A subject"));
        assert_eq!(e.from.as_deref(), Some("someone@example.test"));
        assert_eq!(e.to, vec!["a@x.test".to_owned(), "b@y.test".to_owned()]);
        assert_eq!(e.internet_message_id.as_deref(), Some("<abc@example.test>"));
        assert_eq!(e.snippet.as_deref(), Some("a snippet"));
        assert_eq!(e.size_estimate, 4096);
    }

    #[test]
    fn the_sort_key_is_the_servers_time_and_the_header_is_only_displayed() {
        // D-55. They agree in this fixture, and the point is that they are read from
        // different places so that a lying header cannot reorder the list.
        let e = parse_envelope(MESSAGE).unwrap();
        assert_eq!(e.received_at_millis, 1_788_179_696_000);
        assert_eq!(e.origination_date_millis, Some(1_788_179_696_000));
    }

    #[test]
    fn a_lying_date_header_does_not_move_the_message_in_the_list() {
        let body = br#"{"id":"x","internalDate":"1000","payload":{"headers":[
            {"name":"Date","value":"Fri, 1 Jan 2100 00:00:00 +0000"}]}}"#;
        let e = parse_envelope(body).unwrap();
        assert_eq!(e.received_at_millis, 1000, "the sender chose the sort key");
        assert!(e.origination_date_millis.unwrap() > 4_000_000_000_000);
    }

    #[test]
    fn the_reference_chain_keeps_its_order_and_does_not_repeat_the_parent() {
        let e = parse_envelope(MESSAGE).unwrap();
        assert_eq!(
            e.references,
            vec!["<one@x.test>".to_owned(), "<two@x.test>".to_owned()]
        );
    }

    #[test]
    fn a_parent_absent_from_the_chain_is_appended_to_it() {
        let body = br#"{"id":"x","payload":{"headers":[
            {"name":"References","value":"<one@x.test>"},
            {"name":"In-Reply-To","value":"<other@x.test>"}]}}"#;
        assert_eq!(
            parse_envelope(body).unwrap().references,
            vec!["<one@x.test>".to_owned(), "<other@x.test>".to_owned()]
        );
    }

    #[test]
    fn the_read_and_flagged_axes_come_off_the_labels_that_own_them() {
        let e = parse_envelope(MESSAGE).unwrap();
        assert!(!e.read, "the unread label was present");
        assert!(e.flagged);
    }

    #[test]
    fn only_the_location_labels_become_folders() {
        // A message is in several places at once, and neither its read state nor a user tag
        // is one of them.
        let e = parse_envelope(MESSAGE).unwrap();
        assert_eq!(
            e.folders,
            vec![
                RemoteFolderId("INBOX".into()),
                RemoteFolderId("CATEGORY_UPDATES".into())
            ]
        );
        assert_eq!(
            e.tags,
            vec!["Label_9".to_owned()],
            "a user label became a place, or an axis became a tag"
        );
    }

    #[test]
    fn a_message_with_no_headers_at_all_still_parses() {
        // Every field but the identifier is optional, because a provider that omits one
        // must not make the message unreadable.
        let e = parse_envelope(br#"{"id":"x"}"#).unwrap();
        assert_eq!(e.id.0, "x");
        assert_eq!(e.subject, None);
        assert!(e.read, "absence of the unread label means read");
    }

    #[test]
    fn a_message_with_no_identifier_is_refused() {
        assert!(parse_envelope(br#"{"snippet":"orphan"}"#).is_err());
    }

    #[test]
    fn a_structure_flattens_and_keeps_attachments_by_reference() {
        let body = br#"{"id":"x","payload":{
            "partId":"","mimeType":"multipart/mixed","body":{"size":0},
            "parts":[
              {"partId":"0","mimeType":"text/html","body":{"size":11,"data":"PGI-aGk8L2I-"}},
              {"partId":"1","mimeType":"application/pdf","filename":"r.pdf",
               "body":{"size":40000000,"attachmentId":"ANGjdJ"}}
            ]}}"#;
        let parts = parse_structure(body).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].inline.as_deref(), Some(&b"<b>hi</b>"[..]));
        assert_eq!(parts[2].attachment.as_deref(), Some("ANGjdJ"));
        assert!(
            parts[2].inline.is_none(),
            "40 MB arrived without anybody asking"
        );
        assert_eq!(parts[2].filename.as_deref(), Some("r.pdf"));
    }

    #[test]
    fn a_batch_request_is_one_part_per_target() {
        let body = batch_request_body(&["/a".into(), "/b".into()]);
        let text = String::from_utf8(body).unwrap();
        assert_eq!(text.matches("Content-Type: application/http").count(), 2);
        assert!(text.contains("Content-ID: <item-0>"));
        assert!(text.contains("GET /a\r\n"));
        assert!(text.ends_with(&format!("--{BATCH_BOUNDARY}--\r\n")));
    }

    #[test]
    fn a_batch_answer_is_matched_by_identifier_and_not_by_position() {
        // The provider does not promise order, and matching on position would attach one
        // message's envelope to another's identifier.
        let raw = b"--b\r\n\
Content-Type: application/http\r\n\
Content-ID: <response-item-1>\r\n\
\r\n\
HTTP/1.1 200 OK\r\n\
Content-Type: application/json\r\n\
\r\n\
{\"id\":\"second\"}\r\n\
\r\n\
--b\r\n\
Content-Type: application/http\r\n\
Content-ID: <response-item-0>\r\n\
\r\n\
HTTP/1.1 404 Not Found\r\n\
\r\n\
{\"error\":{}}\r\n\
\r\n\
--b--\r\n";
        let items = parse_batch(raw, "b");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].index, Some(1));
        assert_eq!(items[0].status, 200);
        assert_eq!(items[0].body, b"{\"id\":\"second\"}");
        assert_eq!(items[1].index, Some(0));
        assert_eq!(items[1].status, 404);
    }

    #[test]
    fn the_boundary_is_read_off_the_answers_own_content_type() {
        assert_eq!(
            boundary_of("multipart/mixed; boundary=batch_xyz").as_deref(),
            Some("batch_xyz")
        );
        assert_eq!(
            boundary_of("multipart/mixed; boundary=\"quoted\"; charset=utf-8").as_deref(),
            Some("quoted")
        );
        assert_eq!(boundary_of("application/json"), None);
    }

    #[test]
    fn a_batch_answer_that_is_not_multipart_yields_nothing_rather_than_guessing() {
        assert!(parse_batch(b"{\"error\":{}}", "b").is_empty());
    }

    #[test]
    fn a_label_change_states_both_directions_in_one_request() {
        let body = batch_modify_body(
            &[RemoteMessageId("a".into()), RemoteMessageId("b".into())],
            &["Label_9".into()],
            &["INBOX".into()],
        );
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["ids"], serde_json::json!(["a", "b"]));
        assert_eq!(value["addLabelIds"], serde_json::json!(["Label_9"]));
        assert_eq!(value["removeLabelIds"], serde_json::json!(["INBOX"]));
    }

    #[test]
    fn base64url_bodies_decode_with_or_without_the_padding_the_provider_omits() {
        assert_eq!(decode_base64url("PGI-aGk8L2I-").unwrap(), b"<b>hi</b>");
        assert_eq!(decode_base64url("aGk=").unwrap(), b"hi");
        assert!(decode_base64url("not base64!!").is_err());
    }
}

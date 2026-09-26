//! What Sift asks this provider for, and how it reads the answers.
//!
//! Everything here is a pure function over bytes, which is what makes D-65's fixture corpus
//! worth having: a request builder can be asserted on without a socket, and "Sift MUST NOT
//! fetch whole messages" is a claim about a *target string* that a test can read.
//!
//! # Four rules from `docs/mail/providers/microsoft-graph.md`, made mechanical
//!
//! 1. **Every read names its fields.** [`ENVELOPE_SELECT`] is the whole of what an envelope
//!    fetch asks for, and no target here carries `$expand` — the option that turns a list of
//!    messages into a list of messages *with their attachments' bytes*.
//! 2. **The delta is the only path by which change is applied.** Nothing else in this module
//!    produces a [`Change`].
//! 3. **An expired delta link is a recovery, not an error.** It leaves here as
//!    [`Refusal::CursorInvalidated`], never as a denial.
//! 4. **A paging link is followed only to the host it was issued by.** The provider hands back
//!    absolute URLs; one naming any other host is refused rather than fetched, because the
//!    bearer token travels with every request this adapter makes.

use sift_provider::adapter::{Change, Envelope, Provenance, RemoteFolderId, RemoteMessageId};

use crate::folder::Folder;

/// The one host this adapter speaks to. Consumer and organizational accounts share it.
pub const API_HOST: &str = "graph.microsoft.com";

/// The version every direct request is rooted at. Requests inside a batch are relative to it.
pub const ROOT: &str = "/v1.0";

/// The published cap on requests inside one batch.
///
/// The capability table still declares the batch size *unknown* (Q-9 is open for this
/// provider), and an unknown magnitude plans against the conservative default — which is
/// this same number. The adapter chunks to it regardless, so that a caller planning against
/// a larger figure cannot build a batch the provider refuses whole.
pub const BATCH_LIMIT: usize = 20;

/// The fields an envelope fetch asks for, and the whole of it.
///
/// Each is needed by something named: the list row, FR-37's tags, the read and flagged axes,
/// D-55's sort key, and D-44 — whose join is scoped on `conversationId`, narrowed by
/// `internetMessageId`, and corroborated by a digest over the sender, the origination date
/// and the subject.
///
/// **The reference chain is not here.** The published resource exposes `References` and
/// `In-Reply-To` only inside the whole header block, and asking for that block on every
/// envelope would cost more than the rest of the envelope together. Threading does not need
/// it — this provider supplies a native conversation identifier, which FR-11 prefers — and a
/// digest computed without it is computed without it on both sides of every move, so it still
/// corroborates like with like.
pub const ENVELOPE_SELECT: &[&str] = &[
    "id",
    "conversationId",
    "internetMessageId",
    "subject",
    "from",
    "toRecipients",
    "receivedDateTime",
    "sentDateTime",
    "bodyPreview",
    "parentFolderId",
    "categories",
    "isRead",
    "flag",
];

/// What a folder listing asks for.
pub const FOLDER_SELECT: &[&str] = &["id", "displayName", "parentFolderId", "childFolderCount"];

/// What an attachment listing asks for. **Not `contentBytes`**: the listing is the structure,
/// and the bytes are fetched one attachment at a time only when somebody asks.
pub const ATTACHMENT_SELECT: &[&str] = &["id", "name", "contentType", "size", "isInline"];

/// What a delta page asks for: the identifier, and nothing the envelope fetch will ask again.
pub const DELTA_SELECT: &[&str] = &["id"];

/// How many folders a listing page asks for.
pub const FOLDER_PAGE: u32 = 100;

/// The body part identifiers [`crate::Graph::structure`] describes.
///
/// The provider exposes one body per message rather than a MIME tree, and converts between
/// the two representations on request. Both are offered so that stage 2's choice is made
/// above the adapter, where it is made for every provider alike.
pub const BODY_HTML: &str = "body.html";
pub const BODY_TEXT: &str = "body.text";

/// The prefix of an attachment's part identifier.
pub const ATTACHMENT_PART: &str = "attachment:";

/// What went wrong that is not a transport failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The answer did not parse as this provider's own JSON.
    Malformed(&'static str),
    /// The stored delta link has expired.
    ///
    /// **Not an error.** D-82 moves the folder to `Invalidated` and NFR-18's recovery
    /// follows without asking the user anything.
    CursorInvalidated,
    /// The provider refused in its own terms, with its own message.
    Denied { status: u16, message: String },
}

// ---------------------------------------------------------------------------
// Targets. Every request Sift makes of this provider is one of these.
//
// Each is written relative to the version root, because that is the form a batch takes;
// [`rooted`] turns one into the form a direct request takes.
// ---------------------------------------------------------------------------

/// A relative target, as a direct request sends it.
#[must_use]
pub fn rooted(target: &str) -> String {
    format!("{ROOT}{target}")
}

fn select(fields: &[&str]) -> String {
    format!("$select={}", fields.join(","))
}

/// The account's top-level folders.
#[must_use]
pub fn folders_target() -> String {
    format!(
        "/me/mailFolders?{}&$top={FOLDER_PAGE}",
        select(FOLDER_SELECT)
    )
}

/// The folders directly beneath one folder.
#[must_use]
pub fn child_folders_target(parent: &str) -> String {
    format!(
        "/me/mailFolders/{}/childFolders?{}&$top={FOLDER_PAGE}",
        encode(parent),
        select(FOLDER_SELECT)
    )
}

/// The folder a well-known name denotes on this account.
#[must_use]
pub fn well_known_target(name: &str) -> String {
    format!("/me/mailFolders/{}?$select=id", encode(name))
}

/// The start of a folder's delta walk. Every later page is a link the provider issued.
#[must_use]
pub fn delta_target(folder: &RemoteFolderId) -> String {
    format!(
        "/me/mailFolders/{}/messages/delta?{}",
        encode(&folder.0),
        select(DELTA_SELECT)
    )
}

/// One envelope. **Named fields only** — the rule this module exists to keep.
#[must_use]
pub fn envelope_target(id: &RemoteMessageId) -> String {
    format!("/me/messages/{}?{}", encode(&id.0), select(ENVELOPE_SELECT))
}

/// One message's categories, which a tag change has to read before it writes: the provider
/// replaces the list whole rather than adding to it.
#[must_use]
pub fn categories_target(id: &RemoteMessageId) -> String {
    format!("/me/messages/{}?$select=categories", encode(&id.0))
}

/// One message, as a property update addresses it.
#[must_use]
pub fn message_target(id: &RemoteMessageId) -> String {
    format!("/me/messages/{}", encode(&id.0))
}

#[must_use]
pub fn move_target(id: &RemoteMessageId) -> String {
    format!("/me/messages/{}/move", encode(&id.0))
}

#[must_use]
pub fn permanent_delete_target(id: &RemoteMessageId) -> String {
    format!("/me/messages/{}/permanentDelete", encode(&id.0))
}

/// A message's attachments, **described rather than fetched**.
#[must_use]
pub fn attachments_target(id: &RemoteMessageId) -> String {
    format!(
        "/me/messages/{}/attachments?{}",
        encode(&id.0),
        select(ATTACHMENT_SELECT)
    )
}

/// One attachment's bytes, raw.
#[must_use]
pub fn attachment_value_target(id: &RemoteMessageId, attachment: &str) -> String {
    format!(
        "/me/messages/{}/attachments/{}/$value",
        encode(&id.0),
        encode(attachment)
    )
}

/// One message's body — the part stage 2 chose, and nothing beside it.
#[must_use]
pub fn body_target(id: &RemoteMessageId) -> String {
    format!("/me/messages/{}?$select=body", encode(&id.0))
}

/// The batch envelope. Not a schema endpoint: the requests inside it are.
#[must_use]
pub fn batch_target() -> String {
    rooted("/$batch")
}

/// The preference header a body fetch sends, which decides the representation returned.
#[must_use]
pub fn body_preference(part: &str) -> Option<&'static str> {
    match part {
        BODY_HTML => Some("outlook.body-content-type=\"html\""),
        BODY_TEXT => Some("outlook.body-content-type=\"text\""),
        _ => None,
    }
}

/// The preference header a delta page sends: L-26's page size, as a ceiling the provider
/// may answer below.
#[must_use]
pub fn page_preference(page_size: u32) -> String {
    format!("odata.maxpagesize={page_size}")
}

/// Percent-encode a path component.
///
/// Identifiers here are the provider's, but a folder identifier reaches a path from a stored
/// row, and one that could carry a `/` or a `?` into a target is a request the caller did not
/// write.
#[must_use]
pub fn encode(value: &str) -> String {
    sift_provider::oauth::form_encode(value)
}

/// Turn a paging or delta link the provider issued into a target this adapter will send.
///
/// # Errors
/// [`Refusal::Malformed`] where the link names another host, another scheme, or another
/// version. **The bearer token travels with every request**, so a link that pointed anywhere
/// else would hand it to whoever wrote the link.
pub fn link_target(link: &str) -> Result<String, Refusal> {
    let path = link
        .strip_prefix("https://")
        .and_then(|rest| rest.strip_prefix(API_HOST))
        .ok_or(Refusal::Malformed(
            "a paging link named a host other than the provider's",
        ))?;
    if path.starts_with(&format!("{ROOT}/")) {
        Ok(path.to_owned())
    } else {
        Err(Refusal::Malformed(
            "a paging link left the version this adapter speaks",
        ))
    }
}

// ---------------------------------------------------------------------------
// Answers.
// ---------------------------------------------------------------------------

fn json(bytes: &[u8]) -> Result<serde_json::Value, Refusal> {
    serde_json::from_slice(bytes).map_err(|_| Refusal::Malformed("the answer was not JSON"))
}

fn error_field<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    value.get("error")?.get(field)?.as_str()
}

/// Read the provider's own error document.
///
/// D-88's classifier turns on this distinction: a **well-formed** provider error is
/// authoritative, and anything that does not parse as one is evidence about the network
/// rather than about the account.
#[must_use]
pub fn refusal(status: u16, body: &[u8]) -> Refusal {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| error_field(&v, "message").map(str::to_owned))
        .unwrap_or_else(|| "the provider refused and did not say why".to_owned());
    Refusal::Denied { status, message }
}

/// The error codes by which the provider says a delta link's state is gone.
const SYNC_STATE_GONE: &[&str] = &["syncstatenotfound", "syncstateinvalid", "resyncrequired"];

/// Whether a refused delta page means the stored link has expired.
///
/// **410 always does**, and so does any status whose well-formed error names the sync state.
/// A 404 does **not**, on its own: that is the folder being gone, which is D-83's retirement
/// rather than a cursor to recover.
#[must_use]
pub fn delta_link_expired(status: u16, body: &[u8]) -> bool {
    if status == 410 {
        return true;
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| error_field(&v, "code").map(str::to_ascii_lowercase))
        .is_some_and(|code| SYNC_STATE_GONE.contains(&code.as_str()))
}

fn string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn array<'a>(value: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// One page of a folder listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderPage {
    pub folders: Vec<Folder>,
    /// The next page, as a target this adapter will send.
    pub next: Option<String>,
}

/// # Errors
/// [`Refusal::Malformed`] where the answer is not a folder listing, or where its paging link
/// leaves the provider.
pub fn parse_folders(body: &[u8]) -> Result<FolderPage, Refusal> {
    let value = json(body)?;
    let items = value
        .get("value")
        .and_then(|v| v.as_array())
        .ok_or(Refusal::Malformed("the answer carried no folders"))?;
    let folders = items
        .iter()
        .filter_map(|f| {
            let id = string(f, "id")?;
            Some(Folder {
                display_name: string(f, "displayName").unwrap_or_else(|| id.clone()),
                parent: string(f, "parentFolderId"),
                children: f
                    .get("childFolderCount")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                id,
            })
        })
        .collect();
    Ok(FolderPage {
        folders,
        next: next_link(&value)?,
    })
}

fn next_link(value: &serde_json::Value) -> Result<Option<String>, Refusal> {
    value
        .get("@odata.nextLink")
        .and_then(|l| l.as_str())
        .map(link_target)
        .transpose()
}

/// The identifier a well-known name answered with.
#[must_use]
pub fn parse_folder_id(value: &serde_json::Value) -> Option<String> {
    string(value, "id")
}

/// One page of a delta walk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeltaPage {
    pub changes: Vec<Change>,
    /// The next page of this round, where one follows.
    pub next: Option<String>,
    /// The link that starts the next round, where this page ended one.
    pub delta: Option<String>,
}

/// Read one page of a folder's delta, in the order the provider reported it.
///
/// `live` says whether the folder has finished its first round. That is the whole of the
/// provenance rule here: **the first round discovers, and only a later round delivers** —
/// FR-23 defines new mail as delivered, and a first round that delivered would announce the
/// user's entire mailbox the first time they added an account.
///
/// A later round cannot tell an arrival from a change to a message already held: the provider
/// reports both as the message, with no word for which. So each is reported **twice** —
/// present, so an arrival is inserted, and flags-changed, so the envelope of one already held
/// is fetched again and its read and flagged state is not left stale. Applying both is safe
/// for the reason every reapplication is: the store keeps the provenance a message first
/// arrived with, and a presence of something already held is an update.
///
/// **Order is preserved rather than deduplicated.** A message that left and came back inside
/// one page is two changes, and collapsing them would keep whichever the collapse chose.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer is not a delta page, where it ends neither in a
/// next link nor a delta link, or where either link leaves the provider.
pub fn parse_delta(body: &[u8], live: bool) -> Result<DeltaPage, Refusal> {
    let value = json(body)?;
    let items = value
        .get("value")
        .and_then(|v| v.as_array())
        .ok_or(Refusal::Malformed("the answer carried no delta"))?;
    let mut changes = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = string(item, "id").map(RemoteMessageId) else {
            continue;
        };
        // Deleted and moved-out look the same from inside the folder, and are the same to it:
        // the message is no longer here. Under unstable identifiers a move is exactly this
        // plus an arrival somewhere else, and D-44 is what rejoins the two.
        if item.get("@removed").is_some() {
            changes.push(Change::Removed { id });
        } else if live {
            changes.push(Change::Present {
                id: id.clone(),
                provenance: Provenance::Delivered,
            });
            changes.push(Change::FlagsChanged { id });
        } else {
            changes.push(Change::Present {
                id,
                provenance: Provenance::Discovered,
            });
        }
    }
    let next = next_link(&value)?;
    let delta = value
        .get("@odata.deltaLink")
        .and_then(|l| l.as_str())
        .map(link_target)
        .transpose()?;
    if next.is_none() && delta.is_none() {
        return Err(Refusal::Malformed(
            "a delta page ended in neither a next link nor a delta link",
        ));
    }
    Ok(DeltaPage {
        changes,
        next,
        delta,
    })
}

/// A message resource's categories — the tag axis, by name.
#[must_use]
pub fn parse_categories(value: &serde_json::Value) -> Vec<String> {
    array(value, "categories")
        .iter()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect()
}

fn address(value: Option<&serde_json::Value>) -> Option<String> {
    value?
        .get("emailAddress")?
        .get("address")?
        .as_str()
        .map(str::to_owned)
}

/// Read a message resource into an envelope.
///
/// # Errors
/// [`Refusal::Malformed`] where the resource carries no identifier.
pub fn parse_envelope(value: &serde_json::Value) -> Result<Envelope, Refusal> {
    let id = string(value, "id").ok_or(Refusal::Malformed("a message carried no identifier"))?;
    Ok(Envelope {
        id: RemoteMessageId(id),
        // The provider's own conversation identifier: FR-11's preference, and D-44's scope.
        thread_id: string(value, "conversationId"),
        // D-44: it narrows and never keys.
        internet_message_id: string(value, "internetMessageId"),
        references: Vec::new(),
        subject: string(value, "subject"),
        from: address(value.get("from")),
        to: array(value, "toRecipients")
            .iter()
            .filter_map(|r| address(Some(r)))
            .collect(),
        // The server's own received time. D-55 orders on this and never on the sender's.
        received_at_millis: value
            .get("receivedDateTime")
            .and_then(|d| d.as_str())
            .and_then(iso_millis)
            .unwrap_or(0),
        origination_date_millis: value
            .get("sentDateTime")
            .and_then(|d| d.as_str())
            .and_then(iso_millis),
        snippet: string(value, "bodyPreview"),
        // Exactly one location. The capability says so, and a list of one is how the
        // interface spells it.
        folders: string(value, "parentFolderId")
            .map(RemoteFolderId)
            .into_iter()
            .collect(),
        tags: parse_categories(value),
        read: value
            .get("isRead")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        // `complete` is a flag the user has finished with. The flagged axis is the one still
        // asking for attention.
        flagged: value
            .get("flag")
            .and_then(|f| f.get("flagStatus"))
            .and_then(|s| s.as_str())
            == Some("flagged"),
        // The published resource carries no size. Zero is "not stated", which is what the
        // interface's advisory field allows; L-13 bounds the transfer regardless.
        size_estimate: 0,
    })
}

/// One attachment as the listing describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub name: Option<String>,
    pub media_type: String,
    pub size: u64,
}

/// One page of an attachment listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttachmentPage {
    pub attachments: Vec<Attachment>,
    pub next: Option<String>,
}

/// # Errors
/// [`Refusal::Malformed`] where the answer is not an attachment listing.
pub fn parse_attachments(body: &[u8]) -> Result<AttachmentPage, Refusal> {
    let value = json(body)?;
    let items = value
        .get("value")
        .and_then(|v| v.as_array())
        .ok_or(Refusal::Malformed("the answer carried no attachments"))?;
    let attachments = items
        .iter()
        .filter_map(|a| {
            let kind = a.get("@odata.type").and_then(|t| t.as_str()).unwrap_or("");
            // A reference attachment is a link to a file somewhere else. It has no bytes here
            // to fetch, and fetching the link would be a network request on the sender's
            // behalf — the thing the broker exists to decide.
            if kind.ends_with("referenceAttachment") {
                return None;
            }
            let fallback = if kind.ends_with("itemAttachment") {
                "message/rfc822"
            } else {
                "application/octet-stream"
            };
            Some(Attachment {
                id: string(a, "id")?,
                name: string(a, "name"),
                media_type: string(a, "contentType")
                    .unwrap_or_else(|| fallback.to_owned())
                    .to_ascii_lowercase(),
                size: a
                    .get("size")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            })
        })
        .collect();
    Ok(AttachmentPage {
        attachments,
        next: next_link(&value)?,
    })
}

/// The body a body fetch answered with, as bytes.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer carries no body.
pub fn parse_body(body: &[u8]) -> Result<Vec<u8>, Refusal> {
    json(body)?
        .get("body")
        .and_then(|b| b.get("content"))
        .and_then(|c| c.as_str())
        .map(|c| c.as_bytes().to_vec())
        .ok_or(Refusal::Malformed("the message carried no body"))
}

// ---------------------------------------------------------------------------
// The batch envelope.
// ---------------------------------------------------------------------------

/// One request inside a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchRequest {
    pub method: &'static str,
    /// Relative to the version root.
    pub url: String,
    pub body: Option<serde_json::Value>,
    pub prefer: Option<String>,
}

impl BatchRequest {
    #[must_use]
    pub fn get(url: String) -> Self {
        Self {
            method: "GET",
            url,
            body: None,
            prefer: None,
        }
    }

    #[must_use]
    pub fn with_body(method: &'static str, url: String, body: serde_json::Value) -> Self {
        Self {
            method,
            url,
            body: Some(body),
            prefer: None,
        }
    }
}

/// The body of a batch: one request per element, **identified by its index**.
///
/// The provider answers a batch in whatever order it finished the work, so the index is the
/// only thing that ties an answer back to what asked for it.
#[must_use]
pub fn batch_body(requests: &[BatchRequest]) -> Vec<u8> {
    let items: Vec<serde_json::Value> = requests
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let mut item = serde_json::Map::new();
            item.insert("id".into(), serde_json::Value::from(index.to_string()));
            item.insert("method".into(), serde_json::Value::from(request.method));
            item.insert("url".into(), serde_json::Value::from(request.url.clone()));
            let mut headers = serde_json::Map::new();
            if let Some(body) = &request.body {
                item.insert("body".into(), body.clone());
                headers.insert(
                    "Content-Type".into(),
                    serde_json::Value::from("application/json"),
                );
            }
            if let Some(prefer) = &request.prefer {
                headers.insert("Prefer".into(), serde_json::Value::from(prefer.clone()));
            }
            if !headers.is_empty() {
                item.insert("headers".into(), serde_json::Value::Object(headers));
            }
            serde_json::Value::Object(item)
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({ "requests": items })).unwrap_or_default()
}

/// One answer inside a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchItem {
    /// The index of the request this answers.
    pub index: usize,
    pub status: u16,
    /// The element's own `Retry-After`, in milliseconds. A batch is throttled per element.
    pub retry_after_millis: Option<u64>,
    pub body: serde_json::Value,
}

/// Read a batch answer. **Per element, never for the batch**: a batch that half-succeeds is
/// normal, and the envelope's own status says only that the envelope was read.
///
/// An element whose identifier is not an index this adapter issued is dropped: an answer to a
/// question nobody asked is not something to believe.
///
/// # Errors
/// [`Refusal::Malformed`] where the answer is not a batch answer.
pub fn parse_batch(body: &[u8], asked: usize) -> Result<Vec<BatchItem>, Refusal> {
    let value = json(body)?;
    let items = value
        .get("responses")
        .and_then(|r| r.as_array())
        .ok_or(Refusal::Malformed("the batch answer carried no responses"))?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let index = item.get("id")?.as_str()?.parse::<usize>().ok()?;
            if index >= asked {
                return None;
            }
            let status = u16::try_from(item.get("status")?.as_u64()?).ok()?;
            let retry_after_millis = item
                .get("headers")
                .and_then(|h| h.as_object())
                .and_then(|h| {
                    h.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                        .map(|(_, v)| v)
                })
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                })
                .and_then(|seconds: u64| seconds.checked_mul(1000));
            Some(BatchItem {
                index,
                status,
                retry_after_millis,
                body: item.get("body").cloned().unwrap_or_default(),
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Time.
// ---------------------------------------------------------------------------

/// An ISO 8601 instant, as the provider writes one, in milliseconds since the epoch.
///
/// The provider writes UTC with a trailing `Z` and an optional fraction. An offset form is
/// read too, because a timestamp this parser refused would sort the message to 1970.
#[must_use]
pub fn iso_millis(value: &str) -> Option<u64> {
    let value = value.trim();
    let (date, rest) = value.split_once('T')?;
    let mut date_fields = date.splitn(3, '-');
    let year: i64 = date_fields.next()?.parse().ok()?;
    let month: u32 = date_fields.next()?.parse().ok()?;
    let day: u32 = date_fields.next()?.parse().ok()?;

    let (clock, offset_seconds) = if let Some(clock) = rest.strip_suffix('Z') {
        (clock, 0i64)
    } else if let Some(at) = rest.rfind(['+', '-']) {
        let (clock, offset) = rest.split_at(at);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (h, m) = offset[1..].split_once(':')?;
        let minutes = h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?;
        (clock, sign * minutes * 60)
    } else {
        (rest, 0)
    };

    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut clock_fields = whole.splitn(3, ':');
    let hour: i64 = clock_fields.next()?.parse().ok()?;
    let minute: i64 = clock_fields.next()?.parse().ok()?;
    let second: i64 = clock_fields.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    let millis: i64 = fraction
        .chars()
        .chain(core::iter::repeat('0'))
        .take(3)
        .collect::<String>()
        .parse()
        .ok()?;

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    u64::try_from(seconds * 1_000 + millis).ok()
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_fetch_names_its_fields_and_never_expands() {
        let target = envelope_target(&RemoteMessageId("AAMk=".into()));
        assert!(target.contains("$select=id,conversationId,internetMessageId"));
        assert!(!target.contains("$expand"), "{target}");
        assert!(
            !target.contains("body,") && !target.ends_with(",body"),
            "an envelope asked for the body: {target}"
        );
    }

    #[test]
    fn no_target_expands() {
        let id = RemoteMessageId("m".into());
        for target in [
            folders_target(),
            child_folders_target("f"),
            well_known_target("inbox"),
            delta_target(&RemoteFolderId("f".into())),
            envelope_target(&id),
            categories_target(&id),
            attachments_target(&id),
            body_target(&id),
        ] {
            assert!(!target.contains("$expand"), "{target}");
        }
    }

    #[test]
    fn an_identifier_cannot_carry_a_second_path_segment_or_query() {
        let target = envelope_target(&RemoteMessageId("a/b?x=1".into()));
        assert!(
            target.starts_with("/me/messages/a%2Fb%3Fx%3D1?"),
            "{target}"
        );
    }

    #[test]
    fn the_attachment_listing_does_not_ask_for_the_bytes() {
        let target = attachments_target(&RemoteMessageId("m".into()));
        assert!(!target.contains("contentBytes"));
    }

    #[test]
    fn a_link_is_followed_only_to_the_provider() {
        assert_eq!(
            link_target("https://graph.microsoft.com/v1.0/me/mailFolders?$skip=10"),
            Ok("/v1.0/me/mailFolders?$skip=10".to_owned())
        );
        for hostile in [
            "https://evil.example/v1.0/me/mailFolders",
            "https://graph.microsoft.com.evil.example/v1.0/me",
            "http://graph.microsoft.com/v1.0/me",
            "https://graph.microsoft.com/beta/me/mailFolders",
            "/v1.0/me/mailFolders",
        ] {
            assert!(link_target(hostile).is_err(), "{hostile} was followed");
        }
    }

    #[test]
    fn a_captive_portals_page_is_malformed_rather_than_authoritative() {
        assert!(matches!(
            parse_delta(b"<html>Sign in</html>", true),
            Err(Refusal::Malformed(_))
        ));
    }

    #[test]
    fn a_well_formed_denial_carries_the_providers_own_words() {
        let r = refusal(
            403,
            br#"{"error":{"code":"ErrorAccessDenied","message":"Access is denied."}}"#,
        );
        assert_eq!(
            r,
            Refusal::Denied {
                status: 403,
                message: "Access is denied.".into()
            }
        );
    }

    #[test]
    fn an_expired_link_is_recognised_by_status_or_by_its_code_and_a_missing_folder_is_not() {
        assert!(delta_link_expired(410, b"{}"));
        assert!(delta_link_expired(
            400,
            br#"{"error":{"code":"SyncStateNotFound","message":"x"}}"#
        ));
        assert!(delta_link_expired(
            400,
            br#"{"error":{"code":"resyncRequired","message":"x"}}"#
        ));
        assert!(!delta_link_expired(
            404,
            br#"{"error":{"code":"ErrorItemNotFound","message":"x"}}"#
        ));
        assert!(!delta_link_expired(400, b"not json"));
    }

    #[test]
    fn the_first_round_discovers_and_a_later_round_delivers_and_refetches() {
        let body = br#"{"value":[{"id":"a"},{"id":"b","@removed":{"reason":"deleted"}}],
            "@odata.deltaLink":"https://graph.microsoft.com/v1.0/me/mailFolders/f/messages/delta?$deltatoken=t"}"#;
        let first = parse_delta(body, false).unwrap();
        assert_eq!(
            first.changes,
            vec![
                Change::Present {
                    id: RemoteMessageId("a".into()),
                    provenance: Provenance::Discovered
                },
                Change::Removed {
                    id: RemoteMessageId("b".into())
                },
            ]
        );
        let later = parse_delta(body, true).unwrap();
        assert_eq!(
            later.changes,
            vec![
                Change::Present {
                    id: RemoteMessageId("a".into()),
                    provenance: Provenance::Delivered
                },
                Change::FlagsChanged {
                    id: RemoteMessageId("a".into())
                },
                Change::Removed {
                    id: RemoteMessageId("b".into())
                },
            ]
        );
        assert_eq!(
            later.delta.as_deref(),
            Some("/v1.0/me/mailFolders/f/messages/delta?$deltatoken=t")
        );
        assert_eq!(later.next, None);
    }

    #[test]
    fn a_delta_page_with_no_way_forward_is_malformed() {
        assert!(parse_delta(br#"{"value":[]}"#, true).is_err());
    }

    #[test]
    fn a_delta_link_to_another_host_is_refused() {
        let body = br#"{"value":[],"@odata.deltaLink":"https://evil.example/v1.0/x"}"#;
        assert!(parse_delta(body, true).is_err());
    }

    #[test]
    fn a_message_resource_becomes_an_envelope() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"id":"m1","conversationId":"c1","internetMessageId":"<a@example.invalid>",
                "subject":"Hello","from":{"emailAddress":{"name":"A","address":"a@example.invalid"}},
                "toRecipients":[{"emailAddress":{"address":"b@example.invalid"}}],
                "receivedDateTime":"2026-09-01T10:00:00Z","sentDateTime":"2026-09-01T09:59:58Z",
                "bodyPreview":"Hi","parentFolderId":"inbox-id","categories":["Receipts"],
                "isRead":true,"flag":{"flagStatus":"flagged"}}"#,
        )
        .unwrap();
        let e = parse_envelope(&value).unwrap();
        assert_eq!(e.thread_id.as_deref(), Some("c1"));
        assert_eq!(
            e.internet_message_id.as_deref(),
            Some("<a@example.invalid>")
        );
        assert_eq!(e.from.as_deref(), Some("a@example.invalid"));
        assert_eq!(e.to, vec!["b@example.invalid".to_owned()]);
        assert_eq!(e.received_at_millis, 1_788_256_800_000);
        assert_eq!(e.origination_date_millis, Some(1_788_256_798_000));
        assert_eq!(e.folders, vec![RemoteFolderId("inbox-id".into())]);
        assert_eq!(e.tags, vec!["Receipts".to_owned()]);
        assert!(e.read && e.flagged);
    }

    #[test]
    fn a_completed_flag_is_not_flagged() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"id":"m1","flag":{"flagStatus":"complete"}}"#).unwrap();
        assert!(!parse_envelope(&value).unwrap().flagged);
    }

    #[test]
    fn a_message_with_no_identifier_is_refused() {
        assert!(parse_envelope(&serde_json::json!({"subject":"x"})).is_err());
    }

    #[test]
    fn timestamps_read_with_and_without_a_fraction_or_an_offset() {
        assert_eq!(iso_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_millis("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(iso_millis("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(
            iso_millis("2026-09-01T12:00:00+02:00"),
            iso_millis("2026-09-01T10:00:00Z")
        );
        assert_eq!(iso_millis("yesterday"), None);
        assert_eq!(iso_millis("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn a_batch_body_is_one_indexed_element_per_request() {
        let body = batch_body(&[
            BatchRequest::get("/me/messages/a".into()),
            BatchRequest::with_body(
                "PATCH",
                "/me/messages/b".into(),
                serde_json::json!({"isRead":true}),
            ),
        ]);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let requests = value["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["id"], "0");
        assert!(requests[0].get("headers").is_none());
        assert_eq!(requests[1]["id"], "1");
        assert_eq!(requests[1]["headers"]["Content-Type"], "application/json");
        assert_eq!(requests[1]["body"]["isRead"], true);
    }

    #[test]
    fn a_batch_answer_is_read_per_element_and_an_unasked_index_is_dropped() {
        let answer = br#"{"responses":[
            {"id":"1","status":429,"headers":{"Retry-After":"7"},"body":{}},
            {"id":"0","status":200,"body":{"id":"a"}},
            {"id":"9","status":200,"body":{"id":"z"}}
        ]}"#;
        let items = parse_batch(answer, 2).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].index, 1);
        assert_eq!(items[0].retry_after_millis, Some(7_000));
        assert_eq!(items[1].index, 0);
        assert_eq!(items[1].status, 200);
    }

    #[test]
    fn a_reference_attachment_is_not_described_as_bytes_to_fetch() {
        let page = parse_attachments(
            br##"{"value":[
                {"@odata.type":"#microsoft.graph.fileAttachment","id":"a1","name":"r.pdf","contentType":"Application/PDF","size":41943040,"isInline":false},
                {"@odata.type":"#microsoft.graph.referenceAttachment","id":"a2","name":"link","size":10},
                {"@odata.type":"#microsoft.graph.itemAttachment","id":"a3","name":"fwd","size":900}
            ]}"##,
        )
        .unwrap();
        let ids: Vec<&str> = page.attachments.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a1", "a3"]);
        assert_eq!(page.attachments[0].media_type, "application/pdf");
        assert_eq!(page.attachments[1].media_type, "message/rfc822");
    }
}

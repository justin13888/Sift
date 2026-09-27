//! The adapter itself: six responsibilities over one transport.

use core::cell::RefCell;
use sift_foundation::limits::{
    L1_BODY_PART_BYTES, L2_MIME_PARTS, L26_BACKFILL_PAGE, L32_SERVER_SEARCH_HITS,
};
use sift_provider::adapter::{
    Adapter, Cursor, Delta, Envelope, Failure, MutationOutcome, Operation, PartDescriptor,
    RemoteFolder, RemoteFolderId, RemoteMessageId, SearchTerm, SpecialUse, WireMutation,
};
use sift_provider::capability::Capabilities;
use sift_provider::transport::{Request, Response, Transport, TransportError};

use crate::folder::{self, Folder};
use crate::wire::{self, BatchItem, BatchRequest, Refusal};

/// What can go wrong, above the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// The wire failed. The scheduler decides what happens next, under D-87 — never this.
    Transport(TransportError),
    /// The provider answered, and the answer was not usable.
    Refusal(Refusal),
    /// The access token was refused.
    ///
    /// **Not a denied grant.** It says the presented token is not currently good, which is the
    /// ordinary state of an expired one; D-88's single-flight refresh runs in the credential
    /// broker. Only the refresh itself can conclude that the *grant* is gone.
    TokenRejected,
    /// Something this provider will not do.
    Unsupported(&'static str),
}

impl From<TransportError> for GraphError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<Refusal> for GraphError {
    fn from(e: Refusal) -> Self {
        Self::Refusal(e)
    }
}

impl core::fmt::Display for GraphError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "the wire failed: {e:?}"),
            Self::Refusal(Refusal::CursorInvalidated) => {
                write!(f, "the stored delta link has expired")
            }
            Self::Refusal(Refusal::Malformed(why)) => write!(f, "{why}"),
            Self::Refusal(Refusal::Denied { status, message }) => {
                write!(f, "the provider refused ({status}): {message}")
            }
            Self::TokenRejected => write!(f, "the access token was refused"),
            Self::Unsupported(what) => write!(f, "{what}"),
        }
    }
}

/// Where a folder is in its walk, carried in the opaque cursor.
///
/// **The link is the cursor.** A folder's first round is its backfill, and the provider's
/// sync state is fixed by the request that starts it: a change made while the round is still
/// paging is reported by the round after, not lost behind it. So D-82's cursor-first rule
/// holds structurally here — there is no request that walks without holding a position — and
/// what the cursor records is only the link to ask next and whether the first round is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Whether the first round has completed. Only after it does a presence mean *delivered*.
    pub live: bool,
    /// The next page of this round, or the delta link that starts the next one.
    pub link: String,
}

impl Position {
    #[must_use]
    pub fn encode(&self) -> Cursor {
        let value = serde_json::json!({
            "phase": if self.live { "live" } else { "backfill" },
            "link": self.link,
        });
        Cursor(serde_json::to_vec(&value).unwrap_or_default())
    }

    /// # Errors
    /// [`Refusal::Malformed`] where the stored cursor is not one this adapter wrote, or where
    /// its link would leave the provider.
    pub fn decode(cursor: &Cursor) -> Result<Self, Refusal> {
        let value: serde_json::Value = serde_json::from_slice(&cursor.0)
            .map_err(|_| Refusal::Malformed("the stored cursor is not one this adapter wrote"))?;
        let link = value
            .get("link")
            .and_then(|l| l.as_str())
            .ok_or(Refusal::Malformed("the stored cursor carried no link"))?;
        // A stored link is re-checked rather than trusted: the store is where it came from,
        // and the bearer token goes wherever it points.
        if !link.starts_with(&format!("{}/", wire::ROOT)) {
            return Err(Refusal::Malformed(
                "the stored cursor pointed outside the provider",
            ));
        }
        Ok(Self {
            live: value.get("phase").and_then(|p| p.as_str()) == Some("live"),
            link: link.to_owned(),
        })
    }
}

/// The adapter.
///
/// The transport is behind a cell because the contract takes `&self` — the adapter is a
/// **plan executor**, not a place state lives, and the layers above hold every durable thing
/// it produces.
pub struct Graph<T: Transport> {
    transport: RefCell<T>,
    /// The bearer presented on every request. Replaced by the credential broker, which is
    /// the only place D-88's rules exist.
    access_token: RefCell<String>,
    capabilities: Capabilities,
}

impl<T: Transport> core::fmt::Debug for Graph<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Graph").finish_non_exhaustive()
    }
}

impl<T: Transport> Graph<T> {
    #[must_use]
    pub fn new(transport: T, access_token: &str) -> Self {
        Self {
            transport: RefCell::new(transport),
            access_token: RefCell::new(access_token.to_owned()),
            capabilities: crate::capabilities(),
        }
    }

    /// Present a different access token from now on.
    pub fn set_access_token(&self, token: &str) {
        *self.access_token.borrow_mut() = token.to_owned();
    }

    /// The transport, for a test that needs to assert on what was *asked for*.
    #[must_use]
    pub fn transport(&self) -> core::cell::Ref<'_, T> {
        self.transport.borrow()
    }

    fn send(&self, request: Request<'_>) -> Result<Response, GraphError> {
        let token = self.access_token.borrow().clone();
        let request = request.header("authorization", &format!("Bearer {token}"));
        let response = self.transport.borrow_mut().exchange(&request)?;
        // The token is stale, or was withdrawn. The broker decides which — this adapter must
        // not.
        if response.status == 401 {
            return Err(GraphError::TokenRejected);
        }
        Ok(response)
    }

    /// A `GET` of a rooted target whose answer must be a success.
    fn get(&self, target: &str, prefer: Option<&str>) -> Result<Vec<u8>, GraphError> {
        let mut request = Request::new("GET", target);
        if let Some(prefer) = prefer {
            request = request.header("prefer", prefer);
        }
        let response = self.send(request)?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(wire::refusal(response.status, &response.body).into())
        }
    }

    /// One batch of at most [`wire::BATCH_LIMIT`] requests, answered **per element**.
    ///
    /// The result is aligned with `requests`; `None` is an element the answer did not carry.
    fn post_batch(&self, requests: &[BatchRequest]) -> Result<Vec<Option<BatchItem>>, GraphError> {
        debug_assert!(requests.len() <= wire::BATCH_LIMIT);
        let body = wire::batch_body(requests);
        let response = self.send(
            Request::new("POST", &wire::batch_target())
                .header("content-type", "application/json")
                .body(&body),
        )?;
        if !response.is_success() {
            return Err(wire::refusal(response.status, &response.body).into());
        }
        let mut aligned: Vec<Option<BatchItem>> = vec![None; requests.len()];
        for item in wire::parse_batch(&response.body, requests.len())? {
            let index = item.index;
            aligned[index] = Some(item);
        }
        Ok(aligned)
    }

    /// Any number of **reads**, chunked to the provider's cap. A failure anywhere fails the
    /// whole read, which is safe because nothing read has changed anything.
    fn read_batch(
        &self,
        requests: Vec<BatchRequest>,
    ) -> Result<Vec<Option<BatchItem>>, GraphError> {
        let mut out = Vec::with_capacity(requests.len());
        for chunk in requests.chunks(wire::BATCH_LIMIT) {
            out.extend(self.post_batch(chunk)?);
        }
        Ok(out)
    }

    /// Every folder, walked down through the children the provider says each one has.
    fn walk_folders(&self) -> Result<Vec<Folder>, GraphError> {
        let mut folders: Vec<Folder> = Vec::new();
        let mut pending = std::collections::VecDeque::from([wire::rooted(&wire::folders_target())]);
        let mut asked: Vec<String> = Vec::new();
        while let Some(target) = pending.pop_front() {
            // A paging link that points back at a page already read is a loop the provider
            // wrote, and following it would never end.
            if asked.contains(&target) {
                continue;
            }
            asked.push(target.clone());
            let page = wire::parse_folders(&self.get(&target, None)?)?;
            for found in page.folders {
                if folders.iter().any(|f| f.id == found.id) {
                    continue;
                }
                if found.children > 0 {
                    pending.push_back(wire::rooted(&wire::child_folders_target(&found.id)));
                }
                folders.push(found);
            }
            if let Some(next) = page.next {
                pending.push_front(next);
            }
        }
        Ok(folders)
    }

    /// FR-5, through the provider's own well-known names — one batch, one element each.
    fn resolve_special_use(&self) -> Result<Vec<(SpecialUse, String)>, GraphError> {
        let requests = folder::WELL_KNOWN
            .iter()
            .map(|(name, _)| BatchRequest::get(wire::well_known_target(name)))
            .collect();
        let answers = self.read_batch(requests)?;
        Ok(folder::WELL_KNOWN
            .iter()
            .zip(answers)
            .filter_map(|((_, use_), item)| {
                let item = item?;
                // A name the account does not have answers 404, and resolves to nothing:
                // FR-5's prompt-once rather than a guess.
                if !(200..300).contains(&item.status) {
                    return None;
                }
                Some((*use_, wire::parse_folder_id(&item.body)?))
            })
            .collect())
    }

    /// The categories each tag intent's message holds now, or the outcome that settles it.
    fn categories_for(
        &self,
        batch: &[WireMutation],
        outcomes: &mut [Option<MutationOutcome>],
    ) -> Result<Vec<Option<Vec<String>>>, GraphError> {
        let mut categories: Vec<Option<Vec<String>>> = vec![None; batch.len()];
        // A name L-15 refuses is settled before anything is asked: reading a message's
        // categories to learn that a tag will not be written is a request for nothing.
        for (i, mutation) in batch.iter().enumerate() {
            if let Operation::AddTag(name) = &mutation.operation
                && !self.capabilities.accepts_tag_name(name)
            {
                outcomes[i] = Some(MutationOutcome::Refused);
            }
        }
        let wanted: Vec<usize> = batch
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                outcomes[*i].is_none()
                    && matches!(m.operation, Operation::AddTag(_) | Operation::RemoveTag(_))
            })
            .map(|(i, _)| i)
            .collect();
        if wanted.is_empty() {
            return Ok(categories);
        }
        let requests = wanted
            .iter()
            .map(|i| BatchRequest::get(wire::categories_target(&batch[*i].message)))
            .collect();
        for (i, item) in wanted.iter().zip(self.read_batch(requests)?) {
            match item {
                Some(item) if (200..300).contains(&item.status) => {
                    categories[*i] = Some(wire::parse_categories(&item.body));
                }
                Some(item) => outcomes[*i] = Some(element_outcome(item.status)),
                None => outcomes[*i] = Some(MutationOutcome::Transient),
            }
        }
        Ok(categories)
    }
}

/// The write one intent resolves to, or the outcome that settles it without one.
///
/// **Every intent is resolved inside the adapter.** The layers above never learn that a move
/// here is a POST that answers with a new identifier, or that a tag change has to restate the
/// whole category list.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Write(BatchRequest),
    Settled(MutationOutcome),
}

/// Resolve one intent against the message's current categories, where it needs them.
#[must_use]
pub fn resolve(
    mutation: &WireMutation,
    categories: Option<&[String]>,
    capabilities: &Capabilities,
) -> Resolved {
    let id = &mutation.message;
    let move_to = |destination: &str| {
        Resolved::Write(BatchRequest::with_body(
            "POST",
            wire::move_target(id),
            serde_json::json!({ "destinationId": destination }),
        ))
    };
    let patch = |body: serde_json::Value| {
        Resolved::Write(BatchRequest::with_body(
            "PATCH",
            wire::message_target(id),
            body,
        ))
    };
    match &mutation.operation {
        // `ArchiveSemantics::MoveToSpecialUse`: a move, addressed by the well-known name so
        // that no folder identifier has to be looked up first.
        Operation::Archive => move_to(folder::ARCHIVE),
        // `TrashSemantics::MoveToTrash`: recoverable, through any client the user owns.
        Operation::DeleteToTrash => move_to(folder::DELETED_ITEMS),
        Operation::MoveTo(folder) => move_to(&folder.0),
        Operation::PermanentlyDelete => Resolved::Write(BatchRequest {
            method: "POST",
            url: wire::permanent_delete_target(id),
            body: None,
            prefer: None,
        }),
        Operation::SetRead(read) => patch(serde_json::json!({ "isRead": read })),
        Operation::SetFlagged(flagged) => patch(serde_json::json!({
            "flag": { "flagStatus": if *flagged { "flagged" } else { "notFlagged" } }
        })),
        Operation::AddTag(name) => {
            // L-15 bounds the name, and the capability decides whether tags exist at all.
            if !capabilities.accepts_tag_name(name) {
                return Resolved::Settled(MutationOutcome::Refused);
            }
            let current = categories.unwrap_or_default();
            if current.iter().any(|c| c == name) {
                // Already there. Nothing to send, and the intent has succeeded — which is
                // what "exactly once observable" means.
                return Resolved::Settled(MutationOutcome::Applied);
            }
            let mut next = current.to_vec();
            next.push(name.clone());
            patch(serde_json::json!({ "categories": next }))
        }
        Operation::RemoveTag(name) => {
            let current = categories.unwrap_or_default();
            if !current.iter().any(|c| c == name) {
                return Resolved::Settled(MutationOutcome::Applied);
            }
            let next: Vec<&String> = current.iter().filter(|c| *c != name).collect();
            patch(serde_json::json!({ "categories": next }))
        }
        // **Absent, not approximated.** The version of the API this adapter speaks has no
        // report call — only the move to the junk folder, which the capability table offers
        // as a move under FR-13, labelled as what it is. Sending the move here and calling it
        // a report is the "silently does something else" FR-39 forbids.
        Operation::ReportJunk | Operation::ReportNotJunk => {
            Resolved::Settled(MutationOutcome::Refused)
        }
    }
}

/// What one element's status says about the intent it carried.
#[must_use]
pub fn element_outcome(status: u16) -> MutationOutcome {
    match status {
        200..=299 => MutationOutcome::Applied,
        // The message is gone from under this identifier — deleted, or moved and so
        // renamed. The intent cannot apply to it and never will; the delta that reports the
        // move is what the layers above learn the new identifier from.
        404 | 410 => MutationOutcome::Refused,
        // The provider refused the request itself, in terms retrying will not change.
        400 | 403 | 405 | 413 | 422 => MutationOutcome::Refused,
        // A throttle, a conflict on the change key, the provider's own trouble. Retryable,
        // at the scheduler's discretion.
        _ => MutationOutcome::Transient,
    }
}

/// What a batch that never came back means for every intent it carried.
fn unanswered(error: &GraphError) -> MutationOutcome {
    match error {
        // The request went out and the answer did not come back. D-85's *Reconciling*.
        GraphError::Transport(TransportError::Unknown) => MutationOutcome::Unknown,
        GraphError::Refusal(Refusal::Denied { status, .. }) => element_outcome(*status),
        _ => MutationOutcome::Transient,
    }
}

impl<T: Transport> Adapter for Graph<T> {
    type Error = GraphError;

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Self::Error> {
        let folders = self.walk_folders()?;
        let resolved = self.resolve_special_use()?;
        Ok(folders
            .iter()
            .map(|f| folder::as_remote(f, &resolved))
            .collect())
    }

    fn delta(
        &self,
        folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, Self::Error> {
        let position = match cursor {
            Some(cursor) => Position::decode(cursor)?,
            None => Position {
                live: false,
                link: wire::rooted(&wire::delta_target(folder)),
            },
        };
        let page_size = u32::try_from(L26_BACKFILL_PAGE).unwrap_or(500);
        let response = self.send(
            Request::new("GET", &position.link).header("prefer", &wire::page_preference(page_size)),
        )?;
        if !response.is_success() {
            // The one refusal this adapter reads as progress rather than as a fault. Links
            // expire, and falling outside one is D-82's `Invalidated`, which NFR-18 recovers
            // from without asking the user anything.
            if wire::delta_link_expired(response.status, &response.body) {
                return Err(Refusal::CursorInvalidated.into());
            }
            return Err(wire::refusal(response.status, &response.body).into());
        }
        let page = wire::parse_delta(&response.body, position.live)?;
        Ok(match (page.next, page.delta) {
            // More of this round. The phase is unchanged, so an interruption resumes the same
            // round from the last committed page — L-26's granularity.
            (Some(next), _) => Delta {
                changes: page.changes,
                next: Position {
                    live: position.live,
                    link: next,
                }
                .encode(),
                more: true,
            },
            // The round is over, and the link to the next one is the cursor. From here on the
            // folder is live, whichever round this was.
            (None, Some(delta)) => Delta {
                changes: page.changes,
                next: Position {
                    live: true,
                    link: delta,
                }
                .encode(),
                more: false,
            },
            (None, None) => {
                return Err(Refusal::Malformed("a delta page had no way forward").into());
            }
        })
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let requests = ids
            .iter()
            .map(|id| BatchRequest::get(wire::envelope_target(id)))
            .collect();
        let mut out = Vec::with_capacity(ids.len());
        for item in self.read_batch(requests)? {
            // An element the answer did not carry leaves a message with no envelope, which
            // the store would record as present-and-empty. Failing the page instead means it
            // is asked again, which is safe because reapplication is.
            let item = item.ok_or(TransportError::Transient)?;
            match item.status {
                200..=299 => {}
                // Vanished between the delta and the fetch — ordinary, and the delta that
                // removes it is already on its way. Skipped, not failed.
                404 | 410 => continue,
                // Throttled **per element**. The page is retried at the provider's stated
                // delay rather than stored with holes in it.
                429 => {
                    return Err(TransportError::Throttled {
                        retry_after_millis: item.retry_after_millis.unwrap_or(0),
                    }
                    .into());
                }
                500..=599 => return Err(TransportError::Transient.into()),
                _ => continue,
            }
            let Ok(envelope) = wire::parse_envelope(&item.body) else {
                continue;
            };
            // **Only what was asked for.** An answer carrying an envelope nobody requested is
            // not something to believe: it would enter the store as a message this folder
            // never listed.
            if ids.contains(&envelope.id) {
                out.push(envelope);
            }
        }
        Ok(out)
    }

    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, Self::Error> {
        // The provider exposes one body, in whichever representation is asked for, and no
        // MIME tree. Both representations are described, and stage 2 chooses between them
        // above the adapter as it does for every provider.
        let mut parts = vec![
            PartDescriptor {
                id: wire::BODY_HTML.to_owned(),
                media_type: "text/html".to_owned(),
                filename: None,
                size: 0,
            },
            PartDescriptor {
                id: wire::BODY_TEXT.to_owned(),
                media_type: "text/plain".to_owned(),
                filename: None,
                size: 0,
            },
        ];
        // Attachments are **described**, never fetched: a message carrying forty megabytes
        // costs one listing until somebody asks for the attachment.
        let mut target = Some(wire::rooted(&wire::attachments_target(id)));
        let bound = usize::try_from(L2_MIME_PARTS).unwrap_or(usize::MAX);
        while let Some(next) = target.take() {
            let page = wire::parse_attachments(&self.get(&next, None)?)?;
            for attachment in page.attachments {
                // L-2, refused past rather than truncated at: a structure larger than the
                // bound is one whose shape the sender chose.
                if parts.len() >= bound {
                    return Err(
                        Refusal::Malformed("the message has more parts than L-2 allows").into(),
                    );
                }
                parts.push(PartDescriptor {
                    id: format!("{}{}", wire::ATTACHMENT_PART, attachment.id),
                    media_type: attachment.media_type,
                    // An attachment always has a filename here, so it is never mistaken for
                    // body content — even an unnamed one.
                    filename: Some(attachment.name.unwrap_or_default()),
                    size: attachment.size,
                });
            }
            target = page.next;
        }
        Ok(parts)
    }

    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, Self::Error> {
        if let Some(prefer) = wire::body_preference(part) {
            let body = self.get(&wire::rooted(&wire::body_target(id)), Some(prefer))?;
            let body = wire::parse_body(&body)?;
            // L-1, refused rather than truncated. The structure could not state a body's
            // size — the provider has none to give — so stage 2 had nothing to fall back on,
            // and this is the one place the bound can hold before the sanitizer is handed it.
            if u64::try_from(body.len()).unwrap_or(u64::MAX) > L1_BODY_PART_BYTES {
                return Err(TransportError::TooLarge {
                    limit_bytes: L1_BODY_PART_BYTES,
                }
                .into());
            }
            return Ok(body);
        }
        if let Some(attachment) = part.strip_prefix(wire::ATTACHMENT_PART) {
            return self.get(
                &wire::rooted(&wire::attachment_value_target(id, attachment)),
                None,
            );
        }
        Err(Refusal::Malformed("the message has no such part").into())
    }

    /// Resolved **per element, never for the batch**. A batch that half-succeeds is the
    /// ordinary case, and the envelope's own success says only that the envelope was read.
    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error> {
        let mut outcomes: Vec<Option<MutationOutcome>> = vec![None; batch.len()];
        let categories = self.categories_for(batch, &mut outcomes)?;

        let mut writes: Vec<(usize, BatchRequest)> = Vec::new();
        for (i, mutation) in batch.iter().enumerate() {
            if outcomes[i].is_some() {
                continue;
            }
            match resolve(mutation, categories[i].as_deref(), &self.capabilities) {
                Resolved::Write(request) => writes.push((i, request)),
                Resolved::Settled(outcome) => outcomes[i] = Some(outcome),
            }
        }

        let mut written = false;
        for chunk in writes.chunks(wire::BATCH_LIMIT) {
            let requests: Vec<BatchRequest> = chunk.iter().map(|(_, r)| r.clone()).collect();
            match self.post_batch(&requests) {
                Ok(answers) => {
                    for ((i, _), answer) in chunk.iter().zip(answers) {
                        outcomes[*i] = Some(match answer {
                            Some(item) => element_outcome(item.status),
                            // Sent, and not answered. Establishing server state before a
                            // retry is D-85's, and this is what tells it to.
                            None => MutationOutcome::Unknown,
                        });
                    }
                    written = true;
                }
                // Nothing has been written yet, so the failure is the batch's alone and the
                // scheduler gets its classification whole — a throttle keeps its number.
                Err(e) if !written => return Err(e),
                // Earlier chunks were applied. Their outcomes stand, and the rest are settled
                // by what happened to their chunk rather than thrown away with it.
                Err(e) => {
                    for (i, _) in chunk {
                        outcomes[*i] = Some(unanswered(&e));
                    }
                }
            }
        }
        Ok(outcomes
            .into_iter()
            .map(|o| o.unwrap_or(MutationOutcome::Transient))
            .collect())
    }

    /// **There is no doorbell.**
    ///
    /// The provider's change notifications are delivered to a publicly reachable endpoint,
    /// and Sift never opens a listening socket for any purpose (NFR-24). The capability table
    /// says `PollOnly`, so nothing above this ever calls it — and it refuses rather than
    /// quietly succeeding, because a watch that returned `Ok` and watched nothing is the worst
    /// of the possible answers.
    fn watch(&self, _folders: &[RemoteFolderId]) -> Result<(), Self::Error> {
        Err(GraphError::Unsupported(
            "this provider's change notifications need a listening endpoint, which Sift never opens",
        ))
    }

    /// FR-21, over `$search` on the whole mailbox: one request, identifiers only.
    ///
    /// Every folder, because a search is not scoped to where sync happens to be watching —
    /// and identifiers that are unstable on move are still the right thing to answer with: the
    /// caller joins them to the store the way a delta's are joined, at the moment they are
    /// current.
    fn search(
        &self,
        terms: &[SearchTerm],
        limit: usize,
    ) -> Result<Vec<RemoteMessageId>, Self::Error> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query = wire::search_query(terms).map_err(GraphError::Unsupported)?;
        // Every value was punctuation: nothing to ask, which is the local reading too.
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let bound = limit.min(usize::try_from(L32_SERVER_SEARCH_HITS).unwrap_or(limit));
        let target = wire::rooted(&wire::search_target(
            &query,
            u32::try_from(bound).unwrap_or(u32::MAX),
        ));
        let ids = wire::parse_search(&self.get(&target, None)?)?;
        // `$top` is a request, not a promise; an answer past it is not believed.
        Ok(ids.into_iter().take(bound).collect())
    }

    fn present_credential(&self, secret: &str) {
        self.set_access_token(secret);
    }

    fn wire_bytes(&self) -> (u64, u64) {
        self.transport.borrow().wire_bytes()
    }

    fn classify(&self, error: &Self::Error) -> Failure {
        match error {
            Self::Error::Refusal(Refusal::CursorInvalidated) => Failure::CursorInvalidated,
            Self::Error::TokenRejected => Failure::CredentialRefused,
            Self::Error::Transport(TransportError::Throttled { retry_after_millis }) => {
                Failure::Throttled {
                    retry_after_millis: *retry_after_millis,
                }
            }
            Self::Error::Transport(TransportError::Transient) => Failure::Transient,
            Self::Error::Transport(TransportError::Unknown) => Failure::Unknown,
            // An answer that did not parse says nothing about the account and everything about
            // what answered. A captive portal's sign-in page arrives here, and treating it as
            // settled would degrade every account on a hotel network — NFR-34's cascade.
            Self::Error::Refusal(Refusal::Malformed(_)) => Failure::Transient,
            Self::Error::Transport(TransportError::NoFixture(_))
            | Self::Error::Transport(TransportError::TooLarge { .. })
            | Self::Error::Transport(TransportError::Refused(_))
            | Self::Error::Refusal(Refusal::Denied { .. })
            | Self::Error::Unsupported(_) => Failure::Permanent,
        }
    }
}

//! The adapter itself: six responsibilities over one transport.

use core::cell::RefCell;
use sift_foundation::limits::L26_BACKFILL_PAGE;
use sift_provider::adapter::{
    Adapter, Change, Cursor, Delta, Envelope, Failure, MutationOutcome, Operation, PartDescriptor,
    RemoteFolder, RemoteFolderId, RemoteMessageId, WireMutation,
};
use sift_provider::capability::Capabilities;
use sift_provider::transport::{Request, Response, Transport, TransportError};

use crate::label::{self, Label};
use crate::wire::{self, Refusal};

/// What can go wrong, above the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailError {
    /// The wire failed. The scheduler decides what happens next, under D-87 — never this.
    Transport(TransportError),
    /// The provider answered, and the answer was not usable.
    Refusal(Refusal),
    /// The access token was refused.
    ///
    /// **This is not the same as a denied grant.** It says the presented token is not
    /// currently good, which is the ordinary state of an expired one; D-88's single-flight
    /// refresh runs in the credential broker and the request is tried again. Only the
    /// refresh itself can conclude that the *grant* is gone.
    TokenRejected,
    /// Something FR-13 names that this provider will not do under the scope Sift asks for.
    Unsupported(&'static str),
}

impl From<TransportError> for GmailError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<Refusal> for GmailError {
    fn from(e: Refusal) -> Self {
        Self::Refusal(e)
    }
}

impl core::fmt::Display for GmailError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "the wire failed: {e:?}"),
            Self::Refusal(Refusal::CursorInvalidated) => {
                write!(
                    f,
                    "the stored cursor is outside the retained history window"
                )
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
/// **The history identifier is acquired before the backfill starts and carried through it
/// unchanged.** That is D-82's cursor-first rule made structural rather than remembered: the
/// only way to reach `Live` is to have been holding a history identifier the whole time, so
/// there is no code path that walks first and takes a cursor afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub history: String,
    /// `Some` while the backfill is still walking, `None` once it is live.
    pub backfill_page: Option<String>,
    pub backfilling: bool,
    /// A continuation inside one run of history pages.
    pub history_page: Option<String>,
}

impl Position {
    #[must_use]
    pub fn encode(&self) -> Cursor {
        let mut value = serde_json::Map::new();
        value.insert(
            "phase".into(),
            serde_json::Value::from(if self.backfilling { "backfill" } else { "live" }),
        );
        value.insert(
            "history".into(),
            serde_json::Value::from(self.history.clone()),
        );
        if let Some(page) = &self.backfill_page {
            value.insert("page".into(), serde_json::Value::from(page.clone()));
        }
        if let Some(page) = &self.history_page {
            value.insert("historyPage".into(), serde_json::Value::from(page.clone()));
        }
        Cursor(serde_json::to_vec(&value).unwrap_or_default())
    }

    /// # Errors
    /// [`Refusal::Malformed`] where the stored cursor is not one this adapter wrote.
    pub fn decode(cursor: &Cursor) -> Result<Self, Refusal> {
        let value: serde_json::Value = serde_json::from_slice(&cursor.0)
            .map_err(|_| Refusal::Malformed("the stored cursor is not one this adapter wrote"))?;
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Ok(Self {
            history: text("history")
                .ok_or(Refusal::Malformed("the stored cursor carried no position"))?,
            backfill_page: text("page"),
            backfilling: value.get("phase").and_then(|p| p.as_str()) == Some("backfill"),
            history_page: text("historyPage"),
        })
    }
}

/// The adapter.
///
/// The transport is behind a cell because the contract takes `&self` — the adapter is a
/// **plan executor**, not a place state lives, and the layers above hold every durable thing
/// it produces.
pub struct Gmail<T: Transport> {
    transport: RefCell<T>,
    /// The bearer presented on every request. Replaced by the credential broker, which is
    /// the only place D-88's rules exist.
    access_token: RefCell<String>,
    capabilities: Capabilities,
    /// The label set, as last enumerated. Needed to resolve a tag *name* to the identifier
    /// the wire takes, which is a mapping only this adapter should know about.
    labels: RefCell<Vec<Label>>,
}

impl<T: Transport> core::fmt::Debug for Gmail<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Gmail")
            .field("labels", &self.labels.borrow().len())
            .finish_non_exhaustive()
    }
}

impl<T: Transport> Gmail<T> {
    #[must_use]
    pub fn new(transport: T, access_token: &str) -> Self {
        Self {
            transport: RefCell::new(transport),
            access_token: RefCell::new(access_token.to_owned()),
            capabilities: crate::capabilities(),
            labels: RefCell::new(Vec::new()),
        }
    }

    /// Present a different access token from now on. D-88's write-before-use ordering is the
    /// broker's; this is only where the result lands.
    pub fn set_access_token(&self, token: &str) {
        *self.access_token.borrow_mut() = token.to_owned();
    }

    /// The transport, for a test that needs to assert on what was *asked for*.
    ///
    /// Claims like "Sift MUST NOT fetch whole messages" and "the cursor is taken before the
    /// walk" are claims about requests, and nothing about a response can check either.
    #[must_use]
    pub fn transport(&self) -> core::cell::Ref<'_, T> {
        self.transport.borrow()
    }

    /// Bytes on the wire since this adapter was built — FR-36.
    #[must_use]
    pub fn wire_bytes(&self) -> (u64, u64) {
        self.transport.borrow().wire_bytes()
    }

    fn send(&self, request: Request<'_>) -> Result<Response, GmailError> {
        let token = self.access_token.borrow().clone();
        let request = request.header("authorization", &format!("Bearer {token}"));
        let response = self.transport.borrow_mut().exchange(&request)?;
        // The token is stale, or was withdrawn. Either way the request is not answered and
        // the broker decides which — this adapter must not.
        if response.status == 401 {
            return Err(GmailError::TokenRejected);
        }
        Ok(response)
    }

    /// A `GET` whose answer must be a success.
    fn get(&self, target: &str) -> Result<Vec<u8>, GmailError> {
        let response = self.send(Request::new("GET", target))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(wire::refusal(response.status, &response.body).into())
        }
    }

    fn post(&self, target: &str, content_type: &str, body: &[u8]) -> Result<Response, GmailError> {
        self.send(
            Request::new("POST", target)
                .header("content-type", content_type)
                .body(body),
        )
    }

    /// The account's history identifier, taken **before** anything is walked.
    ///
    /// # Errors
    /// See [`GmailError`].
    pub fn history_id(&self) -> Result<String, GmailError> {
        Ok(wire::parse_history_id(&self.get(&wire::profile_target())?)?)
    }

    /// Refresh the cached label set and hand back everything on the tag axis.
    ///
    /// # Errors
    /// See [`GmailError`].
    pub fn tags(&self) -> Result<Vec<String>, GmailError> {
        self.load_labels()?;
        Ok(self
            .labels
            .borrow()
            .iter()
            .filter(|l| {
                matches!(
                    l.axis(),
                    label::Axis::Tag | label::Axis::UndecidedByTheModel
                )
            })
            .map(|l| l.name.clone())
            .collect())
    }

    fn load_labels(&self) -> Result<(), GmailError> {
        let labels = wire::parse_labels(&self.get(&wire::labels_target())?)?;
        *self.labels.borrow_mut() = labels;
        Ok(())
    }

    /// The identifier a tag name resolves to, creating it where FR-37 permits.
    fn label_id_for(&self, name: &str, create: bool) -> Result<Option<String>, GmailError> {
        if self.labels.borrow().is_empty() {
            self.load_labels()?;
        }
        if let Some(found) = self
            .labels
            .borrow()
            .iter()
            .find(|l| !l.system && l.name == name)
        {
            return Ok(Some(found.id.clone()));
        }
        if !create {
            return Ok(None);
        }
        // L-15 bounds the name, and the capability decides whether tags exist at all.
        if !self.capabilities.accepts_tag_name(name) {
            return Err(GmailError::Unsupported("the tag name is not acceptable"));
        }
        let body = serde_json::to_vec(&serde_json::json!({ "name": name })).unwrap_or_default();
        let response = self.post(&format!("{}/labels", wire::USER), "application/json", &body)?;
        if !response.is_success() {
            return Err(wire::refusal(response.status, &response.body).into());
        }
        let created: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|_| Refusal::Malformed("the created label was not returned"))?;
        let id = created
            .get("id")
            .and_then(|i| i.as_str())
            .ok_or(Refusal::Malformed(
                "the created label carried no identifier",
            ))?
            .to_owned();
        self.labels.borrow_mut().push(Label {
            id: id.clone(),
            name: name.to_owned(),
            system: false,
        });
        Ok(Some(id))
    }

    fn backfill(&self, folder: &RemoteFolderId, position: &Position) -> Result<Delta, GmailError> {
        let page_size = u32::try_from(L26_BACKFILL_PAGE).unwrap_or(500);
        let target = wire::list_target(folder, position.backfill_page.as_deref(), page_size);
        let page = wire::parse_list(&self.get(&target)?)?;
        let more = page.next_page.is_some();
        Ok(Delta {
            changes: page
                .ids
                .into_iter()
                .map(|id| Change::Present {
                    id,
                    // A backfill discovers. Only the delta delivers.
                    provenance: sift_provider::adapter::Provenance::Discovered,
                })
                .collect(),
            next: Position {
                history: position.history.clone(),
                backfill_page: page.next_page,
                backfilling: more,
                history_page: None,
            }
            .encode(),
            more,
        })
    }

    fn live(&self, folder: &RemoteFolderId, position: &Position) -> Result<Delta, GmailError> {
        let page_size = u32::try_from(L26_BACKFILL_PAGE).unwrap_or(500);
        let target = wire::history_target(
            &position.history,
            folder,
            position.history_page.as_deref(),
            page_size,
        );
        let response = self.send(Request::new("GET", &target))?;
        // The one status this adapter reads as progress rather than as a fault. The window
        // is finite and falling outside it is D-82's `Invalidated`, which NFR-18 recovers
        // from without asking the user anything.
        if response.status == 404 {
            return Err(Refusal::CursorInvalidated.into());
        }
        if !response.is_success() {
            return Err(wire::refusal(response.status, &response.body).into());
        }
        let page = wire::parse_history(&response.body, folder)?;
        let more = page.next_page.is_some();
        Ok(Delta {
            changes: page.changes,
            next: Position {
                // While pages remain the resume point is unchanged, so an interruption
                // repeats the run rather than skipping its tail.
                history: if more {
                    position.history.clone()
                } else {
                    page.history_id.unwrap_or_else(|| position.history.clone())
                },
                backfill_page: None,
                backfilling: false,
                history_page: page.next_page,
            }
            .encode(),
            more,
        })
    }
}

/// A label change, as one message's intent resolves to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LabelChange {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

/// The system location labels a move takes a message out of.
///
/// Category labels are **not** here: the provider assigns them itself, and removing one
/// would be Sift undoing a classification nobody asked it to touch.
const MOVED_OUT_OF: &[&str] = &[label::INBOX, label::SPAM, label::TRASH];

impl<T: Transport> Adapter for Gmail<T> {
    type Error = GmailError;

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Self::Error> {
        self.load_labels()?;
        Ok(self
            .labels
            .borrow()
            .iter()
            .filter_map(Label::as_folder)
            .collect())
    }

    fn delta(
        &self,
        folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, Self::Error> {
        let position = match cursor {
            Some(cursor) => Position::decode(cursor)?,
            // D-82, structurally: the cursor is taken here, before a single message is
            // listed. Backfill-first would lose every change during a walk that on a large
            // mailbox is hours, and lose it silently.
            None => Position {
                history: self.history_id()?,
                backfill_page: None,
                backfilling: true,
                history_page: None,
            },
        };
        if position.backfilling {
            self.backfill(folder, &position)
        } else {
            self.live(folder, &position)
        }
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let targets: Vec<String> = ids.iter().map(wire::envelope_target).collect();
        let body = wire::batch_request_body(&targets);
        let response = self.post(&wire::batch_target(), &wire::batch_content_type(), &body)?;
        if !response.is_success() {
            return Err(wire::refusal(response.status, &response.body).into());
        }
        let boundary = response
            .header("content-type")
            .and_then(wire::boundary_of)
            .ok_or(Refusal::Malformed("the batch answer was not multipart"))?;

        let mut out = Vec::with_capacity(ids.len());
        for item in wire::parse_batch(&response.body, &boundary) {
            // A message that vanished between the delta and the fetch is ordinary — the
            // delta that removes it is already on its way. Skipped, not failed.
            if !(200..300).contains(&item.status) {
                continue;
            }
            let Ok(envelope) = wire::parse_envelope(&item.body) else {
                continue;
            };
            // **Only what was asked for.** An answer carrying an envelope nobody requested
            // is not something to believe: it would enter the store as a message this folder
            // never listed, with a provenance the delta never assigned it.
            if ids.contains(&envelope.id) {
                out.push(envelope);
            }
        }
        Ok(out)
    }

    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, Self::Error> {
        let parts = wire::parse_structure(&self.get(&wire::structure_target(id))?)?;
        Ok(parts
            .into_iter()
            // The container nodes — `multipart/*` — are structure rather than content, and a
            // caller choosing what to render has nothing to do with them.
            .filter(|p| !p.mime_type.starts_with("multipart/"))
            .map(|p| PartDescriptor {
                id: p.id,
                media_type: p.mime_type,
                filename: p.filename,
                size: p.size,
            })
            .collect())
    }

    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, Self::Error> {
        // An attachment is asked for by reference, which is what keeps the structure fetch
        // from carrying its bytes.
        if let Some(attachment) = part.strip_prefix("attachment:") {
            let body = self.get(&wire::attachment_target(id, attachment))?;
            return Ok(wire::parse_attachment(&body)?);
        }
        let structure = wire::parse_structure(&self.get(&wire::structure_target(id))?)?;
        let found = structure
            .iter()
            .find(|p| p.id == part)
            .ok_or(Refusal::Malformed("the message has no such part"))?;
        if let Some(inline) = &found.inline {
            return Ok(inline.clone());
        }
        if let Some(attachment) = &found.attachment {
            let body = self.get(&wire::attachment_target(id, attachment))?;
            return Ok(wire::parse_attachment(&body)?);
        }
        Ok(Vec::new())
    }

    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error> {
        let mut outcomes = Vec::with_capacity(batch.len());
        for mutation in batch {
            outcomes.push(self.apply_one(mutation)?);
        }
        Ok(outcomes)
    }

    /// **There is no doorbell.**
    ///
    /// D-7 chose one and the wire protocol removed the choice: the only push this provider
    /// offers a desktop client is IMAP IDLE, and IMAP over OAuth requires a scope that also
    /// authorizes sending. See `crate::oauth`. The capability table says `PollOnly`, so
    /// nothing above this ever calls it — and it refuses rather than quietly succeeding,
    /// because a watch that returned `Ok` and watched nothing is the worst of the three
    /// possible answers.
    fn watch(&self, _folders: &[RemoteFolderId]) -> Result<(), Self::Error> {
        Err(GmailError::Unsupported(
            "this provider offers no change notification that does not also authorize sending",
        ))
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
            // An answer that did not parse says nothing about the account and everything
            // about what answered. A captive portal's sign-in page arrives here, and
            // treating it as settled would degrade every account on a hotel network — which
            // is NFR-34's cascade, reaching the folder state machine instead of the
            // credential store.
            Self::Error::Refusal(Refusal::Malformed(_)) => Failure::Transient,
            Self::Error::Transport(TransportError::NoFixture(_))
            | Self::Error::Transport(TransportError::TooLarge { .. })
            | Self::Error::Transport(TransportError::Refused(_))
            | Self::Error::Refusal(Refusal::Denied { .. })
            | Self::Error::Unsupported(_) => Failure::Permanent,
        }
    }
}

impl<T: Transport> Gmail<T> {
    /// Resolve one intent to the labels it moves, or to a request of its own.
    ///
    /// # Errors
    /// See [`GmailError`].
    pub fn resolve(&self, operation: &Operation) -> Result<LabelChange, GmailError> {
        Ok(match operation {
            // Archiving is removing the inbox label. That is the whole of it here, which is
            // what `ArchiveSemantics::RemoveFromInbox` declares.
            Operation::Archive => LabelChange {
                remove: vec![label::INBOX.to_owned()],
                ..LabelChange::default()
            },
            Operation::SetRead(true) => LabelChange {
                remove: vec![label::UNREAD.to_owned()],
                ..LabelChange::default()
            },
            Operation::SetRead(false) => LabelChange {
                add: vec![label::UNREAD.to_owned()],
                ..LabelChange::default()
            },
            Operation::SetFlagged(flagged) => {
                let starred = vec![label::STARRED.to_owned()];
                if *flagged {
                    LabelChange {
                        add: starred,
                        ..LabelChange::default()
                    }
                } else {
                    LabelChange {
                        remove: starred,
                        ..LabelChange::default()
                    }
                }
            }
            // Reporting junk moves the message to spam, which on this provider is the same
            // label operation the interface calls a move.
            Operation::ReportJunk => LabelChange {
                add: vec![label::SPAM.to_owned()],
                remove: vec![label::INBOX.to_owned()],
            },
            Operation::ReportNotJunk => LabelChange {
                add: vec![label::INBOX.to_owned()],
                remove: vec![label::SPAM.to_owned()],
            },
            // A move on a provider whose cardinality is *one or more* has to say what it
            // means. It means: land in the target, and leave the system locations that are
            // mutually exclusive with it. User labels are left alone, because on this
            // provider they are tags and a move is not an untag.
            Operation::MoveTo(folder) => LabelChange {
                add: vec![folder.0.clone()],
                remove: MOVED_OUT_OF
                    .iter()
                    .filter(|l| **l != folder.0)
                    .map(|l| (*l).to_owned())
                    .collect(),
            },
            Operation::AddTag(name) => {
                let id = self
                    .label_id_for(name, true)?
                    .ok_or(GmailError::Unsupported("the tag could not be created"))?;
                LabelChange {
                    add: vec![id],
                    ..LabelChange::default()
                }
            }
            Operation::RemoveTag(name) => match self.label_id_for(name, false)? {
                Some(id) => LabelChange {
                    remove: vec![id],
                    ..LabelChange::default()
                },
                // The tag is already not there. Nothing to send, and the intent has
                // succeeded — which is what "exactly once observable" means.
                None => LabelChange::default(),
            },
            Operation::DeleteToTrash | Operation::PermanentlyDelete => LabelChange::default(),
        })
    }

    fn apply_one(&self, mutation: &WireMutation) -> Result<MutationOutcome, GmailError> {
        // Every mutation this provider accepts is naturally idempotent — a label set to a
        // value it already holds is not an error, and neither is trashing what is already
        // trashed. There is no idempotency key to send, and D-85's *Reconciling* therefore
        // resolves by re-issuing rather than by first establishing server state.
        //
        // That is a property of the operations rather than a convenience, and it is why
        // FR-13's one non-idempotent intent is also the one this provider will not do.
        let response = match &mutation.operation {
            Operation::DeleteToTrash => self.post(
                &wire::trash_target(&mutation.message),
                "application/json",
                b"{}",
            )?,
            // The scope Sift asks for cannot do this, and the scope that can also authorizes
            // sending. Refused rather than approximated: `docs/mail/mutations.md` requires
            // an unsupported operation be *absent*, and the capability table already says
            // so — reaching here at all means a caller ignored it.
            Operation::PermanentlyDelete => {
                return Ok(MutationOutcome::Refused);
            }
            operation => {
                let change = self.resolve(operation)?;
                if change.add.is_empty() && change.remove.is_empty() {
                    return Ok(MutationOutcome::Applied);
                }
                self.post(
                    &wire::batch_modify_target(),
                    "application/json",
                    &wire::batch_modify_body(
                        core::slice::from_ref(&mutation.message),
                        &change.add,
                        &change.remove,
                    ),
                )?
            }
        };
        Ok(match response.status {
            s if (200..300).contains(&s) => MutationOutcome::Applied,
            // The message is gone. The intent cannot apply and never will, which is settled
            // rather than retryable.
            404 => MutationOutcome::Refused,
            403 => MutationOutcome::Refused,
            _ => MutationOutcome::Transient,
        })
    }
}

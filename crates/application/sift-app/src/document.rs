//! Opening a message body, and answering the body view's resource loads.
//!
//! # Why the broker outlives the render
//!
//! The pipeline's fifth stage binds every fetching position to a capability token and hands
//! back HTML addressed under it. The body view then *asks* for those resources, one at a
//! time, after the HTML has been handed over — so a broker created for the render and dropped
//! at the end of it would answer nothing, and every image in every message would fail.
//!
//! So the broker lives with the application, and a document's token is revoked when the view
//! navigates away from it. That is D-90's rule rather than a convenience: the view outlives
//! the messages it renders, so binding the token to the view would let one token serve two
//! messages and falsify the property D-28 rests on — that two messages share no address
//! space.

use sift_broker::broker::Answer;
use sift_foundation::identity::LocalId;

use crate::App;

/// A rendered message body, and everything a reader needs to draw its chrome.
///
/// The counts are here rather than in the document because the blocked-content chrome is
/// **native and never in the document**: a control inside the body is one a sender can
/// counterfeit, and a count a sender could write is not a count.
#[derive(Debug, Clone)]
pub struct Document {
    /// Post-sanitization HTML. A raw provider payload never reaches a shell.
    pub html: String,
    /// D-28's per-document capability token. Unguessable, and revoked at navigation.
    pub token: String,
    /// How many fetching positions were refused, and why, for the reader's own chrome.
    pub blocked: usize,
    /// Links, as the confirmation sheet must show them: what to display, and the real target.
    pub links: Vec<Link>,
    /// Which stages ran. FR-33's debug view is a view of this.
    pub stages: Vec<String>,
}

/// One link in a body.
#[derive(Debug, Clone)]
pub struct Link {
    /// What the document said.
    pub displayed: String,
    /// Where it actually goes. Shown to the user before anything opens — never followed to
    /// find out, because following a wrapper *is* the tracking event FR-30 refuses to cause.
    pub target: String,
}

impl App {
    /// Fetch a message's chosen part and run it through the seven stages.
    ///
    /// **Structure first, and then one part.** The structure costs a few kilobytes; a forty
    /// megabyte attachment beside it costs nothing until somebody asks for it, which is a
    /// claim about requests rather than about intentions.
    ///
    /// # Errors
    /// No account holds the message, it has not been synced, it carries no renderable part,
    /// or a stage refused it. A refusal is a degradation to FR-9's raw view rather than a
    /// panic — the pipeline catches at every stage boundary.
    pub fn open_document(&mut self, id: LocalId, dark: bool) -> Result<Document, String> {
        let owner = self
            .owner_of_stored(id)
            .ok_or("no account holds this message")?;
        let account = self.account(&owner)?;
        let remote: String = account
            .store
            .store
            .query_row(
                "SELECT remote_id FROM message WHERE id = ?1",
                rusqlite::params![id.to_bytes().to_vec()],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .ok_or("this message has no remote identifier yet — sync first")?;
        let adapter = account
            .adapter
            .as_ref()
            .ok_or("this account has no provider behind it")?;

        let remote_id = sift_provider::adapter::RemoteMessageId(remote);
        let parts = adapter.structure(&remote_id).map_err(|e| e.to_string())?;
        // Stage 2: HTML preferred, plain text as the fallback, and plain text where the HTML
        // exceeds L-1 — which rejects rather than truncating.
        let chosen =
            sift_mime::select::choose(&parts).ok_or("this message carries no renderable part")?;
        let bytes = adapter
            .fetch_part(&remote_id, &chosen.part)
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let selected = sift_pipeline::Selected {
            html: chosen.is_html.then(|| text.clone()),
            text: (!chosen.is_html).then_some(text),
            reason: Some(format!("{:?}", chosen.reason)),
        };

        let mut context = sift_pipeline::Context {
            // Nothing has authenticated this message yet, so the origin is null and every
            // resource is third-party under the strictest rules. D-11's fourth priority is
            // exactly this case, and it is the correct answer rather than a placeholder.
            origin: sift_block::origin::Origin::Null,
            blocker: None,
            dark,
            broker: &mut self.resources,
        };
        let rendered = sift_pipeline::render(&selected, &mut context).map_err(|e| e.to_string())?;

        // A refusal is anything that is not an allow, which includes `AbsentAuthority`:
        // D-10 makes a missing filter engine **deny**, never fall through, so a shed that
        // dropped the engine shows as content withheld rather than as content loaded.
        let blocked = rendered
            .verdicts
            .iter()
            .filter(|v| !v.permits_fetch())
            .count();

        Ok(Document {
            html: rendered.html,
            token: rendered.token.as_str().to_owned(),
            blocked,
            // A link is **not** a fetching position: it keeps its real address, because it
            // is followed on an explicit confirmation and never in place.
            links: rendered
                .links
                .iter()
                .map(|target| Link {
                    displayed: target.clone(),
                    target: target.clone(),
                })
                .collect(),
            stages: rendered.stages.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    /// Answer one resource load from the body view.
    ///
    /// The address is validated against the **live** token. A fabricated one resolves to
    /// nothing, which is what makes D-28's addressing a boundary rather than a naming scheme.
    ///
    /// With no filter list loaded the authority is absent, and D-10 makes an absent authority
    /// **deny** rather than fall through — so until the lists ship, every remote fetch is
    /// refused and the reader says so. That is the correct answer rather than a gap: a
    /// blocker that failed open would be worse than no blocker, because the product would
    /// claim a protection it was not providing.
    pub fn resolve_resource(&mut self, url: &str, transferred: Option<u64>) -> Answer {
        let request = sift_broker::broker::Request {
            url: url.to_owned(),
            transferred_length: transferred,
        };
        self.resources.answer(
            &request,
            &sift_block::engine::Authority::Absent,
            &sift_block::origin::Infrastructure::default(),
        )
    }

    /// Revoke a document's token.
    ///
    /// Called when the body view navigates away, **before** the next document exists — which
    /// is what keeps "two messages share no address space" true across a reused view.
    pub fn close_document(&mut self, token: &str) -> bool {
        // By text rather than by value: a shell holds the token as a string, and nothing
        // should be given a way to *construct* one — that is the part that stays unforgeable.
        self.resources.revoke_named(token)
    }
}

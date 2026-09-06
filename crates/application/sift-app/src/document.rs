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
    /// Every position in the document that would fetch something, refused or not.
    ///
    /// Carried beside the refusals rather than inferred from them, because "three images, all
    /// three withheld" and "three images, one withheld" are different sentences and only the
    /// pair distinguishes them. FR-33's debug view is a view of this.
    pub fetching_positions: usize,
    /// How many of those were refused. `withheld.len()`, kept because a count is what the
    /// chrome leads with and a shell should not have to compute one.
    pub blocked: usize,
    /// Each refusal, with the rule that caused it. The disclosure behind the count.
    pub withheld: Vec<Withheld>,
    /// Whether "always load from this sender" has anything to key a durable allowance on.
    ///
    /// False where the synthetic origin is null — which is every unauthenticated message, and
    /// therefore most of them until D-11's authentication inputs are wired. The control is
    /// **absent** rather than disabled in that case, because an allowance keyed on nothing
    /// would apply to everyone.
    pub may_always_allow: bool,
    /// Links, as the confirmation sheet must show them.
    pub links: Vec<Link>,
    /// FR-42's destination, where the message declares one.
    ///
    /// Surfaced under the same display rules as any other link, and **never requested**: the
    /// historical form is a message, which the no-send constraint forbids outright, and the
    /// modern form is an HTTP request carrying a per-recipient token — which is precisely what
    /// FR-29 treats as evidence that a resource is tracking the reader.
    pub unsubscribe: Option<Link>,
    /// Which stages ran. FR-33's debug view is a view of this.
    pub stages: Vec<String>,
}

/// One link in a body, as FR-30 requires it be shown.
#[derive(Debug, Clone)]
pub struct Link {
    /// The form a person reads: punycode-decoded and bidi-**stripped**.
    ///
    /// Stripped rather than isolated, which is the opposite of what NFR-54 does to a display
    /// name — and the difference is the whole defence. A display name is prose, where a
    /// right-to-left run is legitimate content. A URL is a structured identifier whose
    /// component order carries meaning, and a control that reverses it has no honest use.
    pub displayed: String,
    /// The address that will actually be opened. Never followed to find out where it goes,
    /// because following a wrapper *is* the tracking event FR-30 refuses to cause.
    pub target: String,
    /// The wrapper this was recovered from, kept available on request rather than discarded.
    /// A user who cannot see that a link was wrapped cannot judge who wrapped it.
    pub wrapper: Option<String>,
    /// A `mailto:`, which is shown and **reported as requiring a mail handler** rather than
    /// silently omitted — a user who cannot see it cannot know it existed.
    pub needs_a_mail_handler: bool,
}

/// One fetching position the broker refused, and the rule that refused it.
///
/// This is what the blocked-content disclosure lists. It is native chrome by construction: a
/// count a sender could write into the document is not a count, and a "load images" control
/// inside the body is one a sender can counterfeit.
#[derive(Debug, Clone)]
pub struct Withheld {
    /// Where it appeared — `img`/`src`, `link`/`href` — so the disclosure says what was lost
    /// rather than only how much.
    pub element: String,
    pub attribute: String,
    /// The address the sender wrote, shown under the same rules as a link: decoded, stripped,
    /// and never fetched from.
    pub displayed: String,
    /// Why. `AbsentAuthority` is the honest answer until the filter lists ship, and it says
    /// *shed* rather than *rule* — which is a different sentence and the true one.
    pub rule: String,
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

        // Bound before the context borrows it, because whether an allowance has anything to
        // key on is a property of the origin rather than of the render.
        let context_origin = sift_block::origin::Origin::Null;
        let mut context = sift_pipeline::Context {
            // Nothing has authenticated this message yet, so the origin is null and every
            // resource is third-party under the strictest rules. D-11's fourth priority is
            // exactly this case, and it is the correct answer rather than a placeholder.
            origin: context_origin.clone(),
            blocker: None,
            dark,
            broker: &mut self.resources,
        };
        let rendered = sift_pipeline::render(&selected, &mut context).map_err(|e| e.to_string())?;

        // A refusal is anything that is not an allow, which includes `AbsentAuthority`:
        // D-10 makes a missing filter engine **deny**, never fall through, so a shed that
        // dropped the engine shows as content withheld rather than as content loaded.
        //
        // `positions` and `verdicts` are in step by construction — the pipeline documents
        // that, and I2 is asserted over the pair — so zipping them is reading the pairing
        // rather than assuming one.
        let withheld: Vec<Withheld> = rendered
            .positions
            .iter()
            .zip(&rendered.verdicts)
            .filter(|(_, verdict)| !verdict.permits_fetch())
            .map(|(position, verdict)| Withheld {
                element: position.element.clone(),
                attribute: position.attribute.clone(),
                displayed: sift_block::link::display_form(&position.original),
                rule: describe(verdict),
            })
            .collect();

        // Nothing keys a durable allowance while the origin is null, so the control that
        // would set one is absent rather than present and ineffective.
        // **Asked of the origin, not re-derived here.** This used to spell the predicate out
        // as "not null", and when the boundary tightened to "attested" the two disagreed —
        // leaving the interface free to draw a button the boundary would refuse, which is the
        // defect class this whole change exists to remove. One predicate, one place.
        let may_always_allow = context_origin.can_carry_a_durable_allowance();

        let links: Vec<Link> = rendered
            .links
            .iter()
            // No removal rules are loaded, so nothing is stripped and the wrapper recovery is
            // the only transformation. That is the same shed as the absent authority above:
            // the answer is honest about what it did rather than claiming a cleaning it did
            // not perform.
            .map(|target| Link::of(&sift_block::link::unwrap(target, &[])))
            .collect();

        // **The allowance is carried across the re-render.** Accepting withheld content
        // re-opens the message, which mints a new token and revokes the old one — so an
        // allowance held against the token died with the click that granted it, and the
        // button was a no-op with a passing test beside it. The decision is about the
        // message, so it is applied here, to whatever token this render just minted.
        // Opening a different message ends the previous allowance: show-once applies to the
        // message in front of the user, so navigating away is what "once" is bounded by.
        if self.allowed_once_message == Some(id) {
            self.resources.allow_once(rendered.token.as_str());
        } else {
            self.allowed_once_message = None;
        }

        Ok(Document {
            html: rendered.html,
            token: rendered.token.as_str().to_owned(),
            fetching_positions: rendered.positions.len(),
            blocked: withheld.len(),
            withheld,
            may_always_allow,
            unsubscribe: unsubscribe_among(&links),
            links,
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

impl Link {
    /// The presentation-facing form of a destination the block layer resolved.
    fn of(destination: &sift_block::link::Destination) -> Self {
        Self {
            displayed: destination.display.clone(),
            target: destination.resolved.clone(),
            wrapper: destination.wrapper.clone(),
            needs_a_mail_handler: destination.needs_a_mail_handler,
        }
    }
}

/// Name a decision in the words that are true of it.
///
/// `AbsentAuthority` is not "blocked by a rule" — no rule ran. Saying so is the difference
/// between a user believing a filter list caught something and knowing that Sift withheld it
/// because it had nothing to decide with.
fn describe(decision: &sift_block::engine::Decision) -> String {
    use sift_block::engine::{Decision, Verdict};
    match decision {
        Decision::Agreed(Verdict::Block) => "a filter rule".to_owned(),
        Decision::Agreed(Verdict::Allow) => "allowed".to_owned(),
        Decision::Disagreed { authority, .. } => {
            format!("{authority:?}, with the backstop disagreeing — a defect worth reporting")
        }
        Decision::AbsentAuthority => "no filter list is loaded, so nothing is fetched".to_owned(),
    }
}

/// FR-42 — the unsubscribe destination a message declares, recovered from what is renderable.
///
/// **The header form is the authoritative one and is not available here yet**: `List-Unsubscribe`
/// is a header, and the envelope Sift stores does not carry it. Until it does, this recovers the
/// in-body form, which is what most senders provide anyway and is the one a person actually
/// clicks. When the header arrives it takes precedence and this becomes the fallback.
///
/// Matching is on the address rather than on link text because link text is the sender's, and a
/// sender who wants a click will label anything "unsubscribe".
fn unsubscribe_among(links: &[Link]) -> Option<Link> {
    links
        .iter()
        .find(|l| {
            let target = l.target.to_ascii_lowercase();
            target.contains("unsubscribe")
                || target.contains("optout")
                || target.contains("opt-out")
                || (l.needs_a_mail_handler && target.contains("unsub"))
        })
        .cloned()
}

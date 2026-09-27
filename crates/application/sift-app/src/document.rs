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

use sift_broker::broker::{Answer, Grant, Reason, Unavailable};
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
    /// Why, as the broker would answer a request for it now. With no window open or the
    /// engine shed it says *shed* rather than *rule* — a different sentence, and the true one.
    pub rule: String,
}

impl App {
    /// Fetch a message's chosen part and run it through the seven stages.
    ///
    /// **Structure first, and then one part.** The structure costs a few kilobytes; a forty
    /// megabyte attachment beside it costs nothing until somebody asks for it, which is a
    /// claim about requests rather than about intentions.
    ///
    /// `dark` asks for the dark transform; `increased_contrast` is the system's
    /// increased-contrast preference as the shell read it, which raises the threshold the
    /// transform's contrast repair targets. It changes nothing where `dark` is false.
    ///
    /// # Errors
    /// No account holds the message, it has not been synced, it carries no renderable part,
    /// or a stage refused it. A refusal is a degradation to FR-9's raw view rather than a
    /// panic — the pipeline catches at every stage boundary.
    pub fn open_document(
        &mut self,
        id: LocalId,
        dark: bool,
        increased_contrast: bool,
    ) -> Result<Document, String> {
        // "The engine MUST be loaded before the first body renders." Where a window is open
        // at L0 this loads it if nothing has yet; anywhere else it stays absent and denies.
        self.reconcile_filter_engine();
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
            blocker: Some(&self.filter),
            dark,
            // The UI shell: the system's increased-contrast preference raises the bar inside
            // the message too, not only around it.
            increased_contrast,
            broker: &mut self.resources,
        };
        let rendered = sift_pipeline::render(&selected, &mut context).map_err(|e| e.to_string())?;

        // D-81: body text and attachment filenames enter the index at first fetch, from the
        // sanitized tree — and again on a later fetch, which after an eviction is what puts a
        // body back. A write that fails does not withhold the message from the person who
        // asked to read it; the next open writes it again.
        let names: Vec<&str> = parts.iter().filter_map(|p| p.filename.as_deref()).collect();
        if let Some(account) = self.accounts.get(&owner) {
            let _ = index_body(account, id, &rendered.text, &names);
        }

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
            // No removal rules are bundled, so nothing is stripped and the wrapper recovery is
            // the only transformation. The answer is honest about what it did rather than
            // claiming a cleaning it did not perform.
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

        // **What was withheld is asked of the broker**, after the allowance above is applied,
        // so the count is the answer the body view's requests will actually receive. It used
        // to be the authority's verdicts alone, which never consulted the allowance: with an
        // engine loaded that would have said "nothing withheld" over a message whose every
        // image the broker was refusing, and hidden the very control that would load them.
        //
        // A token that names no live document cannot happen here — it was minted a moment
        // ago — but were it to, every position is reported withheld rather than none.
        let infrastructure = sift_block::origin::Infrastructure::default();
        self.resources_under_tier();
        let refusals = self
            .resources
            .withheld(rendered.token.as_str(), &self.filter, &infrastructure)
            .unwrap_or_else(|| vec![Some(Reason::Shed); rendered.positions.len()]);
        let authority_loaded = self.filter.is_loaded();
        // In step by construction: the pipeline binds one broker position per sanitized
        // position, in order.
        let withheld: Vec<Withheld> = rendered
            .positions
            .iter()
            .zip(refusals)
            .filter_map(|(position, refusal)| {
                let reason = refusal?;
                Some(Withheld {
                    element: position.element.clone(),
                    attribute: position.attribute.clone(),
                    displayed: sift_block::link::display_form(&position.original),
                    rule: describe(&reason, authority_loaded),
                })
            })
            .collect();

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
    /// Answered under the authority the application holds: the bundled lists while a window
    /// is open at L0, and **absent** otherwise — no window, or a shed tier. D-10 makes an
    /// absent authority **deny** rather than fall through, so in that state every remote
    /// fetch is refused and the reader says so. A blocker that failed open would be worse
    /// than no blocker, because the product would claim a protection it was not providing.
    pub fn resolve_resource(&mut self, url: &str, transferred: Option<u64>) -> Answer {
        self.reconcile_filter_engine();
        self.resources_under_tier();
        let request = sift_broker::broker::Request {
            url: url.to_owned(),
            transferred_length: transferred,
        };
        self.resources.answer(
            &request,
            &self.filter,
            &sift_block::origin::Infrastructure::default(),
        )
    }

    /// Answer one resource load and, where the answer is the bytes, grant the fetch.
    ///
    /// **The same decision as [`App::resolve_resource`]**, taken under the same authority
    /// and tier. The grant is all that leaves: the fetch it permits is [`fetch_resource`]'s,
    /// and runs with nothing of the application's held — D-91 keeps blocking work off the
    /// thread the engine asked on, and #114 keeps the session off a network round trip.
    ///
    /// # Errors
    /// The blocked or unavailable answer the request gets instead.
    pub fn grant_resource(&mut self, url: &str) -> Result<Grant, Answer> {
        self.reconcile_filter_engine();
        self.resources_under_tier();
        self.resources.grant(
            &sift_broker::broker::Request {
                url: url.to_owned(),
                transferred_length: None,
            },
            &self.filter,
            &sift_block::origin::Infrastructure::default(),
        )
    }

    /// Put the broker under the tier last recorded, so a fetch and the count a reader is shown
    /// are both decided by the network the reader is actually on.
    pub(crate) const fn resources_under_tier(&mut self) {
        self.resources.tier_permits_fetch = fetches_on_demand(self.network);
        self.resources.tier_permits_prefetch = self.network.prefetch();
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

/// Whether a tier permits a fetch a person asked for — an allowed image in the message they
/// are reading.
///
/// **The on-demand rule `network-conditions.md` states for server-side search**, and for the
/// same reason: one bounded request a person asked for, which NFR-31 excludes from Minimal's
/// steady-state figure as it excludes opening a message. Not while paused, which stops
/// fetching; not behind a captive portal, whose one reattempt is the account's; not with no
/// path, where NFR-38 allows zero attempts. Prefetch is a different question and NFR-32
/// answers it separately.
pub(crate) const fn fetches_on_demand(tier: sift_net::tier::Tier) -> bool {
    tier.permits_server_search()
}

/// A fetch the broker granted, validated and on its way to the body view.
pub type ResourceStream = sift_broker::stream::Stream<Remote>;

/// Fetch what a grant allows, and validate it before a byte is handed over.
///
/// **Blocking**, and called with nothing held: the slot wait, the connection, and the header
/// all happen here, so the caller runs it off the engine's thread and outside the session.
/// The deadline starts now, which is the moment the grant is carried away from the broker.
///
/// # Errors
/// The blocked or unavailable answer the request gets instead of the bytes.
pub fn fetch_resource(grant: Grant) -> Result<ResourceStream, Answer> {
    sift_broker::stream::open(
        grant,
        &mut Web,
        std::time::Instant::now() + sift_broker::stream::LOAD_DEADLINE,
    )
}

/// The broker's outward edge, over the one transport Sift has — D-59's trait, implemented at
/// the layer that may reach the network.
///
/// **What leaves is the address and nothing that identifies the reader**: no cookie, no
/// referrer, no header naming Sift or its version. The user agent is the generic token some
/// image hosts refuse to answer without, and the same for every installation.
#[derive(Debug, Clone, Copy)]
struct Web;

impl sift_broker::stream::Fetch for Web {
    type Source = Remote;

    fn open(&mut self, url: &str) -> Result<Remote, Unavailable> {
        let (host, target) = remote_target(url).ok_or(Unavailable::NotFetchable)?;
        let mut https = sift_http::Https::to(&host).map_err(|_| Unavailable::NotFetchable)?;
        let streamed = https
            .get_streaming(
                &target,
                &[
                    (
                        "accept".to_owned(),
                        "image/png,image/gif,image/jpeg,image/webp".to_owned(),
                    ),
                    ("user-agent".to_owned(), "Mozilla/5.0".to_owned()),
                ],
            )
            .map_err(|_| Unavailable::NotFetchable)?;
        // Only the resource itself. A redirect names an address no check above has seen, and
        // following it would be a fetch the broker never decided.
        if streamed.head().status != 200 {
            return Err(Unavailable::NotFetchable);
        }
        let declared = streamed
            .head()
            .get("content-length")
            .and_then(|v| v.trim().parse().ok());
        Ok(Remote { streamed, declared })
    }
}

/// One remote resource's body, as the transport reads it.
#[derive(Debug)]
pub struct Remote {
    streamed: sift_http::Streamed,
    declared: Option<u64>,
}

impl sift_broker::stream::Source for Remote {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Unavailable> {
        std::io::Read::read(&mut self.streamed, buf).map_err(|_| Unavailable::NotFetchable)
    }

    fn declared_length(&self) -> Option<u64> {
        self.declared
    }
}

/// The host and request target a position's address is fetched from, or `None` where it is
/// not one Sift will fetch.
///
/// **HTTPS only, on its own port.** A plain `http:` address is fetched over TLS at the same
/// host and path — the upgrade a browser's HTTPS-only mode makes — because a cleartext fetch
/// would disclose the reader to every network between them and the sender, and fails rather
/// than falling back. An explicit port other than the scheme's own, credentials in the
/// address, an address literal, and a local name are refused: each reaches something on the
/// reader's own network rather than a sender's image host.
///
/// The target is written into a request line, so every byte of it is either printable ASCII
/// or percent-encoded, and a control character refuses the whole address rather than being
/// encoded into one the server will decode.
fn remote_target(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => "443",
        "http" => "80",
        _ => return None,
    };
    let rest = rest.split('#').next().unwrap_or("");
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(split);
    if authority.contains('@') || authority.starts_with('[') {
        return None;
    }
    let host = match authority.split_once(':') {
        Some((host, port)) if port == default_port || port.is_empty() => host,
        Some(_) => return None,
        None => authority,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal");
    let literal = host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    let well_formed = !host.is_empty()
        && host.contains('.')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if local || literal || !well_formed {
        return None;
    }

    let mut target = String::with_capacity(path.len() + 1);
    if !path.starts_with('/') {
        target.push('/');
    }
    for b in path.bytes() {
        match b {
            0x00..=0x1f | 0x7f => return None,
            b' ' | 0x80..=0xff | b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}' => {
                target.push_str(&format!("%{b:02X}"))
            }
            _ => target.push(char::from(b)),
        }
    }
    Some((host, target))
}

/// Write a message's body text and attachment filenames into its index entry — D-81's
/// first-fetch half.
///
/// Into the entry the message row already has: envelope fields arrived with the row at ingest,
/// and this adds what only a fetch can supply. Written in the index's normalization form, the
/// same one NFR-54 applies, so the text the index holds is the text it tokenized. Nothing is
/// written where the entry already holds exactly this, so reopening a message costs no write.
///
/// # Errors
/// The store refused.
pub(crate) fn index_body(
    account: &crate::OpenAccount,
    id: LocalId,
    text: &str,
    attachments: &[&str],
) -> Result<usize, String> {
    let body = sift_foundation::normalize::for_index(text);
    let names = sift_foundation::normalize::for_index(&attachments.join("\n"));
    account
        .store
        .store
        .execute(
            "UPDATE message_text SET body = ?2, attachments = ?3
             WHERE rowid = (SELECT docid FROM message_text_key WHERE message_id = ?1)
               AND (body IS NOT ?2 OR attachments IS NOT ?3)",
            rusqlite::params![id.to_bytes().to_vec(), body, names],
        )
        .map_err(|e| e.to_string())
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

/// Name a refusal in the words that are true of it.
///
/// The shed is not "blocked by a rule" — no rule ran. Saying so is the difference between a
/// user believing a filter list caught something and knowing that Sift withheld it because it
/// had nothing to decide with.
///
/// **With the authority absent, a sender refusal is reported as the shed**, because that is
/// the answer that stays true after the user acts: allowing the sender would still fetch
/// nothing, and a reason that invites a click which cannot help is the complaint FR-8's
/// button was built to answer.
///
/// **Every sentence is in the reader's terms** (failure-model: a notice names its cause in
/// the user's terms). The layer verdicts, the heuristic evidence, and the limits-register
/// identifier belong to FR-33's debug view, not here: a reader cannot act on
/// "authority and backstop agreed: Block", and a `Debug` dump of findings is not a cause.
fn describe(reason: &Reason, authority_loaded: bool) -> String {
    match reason {
        Reason::Shed => SHED.to_owned(),
        Reason::NotAllowedBySender if !authority_loaded => SHED.to_owned(),
        Reason::NotAllowedBySender => "remote content is blocked until you allow it".to_owned(),
        Reason::Rule(_) => "a filter list identifies this address as a tracker".to_owned(),
        Reason::Heuristic(findings) => {
            let signs: Vec<&str> = findings.iter().map(|f| f.reason.name()).collect();
            if signs.is_empty() {
                "it looks like a tracker".to_owned()
            } else {
                format!("it looks like a tracker: {}", signs.join(", "))
            }
        }
        Reason::NetworkPolicy => "the current network policy allows no fetches".to_owned(),
        Reason::Bounds(bound) if bound.starts_with("L-11") || bound.starts_with("L-12") => {
            "the image's dimensions are larger than Sift will decode".to_owned()
        }
        Reason::Bounds(_) => "the image is larger than Sift will load".to_owned(),
        Reason::Validation(_) => "it is not an image format Sift will display".to_owned(),
    }
}

/// FR-33's reason when D-10's authority is absent.
const SHED: &str = "no filter list is loaded, so nothing is fetched";

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

#[cfg(test)]
mod tests {
    use super::*;
    use sift_block::heuristic::{Finding, Reason as Sign};

    /// No reader-facing reason may leak a layer verdict, a limits identifier, or `Debug`
    /// output — the failure model's "names its cause in the user's terms".
    fn in_the_readers_terms(text: &str) {
        for leak in ["authority", "backstop", "L-1", "{", "(", "Finding", "Block"] {
            assert!(!text.contains(leak), "{leak:?} leaked into {text:?}");
        }
    }

    #[test]
    fn a_validation_refusal_and_a_raster_bound_are_in_the_readers_terms() {
        for reason in [
            Reason::Validation("not a raster format the broker passes through"),
            Reason::Bounds("L-12 rasterized pixels"),
        ] {
            in_the_readers_terms(&describe(&reason, true));
        }
        assert_eq!(
            describe(&Reason::Bounds("L-12 rasterized pixels"), true),
            describe(&Reason::Bounds("L-11 decoded pixels"), true),
            "the two pixel bounds are one sentence to a reader"
        );
    }

    #[test]
    fn a_remote_address_is_fetched_over_tls_at_its_own_host_and_path() {
        assert_eq!(
            remote_target("https://cdn.example.test/a/b.png?x=1#frag"),
            Some(("cdn.example.test".to_owned(), "/a/b.png?x=1".to_owned()))
        );
        // Upgraded, never fetched in the clear.
        assert_eq!(
            remote_target("http://CDN.example.test:80?q"),
            Some(("cdn.example.test".to_owned(), "/?q".to_owned()))
        );
        assert_eq!(
            remote_target("https://cdn.example.test/a b/é"),
            Some(("cdn.example.test".to_owned(), "/a%20b/%C3%A9".to_owned()))
        );
    }

    #[test]
    fn an_address_that_reaches_the_readers_own_network_is_not_fetched() {
        for url in [
            "https://localhost/x.png",
            "https://printer.local/x.png",
            "https://192.168.1.1/x.png",
            "https://[::1]/x.png",
            "https://user:pass@cdn.example.test/x.png",
            "https://cdn.example.test:8443/x.png",
            "http://cdn.example.test:443/x.png",
            "ftp://cdn.example.test/x.png",
            "file:///etc/passwd",
            "https:///x.png",
            "https://intranet/x.png",
        ] {
            assert_eq!(remote_target(url), None, "{url}");
        }
    }

    #[test]
    fn a_control_character_refuses_the_address_rather_than_reaching_the_request_line() {
        // The target is written into `GET <target> HTTP/1.1`, so a line break in it would be
        // a second header the sender wrote.
        assert_eq!(
            remote_target("https://cdn.example.test/x.png\r\nx-injected: 1"),
            None
        );
    }

    #[test]
    fn an_allowed_image_is_fetched_on_demand_in_every_tier_that_permits_a_fetch() {
        use sift_net::tier::Tier;
        for tier in [Tier::Unrestricted, Tier::Conservative, Tier::Minimal] {
            assert!(fetches_on_demand(tier), "{tier:?}");
        }
        for tier in [Tier::OfflinePortal, Tier::OfflineNoPath, Tier::Paused] {
            assert!(!fetches_on_demand(tier), "{tier:?}");
        }
    }

    #[test]
    fn the_broker_follows_the_recorded_tier_and_never_prefetches_outside_unrestricted() {
        use sift_net::tier::Tier;
        let mut app = App::new();
        assert!(
            !app.resources.may_prefetch(),
            "the unknown state prefetched"
        );
        assert!(app.resources.tier_permits_fetch);
        app.set_network_tier(Tier::Paused);
        assert!(!app.resources.tier_permits_fetch, "a pause still fetched");
        app.set_network_tier(Tier::Unrestricted);
        assert!(app.resources.may_prefetch());
    }

    #[test]
    fn a_rule_refusal_names_a_filter_list_not_the_layer_verdict() {
        let text = describe(
            &Reason::Rule("authority and backstop agreed: Block".to_owned()),
            true,
        );
        assert_eq!(text, "a filter list identifies this address as a tracker");
        in_the_readers_terms(&text);
    }

    #[test]
    fn a_heuristic_refusal_names_each_sign_in_words() {
        let findings = vec![
            Finding {
                reason: Sign::PixelDimensions,
                evidence: "1x1".to_owned(),
            },
            Finding {
                reason: Sign::HiddenByStyle,
                evidence: "display:none".to_owned(),
            },
        ];
        let text = describe(&Reason::Heuristic(findings), true);
        assert_eq!(
            text,
            "it looks like a tracker: tracking-pixel dimensions, hidden by style"
        );
        in_the_readers_terms(&text);
        assert_eq!(
            describe(&Reason::Heuristic(Vec::new()), true),
            "it looks like a tracker"
        );
    }

    #[test]
    fn a_network_policy_refusal_says_so() {
        let text = describe(&Reason::NetworkPolicy, true);
        assert_eq!(text, "the current network policy allows no fetches");
        in_the_readers_terms(&text);
    }

    #[test]
    fn a_bound_refusal_names_the_size_not_the_register_entry() {
        let pixels = describe(&Reason::Bounds("L-11 decoded pixels"), true);
        assert_eq!(
            pixels,
            "the image's dimensions are larger than Sift will decode"
        );
        in_the_readers_terms(&pixels);
        let bytes = describe(&Reason::Bounds("L-10 image bytes"), true);
        assert_eq!(bytes, "the image is larger than Sift will load");
        in_the_readers_terms(&bytes);
    }
}

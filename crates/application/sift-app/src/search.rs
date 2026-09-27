//! FR-19, FR-20 and FR-21 — searching, and being honest about what was searched.
//!
//! # The interpretation is part of the result
//!
//! A structured query language nobody can see the parse of is one where a typo silently
//! becomes a free-text word and the user concludes their mail is missing. So a search returns
//! **what it understood** alongside what it found, and the shell shows it. That is a UX call
//! rather than a requirement, and it is the one that makes the operators usable: `form:alice`
//! is a plausible typo for `from:alice`, and the two produce very different result sets.
//!
//! An operator this build does not know stays free text rather than being dropped. `foo:bar`
//! in a subject line is something somebody might genuinely be searching for, and discarding it
//! would return the wrong results rather than none.
//!
//! # The whole mailbox, and then the limit
//!
//! Each account's own full-text index (D-5, D-80, one per account under D-6) is searched for
//! every message that satisfies every term, with FR-20's structured operators applied as
//! predicates on those same rows, and hands back its leading matches in D-79's order — as many
//! as could still be shown once the overlay has had its say, and no more, so a keystroke's cost
//! does not grow with the mailbox (NFR-5). The accounts' results are merged under D-79 — on features
//! that mean the same thing in every account, falling back to D-55's list order where the
//! query carries no relevance signal — and only **then** truncated to the number asked for.
//! Truncating first would search the newest page of each mailbox and report the rest as not
//! containing a match, which is indistinguishable from a mailbox with no such message in it.
//!
//! Rows are read **through the overlay** (D-51) as the merged order is walked, so a message the
//! user has just archived is not a result, and `is:unread` answers with the state the user
//! sees rather than the server's.
//!
//! # What is searched, and what is not, stated rather than implied
//!
//! Every message's sender, recipients, subject and preview are indexed at ingest. Body text
//! and attachment filenames are indexed when a body is **first fetched** (D-81), so the bodies
//! of messages nobody has opened are not in the local index — D-81's stated consequence, and
//! [`Report::caveats`] says so under every query it applies to. FR-21's server-side search is
//! how a query reaches those bodies, and a search that admits its scope is better than one that
//! quietly covers less than the user assumes.
//!
//! # FR-21 — two halves, and their labels
//!
//! [`App::search`] is the local half: no network, every keystroke. It also plans the server
//! half and reports how many accounts it would ask ([`Report::delegable_accounts`]), and why any
//! account the policy tier holds back will not be asked. [`App::search_with_server`] is the
//! second half, which a shell runs after the first is on screen and off its main loop: each
//! account whose declared capability covers the query is asked for at most L-32 identifiers,
//! what the store does not hold arrives as envelopes and is ingested, and the result joins the
//! local candidates under D-79. Results are labelled by where they came from, because merging
//! two sources without saying which is which is the shape of the mistake FR-21 exists to
//! prevent.

use std::collections::{BTreeSet, HashMap};

use rusqlite::OptionalExtension as _;
use sift_foundation::identity::LocalId;
use sift_foundation::limits::L32_SERVER_SEARCH_HITS;
use sift_index::merge::{self, Features, Result_};
use sift_index::query::{Column, Query, Term, TextScope};
use sift_provider::adapter::{Failure, SearchTerm};
use sift_provider::erased::ErasedAdapter;

use crate::rows::MessageRow;
use crate::{App, OpenAccount};

/// Where a result came from — FR-21.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Sift's own index found it.
    Local,
    /// The provider's search found it and Sift's own index did not — usually because the words
    /// matched are in a body nobody has opened. A message both found is labelled
    /// [`Source::Local`]: it is one row, and it was already in the local results.
    Server,
}

/// One result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub row: MessageRow,
    pub source: Source,
    /// D-79's corpus-independent features of this message against this query — what placed
    /// it where it is in the merged order.
    pub features: Features,
}

/// What a search found, and what it understood.
#[derive(Debug, Clone)]
pub struct Report {
    /// In D-79's merged order, truncated to the limit after the merge.
    pub hits: Vec<Hit>,
    /// How each term was read, in the order it was written. Shown to the user.
    pub interpretation: Vec<String>,
    /// What this query asked for that this build cannot answer, in the user's words.
    ///
    /// **Empty results and unsearched fields look identical**, and that is the failure this
    /// exists to prevent: a person who searches `has:attachment`, gets nothing, and concludes
    /// they have no attachments has been misled by a filter that was never evaluated. A search
    /// that admits its scope is better than one that quietly covers less than assumed.
    pub caveats: Vec<String>,
    /// How many accounts could search server-side for this query and have not been asked.
    ///
    /// Non-zero from [`App::search`] exactly when [`App::search_with_server`] has something to
    /// add, which is how a shell knows to ask for the second half — and zero from the second
    /// half itself, which asked every one of them.
    pub delegable_accounts: usize,
}

/// Whether, and how, one account's server is asked about one query.
enum Plan {
    Ask {
        /// FR-20's terms this account evaluates itself.
        terms: Vec<SearchTerm>,
        /// The rest, applied locally to what the server returns.
        local_only: Query,
    },
    /// Not asked. `Some` carries what the report must say about it; `None` is an account that
    /// could not have been asked about this query at all, whose silence is already covered by
    /// the local caveats.
    Skip(Option<String>),
}

/// What the server half managed to cover, across every account in scope.
#[derive(Debug, Clone, Copy, Default)]
struct Coverage {
    /// Every account's server searched the text, so unopened bodies were reached.
    bodies: bool,
    /// And every one evaluated attachment presence itself.
    attachments: bool,
}

impl App {
    /// Search every account, or one.
    ///
    /// # Errors
    /// The named account does not exist, or the store refused.
    pub fn search(
        &mut self,
        input: &str,
        account: Option<&str>,
        limit: u32,
    ) -> Result<Report, String> {
        let query = Query::parse(input);
        let names = self.scope(account)?;
        let (owner, candidates) = self.local_candidates(&names, &query, limit)?;

        // Nothing leaves the process here. What the server half *would* do is planned, so the
        // shell knows whether to ask for it, and an account the tier or a pause holds back says
        // so now rather than after a second round trip that was never going to happen.
        let mut delegable = 0;
        let mut notes = Vec::new();
        for name in &names {
            match self.plan(name, &query) {
                Plan::Ask { .. } => delegable += 1,
                Plan::Skip(Some(note)) => notes.push(note),
                Plan::Skip(None) => {}
            }
        }

        let mut report = self.assemble(&query, candidates, &owner, limit)?;
        report.caveats = caveats(&query, Coverage::default());
        report.caveats.extend(notes);
        report.delegable_accounts = delegable;
        Ok(report)
    }

    /// Search every account, or one, **and ask each provider that can** — FR-21.
    ///
    /// The second half of a search, and the only one that touches the network. A shell shows
    /// [`App::search`]'s local results first and calls this afterwards, off its main loop (#49),
    /// when that report's [`Report::delegable_accounts`] is non-zero; a slow or failing provider
    /// therefore delays only its own half and never the local one.
    ///
    /// For each account whose capability covers the query and whose tier permits it, the
    /// provider is asked for at most L-32 identifiers. Those the store does not hold have their
    /// **envelopes** fetched and ingested as discovered — never a body — so every result can be
    /// opened and triaged like any other row, and the body a person then opens is indexed by
    /// D-81's first-fetch rule. Terms the provider does not evaluate are applied locally to what
    /// it returned. The server's hits join the local candidates under D-79, deduplicated to the
    /// local row where the local index already found the message, and labelled
    /// [`Source::Server`] where it did not.
    ///
    /// **The query is not retained.** It lives for this call and in the request that carries
    /// it, and nothing here writes it anywhere — privacy's *queries are not retained*.
    ///
    /// # Errors
    /// The named account does not exist, or the store refused. A provider that fails, refuses
    /// or cannot be reached is not an error: its half is skipped and the report says so.
    pub fn search_with_server(
        &mut self,
        input: &str,
        account: Option<&str>,
        limit: u32,
    ) -> Result<Report, String> {
        let query = Query::parse(input);
        let names = self.scope(account)?;
        let (mut owner, mut candidates) = self.local_candidates(&names, &query, limit)?;
        let local: BTreeSet<u128> = candidates.iter().map(|c| c.message).collect();

        let mut notes = Vec::new();
        let mut coverage = Coverage {
            bodies: true,
            attachments: true,
        };
        for name in &names {
            let (terms, local_only) = match self.plan(name, &query) {
                Plan::Ask { terms, local_only } => (terms, local_only),
                Plan::Skip(note) => {
                    coverage = Coverage::default();
                    notes.extend(note);
                    continue;
                }
            };
            let evaluates_attachments = !query
                .terms
                .iter()
                .any(|t| matches!(t, Term::HasAttachment(_)))
                || terms
                    .iter()
                    .any(|t| matches!(t, SearchTerm::HasAttachment(_)));
            let found = match self.ask(name, &terms)? {
                Ok(found) => found,
                Err(note) => {
                    coverage = Coverage::default();
                    notes.push(note);
                    continue;
                }
            };
            coverage.attachments &= evaluates_attachments;
            if found.len() >= usize::try_from(L32_SERVER_SEARCH_HITS).unwrap_or(usize::MAX) {
                notes.push(format!(
                    "`{name}`'s server returned its first {L32_SERVER_SEARCH_HITS} matches, and \
                     may hold more. A narrower search shows them."
                ));
            }
            if !local_only.terms.is_empty() {
                notes.push(format!(
                    "`{name}`'s server cannot evaluate {}, so that was applied to the messages it \
                     returned rather than to its whole mailbox.",
                    local_only
                        .terms
                        .iter()
                        .map(describe)
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }

            let account = self
                .accounts
                .get(name)
                .ok_or_else(|| format!("no account `{name}`"))?;
            let count = u32::try_from(found.len()).unwrap_or(u32::MAX);
            let passing: Vec<LocalId> = matching(account, &local_only, count, Some(&found))?
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| !local.contains(&id.as_u128()))
                .collect();
            // A hit the local index also matches is a local result the local cut had not
            // reached, and is labelled as one; only what the local index cannot find is the
            // server's. Either way it is one row.
            let also_local = matching(account, &query, count, Some(&passing))?;
            let also: BTreeSet<u128> = also_local.iter().map(|(id, _)| id.as_u128()).collect();
            let server_only: Vec<LocalId> = passing
                .iter()
                .copied()
                .filter(|id| !also.contains(&id.as_u128()))
                .collect();
            let labelled = also_local
                .into_iter()
                .map(|(id, features)| (id, merge::Source::Local, features))
                .chain(
                    server_features(account, &query, &server_only)?
                        .into_iter()
                        .map(|(id, features)| (id, merge::Source::Server, features)),
                );
            for (id, source, features) in labelled {
                owner.insert(id.as_u128(), name);
                candidates.push(Result_ {
                    message: id.as_u128(),
                    source,
                    features,
                    local_score: None,
                });
            }
        }

        let mut report = self.assemble(&query, candidates, &owner, limit)?;
        report.caveats = caveats(&query, coverage);
        report.caveats.extend(notes);
        report.delegable_accounts = 0;
        Ok(report)
    }

    /// The accounts a search covers: the one named, or every one.
    fn scope(&self, account: Option<&str>) -> Result<Vec<String>, String> {
        Ok(match account {
            Some(name) => {
                if !self.accounts.contains_key(name) {
                    return Err(format!("no account `{name}`"));
                }
                vec![name.to_owned()]
            }
            None => self
                .account_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        })
    }

    /// Each account's leading local matches in D-79's order — enough of them that the merged
    /// order's first `limit` visible rows are all among them — before anything is cut.
    fn local_candidates<'n>(
        &self,
        names: &'n [String],
        query: &Query,
        limit: u32,
    ) -> Result<(HashMap<u128, &'n str>, Vec<Result_>), String> {
        let mut owner: HashMap<u128, &str> = HashMap::new();
        let mut candidates = Vec::new();
        for name in names {
            let account = self
                .accounts
                .get(name)
                .ok_or_else(|| format!("no account `{name}`"))?;
            for (id, features) in matching(account, query, limit, None)? {
                owner.insert(id.as_u128(), name);
                candidates.push(Result_ {
                    message: id.as_u128(),
                    source: merge::Source::Local,
                    features,
                    local_score: None,
                });
            }
        }
        Ok((owner, candidates))
    }

    /// D-79's order across accounts and sources, and only then the limit — applied to what the
    /// user will actually see, so a row the overlay hides does not use up a place.
    fn assemble(
        &self,
        query: &Query,
        candidates: Vec<Result_>,
        owner: &HashMap<u128, &str>,
        limit: u32,
    ) -> Result<Report, String> {
        let mut hits = Vec::new();
        for result in merge::merge(query, candidates) {
            if hits.len() >= limit as usize {
                break;
            }
            let Some(account) = owner
                .get(&result.message)
                .and_then(|name| self.accounts.get(*name))
            else {
                continue;
            };
            let Some(row) = crate::rows::message_row(account, LocalId::from_u128(result.message))?
            else {
                continue;
            };
            if seen_as_asked(query, &row) {
                hits.push(Hit {
                    row,
                    source: match result.source {
                        merge::Source::Local => Source::Local,
                        merge::Source::Server => Source::Server,
                    },
                    features: result.features,
                });
            }
        }
        Ok(Report {
            hits,
            interpretation: query.terms.iter().map(describe).collect(),
            caveats: Vec::new(),
            delegable_accounts: 0,
        })
    }

    /// Whether one account's server is asked about `query`, and with which terms.
    ///
    /// **Capability first, never provider** (D-12): the account's declared
    /// [`ServerSearch`](sift_provider::capability::ServerSearch) decides which terms are sent,
    /// and the rest are applied locally to what comes back. Then the policy tier — FR-21 puts
    /// server search under it — and the account's own pause (D-95).
    fn plan(&self, name: &str, query: &Query) -> Plan {
        let Some(account) = self.accounts.get(name) else {
            return Plan::Skip(None);
        };
        let capability = account.capabilities.server_search;
        // A capability shape has no provider behind it, and never will.
        if !capability.offered()
            || (account.adapter.is_none() && crate::shape_named(&account.kind).is_ok())
        {
            return Plan::Skip(None);
        }

        let mut terms = Vec::new();
        let mut local_only = Vec::new();
        for term in &query.terms {
            // A text term of punctuation alone asks nothing locally, and is not sent either.
            if term.text_scope().is_some_and(|scope| scope.is_empty()) {
                continue;
            }
            match delegated(term) {
                Some(wire) if capability.evaluates(&wire) => terms.push(wire),
                _ => local_only.push(term.clone()),
            }
        }
        // Worth a request only if the server can say something the local index cannot: text,
        // which reaches bodies nobody opened, or attachment presence, which sync does not
        // record. A query of flags, folders and dates is a list, and the list is local.
        let reaches_further = terms.iter().any(|t| {
            matches!(
                t,
                SearchTerm::Word(_)
                    | SearchTerm::Phrase(_)
                    | SearchTerm::Sender(_)
                    | SearchTerm::Recipient(_)
                    | SearchTerm::Subject(_)
                    | SearchTerm::HasAttachment(_)
            )
        });
        if !reaches_further {
            return Plan::Skip(None);
        }

        let held = |why: &str| {
            Plan::Skip(Some(format!(
                "`{name}` was not searched on its server because {why}, so only the mail Sift \
                 already holds was searched there."
            )))
        };
        if self.is_paused(name) {
            return held("syncing is paused for it");
        }
        match self.network {
            sift_net::tier::Tier::OfflineNoPath => return held("there is no network connection"),
            sift_net::tier::Tier::OfflinePortal => {
                return held("the network is asking you to sign in to it");
            }
            sift_net::tier::Tier::Paused => return held("syncing is paused"),
            tier if !tier.permits_server_search() => return held("the network policy forbids it"),
            _ => {}
        }
        if account.needs_authentication {
            return held("it needs you to sign in again");
        }
        Plan::Ask {
            terms,
            local_only: Query { terms: local_only },
        }
    }

    /// Ask one account's provider, and bring what it found into the store.
    ///
    /// Answers the local identities of the messages it found, in the provider's order, or the
    /// sentence the report carries about why it could not.
    ///
    /// # Errors
    /// The store refused — which, unlike a provider's failure, is not something to report and
    /// carry on from.
    fn ask(
        &mut self,
        name: &str,
        terms: &[SearchTerm],
    ) -> Result<Result<Vec<LocalId>, String>, String> {
        let failed = |kind: &str| {
            format!(
                "`{name}`'s server search did not complete ({kind}), so only the mail Sift \
                 already holds was searched there."
            )
        };
        // A restored account is reconnected the way a sync reconnects it: this is already a
        // network call, and a launch that reconnected every account up front would be one that
        // waits on the network.
        if self.account(name)?.adapter.is_none()
            && let Err(why) = self.reconnect(name)
        {
            return Ok(Err(failed(&why)));
        }
        let account = self.account(name)?;
        let Some(adapter) = account.adapter.take() else {
            return Ok(Err(failed("it has no provider behind it")));
        };
        let outcome = found(adapter.as_ref(), account, terms);
        // The adapter goes back before the result is examined, as it does after a sync.
        account.adapter = Some(adapter);
        Ok(outcome?.map_err(|failure| failed(failure_words(failure))))
    }
}

/// One search against one provider: identifiers, then envelopes for the ones not held, then
/// the local identity of each. The inner error is the provider's; the outer one the store's.
fn found(
    adapter: &dyn ErasedAdapter,
    account: &mut OpenAccount,
    terms: &[SearchTerm],
) -> Result<Result<Vec<LocalId>, Failure>, String> {
    let bound = usize::try_from(L32_SERVER_SEARCH_HITS).unwrap_or(usize::MAX);
    let remote = match ErasedAdapter::search(adapter, terms, bound) {
        Ok(ids) => ids,
        Err(e) => return Ok(Err(e.failure)),
    };
    let unknown = sift_sync::ingest::unknown_to_us(&account.store.store, &remote)
        .map_err(|e| e.to_string())?;
    // Envelopes only, batched as a sync batches them. *Sift MUST NOT fetch whole messages*
    // holds here as it does for a backfill.
    let batch = account.capabilities.batch_size() as usize;
    let mut envelopes = Vec::with_capacity(unknown.len());
    for chunk in unknown.chunks(batch.max(1)) {
        match ErasedAdapter::fetch_envelopes(adapter, chunk) {
            Ok(mut more) => envelopes.append(&mut more),
            Err(e) => return Ok(Err(e.failure)),
        }
    }
    if !envelopes.is_empty() {
        sift_sync::ingest::ingest_found(&mut account.store.store, &envelopes, &account.ids)
            .map_err(|e| e.to_string())?;
    }

    // Every identifier the provider named that is now a row — whether it was held before or
    // arrived just now. One whose envelope did not come back vanished between the two
    // requests, and is not a result.
    let mut stmt = account
        .store
        .store
        .prepare("SELECT id FROM message WHERE remote_id = ?1")
        .map_err(|e| e.to_string())?;
    let mut local = Vec::with_capacity(remote.len());
    for id in &remote {
        let key: Option<Vec<u8>> = stmt
            .query_row([&id.0], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(key) = key {
            let bytes: [u8; 16] = key.try_into().map_err(|_| "identity is not 16 bytes")?;
            local.push(LocalId::from_bytes(bytes));
        }
    }
    Ok(Ok(local))
}

/// A provider's failure, in words a person can act on. The kind only: what the provider said
/// is a content value under D-68, and a report shown in the window is not where it belongs.
const fn failure_words(failure: Failure) -> &'static str {
    match failure {
        Failure::CredentialRefused => "it needs you to sign in again",
        Failure::Throttled { .. } => "the provider asked Sift to slow down",
        Failure::Transient | Failure::Unknown | Failure::CursorInvalidated => {
            "the provider did not answer"
        }
        Failure::Permanent => "the provider refused the search",
    }
}

/// FR-20's term as a provider is asked it, or `None` for one no provider is asked: an operator
/// this build does not know stays text, and is applied locally rather than guessed at.
fn delegated(term: &Term) -> Option<SearchTerm> {
    Some(match term {
        Term::Word(t) => SearchTerm::Word(t.clone()),
        Term::Phrase(t) => SearchTerm::Phrase(t.clone()),
        Term::Sender(t) => SearchTerm::Sender(t.clone()),
        Term::Recipient(t) => SearchTerm::Recipient(t.clone()),
        Term::Subject(t) => SearchTerm::Subject(t.clone()),
        Term::HasAttachment(want) => SearchTerm::HasAttachment(*want),
        Term::Unread(want) => SearchTerm::Unread(*want),
        Term::Location(t) => SearchTerm::Location(t.clone()),
        Term::Before(millis) => SearchTerm::Before(*millis),
        Term::After(millis) => SearchTerm::After(*millis),
        Term::Unknown(_) => return None,
    })
}

/// D-79's features of messages a provider found, against the whole query.
///
/// **Computed from the message, not from the provider's ranking**, which is what lets a server
/// result join a merge whose other members came from local indexes (D-79). Which field matched
/// is asked of the local index, which holds every envelope field; a message whose match is in
/// none of them matched where only the server could see — its body — and is credited there.
/// The provider matched every term it was sent and the local filter every other, so coverage
/// is complete.
fn server_features(
    account: &OpenAccount,
    query: &Query,
    ids: &[LocalId],
) -> Result<Vec<(LocalId, Features)>, String> {
    let scopes: Vec<(TextScope<'_>, bool)> = query
        .terms
        .iter()
        .filter_map(|t| {
            t.text_scope()
                .map(|scope| (scope, matches!(t, Term::Phrase(_))))
        })
        .filter(|(scope, _)| !scope.is_empty())
        .collect();
    let relevance = query.carries_a_relevance_signal() && !scopes.is_empty();
    let terms = u32::try_from(scopes.len()).unwrap_or(u32::MAX);
    let store = &account.store.store;
    let mut matched = store
        .prepare(
            "SELECT EXISTS (SELECT 1 FROM message_text_key k
                            WHERE k.message_id = ?1
                              AND k.docid IN (SELECT rowid FROM message_text
                                              WHERE message_text MATCH ?2))",
        )
        .map_err(|e| e.to_string())?;
    let mut received = store
        .prepare("SELECT received_at_millis FROM message WHERE id = ?1")
        .map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let key = id.to_bytes().to_vec();
        let mut credit = |wanted: &[Column]| -> Result<bool, String> {
            if !relevance {
                return Ok(false);
            }
            for (scope, _) in &scopes {
                let columns: Vec<Column> = wanted
                    .iter()
                    .copied()
                    .filter(|c| scope.columns.contains(c))
                    .collect();
                if columns.is_empty() {
                    continue;
                }
                let hit: bool = matched
                    .query_row(rusqlite::params![key, scope.expression(&columns)], |r| {
                        r.get(0)
                    })
                    .map_err(|e| e.to_string())?;
                if hit {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        let subject = credit(&[Column::Subject])?;
        let sender = credit(&[Column::Sender])?;
        let local_body = credit(&[Column::Snippet, Column::Body])?;
        let millis: i64 = received
            .query_row([&key], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        out.push((
            *id,
            Features {
                matched_subject: subject,
                matched_sender: sender,
                matched_body: local_body || (relevance && !subject && !sender),
                matched_phrase: relevance && scopes.iter().any(|(_, phrase)| *phrase),
                terms_present: terms,
                terms_total: terms,
                received_millis: u64::try_from(millis).unwrap_or(0),
            },
        ));
    }
    Ok(out)
}

/// One account's leading messages that satisfy every term, in D-79's order, with its features
/// of each.
///
/// Terms are conjunctive, which is what a person means by typing two of them. The text terms
/// are one query against the account's full-text index; the structured operators are
/// predicates on the same rows, in the same statement, so nothing is cut before they apply.
///
/// # Bounded per keystroke (NFR-5)
///
/// The order, the field credit and the cut are all the statement's, so the rows that leave the
/// store are at most `limit` plus the messages the overlay has an opinion about — never the
/// whole mailbox, which is what a one-letter prefix or a bare `is:unread` would otherwise read.
/// That count is exact rather than generous: the merged order's first `limit` visible rows
/// from this account are its first `limit` rows here, except for rows the overlay hides or
/// re-reads, and every one of those is a message the overlay names.
///
/// `only` restricts the statement to the messages a provider found, which is how the terms it
/// could not evaluate are applied to what it returned (FR-21): the same predicates, over those
/// rows and no others.
fn matching(
    account: &OpenAccount,
    query: &Query,
    limit: u32,
    only: Option<&[LocalId]>,
) -> Result<Vec<(LocalId, Features)>, String> {
    let literal = |id: &LocalId| {
        let hex: String = id.to_bytes().iter().map(|b| format!("{b:02X}")).collect();
        format!("X'{hex}'")
    };
    // The messages the overlay has an opinion about (D-51). Their identities are ours, not
    // the user's text, so they are written into the statement as literals rather than bound
    // one parameter each, which no parameter limit then caps.
    let pending: BTreeSet<LocalId> = account.queue.entries().iter().map(|q| q.message).collect();
    let pending_literals = pending.iter().map(literal).collect::<Vec<_>>().join(", ");
    let cut = i64::from(limit).saturating_add(i64::try_from(pending.len()).unwrap_or(i64::MAX));

    let scopes: Vec<(TextScope<'_>, bool)> = query
        .terms
        .iter()
        .filter_map(|t| {
            t.text_scope()
                .map(|scope| (scope, matches!(t, Term::Phrase(_))))
        })
        .filter(|(scope, _)| !scope.is_empty())
        .collect();

    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    // Our identities, not the user's text, so literals rather than parameters — as above.
    if let Some(only) = only {
        if only.is_empty() {
            return Ok(Vec::new());
        }
        clauses.push(format!(
            "m.id IN ({})",
            only.iter().map(literal).collect::<Vec<_>>().join(", ")
        ));
    }
    for term in &query.terms {
        match term {
            Term::Before(millis) => {
                clauses.push("m.received_at_millis < ?".to_owned());
                values.push(millis_value(*millis));
            }
            Term::After(millis) => {
                clauses.push("m.received_at_millis > ?".to_owned());
                values.push(millis_value(*millis));
            }
            Term::HasAttachment(want) => {
                clauses.push("m.has_attachments = ?".to_owned());
                values.push(i64::from(*want).into());
            }
            // FR-5: a folder is matched on its semantic use *or* its display name. The
            // semantic one means the same thing in every locale.
            Term::Location(name) => {
                clauses.push(
                    "EXISTS (SELECT 1 FROM message_location l JOIN folder f ON f.id = l.folder_id
                             WHERE l.message_id = m.id
                               AND (lower(f.special_use) = lower(?)
                                    OR lower(f.display_name) = lower(?)))"
                        .to_owned(),
                );
                values.push(name.clone().into());
                values.push(name.clone().into());
            }
            // Read state is the overlay's as much as the server's. The server's answer is
            // asked here, and a message the overlay names is let through whatever the server
            // says, so the row the user sees decides it — see `seen_as_asked`.
            Term::Unread(want) => {
                clauses.push(format!(
                    "(((m.flags & {read}) = 0) = ? OR m.id IN ({pending_literals}))",
                    read = sift_store::flags::READ
                ));
                values.push(i64::from(*want).into());
            }
            Term::Word(_)
            | Term::Phrase(_)
            | Term::Sender(_)
            | Term::Recipient(_)
            | Term::Subject(_)
            | Term::Unknown(_) => {}
        }
    }

    // Which field each term matched in, for D-79 — asked of the index inside the statement,
    // and only where the order depends on it: a query with no relevance signal is ordered by
    // received time alone. Credit in a field is "any text term matched there", as D-79 reads
    // it; the preview and the body are both the text of the message.
    let relevance = query.carries_a_relevance_signal() && !scopes.is_empty();
    let mut credit_values: Vec<rusqlite::types::Value> = Vec::new();
    let mut credit = |wanted: &[Column]| -> String {
        if !relevance {
            return "0".to_owned();
        }
        let tests: Vec<String> = scopes
            .iter()
            .filter_map(|(scope, _)| {
                let columns: Vec<Column> = wanted
                    .iter()
                    .copied()
                    .filter(|c| scope.columns.contains(c))
                    .collect();
                if columns.is_empty() {
                    return None;
                }
                credit_values.push(scope.expression(&columns).into());
                Some(
                    "t.rowid IN (SELECT rowid FROM message_text WHERE message_text MATCH ?)"
                        .to_owned(),
                )
            })
            .collect();
        if tests.is_empty() {
            "0".to_owned()
        } else {
            format!("({})", tests.join(" OR "))
        }
    };
    let subject = credit(&[Column::Subject]);
    let sender = credit(&[Column::Sender]);
    let body = credit(&[Column::Snippet, Column::Body]);
    // A phrase every returned message matched is a phrase match for each of them.
    let phrase = relevance && scopes.iter().any(|(_, phrase)| *phrase);

    let filter = |sql: &mut String, first: &str| {
        for (n, clause) in clauses.iter().enumerate() {
            sql.push_str(if n == 0 { first } else { " AND " });
            sql.push_str(clause);
        }
    };
    // D-79's order within this account, and D-55's where the query carries no relevance
    // signal. The weights are `Features::score`'s field weights, doubled to stay integers;
    // everything else in that score is the same for every row this statement returns.
    let sql = if scopes.is_empty() {
        let mut sql = "SELECT m.id, m.received_at_millis, 0, 0, 0 FROM message m".to_owned();
        filter(&mut sql, " WHERE ");
        sql.push_str(" ORDER BY m.received_at_millis DESC, m.id DESC LIMIT ?");
        sql
    } else {
        let expression = scopes
            .iter()
            .map(|(scope, _)| format!("({})", scope.expression(scope.columns)))
            .collect::<Vec<_>>()
            .join(" AND ");
        values.insert(0, expression.into());
        let mut sql = format!(
            "SELECT m.id, m.received_at_millis, {subject} AS s, {sender} AS f, {body} AS b
             FROM message_text t
             JOIN message_text_key k ON k.docid = t.rowid
             JOIN message m ON m.id = k.message_id
             WHERE message_text MATCH ?"
        );
        filter(&mut sql, " AND ");
        sql.push_str(
            " ORDER BY 4 * s + 2 * f + b DESC, m.received_at_millis DESC, m.id DESC LIMIT ?",
        );
        sql
    };
    // Positional parameters bind in the order they are written: the credit tests in the
    // projection, then the match and the predicates, then the cut.
    credit_values.append(&mut values);
    credit_values.push(cut.into());

    let terms = u32::try_from(scopes.len()).unwrap_or(u32::MAX);
    let store = &account.store.store;
    let mut stmt = store.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(credit_values), |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for row in rows {
        let (key, received, subject, sender, body) = row.map_err(|e| e.to_string())?;
        let bytes: [u8; 16] = key.try_into().map_err(|_| "identity is not 16 bytes")?;
        found.push((
            LocalId::from_bytes(bytes),
            Features {
                matched_subject: subject,
                matched_sender: sender,
                matched_body: body,
                matched_phrase: phrase,
                // Conjunctive: every text term is present in every message returned.
                terms_present: terms,
                terms_total: terms,
                received_millis: u64::try_from(received).unwrap_or(0),
            },
        ));
    }
    Ok(found)
}

fn millis_value(millis: u64) -> rusqlite::types::Value {
    i64::try_from(millis).unwrap_or(i64::MAX).into()
}

/// Whether the row the user sees satisfies the operators the overlay has an opinion about.
///
/// `is:unread` is decided here rather than in the statement, because the base row is the
/// server's and the overlay is the user's: a message they marked read a moment ago is not
/// unread, whatever the server has yet to hear.
fn seen_as_asked(query: &Query, row: &MessageRow) -> bool {
    query.terms.iter().all(|term| match term {
        Term::Unread(want) => row.unread == *want,
        _ => true,
    })
}

/// What this build cannot answer about a particular query.
///
/// Keyed on the terms actually used rather than listed unconditionally: a caveat printed under
/// every search is one nobody reads, and the one that matters is the one about the operator the
/// user just typed.
///
/// `coverage` is what the server half reached for every account in scope; each caveat it
/// makes untrue is dropped, and none is dropped on the strength of a server that was not asked.
fn caveats(query: &Query, coverage: Coverage) -> Vec<String> {
    let mut out = Vec::new();
    if !coverage.bodies
        && query.terms.iter().any(|t| {
            t.text_scope()
                .is_some_and(|s| s.columns.contains(&Column::Body))
        })
    {
        out.push(
            "Message bodies are searched only for messages you have opened. Sift indexes a \
             body when it is first fetched, so mail nobody has opened was matched on its \
             sender, recipients, subject and preview."
                .to_owned(),
        );
    }
    if !coverage.attachments
        && query
            .terms
            .iter()
            .any(|t| matches!(t, Term::HasAttachment(_)))
    {
        out.push(
            "`has:attachment` matched nothing. Sync records the envelope, which does not say \
             whether a message carries attachments — the structure is fetched only when a \
             message is opened, which is what keeps a forty-megabyte part free until asked for."
                .to_owned(),
        );
    }
    out
}

/// How a term was read, in words a person can check against what they typed.
fn describe(term: &Term) -> String {
    match term {
        Term::Word(t) => format!("anywhere: {t}"),
        Term::Phrase(t) => format!("the exact phrase: {t}"),
        Term::Sender(t) => format!("from: {t}"),
        Term::Recipient(t) => format!("to: {t}"),
        Term::Subject(t) => format!("subject: {t}"),
        Term::HasAttachment(true) => "with an attachment".to_owned(),
        Term::HasAttachment(false) => "without an attachment".to_owned(),
        Term::Unread(true) => "unread".to_owned(),
        Term::Unread(false) => "already read".to_owned(),
        Term::Location(t) => format!("in: {t}"),
        Term::Before(t) => format!("before: {t}"),
        Term::After(t) => format!("after: {t}"),
        // The one worth showing plainly: it looks like an operator and was read as text.
        Term::Unknown(t) => format!("anywhere: {t} — read as text, not as an operator"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mail<'a> {
        sender: &'a str,
        recipients: &'a str,
        subject: &'a str,
        received: u64,
    }

    impl Default for Mail<'_> {
        fn default() -> Self {
            Self {
                sender: "someone@example.test",
                recipients: "me@example.test",
                subject: "",
                received: 1,
            }
        }
    }

    /// A message written the way ingest writes one: a row, which brings its index entry.
    fn put(app: &mut App, account: &str, mail: &Mail<'_>) -> LocalId {
        let account = app.account(account).unwrap();
        let id = account.ids.next();
        account
            .store
            .store
            .execute(
                "INSERT INTO message (id, fallback_digest, digest_rule_version,
                                      received_at_millis, sender, recipients, subject,
                                      provenance)
                 VALUES (?1, X'00', 1, ?2, ?3, ?4, ?5, 'Delivered')",
                rusqlite::params![
                    id.to_bytes().to_vec(),
                    i64::try_from(mail.received).unwrap(),
                    mail.sender,
                    mail.recipients,
                    mail.subject
                ],
            )
            .unwrap();
        account
            .store
            .store
            .execute(
                "INSERT INTO message_location (message_id, folder_id) VALUES (?1, 1)",
                [id.to_bytes().to_vec()],
            )
            .unwrap();
        id
    }

    fn found(app: &mut App, query: &str, limit: u32) -> Vec<LocalId> {
        app.search(query, None, limit)
            .unwrap()
            .hits
            .iter()
            .map(|h| h.row.id)
            .collect()
    }

    fn one_account() -> App {
        let mut app = App::new();
        app.add_account("work", "rich").unwrap();
        app
    }

    #[test]
    fn a_match_older_than_the_limit_is_still_found() {
        // The defect this module used to have: the newest page was read first and filtered
        // second, so a sender whose last message was older than it did not exist.
        let mut app = one_account();
        let old = put(
            &mut app,
            "work",
            &Mail {
                sender: "Rare Sender <rare@example.test>",
                subject: "Once upon a time",
                received: 1,
                ..Mail::default()
            },
        );
        for n in 0..250 {
            put(
                &mut app,
                "work",
                &Mail {
                    subject: "Newsletter",
                    received: 1_000 + n,
                    ..Mail::default()
                },
            );
        }
        assert_eq!(found(&mut app, "from:rare", 200), vec![old]);
        assert_eq!(found(&mut app, "upon", 200), vec![old]);
        assert_eq!(
            found(&mut app, "newsletter", 200).len(),
            200,
            "the limit was not applied"
        );
    }

    #[test]
    fn a_phrase_only_in_a_fetched_body_is_found_and_goes_with_its_message() {
        let mut app = one_account();
        let id = put(&mut app, "work", &Mail::default());
        assert!(found(&mut app, "\"tapas place\"", 10).is_empty());

        // First fetch writes the body into the entry the row already has.
        let account = app.account("work").unwrap();
        crate::document::index_body(account, id, "Meet at\nthe tapas place on Fifth", &[]).unwrap();
        assert_eq!(found(&mut app, "\"tapas place\"", 10), vec![id]);
        let caveats = app.search("tapas", None, 10).unwrap().caveats;
        assert!(
            caveats[0].contains("only for messages you have opened"),
            "{caveats:?}"
        );

        // NFR-14 evicts the body for space and leaves its indexed text: the message stays
        // findable and meets FR-12's "not cached" state rather than vanishing from search.
        app.account("work")
            .unwrap()
            .store
            .store
            .execute("UPDATE message SET body_blob = NULL", [])
            .unwrap();
        assert_eq!(found(&mut app, "\"tapas place\"", 10), vec![id]);

        // NFR-52 evicts the message itself, and its index entry goes with it as one unit.
        app.account("work")
            .unwrap()
            .store
            .store
            .execute(
                "DELETE FROM message WHERE id = ?1",
                [id.to_bytes().to_vec()],
            )
            .unwrap();
        assert!(found(&mut app, "\"tapas place\"", 10).is_empty());
    }

    #[test]
    fn an_unsegmentable_body_is_found_by_a_substring() {
        let mut app = one_account();
        let id = put(&mut app, "work", &Mail::default());
        let account = app.account("work").unwrap();
        crate::document::index_body(account, id, "明日の東京都の天気予報です", &[]).unwrap();
        assert_eq!(found(&mut app, "天気予報", 10), vec![id]);
        assert_eq!(found(&mut app, "東京", 10), vec![id]);
        assert!(found(&mut app, "大阪", 10).is_empty());
    }

    #[test]
    fn composed_and_decomposed_forms_find_each_other() {
        let mut app = one_account();
        let composed = put(
            &mut app,
            "work",
            &Mail {
                subject: "R\u{e9}sum\u{e9} attached",
                ..Mail::default()
            },
        );
        let decomposed = put(
            &mut app,
            "work",
            &Mail {
                subject: "Cafe\u{301} tonight",
                received: 2,
                ..Mail::default()
            },
        );
        assert_eq!(found(&mut app, "re\u{301}sume\u{301}", 10), vec![composed]);
        assert_eq!(found(&mut app, "caf\u{e9}", 10), vec![decomposed]);
        // And diacritics fold, as D-81 requires.
        assert_eq!(found(&mut app, "resume", 10), vec![composed]);
    }

    #[test]
    fn to_searches_the_recipients_and_nothing_else() {
        let mut app = one_account();
        let to_bob = put(
            &mut app,
            "work",
            &Mail {
                recipients: "Bob <bob@example.test>, carol@example.test",
                ..Mail::default()
            },
        );
        put(
            &mut app,
            "work",
            &Mail {
                sender: "bob@example.test",
                subject: "bob",
                received: 2,
                ..Mail::default()
            },
        );
        assert_eq!(found(&mut app, "to:bob", 10), vec![to_bob]);
        assert_eq!(found(&mut app, "to:carol@example", 10), vec![to_bob]);
        let report = app.search("to:bob", None, 10).unwrap();
        assert!(report.caveats.is_empty(), "{:?}", report.caveats);
        assert_eq!(report.interpretation, vec!["to: bob".to_owned()]);
    }

    #[test]
    fn operators_are_predicates_on_the_indexed_rows() {
        let mut app = one_account();
        let early = put(
            &mut app,
            "work",
            &Mail {
                subject: "Invoice",
                received: 1_000,
                ..Mail::default()
            },
        );
        let late = put(
            &mut app,
            "work",
            &Mail {
                subject: "Invoice",
                received: 5_000,
                ..Mail::default()
            },
        );
        assert_eq!(found(&mut app, "invoice before:1970-01-01", 10), Vec::new());
        assert_eq!(found(&mut app, "invoice", 10), vec![late, early]);
        assert_eq!(
            found(&mut app, "invoice after:1970-01-01", 10),
            vec![late, early]
        );
        assert_eq!(found(&mut app, "invoice in:inbox", 10), vec![late, early]);
        assert!(found(&mut app, "invoice in:archive", 10).is_empty());
        // No text at all: every message, in D-55's order.
        assert_eq!(found(&mut app, "is:unread", 10), vec![late, early]);
        assert!(found(&mut app, "is:read", 10).is_empty());
    }

    #[test]
    fn accounts_merge_under_d79_and_the_limit_comes_after_the_merge() {
        let mut app = one_account();
        app.add_account("home", "rich").unwrap();
        // The newer message matches only in its sender; the older one in its subject. D-79
        // puts the subject match first whichever account it is in, and a limit of one keeps it.
        put(
            &mut app,
            "work",
            &Mail {
                sender: "invoices@example.test",
                received: 9_000,
                ..Mail::default()
            },
        );
        let subject = put(
            &mut app,
            "home",
            &Mail {
                subject: "Your invoice",
                received: 1,
                ..Mail::default()
            },
        );
        assert_eq!(found(&mut app, "invoice", 1), vec![subject]);
        let hits = app.search("invoice", None, 10).unwrap().hits;
        assert_eq!(hits.len(), 2);
        assert!(hits[0].features.matched_subject && hits[1].features.matched_sender);
        // One account, by name, sees only its own.
        let home = app.search("invoice", Some("home"), 10).unwrap().hits;
        assert_eq!(
            home.iter().map(|h| h.row.id).collect::<Vec<_>>(),
            vec![subject]
        );
    }

    #[test]
    fn the_field_credit_decides_which_rows_survive_an_accounts_cut() {
        // D-79 within one account: the statement cuts at `limit`, so its ORDER BY — not the
        // merge — decides which rows are read at all. The subject match is the oldest; recency
        // alone would cut it at a limit of one.
        let mut app = one_account();
        let subject = put(
            &mut app,
            "work",
            &Mail {
                subject: "Your invoice",
                received: 1,
                ..Mail::default()
            },
        );
        let sender = put(
            &mut app,
            "work",
            &Mail {
                sender: "invoices@example.test",
                received: 5_000,
                ..Mail::default()
            },
        );
        let body = put(
            &mut app,
            "work",
            &Mail {
                received: 9_000,
                ..Mail::default()
            },
        );
        let account = app.account("work").unwrap();
        crate::document::index_body(account, body, "The invoice is attached", &[]).unwrap();

        assert_eq!(found(&mut app, "invoice", 1), vec![subject]);
        assert_eq!(found(&mut app, "invoice", 2), vec![subject, sender]);
        assert_eq!(found(&mut app, "invoice", 3), vec![subject, sender, body]);
    }

    #[test]
    fn typing_finds_a_word_before_it_is_finished() {
        // FR-19: results as the user types, so the last word is usually half a word.
        let mut app = one_account();
        let id = put(
            &mut app,
            "work",
            &Mail {
                subject: "Quarterly report",
                ..Mail::default()
            },
        );
        for typed in ["q", "quar", "quarterly rep", "from:some"] {
            assert_eq!(found(&mut app, typed, 10), vec![id], "{typed}");
        }
        // Punctuation alone asks nothing, and does not empty the list.
        assert_eq!(found(&mut app, "-", 10), vec![id]);
        // Nothing a person types is read as the index's own syntax.
        for hostile in [
            "\"",
            "a\"b",
            "NEAR(",
            "x OR",
            "*",
            "{subject}:x",
            "-quarterly",
        ] {
            let _ = app.search(hostile, None, 10).unwrap();
        }
    }

    #[test]
    fn a_body_is_indexed_when_a_message_is_first_opened() {
        // The whole first-fetch path, over the recorded corpus: sync, open, and the body the
        // reader was shown is in the index.
        let mut app = App::new();
        app.add_replayed_account("mail").unwrap();
        app.sync("mail", 10).unwrap();
        let account = app.account("mail").unwrap();
        let rows = crate::rows::message_rows(account, 50).unwrap();
        let mut indexed = 0;
        for row in rows {
            if app.open_document(row.id, false).is_err() {
                continue;
            }
            let account = app.account("mail").unwrap();
            let body: String = account
                .store
                .store
                .query_row(
                    "SELECT coalesce(body, '') FROM message_text
                     WHERE rowid = (SELECT docid FROM message_text_key WHERE message_id = ?1)",
                    [row.id.to_bytes().to_vec()],
                    |r| r.get(0),
                )
                .unwrap();
            if body.trim().is_empty() {
                continue;
            }
            indexed += 1;
            // Nothing the sanitizer removes is searchable: markup is not text.
            assert!(!body.contains('<'), "markup was indexed: {body}");
        }
        assert!(indexed > 0, "no opened message had its body indexed");
    }

    #[test]
    fn an_attachment_is_found_by_its_filename_once_its_message_is_opened() {
        // D-81: filenames arrive with the structure, which is fetched when a message is opened,
        // so before that the name is nowhere in the index and after it the message is found by
        // it. The replayed corpus's ordinary messages carry `statement.pdf`.
        let mut app = App::new();
        app.add_replayed_account("mail").unwrap();
        app.sync("mail", 10).unwrap();
        let rows = crate::rows::message_rows(app.account("mail").unwrap(), 50).unwrap();
        let before = found(&mut app, "statement.pdf", 50);
        let opened = rows
            .iter()
            .map(|r| r.id)
            .find(|id| !before.contains(id) && app.open_document(*id, false).is_ok())
            .expect("no message outside the results opened");

        let attachments: String = app
            .account("mail")
            .unwrap()
            .store
            .store
            .query_row(
                "SELECT coalesce(attachments, '') FROM message_text
                 WHERE rowid = (SELECT docid FROM message_text_key WHERE message_id = ?1)",
                [opened.to_bytes().to_vec()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            attachments.contains("statement.pdf"),
            "the filename was not indexed: {attachments:?}"
        );
        assert!(found(&mut app, "statement.pdf", 50).contains(&opened));
        assert!(found(&mut app, "statem", 50).contains(&opened));
    }

    fn mark_read(app: &mut App, account: &str, id: LocalId) {
        app.account(account)
            .unwrap()
            .store
            .store
            .execute(
                "UPDATE message SET flags = ?1 WHERE id = ?2",
                rusqlite::params![sift_store::flags::READ, id.to_bytes().to_vec()],
            )
            .unwrap();
    }

    #[test]
    fn the_overlay_decides_what_fills_the_limit() {
        // The statement cuts each account at `limit` plus the messages the overlay names. A
        // cut at `limit` alone would spend a place on the archived row and return one short;
        // an unread check on the server's flags alone would miss the message the user has
        // just marked unread.
        let mut app = one_account();
        let ids: Vec<LocalId> = (1..=5)
            .map(|n| {
                let id = put(
                    &mut app,
                    "work",
                    &Mail {
                        subject: "Report",
                        received: n,
                        ..Mail::default()
                    },
                );
                mark_read(&mut app, "work", id);
                id
            })
            .collect();
        let queue = &mut app.accounts.get_mut("work").unwrap().queue;
        queue.enqueue(1, ids[0], sift_mutations::intent::Intent::MarkUnread, 0);
        queue.enqueue(2, ids[4], sift_mutations::intent::Intent::Archive, 0);

        assert_eq!(found(&mut app, "is:unread", 1), vec![ids[0]]);
        assert_eq!(found(&mut app, "is:read", 2), vec![ids[3], ids[2]]);
        assert_eq!(found(&mut app, "report is:read", 2), vec![ids[3], ids[2]]);
        assert_eq!(
            found(&mut app, "report", 5),
            vec![ids[3], ids[2], ids[1], ids[0]]
        );
    }

    #[test]
    fn a_keystroke_reads_no_more_rows_than_it_can_show() {
        // NFR-5: neither a bare operator nor a one-letter prefix over a large mailbox takes
        // every matching message out of the store.
        let mut app = one_account();
        for n in 0..300 {
            put(
                &mut app,
                "work",
                &Mail {
                    subject: "Report",
                    received: n,
                    ..Mail::default()
                },
            );
        }
        let account = app.account("work").unwrap();
        for query in ["is:unread", "r", "report", "\"report\"", "from:someone"] {
            let rows = matching(account, &Query::parse(query), 20, None).unwrap();
            assert_eq!(rows.len(), 20, "{query} read {} rows", rows.len());
        }
    }

    // -----------------------------------------------------------------------
    // FR-21 — the server half, over D-65's recorded corpus.
    // -----------------------------------------------------------------------

    /// A replayed account after its first sync: envelopes for what the folders list, no
    /// bodies, and an archived message (`m5`) whose body holds a word nothing local does.
    fn synced() -> App {
        let mut app = App::new();
        app.add_replayed_account("mail").unwrap();
        app.sync("mail", 10).unwrap();
        app
    }

    fn held(app: &mut App) -> i64 {
        app.account("mail")
            .unwrap()
            .store
            .store
            .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap()
    }

    fn remote_of(app: &mut App, id: LocalId) -> String {
        app.account("mail")
            .unwrap()
            .store
            .store
            .query_row(
                "SELECT remote_id FROM message WHERE id = ?1",
                [id.to_bytes().to_vec()],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_word_only_in_an_unopened_body_is_found_by_the_server_and_opens() {
        // The issue's acceptance, end to end: never fetched, found by the server, labelled as
        // such, opened, and then held by the local index after that first fetch.
        let mut app = synced();

        let local = app.search("quokka", None, 20).unwrap();
        assert!(
            local.hits.is_empty(),
            "the local index knew a body it never saw"
        );
        assert_eq!(local.delegable_accounts, 1, "the shell was not told to ask");
        assert!(
            local.caveats[0].contains("only for messages you have opened"),
            "{:?}",
            local.caveats
        );

        let before = held(&mut app);
        let report = app.search_with_server("quokka", None, 20).unwrap();
        assert_eq!(report.delegable_accounts, 0);
        let found: Vec<(String, Source)> = report
            .hits
            .iter()
            .map(|h| (remote_of(&mut app, h.row.id), h.source))
            .collect();
        assert!(
            found.contains(&("m5".to_owned(), Source::Server)),
            "the archived message was not a server result: {found:?}"
        );
        assert!(found.iter().all(|(_, source)| *source == Source::Server));
        // One row per message: what was already held was not inserted again, and what was not
        // was inserted once.
        let now = held(&mut app);
        assert!(now > before && now <= before + 2, "{before} -> {now}");
        let again = app.search_with_server("quokka", None, 20).unwrap();
        assert_eq!(
            held(&mut app),
            now,
            "a second search inserted a second copy"
        );
        assert_eq!(again.hits.len(), report.hits.len());
        // Every account's server searched the text, so the body caveat is no longer true.
        assert!(
            report
                .caveats
                .iter()
                .all(|c| !c.contains("only for messages you have opened")),
            "{:?}",
            report.caveats
        );

        // It opens — and its body is then in the local index, by D-81's first-fetch rule.
        let m5 = report
            .hits
            .iter()
            .map(|h| h.row.id)
            .find(|id| remote_of(&mut app, *id) == "m5")
            .unwrap();
        app.open_document(m5, false).unwrap();
        let after = app.search("quokka", None, 20).unwrap();
        assert!(
            after
                .hits
                .iter()
                .any(|h| h.row.id == m5 && h.source == Source::Local),
            "the opened body did not reach the local index"
        );
    }

    #[test]
    fn offline_the_server_half_is_skipped_and_said_to_be() {
        // FR-21 under the policy tier: no attempt of any kind with no path (NFR-38), and a
        // report that names the skipped half rather than a silently smaller set.
        let mut app = synced();
        app.set_network_tier(sift_net::tier::Tier::OfflineNoPath);
        let before = held(&mut app);

        let local = app.search("quokka", None, 20).unwrap();
        assert_eq!(
            local.delegable_accounts, 0,
            "the shell was sent to ask offline"
        );
        assert!(
            local
                .caveats
                .iter()
                .any(|c| c.contains("`mail` was not searched on its server")
                    && c.contains("no network connection")),
            "{:?}",
            local.caveats
        );

        let report = app.search_with_server("quokka", None, 20).unwrap();
        assert!(report.hits.is_empty());
        assert_eq!(held(&mut app), before, "something was fetched with no path");
        assert!(
            report
                .caveats
                .iter()
                .any(|c| c.contains("no network connection")),
            "{:?}",
            report.caveats
        );
        // And the body caveat stands, because no server reached the bodies.
        assert!(report.caveats[0].contains("only for messages you have opened"));
    }

    #[test]
    fn a_failing_provider_costs_its_own_half_and_nothing_else() {
        // The recorded corpus has no answer for this query, which the replay harness reports as
        // a settled failure. The local half is still the answer, and the report says why it is
        // the only one.
        let mut app = synced();
        let local = app.search("receipt", None, 20).unwrap();
        assert!(!local.hits.is_empty());
        let report = app.search_with_server("receipt", None, 20).unwrap();
        assert_eq!(
            report.hits.iter().map(|h| h.row.id).collect::<Vec<_>>(),
            local.hits.iter().map(|h| h.row.id).collect::<Vec<_>>()
        );
        assert!(report.hits.iter().all(|h| h.source == Source::Local));
        assert!(
            report
                .caveats
                .iter()
                .any(|c| c.contains("server search did not complete")),
            "{:?}",
            report.caveats
        );
    }

    #[test]
    fn an_operator_the_server_cannot_evaluate_is_applied_to_what_it_returned() {
        // This account's capability does not cover a location, so the server is asked for the
        // word alone and `in:inbox` is applied locally: the archived message is in no folder
        // and does not survive it. The term is never dropped, and the report says where it ran.
        let mut app = synced();
        let report = app.search_with_server("quokka in:inbox", None, 20).unwrap();
        let remotes: Vec<String> = report
            .hits
            .iter()
            .map(|h| remote_of(&mut app, h.row.id))
            .collect();
        assert!(!remotes.contains(&"m5".to_owned()), "{remotes:?}");
        assert!(
            report
                .caveats
                .iter()
                .any(|c| c.contains("cannot evaluate in: inbox")),
            "{:?}",
            report.caveats
        );
    }

    #[test]
    fn an_account_with_no_provider_is_never_asked_and_never_blamed() {
        // A capability shape declares every operator and has nothing behind it. It is not
        // delegable, and its silence is covered by the local caveats rather than a failure.
        let mut app = one_account();
        put(
            &mut app,
            "work",
            &Mail {
                subject: "Invoice",
                ..Mail::default()
            },
        );
        let local = app.search("invoice", None, 10).unwrap();
        assert_eq!(local.delegable_accounts, 0);
        let report = app.search_with_server("invoice", None, 10).unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].source, Source::Local);
        assert!(
            report.caveats.iter().all(|c| !c.contains("server")),
            "{:?}",
            report.caveats
        );
    }

    #[test]
    fn a_list_query_is_answered_locally_and_the_server_is_not_asked() {
        // Flags, folders and dates are a list, and the list is local: a server asked for them
        // could only repeat it.
        let mut app = synced();
        let local = app.search("is:unread in:inbox", None, 20).unwrap();
        assert_eq!(local.delegable_accounts, 0);
    }

    #[test]
    fn every_term_but_an_unknown_operator_is_offered_to_a_provider() {
        for input in [
            "w",
            "\"p q\"",
            "from:s",
            "to:r",
            "subject:t",
            "has:attachment",
            "is:unread",
            "in:inbox",
            "before:2026-01-01",
            "after:2026-01-01",
        ] {
            let q = Query::parse(input);
            assert!(delegated(&q.terms[0]).is_some(), "{input}");
        }
        assert!(delegated(&Query::parse("ticket:12345").terms[0]).is_none());
    }
}

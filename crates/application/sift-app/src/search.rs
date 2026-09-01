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
//! # What is searched, and what is not, stated rather than implied
//!
//! This runs against the store's own columns — sender, subject, snippet, flags, attachment
//! presence, folder membership, received time. **Message bodies are not searched yet**: the
//! full-text index exists and the sync path does not populate it, so a body-text match would
//! return nothing and look like a mailbox with no such message in it. [`Report::covers_bodies`]
//! says so, and the shell says so, because a search that quietly covers less than the user
//! assumes is worse than one that admits its scope.
//!
//! # FR-21's source labels
//!
//! Results are labelled by where they came from. Nothing delegates to a provider yet, so every
//! result is [`Source::Local`] — and the label exists now rather than later because merging two
//! sources without saying which is which is the shape of the mistake FR-21 exists to prevent.

use sift_foundation::identity::LocalId;
use sift_index::query::{Query, Term};

use crate::rows::MessageRow;
use crate::{App, OpenAccount};

/// Where a result came from — FR-21.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Sift's own store.
    Local,
    /// The provider answered. **Not reachable yet**, and the variant exists because a merge
    /// that does not distinguish the two is the mistake the requirement is about.
    Server,
}

/// One result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub row: MessageRow,
    pub source: Source,
}

/// What a search found, and what it understood.
#[derive(Debug, Clone)]
pub struct Report {
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
    /// How many accounts could have been asked to search server-side, and were not.
    pub delegable_accounts: usize,
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
        let interpretation = query.terms.iter().map(describe).collect();

        let names: Vec<String> = match account {
            Some(name) => vec![name.to_owned()],
            None => self
                .account_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };

        let mut hits = Vec::new();
        for name in &names {
            let account = self.account(name)?;
            for row in crate::rows::message_rows(account, limit)? {
                if matches(&query, &row, account) {
                    hits.push(Hit {
                        row,
                        source: Source::Local,
                    });
                }
            }
        }
        // D-55's order, which is also D-79's fallback: a query carrying no relevance signal
        // is ordered by received time rather than by a score, because ranking a pure-metadata
        // filter would be inventing an order rather than reporting one.
        hits.sort_by(|a, b| {
            b.row
                .received_millis
                .cmp(&a.row.received_millis)
                .then_with(|| b.row.id.cmp(&a.row.id))
        });
        hits.truncate(limit as usize);

        Ok(Report {
            hits,
            interpretation,
            caveats: caveats(&query),
            delegable_accounts: 0,
        })
    }
}

/// What this build cannot answer about a particular query.
///
/// Keyed on the terms actually used rather than listed unconditionally: a caveat printed under
/// every search is one nobody reads, and the one that matters is the one about the operator the
/// user just typed.
fn caveats(query: &Query) -> Vec<String> {
    let mut out = Vec::new();
    if query
        .terms
        .iter()
        .any(|t| matches!(t, Term::Word(_) | Term::Phrase(_) | Term::Unknown(_)))
    {
        out.push(
            "Message bodies are not searched. The full-text index is not populated by the sync \
             path yet, so this looked at senders, subjects and snippets only."
                .to_owned(),
        );
    }
    if query.terms.iter().any(|t| matches!(t, Term::Recipient(_))) {
        out.push(
            "`to:` matched nothing. Recipients are not stored yet, and a filter that passed \
             everything instead would return a mailbox and call it a result set."
                .to_owned(),
        );
    }
    if query
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

/// Whether one row satisfies every term. Terms are conjunctive, which is what a person means
/// by typing two of them.
fn matches(query: &Query, row: &MessageRow, account: &OpenAccount) -> bool {
    query.terms.iter().all(|term| match term {
        // Free text and an unknown operator are the same thing here: matched across the
        // fields this build actually has, rather than dropped.
        Term::Word(text) | Term::Unknown(text) => {
            contains(&row.sender, text)
                || contains(&row.subject, text)
                || contains(&row.snippet, text)
        }
        // Adjacency matters, which is what makes a phrase different from its words — and
        // over a snippet rather than a body, which is the coverage this reports honestly.
        Term::Phrase(text) => contains(&row.subject, text) || contains(&row.snippet, text),
        Term::Sender(text) => contains(&row.sender, text),
        // Recipients are not stored on the row yet. **Never matched rather than always
        // matched**: a filter that silently passes everything is one that returns a mailbox
        // and calls it a result set.
        Term::Recipient(_) => false,
        Term::Subject(text) => contains(&row.subject, text),
        Term::HasAttachment(want) => row.has_attachments == *want,
        Term::Unread(want) => row.unread == *want,
        Term::Location(name) => in_folder(account, row.id, name),
        Term::Before(millis) => row.received_millis < *millis,
        Term::After(millis) => row.received_millis > *millis,
    })
}

/// Case-insensitive, over the normalized form both sides already carry.
fn contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// FR-5: a folder is matched on its semantic use *or* its display name. The semantic one
/// first, because that is the one that means the same thing in every locale.
fn in_folder(account: &OpenAccount, message: LocalId, name: &str) -> bool {
    account
        .store
        .store
        .query_row(
            "SELECT 1 FROM message_location l JOIN folder f ON f.id = l.folder_id
             WHERE l.message_id = ?1
               AND (lower(f.special_use) = lower(?2) OR lower(f.display_name) = lower(?2))",
            rusqlite::params![message.to_bytes().to_vec(), name],
            |_| Ok(true),
        )
        .unwrap_or(false)
}

/// How a term was read, in words a person can check against what they typed.
fn describe(term: &Term) -> String {
    match term {
        Term::Word(t) => format!("anywhere: {t}"),
        Term::Phrase(t) => format!("the exact phrase: {t}"),
        Term::Sender(t) => format!("from: {t}"),
        Term::Recipient(t) => format!("to: {t} — not searched; recipients are not stored yet"),
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

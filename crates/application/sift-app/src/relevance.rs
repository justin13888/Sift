//! The relevance corpus — recording judgements, and scoring D-79's ranking against them.
//!
//! # What this is the instrument for
//!
//! D-5 concedes weaker ranking and gates its own reconsideration on **measured ranking
//! failure**; D-79 trades peak within-account quality for cross-account coherence. Neither
//! condition can fire without real queries against a known mailbox, each recorded with the
//! message the person issuing it was looking for — `docs/product/relevance-corpus.md` is the
//! protocol. This module is the two halves of the instrument: turning one such judgement into a
//! durable record, and ranking every recorded query the way D-79 would and reporting where the
//! sought message landed.
//!
//! **It measures; it never generates.** A synthetic query has no correct answer that was not
//! generated beside it, so nothing here invents a query or a target.
//!
//! # Why a judgement is keyed on the provider's identifiers, not on D-78's
//!
//! A local identity is minted when a message is ingested. Removing and re-adding the account,
//! or a resynchronization, mints new ones, and a corpus keyed on them would silently stop
//! resolving — every judgement reading as "not returned", which is indistinguishable from a
//! ranking failure. So a record carries the provider's own identifier and the internet message
//! identifier beside it, and resolution tries them in that order. A message carrying neither is
//! refused at record time rather than written as a judgement that cannot be read back.
//!
//! # The file
//!
//! Plain text, one judgement per line, fields separated by a tab: account, provider identifier,
//! internet message identifier (`-` where absent), then the query exactly as typed. Lines that
//! begin with `#` are comments; the first is the format marker. The query is last so that it
//! needs no quoting. It is the user's own mail, so the file is created readable by its owner
//! only and lives outside the source tree.

use std::io::Write as _;
use std::path::Path;

use sift_foundation::identity::LocalId;
use sift_index::merge::{self, Features, Result_, Source};
use sift_index::query::{Query, Term};

use crate::App;
use crate::rows::MessageRow;

/// The first line of every corpus file.
pub const FORMAT: &str = "# sift relevance corpus v1";

/// The field written where an identifier is absent.
const ABSENT: &str = "-";

/// One recorded judgement: this query was issued, and this was the message sought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judgement {
    /// The account's display label in the container the judgement was recorded against.
    pub account: String,
    pub remote_id: Option<String>,
    pub internet_message_id: Option<String>,
    /// Exactly as typed, operators included.
    pub query: String,
}

impl Judgement {
    /// The judgement as one line of the corpus file, without its terminator.
    ///
    /// # Errors
    /// A field holds a tab or a line break, which the format cannot carry, or the account
    /// label begins with the comment marker.
    pub fn line(&self) -> Result<String, String> {
        let fields = [
            self.account.as_str(),
            self.remote_id.as_deref().unwrap_or(ABSENT),
            self.internet_message_id.as_deref().unwrap_or(ABSENT),
            self.query.as_str(),
        ];
        if let Some(bad) = fields
            .iter()
            .find(|f| f.contains(['\t', '\n', '\r']) || f.is_empty())
        {
            return Err(format!(
                "`{bad}` cannot be recorded: a field may not be empty or hold a tab or line break"
            ));
        }
        if self.account.starts_with('#') {
            return Err("an account label beginning with `#` would read back as a comment".into());
        }
        Ok(fields.join("\t"))
    }

    /// Append this judgement to a corpus file, creating it with the format marker if absent.
    ///
    /// # Errors
    /// The line cannot be formed, or the file cannot be written.
    pub fn append_to(&self, path: &Path) -> Result<(), String> {
        let line = self.line()?;
        let fresh = !path.exists();
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        // The queries are about the user's own mail. NFR-22's reasoning applies to a file on
        // their own disk as much as to anything sent anywhere: nobody else on the machine
        // needs to read what they search for.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut text = String::new();
        if fresh {
            text.push_str(FORMAT);
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        file.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Parse a corpus file's contents.
///
/// # Errors
/// The format marker is missing, or a line does not have four fields. The line number is named,
/// because a corpus is edited by hand and a refusal that does not say where is one nobody fixes.
pub fn parse(text: &str) -> Result<Vec<Judgement>, String> {
    let mut lines = text.lines().enumerate();
    match lines.next() {
        Some((_, first)) if first.trim_end() == FORMAT => {}
        _ => return Err(format!("the first line is not `{FORMAT}`")),
    }
    let mut out = Vec::new();
    for (index, line) in lines {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.splitn(4, '\t').collect();
        let [account, remote, internet, query] = fields[..] else {
            return Err(format!(
                "line {}: expected account, provider identifier, internet message identifier \
                 and query, separated by tabs",
                index + 1
            ));
        };
        let present = |f: &str| (f != ABSENT).then(|| f.to_owned());
        out.push(Judgement {
            account: account.to_owned(),
            remote_id: present(remote),
            internet_message_id: present(internet),
            query: query.to_owned(),
        });
    }
    Ok(out)
}

/// Where the sought message landed for one judgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub judgement: Judgement,
    /// Whether the sought message is still in the store. An unresolved judgement is **not** a
    /// ranking failure and is reported apart: the message was deleted, or the account is not in
    /// this container, and neither says anything about ranking.
    pub resolved: bool,
    /// One-based position under D-79's merge, or `None` where the query did not return it.
    pub ranked: Option<usize>,
    /// The same under D-55's list order, which is the order a query carrying no relevance
    /// signal falls back to — the baseline D-79's ranking has to beat to be worth its cost.
    pub listed: Option<usize>,
    /// How many messages the query returned.
    pub returned: usize,
}

/// A whole run's figures.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub judgements: usize,
    pub unresolved: usize,
    pub ranked: Figures,
    pub listed: Figures,
}

/// Figures for one ordering, over the resolved judgements only.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Figures {
    /// Mean reciprocal rank, a miss counting zero.
    pub mean_reciprocal_rank: f64,
    pub first: usize,
    pub top_three: usize,
    pub top_ten: usize,
    /// The query did not return the sought message at all. A recall failure rather than a
    /// ranking one, and the same under every ordering.
    pub missed: usize,
}

impl Figures {
    fn over<'a>(positions: impl Iterator<Item = Option<usize>> + 'a) -> Self {
        let mut figures = Self::default();
        let mut sum = 0.0;
        let mut count = 0u32;
        for position in positions {
            count += 1;
            match position {
                None => figures.missed += 1,
                Some(p) => {
                    #[allow(clippy::cast_precision_loss)] // A rank is far below 2^52.
                    let reciprocal = 1.0 / p as f64;
                    sum += reciprocal;
                    figures.first += usize::from(p == 1);
                    figures.top_three += usize::from(p <= 3);
                    figures.top_ten += usize::from(p <= 10);
                }
            }
        }
        if count > 0 {
            figures.mean_reciprocal_rank = sum / f64::from(count);
        }
        figures
    }
}

/// Summarize a run.
#[must_use]
pub fn summarize(outcomes: &[Outcome]) -> Summary {
    let resolved = || outcomes.iter().filter(|o| o.resolved);
    Summary {
        judgements: outcomes.len(),
        unresolved: outcomes.iter().filter(|o| !o.resolved).count(),
        ranked: Figures::over(resolved().map(|o| o.ranked)),
        listed: Figures::over(resolved().map(|o| o.listed)),
    }
}

/// D-79's corpus-independent features of one row against one query.
///
/// The snippet stands in for the body: the sync path does not populate the full-text index, so
/// a body match is a snippet match, and the report says so rather than implying more.
#[must_use]
pub fn features(query: &Query, row: &MessageRow) -> Features {
    let mut f = Features {
        received_millis: row.received_millis,
        ..Features::default()
    };
    for term in &query.terms {
        let (text, sender, subject, body, phrase) = match term {
            Term::Word(t) | Term::Unknown(t) => (t, true, true, true, false),
            Term::Phrase(t) => (t, false, true, true, true),
            Term::Subject(t) => (t, false, true, false, false),
            Term::Sender(t) => (t, true, false, false, false),
            _ => continue,
        };
        f.terms_total += 1;
        let in_sender = sender && contains(&row.sender, text);
        let in_subject = subject && contains(&row.subject, text);
        let in_body = body && contains(&row.snippet, text);
        f.matched_sender |= in_sender;
        f.matched_subject |= in_subject;
        f.matched_body |= in_body;
        let found = in_sender || in_subject || in_body;
        f.matched_phrase |= phrase && found;
        f.terms_present += u32::from(found);
    }
    f
}

fn contains(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Every message a query returns, however many — ranking is measured over the whole set,
/// not over whatever a result list happens to show.
const EVERYTHING: u32 = u32::MAX;

impl App {
    /// The judgement recording `query` as having sought `message`.
    ///
    /// # Errors
    /// No open account holds the message, or it carries no identifier that outlives this
    /// installation.
    pub fn relevance_judgement(&self, query: &str, message: LocalId) -> Result<Judgement, String> {
        let account = self
            .owner_of_stored(message)
            .ok_or_else(|| format!("no open account holds {message}"))?;
        let (remote_id, internet_message_id) = self.accounts[&account]
            .store
            .store
            .query_row(
                "SELECT remote_id, internet_message_id FROM message WHERE id = ?1",
                rusqlite::params![message.to_bytes().to_vec()],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?;
        if remote_id.is_none() && internet_message_id.is_none() {
            return Err(format!(
                "{message} carries neither a provider identifier nor an internet message \
                 identifier, so a judgement about it would not survive a resynchronization"
            ));
        }
        Ok(Judgement {
            account,
            remote_id,
            internet_message_id,
            query: query.split_whitespace().collect::<Vec<_>>().join(" "),
        })
    }

    /// Rank every judgement's query the way D-79 would, and the way D-55 would.
    ///
    /// # Errors
    /// The store refused. A judgement naming an absent account or message is an unresolved
    /// outcome rather than an error, so one stale line does not void a whole run.
    pub fn relevance_evaluate(&mut self, corpus: &[Judgement]) -> Result<Vec<Outcome>, String> {
        let mut out = Vec::with_capacity(corpus.len());
        for judgement in corpus {
            let target = self.resolve_judgement(judgement);
            let report = self.search(&judgement.query, None, EVERYTHING)?;
            let query = Query::parse(&judgement.query);

            // `search` returns D-55's order; D-79's is computed over the same set.
            let listed = target.and_then(|t| report.hits.iter().position(|h| h.row.id == t));
            let candidates = report
                .hits
                .iter()
                .map(|h| Result_ {
                    message: u128::from_be_bytes(h.row.id.to_bytes()),
                    source: Source::Local,
                    features: features(&query, &h.row),
                    local_score: None,
                })
                .collect();
            let merged = merge::merge(&query, candidates);
            let ranked = target.and_then(|t| {
                let key = u128::from_be_bytes(t.to_bytes());
                merged.iter().position(|r| r.message == key)
            });

            out.push(Outcome {
                judgement: judgement.clone(),
                resolved: target.is_some(),
                ranked: ranked.map(|p| p + 1),
                listed: listed.map(|p| p + 1),
                returned: report.hits.len(),
            });
        }
        Ok(out)
    }

    /// The local identity a judgement names in this container, if it still exists.
    ///
    /// The provider identifier first, because it is unique within an account by construction;
    /// the internet message identifier only after it, because R-5 records that it is not.
    fn resolve_judgement(&self, judgement: &Judgement) -> Option<LocalId> {
        let account = self.accounts.get(&judgement.account)?;
        let lookup = |column: &str, value: &str| -> Option<LocalId> {
            let key: Vec<u8> = account
                .store
                .store
                .query_row(
                    &format!(
                        "SELECT id FROM message WHERE {column} = ?1
                         ORDER BY received_at_millis DESC, id DESC LIMIT 1"
                    ),
                    [value],
                    |r| r.get(0),
                )
                .ok()?;
            Some(LocalId::from_bytes(key.try_into().ok()?))
        };
        judgement
            .remote_id
            .as_deref()
            .and_then(|v| lookup("remote_id", v))
            .or_else(|| {
                judgement
                    .internet_message_id
                    .as_deref()
                    .and_then(|v| lookup("internet_message_id", v))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judgement(query: &str) -> Judgement {
        Judgement {
            account: "work".into(),
            remote_id: Some("r-1".into()),
            internet_message_id: None,
            query: query.into(),
        }
    }

    #[test]
    fn a_judgement_survives_the_file() {
        let j = judgement("from:alice \"quarterly report\"");
        let text = format!("{FORMAT}\n# a comment\n\n{}\n", j.line().unwrap());
        assert_eq!(parse(&text).unwrap(), vec![j]);
    }

    #[test]
    fn a_file_without_the_marker_is_refused() {
        assert!(parse("work\tr-1\t-\tinvoice\n").is_err());
    }

    #[test]
    fn a_short_line_names_where_it_is() {
        let err = parse(&format!("{FORMAT}\nwork\tr-1\n")).unwrap_err();
        assert!(err.starts_with("line 2"), "{err}");
    }

    #[test]
    fn a_field_the_format_cannot_carry_is_refused_at_record_time() {
        assert!(judgement("a\tb").line().is_err());
        let mut j = judgement("invoice");
        j.account = "#work".into();
        assert!(j.line().is_err());
    }

    #[test]
    fn appending_creates_the_file_with_its_marker_once() {
        let dir = std::env::temp_dir().join(format!("sift-relevance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corpus.tsv");
        let _ = std::fs::remove_file(&path);
        judgement("one").append_to(&path).unwrap();
        judgement("two").append_to(&path).unwrap();
        let read = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read.len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o077,
                0,
                "the corpus is readable by others: {mode:o}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn insert(app: &mut App, remote: &str, subject: &str, sender: &str, received: u64) -> LocalId {
        let account = app.account("work").unwrap();
        let id = account.ids.next();
        account
            .store
            .store
            .execute(
                "INSERT INTO message (id, remote_id, fallback_digest, digest_rule_version,
                                      received_at_millis, sender, recipients, subject,
                                      provenance)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5, '', ?6, 'Delivered')",
                rusqlite::params![
                    id.to_bytes().to_vec(),
                    remote,
                    vec![0u8; 32],
                    i64::try_from(received).unwrap(),
                    sender,
                    subject
                ],
            )
            .unwrap();
        id
    }

    #[test]
    fn the_sought_message_is_ranked_by_d79_and_by_list_order_over_the_same_set() {
        let mut app = App::new();
        app.add_account("work", "rich").unwrap();
        // The subject match is older, so list order puts it second and D-79 puts it first.
        let sought = insert(
            &mut app,
            "r-old",
            "Invoice for March",
            "billing@example.test",
            100,
        );
        insert(
            &mut app,
            "r-new",
            "Re: lunch",
            "invoice-bot@example.test",
            200,
        );
        insert(
            &mut app,
            "r-other",
            "Unrelated",
            "someone@example.test",
            300,
        );

        let j = app.relevance_judgement("invoice", sought).unwrap();
        assert_eq!(j.remote_id.as_deref(), Some("r-old"));
        let lost = Judgement {
            remote_id: Some("gone".into()),
            ..j.clone()
        };

        let outcomes = app.relevance_evaluate(&[j, lost]).unwrap();
        assert_eq!(outcomes[0].returned, 2);
        assert_eq!(outcomes[0].ranked, Some(1));
        assert_eq!(outcomes[0].listed, Some(2));
        assert!(!outcomes[1].resolved);

        let summary = summarize(&outcomes);
        assert_eq!(summary.unresolved, 1);
        assert_eq!(summary.ranked.first, 1);
        assert!((summary.ranked.mean_reciprocal_rank - 1.0).abs() < f64::EPSILON);
        assert!((summary.listed.mean_reciprocal_rank - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_message_with_no_durable_identifier_is_not_recorded() {
        let mut app = App::new();
        app.add_account("work", "rich").unwrap();
        let account = app.account("work").unwrap();
        let id = account.ids.next();
        crate::insert_message(account, id, "Invoice", 1).unwrap();
        let err = app.relevance_judgement("invoice", id).unwrap_err();
        assert!(err.contains("would not survive"), "{err}");
    }
}

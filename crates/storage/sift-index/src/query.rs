//! FR-20 — structured search operators.
//!
//! The grammar must support **at minimum** sender, recipient, subject, attachment presence,
//! unread state, location, before and after dates, and quoted phrases. FR-21's server-side
//! fallback translates the same operators into what a provider will accept, which is why
//! D-20's phase assignment puts the two together: splitting them would mean specifying the
//! operators twice.

/// One term of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// Free text, matched across every indexed field.
    Word(String),
    /// A quoted phrase. Adjacency matters, which is what makes this different from the
    /// words it contains.
    Phrase(String),
    Sender(String),
    Recipient(String),
    Subject(String),
    HasAttachment(bool),
    Unread(bool),
    /// A folder, by its semantic kind or its display name.
    Location(String),
    /// Milliseconds since the epoch.
    Before(u64),
    After(u64),
    /// A term whose operator this build does not know.
    ///
    /// Kept as free text rather than dropped: `foo:bar` in a subject is something somebody
    /// might genuinely be searching for, and discarding it would silently return the wrong
    /// results rather than none.
    Unknown(String),
}

/// A parsed query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    /// Parse a query string.
    ///
    /// Never fails. A search box that rejected input would leave the user with no results
    /// and no way to tell why, and FR-19 requires results **as the user types** — most of
    /// which are therefore mid-word and syntactically incomplete by definition.
    #[must_use]
    pub fn parse(input: &str) -> Self {
        let mut terms = Vec::new();
        let mut rest = input.trim();

        while !rest.is_empty() {
            let (token, remainder) = take_token(rest);
            rest = remainder.trim_start();
            if token.is_empty() {
                continue;
            }
            terms.push(parse_term(&token));
        }
        Self { terms }
    }

    /// Whether the query says anything about relevance.
    ///
    /// D-79: where a query carries no relevance signal, order falls back to D-55's list
    /// order — because ranking a pure-metadata filter by a made-up score would be inventing
    /// an order rather than reporting one.
    #[must_use]
    pub fn carries_a_relevance_signal(&self) -> bool {
        self.terms.iter().any(|t| {
            matches!(
                t,
                Term::Word(_) | Term::Phrase(_) | Term::Subject(_) | Term::Unknown(_)
            )
        })
    }

    /// The terms a server-side search must be able to express — FR-21.
    ///
    /// What an account can delegate is a declared capability; this is what would be asked.
    #[must_use]
    pub fn delegable(&self) -> Vec<&Term> {
        self.terms
            .iter()
            .filter(|t| !matches!(t, Term::Unknown(_)))
            .collect()
    }
}

/// Take one token, keeping a quoted phrase together.
fn take_token(input: &str) -> (String, &str) {
    let mut chars = input.char_indices();
    let mut token = String::new();
    let mut in_quotes = false;

    for (i, c) in chars.by_ref() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                token.push(c);
            }
            c if c.is_whitespace() && !in_quotes => return (token, &input[i..]),
            c => token.push(c),
        }
    }
    (token, "")
}

fn parse_term(token: &str) -> Term {
    if let Some(phrase) = token.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        return Term::Phrase(phrase.to_owned());
    }
    let Some((operator, value)) = token.split_once(':') else {
        return Term::Word(token.to_ascii_lowercase());
    };
    let value = value.trim_matches('"');
    if value.is_empty() {
        // `from:` with nothing after it is a query being typed, not an error.
        return Term::Word(token.to_ascii_lowercase());
    }
    match operator.to_ascii_lowercase().as_str() {
        "from" | "sender" => Term::Sender(value.to_ascii_lowercase()),
        "to" | "recipient" => Term::Recipient(value.to_ascii_lowercase()),
        "subject" => Term::Subject(value.to_ascii_lowercase()),
        "has" if value.eq_ignore_ascii_case("attachment") => Term::HasAttachment(true),
        "is" if value.eq_ignore_ascii_case("unread") => Term::Unread(true),
        "is" if value.eq_ignore_ascii_case("read") => Term::Unread(false),
        "in" | "folder" => Term::Location(value.to_ascii_lowercase()),
        "before" => parse_date(value).map_or_else(|| Term::Unknown(token.to_owned()), Term::Before),
        "after" => parse_date(value).map_or_else(|| Term::Unknown(token.to_owned()), Term::After),
        _ => Term::Unknown(token.to_owned()),
    }
}

/// `YYYY-MM-DD`, as milliseconds since the epoch.
///
/// Deliberately one format. A search box that accepted several would have to guess between
/// them, and guessing wrong about a date silently returns the wrong messages.
fn parse_date(value: &str) -> Option<u64> {
    let mut parts = value.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil epoch — Howard Hinnant's algorithm, which is exact and needs no
    // calendar library.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operator_fr20_names_is_supported() {
        let q = Query::parse(
            "from:a@b.test to:c@d.test subject:report has:attachment is:unread \
             in:inbox before:2026-01-01 after:2025-01-01 \"exact phrase\" loose",
        );
        let has = |m: fn(&Term) -> bool| q.terms.iter().any(m);
        assert!(has(|t| matches!(t, Term::Sender(_))));
        assert!(has(|t| matches!(t, Term::Recipient(_))));
        assert!(has(|t| matches!(t, Term::Subject(_))));
        assert!(has(|t| matches!(t, Term::HasAttachment(true))));
        assert!(has(|t| matches!(t, Term::Unread(true))));
        assert!(has(|t| matches!(t, Term::Location(_))));
        assert!(has(|t| matches!(t, Term::Before(_))));
        assert!(has(|t| matches!(t, Term::After(_))));
        assert!(has(|t| matches!(t, Term::Phrase(_))));
        assert!(has(|t| matches!(t, Term::Word(_))));
    }

    #[test]
    fn a_quoted_phrase_survives_its_spaces() {
        let q = Query::parse("\"quarterly report\"");
        assert_eq!(q.terms, vec![Term::Phrase("quarterly report".into())]);
    }

    #[test]
    fn a_half_typed_query_parses_rather_than_failing() {
        // FR-19 requires results **as the user types**, so most queries this sees are
        // syntactically incomplete by definition. Rejecting them would leave the user with
        // no results and no way to tell why.
        for input in ["from:", "\"unterminated", "subject", ":", "  ", "from:a@"] {
            let _ = Query::parse(input);
        }
    }

    #[test]
    fn an_unknown_operator_stays_searchable_as_text() {
        // `foo:bar` in a subject is something somebody might genuinely be looking for, and
        // discarding it would return the wrong results rather than none.
        let q = Query::parse("ticket:12345");
        assert_eq!(q.terms, vec![Term::Unknown("ticket:12345".into())]);
        assert!(q.carries_a_relevance_signal());
    }

    #[test]
    fn a_pure_metadata_filter_carries_no_relevance_signal() {
        // D-79: ranking a metadata filter by a made-up score would be inventing an order
        // rather than reporting one, so it falls back to D-55's list order.
        assert!(!Query::parse("is:unread has:attachment in:inbox").carries_a_relevance_signal());
        assert!(Query::parse("is:unread invoice").carries_a_relevance_signal());
    }

    #[test]
    fn dates_convert_to_the_epoch_correctly() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2000-01-01"), Some(946_684_800_000));
        assert_eq!(parse_date("2026-01-01"), Some(1_767_225_600_000));
    }

    #[test]
    fn a_malformed_date_becomes_an_unknown_term_rather_than_a_wrong_one() {
        // Guessing wrong about a date silently returns the wrong messages, which is worse
        // than not understanding it.
        for input in ["before:notadate", "after:2026-13-01", "before:2026-01-99"] {
            let q = Query::parse(input);
            assert!(
                matches!(q.terms[0], Term::Unknown(_)),
                "{input} -> {:?}",
                q.terms[0]
            );
        }
    }

    #[test]
    fn everything_but_an_unknown_term_can_be_delegated() {
        let q = Query::parse("from:a@b.test ticket:1 invoice");
        assert_eq!(q.delegable().len(), 2);
    }
}

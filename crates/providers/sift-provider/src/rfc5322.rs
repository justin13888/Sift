//! The date form every adapter meets, in the one place all four can share it.
//!
//! An adapter reads the `Date` header because D-44's corroborating digest is over
//! "originator address, origination date, normalized subject and reference chain", and
//! because D-55 requires the header be *displayed* even though it is never ordered on.
//!
//! It lives here rather than in the MIME parser because the parse is needed at envelope
//! fetch, which is long before any body exists — and pulling the whole rendering pipeline
//! into the providers layer to read one header would be a dependency nobody could justify.
//!
//! **The header is the sender's, so it is arbitrary.** Nothing here fails: a date that does
//! not parse is `None`, and a `None` origination date is a display that omits it and a
//! digest computed without it. The alternative — rejecting the message — would let a sender
//! make a message unreadable by writing four bad characters.

/// The address out of an originator or recipient header.
///
/// `Display Name <someone@example.test>` yields the part inside the angle brackets, and a
/// bare address yields itself. **The display name is discarded here on purpose**: D-44's
/// digest corroborates on the address, and a display name is chosen freely by the sender —
/// two messages agreeing on it is not evidence of anything.
///
/// Returns `None` where nothing address-shaped is present, which is what a group syntax or
/// an empty header gives.
#[must_use]
pub fn address_of(header: &str) -> Option<String> {
    let cleaned = strip_comments(header);
    let candidate = match (cleaned.rfind('<'), cleaned.rfind('>')) {
        (Some(open), Some(close)) if close > open => cleaned[open + 1..close].trim().to_owned(),
        _ => cleaned.trim().trim_matches('"').trim().to_owned(),
    };
    // An address has exactly one `@` with something either side of it. Anything else is
    // either a group, an empty header, or a sender being creative, and none of those is an
    // address this may pretend to have found.
    let (local, domain) = candidate.split_once('@')?;
    (!local.is_empty() && !domain.is_empty() && !domain.contains('@'))
        .then(|| candidate.to_ascii_lowercase())
}

/// Every address in a header that may carry several.
#[must_use]
pub fn addresses_of(header: &str) -> Vec<String> {
    split_outside_quotes(&strip_comments(header))
        .iter()
        .filter_map(|part| address_of(part))
        .collect()
}

/// Split on commas that are not inside a quoted display name.
fn split_outside_quotes(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => out.push(core::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

/// Parse an internet-message date into milliseconds since the epoch.
///
/// Returns `None` for anything that does not parse, including a date outside what the
/// arithmetic here can represent.
#[must_use]
pub fn parse_date_millis(value: &str) -> Option<i64> {
    // An optional day-of-week, then the parts. Comments in parentheses are dropped, which
    // is where the obsolete forms put a timezone name.
    let cleaned = strip_comments(value);
    let rest = cleaned.split_once(',').map_or(cleaned.as_str(), |(_, r)| r);
    let mut fields = rest.split_whitespace();

    let day: u32 = fields.next()?.parse().ok()?;
    let month = month_number(fields.next()?)?;
    let year_text = fields.next()?;
    let mut year: i64 = year_text.parse().ok()?;
    // The obsolete two- and three-digit years, per the specification's own rule.
    if year_text.len() == 2 {
        year += if year < 50 { 2000 } else { 1900 };
    } else if year_text.len() == 3 {
        year += 1900;
    }

    let mut time = fields.next()?.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next().unwrap_or("0").parse().ok()?;
    if !((1..=31).contains(&day) && hour < 24 && minute < 60 && second <= 60) {
        return None;
    }

    let offset_minutes = fields.next().map_or(0, zone_offset_minutes);

    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?
        .checked_sub(offset_minutes * 60)?;
    seconds.checked_mul(1000)
}

/// Drop parenthesised comments, which may nest.
fn strip_comments(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth = 0usize;
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|m| lower.starts_with(m))
        .map(|i| i as u32 + 1)
}

/// The zone, in minutes east of UTC.
///
/// `-0000` means "the time is UTC but the sender declined to say where they were", and the
/// obsolete alphabetic zones are treated as UTC — which is what the specification requires,
/// because they were widely wrong in practice and are not to be trusted.
fn zone_offset_minutes(zone: &str) -> i64 {
    let bytes = zone.as_bytes();
    if bytes.len() >= 5 && (bytes[0] == b'+' || bytes[0] == b'-') {
        let hours: i64 = zone[1..3].parse().unwrap_or(0);
        let minutes: i64 = zone[3..5].parse().unwrap_or(0);
        let magnitude = hours * 60 + minutes;
        return if bytes[0] == b'-' {
            -magnitude
        } else {
            magnitude
        };
    }
    0
}

/// Days from 1970-01-01 to the given civil date. Howard Hinnant's algorithm, which is exact
/// for the whole proleptic Gregorian calendar and needs no table.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_the_epoch() {
        assert_eq!(parse_date_millis("Thu, 1 Jan 1970 00:00:00 +0000"), Some(0));
    }

    #[test]
    fn an_ordinary_header_parses() {
        // 2026-08-31T12:34:56Z
        assert_eq!(
            parse_date_millis("Mon, 31 Aug 2026 12:34:56 +0000"),
            Some(1_788_179_696_000)
        );
    }

    #[test]
    fn a_zone_is_applied_in_the_right_direction() {
        let utc = parse_date_millis("Mon, 31 Aug 2026 12:00:00 +0000").unwrap();
        // Noon in a zone five hours behind UTC is 17:00 UTC.
        assert_eq!(
            parse_date_millis("Mon, 31 Aug 2026 12:00:00 -0500"),
            Some(utc + 5 * 3_600_000)
        );
        assert_eq!(
            parse_date_millis("Mon, 31 Aug 2026 12:00:00 +0530"),
            Some(utc - (5 * 60 + 30) * 60_000)
        );
    }

    #[test]
    fn the_day_of_week_is_optional_and_never_believed() {
        // A wrong day name does not change the answer, because the date decides.
        let a = parse_date_millis("31 Aug 2026 12:00:00 +0000");
        let b = parse_date_millis("Fri, 31 Aug 2026 12:00:00 +0000");
        assert_eq!(a, b);
        assert!(a.is_some());
    }

    #[test]
    fn seconds_are_optional() {
        assert_eq!(
            parse_date_millis("Mon, 31 Aug 2026 12:34 +0000"),
            Some(1_788_179_640_000)
        );
    }

    #[test]
    fn an_obsolete_zone_name_is_treated_as_utc_rather_than_guessed() {
        // The specification's own instruction: these were widely wrong in practice.
        assert_eq!(
            parse_date_millis("Mon, 31 Aug 2026 12:00:00 GMT"),
            parse_date_millis("Mon, 31 Aug 2026 12:00:00 +0000")
        );
    }

    #[test]
    fn a_comment_is_dropped_rather_than_confusing_the_fields() {
        assert_eq!(
            parse_date_millis("Mon, 31 Aug 2026 12:00:00 -0700 (PDT)"),
            parse_date_millis("Mon, 31 Aug 2026 19:00:00 +0000")
        );
    }

    #[test]
    fn the_obsolete_two_digit_year_follows_the_specified_rule() {
        assert_eq!(
            parse_date_millis("1 Jan 70 00:00:00 +0000"),
            parse_date_millis("1 Jan 1970 00:00:00 +0000")
        );
        assert_eq!(
            parse_date_millis("1 Jan 49 00:00:00 +0000"),
            parse_date_millis("1 Jan 2049 00:00:00 +0000")
        );
    }

    #[test]
    fn a_date_before_the_epoch_is_negative_rather_than_wrapping() {
        let before = parse_date_millis("1 Jan 1969 00:00:00 +0000").unwrap();
        assert_eq!(before, -365 * 86_400_000);
    }

    #[test]
    fn nothing_a_sender_can_write_makes_this_fail() {
        // The header is arbitrary. Rejecting the message would let four bad characters
        // make it unreadable.
        for hostile in [
            "",
            ",",
            "not a date at all",
            "Mon, 99 Zzz 99999999999999999999 99:99:99 +9999",
            "Mon, 31 Aug 2026 12:00:00 (((((",
            "1 Jan 1970 00:00:00 +",
            "\u{202e}1 Jan 1970 00:00:00 +0000",
        ] {
            let _ = parse_date_millis(hostile);
        }
        assert_eq!(parse_date_millis("not a date at all"), None);
        assert_eq!(parse_date_millis(""), None);
    }

    #[test]
    fn an_address_is_taken_from_the_angle_brackets_and_the_display_name_discarded() {
        // Two messages agreeing on a display name is not evidence of anything.
        assert_eq!(
            address_of("Someone Real <someone@example.test>").as_deref(),
            Some("someone@example.test")
        );
        assert_eq!(
            address_of("someone@example.test").as_deref(),
            Some("someone@example.test")
        );
        assert_eq!(
            address_of("\"Ends, With A Comma\" <a@b.test>").as_deref(),
            Some("a@b.test")
        );
    }

    #[test]
    fn a_display_name_that_is_itself_an_address_does_not_win() {
        // The oldest display trick in mail: the name is an address, the real one is not.
        assert_eq!(
            address_of("security@bank.test <attacker@example.test>").as_deref(),
            Some("attacker@example.test")
        );
    }

    #[test]
    fn an_address_is_folded_to_one_case_so_a_digest_agrees_with_itself() {
        assert_eq!(
            address_of("Someone <SoMeOne@Example.TEST>").as_deref(),
            Some("someone@example.test")
        );
    }

    #[test]
    fn nothing_address_shaped_yields_nothing_rather_than_a_guess() {
        for hostile in [
            "",
            "undisclosed-recipients:;",
            "@",
            "a@",
            "@b",
            "a@b@c",
            "  ",
        ] {
            assert_eq!(address_of(hostile), None, "{hostile}");
        }
    }

    #[test]
    fn a_recipient_list_splits_on_commas_outside_the_quotes() {
        let got = addresses_of("\"Last, First\" <a@x.test>, b@y.test, Nobody <>");
        assert_eq!(got, vec!["a@x.test".to_owned(), "b@y.test".to_owned()]);
    }

    #[test]
    fn an_impossible_time_is_refused_rather_than_normalized() {
        assert_eq!(parse_date_millis("1 Jan 2026 25:00:00 +0000"), None);
        assert_eq!(parse_date_millis("32 Jan 2026 00:00:00 +0000"), None);
    }
}

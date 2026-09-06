//! NFR-54 and D-100 — normalizing attacker-controlled text, once, before it reaches
//! anything that renders or indexes it.
//!
//! # Why this is in the foundation layer
//!
//! NFR-54 says the presentation layer normalizes every attacker-controlled string it
//! emits. D-81 says the full-text index's normalization form **MUST be the same one**,
//! and adds that "the coupling must be asserted by test rather than assumed".
//!
//! Under D-59 the index is in the storage layer and may not reach the presentation layer.
//! Two implementations of one form is exactly the shape R-16 warns about in a different
//! context — one that produces a store the other cannot read — so the implementation is
//! here, below both, and [`agrees`] is the assertion D-81 asks for.
//!
//! # What "attacker-controlled" covers
//!
//! Sender and recipient display names, subjects, snippets, folder and tag names, and
//! attachment names. The threat model's own note is that this path reaches **native
//! chrome** — the list, the reader, notifications, the tray — and that **no sanitizer
//! invariant sees it**. I1 through I10 are about the body view; this is the other half of
//! the hostile input, and it has no backstop at all.
//!
//! # Three rules, in this order
//!
//! 1. **Other control characters are removed.** C0 and C1 have no legitimate place in a
//!    display value and several of them terminate or reflow native text rendering.
//! 2. **Bidirectional controls are isolated rather than stripped** (D-100). Stripping is
//!    the safest-*looking* answer and it is not the one chosen: a display name in Hebrew
//!    or Arabic legitimately contains them, and removing them mangles real mail. Isolation
//!    wraps the value so its directionality cannot escape into the chrome around it.
//!    D-100 records the cost honestly — this leaves attacker-chosen control characters
//!    inside the string.
//! 3. **Normalization precedes truncation, and truncation is at a grapheme boundary**
//!    (D-100). The other order can compose a character out of the bytes that survive the
//!    cut, and cutting mid-cluster can leave a combining mark orphaned onto whatever
//!    follows it.
//!
//! The bound is L-25 — the internet message format's own line bound, far above any
//! legitimate value and far below a denial of service against native chrome.

use crate::limits::L25_DISPLAY_CHARS;
use crate::state::ContentValue;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// First strong isolate. Opens an isolated run whose direction is taken from its first
/// strong character, which is what makes it correct for a value whose script Sift does
/// not know in advance.
const FSI: char = '\u{2068}';
/// Pop directional isolate.
const PDI: char = '\u{2069}';

/// The bidirectional control characters D-100 keeps.
///
/// Everything here is *format* rather than content, and every one of them is legitimate
/// in real mail from right-to-left scripts.
const fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'              // arabic letter mark
        | '\u{200E}'            // left-to-right mark
        | '\u{200F}'            // right-to-left mark
        | '\u{202A}'..='\u{202E}' // embeddings and overrides
        | '\u{2066}'..='\u{2069}' // isolates
    )
}

/// A control character with no place in a display value.
///
/// C0 and C1, minus nothing: a subject does not legitimately contain a newline, a tab or
/// a bell. The bidi controls above are *format* characters rather than controls and are
/// not caught by this.
fn is_stripped_control(c: char) -> bool {
    (c.is_control() && !is_bidi_control(c)) || matches!(c, '\u{200B}' | '\u{FEFF}')
}

/// The normalization form, applied once and shared.
///
/// NFC rather than NFD: it is the form the web and the mail ecosystem already produce, so
/// it is the one that makes the *fewest* strings differ from what a sender sent, and D-44's
/// fallback identity digest normalizes a subject that has already been through here.
fn normalized(s: &str) -> String {
    s.chars()
        .filter(|c| !is_stripped_control(*c))
        .nfc()
        .collect()
}

/// The form the full-text index stores, and the form D-44's digest is taken over.
///
/// Not truncated and not isolated: a body is not a display value, and an isolate in the
/// index would be a token nobody searches for.
#[must_use]
pub fn for_index(raw: &str) -> String {
    normalized(raw)
}

/// The form that crosses the shell boundary as a [`ContentValue`].
///
/// Normalized, bounded at L-25 on a grapheme boundary, and isolated. This is the only
/// function that should construct a `ContentValue`.
#[must_use]
pub fn for_display(raw: &str) -> ContentValue {
    let normalized = normalized(raw);
    let bounded = truncate_at_grapheme(&normalized, L25_DISPLAY_CHARS as usize);

    let mut out = String::with_capacity(bounded.len() + FSI.len_utf8() + PDI.len_utf8());
    out.push(FSI);
    out.push_str(bounded);
    out.push(PDI);
    ContentValue::assume_normalized(out)
}

/// Truncate to at most `max` grapheme clusters.
///
/// Clusters rather than `char`s, because a `char` boundary can fall between a base
/// character and its combining marks — which both changes what is displayed and leaves
/// the marks to compose onto whatever the renderer puts next.
#[must_use]
pub fn truncate_at_grapheme(s: &str, max: usize) -> &str {
    match s.grapheme_indices(true).nth(max) {
        Some((byte, _)) => &s[..byte],
        None => s,
    }
}

/// Number of grapheme clusters — what L-25 and L-16 count.
#[must_use]
pub fn grapheme_len(s: &str) -> usize {
    s.graphemes(true).count()
}

/// NFR-53 — the name an attachment is written under, derived from the one a sender chose.
///
/// # Why this is a different rule from [`for_display`]
///
/// [`for_display`] *isolates* bidirectional controls rather than stripping them, because a
/// display name is prose and a right-to-left run in it is real mail. A filename is not prose.
/// It is a structured identifier whose last component decides what the platform does when the
/// file is opened, and a right-to-left override inside one produces a name that renders as a
/// document and executes as a program. So the rule here is [`crate::normalize`]'s other one —
/// the one `display_form` applies to a URL: **remove them**, because there is no legitimate
/// use for reordering the visible components of a path.
///
/// # What is removed, and why each one
///
/// Path separators and the platform's alternate separator, because a name containing one is a
/// name proposing a directory. Traversal segments, because `..` is a name proposing a
/// *different* directory. A leading dot, because a file that does not appear in the chooser
/// the user just used is a file written somewhere the user did not watch. Colons, which are
/// the separator on the one platform whose path syntax nobody remembers. The reserved device
/// names, which are cheap to exclude and expensive to discover. And a trailing dot or space,
/// which some filesystems silently drop — turning a shown path into a different written one,
/// which is the exact property NFR-53 exists to hold.
///
/// # This derives a name; it does not resolve a path
///
/// The result is a single path *component*, and it is still not a path. Joining it to a
/// directory, refusing to overwrite what is already there, and showing the final result are
/// the caller's, because only the caller knows the directory the user chose.
#[must_use]
pub fn for_file_name(raw: &str) -> String {
    let normalized: String = normalized(raw)
        .chars()
        .filter(|c| !is_bidi_control(*c))
        .map(|c| if is_separator(c) { '_' } else { c })
        .collect();

    // Traversal is handled at the *ends* rather than as a substring anywhere. Stripping every
    // `..` would rewrite `report..final.pdf` into `report.final.pdf`, which is a silent change
    // to a name the user is about to be shown — and once separators are gone there is only one
    // segment left, so `..` can only be the whole of it.
    let candidate = normalized
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);

    let candidate = truncate_to_bytes_at_grapheme(candidate, L53_FILE_NAME_BYTES);
    if candidate.is_empty() || is_reserved_device_name(candidate) {
        return FALLBACK_FILE_NAME.to_owned();
    }
    candidate.to_owned()
}

/// The bound on a derived name, **in bytes**.
///
/// Not L-25, and not a grapheme count. L-25 is a *display* bound on prose, and it counts
/// graphemes because what it bounds is how much a person reads. A filesystem bounds a path
/// component in **bytes**, and a name of 200 emoji is 800 of them — so counting graphemes here
/// would produce a name that passes every check and then fails at the write, which is the one
/// place NFR-53's "the exact final path MUST be shown" cannot be honoured.
///
/// 200 leaves the caller room for a disambiguating suffix inside the 255-byte limit the common
/// filesystems share.
const L53_FILE_NAME_BYTES: usize = 200;

/// What a name that derived to nothing becomes. A message can carry an attachment named `..`
/// or named entirely of control characters, and the answer is a name rather than a refusal —
/// the bytes are still the user's.
const FALLBACK_FILE_NAME: &str = "attachment";

/// The longest prefix that fits `max` bytes without cutting a grapheme cluster.
///
/// Both halves matter. Cutting on a byte boundary can produce invalid UTF-8; cutting on a
/// `char` boundary can orphan a combining mark onto whatever the renderer puts next, which is
/// the same reason [`truncate_at_grapheme`] exists. This one differs only in what it counts.
fn truncate_to_bytes_at_grapheme(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let end = s
        .grapheme_indices(true)
        .map(|(i, g)| i + g.len())
        .take_while(|end| *end <= max)
        .last()
        .unwrap_or(0);
    &s[..end]
}

const fn is_separator(c: char) -> bool {
    matches!(c, '/' | '\\' | ':')
}

/// The device names Windows resolves before it looks at the filesystem, with or without an
/// extension. Sift does not ship there today, and excluding them costs one comparison against
/// a name that is about to be shown to a user anyway — whereas discovering the omission means
/// discovering it on somebody's machine.
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")),
                    Some(d) if d.len() == 1 && d.chars().all(|c| c.is_ascii_digit() && c != '0'))
}

/// D-81's coupling, as a function rather than as a comment: the display form and the
/// index form agree about normalization.
///
/// Exposed rather than private so that the index and the presentation layer can each
/// assert it against their own inputs, which is what "asserted by test rather than
/// assumed" means when the two live in different crates.
#[must_use]
pub fn agrees(raw: &str) -> bool {
    let indexed = for_index(raw);
    let displayed = for_display(raw);
    let inner = displayed
        .as_str()
        .strip_prefix(FSI)
        .and_then(|s| s.strip_suffix(PDI))
        .unwrap_or(displayed.as_str());
    indexed.starts_with(inner)
}

#[cfg(test)]
mod file_name_tests {
    use super::*;

    /// NFR-53's whole point. Each of these is a name a message may legitimately carry and
    /// none of them may become a path.
    #[test]
    fn a_sender_supplied_name_never_proposes_a_directory() {
        for hostile in [
            "../../.ssh/authorized_keys",
            "/etc/passwd",
            r"..\..\Windows\System32\cmd.exe",
            "..",
            "....",
            "/",
        ] {
            let derived = for_file_name(hostile);
            assert!(
                !derived.contains('/') && !derived.contains('\\') && !derived.contains(':'),
                "`{hostile}` derived to `{derived}`, which contains a separator"
            );
            assert!(
                derived != ".." && !derived.starts_with('.'),
                "`{hostile}` derived to `{derived}`, which is still a traversal or hidden"
            );
        }
    }

    /// The failure the requirement names in its own words: a right-to-left override inside a
    /// filename produces a name that renders as a document and executes as a program. The
    /// classic is `exe.txt` written so it displays as `txt.exe` reversed — the extension the
    /// user reads is not the extension the platform acts on.
    #[test]
    fn a_bidi_override_cannot_disguise_an_extension() {
        let disguised = "annual_report\u{202E}fdp.exe";
        let derived = for_file_name(disguised);
        assert!(
            !derived.chars().any(is_bidi_control),
            "`{derived}` still reorders itself when rendered"
        );
        assert!(
            derived.ends_with(".exe"),
            "the real extension survives: {derived}"
        );
    }

    /// A display name isolates bidi controls and a filename removes them. The two rules
    /// disagreeing is deliberate, and this is the assertion that keeps them from being
    /// quietly unified by somebody tidying up.
    #[test]
    fn the_display_rule_and_the_file_rule_deliberately_differ() {
        let arabic = "\u{202B}\u{645}\u{644}\u{641}.pdf";
        let displayed = for_display(arabic);
        assert!(
            displayed.as_str().chars().any(is_bidi_control),
            "a display value keeps them — D-100"
        );
        assert!(
            !for_file_name(arabic).chars().any(is_bidi_control),
            "a file name does not — NFR-53"
        );
    }

    /// Removing every `..` as a substring would silently rewrite this, and a shown path that
    /// differs from the written one is the property NFR-53 is about.
    #[test]
    fn a_dot_inside_a_name_is_not_traversal() {
        assert_eq!(for_file_name("report..final.pdf"), "report..final.pdf");
        assert_eq!(for_file_name("v1.2.3-notes.txt"), "v1.2.3-notes.txt");
    }

    /// A name is derived rather than refused: the bytes are still the user's mail, and an
    /// attachment they cannot save because its name was hostile is an attachment the sender
    /// took from them.
    #[test]
    fn a_name_that_derives_to_nothing_still_gets_one() {
        for empty in ["", "...", "   ", "\u{202E}\u{202E}", "\u{0}\u{7}"] {
            assert_eq!(for_file_name(empty), "attachment");
        }
    }

    /// A trailing dot or space is dropped by some filesystems *after* the path is shown,
    /// which turns the shown path and the written one into two different things.
    #[test]
    fn a_trailing_dot_or_space_is_removed_here_rather_than_by_the_filesystem() {
        assert_eq!(for_file_name("invoice.pdf "), "invoice.pdf");
        assert_eq!(for_file_name("invoice.pdf."), "invoice.pdf");
        assert_eq!(for_file_name("invoice.pdf . . "), "invoice.pdf");
    }

    #[test]
    fn the_reserved_device_names_are_excluded() {
        for reserved in ["CON", "nul.txt", "COM1", "LPT9.pdf", "aux"] {
            assert_eq!(
                for_file_name(reserved),
                "attachment",
                "`{reserved}` resolves before the filesystem is consulted"
            );
        }
        // Not reserved, and a real name somebody may well send.
        assert_eq!(for_file_name("COM10.log"), "COM10.log");
        assert_eq!(for_file_name("console.log"), "console.log");
    }

    /// Long enough to leave the caller room for a disambiguating suffix inside the 255-byte
    /// component limit, even where every grapheme costs four bytes.
    /// A grapheme count would pass this and then fail at the write, which is the one place
    /// "the exact final path is shown" cannot be honoured — the shown path was never written.
    #[test]
    fn a_name_is_bounded_in_the_unit_a_filesystem_bounds_it_in() {
        for wide in ["\u{1F600}", "\u{e0}", "e\u{301}", "a"] {
            let derived = for_file_name(&wide.repeat(500));
            assert!(
                derived.len() <= L53_FILE_NAME_BYTES,
                "`{wide}` repeated derived to {} bytes",
                derived.len()
            );
            assert!(derived.len() + " (99)".len() < 255);
            // Truncation did not cut a cluster in half.
            assert_eq!(derived, for_file_name(&derived));
        }
    }

    /// Control characters go before anything else looks at the string, so a name cannot
    /// smuggle a newline into whatever the final path is printed into.
    #[test]
    fn control_characters_are_gone() {
        assert_eq!(for_file_name("in\u{a}voice\u{0}.pdf"), "invoice.pdf");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner(v: &ContentValue) -> &str {
        v.as_str()
            .strip_prefix(FSI)
            .unwrap()
            .strip_suffix(PDI)
            .unwrap()
    }

    #[test]
    fn a_display_value_is_isolated_rather_than_stripped() {
        // D-100 chose isolation over stripping. The controls are still there; what changed
        // is that their effect cannot reach the chrome around the value.
        let hostile = "Report\u{202E}gnp.exe";
        let v = for_display(hostile);
        assert!(v.as_str().starts_with(FSI) && v.as_str().ends_with(PDI));
        assert!(
            inner(&v).contains('\u{202E}'),
            "the override was stripped; D-100 keeps it and isolates the value instead"
        );
    }

    #[test]
    fn legitimate_right_to_left_text_survives() {
        // The reason stripping was rejected: these characters are real mail, not an
        // attack, and removing them mangles a correct display name.
        let name = "\u{200F}שלום\u{200E} Ltd";
        let v = for_display(name);
        assert!(inner(&v).contains('\u{200F}'));
        assert!(inner(&v).contains('\u{200E}'));
    }

    #[test]
    fn other_control_characters_are_removed() {
        let v = for_display("Sub\u{0007}ject\nwith\ttabs\u{007F}");
        let i = inner(&v);
        assert!(!i.contains('\u{0007}'), "bell survived");
        assert!(!i.contains('\n'), "newline survived into native chrome");
        assert!(!i.contains('\t'));
        assert!(!i.contains('\u{007F}'));
        assert_eq!(i, "Subjectwithtabs");
    }

    #[test]
    fn zero_width_space_and_byte_order_mark_are_removed() {
        // Neither is a "control" by Unicode's classification, and both are used to make two
        // different strings render identically.
        let v = for_display("pay\u{200B}pal\u{FEFF}.com");
        assert_eq!(inner(&v), "paypal.com");
    }

    #[test]
    fn normalization_precedes_truncation() {
        // The other order can compose a character out of the bytes that survive the cut.
        // Decomposed "é" is two scalars; NFC makes it one, so a value that is exactly at
        // the bound after normalization must not be truncated.
        let decomposed = "e\u{0301}".repeat(L25_DISPLAY_CHARS as usize);
        let v = for_display(&decomposed);
        assert_eq!(
            grapheme_len(inner(&v)),
            L25_DISPLAY_CHARS as usize,
            "truncating first would have cut this in half"
        );
        assert!(
            inner(&v).contains('é'),
            "normalization did not run before truncation"
        );
    }

    #[test]
    fn truncation_is_at_a_grapheme_boundary() {
        // A cluster that is several scalars long must not be cut through the middle,
        // leaving combining marks to compose onto whatever the renderer puts next.
        let flag = "🇬🇧";
        let s = flag.repeat(2000);
        let cut = truncate_at_grapheme(&s, 10);
        assert_eq!(grapheme_len(cut), 10);
        assert_eq!(cut, flag.repeat(10), "a flag was cut in half");
    }

    #[test]
    fn a_display_value_is_bounded_by_l25() {
        let v = for_display(&"a".repeat(10_000));
        assert_eq!(grapheme_len(inner(&v)), L25_DISPLAY_CHARS as usize);
    }

    #[test]
    fn the_bound_is_on_the_senders_value_not_on_sifts_isolate() {
        // The isolate marks are Sift's own and are not part of what L-25 counts. Counting
        // them would silently shorten every value by two.
        let v = for_display(&"a".repeat(10_000));
        assert_eq!(grapheme_len(v.as_str()), L25_DISPLAY_CHARS as usize + 2);
    }

    #[test]
    fn the_index_form_carries_no_isolate() {
        // An isolate in the index is a token nobody searches for.
        let indexed = for_index("hello");
        assert!(!indexed.contains(FSI) && !indexed.contains(PDI));
    }

    #[test]
    fn the_index_form_is_not_truncated() {
        // L-25 bounds a display value. A body is not one, and bounding it here would
        // silently make long messages unsearchable past the first thousand characters.
        let long = "word ".repeat(10_000);
        assert!(for_index(&long).len() > L25_DISPLAY_CHARS as usize);
    }

    #[test]
    fn display_and_index_agree_about_normalization() {
        // D-81: "the coupling must be asserted by test rather than assumed."
        for raw in [
            "plain",
            "e\u{0301}clair",
            "\u{200F}שלום",
            "Report\u{202E}gnp.exe",
            "ＦＵＬＬＷＩＤＴＨ",
            "\u{1F1EC}\u{1F1E7} flag",
            "",
            &"x".repeat(5_000),
        ] {
            assert!(agrees(raw), "display and index disagree about {raw:?}");
        }
    }

    #[test]
    fn normalizing_twice_changes_nothing() {
        // The property that lets a value be normalized once, at the boundary, rather than
        // defensively at every use.
        for raw in ["e\u{0301}", "plain", "\u{200F}שלום\u{202E}x"] {
            let once = for_index(raw);
            assert_eq!(for_index(&once), once, "not idempotent for {raw:?}");
        }
    }

    #[test]
    fn an_empty_value_is_still_isolated() {
        let v = for_display("");
        assert_eq!(v.as_str(), format!("{FSI}{PDI}"));
    }
}

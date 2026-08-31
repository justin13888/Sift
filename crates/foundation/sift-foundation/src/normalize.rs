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

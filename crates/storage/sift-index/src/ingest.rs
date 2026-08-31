//! D-81 — what enters the index, when, and how it is tokenized.

use sift_foundation::normalize;

/// When a field is indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Envelope fields, at ingest. Every message has these.
    Ingest,
    /// Body text, at **first fetch**.
    ///
    /// The consequence is stated rather than hidden: local search covers every message's
    /// envelope and **only the bodies the user has read**, so local search quality varies
    /// with reading habits in a way users will not predict. FR-21's server-side fallback is
    /// how a query reaches the body text of mail nobody has opened.
    FirstFetch,
}

/// A token, and the script class that decides how it was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// True where the token came from the trigram index rather than word segmentation.
    pub trigram: bool,
}

/// Whether a character belongs to a script word segmentation cannot segment.
///
/// D-81 makes the trigram index **required rather than conditional**, and this is why:
/// Chinese, Japanese and Thai do not put spaces between words, so a word-segmenting
/// tokenizer produces one enormous token per run and the text becomes unsearchable. A
/// conditional index would mean those users discover the feature does not work for them.
fn needs_trigrams(c: char) -> bool {
    matches!(u32::from(c),
        0x2E80..=0x9FFF      // CJK radicals through unified ideographs
        | 0xAC00..=0xD7AF    // Hangul syllables
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0x0E00..=0x0E7F    // Thai
        | 0x1780..=0x17FF    // Khmer
        | 0x0E80..=0x0EFF    // Lao
    )
}

/// Tokenize text for the index.
///
/// Three properties D-81 fixes, each with a consequence:
///
/// - **Unicode word segmentation**, so `don't` is one token and `a,b` is two.
/// - **Diacritic folding**, so a search for `cafe` finds `café`. Real mail is written both
///   ways by the same person.
/// - **A fixed normalization form, the same one NFR-54 applies.** That coupling is asserted
///   by test rather than assumed, because two forms would mean the indexed text and the
///   displayed text were different strings that happened to look alike.
#[must_use]
pub fn tokenize(text: &str) -> Vec<Token> {
    let normalized = normalize::for_index(text);
    let mut tokens = Vec::new();
    let mut word = String::new();

    let flush = |word: &mut String, tokens: &mut Vec<Token>| {
        if !word.is_empty() {
            tokens.push(Token {
                text: fold(word),
                trigram: false,
            });
            word.clear();
        }
    };

    let mut unsegmentable = String::new();
    for c in normalized.chars() {
        if needs_trigrams(c) {
            flush(&mut word, &mut tokens);
            unsegmentable.push(c);
            continue;
        }
        if !unsegmentable.is_empty() {
            tokens.extend(trigrams(&unsegmentable));
            unsegmentable.clear();
        }
        if c.is_alphanumeric() || c == '\'' || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut tokens);
        }
    }
    flush(&mut word, &mut tokens);
    if !unsegmentable.is_empty() {
        tokens.extend(trigrams(&unsegmentable));
    }
    tokens
}

/// Overlapping three-character windows, scoped to the text that needs them.
///
/// D-81 scopes the trigram index to the scripts that need it rather than applying it
/// everywhere, because trigramming Latin text would multiply the index size for no gain —
/// and NFR-52's budget has to absorb whatever this produces.
fn trigrams(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 3 {
        return vec![Token {
            text: text.to_owned(),
            trigram: true,
        }];
    }
    chars
        .windows(3)
        .map(|w| Token {
            text: w.iter().collect(),
            trigram: true,
        })
        .collect()
}

/// Fold diacritics and case.
fn fold(word: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    word.nfd()
        .filter(|c| !matches!(u32::from(*c), 0x0300..=0x036F))
        .collect::<String>()
        .to_lowercase()
}

/// What is indexed for one message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    pub subject: Vec<Token>,
    pub sender: Vec<Token>,
    pub recipients: Vec<Token>,
    /// Extracted **after sanitization**, from the tree the pipeline already produced —
    /// "what is searchable is what was renderable". Indexing raw markup would make every
    /// message match a search for `div`.
    pub body: Vec<Token>,
    /// Attachment filenames are indexed. Folder and tag names are **not**: FR-20's
    /// structured operators address those, and indexing them as free text would mean a
    /// search for `inbox` matched every message in it.
    pub attachment_names: Vec<Token>,
}

impl Document {
    /// Index the envelope. Every message gets this.
    #[must_use]
    pub fn at_ingest(
        subject: &str,
        sender: &str,
        recipients: &str,
        attachments: &[String],
    ) -> Self {
        Self {
            subject: tokenize(subject),
            sender: tokenize(sender),
            recipients: tokenize(recipients),
            body: Vec::new(),
            attachment_names: attachments.iter().flat_map(|a| tokenize(a)).collect(),
        }
    }

    /// Add body text at first fetch.
    ///
    /// Re-run if the body is fetched again after eviction, which is the case that makes the
    /// content-storing choice pay for itself.
    pub fn add_body(&mut self, extracted_text: &str) {
        self.body = tokenize(extracted_text);
    }

    #[must_use]
    pub fn has_body(&self) -> bool {
        !self.body.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(tokens: &[Token]) -> Vec<String> {
        tokens.iter().map(|t| t.text.clone()).collect()
    }

    #[test]
    fn words_are_segmented_and_folded() {
        assert_eq!(texts(&tokenize("Hello, World!")), vec!["hello", "world"]);
        assert_eq!(texts(&tokenize("don't stop")), vec!["don't", "stop"]);
    }

    #[test]
    fn a_search_for_cafe_finds_cafe_with_an_accent() {
        // Real mail is written both ways by the same person.
        assert_eq!(texts(&tokenize("café")), texts(&tokenize("cafe")));
        assert_eq!(texts(&tokenize("naïve")), texts(&tokenize("naive")));
    }

    #[test]
    fn unsegmentable_scripts_get_trigrams() {
        // Required rather than conditional: a word-segmenting tokenizer produces one
        // enormous token per run for these, and the text becomes unsearchable.
        let tokens = tokenize("日本語のテキスト");
        assert!(tokens.iter().all(|t| t.trigram), "CJK was word-segmented");
        assert!(tokens.len() > 3, "the run was not broken up: {tokens:?}");
    }

    #[test]
    fn trigrams_are_scoped_to_the_text_that_needs_them() {
        // Trigramming Latin text would multiply the index size for no gain, and NFR-52's
        // budget has to absorb whatever this produces.
        let tokens = tokenize("hello 日本 world");
        assert!(
            tokens.iter().any(|t| !t.trigram),
            "Latin text was trigrammed"
        );
        assert!(tokens.iter().any(|t| t.trigram), "CJK text was not");
    }

    #[test]
    fn a_short_unsegmentable_run_is_one_token() {
        let tokens = tokenize("日本");
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].trigram);
    }

    #[test]
    fn the_index_form_is_the_one_nfr54_applies() {
        // D-81: "the coupling must be asserted by test rather than assumed". Two forms would
        // mean the indexed text and the displayed text were different strings that happened
        // to look alike.
        for raw in [
            "e\u{0301}clair",
            "\u{200F}שלום",
            "plain",
            "ＦＵＬＬＷＩＤＴＨ",
        ] {
            assert!(
                normalize::agrees(raw),
                "display and index disagree about {raw:?}"
            );
        }
    }

    #[test]
    fn envelope_fields_are_indexed_at_ingest_and_body_is_not() {
        let d = Document::at_ingest(
            "Quarterly report",
            "a@b.test",
            "c@d.test",
            &["chart.png".into()],
        );
        assert!(!d.subject.is_empty());
        assert!(!d.attachment_names.is_empty());
        assert!(
            !d.has_body(),
            "body was indexed before the message was read"
        );
    }

    #[test]
    fn body_text_arrives_at_first_fetch_and_can_arrive_again() {
        // Re-indexed if the body is fetched again after eviction — the case that makes the
        // content-storing choice pay for itself.
        let mut d = Document::at_ingest("s", "a", "b", &[]);
        d.add_body("the quarterly figures");
        assert!(d.has_body());
        d.add_body("the quarterly figures");
        assert!(d.has_body());
    }

    #[test]
    fn hostile_text_does_not_panic_the_tokenizer() {
        for text in [
            "",
            "\u{0000}",
            &"a".repeat(100_000),
            "\u{202E}",
            "🇬🇧🇬🇧",
            "\u{FFFD}",
        ] {
            let _ = tokenize(text);
        }
    }
}

//! The fidelity corpus, run against the sanitizer's invariants.
//!
//! The corpus is `fixtures/`: real-world-shaped messages in the six categories the reference
//! environment names, and every published mutation-XSS payload as a permanent regression
//! vector (NFR-40 method 5). It is checked in as data rather than written into a test's source
//! so that the same bytes can seed NFR-40's fuzzing (method 4) and its dual-parser divergence
//! run (method 3), and so that NFR-26's snapshots and NFR-47's contrast measurement are taken
//! over the set this file already holds to I1 through I10.
//!
//! What this file asserts, per entry:
//!
//! - **Messages** are accepted. The limits register's rule is that the corpus decides: a
//!   legitimate message a bound rejects means the bound is wrong, so a rejection here fails.
//!   Nothing forbidden survives into the re-parsed tree, the output is parse-stable (I8) and
//!   idempotent (I6), no visible text is invented, lost or reordered (I9), and the structure
//!   NFR-50 names — alternative text, table structure, heading levels, direction and
//!   language — survives sanitization.
//! - **Vectors** meet their recorded expectation. A `clean` vector leaves nothing forbidden in
//!   the tree **as the engine will build it** — judged by [`audit`], never by grepping the
//!   bytes — and is parse-stable. A `rejected` vector is refused by a bound.
//!
//! And of the corpus as a whole: every file has a provenance, every category is present, and
//! vector identifiers are assigned in order and never reused.

use sift_sanitize::audit::audit;
use sift_sanitize::document::{self, Document};
use sift_sanitize::sanitize::{Sanitized, check_parse_stability, sanitize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

// ---------------------------------------------------------------------------
// Reading the corpus
// ---------------------------------------------------------------------------

/// The six categories the reference environment's fidelity corpus names.
const CATEGORIES: &[&str] = &[
    "marketing",
    "transactional",
    "mailing-list",
    "cjk",
    "rtl",
    "plain-text",
];

struct Message {
    file: String,
    category: String,
    /// What reaches the sanitizer: the file itself, or for plain text the escaped `<pre>`
    /// the pipeline's stage 3 builds from it.
    source: String,
    /// The plain text as written, for a `text/plain` entry.
    plain: Option<String>,
}

fn manifest_rows(path: &Path) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('|').map(|f| f.trim().to_owned()).collect())
        .collect()
}

fn messages() -> Vec<Message> {
    let dir = fixtures().join("messages");
    manifest_rows(&dir.join("manifest.txt"))
        .into_iter()
        .map(|row| {
            let [file, category, _provenance] = row.as_slice() else {
                panic!("manifest row does not have three fields: {row:?}");
            };
            let raw = std::fs::read_to_string(dir.join(file))
                .unwrap_or_else(|e| panic!("manifest names {file}, which cannot be read: {e}"));
            let (source, plain) = if category == "plain-text" {
                (format!("<pre>{}</pre>", escape(&raw)), Some(raw))
            } else {
                (raw, None)
            };
            Message {
                file: file.clone(),
                category: category.clone(),
                source,
                plain,
            }
        })
        .collect()
}

/// The escape the pipeline applies to a `text/plain` part before stage 3 — a plain part is
/// escaped rather than parsed, so it cannot author markup.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Debug)]
struct Vector {
    id: String,
    expect: String,
    source: String,
    payload: String,
}

fn vectors() -> Vec<Vector> {
    let path = fixtures().join("mxss").join("vectors.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("@ ") else {
            assert!(
                line.trim().is_empty() || line.starts_with('#'),
                "a line outside any record: {line:?}"
            );
            continue;
        };
        let fields: Vec<&str> = header.split('|').map(str::trim).collect();
        let [id, expect, source] = fields.as_slice() else {
            panic!("vector header does not have three fields: {line:?}");
        };
        // The payload is the next line, byte for byte: a literal tab or a trailing space may
        // be the whole point of the vector.
        let payload = lines
            .next()
            .unwrap_or_else(|| panic!("{id} has a header and no payload"));
        out.push(Vector {
            id: (*id).to_owned(),
            expect: (*expect).to_owned(),
            source: (*source).to_owned(),
            payload: payload.to_owned(),
        });
    }
    out
}

fn clean(name: &str, html: &str) -> Sanitized {
    sanitize(html).unwrap_or_else(|e| {
        panic!(
            "{name} was rejected ({e:?}). The corpus decides: a legitimate message a bound \
             rejects means the bound moves, with a note naming this message"
        )
    })
}

/// Collapse every run of whitespace, so text is compared as a reader sees it rather than as
/// the serializer happened to lay it out.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// The corpus itself
// ---------------------------------------------------------------------------

#[test]
fn every_message_file_has_a_provenance_and_every_listed_file_exists() {
    let dir = fixtures().join("messages");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
        .expect("fixtures/messages exists")
        .map(|e| {
            e.expect("readable entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != "manifest.txt")
        .collect();

    let rows = manifest_rows(&dir.join("manifest.txt"));
    let mut listed = BTreeSet::new();
    for row in &rows {
        let [file, category, provenance] = row.as_slice() else {
            panic!("manifest row does not have three fields: {row:?}");
        };
        assert!(listed.insert(file.clone()), "{file} is listed twice");
        assert!(
            CATEGORIES.contains(&category.as_str()),
            "{file}: unknown category {category:?}"
        );
        assert!(
            matches!(provenance.as_str(), "constructed" | "captured"),
            "{file}: provenance must be `constructed` or `captured`, not {provenance:?}"
        );
    }

    assert_eq!(
        on_disk, listed,
        "the manifest and the directory disagree: nothing enters the corpus without a provenance"
    );
}

#[test]
fn every_category_the_reference_environment_names_is_present() {
    let present: BTreeSet<String> = messages().into_iter().map(|m| m.category).collect();
    for category in CATEGORIES {
        assert!(
            present.contains(*category),
            "the fidelity corpus has no {category} message"
        );
    }
}

#[test]
fn vector_identifiers_are_assigned_in_order_and_never_reused() {
    let all = vectors();
    assert!(!all.is_empty(), "no vectors were read");
    for (i, v) in all.iter().enumerate() {
        assert_eq!(
            v.id,
            format!("mx-{:03}", i + 1),
            "identifiers are stable and sequential; a vector is never removed or renumbered"
        );
        assert!(
            matches!(v.expect.as_str(), "clean" | "rejected"),
            "{}: expectation must be `clean` or `rejected`, not {:?}",
            v.id,
            v.expect
        );
        assert!(!v.source.is_empty(), "{} names no source", v.id);
        assert!(
            !v.payload.trim().is_empty(),
            "{} has an empty payload",
            v.id
        );
    }
}

// ---------------------------------------------------------------------------
// Messages: I1 through I10 hold, and nothing a reader relies on is lost
// ---------------------------------------------------------------------------

#[test]
fn every_message_is_accepted_and_nothing_forbidden_survives() {
    for m in messages() {
        let out = clean(&m.file, &m.source);
        let violations = audit(&out.html);
        assert!(
            violations.is_empty(),
            "{}: forbidden content survived into the tree: {violations:?}",
            m.file
        );
    }
}

#[test]
fn every_message_is_parse_stable_and_idempotent() {
    // I8 and I6. The corpus is where a parse differential that only real markup reaches —
    // table foster-parenting, conditional comments, a stray `</p>` — would show up.
    for m in messages() {
        // The two passes compared directly first, so a failure shows where they differ.
        let once = clean(&m.file, &m.source);
        let twice = clean(&m.file, &once.html);
        assert_eq!(once.html, twice.html, "{}: not idempotent", m.file);
        assert!(
            check_parse_stability(&m.source).expect("within bounds"),
            "{}: output reparses to a different tree",
            m.file
        );
    }
}

#[test]
fn no_visible_text_is_invented_lost_or_reordered() {
    // I9, and NFR-50's reading order. Compared as a reader sees it: the text of the re-parsed
    // tree, outside the elements whose text no reader sees.
    for m in messages() {
        let before = words(&document::read(&m.source).visible_text);
        let after = words(&document::read(&clean(&m.file, &m.source).html).visible_text);
        assert_eq!(before, after, "{}: visible text changed", m.file);
    }
}

#[test]
fn a_plain_text_part_is_shown_exactly_as_written() {
    // Including the text that looks like markup: a plain part is escaped, never parsed.
    for m in messages() {
        let Some(plain) = &m.plain else { continue };
        let out = clean(&m.file, &m.source);
        assert_eq!(
            document::read(&out.html).visible_text,
            *plain,
            "{}: plain text did not survive byte for byte",
            m.file
        );
        assert!(out.positions.is_empty(), "{}: plain text fetched", m.file);
        assert!(out.links.is_empty(), "{}: plain text grew a link", m.file);
    }
}

/// Ordered `(tag, attribute, value)` triples for the named tags and attributes, across the
/// whole tree — structure a screen reader navigates by.
fn structure(doc: &Document, tags: &[&str], attributes: &[&str]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for e in &doc.elements {
        if !tags.contains(&e.tag.as_str()) {
            continue;
        }
        out.push((e.tag.clone(), String::new(), String::new()));
        for &a in attributes {
            if let Some(v) = e.attribute(a) {
                out.push((e.tag.clone(), a.to_owned(), v.to_owned()));
            }
        }
    }
    out
}

fn assert_survives(
    label: &str,
    file: &str,
    before: &Document,
    after: &Document,
    tags: &[&str],
    attributes: &[&str],
) {
    assert_eq!(
        structure(before, tags, attributes),
        structure(after, tags, attributes),
        "{file}: {label} did not survive sanitization (NFR-50 — an accessibility defect, not a \
         cosmetic one)"
    );
}

#[test]
fn nfr50_accessibility_structure_survives_across_the_corpus() {
    for m in messages() {
        let before = document::read(&m.source);
        let after = document::read(&clean(&m.file, &m.source).html);

        assert_survives(
            "alternative text",
            &m.file,
            &before,
            &after,
            &["img"],
            &["alt"],
        );
        assert_survives(
            "heading levels",
            &m.file,
            &before,
            &after,
            &["h1", "h2", "h3", "h4", "h5", "h6"],
            &[],
        );
        assert_survives(
            "table structure",
            &m.file,
            &before,
            &after,
            &[
                "table", "caption", "colgroup", "col", "thead", "tbody", "tfoot", "tr", "th", "td",
            ],
            &["scope", "headers", "colspan", "rowspan"],
        );
        assert_survives(
            "list structure",
            &m.file,
            &before,
            &after,
            &["ul", "ol", "li", "dl", "dt", "dd", "blockquote"],
            &[],
        );
    }
}

#[test]
fn direction_and_language_survive_on_sender_content() {
    // A right-to-left message rendered left-to-right, or a Japanese one shaped with a
    // Chinese font, is a message the reader was not sent. Checked on sender content below the
    // scaffolding: `dir` and `lang` on `html` and `body` are lost when the walk unwraps
    // them, which is a known gap tracked on its own rather than asserted away here.
    let scaffolding = ["html", "head", "body"];
    for m in messages() {
        let pick = |doc: &Document| -> Vec<(String, String, String)> {
            doc.elements
                .iter()
                .filter(|e| !scaffolding.contains(&e.tag.as_str()))
                .flat_map(|e| {
                    ["dir", "lang"].into_iter().filter_map(|a| {
                        e.attribute(a)
                            .map(|v| (e.tag.clone(), a.to_owned(), v.to_owned()))
                    })
                })
                .collect()
        };
        let before = pick(&document::read(&m.source));
        let after = pick(&document::read(&clean(&m.file, &m.source).html));
        assert_eq!(before, after, "{}: dir or lang was lost", m.file);
    }
}

#[test]
fn every_fetching_position_is_recorded_with_what_the_sender_asked_for() {
    // I2 over real-shaped mail: images, srcset, background and font URLs all become internal
    // addresses, and the broker and debug view still see the original.
    for m in messages() {
        let out = clean(&m.file, &m.source);
        for p in &out.positions {
            assert!(!p.original.is_empty(), "{}: {p:?}", m.file);
            assert!(
                !out.html.contains(&p.original),
                "{}: {} survived in the output unrewritten",
                m.file,
                p.original
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Vectors: NFR-40 method 5
// ---------------------------------------------------------------------------

#[test]
fn every_published_mutation_xss_vector_meets_its_recorded_expectation() {
    let mut failures = Vec::new();
    for v in vectors() {
        match (v.expect.as_str(), sanitize(&v.payload)) {
            ("rejected", Ok(_)) => failures.push(format!(
                "{} ({}): expected a bound to reject it, and it was accepted",
                v.id, v.source
            )),
            ("rejected", Err(_)) => {}
            (_, Err(e)) => failures.push(format!(
                "{} ({}): expected clean, and it was rejected ({e:?})",
                v.id, v.source
            )),
            (_, Ok(out)) => {
                // Judged against the re-parsed tree. A vector handled correctly often still
                // contains `onerror` and `alert` — as escaped characters in an attribute
                // value, which the engine renders rather than runs.
                let violations = audit(&out.html);
                if !violations.is_empty() {
                    failures.push(format!(
                        "{} ({}): survived into the tree\n    {}\n    -> {}\n    {violations:?}",
                        v.id, v.source, v.payload, out.html
                    ));
                }
                match check_parse_stability(&v.payload) {
                    Ok(true) => {}
                    Ok(false) => failures.push(format!(
                        "{} ({}): not parse-stable (I8)\n    {}",
                        v.id, v.source, v.payload
                    )),
                    Err(e) => failures.push(format!(
                        "{} ({}): the second pass was rejected ({e:?})",
                        v.id, v.source
                    )),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

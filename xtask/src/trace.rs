//! `cargo xtask trace` — requirement traceability.
//!
//! > "A requirement is verified by tests that name it — 'which tests prove NFR-17' answerable
//! > by search. **A requirement with no instrument is unmet**, not deferred."
//!
//! This is that search, run as a check. It reads the requirement index, walks the source, and
//! reports every identifier the code never mentions.
//!
//! # What this proves and what it does not
//!
//! It proves a requirement is **named** somewhere a reader can find. It does not prove the
//! naming is a test, that the test is adequate, or that the implementation is correct. It is
//! the cheapest half of traceability and it catches the failure that actually happens: a
//! requirement nobody has looked at since it was written.
//!
//! Struck requirements are excluded. NFR-36 is struck as a verbatim duplicate of FR-36, its
//! number is retired and must never be reused, and a checker that demanded coverage for it
//! would be demanding coverage for a decision to delete something.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Identifiers the index carries, minus the struck ones.
fn requirements() -> Result<BTreeSet<String>, String> {
    let path = repo_root().join("docs/requirements.md");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    let mut out = BTreeSet::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("| ") else {
            continue;
        };
        let id = rest.split('|').next().unwrap_or("").trim();
        // A struck requirement is written `~~NFR-36~~`. Its number is retired, and demanding
        // coverage for it would be demanding coverage for a deletion.
        if id.starts_with("~~") {
            continue;
        }
        if id.starts_with("FR-") || id.starts_with("NFR-") {
            out.insert(id.to_owned());
        }
    }
    if out.is_empty() {
        return Err("no requirements parsed — has the index's table format changed?".to_owned());
    }
    Ok(out)
}

/// Every identifier mentioned anywhere in the source tree.
fn mentioned() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![repo_root().join("crates"), repo_root().join("shells")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if is_source(&path)
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                collect(&text, &mut found);
            }
        }
    }
    found
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "rs" | "swift" | "h" | "toml" | "sql" | "md"))
}

/// Pull `FR-n` and `NFR-n` out of text.
fn collect(text: &str, out: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let is_start = bytes[i] == b'F' || bytes[i] == b'N';
        if !is_start || (i > 0 && bytes[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        for prefix in ["NFR-", "FR-"] {
            if let Some(after) = rest.strip_prefix(prefix) {
                let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() {
                    out.insert(format!("{prefix}{digits}"));
                }
                break;
            }
        }
        i += 1;
    }
}

pub(crate) fn run() -> Result<(), String> {
    let required = requirements()?;
    let found = mentioned();
    let missing: Vec<&String> = required.difference(&found).collect();

    if missing.is_empty() {
        println!(
            "trace: {} requirements, every one named in the source",
            required.len()
        );
        return Ok(());
    }

    Err(format!(
        "{} requirement(s) are in docs/requirements.md and named nowhere in the source:\n  {}\n\n\
         verification.md: **a requirement with no instrument is unmet**, not deferred. Naming \
         it beside the code\nthat implements it, or beside the test that proves it, is what \
         makes \"which tests prove this\"\nanswerable by search.\n\n\
         {} of {} are covered.",
        missing.len(),
        missing
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        required.len() - missing.len(),
        required.len(),
    ))
}

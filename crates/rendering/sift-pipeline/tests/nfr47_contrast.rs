//! NFR-47 — the dark transform meets the contrast threshold for at least 95% of the fidelity
//! corpus. **Blocking**: this is the gate D-64 registered as outstanding, now with its metric,
//! its threshold and their derivation recorded in `docs/rendering/dark-mode.md`.
//!
//! The corpus is the one `sift-sanitize/fixtures/messages` holds and its own test admits, so
//! this gate measures the same set NFR-26 and NFR-40 do rather than a set chosen for it. Each
//! message goes through the shipped pipeline with the transform on — once at the ordinary
//! threshold, once with the system's increased-contrast preference set — and meets the gate
//! when the transform ran and left no text-on-background pair unrepaired.
//!
//! What counts, and why:
//!
//! - **Ran, nothing unrepaired** — meets it.
//! - **Ran, something unrepaired** — fails it, and is what the debug view surfaces.
//! - **The sender declared their own dark mode** — outside the denominator. FR-32 honours the
//!   sender first and the transform produces no output to measure; counting it either way
//!   would score Sift on the sender's palette.
//! - **The CSS was refused** — fails it. The reader asked for dark and received the light
//!   original, which is not a transform that met a threshold.
//!
//! A corpus in which the transform ran on no message leaves the gate unperformed, and that is
//! a failure here rather than a vacuous pass.

use sift_block::origin::{Authentication, Origin};
use sift_broker::broker::Broker;
use sift_pipeline::{Context, Selected, TransformOutcome, render};
use std::path::{Path, PathBuf};

/// NFR-47's share, in per cent.
const REQUIRED_PERCENT: usize = 95;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../sift-sanitize/fixtures/messages")
}

/// Every message the corpus manifest admits, as stage 2 would hand it over.
fn messages() -> Vec<(String, Selected)> {
    let dir = corpus();
    let manifest = std::fs::read_to_string(dir.join("manifest.txt"))
        .unwrap_or_else(|e| panic!("cannot read the corpus manifest: {e}"));
    manifest
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let fields: Vec<&str> = line.split('|').map(str::trim).collect();
            let [file, category, _provenance] = fields.as_slice() else {
                panic!("manifest row does not have three fields: {line}");
            };
            let body = std::fs::read_to_string(dir.join(file))
                .unwrap_or_else(|e| panic!("manifest names {file}, which cannot be read: {e}"));
            let selected = if *category == "plain-text" {
                Selected {
                    html: None,
                    text: Some(body),
                    reason: None,
                }
            } else {
                Selected {
                    html: Some(body),
                    text: None,
                    reason: None,
                }
            };
            ((*file).to_owned(), selected)
        })
        .collect()
}

fn origin() -> Origin {
    Origin::derive(&Authentication {
        signing_domain: Some("example.test".into()),
        envelope_domain: Some("example.test".into()),
        sender_policy_passed: true,
        from_domain: Some("example.test".into()),
    })
}

/// How many messages were measured, and how many of them met the threshold.
fn measure(increased_contrast: bool) -> (usize, usize, Vec<String>) {
    let (mut measured, mut met, mut failed) = (0, 0, Vec::new());
    for (file, selected) in messages() {
        let mut broker = Broker::new();
        let mut context = Context {
            origin: origin(),
            blocker: None,
            dark: true,
            increased_contrast,
            broker: &mut broker,
        };
        let rendered = render(&selected, &mut context).unwrap_or_else(|e| {
            panic!("{file}: the corpus admits it, the pipeline refused it: {e}")
        });
        match rendered.transform {
            TransformOutcome::SenderDeclaredTheirOwn => {}
            TransformOutcome::Ran { unrepaired: 0, .. } => {
                measured += 1;
                met += 1;
            }
            other => {
                measured += 1;
                failed.push(format!("{file}: {other:?}"));
            }
        }
    }
    (measured, met, failed)
}

fn gate(increased_contrast: bool) {
    let (measured, met, failed) = measure(increased_contrast);
    assert!(
        measured > 0,
        "NFR-47 unperformed: the transform ran on no corpus message, which is not a pass"
    );
    assert!(
        met * 100 >= REQUIRED_PERCENT * measured,
        "NFR-47 (increased contrast: {increased_contrast}): {met} of {measured} corpus messages \
         meet the threshold, below {REQUIRED_PERCENT}%. Failing: {failed:#?}"
    );
}

#[test]
fn nfr47_the_dark_transform_meets_the_threshold_across_the_corpus() {
    gate(false);
}

#[test]
fn nfr47_holds_at_the_raised_threshold_the_increased_contrast_preference_selects() {
    gate(true);
}

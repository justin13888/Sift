//! `cargo xtask deps` — the third dependency gate.
//!
//! The first two gates ask a question about the tree as it stands: is anything under an
//! advisory, is anything licensed in a way the App Store channel cannot carry. This one
//! asks a question about *change*: "a dependency added to the workspace, or a transitive
//! one appearing with an upgrade, is reviewed rather than absorbed."
//!
//! The threshold is deliberately low, and R-12 is why. Every decision in `docs/` is
//! defended against its local alternative and every defence holds on its own page; the
//! aggregate is the commitment nobody has explicitly accepted. R-12 stops at the
//! boundary of what this project builds and "cannot count what it imports" — Q-21 is
//! that gap. This file is where the import side becomes countable.

use crate::meta::workspace;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

fn approved_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../deps/approved.txt")
}

/// Crates that carry a root certificate store of their own.
///
/// D-30 chooses the platform's trust store and gives the reason: an installation whose
/// administrator has placed a root in the system store — a corporate inspection proxy, which
/// is the ordinary case for generic IMAP — expects it to work, and a bundled store silently
/// breaks every such account with a certificate error the user cannot act on.
///
/// The verifier Sift uses declares both of these for platforms Sift does not ship to, so
/// they appear in the resolve graph. This gate is about the *shipped* targets: neither may
/// be compiled into a macOS or Linux build, and the check is here rather than in a comment
/// because a feature flag flipped upstream would move one without any diff saying so.
const BUNDLED_ROOT_STORES: &[&str] = &["webpki-roots", "webpki-root-certs"];

/// The targets D-46 and `docs/product/platforms-and-distribution.md` name.
const SHIPPED_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

/// D-30, mechanically: no bundled root store reaches a shipped binary.
fn no_bundled_root_store() -> Result<(), String> {
    let mut findings = Vec::new();
    for target in SHIPPED_TARGETS {
        let output = Command::new("cargo")
            .args(["tree", "--workspace", "--edges", "normal", "--prefix", "none", "--target", target])
            .output()
            .map_err(|e| format!("cargo tree did not run: {e}"))?;
        if !output.status.success() {
            // A target whose standard library is not installed cannot be checked here, and
            // saying so is better than reporting a pass nobody earned.
            println!("deps: {target} could not be resolved, so D-30 is unchecked there");
            continue;
        }
        let tree = String::from_utf8_lossy(&output.stdout);
        for line in tree.lines() {
            let name = line.split_whitespace().next().unwrap_or("");
            if BUNDLED_ROOT_STORES.contains(&name) {
                findings.push(format!("  {name} is compiled into the {target} build"));
            }
        }
    }
    if findings.is_empty() {
        return Ok(());
    }
    Err(format!(
        "D-30 requires certificate verification against the platform's own trust store, and a \
         bundled one reached a shipped target:\n{}\n\nA bundled store does not merely differ \
         from the platform's — it breaks every account behind an inspection proxy, with a \
         certificate error the user cannot act on.",
        findings.join("\n")
    ))
}

pub(crate) fn run(bless: bool) -> Result<(), String> {
    let current = current_tree()?;
    no_bundled_root_store()?;

    if bless {
        write_approved(&current)?;
        println!("deps: approved list rewritten — {} crates", current.len());
        return Ok(());
    }

    let approved = read_approved()?;
    let added: Vec<_> = current.difference(&approved).cloned().collect();
    let removed: Vec<_> = approved.difference(&current).cloned().collect();

    if added.is_empty() && removed.is_empty() {
        println!("deps: {} vendored crates, all reviewed", current.len());
        return Ok(());
    }

    let mut report = String::new();
    if !added.is_empty() {
        report.push_str(&format!(
            "{} dependency edge(s) arrived without review:\n",
            added.len()
        ));
        for a in &added {
            report.push_str(&format!("  + {a}\n"));
        }
        report.push_str(
            "\nBefore blessing these, the questions docs/build/workspace.md asks:\n\
             - Is any of them on a hostile-input path? A MIME or CSS dependency that is a thin\n  \
               wrapper over unsafe parsing is held to the unsafe rule, not merely to review.\n\
             - Does its licence clear the allowlist in deny.toml? A single copyleft crate\n  \
               defeats the App Store channel (#26).\n\
             - Does it raise the toolchain floor? A vendored dependency's floor becomes Sift's\n  \
               on the day it is vendored.\n\
             - Does it start a thread, arm a timer, or open a socket of its own? Those are\n  \
               NFR-11 and NFR-24 respectively, and neither is visible in a diff.\n\n\
             Where the answer is not obvious, record it in deps/NOTES.md rather than in a\n\
             commit message nobody will find again.\n",
        );
    }
    if !removed.is_empty() {
        report.push_str(&format!(
            "\n{} approved dependenc(ies) no longer present — bless to record the removal:\n",
            removed.len()
        ));
        for r in &removed {
            report.push_str(&format!("  - {r}\n"));
        }
    }
    report.push_str("\nRun `cargo xtask deps --bless` once the answers are satisfactory.");
    Err(report)
}

/// Every crate in the resolved graph that is not a workspace member, as `name version`.
/// Versions are included because "a transitive one appearing with an upgrade" is exactly
/// the case a name-only list would miss.
fn current_tree() -> Result<BTreeSet<String>, String> {
    let ws = workspace()?;
    let members: BTreeSet<&str> = ws.members.iter().map(|m| m.name.as_str()).collect();

    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|e| format!("could not run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("cargo metadata is not JSON: {e}"))?;

    Ok(v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let version = p["version"].as_str()?;
            (!members.contains(name)).then(|| format!("{name} {version}"))
        })
        .collect())
}

fn read_approved() -> Result<BTreeSet<String>, String> {
    let path = approved_path();
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read {}: {e}\n\
             If this is a first run, `cargo xtask deps --bless` creates it.",
            path.display()
        )
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

fn write_approved(set: &BTreeSet<String>) -> Result<(), String> {
    let path = approved_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut s = String::from(
        "# Every crate vendored into Sift, with its version.\n\
         #\n\
         # This file is the third dependency gate of docs/build/workspace.md: a dependency\n\
         # added to the workspace, or a transitive one appearing with an upgrade, is reviewed\n\
         # rather than absorbed. A diff to this file is that review, and it is deliberately\n\
         # noisy — the threshold is low on purpose.\n\
         #\n\
         # Regenerate with `cargo xtask deps --bless`. Where the answers to the gate's\n\
         # questions were not obvious, they are recorded in deps/NOTES.md.\n\n",
    );
    for line in set {
        s.push_str(line);
        s.push('\n');
    }
    std::fs::write(&path, s).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

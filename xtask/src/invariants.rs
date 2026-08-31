//! `cargo xtask invariants` — the rules the specification states as code-review rules,
//! turned into build failures.
//!
//! Each of the three below is written in `docs/` as a prohibition rather than a design:
//! "a `match` on provider identity anywhere above the adapter layer is a defect",
//! "per-account or per-folder sleep loops are prohibited — this is a code-review rule",
//! and NFR-24's absolute. A code-review rule holds until the reviewer is tired, so each
//! one is asserted here instead.
//!
//! A line may opt out with a trailing `// sift-allow: <rule> — <reason>`, which makes
//! the exception visible in review rather than invisible in a reviewer's attention.

use crate::meta::{Member, workspace};
use std::path::Path;

struct Rule {
    name: &'static str,
    /// Substrings that are defects where the rule applies.
    banned: &'static [&'static str],
    /// Layers the rule covers. Empty means every layer.
    layers: &'static [&'static str],
    /// Crates exempt by name, because they are where the mechanism lives.
    exempt: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        name: "no-listening-socket",
        banned: &["TcpListener", "UnixListener", "UdpSocket", ".listen("],
        layers: &[],
        exempt: &[],
        why: "NFR-24: Sift MUST NOT open a listening socket of any kind, for any purpose — \
              not TCP, not a Unix domain socket, not an abstract namespace socket. There is no \
              carve-out. Single-instance is the platform's own mechanism, and D-36's \
              authorization callback arrives through a registered URI scheme. The forbidden \
              pattern is named in docs/architecture/shell-boundary.md: a lock file beside a \
              Unix domain socket that the second instance connects to.",
    },
    Rule {
        name: "no-provider-names",
        banned: &[
            "Gmail",
            "gmail",
            "GMAIL",
            "Jmap",
            "JMAP",
            "jmap",
            "Imap",
            "IMAP",
            "imap",
            "Outlook",
            "outlook",
            "MicrosoftGraph",
            "msgraph",
        ],
        layers: &["application", "presentation", "abi"],
        exempt: &[],
        why: "D-12: everything above the adapter layer plans against declared capabilities. \
              A `match` on provider identity above the adapter layer is a defect. If the \
              behaviour differs by provider, that difference is a capability row the model is \
              missing — see the five growth rules in docs/mail/provider-model.md.",
    },
    Rule {
        name: "no-sleep-loops",
        banned: &[
            "thread::sleep",
            "time::sleep",
            "sleep_until",
            "std::thread::park_timeout",
        ],
        layers: &[],
        exempt: &["sift-scheduler"],
        why: "D-25: all periodic work goes through the single coalesced timing wheel. \
              Per-account and per-folder sleep loops are prohibited, and the cost that matters \
              is wakeups rather than cycles (NFR-11). D-87 is the specific case most likely to \
              be got wrong: a provider's stated retry delay is a deadline on the wheel, never a \
              sleep.",
    },
];

pub(crate) fn run() -> Result<(), String> {
    let ws = workspace()?;
    let mut findings = Vec::new();
    let mut scanned = 0usize;

    for m in &ws.members {
        if m.name == "xtask" {
            continue;
        }
        for file in rust_sources(&m.root) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            scanned += 1;
            for (n, line) in text.lines().enumerate() {
                if allows(line) {
                    continue;
                }
                let code = strip_comment(line);
                for rule in RULES {
                    if !applies(rule, m) {
                        continue;
                    }
                    for tok in rule.banned {
                        if code.contains(tok) {
                            findings.push(format!(
                                "{}:{}: `{tok}` breaks {}\n  {}",
                                file.display(),
                                n + 1,
                                rule.name,
                                rule.why
                            ));
                        }
                    }
                }
            }
        }
    }

    if findings.is_empty() {
        println!(
            "invariants: {scanned} files, {} rules, no findings",
            RULES.len()
        );
        Ok(())
    } else {
        Err(findings.join("\n\n"))
    }
}

fn applies(rule: &Rule, m: &Member) -> bool {
    if rule.exempt.contains(&m.name.as_str()) {
        return false;
    }
    rule.layers.is_empty() || m.layer.as_deref().is_some_and(|l| rule.layers.contains(&l))
}

/// Everything after `//` is prose. Provider names and the word "sleep" belong in prose
/// — the specification uses them constantly — so only code is scanned.
fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

fn allows(line: &str) -> bool {
    line.contains("sift-allow:")
}

fn rust_sources(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("src")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Reported by `cargo xtask arch` as context rather than as a finding: which crates a
/// rule currently covers, so that adding a layer does not silently widen or narrow one.
pub(crate) fn coverage() -> Result<(), String> {
    let ws = workspace()?;
    for rule in RULES {
        let names: Vec<&str> = if rule.layers.is_empty() {
            ws.members
                .iter()
                .filter(|m| m.name != "xtask" && !rule.exempt.contains(&m.name.as_str()))
                .map(|m| m.name.as_str())
                .collect()
        } else {
            ws.crates_in(rule.layers)
                .into_iter()
                .filter(|m| !rule.exempt.contains(&m.name.as_str()))
                .map(|m| m.name.as_str())
                .collect()
        };
        println!("  {:<22} {} crates", rule.name, names.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Member;

    fn rule(name: &str) -> &'static Rule {
        RULES.iter().find(|r| r.name == name).expect("rule exists")
    }

    /// Provider names and the word "sleep" appear constantly in prose — the whole
    /// specification is about them. Scanning comments would make the rules unusable and
    /// the first response would be to delete them.
    #[test]
    fn prose_is_not_code() {
        assert_eq!(strip_comment("//! The Gmail adapter."), "");
        assert_eq!(strip_comment("let x = 1; // see IMAP notes"), "let x = 1; ");
        assert_eq!(strip_comment("let x = 1;"), "let x = 1;");
    }

    #[test]
    fn an_exception_is_visible_rather_than_absent() {
        assert!(allows(
            "let l = TcpListener::bind(a); // sift-allow: no-listening-socket — test"
        ));
        assert!(!allows("let l = TcpListener::bind(a);"));
    }

    #[test]
    fn the_provider_rule_covers_only_above_the_adapter_layer() {
        let r = rule("no-provider-names");
        // Above: the rule applies.
        for layer in ["application", "presentation", "abi"] {
            let m = Member::synthetic("c", Some(layer), &[]);
            assert!(applies(r, &m), "{layer} is above the adapter layer");
        }
        // At and below: an adapter must be able to name its own provider, and the
        // Account entity below stores which provider an account is.
        for layer in ["providers", "rendering", "storage", "foundation"] {
            let m = Member::synthetic("c", Some(layer), &[]);
            assert!(!applies(r, &m), "{layer} is not above the adapter layer");
        }
    }

    #[test]
    fn the_socket_rule_has_no_carve_out() {
        let r = rule("no-listening-socket");
        assert!(
            r.layers.is_empty() && r.exempt.is_empty(),
            "NFR-24 admits no exception"
        );
        for layer in LAYERS_FOR_TEST {
            assert!(applies(r, &Member::synthetic("c", Some(layer), &[])));
        }
    }

    #[test]
    fn only_the_scheduler_may_sleep() {
        let r = rule("no-sleep-loops");
        assert!(!applies(
            r,
            &Member::synthetic("sift-scheduler", Some("application"), &[])
        ));
        assert!(applies(
            r,
            &Member::synthetic("sift-sync", Some("application"), &[])
        ));
    }

    #[test]
    fn every_rule_states_why() {
        for r in RULES {
            assert!(!r.banned.is_empty(), "{} bans nothing", r.name);
            assert!(r.why.len() > 80, "{} does not say why", r.name);
        }
    }

    const LAYERS_FOR_TEST: &[&str] = crate::layers::LAYERS;
}

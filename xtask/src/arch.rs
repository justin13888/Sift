//! `cargo xtask arch` — the crate graph of D-59, asserted.
//!
//! Crate boundaries are Rust's most expensive refactor and these were drawn before any
//! code argued for them. That is the decision's own recorded weakness, and it is the
//! reason the graph is checked by a build step rather than by review: a wrong edge is
//! cheap to add and expensive to remove.

use crate::layers::{ADAPTERS, FORBIDDEN, LAYERS, MAY_REACH_THE_ABI, UNSAFE_PERMITTED, rank};
use crate::meta::{Workspace, workspace};
use std::collections::BTreeSet;

pub(crate) fn run() -> Result<(), String> {
    let ws = workspace()?;
    let mut findings: Vec<String> = Vec::new();

    every_crate_has_a_layer(&ws, &mut findings);
    no_upward_edges(&ws, &mut findings);
    named_prohibitions(&ws, &mut findings);
    adapters_cannot_see_each_other(&ws, &mut findings);
    the_abi_is_a_leaf(&ws, &mut findings);
    unsafe_is_confined(&ws, &mut findings);

    if findings.is_empty() {
        let n = ws.members.len();
        println!("arch: {n} crates, every edge one-way, the ABI a leaf, unsafe in four places");
        Ok(())
    } else {
        Err(findings.join("\n"))
    }
}

fn every_crate_has_a_layer(ws: &Workspace, out: &mut Vec<String>) {
    for m in &ws.members {
        // Tooling is deliberately outside crates/: it enforces the layer table rather
        // than living in it. Everything else must be placed.
        if m.name == "xtask" {
            continue;
        }
        if m.layer.is_none() {
            out.push(format!(
                "{}: sits outside crates/<layer>/, so no rule in D-59's table reaches it.\n  \
                 Layers are: {}",
                m.name,
                LAYERS.join(", ")
            ));
        }
    }
}

/// "Each may depend on those below and nothing above."
fn no_upward_edges(ws: &Workspace, out: &mut Vec<String>) {
    for m in &ws.members {
        let Some(from) = m.layer.as_deref().and_then(rank) else {
            continue;
        };
        for dep in ws.internal_deps(m) {
            let Some(to) = dep.layer.as_deref().and_then(rank) else {
                continue;
            };
            if to > from {
                out.push(format!(
                    "{} ({}) depends on {} ({}): that edge points upward.\n  \
                     D-59: every edge points one way, from the shell boundary down toward the store.\n  \
                     If the lower crate needs something from the higher one, the lower crate defines \
                     the trait and the higher one implements it.",
                    m.name,
                    m.layer.as_deref().unwrap_or("?"),
                    dep.name,
                    dep.layer.as_deref().unwrap_or("?"),
                ));
            }
        }
    }
}

/// The edges the layer table names rather than leaves to the ordering.
fn named_prohibitions(ws: &Workspace, out: &mut Vec<String>) {
    for m in &ws.members {
        let Some(from) = m.layer.as_deref() else {
            continue;
        };
        for dep in ws.internal_deps(m) {
            let Some(to) = dep.layer.as_deref() else {
                continue;
            };
            for (l, forbidden, words) in FORBIDDEN {
                if from == *l && to == *forbidden {
                    out.push(format!(
                        "{} -> {}: {}.\n  \
                         The resource broker is the one component in the rendering layer with an \
                         edge outward, and it expresses that edge as a trait it defines.",
                        m.name, dep.name, words
                    ));
                }
            }
        }
    }
}

/// D-59 names this edge explicitly: adapters are separate crates and may not see each
/// other. An adapter's fitness is tested by whether it compiles against the capability
/// crate alone, which is what makes D-12's "no `match` on provider identity" mechanical.
fn adapters_cannot_see_each_other(ws: &Workspace, out: &mut Vec<String>) {
    for m in &ws.members {
        if !ADAPTERS.contains(&m.name.as_str()) {
            continue;
        }
        for dep in ws.internal_deps(m) {
            if ADAPTERS.contains(&dep.name.as_str()) {
                out.push(format!(
                    "{} depends on {}: provider adapters may not see each other.\n  \
                     An adapter that needs another adapter is a capability the model is missing, \
                     not a dependency.",
                    m.name, dep.name
                ));
            }
            if dep.layer.as_deref() == Some("presentation") {
                out.push(format!(
                    "{} depends on {}: an adapter may not reach the presentation layer.",
                    m.name, dep.name
                ));
            }
        }
    }
}

/// "The C ABI is a leaf crate that nothing in the core depends on." This is D-17's
/// tripwire made operable: the whole boundary surface stays readable in one place.
fn the_abi_is_a_leaf(ws: &Workspace, out: &mut Vec<String>) {
    for m in &ws.members {
        if m.layer
            .as_deref()
            .is_some_and(|l| MAY_REACH_THE_ABI.contains(&l))
        {
            continue;
        }
        for dep in ws.internal_deps(m) {
            if dep.name == "sift-abi" {
                out.push(format!(
                    "{} depends on sift-abi: the ABI is a leaf and nothing in the core depends \
                     on it.\n  If its surface grows past what one file can hold, D-17 says that \
                     is the signal this was the wrong shape.",
                    m.name
                ));
            }
        }
    }
}

/// The unsafe prohibition is expressed as a crate-level lint in every crate but four.
/// This asserts the set of opt-outs is exactly those four — a fifth exception has to
/// ask rather than arrive.
fn unsafe_is_confined(ws: &Workspace, out: &mut Vec<String>) {
    let declared: BTreeSet<&str> = ws
        .members
        .iter()
        .filter(|m| m.name != "xtask" && m.allows_unsafe)
        .map(|m| m.name.as_str())
        .collect();
    let permitted: BTreeSet<&str> = UNSAFE_PERMITTED.iter().copied().collect();

    for extra in declared.difference(&permitted) {
        out.push(format!(
            "{extra} allows unsafe code, and is not one of the four places overview.md permits it.\n  \
             The four are: {}.\n  \
             A fifth exception is a change to docs/architecture/overview.md and \
             docs/build/workspace.md first, and to this list second.",
            UNSAFE_PERMITTED.join(", ")
        ));
    }
    for missing in permitted.difference(&declared) {
        out.push(format!(
            "{missing} is one of the four crates permitted unsafe code, but does not declare\n  \
             `[lints.rust] unsafe_code = \"allow\"`. Either it no longer needs the exception — in \
             which case take it off the list — or it is silently inheriting the workspace's \
             `forbid` and will not compile when it does."
        ));
    }
}

#[cfg(test)]
mod tests {
    //! Every rule below was confirmed against the real tree by mutating it, watching the
    //! check fail, and restoring. These are those cases made permanent, so that a rule
    //! which quietly stops firing is a test failure rather than a silence.

    use super::*;
    use crate::meta::Member;

    /// The four permitted crates, declared correctly, so a test exercising one rule does
    /// not trip the unsafe rule as a side effect.
    fn permitted() -> Vec<Member> {
        UNSAFE_PERMITTED
            .iter()
            .map(|n| {
                let layer = match *n {
                    "sift-abi" => "abi",
                    "sift-alloc" => "foundation",
                    _ => "storage",
                };
                Member::synthetic(n, Some(layer), &[]).allowing_unsafe()
            })
            .collect()
    }

    fn findings_for(extra: Vec<Member>, check: fn(&Workspace, &mut Vec<String>)) -> Vec<String> {
        let mut members = permitted();
        members.extend(extra);
        let ws = Workspace::of(members);
        let mut out = Vec::new();
        check(&ws, &mut out);
        out
    }

    #[test]
    fn an_upward_edge_is_a_finding() {
        let f = findings_for(
            vec![
                Member::synthetic("sift-foundation", Some("foundation"), &["sift-index"]),
                Member::synthetic("sift-index", Some("storage"), &[]),
            ],
            no_upward_edges,
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("points upward"));
    }

    #[test]
    fn a_downward_edge_is_fine() {
        let f = findings_for(
            vec![
                Member::synthetic("sift-sync", Some("application"), &["sift-index"]),
                Member::synthetic("sift-index", Some("storage"), &[]),
            ],
            no_upward_edges,
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn a_same_layer_edge_is_fine() {
        // sift-store depends on sift-crypto; both are storage. The rule is "nothing
        // above", not "nothing beside".
        let f = findings_for(
            vec![
                Member::synthetic("sift-store", Some("storage"), &["sift-crypto"]),
                Member::synthetic("sift-crypto", Some("storage"), &[]),
            ],
            no_upward_edges,
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn one_adapter_may_not_see_another() {
        let f = findings_for(
            vec![
                Member::synthetic("sift-jmap", Some("providers"), &["sift-imap"]),
                Member::synthetic("sift-imap", Some("providers"), &[]),
            ],
            adapters_cannot_see_each_other,
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("may not see each other"));
    }

    #[test]
    fn an_adapter_may_see_the_capability_crate() {
        let f = findings_for(
            vec![
                Member::synthetic("sift-imap", Some("providers"), &["sift-provider"]),
                Member::synthetic("sift-provider", Some("providers"), &[]),
            ],
            adapters_cannot_see_each_other,
        );
        assert!(
            f.is_empty(),
            "an adapter's fitness is that it compiles against this alone"
        );
    }

    #[test]
    fn nothing_may_depend_on_the_abi() {
        let f = findings_for(
            vec![Member::synthetic(
                "sift-presentation",
                Some("presentation"),
                &["sift-abi"],
            )],
            the_abi_is_a_leaf,
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("leaf"));
    }

    #[test]
    fn rendering_may_not_reach_the_store() {
        let f = findings_for(
            vec![
                Member::synthetic("sift-broker", Some("rendering"), &["sift-blobs"]),
                Member::synthetic("sift-blobs", Some("storage"), &[]),
            ],
            named_prohibitions,
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("may not reach the store"));
    }

    #[test]
    fn a_fifth_unsafe_exception_has_to_ask() {
        let f = findings_for(
            vec![Member::synthetic("sift-broker", Some("rendering"), &[]).allowing_unsafe()],
            unsafe_is_confined,
        );
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("not one of the four"));
    }

    #[test]
    fn a_permitted_crate_losing_its_exception_is_a_finding() {
        // Silently inheriting the workspace's `forbid` compiles until the day it does not.
        let ws = Workspace::of(vec![
            Member::synthetic("sift-abi", Some("abi"), &[]),
            Member::synthetic("sift-alloc", Some("foundation"), &[]).allowing_unsafe(),
            Member::synthetic("sift-crypto", Some("storage"), &[]).allowing_unsafe(),
            Member::synthetic("sift-store", Some("storage"), &[]).allowing_unsafe(),
        ]);
        let mut out = Vec::new();
        unsafe_is_confined(&ws, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("sift-abi"));
    }

    #[test]
    fn tooling_is_outside_the_layer_table() {
        let f = findings_for(
            vec![Member::synthetic("xtask", None, &[])],
            every_crate_has_a_layer,
        );
        assert!(
            f.is_empty(),
            "xtask enforces the table rather than living in it"
        );
    }

    #[test]
    fn an_unplaced_crate_is_a_finding() {
        let f = findings_for(
            vec![Member::synthetic("sift-stray", None, &[])],
            every_crate_has_a_layer,
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn the_layer_table_matches_the_directories_on_disk() {
        // A layer added to LAYERS without a directory, or a directory without a layer,
        // silently narrows every rule scoped by layer.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
        let mut on_disk: Vec<String> = std::fs::read_dir(&root)
            .expect("crates/ exists")
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        on_disk.sort();
        let mut declared: Vec<String> = LAYERS.iter().map(|s| (*s).to_owned()).collect();
        declared.sort();
        assert_eq!(declared, on_disk, "crates/ and LAYERS disagree");
    }
}

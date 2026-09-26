//! D-13's drift check: the half that needs the network.
//!
//! Fetches the provider's published v1.0 metadata and checks that every endpoint the committed
//! list names still has the evidence it cites — the navigation property, or the bound action —
//! and that every field a `$select` asks for is still a property of its entity type. It needs
//! the network, so it is an example rather than a test: D-63 puts a gate that cannot run on a
//! hosted runner in the per-integration tier.
//!
//! An endpoint whose evidence is `documented:` is published in the reference documentation but
//! not declared in the metadata. It is reported as **unchecked**, never as passed — a silent
//! pass would make the mechanism decorative.
//!
//! `mise run graph-schema`

// Printing is what a diagnostic *is*. The workspace denies it everywhere else because a
// library that prints has no way to be quiet, and NFR-55 bounds what Sift writes down —
// neither applies to a command a person runs by hand and reads the output of.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use sift_graph::{schema, wire};
use sift_provider::transport::{Request, Transport};

fn main() -> std::process::ExitCode {
    let mut https = match sift_http::Https::to(wire::API_HOST) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("the trust store could not be consulted: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let metadata = match https.exchange(&Request::new("GET", &wire::rooted("/$metadata"))) {
        Ok(r) if r.is_success() => match String::from_utf8(r.body) {
            Ok(text) => text,
            Err(_) => {
                eprintln!("the metadata was not UTF-8");
                return std::process::ExitCode::FAILURE;
            }
        },
        other => {
            eprintln!("the metadata could not be fetched: {other:?}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut drift = 0usize;
    let mut unchecked = 0usize;
    for endpoint in schema::endpoints() {
        let found = match endpoint.evidence.split_once(':') {
            Some(("nav", target)) => target.split_once('.').is_some_and(|(entity, nav)| {
                has_member(&metadata, entity, "NavigationProperty", nav)
            }),
            Some(("action", target)) => target
                .split_once('.')
                .is_some_and(|(entity, action)| has_bound_action(&metadata, entity, action)),
            Some(("documented", _)) => {
                println!(
                    "UNCHECKED  {} {}  ({} — not declared in the metadata)",
                    endpoint.method, endpoint.template, endpoint.evidence
                );
                unchecked += 1;
                continue;
            }
            _ => false,
        };
        if !found {
            println!(
                "DRIFT  {} {}  {} is not in the published metadata",
                endpoint.method, endpoint.template, endpoint.evidence
            );
            drift += 1;
        }
    }

    let mut fields = 0usize;
    for selection in schema::selections() {
        for field in &selection.fields {
            fields += 1;
            if !has_member(&metadata, &selection.entity, "Property", field) {
                println!(
                    "DRIFT  {}.{} is no longer a property",
                    selection.entity, field
                );
                drift += 1;
            }
        }
    }

    println!(
        "checked {} committed endpoints ({unchecked} unchecked) and {fields} selected fields against the published v1.0 metadata",
        schema::endpoints().len()
    );
    if drift == 0 {
        println!("no drift");
        std::process::ExitCode::SUCCESS
    } else {
        println!("{drift} item(s) drifted");
        std::process::ExitCode::FAILURE
    }
}

/// The body of an entity type's declaration, and the type it derives from.
fn entity<'a>(metadata: &'a str, name: &str) -> Option<(&'a str, Option<&'a str>)> {
    let open = format!("<EntityType Name=\"{name}\"");
    let start = metadata.find(&open)?;
    let rest = &metadata[start..];
    let head_end = rest.find('>')?;
    let head = &rest[..head_end];
    let body_end = rest.find("</EntityType>").unwrap_or(rest.len());
    let base = head
        .split_once("BaseType=\"")
        .and_then(|(_, b)| b.split_once('"'))
        .map(|(b, _)| b.rsplit('.').next().unwrap_or(b));
    Some((&rest[..body_end], base))
}

/// Whether an entity type, or one it derives from, declares a member of this kind and name.
fn has_member(metadata: &str, name: &str, kind: &str, member: &str) -> bool {
    let wanted = format!("<{kind} Name=\"{member}\"");
    let mut current = Some(name.to_owned());
    // Derivation chains here are a few deep; the bound stops a malformed document looping.
    for _ in 0..16 {
        let Some(name) = current.take() else {
            return false;
        };
        let Some((body, base)) = entity(metadata, &name) else {
            return false;
        };
        if body.contains(&wanted) {
            return true;
        }
        current = base.map(str::to_owned);
    }
    false
}

/// Whether an action of this name is bound to the entity type.
fn has_bound_action(metadata: &str, entity: &str, action: &str) -> bool {
    let open = format!("<Action Name=\"{action}\" IsBound=\"true\"");
    let binding = format!("Name=\"bindingParameter\" Type=\"graph.{entity}\"");
    metadata.match_indices(&open).any(|(start, _)| {
        let rest = &metadata[start..];
        let end = rest.find("</Action>").unwrap_or(rest.len());
        rest[..end].contains(&binding)
    })
}

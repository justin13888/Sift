//! D-13's drift check: the half that needs the network.
//!
//! Fetches the provider's published discovery document and checks that every endpoint the
//! committed list names still exists in it, with the same verb. It needs the network, so it
//! is an example rather than a test — D-63 puts a gate that cannot run on a hosted runner in
//! the per-integration tier.
//!
//! `cargo run -p sift-gmail --example schema-check`

// Printing is what a diagnostic *is*. The workspace denies it everywhere else because
// a library that prints has no way to be quiet, and NFR-55 bounds what Sift writes
// down — neither applies to a command a person runs by hand and reads the output of.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use sift_gmail::schema;
use sift_provider::transport::{Request, Transport};

fn main() -> std::process::ExitCode {
    let mut https = match sift_http::Https::to(sift_gmail::oauth::API_HOST) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("the trust store could not be consulted: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let response = match https.exchange(&Request::new("GET", "/$discovery/rest?version=v1")) {
        Ok(r) if r.is_success() => r,
        other => {
            eprintln!("the discovery document could not be fetched: {other:?}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let document: serde_json::Value = match serde_json::from_slice(&response.body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("the discovery document did not parse: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut published: Vec<(String, String, String)> = Vec::new();
    collect(&document, &mut published);
    let base = document
        .get("servicePath")
        .and_then(|s| s.as_str())
        .unwrap_or("gmail/v1/");

    let mut drift = 0usize;
    for endpoint in schema::endpoints() {
        let wanted = endpoint.template.trim_start_matches('/');
        match published.iter().find(|(id, _, _)| *id == endpoint.id) {
            Some((_, method, path)) => {
                let full = format!("{base}{path}");
                if *method != endpoint.method || full != wanted {
                    println!(
                        "DRIFT  {}  committed {} /{wanted}  published {method} /{full}",
                        endpoint.id, endpoint.method
                    );
                    drift += 1;
                }
            }
            None => {
                println!(
                    "GONE   {} is no longer in the published schema",
                    endpoint.id
                );
                drift += 1;
            }
        }
    }

    let revision = document
        .get("revision")
        .and_then(|r| r.as_str())
        .unwrap_or("unknown");
    println!(
        "checked {} committed endpoints against {} published methods (revision {revision})",
        schema::endpoints().len(),
        published.len()
    );
    if !schema::ENDPOINTS.contains(&format!("revision: {revision}")) {
        println!(
            "NOTE   the committed list was transcribed from a different revision; \
             re-transcribing it is the review D-13 asks for"
        );
    }
    if drift == 0 {
        println!("no drift");
        std::process::ExitCode::SUCCESS
    } else {
        println!("{drift} endpoint(s) drifted");
        std::process::ExitCode::FAILURE
    }
}

fn collect(node: &serde_json::Value, out: &mut Vec<(String, String, String)>) {
    if let Some(methods) = node.get("methods").and_then(|m| m.as_object()) {
        for method in methods.values() {
            let (Some(id), Some(verb), Some(path)) = (
                method.get("id").and_then(|v| v.as_str()),
                method.get("httpMethod").and_then(|v| v.as_str()),
                method.get("path").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            out.push((id.to_owned(), verb.to_owned(), path.to_owned()));
        }
    }
    if let Some(resources) = node.get("resources").and_then(|r| r.as_object()) {
        for resource in resources.values() {
            collect(resource, out);
        }
    }
}

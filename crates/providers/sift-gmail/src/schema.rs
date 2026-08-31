//! D-13's drift check: the offline half.
//!
//! The committed endpoint list beside this crate is the hand-written side of the diff the
//! decision requires. This module reads it and holds every target the adapter can construct
//! against it, so that a request to an endpoint nobody wrote down fails a test rather than a
//! server.
//!
//! The other half — comparing the committed list against the provider's live published
//! document — needs the network, so it is
//! `cargo run -p sift-gmail --example schema-check` and belongs to D-63's per-integration
//! tier rather than to per-change.

/// The endpoint list, as committed.
pub const ENDPOINTS: &str = include_str!("../schema/endpoints.txt");

/// One method and path template from the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub method: String,
    pub template: String,
    pub id: String,
}

/// Read the committed list.
#[must_use]
pub fn endpoints() -> Vec<Endpoint> {
    ENDPOINTS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let (spec, id) = line.split_once('#')?;
            let mut fields = spec.split_whitespace();
            Some(Endpoint {
                method: fields.next()?.to_owned(),
                template: fields.next()?.to_owned(),
                id: id.trim().to_owned(),
            })
        })
        .collect()
}

/// Whether a concrete path matches a template, treating `{name}` as one path segment.
#[must_use]
pub fn matches(template: &str, path: &str) -> bool {
    // The query string is not part of the schema's path.
    let path = path.split('?').next().unwrap_or(path);
    let expected: Vec<&str> = template.split('/').collect();
    let actual: Vec<&str> = path.split('/').collect();
    expected.len() == actual.len()
        && expected.iter().zip(actual.iter()).all(|(e, a)| {
            if e.starts_with('{') && e.ends_with('}') {
                !a.is_empty()
            } else {
                e == a
            }
        })
}

/// The identifier of the endpoint a request lands on, where it lands on one.
#[must_use]
pub fn resolve(method: &str, path: &str) -> Option<String> {
    endpoints()
        .into_iter()
        // The user segment is `me` in every request Sift makes; the template names it
        // `{userId}`, so it matches as a parameter like any other.
        .find(|e| e.method == method && matches(&e.template, path))
        .map(|e| e.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire;
    use sift_provider::adapter::{RemoteFolderId, RemoteMessageId};

    fn message() -> RemoteMessageId {
        RemoteMessageId("18f2c".into())
    }

    /// Every target this adapter can construct, with the verb it is sent with.
    ///
    /// **The list is exhaustive by construction**: a target builder added to `wire` without
    /// a line here has no test, and a target built here without a line in the committed
    /// schema fails the assertion below.
    fn every_target() -> Vec<(&'static str, String)> {
        vec![
            ("GET", wire::profile_target()),
            ("GET", wire::labels_target()),
            ("POST", format!("{}/labels", wire::USER)),
            (
                "GET",
                wire::list_target(&RemoteFolderId("INBOX".into()), None, 500),
            ),
            (
                "GET",
                wire::list_target(&RemoteFolderId("INBOX".into()), Some("tok"), 500),
            ),
            (
                "GET",
                wire::history_target("1", &RemoteFolderId("INBOX".into()), None, 500),
            ),
            ("GET", wire::envelope_target(&message())),
            ("GET", wire::structure_target(&message())),
            ("GET", wire::attachment_target(&message(), "ANGjdJ")),
            ("POST", wire::batch_modify_target()),
            ("POST", wire::trash_target(&message())),
            ("POST", wire::untrash_target(&message())),
        ]
    }

    #[test]
    fn every_target_the_adapter_builds_is_an_endpoint_somebody_wrote_down() {
        for (method, target) in every_target() {
            assert!(
                resolve(method, &target).is_some(),
                "{method} {target} matches no endpoint in schema/endpoints.txt"
            );
        }
    }

    #[test]
    fn the_committed_list_parses_and_is_the_size_it_says_it_is() {
        let endpoints = endpoints();
        assert_eq!(endpoints.len(), 10, "the header's count and the list disagree");
        assert!(ENDPOINTS.contains("revision: 20260824"));
    }

    #[test]
    fn nothing_in_the_committed_list_sends() {
        // The published schema declares `gmail.users.messages.send` and
        // `gmail.users.drafts.send`. This is the test that fires if either is ever added,
        // which is the whole reason the client is hand-written rather than generated.
        for endpoint in endpoints() {
            let id = endpoint.id.to_ascii_lowercase();
            assert!(
                !id.contains("send") && !id.contains("draft"),
                "`{}` would put a send path in the shipped binary",
                endpoint.id
            );
        }
    }

    #[test]
    fn a_template_matches_only_a_path_of_the_same_shape() {
        assert!(matches("/a/{id}/b", "/a/xyz/b"));
        assert!(matches("/a/{id}", "/a/xyz?format=metadata"));
        assert!(!matches("/a/{id}/b", "/a/xyz"));
        assert!(!matches("/a/{id}", "/a/xyz/b"));
        assert!(!matches("/a/{id}", "/a/"));
    }

    #[test]
    fn the_batch_endpoint_is_deliberately_absent_from_the_schema() {
        // It is not a Gmail method — it is the transport-level batch envelope, and the
        // requests inside it are the ones the schema covers. That is why `batch_target` is
        // not in `every_target` above, and this test says so rather than leaving a reader to
        // wonder whether it was forgotten.
        assert_eq!(wire::batch_target(), "/batch/gmail/v1");
        assert!(resolve("POST", &wire::batch_target()).is_none());
    }

    #[test]
    fn a_batched_envelope_fetch_is_still_a_schema_endpoint() {
        // What travels inside the batch is what has to match, and it does.
        let target = wire::envelope_target(&message());
        assert_eq!(
            resolve("GET", &target).as_deref(),
            Some("gmail.users.messages.get")
        );
    }
}

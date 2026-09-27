//! D-13's drift check: the offline half.
//!
//! The committed list beside this crate is the hand-written side of the diff the decision
//! requires. This module reads it and holds every target the adapter can construct, and every
//! field it selects, against it — so that a request to an endpoint nobody wrote down fails a
//! test rather than a server.
//!
//! The other half — comparing the committed list against the provider's live published
//! metadata — needs the network, so it is `mise run graph-schema` and belongs to D-63's
//! per-integration tier rather than to per-change.

/// The list, as committed.
pub const ENDPOINTS: &str = include_str!("../schema/endpoints.txt");

/// One method and path template from the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub method: String,
    pub template: String,
    /// What the live check looks for in the published metadata.
    pub evidence: String,
}

/// One `SELECT` line: an entity type and the fields asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub entity: String,
    pub fields: Vec<String>,
}

fn lines() -> impl Iterator<Item = &'static str> {
    ENDPOINTS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Read the committed endpoints.
#[must_use]
pub fn endpoints() -> Vec<Endpoint> {
    lines()
        .filter(|l| !l.starts_with("SELECT"))
        .filter_map(|line| {
            let (spec, evidence) = line.split_once('#')?;
            let mut fields = spec.split_whitespace();
            Some(Endpoint {
                method: fields.next()?.to_owned(),
                template: fields.next()?.to_owned(),
                evidence: evidence.trim().to_owned(),
            })
        })
        .collect()
}

/// Read the committed field lists.
#[must_use]
pub fn selections() -> Vec<Selection> {
    lines()
        .filter_map(|line| line.strip_prefix("SELECT"))
        .filter_map(|rest| {
            let mut words = rest.split_whitespace();
            Some(Selection {
                entity: words.next()?.to_owned(),
                fields: words.map(str::to_owned).collect(),
            })
        })
        .collect()
}

/// Whether a concrete path matches a template, treating `{name}` as one path segment.
#[must_use]
pub fn matches(template: &str, path: &str) -> bool {
    // The query string is not part of the template's path.
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

/// The evidence of the endpoint a request lands on, where it lands on one.
///
/// `path` is either relative to the version root or rooted at it.
#[must_use]
pub fn resolve(method: &str, path: &str) -> Option<String> {
    let path = path.strip_prefix(crate::wire::ROOT).unwrap_or(path);
    endpoints()
        .into_iter()
        .find(|e| e.method == method && matches(&e.template, path))
        .map(|e| e.evidence)
}

/// The fields a target's `$select` names, if it has one.
#[must_use]
pub fn selected(target: &str) -> Vec<String> {
    target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.strip_prefix("$select="))
        .flat_map(|fields| fields.split(','))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire;
    use sift_provider::adapter::{RemoteFolderId, RemoteMessageId};

    fn message() -> RemoteMessageId {
        RemoteMessageId("AAMkAGI2=".into())
    }

    /// Every target this adapter can construct, with the verb it is sent with and the entity
    /// type its `$select` is asked of.
    ///
    /// **Exhaustive by construction**: a target builder added to `wire` without a line here
    /// has no test, and a target built here without a line in the committed list fails below.
    fn every_target() -> Vec<(&'static str, String, Option<&'static str>)> {
        vec![
            ("GET", wire::folders_target(), Some("mailFolder")),
            ("GET", wire::child_folders_target("f1"), Some("mailFolder")),
            ("GET", wire::well_known_target("inbox"), Some("mailFolder")),
            (
                "GET",
                wire::delta_target(&RemoteFolderId("f1".into())),
                Some("message"),
            ),
            ("GET", wire::envelope_target(&message()), Some("message")),
            (
                "GET",
                wire::search_target("quokka from:a@example.invalid", 50),
                Some("message"),
            ),
            ("GET", wire::categories_target(&message()), Some("message")),
            ("GET", wire::body_target(&message()), Some("message")),
            ("PATCH", wire::message_target(&message()), None),
            ("POST", wire::move_target(&message()), None),
            ("POST", wire::permanent_delete_target(&message()), None),
            (
                "GET",
                wire::attachments_target(&message()),
                Some("attachment"),
            ),
            (
                "GET",
                wire::attachment_value_target(&message(), "AAMkAtt="),
                None,
            ),
        ]
    }

    #[test]
    fn every_target_the_adapter_builds_is_an_endpoint_somebody_wrote_down() {
        for (method, target, _) in every_target() {
            assert!(
                resolve(method, &target).is_some(),
                "{method} {target} matches no endpoint in schema/endpoints.txt"
            );
            // And in the rooted form a direct request sends.
            assert!(resolve(method, &wire::rooted(&target)).is_some());
        }
    }

    #[test]
    fn every_field_the_adapter_selects_is_a_field_somebody_wrote_down() {
        let selections = selections();
        for (_, target, entity) in every_target() {
            let fields = selected(&target);
            let Some(entity) = entity else {
                assert!(fields.is_empty(), "{target} selects with no entity named");
                continue;
            };
            let listed = selections
                .iter()
                .find(|s| s.entity == entity)
                .unwrap_or_else(|| panic!("no SELECT line for {entity}"));
            assert!(!fields.is_empty(), "{target} names no fields");
            for field in fields {
                assert!(
                    listed.fields.contains(&field),
                    "{target} selects `{field}`, which the SELECT line for {entity} does not list"
                );
            }
        }
    }

    #[test]
    fn the_committed_list_parses_and_is_the_size_it_says_it_is() {
        assert_eq!(
            endpoints().len(),
            11,
            "the header's count and the list disagree"
        );
        assert!(ENDPOINTS.contains("endpoints used here: 11"));
        assert_eq!(selections().len(), 3);
    }

    #[test]
    fn nothing_in_the_committed_list_sends() {
        // The published metadata binds `send`, `reply`, `replyAll`, `forward` and the
        // `create*` draft forms to the message type. This fires if any is ever added, which
        // is the whole reason the client is hand-written rather than generated.
        for endpoint in endpoints() {
            let text = format!("{} {}", endpoint.template, endpoint.evidence).to_ascii_lowercase();
            for word in ["send", "reply", "forward", "create", "draft", "sendmail"] {
                assert!(
                    !text.contains(word),
                    "`{}` would put a send path in the shipped binary",
                    endpoint.template
                );
            }
        }
    }

    #[test]
    fn a_template_matches_only_a_path_of_the_same_shape() {
        assert!(matches("/a/{id}/b", "/a/xyz/b"));
        assert!(matches("/a/{id}", "/a/xyz?$select=id"));
        assert!(!matches("/a/{id}/b", "/a/xyz"));
        assert!(!matches("/a/{id}", "/a/xyz/b"));
        assert!(!matches("/a/{id}", "/a/"));
    }

    #[test]
    fn the_batch_endpoint_is_deliberately_absent_from_the_list() {
        // It is the transport-level envelope, and the requests inside it are the ones the
        // list covers. That is why `batch_target` is not in `every_target` above.
        assert_eq!(wire::batch_target(), "/v1.0/$batch");
        assert!(resolve("POST", &wire::batch_target()).is_none());
    }
}

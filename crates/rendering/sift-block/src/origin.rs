//! D-11 and D-37 — the synthetic first-party origin, and FR-28's authentication record.
//!
//! # The problem nobody else has
//!
//! Filter syntax assumes a document origin: third-party matching, domain-scoped rules and
//! hostname-scoped cosmetic filters all key on "what site is this?". **An email has none.**
//! Every rule that distinguishes first- from third-party is meaningless without one.
//!
//! So the origin is synthesized, and the property that makes it worth doing is stated in
//! D-11 rather than discovered: **authentication results directly harden blocking.** A
//! spoofed sender does not get the real sender's allowances, because the origin it gets is
//! the one its own authentication earned.
//!
//! # Failing to null rather than to permissive
//!
//! When nothing resolves the origin is **null**: everything is third-party and the
//! strictest rules apply. That is the correct failure direction, and it has a consequence
//! the interface has to carry — FR-8's durable "always show images from this sender"
//! **cannot exist** for a null-origin message, because the allowlist keys on the origin and
//! there is nothing to key on. The interface must say so rather than offering a control
//! that quietly does nothing.

/// How much the origin is worth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// A passing cryptographic signature. The signing domain is the origin.
    Attested,
    /// The envelope sender's domain, where the sender-policy check passed.
    AttestedByPolicy,
    /// The `From` header's domain, with nothing behind it. **Marked low-confidence**, and
    /// the interface shows it as such.
    Unauthenticated,
}

/// FR-28 — what authentication established, stored on the message at ingest.
///
/// Runs at ingest rather than at render time because it feeds two consumers with different
/// lifetimes: the origin, which the blocker needs on every resource request, and the debug
/// view, which needs it long after.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authentication {
    /// The domain of a **passing** signature. A failing signature is not a weaker signal
    /// than no signature; it is the same signal, and is recorded as `None`.
    pub signing_domain: Option<String>,
    /// The envelope sender's domain, and whether the sender-policy check passed.
    pub envelope_domain: Option<String>,
    pub sender_policy_passed: bool,
    /// The `From` header's domain.
    pub from_domain: Option<String>,
}

/// The origin a message renders under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Synthetic {
        domain: String,
        confidence: Confidence,
    },
    /// Nothing resolved. Everything is third-party.
    Null,
}

impl Origin {
    /// D-11's priority order, applied strictly.
    #[must_use]
    pub fn derive(auth: &Authentication) -> Self {
        if let Some(d) = &auth.signing_domain {
            return Self::Synthetic {
                domain: d.clone(),
                confidence: Confidence::Attested,
            };
        }
        if auth.sender_policy_passed
            && let Some(d) = &auth.envelope_domain
        {
            return Self::Synthetic {
                domain: d.clone(),
                confidence: Confidence::AttestedByPolicy,
            };
        }
        if let Some(d) = &auth.from_domain {
            return Self::Synthetic {
                domain: d.clone(),
                confidence: Confidence::Unauthenticated,
            };
        }
        Self::Null
    }

    #[must_use]
    pub fn domain(&self) -> Option<&str> {
        match self {
            Self::Synthetic { domain, .. } => Some(domain),
            Self::Null => None,
        }
    }

    /// Whether the origin is attested — priority 1 or 2.
    ///
    /// This is the gate D-37 opens on, and **it is what keeps the infrastructure list from
    /// being a hole**: a listed host is first-party only for a sender whose identity was
    /// actually established.
    #[must_use]
    pub const fn is_attested(&self) -> bool {
        matches!(
            self,
            Self::Synthetic {
                confidence: Confidence::Attested | Confidence::AttestedByPolicy,
                ..
            }
        )
    }

    /// Whether a durable per-sender allowance can be keyed on this origin — FR-8.
    ///
    /// False for a null origin, and the interface **MUST say so** rather than presenting a
    /// control that silently does nothing. A one-off "show images once" is still available;
    /// what is unavailable is "always", because there is nothing to remember it against.
    #[must_use]
    pub const fn can_carry_a_durable_allowance(&self) -> bool {
        !matches!(self, Self::Null)
    }
}

/// D-37 — known mail-service infrastructure.
///
/// A bundled list, updated with the binary. A resource host on it is first-party **when and
/// only when the message's origin was attested**.
///
/// D-37 records its own weakness: a flat list widens *any* attested sender to *any* listed
/// host, and the correct refinement is a map of attested-signer to permitted host. The
/// refinement is a strict narrowing of this — the list becomes a map and nothing else
/// changes — so the shape here is the seam rather than an obstacle.
#[derive(Debug, Clone, Default)]
pub struct Infrastructure {
    hosts: Vec<String>,
    /// The refinement, where it is known: an attested signer paired with the hosts it may
    /// use. An entry here takes precedence over the flat list.
    pairs: Vec<(String, Vec<String>)>,
}

impl Infrastructure {
    #[must_use]
    pub fn new(hosts: Vec<String>) -> Self {
        Self {
            hosts,
            pairs: Vec::new(),
        }
    }

    /// Record the narrower form for a signer whose infrastructure is known.
    pub fn pair(&mut self, signer: &str, hosts: Vec<String>) {
        self.pairs.push((signer.to_ascii_lowercase(), hosts));
    }

    /// Whether `host` counts as first-party for a message under `origin`.
    ///
    /// **Gated on attestation.** An unauthenticated sender gets nothing from this list,
    /// which is what stops it being a way to launder a spoofed sender into first-party
    /// treatment.
    #[must_use]
    pub fn is_first_party(&self, origin: &Origin, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let Some(domain) = origin.domain() else {
            return false;
        };

        // The sender's own domain is first-party regardless of the list.
        if same_site(&host, &domain.to_ascii_lowercase()) {
            return true;
        }
        if !origin.is_attested() {
            return false;
        }
        // The refinement first, where it exists: a pair is narrower than the flat list and
        // saying "this signer may use these hosts" is the answer D-37 prefers.
        let signer = domain.to_ascii_lowercase();
        if let Some((_, permitted)) = self.pairs.iter().find(|(s, _)| *s == signer) {
            return permitted.iter().any(|h| same_site(&host, h));
        }
        self.hosts.iter().any(|h| same_site(&host, h))
    }
}

/// Whether `host` is `site` or a subdomain of it.
fn same_site(host: &str, site: &str) -> bool {
    host == site || host.ends_with(&format!(".{site}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(domain: &str) -> Authentication {
        Authentication {
            signing_domain: Some(domain.to_owned()),
            ..Authentication::default()
        }
    }

    #[test]
    fn a_passing_signature_wins() {
        let auth = Authentication {
            signing_domain: Some("signer.test".into()),
            envelope_domain: Some("envelope.test".into()),
            sender_policy_passed: true,
            from_domain: Some("from.test".into()),
        };
        let o = Origin::derive(&auth);
        assert_eq!(o.domain(), Some("signer.test"));
        assert!(o.is_attested());
    }

    #[test]
    fn a_passing_policy_is_second() {
        let auth = Authentication {
            envelope_domain: Some("envelope.test".into()),
            sender_policy_passed: true,
            from_domain: Some("from.test".into()),
            ..Authentication::default()
        };
        assert_eq!(Origin::derive(&auth).domain(), Some("envelope.test"));
        assert!(Origin::derive(&auth).is_attested());
    }

    #[test]
    fn a_failing_policy_does_not_attest() {
        let auth = Authentication {
            envelope_domain: Some("envelope.test".into()),
            sender_policy_passed: false,
            from_domain: Some("from.test".into()),
            ..Authentication::default()
        };
        let o = Origin::derive(&auth);
        assert_eq!(
            o.domain(),
            Some("from.test"),
            "a failed policy check was treated as passing"
        );
        assert!(!o.is_attested());
    }

    #[test]
    fn the_from_header_alone_is_low_confidence() {
        let auth = Authentication {
            from_domain: Some("from.test".into()),
            ..Authentication::default()
        };
        assert_eq!(
            Origin::derive(&auth),
            Origin::Synthetic {
                domain: "from.test".into(),
                confidence: Confidence::Unauthenticated
            }
        );
    }

    #[test]
    fn nothing_resolving_is_null_rather_than_permissive() {
        // The correct failure direction: everything third-party, strictest rules.
        assert_eq!(Origin::derive(&Authentication::default()), Origin::Null);
        assert!(!Origin::Null.is_attested());
    }

    #[test]
    fn a_null_origin_cannot_carry_a_durable_allowance() {
        // FR-8's "always show images from this sender" keys on the origin, and there is
        // nothing to key on. The interface must say so rather than offering a control that
        // quietly does nothing.
        assert!(!Origin::Null.can_carry_a_durable_allowance());
        assert!(Origin::derive(&signed("a.test")).can_carry_a_durable_allowance());
    }

    #[test]
    fn a_spoofed_sender_does_not_inherit_the_real_ones_allowances() {
        // "Authentication results directly harden blocking." The spoofer gets an
        // unauthenticated origin, so D-37's list does not open for it.
        let list = Infrastructure::new(vec!["mailinfra.test".into()]);
        let real = Origin::derive(&signed("bank.test"));
        let spoof = Origin::derive(&Authentication {
            from_domain: Some("bank.test".into()),
            ..Authentication::default()
        });
        assert!(list.is_first_party(&real, "img.mailinfra.test"));
        assert!(
            !list.is_first_party(&spoof, "img.mailinfra.test"),
            "an unauthenticated sender was widened to known infrastructure"
        );
    }

    #[test]
    fn the_senders_own_domain_is_first_party_without_the_list() {
        let o = Origin::derive(&signed("bank.test"));
        assert!(Infrastructure::default().is_first_party(&o, "images.bank.test"));
    }

    #[test]
    fn a_null_origin_makes_everything_third_party() {
        let list = Infrastructure::new(vec!["mailinfra.test".into()]);
        assert!(!list.is_first_party(&Origin::Null, "mailinfra.test"));
        assert!(!list.is_first_party(&Origin::Null, "anything.test"));
    }

    #[test]
    fn the_attested_pair_map_narrows_the_flat_list() {
        // D-37's own preferred refinement: "the list becomes a map, and nothing else
        // changes". A flat list widens any attested sender to any listed host; a pair says
        // which hosts this signer may use.
        let mut list = Infrastructure::new(vec!["shared.test".into(), "other.test".into()]);
        list.pair("bank.test", vec!["bank-cdn.test".into()]);

        let bank = Origin::derive(&signed("bank.test"));
        assert!(list.is_first_party(&bank, "bank-cdn.test"));
        assert!(
            !list.is_first_party(&bank, "other.test"),
            "the pair did not narrow the flat list"
        );

        // A signer with no pair still falls back to the flat list.
        let shop = Origin::derive(&signed("shop.test"));
        assert!(list.is_first_party(&shop, "shared.test"));
    }

    #[test]
    fn a_lookalike_domain_is_not_a_subdomain() {
        let list = Infrastructure::new(vec!["mailinfra.test".into()]);
        let o = Origin::derive(&signed("bank.test"));
        assert!(!list.is_first_party(&o, "evil-mailinfra.test"));
        assert!(!list.is_first_party(&o, "mailinfra.test.evil.test"));
        assert!(list.is_first_party(&o, "cdn.mailinfra.test"));
    }
}

//! What the platform can tell us, and what it cannot.

/// Whether a usable path exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    Online,
    /// A path that carries packets but not to the provider — D-96.
    CaptivePortal,
    /// A path that is up but restricted.
    Limited,
    /// No path at all.
    Offline,
}

/// What kind of link it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClass {
    Wired,
    Wifi,
    Cellular,
    Tethered,
    Vpn,
    /// **Not a failure to report — a report.** D-14: "when nothing resolves, report unknown.
    /// Never guess 'not metered'."
    Unknown,
}

/// Whether the link is metered.
///
/// Three-valued rather than boolean, because the third value is the common one and the
/// expensive one to get wrong: NFR-30 maps unknown to Conservative, and a boolean would have
/// to pick a default, which is the guess D-14 forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metered {
    Yes,
    No,
    Unknown,
}

/// One observation of the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conditions {
    pub reachability: Reachability,
    pub link: LinkClass,
    pub metered: Metered,
    /// The platform's own low-data or constrained mode.
    pub constrained: bool,
    /// A stable identity for this network, so FR-35's override can be remembered against it.
    ///
    /// Without one, a per-network override is a per-session override — which is not what
    /// "the detection will be wrong sometimes" needs, because the network it is wrong about
    /// is one the user returns to.
    pub network_identity: Option<String>,
}

impl Default for Conditions {
    /// What is assumed before anything is observed.
    ///
    /// Offline and unknown, so that the first decision made with no information is the
    /// conservative one rather than the permissive one.
    fn default() -> Self {
        Self {
            reachability: Reachability::Offline,
            link: LinkClass::Unknown,
            metered: Metered::Unknown,
            constrained: false,
            network_identity: None,
        }
    }
}

/// D-96 — how a captive portal is detected.
///
/// **From the behaviour of connections to the user's own providers.** Sift contacts no
/// detection endpoint, its own or anyone else's, and the reasons are two:
///
/// A sixty-second beacon from a resident application is "a coarse record of when this
/// machine is awake and roughly where it is" — the disclosure the privacy egress table exists
/// to enumerate. And under D-33 it would be **permanent**, for Q-18's reason: a build keeps
/// calling the address it shipped with for as long as it stays installed.
///
/// The signal is better anyway. An intercepting portal **cannot present a valid certificate
/// for the provider's name**, so interception is distinguishable from being offline — which
/// a connectivity probe to a third party is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionOutcome {
    Succeeded,
    /// A handshake that failed in a way consistent with interception.
    TlsInterception,
    /// A response that was not the provider's protocol.
    NotTheExpectedProtocol,
    /// Nothing answered.
    NoRoute,
}

/// What one account's connection attempt says about the network.
#[must_use]
pub const fn infer(outcome: ConnectionOutcome) -> Reachability {
    match outcome {
        ConnectionOutcome::Succeeded => Reachability::Online,
        ConnectionOutcome::TlsInterception | ConnectionOutcome::NotTheExpectedProtocol => {
            Reachability::CaptivePortal
        }
        ConnectionOutcome::NoRoute => Reachability::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_observed_is_the_conservative_assumption() {
        let c = Conditions::default();
        assert_eq!(c.metered, Metered::Unknown);
        assert_eq!(c.reachability, Reachability::Offline);
    }

    #[test]
    fn metered_is_three_valued_because_unknown_is_the_common_case() {
        // A boolean would have to pick a default, which is the guess D-14 forbids.
        assert_ne!(Metered::Unknown, Metered::No);
        assert_ne!(Metered::Unknown, Metered::Yes);
    }

    #[test]
    fn interception_is_distinguishable_from_being_offline() {
        // The reason D-96's signal is better than a probe: an intercepting portal cannot
        // present a valid certificate for the provider's name.
        assert_eq!(
            infer(ConnectionOutcome::TlsInterception),
            Reachability::CaptivePortal
        );
        assert_eq!(infer(ConnectionOutcome::NoRoute), Reachability::Offline);
        assert_ne!(
            infer(ConnectionOutcome::TlsInterception),
            infer(ConnectionOutcome::NoRoute)
        );
    }

    #[test]
    fn a_response_that_is_not_the_protocol_is_a_portal() {
        assert_eq!(
            infer(ConnectionOutcome::NotTheExpectedProtocol),
            Reachability::CaptivePortal
        );
    }
}

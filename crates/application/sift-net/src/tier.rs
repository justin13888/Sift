//! D-58, D-95 — the policy tiers.

use crate::condition::{Conditions, LinkClass, Metered, Reachability};

/// The six tiers.
///
/// D-58 makes pause the **sixth row** rather than a second mechanism beside them, which
/// resolves a contradiction the documents carried: FR-22's pause and FR-36's cap were
/// specified separately and would have been implemented twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Wired or wifi, definitely unmetered.
    Unrestricted,
    /// Metered unknown or guessed, or cellular.
    Conservative,
    /// Metered, or the platform's constrained mode.
    Minimal,
    /// A captive portal. **One bounded reattempt per L-28**, of the account's own next
    /// operation — D-96, never a probe to a detection host.
    OfflinePortal,
    /// No path. **Zero connection attempts of any kind**, including the reattempt above,
    /// until the path returns — NFR-38.
    OfflineNoPath,
    /// The user paused, or the FR-36 cap was reached.
    Paused,
}

impl Tier {
    /// Resolve a tier from the network and from state Sift holds.
    ///
    /// D-58 records that this is a wider input than the table implies: the tier comes from
    /// the network's answer **and** from whether the user paused or the cap fired.
    #[must_use]
    pub fn resolve(conditions: &Conditions, paused: bool, over_cap: bool) -> Self {
        if paused || over_cap {
            return Self::Paused;
        }
        match conditions.reachability {
            // Offline is **two states**, and collapsing them contradicts NFR-38: a portal is
            // a usable path that lies, and airplane mode is the absence of a path where a
            // probe can only fail.
            Reachability::Offline => return Self::OfflineNoPath,
            Reachability::CaptivePortal => return Self::OfflinePortal,
            Reachability::Limited | Reachability::Online => {}
        }
        if conditions.constrained || conditions.metered == Metered::Yes {
            return Self::Minimal;
        }
        // NFR-30: **unknown metered maps to Conservative, never Unrestricted.**
        if conditions.metered == Metered::Unknown
            || matches!(conditions.link, LinkClass::Cellular | LinkClass::Unknown)
        {
            return Self::Conservative;
        }
        Self::Unrestricted
    }

    /// Whether the mutation queue flushes.
    ///
    /// **True in every tier above offline, including Paused.** This is the contradiction
    /// D-58 resolves: what pause stops is *fetching*. A pause that also held mutations would
    /// mean a user who archived something and then paused had an archive that never
    /// happened, which is not what the affordance says.
    #[must_use]
    pub const fn mutations_flush(self) -> bool {
        !matches!(self, Self::OfflinePortal | Self::OfflineNoPath)
    }

    /// Whether any speculative prefetch is permitted — NFR-32.
    #[must_use]
    pub const fn prefetch(self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    /// Whether bodies are fetched ahead of being asked for.
    #[must_use]
    pub const fn prefetch_bodies(self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    /// Whether filter lists may update. NFR-43: stale is acceptable, absent is not.
    #[must_use]
    pub const fn filter_list_updates(self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    /// Whether push is preferred over polling.
    ///
    /// **Inverted on cellular, and only on cellular.** A keepalive is cheap in bytes and
    /// each one promotes the radio to a high-power state through the tail timer, which is
    /// what NFR-37 bounds at six radio-waking events per hour.
    #[must_use]
    pub const fn prefer_push(self, link: LinkClass) -> bool {
        !matches!(link, LinkClass::Cellular)
            && matches!(self, Self::Unrestricted | Self::Conservative)
    }

    /// Whether any connection may be attempted.
    #[must_use]
    pub const fn may_connect(self) -> bool {
        !matches!(self, Self::OfflineNoPath)
    }
}

/// FR-35 — a per-network user override, which always wins.
///
/// "Detection will be wrong sometimes" — tethered Ethernet, a corporate VPN over cellular, a
/// hotspot presenting as wifi. D-14 requires this be **designed in from the start rather
/// than bolted on when detection is found wanting**, because bolting it on means every
/// decision site has to remember to consult it.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    entries: Vec<(String, Tier)>,
}

impl Overrides {
    pub fn set(&mut self, network_identity: &str, tier: Tier) {
        self.entries.retain(|(id, _)| id != network_identity);
        self.entries.push((network_identity.to_owned(), tier));
    }

    /// The tier to use: the override where there is one, detection otherwise.
    #[must_use]
    pub fn apply(&self, conditions: &Conditions, detected: Tier) -> Tier {
        conditions
            .network_identity
            .as_ref()
            .and_then(|id| self.entries.iter().find(|(k, _)| k == id))
            .map_or(detected, |(_, t)| *t)
    }
}

/// FR-36 — data accounting.
///
/// **Bytes on the wire** — transport framing, encryption overhead and retransmissions
/// included — taken from the platform's per-connection accounting where offered, never
/// application-layer payload sizes. Changing what it counts later silently changes when a
/// cap fires, which is why the definition is here rather than at each call site.
#[derive(Debug, Clone, Default)]
pub struct Accounting {
    /// Per account per link class.
    per_account: Vec<(u128, LinkClass, u64)>,
    /// Traffic belonging to no account.
    ///
    /// Filter-list and infrastructure-list updates, and FR-3's autoconfiguration discovery —
    /// which happens *before the account exists*, so charging it to one is impossible as
    /// well as wrong. **Charged to the installation, and counting toward no account's cap.**
    installation: u64,
}

impl Accounting {
    pub fn record(&mut self, account: u128, link: LinkClass, bytes_on_the_wire: u64) {
        if let Some(e) = self
            .per_account
            .iter_mut()
            .find(|(a, l, _)| *a == account && *l == link)
        {
            e.2 += bytes_on_the_wire;
        } else {
            self.per_account.push((account, link, bytes_on_the_wire));
        }
    }

    pub const fn record_installation(&mut self, bytes_on_the_wire: u64) {
        self.installation += bytes_on_the_wire;
    }

    #[must_use]
    pub fn total_for(&self, account: u128) -> u64 {
        self.per_account
            .iter()
            .filter(|(a, _, _)| *a == account)
            .map(|(_, _, b)| b)
            .sum()
    }

    #[must_use]
    pub const fn installation_total(&self) -> u64 {
        self.installation
    }

    /// Whether an account has reached its cap.
    ///
    /// The installation's own traffic is **excluded**, which is what stops a filter-list
    /// update pausing somebody's mail.
    #[must_use]
    pub fn over_cap(&self, account: u128, cap: Option<u64>) -> bool {
        cap.is_some_and(|c| self.total_for(account) >= c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(link: LinkClass, metered: Metered) -> Conditions {
        Conditions {
            reachability: Reachability::Online,
            link,
            metered,
            constrained: false,
            network_identity: Some("net-1".into()),
        }
    }

    #[test]
    fn unknown_metered_maps_to_conservative_never_unrestricted() {
        // NFR-30, and the failure it prevents: guessing "not metered" on somebody's phone
        // hotspot spends their data allowance on a backfill.
        assert_eq!(
            Tier::resolve(&online(LinkClass::Wifi, Metered::Unknown), false, false),
            Tier::Conservative
        );
    }

    #[test]
    fn definitely_unmetered_wifi_is_unrestricted() {
        assert_eq!(
            Tier::resolve(&online(LinkClass::Wifi, Metered::No), false, false),
            Tier::Unrestricted
        );
    }

    #[test]
    fn cellular_is_conservative_even_when_it_says_it_is_not_metered() {
        assert_eq!(
            Tier::resolve(&online(LinkClass::Cellular, Metered::No), false, false),
            Tier::Conservative
        );
    }

    #[test]
    fn offline_is_two_states() {
        // Collapsing them contradicts NFR-38: a portal is a usable path that lies, and
        // airplane mode is the absence of a path where a probe can only fail.
        let portal = Conditions {
            reachability: Reachability::CaptivePortal,
            ..Conditions::default()
        };
        let none = Conditions {
            reachability: Reachability::Offline,
            ..Conditions::default()
        };
        assert_eq!(Tier::resolve(&portal, false, false), Tier::OfflinePortal);
        assert_eq!(Tier::resolve(&none, false, false), Tier::OfflineNoPath);
        assert!(Tier::OfflinePortal.may_connect());
        assert!(
            !Tier::OfflineNoPath.may_connect(),
            "airplane mode attempted a connection"
        );
    }

    #[test]
    fn mutations_flush_while_paused() {
        // The contradiction D-58 resolves. A pause that held mutations would mean a user who
        // archived something and then paused had an archive that never happened.
        assert!(Tier::Paused.mutations_flush());
        assert!(Tier::Minimal.mutations_flush());
        assert!(!Tier::OfflineNoPath.mutations_flush());
    }

    #[test]
    fn pausing_and_the_cap_resolve_to_one_tier() {
        // D-58: everything that pauses sync resolves to the same row, rather than two
        // mechanisms implemented separately.
        let c = online(LinkClass::Wifi, Metered::No);
        assert_eq!(Tier::resolve(&c, true, false), Tier::Paused);
        assert_eq!(Tier::resolve(&c, false, true), Tier::Paused);
    }

    #[test]
    fn no_speculative_prefetch_below_unrestricted() {
        // NFR-32, and the broker enforces the same thing from the other side: a prefetch
        // **is** the tracking event.
        for t in [
            Tier::Conservative,
            Tier::Minimal,
            Tier::Paused,
            Tier::OfflinePortal,
        ] {
            assert!(!t.prefetch(), "{t:?} prefetched");
            assert!(!t.filter_list_updates(), "{t:?} updated filter lists");
        }
        assert!(Tier::Unrestricted.prefetch());
    }

    #[test]
    fn push_is_dropped_on_cellular_and_only_on_cellular() {
        // A keepalive is cheap in bytes; each one promotes the radio through the tail timer,
        // which NFR-37 bounds at six wakes per hour.
        assert!(!Tier::Unrestricted.prefer_push(LinkClass::Cellular));
        assert!(Tier::Unrestricted.prefer_push(LinkClass::Wifi));
        assert!(Tier::Conservative.prefer_push(LinkClass::Wired));
    }

    #[test]
    fn a_per_network_override_always_wins() {
        // FR-35. Detection will be wrong sometimes, and the network it is wrong about is one
        // the user returns to — which is why the override is keyed on network identity.
        let mut o = Overrides::default();
        o.set("net-1", Tier::Unrestricted);
        let detected = Tier::Conservative;
        assert_eq!(
            o.apply(&online(LinkClass::Cellular, Metered::Yes), detected),
            Tier::Unrestricted
        );
    }

    #[test]
    fn an_override_for_another_network_does_not_apply() {
        let mut o = Overrides::default();
        o.set("net-2", Tier::Unrestricted);
        assert_eq!(
            o.apply(
                &online(LinkClass::Wifi, Metered::Unknown),
                Tier::Conservative
            ),
            Tier::Conservative
        );
    }

    #[test]
    fn accounting_counts_per_account_per_link_class() {
        let mut a = Accounting::default();
        a.record(1, LinkClass::Wifi, 100);
        a.record(1, LinkClass::Cellular, 50);
        a.record(2, LinkClass::Wifi, 999);
        assert_eq!(a.total_for(1), 150);
        assert_eq!(a.total_for(2), 999);
    }

    #[test]
    fn installation_traffic_counts_toward_no_accounts_cap() {
        // Otherwise a filter-list update would pause somebody's mail — and FR-3's
        // autoconfiguration happens before the account exists, so charging it to one is
        // impossible as well as wrong.
        let mut a = Accounting::default();
        a.record_installation(10_000);
        a.record(1, LinkClass::Wifi, 10);
        assert!(!a.over_cap(1, Some(100)));
        assert_eq!(a.installation_total(), 10_000);
    }

    #[test]
    fn a_cap_fires_on_the_accounts_own_traffic() {
        let mut a = Accounting::default();
        a.record(1, LinkClass::Cellular, 100);
        assert!(a.over_cap(1, Some(100)));
        assert!(!a.over_cap(1, None), "an account with no cap was capped");
    }
}

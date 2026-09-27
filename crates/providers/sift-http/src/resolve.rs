//! Which addresses a name may be dialled at.
//!
//! # Why the check is on the address, not the name
//!
//! A host name is text the far end chose. A message body names image hosts, and a sender who
//! controls a public DNS name can make it answer with the reader's router, printer, or NAS —
//! an address on the reader's own network that no inspection of the *name* can see. The
//! resource broker's promise (`docs/architecture/resource-broker.md`) is that a message is not
//! permitted to aim the reader's machine at their printer, so the refusal has to be made on
//! the address actually connected to.
//!
//! **The name is resolved once, here, and only an address that passed is dialled.** A check
//! made anywhere else — on the name, or on an earlier resolution — leaves a window in which the
//! name re-resolves (DNS rebinding) between the check and the connect.

use sift_provider::transport::TransportError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

/// Turns a host name into the addresses it names.
///
/// Injectable so a test can make a name answer with any address without touching the network;
/// [`System`] is the one Sift ships.
pub trait Resolve: Send + Sync {
    /// # Errors
    /// Where the name could not be resolved at all.
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>>;
}

/// The platform's own resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Resolve for System {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok((host, port).to_socket_addrs()?.collect())
    }
}

/// Whether an address is one on the public internet rather than on the reader's own machine
/// or network.
///
/// Refused: loopback, RFC 1918 private, link-local, shared (carrier-grade NAT) space, "this
/// network", broadcast, and multicast for IPv4; loopback, unspecified, unique-local,
/// link-local, site-local, and multicast for IPv6; and an IPv6 address that embeds an IPv4
/// one (mapped or compatible) is judged by the IPv4 address it carries, so
/// `::ffff:192.168.1.1` is the private address it decodes to.
#[must_use]
pub fn is_public(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    let this_network = a == 0;
    let shared = a == 100 && (b & 0xc0) == 64;
    !(this_network
        || shared
        || v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_multicast())
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(v6) {
        return is_public_v4(v4);
    }
    let first = v6.segments()[0];
    let unique_local = (first & 0xfe00) == 0xfc00;
    let link_local = (first & 0xffc0) == 0xfe80;
    let site_local = (first & 0xffc0) == 0xfec0;
    !(v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || unique_local
        || link_local
        || site_local)
}

/// The IPv4 address an IPv6 one carries, where it is IPv4-mapped (`::ffff:a.b.c.d`) or
/// IPv4-compatible (`::a.b.c.d`). `::` and `::1` are not compatible addresses; they are the
/// unspecified and loopback addresses, and are judged as IPv6.
fn embedded_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4);
    }
    let s = v6.segments();
    let compatible = s[..6].iter().all(|&x| x == 0) && (s[6] != 0 || s[7] > 1);
    compatible.then(|| Ipv4Addr::from((u32::from(s[6]) << 16) | u32::from(s[7])))
}

/// Resolve `host` once and keep only the addresses [`is_public`] admits, in the resolver's
/// order.
///
/// # Errors
/// [`TransportError::Transient`] where the name did not resolve, which may be the network
/// rather than the name; [`TransportError::Refused`] where it resolved and every address it
/// named is on the reader's own machine or network.
pub fn admissible(
    resolver: &dyn Resolve,
    host: &str,
    port: u16,
) -> Result<Vec<SocketAddr>, TransportError> {
    let resolved = resolver
        .resolve(host, port)
        .map_err(|_| TransportError::Transient)?;
    if resolved.is_empty() {
        return Err(TransportError::Transient);
    }
    let public: Vec<SocketAddr> = resolved.into_iter().filter(|a| is_public(a.ip())).collect();
    if public.is_empty() {
        return Err(TransportError::Refused(format!(
            "`{host}` names only addresses on this machine or its local network"
        )));
    }
    Ok(public)
}

/// A resolver that answers every name with a fixed list, for tests.
#[cfg(test)]
pub(crate) struct Fixed(pub Vec<IpAddr>);

#[cfg(test)]
impl Resolve for Fixed {
    fn resolve(&self, _host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok(self.0.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_address_on_the_readers_own_network_is_not_public() {
        for addr in [
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "127.0.0.1",
            "127.8.8.8",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "255.255.255.255",
            "224.0.0.251",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::fb",
            "::ffff:192.168.1.1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::192.168.1.1",
        ] {
            assert!(!is_public(ip(addr)), "{addr} was admitted");
        }
    }

    #[test]
    fn an_address_on_the_public_internet_is_public() {
        for addr in [
            "93.184.216.34",
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2606:2800:220:1:248:1893:25c8:1946",
            "::ffff:93.184.216.34",
        ] {
            assert!(is_public(ip(addr)), "{addr} was refused");
        }
    }

    #[test]
    fn a_name_that_resolves_only_to_a_private_address_is_refused() {
        let got = admissible(&Fixed(vec![ip("192.168.1.1")]), "images.example.com", 443);
        assert!(matches!(got, Err(TransportError::Refused(_))), "{got:?}");
    }

    #[test]
    fn a_name_that_resolves_to_a_mapped_private_address_is_refused() {
        let got = admissible(
            &Fixed(vec![ip("::ffff:192.168.1.1")]),
            "images.example.com",
            443,
        );
        assert!(matches!(got, Err(TransportError::Refused(_))), "{got:?}");
    }

    #[test]
    fn only_the_public_addresses_of_a_mixed_answer_are_dialled() {
        let got = admissible(
            &Fixed(vec![ip("10.0.0.1"), ip("93.184.216.34"), ip("fe80::1")]),
            "images.example.com",
            443,
        )
        .unwrap();
        assert_eq!(got, vec![SocketAddr::new(ip("93.184.216.34"), 443)]);
    }

    #[test]
    fn a_public_name_still_resolves_to_its_address() {
        let got = admissible(
            &Fixed(vec![ip("2606:2800:220:1:248:1893:25c8:1946")]),
            "images.example.com",
            443,
        )
        .unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn a_name_that_resolves_to_nothing_is_transient_rather_than_refused() {
        assert_eq!(
            admissible(&Fixed(vec![]), "images.example.com", 443),
            Err(TransportError::Transient)
        );
    }
}

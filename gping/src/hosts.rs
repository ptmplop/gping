//! Turning what the user typed into something to ping.

use anyhow::{anyhow, bail, Context, Result};
use std::net::{IpAddr, Ipv6Addr, ToSocketAddrs};

pub struct Target {
    /// What to hand to the pinger. For a name this is the address it resolved to, so the
    /// probe goes exactly where the header says it does, and in the address family that
    /// was asked for with -4/-6 (ping would otherwise resolve the name again by itself).
    pub probe: String,
    /// Whether `probe` is an IPv6 target.
    pub ipv6: bool,
    /// The address a name resolved to, shown next to it. `None` when the host is already an
    /// address.
    pub address: Option<IpAddr>,
}

pub fn resolve(host: &str, force_ipv4: bool, force_ipv6: bool) -> Result<Target> {
    let check_family = |ipv6: bool| -> Result<()> {
        if force_ipv4 && ipv6 {
            bail!("{host} is not an IPv4 address");
        }
        if force_ipv6 && !ipv6 {
            bail!("{host} is not an IPv6 address");
        }
        Ok(())
    };

    if let Ok(ip) = host.parse::<IpAddr>() {
        check_family(ip.is_ipv6())?;
        return Ok(Target {
            probe: host.to_string(),
            ipv6: ip.is_ipv6(),
            address: None,
        });
    }
    // A scoped IPv6 literal (fe80::1%en0). The zone is lost if it goes through an IpAddr,
    // and a link-local address is useless without it, so it's passed on as typed.
    if let Some((address, zone)) = host.split_once('%') {
        if !zone.is_empty() && address.parse::<Ipv6Addr>().is_ok() {
            check_family(true)?;
            return Ok(Target {
                probe: host.to_string(),
                ipv6: true,
                address: None,
            });
        }
    }

    let ascii = if host.is_ascii() {
        host.to_string()
    } else {
        idna::domain_to_ascii(host)
            .map_err(|_| anyhow!("Could not encode host {host} to punycode"))?
    };
    let addresses: Vec<IpAddr> = (ascii.as_str(), 80)
        .to_socket_addrs()
        .with_context(|| format!("Resolving {host}"))?
        .map(|s| s.ip())
        .collect();
    let address = if force_ipv4 {
        addresses.iter().find(|ip| ip.is_ipv4())
    } else if force_ipv6 {
        addresses.iter().find(|ip| ip.is_ipv6())
    } else {
        addresses.first()
    };
    let address = match address {
        Some(address) => *address,
        None if force_ipv4 => bail!("Could not resolve '{host}' to IPv4"),
        None if force_ipv6 => bail!("Could not resolve '{host}' to IPv6"),
        None => bail!("Could not resolve hostname {host}"),
    };
    Ok(Target {
        probe: address.to_string(),
        ipv6: address.is_ipv6(),
        address: Some(address),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_used_as_they_are() {
        let target = resolve("192.0.2.7", false, false).unwrap();
        assert_eq!(target.probe, "192.0.2.7");
        assert!(!target.ipv6);
        assert!(target.address.is_none());
        let target = resolve("::1", false, true).unwrap();
        assert!(target.ipv6 && target.address.is_none());
    }

    #[test]
    fn an_address_of_the_wrong_family_is_rejected() {
        assert!(resolve("::1", true, false).is_err());
        assert!(resolve("127.0.0.1", false, true).is_err());
        assert!(resolve("fe80::1%lo0", true, false).is_err());
    }

    #[test]
    fn scoped_ipv6_literals_keep_their_zone() {
        let target = resolve("fe80::1%lo0", false, false).unwrap();
        assert_eq!(target.probe, "fe80::1%lo0");
        assert!(target.ipv6);
    }

    // The probe goes to the address the header shows, in the family asked for. macOS used
    // to show 'localhost (::1)' while ping, resolving the name again, pinged 127.0.0.1.
    #[test]
    fn names_are_probed_at_their_resolved_address() {
        let v4 = resolve("localhost", true, false).unwrap();
        assert_eq!(v4.probe, "127.0.0.1");
        assert_eq!(v4.address, Some("127.0.0.1".parse().unwrap()));
        assert!(!v4.ipv6);
    }
}

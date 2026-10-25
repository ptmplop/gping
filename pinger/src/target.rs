use std::fmt;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum IPVersion {
    V4,
    V6,
    Any,
}

#[derive(Debug, Clone)]
pub enum Target {
    IP(IpAddr),
    Hostname { domain: String, version: IPVersion },
}

impl Target {
    pub fn is_ipv6(&self) -> bool {
        match self {
            Target::IP(ip) => ip.is_ipv6(),
            Target::Hostname { version, .. } => *version == IPVersion::V6,
        }
    }

    /// A hostname that has to be resolved to an IPv4 address. An IPv4 address needs no
    /// telling, and ping flags that say so are not understood by every ping (BusyBox
    /// built without IPv6, Android's).
    #[cfg(unix)]
    pub(crate) fn is_ipv4_hostname(&self) -> bool {
        matches!(
            self,
            Target::Hostname {
                version: IPVersion::V4,
                ..
            }
        )
    }

    /// An address, which ping can start pinging without looking anything up. That
    /// includes a scoped IPv6 address ("fe80::1%en0"), which `IpAddr` doesn't parse.
    #[cfg(unix)]
    pub(crate) fn is_address(&self) -> bool {
        match self {
            Target::IP(_) => true,
            Target::Hostname { domain, .. } => domain
                .split_once('%')
                .is_some_and(|(address, _)| address.parse::<Ipv6Addr>().is_ok()),
        }
    }

    pub fn new_any(value: impl ToString) -> Self {
        let value = value.to_string();
        if let Ok(ip) = value.parse::<IpAddr>() {
            return Self::IP(ip);
        }
        Self::Hostname {
            domain: value,
            version: IPVersion::Any,
        }
    }

    pub fn new_ipv4(value: impl ToString) -> Self {
        let value = value.to_string();
        if let Ok(ip) = value.parse::<Ipv4Addr>() {
            return Self::IP(IpAddr::V4(ip));
        }
        Self::Hostname {
            domain: value.to_string(),
            version: IPVersion::V4,
        }
    }

    pub fn new_ipv6(value: impl ToString) -> Self {
        let value = value.to_string();
        if let Ok(ip) = value.parse::<Ipv6Addr>() {
            return Self::IP(IpAddr::V6(ip));
        }
        Self::Hostname {
            domain: value.to_string(),
            version: IPVersion::V6,
        }
    }
}

impl Display for Target {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Target::IP(v) => Display::fmt(&v, f),
            Target::Hostname { domain, .. } => Display::fmt(&domain, f),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn addresses_need_no_lookup() {
        assert!(Target::new_any("192.0.2.1").is_address());
        assert!(Target::new_ipv6("2001:db8::1").is_address());
        assert!(Target::new_ipv6("fe80::1%en0").is_address());
        assert!(!Target::new_any("example.com").is_address());
        assert!(!Target::new_any("example.com%en0").is_address());
    }
}

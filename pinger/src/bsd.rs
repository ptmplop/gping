use crate::{
    bsd_style_schedule, extract_regex, interval_arg, PingCreationError, PingOptions, PingResult,
    Pinger, Schedule,
};
use lazy_regex::*;
use std::net::IpAddr;

pub static RE: Lazy<Regex> = lazy_regex!(r"time=(?:(?P<ms>[0-9]+).(?P<ns>[0-9]+)\s+ms)");

pub struct BSDPinger {
    options: PingOptions,
}

pub(crate) fn parse_bsd(line: String) -> Option<PingResult> {
    if line.starts_with("PING ") {
        return None;
    }
    if line.contains("Request timeout") {
        return Some(PingResult::Timeout(line));
    }
    extract_regex(&RE, line)
}

impl Pinger for BSDPinger {
    fn from_options(options: PingOptions) -> Result<Self, PingCreationError>
    where
        Self: Sized,
    {
        // BSD ping has no way to send through a named interface: FreeBSD and DragonFly
        // read -I as the address of a multicast interface (and reject "em0" with
        // "invalid multicast interface"), NetBSD and OpenBSD as a source address. A
        // source address is the closest thing on offer.
        if let Some(interface) = &options.interface {
            if interface.parse::<IpAddr>().is_err() {
                return Err(PingCreationError::NotSupported {
                    alternative: format!(
                        "BSD ping cannot bind to the interface {interface:?}; \
                         pass that interface's IP address instead, to use it as the source address"
                    ),
                });
            }
        }
        Ok(Self { options })
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        parse_bsd
    }

    // It says nothing about a lost packet.
    fn schedule(&self) -> Option<Schedule> {
        bsd_style_schedule(&self.options)
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        let ipv6 = self.options.target.is_ipv6();
        // OpenBSD's ping only does IPv6 when it runs as ping6, and NetBSD's and
        // DragonFly's are IPv4 only; FreeBSD installs ping6 as an alias of ping.
        let cmd = if ipv6 { "ping6" } else { "ping" };
        let mut args = vec![interval_arg(self.options.interval)];
        // FreeBSD's ping does both families and otherwise takes whichever address the
        // resolver lists first; the others' ping is IPv4 only, and OpenBSD's rejects -4.
        if cfg!(target_os = "freebsd") && self.options.target.is_ipv4_hostname() {
            args.push("-4".into());
        }
        if let Some(source) = &self.options.interface {
            // NetBSD's IPv4 ping has no -S; its -I is the source address.
            let flag = if cfg!(target_os = "netbsd") && !ipv6 {
                "-I"
            } else {
                "-S"
            };
            args.push(flag.into());
            args.push(source.clone());
        }
        if let Some(raw_args) = &self.options.raw_arguments {
            args.extend(raw_args.iter().cloned());
        }
        args.push(self.options.target.to_string());
        (cmd, args)
    }
}

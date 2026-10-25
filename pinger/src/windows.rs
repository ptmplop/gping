use crate::target::{IPVersion, Target};
use crate::PingCreationError;
use crate::{extract_regex, PingOptions, PingResult, Pinger};
use lazy_regex::*;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use winping::{Buffer, Pinger as WinPinger};

/// How long IcmpSendEcho waits for a reply.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);

pub static RE: Lazy<Regex> = lazy_regex!(r"(?ix-u)time=(?P<ms>\d+)(?:\.(?P<ns>\d+))?");

pub struct WindowsPinger {
    options: PingOptions,
}

impl Pinger for WindowsPinger {
    fn from_options(options: PingOptions) -> Result<Self, PingCreationError> {
        Ok(Self { options })
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        |line| {
            if line.contains("timed out") || line.contains("failure") {
                return Some(PingResult::Timeout(line));
            }
            extract_regex(&RE, line)
        }
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        unimplemented!("ping_args for WindowsPinger is not implemented")
    }

    fn start(&self) -> Result<mpsc::Receiver<PingResult>, PingCreationError> {
        let interval = self.options.interval;
        let parsed_ip = match &self.options.target {
            Target::IP(ip) => *ip,
            Target::Hostname { domain, version } => {
                let ips = (domain.as_str(), 0).to_socket_addrs()?;
                let selected_ips: Vec<_> = if *version == IPVersion::Any {
                    ips.collect()
                } else {
                    ips.into_iter()
                        .filter(|addr| {
                            if *version == IPVersion::V6 {
                                matches!(addr.ip(), IpAddr::V6(_))
                            } else {
                                matches!(addr.ip(), IpAddr::V4(_))
                            }
                        })
                        .collect()
                };
                if selected_ips.is_empty() {
                    return Err(PingCreationError::HostnameError {
                        hostname: domain.clone(),
                        err: std::io::Error::other("no IPs found"),
                    });
                }
                selected_ips[0].ip()
            }
        };

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let mut pinger = WinPinger::new().expect("Failed to create a WinPinger instance");
            pinger.set_timeout(TIMEOUT.as_millis() as u32);
            let mut buffer = Buffer::new();
            loop {
                let sent = Instant::now();
                let result = match pinger.send(parsed_ip, &mut buffer) {
                    Ok(rtt) => PingResult::Pong(Duration::from_millis(rtt as u64), "".to_string()),
                    Err(error) => {
                        // Every failure counts as a timeout. A real one is only known once
                        // IcmpSendEcho has waited for the reply, but others (no route, the
                        // network adapter down) come back at once: hold those back as long,
                        // so that every loss arrives as late as `timeout_delay` says.
                        if !matches!(error, winping::Error::Timeout) {
                            thread::sleep(TIMEOUT.saturating_sub(sent.elapsed()));
                        }
                        PingResult::Timeout("".to_string())
                    }
                };
                if tx.send(result).is_err() {
                    break;
                }
                thread::sleep(interval);
            }
        });

        Ok(rx)
    }
}

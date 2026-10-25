use std::io;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use crate::target::{IPVersion, Target};
use crate::{PingCreationError, PingMode, PingOptions, PingResult, Pinger, RstBehaviour};

pub struct TcpPinger {
    options: PingOptions,
    rst: RstBehaviour,
    port: u16,
}

impl TcpPinger {
    /// Resolve the target once, up front, so that a bad hostname surfaces as an error
    /// from `start` rather than as an endless stream of failed pings.
    fn resolve(&self) -> Result<SocketAddr, PingCreationError> {
        let hostname = self.options.target.to_string();
        // A (host, port) pair rather than "host:port", which is not an address at all
        // when the host is an IPv6 literal. The pair resolves IP literals as they are.
        let wanted = |addr: &SocketAddr| match &self.options.target {
            Target::Hostname {
                version: IPVersion::V4,
                ..
            } => addr.is_ipv4(),
            Target::Hostname {
                version: IPVersion::V6,
                ..
            } => addr.is_ipv6(),
            _ => true,
        };
        (hostname.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|err| PingCreationError::HostnameError {
                hostname: hostname.clone(),
                err,
            })?
            .find(wanted)
            .ok_or_else(|| PingCreationError::HostnameError {
                hostname,
                err: io::Error::other("name resolved to no addresses"),
            })
    }
}

impl Pinger for TcpPinger {
    fn from_options(options: PingOptions) -> Result<Self, PingCreationError> {
        match options.mode {
            PingMode::TCP { rst, port } => Ok(TcpPinger {
                rst,
                port: port.unwrap_or(80),
                options,
            }),
            PingMode::ICMP => Err(PingCreationError::InternalError(
                "ICMP ping options passed to TcpPinger".to_string(),
            )),
        }
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        |_| None // TCP doesn't parse lines
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        ("tcp", vec![]) // unused
    }

    fn start(&self) -> Result<mpsc::Receiver<PingResult>, PingCreationError> {
        let (tx, rx) = mpsc::channel();
        let addr = self.resolve()?;
        let interval = self.options.interval;
        let timeout = connect_timeout(interval);
        let rst = self.rst;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let hung_up = Arc::new(AtomicBool::new(false));

        // Each probe gets a thread of its own, so a slow or silent host doesn't slow the
        // cadence: probes go out every interval, the way ping sends its packets.
        thread::spawn(move || {
            while !hung_up.load(Ordering::Relaxed) {
                if in_flight.load(Ordering::Relaxed) < MAX_IN_FLIGHT {
                    in_flight.fetch_add(1, Ordering::Relaxed);
                    let tx = tx.clone();
                    let in_flight = Arc::clone(&in_flight);
                    let hung_up = Arc::clone(&hung_up);
                    thread::spawn(move || {
                        let result = probe(addr, timeout, rst);
                        // The receiver has hung up, so nothing will read any further pings.
                        if tx.send(result).is_err() {
                            hung_up.store(true, Ordering::Relaxed);
                        }
                        in_flight.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                thread::sleep(interval);
            }
        });

        Ok(rx)
    }
}

/// How long a connection attempt may take before it counts as lost. Not the interval: at the
/// default 0.2s, that made every host more than 200ms away a stream of timeouts.
pub(crate) fn connect_timeout(interval: Duration) -> Duration {
    interval.max(Duration::from_secs(2))
}

/// The most connection attempts in flight at once, so that a host that never answers can't
/// pile up thousands of them at a short interval.
const MAX_IN_FLIGHT: usize = 64;

fn probe(addr: SocketAddr, timeout: Duration, rst: RstBehaviour) -> PingResult {
    let start = Instant::now();
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => PingResult::Pong(start.elapsed(), addr.to_string()),
        // A RST means the host is up, it just isn't listening on this port
        Err(e) if rst == RstBehaviour::Pong && e.kind() == ErrorKind::ConnectionRefused => {
            PingResult::Pong(start.elapsed(), addr.to_string())
        }
        Err(_) => {
            // A loss is reported a fixed time after the probe went out, even when the
            // attempt failed early (no route, say), so that `timeout_delay` can place it.
            thread::sleep(timeout.saturating_sub(start.elapsed()));
            PingResult::Timeout(addr.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn the_timeout_is_not_the_interval() {
        assert_eq!(
            connect_timeout(Duration::from_millis(200)),
            Duration::from_secs(2)
        );
        assert_eq!(
            connect_timeout(Duration::from_secs(5)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn a_listening_port_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(matches!(
            probe(addr, Duration::from_secs(2), RstBehaviour::Drop),
            PingResult::Pong(..)
        ));
    }

    #[test]
    fn a_refused_connection_follows_the_rst_setting() {
        // Bind and drop, to find a port nothing is listening on.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(matches!(
            probe(addr, Duration::from_millis(300), RstBehaviour::Pong),
            PingResult::Pong(..)
        ));
        let started = Instant::now();
        assert!(matches!(
            probe(addr, Duration::from_millis(300), RstBehaviour::Drop),
            PingResult::Timeout(_)
        ));
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "reported after the timeout"
        );
    }

    // IPv6 literals used to be formatted as "::1:80", which is not an address at all.
    #[test]
    fn ipv6_literals_resolve() {
        let options =
            PingOptions::new("::1", Duration::from_secs(1), None).with_mode(PingMode::TCP {
                rst: RstBehaviour::Pong,
                port: Some(443),
            });
        let pinger = TcpPinger::from_options(options).unwrap();
        assert!(pinger.resolve().unwrap().is_ipv6());
    }
}

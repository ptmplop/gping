/// Pinger
/// This crate exposes a simple function to ping remote hosts across different operating systems.
/// Example:
/// ```no_run
/// use std::time::Duration;
/// use pinger::{ping, PingResult, PingOptions};
/// let options = PingOptions::new("tomforb.es".to_string(), Duration::from_secs(1), None);
/// let stream = ping(options).expect("Error pinging");
/// for message in stream {
///     match message {
///         PingResult::Pong(duration, line) => println!("{:?} (line: {})", duration, line),
///         PingResult::Timeout(_) => println!("Timeout!"),
///         PingResult::Unknown(line) => println!("Unknown line: {}", line),
///         PingResult::PingExited(_code, _stderr) => {}
///     }
/// }
/// ```
use lazy_regex::Regex;
use std::ffi::OsStr;
use std::fmt::{Debug, Formatter};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use std::{fmt, io, thread};
use target::Target;
use thiserror::Error;

#[cfg(unix)]
pub mod linux;
#[cfg(unix)]
pub mod macos;
#[cfg(windows)]
pub mod windows;

#[cfg(unix)]
mod bsd;
#[cfg(feature = "fake-ping")]
mod fake;
mod loss;
mod target;
#[cfg(test)]
mod test;

mod tcp;

/// What a connection refused (RST) means for a TCP ping.
///
/// An RST *is* a response: the host answered, it just isn't listening on that port,
/// and the time it took to arrive is a valid round-trip measurement. That differs
/// from a real timeout, where nothing came back at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RstBehaviour {
    /// Record the RST as a pong, with the round-trip time it took to arrive.
    #[default]
    Pong,
    /// Treat the RST as a failed ping, the same as no response at all. Useful when
    /// you care whether a service is actually listening, not whether the host is up.
    Drop,
}

/// How to probe the target. The TCP options only apply to TCP pings, so they live
/// inside that variant rather than sitting unused alongside an ICMP ping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PingMode {
    #[default]
    ICMP,
    TCP {
        /// How to treat a connection refused; see [`RstBehaviour`].
        rst: RstBehaviour,
        /// Port to connect to; defaults to 80 when unset.
        port: Option<u16>,
    },
}

#[derive(Debug, Clone)]
pub struct PingOptions {
    pub target: Target,
    pub interval: Duration,
    pub interface: Option<String>,
    pub raw_arguments: Option<Vec<String>>,
    pub mode: PingMode,
}

impl PingOptions {
    pub fn with_raw_arguments(mut self, raw_arguments: Vec<impl ToString>) -> Self {
        self.raw_arguments = Some(
            raw_arguments
                .into_iter()
                .map(|item| item.to_string())
                .collect(),
        );
        self
    }
}

impl PingOptions {
    pub fn from_target(target: Target, interval: Duration, interface: Option<String>) -> Self {
        Self {
            target,
            interval,
            interface,
            raw_arguments: None,
            mode: PingMode::default(),
        }
    }
    pub fn new(target: impl ToString, interval: Duration, interface: Option<String>) -> Self {
        Self::from_target(Target::new_any(target), interval, interface)
    }

    pub fn new_ipv4(target: impl ToString, interval: Duration, interface: Option<String>) -> Self {
        Self::from_target(Target::new_ipv4(target), interval, interface)
    }

    pub fn new_ipv6(target: impl ToString, interval: Duration, interface: Option<String>) -> Self {
        Self::from_target(Target::new_ipv6(target), interval, interface)
    }
    /// Select how the target is probed; see [`PingMode`].
    pub fn with_mode(mut self, mode: PingMode) -> Self {
        self.mode = mode;
        self
    }
}

pub fn run_ping(
    cmd: impl AsRef<OsStr> + Debug,
    args: Vec<impl AsRef<OsStr> + Debug>,
) -> Result<Child, PingCreationError> {
    Ok(Command::new(cmd.as_ref())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Required to ensure that the output is formatted in the way we expect, not
        // using locale specific delimiters.
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .spawn()?)
}

pub(crate) fn extract_regex(regex: &Regex, line: String) -> Option<PingResult> {
    let cap = regex.captures(&line)?;
    let ms = cap
        .name("ms")
        .expect("No capture group named 'ms'")
        .as_str()
        .parse::<u64>()
        .ok()?;
    let ns = match cap.name("ns") {
        None => 0,
        Some(cap) => {
            let matched_str = cap.as_str();
            let number_of_digits = matched_str.len() as u32;
            let fractional_ms = matched_str.parse::<u64>().ok()?;
            fractional_ms * (10u64.pow(6 - number_of_digits))
        }
    };
    let duration = Duration::from_millis(ms) + Duration::from_nanos(ns);
    Some(PingResult::Pong(duration, line))
}

pub trait Pinger: Send + Sync {
    fn from_options(options: PingOptions) -> std::result::Result<Self, PingCreationError>
    where
        Self: Sized;

    fn parse_fn(&self) -> fn(String) -> Option<PingResult>;

    fn ping_args(&self) -> (&str, Vec<String>);

    fn start(&self) -> Result<mpsc::Receiver<PingResult>, PingCreationError> {
        let (tx, rx) = mpsc::channel();
        let (cmd, args) = self.ping_args();

        let mut child = run_ping(cmd, args)?;
        let stdout = child.stdout.take().expect("child did not have a stdout");
        let stderr = child.stderr.take().expect("child did not have a stderr");

        // ping writes to stderr for every packet it fails to send ("ping: sendto: No
        // route to host" on macOS and the BSDs, for as long as an outage lasts). The pipe
        // only holds 64KiB: unless something keeps reading it, ping ends up blocked in
        // write(2) for good and no further results arrive, even once the network is back.
        let stderr_reader = thread::spawn(move || read_stderr_tail(stderr));

        let parse_fn = self.parse_fn();
        let (line_tx, line_rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(result) = parse_fn(line) {
                    if line_tx.send(result).is_err() {
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            // A timeout reported by ping is only provisional: see `loss`.
            let mut tracker = loss::LossTracker::default();
            loop {
                let received = match tracker.next_deadline() {
                    Some(deadline) => {
                        line_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    }
                    None => line_rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                let now = Instant::now();
                let mut results = match received {
                    Ok(result) => tracker.on_result(result, now),
                    Err(RecvTimeoutError::Timeout) => vec![],
                    // stdout is closed: ping has exited, or is about to.
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                results.extend(tracker.expire(now));
                for result in results {
                    if tx.send(result).is_err() {
                        // Nobody is listening any more, so stop pinging.
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
            }
            for result in tracker.finish() {
                let _ = tx.send(result);
            }
            let status = child.wait().expect("Child wasn't started?");
            let stderr = stderr_reader.join().unwrap_or_default();
            let _ = tx.send(PingResult::PingExited(status, stderr));
        });

        Ok(rx)
    }
}

/// Reads ping's stderr until it closes, keeping the end of it for `PingExited`.
fn read_stderr_tail(mut stderr: ChildStderr) -> String {
    const KEEP: usize = 4096;
    let mut kept = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                kept.extend_from_slice(&buf[..n]);
                if kept.len() > 2 * KEEP {
                    kept.drain(..kept.len() - KEEP);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if kept.len() > KEEP {
        kept.drain(..kept.len() - KEEP);
    }
    // ping echoes things like the hostname back; never panic over its encoding.
    String::from_utf8_lossy(&kept).into_owned()
}

/// The `-i` argument for ping: the interval in seconds, to the millisecond. ping takes
/// fractional seconds, and rounding to a tenth turned `-n 0.25` into 0.2s and
/// `-n 0.04` into `-i0.0`, which iputils refuses for users and floods with for root.
#[cfg(unix)]
pub(crate) fn interval_arg(interval: Duration) -> String {
    format!("-i{}.{:03}", interval.as_secs(), interval.subsec_millis())
}

#[derive(Debug)]
pub enum PingResult {
    Pong(Duration, String),
    Timeout(String),
    Unknown(String),
    PingExited(ExitStatus, String),
}

impl fmt::Display for PingResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self {
            PingResult::Pong(duration, _) => write!(f, "{duration:?}"),
            PingResult::Timeout(_) => write!(f, "Timeout"),
            PingResult::Unknown(_) => write!(f, "Unknown"),
            PingResult::PingExited(status, stderr) => write!(f, "Exited({status}, {stderr})"),
        }
    }
}

#[derive(Error, Debug)]
pub enum PingCreationError {
    #[error("Could not detect ping. Stderr: {stderr:?}\nStdout: {stdout:?}")]
    UnknownPing {
        stderr: Vec<String>,
        stdout: Vec<String>,
    },
    #[error("Error spawning ping: {0}")]
    SpawnError(#[from] io::Error),

    #[error("Installed ping is not supported: {alternative}")]
    NotSupported { alternative: String },

    #[error("Invalid or unresolvable hostname {hostname}: {err}")]
    HostnameError {
        hostname: String,
        // Not #[from]: SpawnError already provides the From<io::Error> impl, and two
        // of them on one enum would collide.
        #[source]
        err: io::Error,
    },

    #[error("Internal error: {0}")]
    InternalError(String),
}

/// The shortest interval a pinger will use.
pub const MIN_INTERVAL: Duration = Duration::from_millis(1);

/// How long after a ping was lost the `Timeout` for it is reported. A missing reply can only
/// be called lost once its packet has had time to be answered, so it is reported this much
/// later than it happened; callers that plot results against time can move it back.
pub fn timeout_delay(options: &PingOptions) -> Duration {
    if fake_ping_requested() {
        return Duration::ZERO;
    }
    match options.mode {
        PingMode::TCP { .. } => tcp::connect_timeout(options.interval),
        // ping reports the timeout, which is then held back for `LATE_REPLY_GRACE`.
        #[cfg(unix)]
        PingMode::ICMP => loss::LATE_REPLY_GRACE,
        // IcmpSendEcho gives up after winping's default timeout.
        #[cfg(windows)]
        PingMode::ICMP => Duration::from_secs(2),
    }
}

fn fake_ping_requested() -> bool {
    cfg!(feature = "fake-ping")
        && std::env::var("PINGER_FAKE_PING")
            .map(|e| e == "1")
            .unwrap_or_default()
}

pub fn get_pinger(
    mut options: PingOptions,
) -> std::result::Result<Arc<dyn Pinger>, PingCreationError> {
    // A shorter interval would reach ping as `-i0.000`, a flood ping for root, and would
    // make the TCP pinger spin: `connect_timeout` refuses a zero timeout, and it sleeps zero
    // between tries.
    options.interval = options.interval.max(MIN_INTERVAL);

    #[cfg(feature = "fake-ping")]
    if fake_ping_requested() {
        return Ok(Arc::new(fake::FakePinger::from_options(options)?));
    }

    if matches!(options.mode, PingMode::TCP { .. }) {
        return Ok(Arc::new(crate::tcp::TcpPinger::from_options(options)?));
    }

    #[cfg(windows)]
    {
        return Ok(Arc::new(windows::WindowsPinger::from_options(options)?));
    }
    #[cfg(unix)]
    {
        if cfg!(target_os = "freebsd")
            || cfg!(target_os = "dragonfly")
            || cfg!(target_os = "openbsd")
            || cfg!(target_os = "netbsd")
        {
            Ok(Arc::new(bsd::BSDPinger::from_options(options)?))
        } else if cfg!(target_os = "macos") {
            Ok(Arc::new(macos::MacOSPinger::from_options(options)?))
        } else {
            Ok(Arc::new(linux::LinuxPinger::from_options(options)?))
        }
    }
}

/// Start pinging a an address. The address can be either a hostname or an IP address.
pub fn ping(
    options: PingOptions,
) -> std::result::Result<mpsc::Receiver<PingResult>, PingCreationError> {
    let pinger = get_pinger(options)?;
    pinger.start()
}

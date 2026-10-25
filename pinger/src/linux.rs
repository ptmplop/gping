use crate::{
    bsd_style_schedule, extract_regex, interval_arg, run_ping, PingCreationError, PingOptions,
    PingResult, Pinger, Schedule,
};
use lazy_regex::*;

pub static UBUNTU_RE: Lazy<Regex> = lazy_regex!(r"(?i-u)time=(?P<ms>\d+)(?:\.(?P<ns>\d+))? *ms");

#[derive(Debug)]
pub enum LinuxPinger {
    // Alpine
    BusyBox(PingOptions),
    // Debian, Ubuntu, etc
    IPTools(PingOptions),
}

impl LinuxPinger {
    pub fn detect_platform_ping(options: PingOptions) -> Result<Self, PingCreationError> {
        let child = run_ping("ping", vec!["-V".to_string()])?;
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if stderr.contains("BusyBox") {
            Ok(LinuxPinger::BusyBox(options))
        } else if stdout.contains("iputils") {
            Ok(LinuxPinger::IPTools(options))
        } else if stdout.contains("inetutils") {
            Err(PingCreationError::NotSupported {
                alternative: "Please use iputils ping, not inetutils.".to_string(),
            })
        } else {
            let first_two_lines_stderr: Vec<String> =
                stderr.lines().take(2).map(str::to_string).collect();
            let first_two_lines_stout: Vec<String> =
                stdout.lines().take(2).map(str::to_string).collect();
            Err(PingCreationError::UnknownPing {
                stdout: first_two_lines_stout,
                stderr: first_two_lines_stderr,
            })
        }
    }
}

impl Pinger for LinuxPinger {
    fn from_options(options: PingOptions) -> Result<Self, PingCreationError>
    where
        Self: Sized,
    {
        Self::detect_platform_ping(options)
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        |line| {
            #[cfg(test)]
            eprintln!("Got line {line}");
            if line.contains("bytes from") {
                return extract_regex(&UBUNTU_RE, line);
            } else if line.contains("no answer yet") {
                // Not `starts_with`: with `-D` the line starts with a timestamp.
                return Some(PingResult::Timeout(line));
            }
            None
        }
    }

    // BusyBox says nothing about a lost packet; iputils reports each one (`-O`).
    fn schedule(&self) -> Option<Schedule> {
        match self {
            LinuxPinger::BusyBox(options) => bsd_style_schedule(options),
            LinuxPinger::IPTools(_) => None,
        }
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        let (LinuxPinger::BusyBox(options) | LinuxPinger::IPTools(options)) = self;
        let mut args = vec![];
        // Alpine doesn't support timeout notifications, so we don't add the -O flag there.
        if let LinuxPinger::IPTools(_) = self {
            // The -O flag ensures we "no answer yet" messages from ping
            // See https://superuser.com/questions/270083/linux-ping-show-time-out
            args.push("-O".to_string());
        }
        args.push(interval_arg(options.interval));

        // Without either flag ping uses whichever address the resolver lists first,
        // which on a dual-stack host is usually IPv6, whatever -4 asked for.
        let mut cmd = "ping";
        if options.target.is_ipv6() {
            if ping_understands_dash_6() {
                args.push("-6".to_string());
            } else {
                cmd = "ping6";
            }
        } else if options.target.is_ipv4_hostname() {
            args.push("-4".to_string());
        }

        // BusyBox takes -I too (an interface name or a source address).
        if let Some(interface) = &options.interface {
            args.push("-I".into());
            args.push(interface.clone());
        }
        if let Some(raw_args) = &options.raw_arguments {
            args.extend(raw_args.iter().cloned());
        }

        // Last, after every option: a getopt that does not permute stops at the first
        // operand and would take everything after it for more operands.
        args.push(options.target.to_string());
        (cmd, args)
    }
}

/// Whether `ping` takes -6. Android still ships iputils s20121221, from before ping and
/// ping6 were merged: its ping is IPv4 only and rejects -6, and IPv6 needs ping6.
#[cfg(target_os = "android")]
fn ping_understands_dash_6() -> bool {
    static ANSWER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| {
        run_ping("ping", vec!["-6", "-V"])
            .and_then(|child| Ok(child.wait_with_output()?))
            .map(|output| output.status.success())
            .unwrap_or(true)
    })
}

#[cfg(not(target_os = "android"))]
fn ping_understands_dash_6() -> bool {
    true
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "linux")]
    fn test_linux_detection() {
        use super::*;
        use os_info::Type;
        use std::time::Duration;

        let platform = LinuxPinger::detect_platform_ping(PingOptions::new(
            "foo.com".to_string(),
            Duration::from_secs(1),
            None,
        ))
        .unwrap();
        match os_info::get().os_type() {
            Type::Alpine => {
                assert!(matches!(platform, LinuxPinger::BusyBox(_)))
            }
            Type::Ubuntu => {
                assert!(matches!(platform, LinuxPinger::IPTools(_)))
            }
            _ => {}
        }
    }
}

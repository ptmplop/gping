//! Command line arguments.

use clap::Parser;

/// Seconds between pings when -n isn't given. FreeBSD and its relatives (and OpenBSD) only
/// let root ping faster than once a second.
#[cfg(any(
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd"
))]
pub const DEFAULT_PING_INTERVAL_SECONDS: f64 = 1.0;
#[cfg(not(any(
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
pub const DEFAULT_PING_INTERVAL_SECONDS: f64 = 0.2;
pub const DEFAULT_CMD_INTERVAL_SECONDS: f64 = 0.5;

/// The longest history -b accepts: a week.
pub const MAX_BUFFER_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Parser, Debug)]
#[command(author, version, name = "gping", about = "Ping, but with a graph.", styles = clap_cargo::style::CLAP_STYLING)]
pub struct Args {
    /// Graph the execution time for a list of commands rather than pinging hosts
    #[arg(long)]
    pub cmd: bool,

    /// Watch interval seconds (provide partial seconds like '0.5'). Default for ping is 0.2 (1.0 on the BSDs), default for cmd is 0.5.
    #[arg(short = 'n', long, value_parser = parse_interval)]
    pub watch_interval: Option<f64>,

    /// Hosts or IPs to ping, or commands to run if --cmd is provided. Can use cloud shorthands like aws:eu-west-1.
    #[arg(allow_hyphen_values = false)]
    pub hosts_or_commands: Vec<String>,

    /// Determines the number of seconds to display in the graph.
    #[arg(short, long, default_value = "30", value_parser = clap::value_parser!(u64).range(1..=MAX_BUFFER_SECONDS))]
    pub buffer: u64,

    /// Resolve ping targets to IPv4 address
    #[arg(short = '4', conflicts_with = "ipv6")]
    pub ipv4: bool,

    /// Resolve ping targets to IPv6 address
    #[arg(short = '6', conflicts_with = "ipv4")]
    pub ipv6: bool,

    #[cfg(not(target_os = "windows"))]
    /// Interface to use when pinging.
    #[arg(short = 'i', long, conflicts_with = "tcp")]
    pub interface: Option<String>,

    /// Uses dot characters instead of braille
    #[arg(short = 's', long)]
    pub simple_graphics: bool,

    /// Vertical margin around the graph (top and bottom)
    #[arg(long, default_value = "1")]
    pub vertical_margin: u16,

    /// Horizontal margin around the graph (left and right)
    #[arg(long, default_value = "0")]
    pub horizontal_margin: u16,

    /// set y-axis minimum
    #[arg(long, help = "Set vertical axis min (ms)")]
    pub ymin: Option<u64>,

    /// set y-axis maximum
    #[arg(long, help = "Set vertical axis max (ms)")]
    pub ymax: Option<u64>,

    /// set y-axis minimum to zero
    #[arg(short = '0', help = "Equivalent to --ymin=0", conflicts_with = "ymin")]
    pub ymin_zero: bool,

    /// Use TCP pings instead of ICMP
    #[arg(long, default_value_t = false)]
    pub tcp: bool,

    /// How to treat a connection refused (RST) when TCP pinging
    #[arg(long, value_enum, default_value_t = RstBehaviour::Pong, requires = "tcp")]
    pub tcp_rst: RstBehaviour,

    /// Port to connect to (only used for TCP pings)
    #[arg(long, default_value_t = 80, requires = "tcp")]
    pub tcp_port: u16,

    #[arg(
        name = "color",
        short = 'c',
        long = "color",
        use_value_delimiter = true,
        value_delimiter = ',',
        help = r#"Assign color to a graph entry.

This option can be defined more than once as a comma separated string, and the
order which the colors are provided will be matched against the hosts or
commands passed to gping.

Hexadecimal RGB color codes are accepted in the form of '#RRGGBB' or the
following color names: 'black', 'red', 'green', 'yellow', 'blue', 'magenta',
'cyan', 'gray', 'dark-gray', 'light-red', 'light-green', 'light-yellow',
'light-blue', 'light-magenta', 'light-cyan', and 'white'"#
    )]
    pub color_codes_or_names: Vec<String>,

    /// Clear the graph from the terminal after closing the program
    #[arg(name = "clear", long = "clear", action)]
    pub clear: bool,

    #[cfg(not(target_os = "windows"))]
    /// Extra arguments to pass to `ping`. These are platform dependent. Everything after
    /// --ping-args is passed on, so it has to come after the hosts.
    #[arg(long, allow_hyphen_values = true, num_args = 0.., conflicts_with_all = ["cmd", "tcp"])]
    pub ping_args: Option<Vec<String>>,
}

/// Mirrors [`pinger::RstBehaviour`] so the choice can be parsed by clap without the
/// pinger crate having to depend on it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum RstBehaviour {
    /// Count it as a successful ping: the host answered, it just isn't listening
    Pong,
    /// Count it as a failed ping
    Drop,
}

impl From<RstBehaviour> for pinger::RstBehaviour {
    fn from(value: RstBehaviour) -> Self {
        match value {
            RstBehaviour::Pong => pinger::RstBehaviour::Pong,
            RstBehaviour::Drop => pinger::RstBehaviour::Drop,
        }
    }
}

fn parse_interval(value: &str) -> Result<f64, String> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| format!("'{value}' is not a number of seconds"))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(format!("the interval must be more than 0 seconds, not '{value}'"));
    }
    if seconds > 3_600.0 {
        return Err(format!("the interval can be at most an hour, not '{value}'"));
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_definition_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn intervals() {
        assert_eq!(parse_interval("0.25"), Ok(0.25));
        assert_eq!(parse_interval("2"), Ok(2.0));
        for bad in ["0", "-1", "nan", "inf", "abc", "7200"] {
            assert!(parse_interval(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn buffer_must_be_positive_and_bounded() {
        assert!(Args::try_parse_from(["gping", "-b", "0", "host"]).is_err());
        assert!(Args::try_parse_from(["gping", "-b", "10000000000000", "host"]).is_err());
        assert_eq!(Args::try_parse_from(["gping", "-b", "60", "host"]).unwrap().buffer, 60);
    }

    // The flag used to have `help = ""`, which blanked its description in --help and the
    // man page.
    #[test]
    fn simple_graphics_is_documented() {
        let command = Args::command();
        let arg = command
            .get_arguments()
            .find(|a| a.get_id() == "simple_graphics")
            .unwrap();
        assert!(arg.get_help().is_some_and(|h| !h.to_string().is_empty()));
    }
}

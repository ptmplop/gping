use crate::bsd::parse_bsd;
use crate::{interval_arg, PingCreationError, PingOptions, PingResult, Pinger};
use lazy_regex::*;

pub static RE: Lazy<Regex> = lazy_regex!(r"time=(?:(?P<ms>[0-9]+).(?P<ns>[0-9]+)\s+ms)");

pub struct MacOSPinger {
    options: PingOptions,
}

impl Pinger for MacOSPinger {
    fn from_options(options: PingOptions) -> Result<Self, PingCreationError>
    where
        Self: Sized,
    {
        Ok(Self { options })
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        parse_bsd
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        let ipv6 = self.options.target.is_ipv6();
        let cmd = if ipv6 { "ping6" } else { "ping" };
        let mut args = vec![interval_arg(self.options.interval)];
        if let Some(interface) = &self.options.interface {
            // ping binds to an interface with -b, ping6 with -B: its -b is the socket
            // buffer size, and "-b en0" makes it exit with "invalid socket buffer size".
            args.push(if ipv6 { "-B" } else { "-b" }.into());
            args.push(interface.clone());
        }

        if let Some(raw_args) = &self.options.raw_arguments {
            args.extend(raw_args.iter().cloned());
        }

        args.push(self.options.target.to_string());
        (cmd, args)
    }
}

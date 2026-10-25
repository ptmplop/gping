#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use crate::bsd::BSDPinger;
    #[cfg(unix)]
    use crate::linux::LinuxPinger;
    #[cfg(unix)]
    use crate::macos::MacOSPinger;
    #[cfg(windows)]
    use crate::windows::WindowsPinger;
    use crate::{PingOptions, PingResult, Pinger};
    use anyhow::bail;
    use ntest::timeout;
    use std::time::Duration;

    const IS_GHA: bool = option_env!("GITHUB_ACTIONS").is_some();

    #[test]
    #[timeout(20_000)]
    fn test_integration_any() {
        run_integration_test(PingOptions::new(
            "tomforb.es",
            Duration::from_millis(500),
            None,
        ))
        .unwrap();
    }
    #[test]
    #[timeout(20_000)]
    fn test_integration_ipv4() {
        run_integration_test(PingOptions::new_ipv4(
            "tomforb.es",
            Duration::from_millis(500),
            None,
        ))
        .unwrap();
    }
    #[test]
    #[timeout(20_000)]
    fn test_integration_ip6() {
        let res = run_integration_test(PingOptions::new_ipv6(
            "tomforb.es",
            Duration::from_millis(500),
            None,
        ));
        // ipv6 tests are allowed to fail on Gitlab CI, as it doesn't support ipv6, apparently.
        if !IS_GHA {
            res.unwrap();
        }
    }

    fn run_integration_test(options: PingOptions) -> anyhow::Result<()> {
        let stream = crate::ping(options.clone())?;

        let mut success = 0;
        let mut errors = 0;

        for message in stream.into_iter().take(3) {
            match message {
                PingResult::Pong(_, m) | PingResult::Timeout(m) => {
                    eprintln!("Message: {}", m);
                    success += 1;
                }
                PingResult::Unknown(line) => {
                    eprintln!("Unknown line: {}", line);
                    errors += 1;
                }
                PingResult::PingExited(code, stderr) => {
                    bail!("Ping exited with code: {}, stderr: {}", code, stderr);
                }
            }
        }
        assert_eq!(success, 3, "Success != 3 with opts {options:?}");
        assert_eq!(errors, 0, "Errors != 0 with opts {options:?}");
        Ok(())
    }

    fn opts() -> PingOptions {
        PingOptions::new("foo".to_string(), Duration::from_secs(1), None)
    }

    fn test_parser<T: Pinger>(contents: &str) {
        let pinger = T::from_options(opts()).unwrap();
        run_parser_test(contents, &pinger);
    }

    fn run_parser_test(contents: &str, pinger: &impl Pinger) {
        let parser = pinger.parse_fn();
        let test_file: Vec<&str> = contents.split("-----").collect();
        let input = test_file[0].trim().split('\n');
        let expected: Vec<&str> = test_file[1].trim().split('\n').collect();
        let parsed: Vec<Option<PingResult>> = input.map(|l| parser(l.to_string())).collect();

        assert_eq!(
            parsed.len(),
            expected.len(),
            "Parsed: {:?}, Expected: {:?}",
            parsed,
            expected
        );

        for (idx, (output, expected)) in parsed.into_iter().zip(expected).enumerate() {
            if let Some(value) = output {
                assert_eq!(
                    format!("{value}").trim(),
                    expected.trim(),
                    "Failed at idx {idx}"
                )
            } else {
                assert_eq!("None", expected.trim(), "Failed at idx {idx}")
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn macos() {
        test_parser::<MacOSPinger>(include_str!("tests/macos.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn freebsd() {
        test_parser::<BSDPinger>(include_str!("tests/bsd.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn dragonfly() {
        test_parser::<BSDPinger>(include_str!("tests/bsd.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn openbsd() {
        test_parser::<BSDPinger>(include_str!("tests/bsd.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn netbsd() {
        test_parser::<BSDPinger>(include_str!("tests/bsd.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn ubuntu() {
        run_parser_test(
            include_str!("tests/ubuntu.txt"),
            &LinuxPinger::IPTools(opts()),
        );
    }

    #[cfg(unix)]
    #[test]
    fn debian() {
        run_parser_test(
            include_str!("tests/debian.txt"),
            &LinuxPinger::IPTools(opts()),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows() {
        test_parser::<WindowsPinger>(include_str!("tests/windows.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn android() {
        run_parser_test(
            include_str!("tests/android.txt"),
            &LinuxPinger::BusyBox(opts()),
        );
    }

    #[cfg(unix)]
    fn argv(pinger: &(impl Pinger + ?Sized)) -> String {
        let (cmd, args) = pinger.ping_args();
        format!("{cmd} {}", args.join(" "))
    }

    // The interval goes to ping to the millisecond, not rounded to a tenth of a second.
    #[cfg(unix)]
    #[test]
    fn interval_is_passed_to_the_millisecond() {
        for (ms, arg) in [
            (250, "-i0.250"),
            (40, "-i0.040"),
            (1250, "-i1.250"),
            (200, "-i0.200"),
        ] {
            let options = PingOptions::new("192.0.2.1", Duration::from_millis(ms), None);
            assert!(argv(&LinuxPinger::IPTools(options.clone())).contains(arg));
            assert!(argv(&LinuxPinger::BusyBox(options.clone())).contains(arg));
            assert!(argv(&MacOSPinger::from_options(options.clone()).unwrap()).contains(arg));
            assert!(argv(&BSDPinger::from_options(options).unwrap()).contains(arg));
        }
    }

    // A zero interval would reach ping as `-i0.000`: a flood ping when run as root.
    #[cfg(unix)]
    #[test]
    fn a_zero_interval_is_raised_to_the_minimum() {
        let options = PingOptions::new("192.0.2.1", Duration::ZERO, None);
        let pinger = crate::get_pinger(options).unwrap();
        assert!(argv(pinger.as_ref()).contains("-i0.001"));
    }

    // Without -4, iputils and BusyBox ping whichever address the resolver lists first.
    #[cfg(unix)]
    #[test]
    fn linux_honours_the_requested_address_family() {
        let v4 = PingOptions::new_ipv4("example.com", Duration::from_secs(1), None);
        let v6 = PingOptions::new_ipv6("example.com", Duration::from_secs(1), None);
        let any = PingOptions::new("example.com", Duration::from_secs(1), None);
        assert_eq!(
            argv(&LinuxPinger::IPTools(v4.clone())),
            "ping -O -i1.000 -4 example.com"
        );
        assert_eq!(
            argv(&LinuxPinger::IPTools(v6.clone())),
            "ping -O -i1.000 -6 example.com"
        );
        assert_eq!(
            argv(&LinuxPinger::IPTools(any.clone())),
            "ping -O -i1.000 example.com"
        );
        assert_eq!(
            argv(&LinuxPinger::BusyBox(v4)),
            "ping -i1.000 -4 example.com"
        );
        assert_eq!(
            argv(&LinuxPinger::BusyBox(v6)),
            "ping -i1.000 -6 example.com"
        );
        assert_eq!(argv(&LinuxPinger::BusyBox(any)), "ping -i1.000 example.com");
        // An address is pinged over its own family without being told.
        let literal = PingOptions::new_ipv4("192.0.2.1", Duration::from_secs(1), None);
        assert_eq!(
            argv(&LinuxPinger::IPTools(literal)),
            "ping -O -i1.000 192.0.2.1"
        );
    }

    #[cfg(unix)]
    #[test]
    fn busybox_passes_the_interface() {
        let options = PingOptions::new("example.com", Duration::from_secs(1), Some("eth1".into()));
        assert_eq!(
            argv(&LinuxPinger::BusyBox(options)),
            "ping -i1.000 -I eth1 example.com"
        );
    }

    // ping6's -b is the socket buffer size; it binds to an interface with -B.
    #[cfg(unix)]
    #[test]
    fn macos_binds_ping6_with_capital_b() {
        let v6 = PingOptions::new("2001:db8::1", Duration::from_secs(1), Some("en0".into()));
        let v4 = PingOptions::new("192.0.2.1", Duration::from_secs(1), Some("en0".into()));
        assert_eq!(
            argv(&MacOSPinger::from_options(v6).unwrap()),
            "ping6 -i1.000 -B en0 2001:db8::1"
        );
        assert_eq!(
            argv(&MacOSPinger::from_options(v4).unwrap()),
            "ping -i1.000 -b en0 192.0.2.1"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bsd_uses_ping6_for_ipv6() {
        let v6 = PingOptions::new("2001:db8::1", Duration::from_secs(1), None);
        assert_eq!(
            argv(&BSDPinger::from_options(v6).unwrap()),
            "ping6 -i1.000 2001:db8::1"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bsd_takes_a_source_address_not_an_interface_name() {
        let name = PingOptions::new("192.0.2.1", Duration::from_secs(1), Some("em0".into()));
        assert!(matches!(
            BSDPinger::from_options(name),
            Err(crate::PingCreationError::NotSupported { .. })
        ));
        let address = PingOptions::new(
            "192.0.2.1",
            Duration::from_secs(1),
            Some("192.0.2.9".into()),
        );
        assert!(argv(&BSDPinger::from_options(address).unwrap()).contains(" 192.0.2.9 192.0.2.1"));
    }

    // `-D` puts a timestamp in front of "no answer yet".
    #[cfg(unix)]
    #[test]
    fn linux_timeouts_with_timestamps() {
        let parse = LinuxPinger::IPTools(opts()).parse_fn();
        assert!(matches!(
            parse("[1792934550.974468] no answer yet for icmp_seq=1".to_string()),
            Some(PingResult::Timeout(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn alpine() {
        run_parser_test(
            include_str!("tests/alpine.txt"),
            &LinuxPinger::BusyBox(opts()),
        );
    }
}

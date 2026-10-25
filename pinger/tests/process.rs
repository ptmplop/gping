//! How the output of a ping process is read, with a shell script standing in for ping.
#![cfg(unix)]

use ntest::timeout;
use pinger::{PingCreationError, PingOptions, PingResult, Pinger, Schedule};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Script {
    script: String,
    schedule: Option<Schedule>,
}

impl Pinger for Script {
    fn from_options(_: PingOptions) -> Result<Self, PingCreationError> {
        unimplemented!()
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        |_| None
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        ("sh", vec!["-c".to_string(), self.script.clone()])
    }

    fn schedule(&self) -> Option<Schedule> {
        self.schedule
    }
}

fn every(millis: u64) -> Option<Schedule> {
    Some(Schedule {
        first: 0,
        interval: Duration::from_millis(millis),
    })
}

fn is_running(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

// With its schedule known, a ping that never says a packet was lost still loses one per
// interval, from the moment the first is overdue. And once nobody is reading those, the
// next one stops it.
#[test]
#[timeout(10_000)]
fn a_silent_ping_reports_overdue_packets() {
    let pid_file = std::env::temp_dir().join(format!("pinger-silent-{}", std::process::id()));
    let script = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
    let pinger = Script {
        script,
        schedule: every(100),
    };
    let started = Instant::now();
    let results = pinger.start().unwrap();
    for _ in 0..3 {
        assert!(matches!(results.recv().unwrap(), PingResult::Timeout(_)));
    }
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(2), "{:?}", elapsed);
    assert!(elapsed < Duration::from_secs(3), "{:?}", elapsed);

    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let _ = std::fs::remove_file(&pid_file);
    let pid = pid.trim();
    assert!(is_running(pid));
    drop(results);
    let deadline = Instant::now() + Duration::from_secs(2);
    while is_running(pid) {
        assert!(Instant::now() < deadline, "ping was left running");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// `-c 2` to a dead host: the last packet is only accounted for by ping's statistics.
#[test]
#[timeout(10_000)]
fn closing_statistics_account_for_unmentioned_losses() {
    let pinger = Script {
        script: "echo '2 packets transmitted, 0 packets received, 100.0% packet loss'".to_string(),
        schedule: every(1000),
    };
    let results: Vec<String> = pinger
        .start()
        .unwrap()
        .iter()
        .map(|r| r.to_string())
        .collect();
    assert_eq!(results[..2], ["Timeout", "Timeout"]);
    assert!(
        results[2].starts_with("Exited(exit status: 0"),
        "{:?}",
        results
    );
    assert_eq!(results.len(), 3);
}

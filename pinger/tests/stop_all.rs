//! `stop_all`, in a test binary of its own because it ends every ping in the process.
#![cfg(unix)]

use ntest::timeout;
use pinger::{PingCreationError, PingOptions, PingResult, Pinger};

struct Silent;

impl Pinger for Silent {
    fn from_options(_: PingOptions) -> Result<Self, PingCreationError> {
        unimplemented!()
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        |_| None
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        ("sh", vec!["-c".to_string(), "exec sleep 30".to_string()])
    }
}

// A ping that prints nothing, as some do for a host that never answers, would never find
// out that nobody is reading its output.
#[test]
#[timeout(10_000)]
fn stop_all_ends_a_ping_that_prints_nothing() {
    let results = Silent.start().unwrap();
    pinger::stop_all();
    match results.recv().unwrap() {
        PingResult::PingExited(status, _) => assert!(!status.success()),
        other => panic!("expected ping to have been stopped, got {}", other),
    }
}

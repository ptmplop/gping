//! Decides which packets were really lost, from what `ping` prints.
//!
//! What ping says about a missing reply is not a verdict on that packet:
//!
//! * iputils' `-O` prints "no answer yet for icmp_seq=N" one interval after sending N,
//!   so with the default 0.2s interval every packet to a host more than 200ms away is
//!   reported as unanswered, and then answered a moment later.
//! * macOS prints "Request timeout for icmp_seq N" only when the number of unanswered
//!   packets reaches a new high. That fires for slow replies, and stays quiet for real
//!   losses once the high-water mark has been reached.
//! * BusyBox, the BSDs and macOS's `ping6` print nothing at all for a lost packet.
//!
//! So a reported timeout is held back for [`LATE_REPLY_GRACE`] and withdrawn if the
//! reply turns up in the meantime, and a packet that a later reply has overtaken is
//! treated the same way, which is how loss gets noticed where ping says nothing.

use crate::PingResult;
use lazy_regex::*;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a packet reported as unanswered may still be answered before it is
/// counted as lost.
pub(crate) const LATE_REPLY_GRACE: Duration = Duration::from_secs(2);

/// How long an answered or lost packet is remembered, so that a duplicate reply, or a
/// reply that only turns up after the packet was counted as lost, is not counted again.
const RESOLVED_MEMORY: Duration = Duration::from_secs(60);
/// Kept well below the 65536 sequence numbers before they wrap around.
const RESOLVED_CAPACITY: usize = 4096;

/// A bigger jump in sequence numbers than this is ping starting over, not lost packets.
const MAX_GAP: u16 = 1024;

/// "icmp_seq=12" (replies everywhere, iputils timeouts), "icmp_seq 12" (macOS timeouts),
/// "seq=12" (BusyBox replies).
static SEQ_RE: Lazy<Regex> = lazy_regex!(r"(?-u)(?:icmp_)?seq[= ](\d+)");

/// The packet a line is about. Sequence numbers are 16 bits on the wire, but macOS
/// prints its own unwrapped counter in "Request timeout" lines, so both are reduced
/// to 16 bits to compare them.
fn seq_of(line: &str) -> Option<u16> {
    let digits = SEQ_RE.captures(line)?.get(1)?.as_str();
    Some(digits.parse::<u64>().ok()? as u16)
}

#[derive(Default)]
pub(crate) struct LossTracker {
    /// Packets that might still be answered: the sequence number, when we learned it
    /// was missing, and the line to report if it stays that way. Oldest first.
    pending: VecDeque<(u16, Instant, String)>,
    /// Packets already answered or counted as lost, oldest first.
    resolved: VecDeque<(u16, Instant)>,
    /// The highest sequence number answered so far.
    highest: Option<u16>,
}

impl LossTracker {
    /// Takes one parsed line and returns what can be reported straight away.
    pub(crate) fn on_result(&mut self, result: PingResult, now: Instant) -> Vec<PingResult> {
        match result {
            PingResult::Timeout(line) => match seq_of(&line) {
                Some(seq) => {
                    if !self.is_resolved(seq) && !self.is_pending(seq) {
                        self.pending.push_back((seq, now, line));
                    }
                    vec![]
                }
                // Nothing to match a late reply against.
                None => vec![PingResult::Timeout(line)],
            },
            PingResult::Pong(duration, line) => match seq_of(&line) {
                Some(seq) => {
                    // A duplicate, or the reply to a packet already counted as lost.
                    if self.is_resolved(seq) {
                        return vec![];
                    }
                    self.pending.retain(|(pending, ..)| *pending != seq);
                    self.note_overtaken(seq, now);
                    self.resolve(seq, now);
                    vec![PingResult::Pong(duration, line)]
                }
                None => vec![PingResult::Pong(duration, line)],
            },
            other => vec![other],
        }
    }

    /// Counts the packets whose grace period has run out as lost.
    pub(crate) fn expire(&mut self, now: Instant) -> Vec<PingResult> {
        let mut lost = vec![];
        while let Some((_, since, _)) = self.pending.front() {
            if now.saturating_duration_since(*since) < LATE_REPLY_GRACE {
                break;
            }
            let (seq, _, line) = self.pending.pop_front().unwrap();
            self.resolve(seq, now);
            lost.push(PingResult::Timeout(line));
        }
        while let Some((_, at)) = self.resolved.front() {
            if self.resolved.len() <= RESOLVED_CAPACITY
                && now.saturating_duration_since(*at) < RESOLVED_MEMORY
            {
                break;
            }
            self.resolved.pop_front();
        }
        lost
    }

    /// When the oldest unanswered packet runs out of grace, if there is one.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .front()
            .map(|(_, since, _)| *since + LATE_REPLY_GRACE)
    }

    /// ping has exited, so nothing still missing is going to be answered.
    pub(crate) fn finish(&mut self) -> Vec<PingResult> {
        self.pending
            .drain(..)
            .map(|(_, _, line)| PingResult::Timeout(line))
            .collect()
    }

    /// A reply to `seq` means every packet sent between the previous highest reply and
    /// this one has been overtaken. ping may never mention them, so they are tracked
    /// from here as if it had reported them unanswered.
    fn note_overtaken(&mut self, seq: u16, now: Instant) {
        let Some(highest) = self.highest else {
            self.highest = Some(seq);
            return;
        };
        let ahead = seq.wrapping_sub(highest);
        if ahead == 0 || ahead >= 0x8000 {
            // Not ahead of the highest reply: a reordered or late one.
            return;
        }
        if ahead <= MAX_GAP {
            for step in 1..ahead {
                let missing = highest.wrapping_add(step);
                if !self.is_resolved(missing) && !self.is_pending(missing) {
                    self.pending.push_back((
                        missing,
                        now,
                        format!("no reply for icmp_seq={missing}"),
                    ));
                }
            }
        }
        self.highest = Some(seq);
    }

    fn resolve(&mut self, seq: u16, now: Instant) {
        self.resolved.push_back((seq, now));
    }

    fn is_resolved(&self, seq: u16) -> bool {
        self.resolved.iter().any(|(resolved, _)| *resolved == seq)
    }

    fn is_pending(&self, seq: u16) -> bool {
        self.pending.iter().any(|(pending, ..)| *pending == seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pong(seq: u16, ms: u64) -> PingResult {
        PingResult::Pong(
            Duration::from_millis(ms),
            format!("64 bytes from 192.0.2.1: icmp_seq={seq} ttl=64 time={ms} ms"),
        )
    }

    fn no_answer(seq: u16) -> PingResult {
        PingResult::Timeout(format!("no answer yet for icmp_seq={seq}"))
    }

    fn summary(results: &[PingResult]) -> Vec<String> {
        results.iter().map(|r| r.to_string()).collect()
    }

    /// Feeds `events` (seconds since start, result) and expires along the way, the
    /// way `Pinger::start` does, returning everything that would have been reported.
    fn run(events: Vec<(f64, PingResult)>, end: f64) -> Vec<PingResult> {
        let start = Instant::now();
        let at = |secs: f64| start + Duration::from_secs_f64(secs);
        let mut tracker = LossTracker::default();
        let mut out = vec![];
        for (secs, result) in events {
            out.extend(tracker.expire(at(secs)));
            out.extend(tracker.on_result(result, at(secs)));
        }
        out.extend(tracker.expire(at(end)));
        out
    }

    // iputils -O with a 300ms round trip and a 200ms interval: every packet is reported
    // as "no answer yet" before its reply arrives. None of them were lost.
    #[test]
    fn slow_replies_are_not_timeouts() {
        let mut events = vec![];
        for seq in 1..=10u16 {
            let sent = seq as f64 * 0.2;
            events.push((sent + 0.2, no_answer(seq)));
            events.push((sent + 0.3, pong(seq, 300)));
        }
        let out = run(events, 10.0);
        assert_eq!(summary(&out), vec!["300ms"; 10]);
    }

    #[test]
    fn unanswered_packets_are_timeouts_once_the_grace_period_is_over() {
        let events = || {
            vec![
                (0.0, pong(1, 10)),
                (0.4, no_answer(2)),
                (0.6, no_answer(3)),
                (0.8, pong(4, 10)),
            ]
        };
        // Not yet: they could still be answered.
        assert_eq!(summary(&run(events(), 1.0)), vec!["10ms", "10ms"]);
        assert_eq!(
            summary(&run(events(), 3.0)),
            vec!["10ms", "10ms", "Timeout", "Timeout"]
        );
    }

    // macOS: "Request timeout" only fires when the unanswered count reaches a new
    // high, so a packet can be lost without ever being reported. The later reply
    // that overtook it is what gives it away.
    #[test]
    fn overtaken_packets_count_as_lost_even_when_ping_says_nothing() {
        let line =
            |seq: u16| format!("64 bytes from 192.0.2.1: icmp_seq={seq} ttl=57 time=10.0 ms");
        let events = vec![
            (0.0, PingResult::Pong(Duration::from_millis(10), line(9))),
            (0.4, PingResult::Pong(Duration::from_millis(10), line(11))),
        ];
        let out = run(events, 3.0);
        assert_eq!(summary(&out), vec!["10ms", "10ms", "Timeout"]);
    }

    #[test]
    fn macos_request_timeout_lines_are_matched_against_late_replies() {
        let events = vec![
            (
                0.2,
                PingResult::Timeout("Request timeout for icmp_seq 0".to_string()),
            ),
            (0.39, pong(0, 386)),
            (0.59, pong(1, 386)),
        ];
        assert_eq!(summary(&run(events, 5.0)), vec!["386ms", "386ms"]);
    }

    #[test]
    fn duplicate_replies_are_counted_once() {
        let dup = PingResult::Pong(
            Duration::from_millis(10),
            "64 bytes from 192.0.2.1: icmp_seq=1 ttl=64 time=10.4 ms (DUP!)".to_string(),
        );
        let events = vec![(0.0, pong(1, 10)), (0.01, dup), (0.2, pong(2, 10))];
        assert_eq!(summary(&run(events, 5.0)), vec!["10ms", "10ms"]);
    }

    // BusyBox prints "seq=", not "icmp_seq=".
    #[test]
    fn busybox_sequence_numbers_are_understood() {
        let line = |seq: u16| format!("64 bytes from 192.0.2.1: seq={seq} ttl=64 time=1.000 ms");
        let events = vec![
            (0.0, PingResult::Pong(Duration::from_millis(1), line(0))),
            (0.6, PingResult::Pong(Duration::from_millis(1), line(3))),
        ];
        assert_eq!(
            summary(&run(events, 5.0)),
            vec!["1ms", "1ms", "Timeout", "Timeout"]
        );
    }

    // A reply that arrives after its packet was already counted as lost must not
    // add a second sample for that packet.
    #[test]
    fn a_reply_after_the_timeout_was_reported_is_dropped() {
        let events = vec![(0.2, no_answer(1)), (3.0, pong(1, 2800))];
        assert_eq!(summary(&run(events, 5.0)), vec!["Timeout"]);
    }

    #[test]
    fn sequence_numbers_wrap_around() {
        let events = vec![
            (0.0, pong(65535, 10)),
            (0.2, pong(0, 10)),
            (0.4, pong(1, 10)),
        ];
        assert_eq!(summary(&run(events, 5.0)), vec!["10ms"; 3]);
    }

    #[test]
    fn lines_without_a_sequence_number_pass_straight_through() {
        let events = vec![
            (0.0, PingResult::Timeout("Request timed out.".to_string())),
            (
                0.1,
                PingResult::Pong(Duration::from_millis(5), "time=5ms".to_string()),
            ),
        ];
        assert_eq!(summary(&run(events, 0.1)), vec!["Timeout", "5ms"]);
    }
}

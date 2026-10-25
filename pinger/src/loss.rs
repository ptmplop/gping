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
//! treated the same way. Where ping's [`Schedule`] is known, a packet that nothing at all
//! has been heard about counts as lost once it is overdue, so an outage shows while it
//! lasts, and a host that never answers shows as losing every packet.

use crate::{PingResult, Schedule};
use lazy_regex::*;
use std::collections::VecDeque;
use std::convert::TryFrom;
use std::time::{Duration, Instant};

/// How long a packet reported as unanswered may still be answered before it is
/// counted as lost.
pub(crate) const LATE_REPLY_GRACE: Duration = Duration::from_secs(2);

/// How many of the latest packets are remembered, to recognise a duplicate reply, or one
/// that turns up after its packet was counted as lost. A reply from further back than
/// this looks the same as ping having moved on by more than half the 16-bit sequence
/// numbers, and is taken that way.
const MEMORY: usize = 0x4000;
const _: () = assert!(MEMORY < 0x8000);

/// How much slower than its interval ping may be sending before a packet counts as
/// overdue. macOS's ping, asked for 200ms, sends every 205ms or so.
const SLACK: f64 = 0.05;

/// How quickly the interval ping is measured to keep follows a change in it.
const CALIBRATION_WEIGHT: f64 = 1.0 / 32.0;

/// Waking this much later than asked means this process (and perhaps ping with it) was
/// stopped or starved for that long, so the clock says nothing about what ping did.
pub(crate) const STALL: Duration = Duration::from_secs(1);

/// Sequence numbers are unwrapped onto a `u64` that starts here, which leaves room below
/// the first packet heard of for any that arrive out of order.
const ORIGIN: u64 = 1 << 32;

/// "icmp_seq=12" (replies everywhere, iputils timeouts), "icmp_seq 12" (macOS timeouts),
/// "seq=12" (BusyBox replies).
static SEQ_RE: Lazy<Regex> = lazy_regex!(r"(?-u)(?:icmp_)?seq[= ](\d+)");

/// "5 packets transmitted, 4 received" (iputils), "5 packets transmitted, 4 packets
/// received" (macOS, the BSDs, BusyBox).
static SUMMARY_RE: Lazy<Regex> =
    lazy_regex!(r"(?-u)(\d+) packets transmitted, (\d+) (?:packets )?received");

/// The packet a line is about. Sequence numbers are 16 bits on the wire, but macOS
/// prints its own unwrapped counter in "Request timeout" lines, so both are reduced
/// to 16 bits to compare them.
fn seq_of(line: &str) -> Option<u16> {
    let digits = SEQ_RE.captures(line)?.get(1)?.as_str();
    Some(digits.parse::<u64>().ok()? as u16)
}

/// How many packets ping sent and how many were answered, from its closing statistics.
pub(crate) fn summary_of(line: &str) -> Option<(u64, u64)> {
    let captures = SUMMARY_RE.captures(line)?;
    let number = |i: usize| captures.get(i)?.as_str().parse::<u64>().ok();
    Some((number(1)?, number(2)?))
}

/// What became of a packet behind the frontier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// Nothing is known: it's from before the first packet heard of.
    Unknown,
    /// Missing, and waiting in `pending` in case a reply turns up.
    Pending,
    Answered,
    /// Lost: ping reported it unanswered, or a later reply overtook it, and no reply
    /// came in the grace period.
    Lost,
    /// Lost for being overdue. That rests on a guess about when ping sent it, so a reply
    /// to it is still taken.
    Overdue,
}

/// Where a sequence number puts a packet.
enum Place {
    /// At the frontier or beyond it: nothing has been heard about it yet.
    Ahead(u64),
    /// Behind the frontier, and remembered.
    Behind(u64),
    /// Further behind than is remembered.
    Forgotten,
}

#[derive(Default)]
pub(crate) struct LossTracker {
    schedule: Option<Schedule>,
    /// The next packet that nothing has been heard about (unwrapped, see [`ORIGIN`]), once
    /// the first one has.
    frontier: Option<u64>,
    /// What became of the packets just behind the frontier, oldest first.
    fates: VecDeque<Fate>,
    /// Missing packets that might still be answered: which, until when, and the line to
    /// report if they aren't (none for a packet ping hasn't mentioned). Soonest first.
    pending: VecDeque<(u64, Instant, Option<String>)>,
    /// The latest packet whose send time is known, and that time.
    anchor: Option<(u64, Instant)>,
    /// The interval ping is seen to keep, in seconds, once replies have shown it.
    measured: Option<f64>,
    /// How many losses have been reported.
    reported: u64,
    /// ping's own count of the packets it sent and the replies it got, once it exits.
    summary: Option<(u64, u64)>,
}

impl LossTracker {
    pub(crate) fn new(schedule: Option<Schedule>) -> LossTracker {
        LossTracker {
            schedule,
            ..LossTracker::default()
        }
    }

    /// ping has started, and is sending its first packet.
    pub(crate) fn started(&mut self, now: Instant) {
        if let (Some(schedule), None) = (self.schedule, self.frontier) {
            let first = ORIGIN + u64::from(schedule.first);
            self.frontier = Some(first);
            self.anchor = Some((first, now));
        }
    }

    /// ping's closing statistics: how many packets it sent, and how many were answered.
    pub(crate) fn summary(&mut self, transmitted: u64, received: u64) {
        self.summary = Some((transmitted, received));
    }

    /// Takes one parsed line and returns what can be reported straight away.
    pub(crate) fn on_result(&mut self, result: PingResult, now: Instant) -> Vec<PingResult> {
        match result {
            PingResult::Timeout(line) => match seq_of(&line) {
                Some(seq) => {
                    self.on_timeout(seq, line, now);
                    vec![]
                }
                // Nothing to match a late reply against.
                None => {
                    self.reported += 1;
                    vec![PingResult::Timeout(line)]
                }
            },
            PingResult::Pong(rtt, line) => match seq_of(&line) {
                Some(seq) => self.on_reply(seq, rtt, line, now),
                None => vec![PingResult::Pong(rtt, line)],
            },
            other => vec![other],
        }
    }

    fn on_timeout(&mut self, seq: u16, line: String, now: Instant) {
        match self.locate(seq) {
            Place::Ahead(packet) => {
                self.skip_to(packet, None, now);
                self.advance(Fate::Pending);
                self.wait_for(packet, now + LATE_REPLY_GRACE, Some(line));
            }
            Place::Behind(packet) => {
                if let Some(fate @ Fate::Unknown) = self.fate_mut(packet) {
                    *fate = Fate::Pending;
                    self.wait_for(packet, now + LATE_REPLY_GRACE, Some(line));
                }
                // Otherwise it's already waited for, answered or counted.
            }
            Place::Forgotten => {}
        }
    }

    fn on_reply(&mut self, seq: u16, rtt: Duration, line: String, now: Instant) -> Vec<PingResult> {
        let sent = now.checked_sub(rtt).unwrap_or(now);
        let mut out = vec![];
        let packet = match self.locate(seq) {
            Place::Ahead(packet) => {
                self.skip_to(packet, Some(sent), now);
                self.advance(Fate::Answered);
                packet
            }
            Place::Behind(packet) => {
                let fate = self.fate_mut(packet).expect("a packet behind the frontier");
                match *fate {
                    // A duplicate, or an answer to a packet that ping itself reported
                    // unanswered well before.
                    Fate::Answered | Fate::Lost => {
                        self.observe(packet, sent);
                        return out;
                    }
                    Fate::Pending => {
                        *fate = Fate::Answered;
                        self.pending.retain(|(waiting, ..)| *waiting != packet);
                    }
                    Fate::Unknown | Fate::Overdue => *fate = Fate::Answered,
                }
                packet
            }
            Place::Forgotten => {
                out.extend(self.restart(seq));
                self.advance(Fate::Answered)
            }
        };
        self.observe(packet, sent);
        out.push(PingResult::Pong(rtt, line));
        out
    }

    /// Counts the packets whose grace period has run out, and those that are overdue,
    /// as lost.
    pub(crate) fn expire(&mut self, now: Instant) -> Vec<PingResult> {
        let mut lost = vec![];
        while self
            .pending
            .front()
            .is_some_and(|(_, deadline, _)| *deadline <= now)
        {
            let (packet, _, line) = self.pending.pop_front().expect("checked above");
            if let Some(fate) = self.fate_mut(packet) {
                *fate = Fate::Lost;
            }
            lost.push(self.lost(packet, line));
        }
        while self.overdue_at().is_some_and(|due| due <= now) {
            let packet = self.advance(Fate::Overdue);
            lost.push(self.lost(packet, None));
        }
        lost
    }

    /// When the next packet might have to be counted as lost, if any might.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let pending = self.pending.front().map(|(_, deadline, _)| *deadline);
        match (pending, self.overdue_at()) {
            (Some(pending), Some(overdue)) => Some(pending.min(overdue)),
            (pending, overdue) => pending.or(overdue),
        }
    }

    /// This process was stopped or starved for `late`, and is only now catching up with
    /// what ping printed meanwhile, or ping was stopped along with it. Either way, wait
    /// to read whatever ping has to say before counting anything as lost.
    pub(crate) fn stalled(&mut self, late: Duration) {
        let delay = late + STALL;
        for (_, deadline, _) in &mut self.pending {
            *deadline += delay;
        }
        if let Some((_, sent)) = &mut self.anchor {
            *sent += delay;
        }
    }

    /// ping has exited, so nothing still missing is going to be answered.
    pub(crate) fn finish(&mut self) -> Vec<PingResult> {
        let mut lost = vec![];
        for (packet, _, line) in std::mem::take(&mut self.pending) {
            lost.push(self.lost(packet, line));
        }
        // ping reports a packet unanswered when it sends the next one, so the last one
        // before it stops (`-c`) is never mentioned. Its statistics account for it.
        if let Some((transmitted, received)) = self.summary {
            let unreported = transmitted
                .saturating_sub(received)
                .saturating_sub(self.reported)
                .min(0x10000);
            let next = self.frontier.unwrap_or(ORIGIN);
            for packet in next..next + unreported {
                lost.push(self.lost(packet, None));
            }
        }
        lost
    }

    /// Finds the packet `seq` is about, relative to the frontier.
    fn locate(&mut self, seq: u16) -> Place {
        let frontier = *self.frontier.get_or_insert(ORIGIN + u64::from(seq));
        let ahead = seq.wrapping_sub(frontier as u16);
        if ahead < 0x8000 {
            return Place::Ahead(frontier + u64::from(ahead));
        }
        let behind = 0x10000 - u64::from(ahead);
        if behind > MEMORY as u64 {
            return Place::Forgotten;
        }
        // Make room for a packet from before the first one heard of.
        while (self.fates.len() as u64) < behind {
            self.fates.push_front(Fate::Unknown);
        }
        Place::Behind(frontier - behind)
    }

    fn fate_mut(&mut self, packet: u64) -> Option<&mut Fate> {
        let behind = self.frontier?.checked_sub(packet)?;
        let index = (self.fates.len() as u64).checked_sub(behind)?;
        self.fates.get_mut(index as usize)
    }

    /// Moves the frontier on by one packet, which met `fate`, and returns that packet.
    fn advance(&mut self, fate: Fate) -> u64 {
        let packet = self.frontier.expect("the frontier is set before it moves");
        self.fates.push_back(fate);
        if self.fates.len() > MEMORY {
            self.fates.pop_front();
        }
        self.frontier = Some(packet + 1);
        packet
    }

    /// Every packet from the frontier up to `packet` is missing: overtaken by a reply to
    /// `packet`, sent at `sent`, or reported unanswered before them.
    fn skip_to(&mut self, packet: u64, sent: Option<Instant>, now: Instant) {
        let latest = now + LATE_REPLY_GRACE;
        while let Some(missing) = self.frontier.filter(|frontier| *frontier < packet) {
            // Counting back from the reply that overtook it, when it was probably sent.
            let deadline = match (self.schedule, sent) {
                (Some(schedule), Some(sent)) => schedule
                    .interval
                    .checked_mul((packet - missing) as u32)
                    .and_then(|ago| sent.checked_sub(ago))
                    .map_or(now, |at| at + LATE_REPLY_GRACE)
                    .clamp(now, latest),
                _ => latest,
            };
            self.advance(Fate::Pending);
            self.wait_for(missing, deadline, None);
        }
    }

    fn wait_for(&mut self, packet: u64, deadline: Instant, line: Option<String>) {
        let at = self
            .pending
            .partition_point(|(_, other, _)| *other <= deadline);
        self.pending.insert(at, (packet, deadline, line));
    }

    /// A reply shows that ping sent `packet` at `sent`.
    fn observe(&mut self, packet: u64, sent: Instant) {
        let Some(schedule) = self.schedule else {
            return;
        };
        if let Some((anchor, at)) = self.anchor {
            if anchor > packet {
                return;
            }
            // How long each packet since the last one known took to go out. Way off the
            // interval asked for, ping was held up (stopped, or this process was), which
            // says nothing about the pace it keeps.
            let nominal = schedule.interval.as_secs_f64();
            let steps = packet - anchor;
            let each = sent.saturating_duration_since(at).as_secs_f64() / steps.max(1) as f64;
            if (1..=16).contains(&steps) && (nominal * 0.5..nominal * 1.5).contains(&each) {
                let measured = self.measured.unwrap_or(nominal);
                self.measured = Some(measured + (each - measured) * CALIBRATION_WEIGHT);
            }
        }
        self.anchor = Some((packet, sent));
    }

    /// When the packet at the frontier is overdue, if ping's schedule is known: when it
    /// should have been sent, allowing for ping running slow, and a grace period on top.
    fn overdue_at(&self) -> Option<Instant> {
        let schedule = self.schedule?;
        let (anchor, sent) = self.anchor?;
        let after = u32::try_from(self.frontier?.checked_sub(anchor)?).ok()?;
        let nominal = schedule.interval.as_secs_f64();
        let interval = Duration::try_from_secs_f64(
            self.measured.unwrap_or(nominal).max(nominal) * (1.0 + SLACK),
        )
        .ok()?;
        sent.checked_add(interval.checked_mul(after)?)?
            .checked_add(LATE_REPLY_GRACE)
    }

    /// ping has moved on by more than half the sequence numbers since the last packet
    /// heard of (or a reply is impossibly late, which looks the same). Starts again from
    /// there, counting whatever was still waited for as lost.
    fn restart(&mut self, seq: u16) -> Vec<PingResult> {
        let mut lost = vec![];
        for (packet, _, line) in std::mem::take(&mut self.pending) {
            lost.push(self.lost(packet, line));
        }
        let frontier = self.frontier.expect("set by locate");
        self.frontier = Some(frontier + u64::from(seq.wrapping_sub(frontier as u16)));
        self.fates.clear();
        self.anchor = None;
        lost
    }

    fn lost(&mut self, packet: u64, line: Option<String>) -> PingResult {
        self.reported += 1;
        PingResult::Timeout(
            line.unwrap_or_else(|| format!("no reply for icmp_seq={}", packet as u16)),
        )
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

    fn timeouts<'a>(results: impl IntoIterator<Item = &'a PingResult>) -> usize {
        results
            .into_iter()
            .filter(|r| matches!(r, PingResult::Timeout(_)))
            .count()
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

    /// Plays `events` through a tracker for a ping that keeps `schedule` and started at
    /// 0s, waking at every deadline the way `Pinger::start` does. Returns what was
    /// reported, with when.
    fn play(
        schedule: Option<Schedule>,
        events: Vec<(f64, PingResult)>,
        end: f64,
    ) -> Vec<(f64, PingResult)> {
        let start = Instant::now();
        let at = |secs: f64| start + Duration::from_secs_f64(secs);
        let secs = |instant: Instant| instant.duration_since(start).as_secs_f64();
        let mut tracker = LossTracker::new(schedule);
        tracker.started(start);
        let mut out = vec![];
        let wake = |tracker: &mut LossTracker, until: Instant, out: &mut Vec<(f64, PingResult)>| {
            while let Some(deadline) = tracker.next_deadline().filter(|d| *d <= until) {
                let reported = tracker.expire(deadline);
                out.extend(reported.into_iter().map(|r| (secs(deadline), r)));
            }
        };
        for (time, result) in events {
            wake(&mut tracker, at(time), &mut out);
            let reported = tracker.on_result(result, at(time));
            out.extend(reported.into_iter().map(|r| (time, r)));
            let reported = tracker.expire(at(time));
            out.extend(reported.into_iter().map(|r| (time, r)));
        }
        wake(&mut tracker, at(end), &mut out);
        out
    }

    fn every(interval_ms: u64) -> Option<Schedule> {
        Some(Schedule {
            first: 0,
            interval: Duration::from_millis(interval_ms),
        })
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

    // Packets lost in an outage that outlasts any time limit on remembering them must
    // not be counted again when the first reply afterwards overtakes them.
    #[test]
    fn a_long_outage_is_counted_once() {
        let mut events = vec![(0.0, pong(0, 10))];
        for seq in 1..=400u16 {
            events.push((0.2 * seq as f64 + 0.2, no_answer(seq)));
        }
        events.push((80.4, pong(401, 10)));
        assert_eq!(timeouts(&run(events, 83.0)), 400);
    }

    // Where ping says nothing about lost packets, the reply after an outage of any
    // length (short of half the sequence numbers) accounts for all of them.
    #[test]
    fn a_silent_outage_of_any_length_is_counted() {
        let events = vec![
            (0.0, pong(0, 10)),
            (400.0, pong(2000, 10)),
            (400.2, pong(2001, 10)),
        ];
        assert_eq!(timeouts(&run(events, 405.0)), 1999);
    }

    // After a jump of more than half the sequence numbers, which can't be told from a
    // very late reply, losses are still noticed.
    #[test]
    fn losses_are_noticed_after_a_huge_jump() {
        let events = vec![
            (0.0, pong(0, 10)),
            (8000.0, pong(40000, 10)),
            (8000.2, pong(40001, 10)),
            (8000.6, pong(40003, 10)),
        ];
        let out = run(events, 8010.0);
        assert_eq!(timeouts(&out), 1);
        assert_eq!(out.len(), 5);
    }

    // A reply from a little before the first one heard of is a reply, not a jump.
    #[test]
    fn replies_from_before_the_first_are_kept() {
        let events = vec![(0.0, pong(5, 10)), (0.05, pong(4, 60)), (0.2, pong(6, 10))];
        assert_eq!(summary(&run(events, 5.0)), vec!["10ms", "60ms", "10ms"]);
    }

    // A host that never answers, behind a ping that never says so: every packet still
    // counts as lost, from two seconds after it was sent, one per interval.
    #[test]
    fn a_silent_ping_to_a_dead_host_loses_every_packet() {
        let out = play(every(200), vec![], 10.0);
        let times: Vec<f64> = out.iter().map(|(t, _)| *t).collect();
        assert_eq!(timeouts(out.iter().map(|(_, r)| r)), out.len());
        assert!((times[0] - 2.0).abs() < 1e-9, "first loss at {}", times[0]);
        assert!((times[1] - times[0] - 0.21).abs() < 1e-6);
        assert_eq!(out.len(), 39);
    }

    // An outage behind a silent ping shows as it happens: each loss is reported about
    // two seconds after its packet went out, not all of them when the host is back.
    #[test]
    fn a_silent_outage_is_reported_as_it_happens() {
        let mut events: Vec<_> = (0..=10u16)
            .map(|seq| (seq as f64 * 0.2 + 0.01, pong(seq, 10)))
            .collect();
        events.push((12.01, pong(60, 10)));
        let out = play(every(200), events, 14.0);
        let losses: Vec<f64> = out
            .iter()
            .filter(|(_, r)| matches!(r, PingResult::Timeout(_)))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(losses.len(), 49);
        // Packet 11 went out at 2.2s, and is counted as lost two seconds after it was due.
        assert!(
            (losses[0] - 4.21).abs() < 1e-6,
            "first loss at {}",
            losses[0]
        );
        // Most were reported before the host came back; the rest when they were due.
        assert!(losses.iter().filter(|t| **t < 12.0).count() >= 35);
        assert!(losses.windows(2).all(|w| w[1] - w[0] < 0.3));
    }

    // If ping falls behind its schedule (it was stopped, or runs slower than allowed
    // for), packets get counted as overdue before they were even sent. Their replies
    // must still come through, or a healthy host would look dead.
    #[test]
    fn replies_to_packets_counted_as_overdue_still_count() {
        let mut events: Vec<_> = (0..=5u16)
            .map(|seq| (seq as f64 * 0.2 + 0.01, pong(seq, 10)))
            .collect();
        // ping stops for five seconds, then carries on where it left off.
        events.extend((6..=40u16).map(|seq| (seq as f64 * 0.2 + 5.01, pong(seq, 10))));
        let out = play(every(200), events, 15.0);
        let replies = out
            .iter()
            .filter(|(_, r)| matches!(r, PingResult::Pong(..)))
            .count();
        assert_eq!(replies, 41);
    }

    // Woken long after a deadline (this process was stopped), with ping's output
    // still to read: nothing counts as lost until it has been.
    #[test]
    fn a_stall_holds_back_judgement() {
        let start = Instant::now();
        let at = |secs: f64| start + Duration::from_secs_f64(secs);
        let mut tracker = LossTracker::new(every(200));
        tracker.started(start);
        for seq in 0..=5u16 {
            tracker.on_result(pong(seq, 10), at(seq as f64 * 0.2 + 0.01));
        }
        let deadline = tracker.next_deadline().unwrap();
        let now = at(30.0);
        tracker.stalled(now - deadline);
        assert!(tracker.expire(now).is_empty());
        for seq in 6..=150u16 {
            let reported = tracker.on_result(pong(seq, 10), now);
            assert_eq!(timeouts(&reported), 0);
        }
        assert!(tracker.expire(now).is_empty());
    }

    // `-c 2` to a dead host: ping only mentions the first packet, and exits.
    #[test]
    fn the_last_packet_counts_by_ping_s_own_statistics() {
        let start = Instant::now();
        let mut tracker = LossTracker::new(None);
        let line = "Request timeout for icmp_seq 0".to_string();
        tracker.on_result(PingResult::Timeout(line), start);
        assert_eq!(
            summary_of("2 packets transmitted, 0 packets received, 100.0% packet loss"),
            Some((2, 0))
        );
        tracker.summary(2, 0);
        assert_eq!(timeouts(&tracker.finish()), 2);
    }

    // With -q, ping prints nothing but its statistics: only what it lost counts.
    #[test]
    fn statistics_alone_count_only_the_losses() {
        let mut tracker = LossTracker::new(None);
        let line = "10 packets transmitted, 8 received, 20% packet loss, time 9012ms";
        let (transmitted, received) = summary_of(line).unwrap();
        tracker.summary(transmitted, received);
        assert_eq!(timeouts(&tracker.finish()), 2);
    }
}

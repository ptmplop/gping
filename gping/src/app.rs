//! Application state: the hosts being watched and what is derived from their samples.

use crate::format::nice_step;
use crate::history::History;
use crate::stats::{percentile, Summary};
use crate::theme::Rgb;
use std::collections::VecDeque;
use std::time::Duration;
use tui::style::Color;

#[derive(Debug, Clone, PartialEq)]
pub enum HostState {
    Running,
    /// The ping process or command loop ended normally (e.g. `--ping-args -c 5`).
    Finished,
    /// It ended with an error, kept so it can be shown and reported on exit.
    Failed(String),
}

/// How a host is doing, as one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Nothing has come back yet.
    Waiting,
    /// No reply for a while.
    Down,
    /// Lost pings in the last few seconds.
    Lossy,
    /// Replying, but slower than usual.
    Slow,
    Steady,
    /// The pinger has stopped: normally, or with an error.
    Finished,
    Failed,
}

/// What a host's big readout shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Readout {
    Waiting,
    /// A latency in microseconds.
    Value(f64),
    /// The newest probe was lost.
    Lost,
    /// No reply for this many seconds.
    NoReply(f64),
}

/// Latency grades, by how far a reply is above the host's usual: none, elevated, high and
/// critical. Measured against the host's own baseline, so a resolver at 7 ms and a path
/// across an ocean at 180 ms are judged on the same terms.
pub fn grade(value: f64, baseline: f64) -> u8 {
    let excess = value - baseline;
    let thresholds = [
        (10_000.0f64).max(0.25 * baseline),
        (30_000.0f64).max(0.75 * baseline),
        (100_000.0f64).max(2.0 * baseline),
    ];
    thresholds.iter().take_while(|&&t| excess >= t).count() as u8
}

/// How long the baseline looks back.
const BASELINE_SECONDS: f64 = 600.0;
/// Replies needed before there is a baseline to judge against.
const BASELINE_MIN_REPLIES: usize = 10;

pub struct Host {
    /// The host or command as the user gave it (after expanding cloud shorthands).
    pub name: String,
    /// The address the name resolved to, when that adds anything to the name.
    pub address: Option<String>,
    /// Line colour: the user's `--color` choice or one from the palette.
    pub color: Color,
    /// An RGB approximation of `color`, for fills and tints.
    pub tint: Rgb,
    /// Whether the colour was chosen with --color, which then wins over heat colouring.
    pub chosen_color: bool,
    /// `(seconds since start, latency in microseconds or NaN when lost)`.
    pub samples: Vec<(f64, f64)>,
    /// The whole session, which outlives the samples trimmed out of `samples`.
    pub session: Session,
    pub history: History,
    pub state: HostState,
    /// Recent replies, for the baseline.
    recent: VecDeque<(f64, f64)>,
}

/// Running totals for the whole session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Session {
    pub sent: u64,
    pub lost: u64,
    /// Latency totals over the replies, in microseconds.
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    /// Sum of the differences between consecutive replies, for the jitter.
    jitter_sum: f64,
    previous: Option<f64>,
    /// When the last reply arrived, in seconds since start.
    pub last_reply: Option<f64>,
}

impl Session {
    fn new() -> Session {
        Session {
            sent: 0,
            lost: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            sum: 0.0,
            jitter_sum: 0.0,
            previous: None,
            last_reply: None,
        }
    }

    pub fn replies(&self) -> u64 {
        self.sent - self.lost
    }

    pub fn mean(&self) -> Option<f64> {
        (self.replies() > 0).then(|| self.sum / self.replies() as f64)
    }

    pub fn jitter(&self) -> Option<f64> {
        (self.replies() > 1).then(|| self.jitter_sum / (self.replies() - 1) as f64)
    }

    pub fn loss(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            self.lost as f64 / self.sent as f64
        }
    }
}

impl Host {
    pub fn new(name: String, address: Option<String>, color: Color, tint: Rgb) -> Host {
        Host {
            name,
            address,
            color,
            tint,
            chosen_color: false,
            samples: Vec::with_capacity(512),
            session: Session::new(),
            history: History::new(0.0),
            state: HostState::Running,
            recent: VecDeque::new(),
        }
    }

    /// Records a reply or a lost ping. Losses are reported late (see
    /// `pinger::timeout_delay`) and arrive back-dated, so a sample can belong before ones
    /// already recorded; it's slotted into place to keep the history in time order.
    pub fn record(&mut self, at: f64, reply: Option<Duration>) {
        let session = &mut self.session;
        session.sent += 1;
        let value = match reply {
            Some(rtt) => {
                let us = rtt.as_secs_f64() * 1_000_000.0;
                session.min = session.min.min(us);
                session.max = session.max.max(us);
                session.sum += us;
                if let Some(previous) = session.previous {
                    session.jitter_sum += (us - previous).abs();
                }
                session.previous = Some(us);
                session.last_reply = Some(session.last_reply.map_or(at, |last| last.max(at)));
                self.recent.push_back((at, us));
                while self
                    .recent
                    .front()
                    .is_some_and(|&(t, _)| t < at - BASELINE_SECONDS)
                {
                    self.recent.pop_front();
                }
                us
            }
            None => {
                session.lost += 1;
                f64::NAN
            }
        };
        let position = self.samples.partition_point(|&(t, _)| t <= at);
        self.samples.insert(position, (at, value));
        self.history.record(at, value);
    }

    /// Forgets the session so far: totals, history and baseline. The chart is untouched.
    pub fn reset(&mut self, now: f64) {
        self.session = Session::new();
        self.history = History::new(now);
        self.recent.clear();
    }

    /// Drops samples older than `before`, keeping the newest of them so the line can still
    /// be drawn in from the left edge.
    pub fn trim(&mut self, before: f64) {
        let stale = self.samples.partition_point(|&(t, _)| t < before);
        if stale > 1 {
            self.samples.drain(..stale - 1);
        }
    }

    /// The samples taken within `[from, to]`.
    pub fn window(&self, [from, to]: [f64; 2]) -> &[(f64, f64)] {
        let start = self.samples.partition_point(|&(t, _)| t < from);
        let end = self.samples.partition_point(|&(t, _)| t <= to);
        &self.samples[start..end.max(start)]
    }

    pub fn summary(&self, bounds: [f64; 2]) -> Option<Summary> {
        Summary::of(self.window(bounds).iter().map(|&(_, v)| v))
    }

    /// The host's usual latency: the median of its replies over the last ten minutes.
    pub fn baseline(&self) -> Option<f64> {
        if self.recent.len() < BASELINE_MIN_REPLIES {
            return None;
        }
        let mut values: Vec<f64> = self.recent.iter().map(|&(_, v)| v).collect();
        Some(percentile(&mut values, 0.5))
    }

    /// How long without a reply counts as down.
    fn down_after(&self, interval: f64) -> f64 {
        let typical = self.baseline().unwrap_or(0.0) / 1_000_000.0;
        3.0f64.max(5.0 * interval).max(3.0 * typical + interval)
    }

    pub fn status(&self, now: f64, interval: f64) -> Status {
        match self.state {
            HostState::Failed(_) => return Status::Failed,
            HostState::Finished => return Status::Finished,
            HostState::Running => {}
        }
        let silent_for = now - self.session.last_reply.unwrap_or(0.0);
        let last_three_lost = self.samples.len() >= 3
            && self.samples[self.samples.len() - 3..]
                .iter()
                .all(|s| s.1.is_nan());
        if last_three_lost || (self.session.sent > 0 && silent_for >= self.down_after(interval)) {
            return Status::Down;
        }
        if self.session.last_reply.is_none() {
            return Status::Waiting;
        }
        if self
            .samples
            .iter()
            .rev()
            .take_while(|&&(t, _)| t >= now - 10.0)
            .any(|s| s.1.is_nan())
        {
            return Status::Lossy;
        }
        let latest: Vec<f64> = self.recent.iter().rev().take(5).map(|&(_, v)| v).collect();
        if let (Some(baseline), false) = (self.baseline(), latest.is_empty()) {
            let mut latest = latest;
            if grade(percentile(&mut latest, 0.5), baseline) >= 1 {
                return Status::Slow;
            }
        }
        Status::Steady
    }

    /// The one number to show for the host right now: the median of the replies from the
    /// last second (so a fast ping interval doesn't make it flicker), or the newest reply
    /// when they come less often than that.
    pub fn readout(&self, now: f64, interval: f64) -> Readout {
        if self.status(now, interval) == Status::Down {
            return Readout::NoReply(now - self.session.last_reply.unwrap_or(0.0));
        }
        let Some(&(newest, value)) = self.samples.last() else {
            return Readout::Waiting;
        };
        if value.is_nan() {
            return Readout::Lost;
        }
        let mut recent: Vec<f64> = self
            .samples
            .iter()
            .rev()
            .take_while(|&&(t, _)| t >= newest - 1.0)
            .map(|&(_, v)| v)
            .filter(|v| !v.is_nan())
            .collect();
        if interval >= 1.0 || recent.is_empty() {
            return Readout::Value(value);
        }
        Readout::Value(percentile(&mut recent, 0.5))
    }
}

pub struct App {
    pub hosts: Vec<Host>,
    /// Seconds of history on screen.
    pub window: f64,
    /// When gping started, in seconds on the same clock as the samples.
    pub started: f64,
    /// The moment the view was frozen at, while paused.
    pub paused_at: Option<f64>,
    /// `--ymin`/`--ymax` in microseconds.
    pub yrange: (Option<f64>, Option<f64>),
}

impl App {
    pub fn new(
        hosts: Vec<Host>,
        window: f64,
        started: f64,
        yrange: (Option<f64>, Option<f64>),
    ) -> App {
        App {
            hosts,
            window,
            started,
            paused_at: None,
            yrange,
        }
    }

    pub fn record(&mut self, host: usize, at: f64, reply: Option<Duration>) {
        if let Some(host) = self.hosts.get_mut(host) {
            host.record(at, reply);
        }
    }

    /// The time span on screen: the last `window` seconds, with now at the right edge.
    pub fn x_bounds(&self, now: f64) -> [f64; 2] {
        let end = self.paused_at.unwrap_or(now);
        [end - self.window, end]
    }

    /// Bounds for the latency axis, from the samples on screen of the given hosts. A bound
    /// pinned with --ymin/--ymax is used verbatim; a derived one gets 10% of headroom so the
    /// line doesn't sit on the edge of the plot.
    pub fn y_bounds_of<'a>(
        &self,
        hosts: impl IntoIterator<Item = &'a Host>,
        x_bounds: [f64; 2],
    ) -> [f64; 2] {
        let (ymin, ymax) = self.yrange;
        let mut data = hosts
            .into_iter()
            .flat_map(|h| h.window(x_bounds))
            .map(|&(_, v)| v)
            .filter(|v| !v.is_nan());
        let (data_min, data_max) = match data.next() {
            None => (0.0, 0.0),
            Some(first) => data.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))),
        };

        let mut min = ymin.unwrap_or(data_min - data_min * 0.1);
        let mut max = ymax.unwrap_or(data_max + data_max * 0.1);

        // Never negative, and never zero height (no data yet, or --ymin pinned above all of
        // it). When the collision is between a pinned --ymax and a derived minimum, it's the
        // derived side that gives way: the user asked for the axis to end there.
        min = min.max(0.0);
        if max <= min {
            if ymin.is_none() && ymax.is_some() {
                min = 0.0;
            }
            if max <= min {
                max = min + 1_000.0;
            }
        }
        [min, max]
    }

    /// Widens derived bounds to at least `max(5 ms, half the typical latency)`, so that an
    /// axis fitted to a quiet host doesn't blow a 0.1 ms wobble up to the full height of
    /// the plot. Pinned bounds stay where they are.
    pub fn with_span_floor(&self, [mut lo, mut hi]: [f64; 2], typical: f64) -> [f64; 2] {
        let floor = (5_000.0f64).max(0.5 * typical);
        let missing = floor - (hi - lo);
        if missing <= 0.0 {
            return [lo, hi];
        }
        match self.yrange {
            (Some(_), Some(_)) => {}
            (Some(_), None) => hi += missing,
            (None, Some(_)) => lo = (lo - missing).max(0.0),
            (None, None) => {
                lo -= missing / 2.0;
                hi += missing / 2.0;
                if lo < 0.0 {
                    hi -= lo;
                    lo = 0.0;
                }
            }
        }
        [lo, hi]
    }

    /// Evenly spaced "nice" values to label and draw guides at, within `bounds`.
    pub fn y_ticks(bounds: [f64; 2], rows: u16) -> (Vec<f64>, f64) {
        let [lo, hi] = bounds;
        let step = nice_step(hi - lo, (rows as usize / 3 + 1).clamp(2, 6));
        let mut ticks = Vec::new();
        let mut tick = (lo / step).ceil() * step;
        while tick <= hi + step * 1e-9 && ticks.len() < 64 {
            ticks.push(tick);
            tick += step;
        }
        (ticks, step)
    }
}

/// Axis bounds eased over time, so the axis doesn't jump around as spikes come and go: they
/// widen immediately (nothing is ever drawn off-scale) and narrow gradually.
#[derive(Debug, Default, Clone, Copy)]
pub struct EasedBounds {
    current: Option<[f64; 2]>,
}

impl EasedBounds {
    /// Moves towards `target`; `dt` is the time since the last frame, in seconds. Pinned
    /// ends are taken as they are.
    pub fn update(&mut self, target: [f64; 2], dt: f64, pinned: (bool, bool)) -> [f64; 2] {
        let bounds = match self.current {
            None => target,
            Some([lo, hi]) => {
                let k = 1.0 - (-dt.max(0.0) / 0.45).exp();
                let lo = if pinned.0 || target[0] < lo {
                    target[0]
                } else {
                    lo + (target[0] - lo) * k
                };
                let hi = if pinned.1 || target[1] > hi {
                    target[1]
                } else {
                    hi + (target[1] - hi) * k
                };
                if hi > lo {
                    [lo, hi]
                } else {
                    target
                }
            }
        };
        self.current = Some(bounds);
        bounds
    }

    /// Whether the bounds are still moving towards `target`.
    pub fn settling(&self, target: [f64; 2]) -> bool {
        self.current.is_some_and(|[lo, hi]| {
            let span = (target[1] - target[0]).abs().max(1.0);
            (lo - target[0]).abs() > span * 0.002 || (hi - target[1]).abs() > span * 0.002
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app holding a single host whose samples are the given microsecond values, one a
    /// second, the last of them taken at `now`.
    fn app_with(samples: &[f64], yrange: (Option<f64>, Option<f64>)) -> (App, f64) {
        let now = 1_000_000.0;
        let mut host = Host::new("host".into(), None, Color::Reset, (0, 0, 0));
        for (i, value) in samples.iter().enumerate() {
            host.samples
                .push((now - (samples.len() - 1 - i) as f64, *value));
        }
        (App::new(vec![host], 30.0, now - 100.0, yrange), now)
    }

    fn bounds(samples: &[f64], yrange: (Option<f64>, Option<f64>)) -> [f64; 2] {
        let (app, now) = app_with(samples, yrange);
        app.y_bounds_of(&app.hosts, app.x_bounds(now))
    }

    // --ymin/--ymax are exact requests: the 10% headroom added to automatically derived
    // bounds must not push the axis past what the user asked for.
    #[test]
    fn user_supplied_y_bounds_are_used_verbatim() {
        assert_eq!(
            bounds(&[1_000.0, 9_000.0], (Some(0.0), Some(10_000.0))),
            [0.0, 10_000.0]
        );
    }

    #[test]
    fn automatic_y_bounds_keep_their_headroom() {
        assert_eq!(bounds(&[1_000.0, 2_000.0], (None, None)), [900.0, 2_200.0]);
    }

    // Only the pinned end is fixed; the other one still scales to the data.
    #[test]
    fn one_sided_y_bounds() {
        assert_eq!(
            bounds(&[1_000.0, 2_000.0], (Some(0.0), None)),
            [0.0, 2_200.0]
        );
        assert_eq!(
            bounds(&[1_000.0, 2_000.0], (None, Some(10_000.0))),
            [900.0, 10_000.0]
        );
    }

    #[test]
    fn y_bounds_with_a_single_sample() {
        assert_eq!(
            bounds(&[5_000.0], (Some(0.0), Some(10_000.0))),
            [0.0, 10_000.0]
        );
    }

    // Before any ping comes back there is nothing to scale to, so the axis has to fall
    // back to something with a non-zero height rather than to infinities.
    #[test]
    fn y_bounds_without_any_data() {
        assert_eq!(bounds(&[], (None, None)), [0.0, 1_000.0]);
        assert_eq!(
            bounds(&[], (Some(2_000.0), Some(8_000.0))),
            [2_000.0, 8_000.0]
        );
        // Lost pings are not data either.
        assert_eq!(bounds(&[f64::NAN, f64::NAN], (None, None)), [0.0, 1_000.0]);
    }

    // --ymin can sit above every sample we have, which would otherwise give an
    // upside-down axis: the pinned bound wins and the axis keeps 1ms of height.
    #[test]
    fn y_min_pinned_above_the_data() {
        assert_eq!(
            bounds(&[1_000.0, 2_000.0], (Some(50_000.0), None)),
            [50_000.0, 51_000.0]
        );
    }

    // Every sample sits above a pinned --ymax: the derived minimum (90% of the smallest one)
    // would land above the maximum, and used to push the user's bound up to match, leaving
    // a 1ms band below all the data and an empty chart.
    #[test]
    fn y_max_pinned_below_the_data_is_kept() {
        assert_eq!(
            bounds(&[100_000.0, 110_000.0], (None, Some(50_000.0))),
            [0.0, 50_000.0]
        );
    }

    // Samples that have scrolled off screen must not keep stretching the axis.
    #[test]
    fn y_bounds_only_consider_the_visible_window() {
        let (mut app, now) = app_with(&[1_000.0, 2_000.0], (None, None));
        app.hosts[0].samples.insert(0, (now - 500.0, 90_000.0));
        assert_eq!(
            app.y_bounds_of(&app.hosts, app.x_bounds(now)),
            [900.0, 2_200.0]
        );
    }

    #[test]
    fn the_span_floor_keeps_quiet_hosts_quiet() {
        let (app, _) = app_with(&[], (None, None));
        // 9.9-10.1 ms: widened to 5 ms around the middle.
        assert_eq!(
            app.with_span_floor([9_900.0, 10_100.0], 10_000.0),
            [7_500.0, 12_500.0]
        );
        // Never below zero: the extra goes on top instead.
        assert_eq!(app.with_span_floor([100.0, 300.0], 200.0), [0.0, 5_000.0]);
        // A slow host's floor is half its typical latency.
        assert_eq!(
            app.with_span_floor([200_000.0, 210_000.0], 200_000.0),
            [155_000.0, 255_000.0]
        );
        // Already wide enough: untouched.
        assert_eq!(
            app.with_span_floor([1_000.0, 90_000.0], 10_000.0),
            [1_000.0, 90_000.0]
        );
    }

    #[test]
    fn the_span_floor_respects_pinned_bounds() {
        let (app, _) = app_with(&[], (Some(10_000.0), None));
        assert_eq!(
            app.with_span_floor([10_000.0, 11_000.0], 10_000.0),
            [10_000.0, 15_000.0]
        );
        let (app, _) = app_with(&[], (Some(10_000.0), Some(11_000.0)));
        assert_eq!(
            app.with_span_floor([10_000.0, 11_000.0], 10_000.0),
            [10_000.0, 11_000.0]
        );
    }

    #[test]
    fn now_is_always_at_the_right_edge() {
        let (app, _) = app_with(&[], (None, None));
        assert_eq!(app.x_bounds(5.0), [-25.0, 5.0]);
        assert_eq!(app.x_bounds(50.0), [20.0, 50.0]);
    }

    #[test]
    fn pausing_freezes_the_view() {
        let (mut app, _) = app_with(&[], (None, None));
        app.paused_at = Some(40.0);
        assert_eq!(app.x_bounds(90.0), [10.0, 40.0]);
    }

    #[test]
    fn easing_widens_at_once_and_narrows_gradually() {
        let mut eased = EasedBounds::default();
        let free = (false, false);
        assert_eq!(eased.update([0.0, 100.0], 0.1, free), [0.0, 100.0]);
        // A spike: the top moves straight up.
        assert_eq!(eased.update([0.0, 500.0], 0.1, free), [0.0, 500.0]);
        // It scrolls away: the top eases back down, not all the way in one frame.
        let [_, hi] = eased.update([0.0, 100.0], 0.1, free);
        assert!(hi < 500.0 && hi > 100.0, "{}", hi);
        assert!(eased.settling([0.0, 100.0]));
        let mut hi = hi;
        for _ in 0..100 {
            hi = eased.update([0.0, 100.0], 0.1, free)[1];
        }
        assert!((hi - 100.0).abs() < 0.01, "{}", hi);
        assert!(!eased.settling([0.0, 100.0]));
    }

    #[test]
    fn pinned_bounds_are_not_eased() {
        let mut eased = EasedBounds::default();
        eased.update([0.0, 10_000.0], 0.1, (true, true));
        assert_eq!(
            eased.update([0.0, 5_000.0], 0.1, (true, true)),
            [0.0, 5_000.0]
        );
    }

    #[test]
    fn ticks_are_nice_and_inside_the_bounds() {
        let (ticks, step) = App::y_ticks([900.0, 2_200.0], 18);
        assert_eq!(step, 500.0);
        assert_eq!(ticks, vec![1_000.0, 1_500.0, 2_000.0]);
        let (ticks, _) = App::y_ticks([0.0, 100_000.0], 20);
        assert_eq!(ticks.first(), Some(&0.0));
        assert_eq!(ticks.last(), Some(&100_000.0));
    }

    #[test]
    fn recording_counts_and_orders_samples() {
        let mut host = Host::new("h".into(), None, Color::Reset, (0, 0, 0));
        host.record(10.0, Some(Duration::from_millis(5)));
        host.record(12.0, Some(Duration::from_millis(7)));
        // A loss reported late, back-dated to before the last reply, slots in between.
        host.record(11.0, None);
        assert_eq!((host.session.sent, host.session.lost), (3, 1));
        assert_eq!((host.session.min, host.session.max), (5_000.0, 7_000.0));
        assert_eq!(host.session.mean(), Some(6_000.0));
        assert_eq!(host.session.jitter(), Some(2_000.0));
        assert_eq!(host.session.last_reply, Some(12.0));
        assert_eq!(host.samples[0], (10.0, 5_000.0));
        assert!(host.samples[1].1.is_nan() && host.samples[1].0 == 11.0);
        assert_eq!(host.samples[2], (12.0, 7_000.0));
    }

    #[test]
    fn trimming_keeps_one_sample_before_the_cut() {
        let mut host = Host::new("h".into(), None, Color::Reset, (0, 0, 0));
        for t in 0..10 {
            host.samples.push((t as f64, 1.0));
        }
        host.trim(5.0);
        assert_eq!(host.samples.first().map(|s| s.0), Some(4.0));
        assert_eq!(host.window([5.0, 7.0]).len(), 3);
    }

    fn host_with(replies: &[(f64, Option<u64>)]) -> Host {
        let mut host = Host::new("h".into(), None, Color::Reset, (0, 0, 0));
        for &(at, ms) in replies {
            host.record(at, ms.map(Duration::from_millis));
        }
        host
    }

    #[test]
    fn grades_are_relative_to_the_baseline() {
        // A 10 ms host: +10 ms is elevated, +30 high, +100 critical.
        assert_eq!(grade(15_000.0, 10_000.0), 0);
        assert_eq!(grade(20_000.0, 10_000.0), 1);
        assert_eq!(grade(45_000.0, 10_000.0), 2);
        assert_eq!(grade(110_000.0, 10_000.0), 3);
        // A 200 ms path is allowed proportionally more.
        assert_eq!(grade(240_000.0, 200_000.0), 0);
        assert_eq!(grade(260_000.0, 200_000.0), 1);
    }

    #[test]
    fn statuses() {
        let interval = 0.2;
        assert_eq!(host_with(&[]).status(1.0, interval), Status::Waiting);
        let steady: Vec<(f64, Option<u64>)> = (0..50).map(|i| (i as f64 * 0.2, Some(10))).collect();
        assert_eq!(host_with(&steady).status(10.0, interval), Status::Steady);
        let mut lossy = steady.clone();
        lossy.push((9.9, None));
        lossy.push((10.0, Some(10)));
        assert_eq!(host_with(&lossy).status(10.1, interval), Status::Lossy);
        // Quiet for longer than three seconds.
        assert_eq!(host_with(&steady).status(14.0, interval), Status::Down);
        let mut slow = steady.clone();
        slow.extend((0..5).map(|i| (10.0 + i as f64 * 0.2, Some(40))));
        assert_eq!(host_with(&slow).status(11.0, interval), Status::Slow);
        let mut failed = host_with(&steady);
        failed.state = HostState::Failed("boom".into());
        assert_eq!(failed.status(10.0, interval), Status::Failed);
    }

    #[test]
    fn readouts() {
        let interval = 0.2;
        assert_eq!(host_with(&[]).readout(0.0, interval), Readout::Waiting);
        // The median of the last second, not just the newest reply.
        let host = host_with(&[
            (0.2, Some(10)),
            (0.4, Some(30)),
            (0.6, Some(12)),
            (0.8, Some(11)),
        ]);
        assert_eq!(host.readout(0.8, interval), Readout::Value(11_000.0));
        let lost = host_with(&[(0.2, Some(10)), (0.4, None)]);
        assert_eq!(lost.readout(0.5, interval), Readout::Lost);
        assert!(
            matches!(lost.readout(9.0, interval), Readout::NoReply(s) if (s - 8.8).abs() < 1e-9)
        );
        // Slow intervals: the newest reply as it is.
        let host = host_with(&[(1.0, Some(10)), (2.0, Some(30))]);
        assert_eq!(host.readout(2.0, 1.0), Readout::Value(30_000.0));
    }

    #[test]
    fn resetting_forgets_the_session() {
        let mut host = host_with(&(0..20).map(|i| (i as f64, Some(10))).collect::<Vec<_>>());
        assert!(host.baseline().is_some());
        host.reset(20.0);
        assert_eq!(host.session.sent, 0);
        assert!(host.baseline().is_none());
        assert!(host.history.is_empty());
        assert_eq!(host.samples.len(), 20, "the chart keeps its samples");
    }
}

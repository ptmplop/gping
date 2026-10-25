//! Application state: the hosts being watched and what is derived from their samples.

use crate::format::nice_step;
use crate::history::History;
use crate::stats::Summary;
use crate::theme::Rgb;
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

pub struct Host {
    /// The host or command as the user gave it (after expanding cloud shorthands).
    pub name: String,
    /// The address the name resolved to, when that adds anything to the name.
    pub address: Option<String>,
    /// Line colour: the user's `--color` choice or one from the palette.
    pub color: Color,
    /// An RGB approximation of `color`, for fills and tints.
    pub tint: Rgb,
    /// `(seconds since start, latency in microseconds or NaN when lost)`.
    pub samples: Vec<(f64, f64)>,
    /// The whole session, which outlives the samples trimmed out of `samples`.
    pub session: Session,
    pub history: History,
    pub state: HostState,
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
            last_reply: None,
        }
    }

    pub fn replies(&self) -> u64 {
        self.sent - self.lost
    }

    pub fn mean(&self) -> Option<f64> {
        (self.replies() > 0).then(|| self.sum / self.replies() as f64)
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
            samples: Vec::with_capacity(512),
            session: Session::new(),
            history: History::new(0.0),
            state: HostState::Running,
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
                session.last_reply = Some(at);
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
    smoothed: Option<[f64; 2]>,
}

impl App {
    pub fn new(hosts: Vec<Host>, window: f64, started: f64, yrange: (Option<f64>, Option<f64>)) -> App {
        App {
            hosts,
            window,
            started,
            paused_at: None,
            yrange,
            smoothed: None,
        }
    }

    pub fn record(&mut self, host: usize, at: f64, reply: Option<Duration>) {
        if let Some(host) = self.hosts.get_mut(host) {
            host.record(at, reply);
        }
    }

    /// The time span on screen. Until a full window has passed the axis starts at launch
    /// time and fills from the left; after that it scrolls with the clock.
    pub fn x_bounds(&self, now: f64) -> [f64; 2] {
        let end = self.paused_at.unwrap_or(now);
        if end - self.started < self.window {
            [self.started, self.started + self.window]
        } else {
            [end - self.window, end]
        }
    }

    /// Bounds for the latency axis, from the samples on screen. A bound pinned with
    /// --ymin/--ymax is used verbatim; a derived one gets 10% of headroom so the line
    /// doesn't sit on the edge of the plot.
    pub fn y_bounds(&self, x_bounds: [f64; 2]) -> [f64; 2] {
        let (ymin, ymax) = self.yrange;
        let mut data = self
            .hosts
            .iter()
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

    /// [`App::y_bounds`], eased over time so the axis doesn't jump around as spikes come
    /// and go: it widens immediately (nothing is ever drawn off-scale for long) and narrows
    /// gradually. `dt` is the time since the last frame, in seconds.
    pub fn smoothed_y_bounds(&mut self, target: [f64; 2], dt: f64) -> [f64; 2] {
        let pinned = (self.yrange.0.is_some(), self.yrange.1.is_some());
        let bounds = match self.smoothed {
            None => target,
            Some([lo, hi]) => {
                let k = 1.0 - (-dt.max(0.0) / 0.45).exp();
                let lo = if pinned.0 || target[0] < lo { target[0] } else { lo + (target[0] - lo) * k };
                let hi = if pinned.1 || target[1] > hi { target[1] } else { hi + (target[1] - hi) * k };
                if hi > lo {
                    [lo, hi]
                } else {
                    target
                }
            }
        };
        self.smoothed = Some(bounds);
        bounds
    }

    /// Evenly spaced "nice" values to label and draw guides at, within `bounds`.
    pub fn y_ticks(bounds: [f64; 2], rows: u16) -> (Vec<f64>, f64) {
        let [lo, hi] = bounds;
        let step = nice_step(hi - lo, (rows as usize / 3).clamp(2, 6));
        let mut ticks = Vec::new();
        let mut tick = (lo / step).ceil() * step;
        while tick <= hi + step * 1e-9 && ticks.len() < 64 {
            ticks.push(tick);
            tick += step;
        }
        (ticks, step)
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
            host.samples.push((now - (samples.len() - 1 - i) as f64, *value));
        }
        (App::new(vec![host], 30.0, now - 100.0, yrange), now)
    }

    fn bounds(samples: &[f64], yrange: (Option<f64>, Option<f64>)) -> [f64; 2] {
        let (app, now) = app_with(samples, yrange);
        app.y_bounds(app.x_bounds(now))
    }

    // --ymin/--ymax are exact requests: the 10% headroom added to automatically derived
    // bounds must not push the axis past what the user asked for.
    #[test]
    fn user_supplied_y_bounds_are_used_verbatim() {
        assert_eq!(bounds(&[1_000.0, 9_000.0], (Some(0.0), Some(10_000.0))), [0.0, 10_000.0]);
    }

    #[test]
    fn automatic_y_bounds_keep_their_headroom() {
        assert_eq!(bounds(&[1_000.0, 2_000.0], (None, None)), [900.0, 2_200.0]);
    }

    // Only the pinned end is fixed; the other one still scales to the data.
    #[test]
    fn one_sided_y_bounds() {
        assert_eq!(bounds(&[1_000.0, 2_000.0], (Some(0.0), None)), [0.0, 2_200.0]);
        assert_eq!(bounds(&[1_000.0, 2_000.0], (None, Some(10_000.0))), [900.0, 10_000.0]);
    }

    #[test]
    fn y_bounds_with_a_single_sample() {
        assert_eq!(bounds(&[5_000.0], (Some(0.0), Some(10_000.0))), [0.0, 10_000.0]);
    }

    // Before any ping comes back there is nothing to scale to, so the axis has to fall
    // back to something with a non-zero height rather than to infinities.
    #[test]
    fn y_bounds_without_any_data() {
        assert_eq!(bounds(&[], (None, None)), [0.0, 1_000.0]);
        assert_eq!(bounds(&[], (Some(2_000.0), Some(8_000.0))), [2_000.0, 8_000.0]);
        // Lost pings are not data either.
        assert_eq!(bounds(&[f64::NAN, f64::NAN], (None, None)), [0.0, 1_000.0]);
    }

    // --ymin can sit above every sample we have, which would otherwise give an
    // upside-down axis: the pinned bound wins and the axis keeps 1ms of height.
    #[test]
    fn y_min_pinned_above_the_data() {
        assert_eq!(bounds(&[1_000.0, 2_000.0], (Some(50_000.0), None)), [50_000.0, 51_000.0]);
    }

    // Every sample sits above a pinned --ymax: the derived minimum (90% of the smallest one)
    // would land above the maximum, and used to push the user's bound up to match, leaving
    // a 1ms band below all the data and an empty chart.
    #[test]
    fn y_max_pinned_below_the_data_is_kept() {
        assert_eq!(bounds(&[100_000.0, 110_000.0], (None, Some(50_000.0))), [0.0, 50_000.0]);
    }

    // Samples that have scrolled off screen must not keep stretching the axis.
    #[test]
    fn y_bounds_only_consider_the_visible_window() {
        let (mut app, now) = app_with(&[1_000.0, 2_000.0], (None, None));
        app.hosts[0].samples.insert(0, (now - 500.0, 90_000.0));
        assert_eq!(app.y_bounds(app.x_bounds(now)), [900.0, 2_200.0]);
    }

    #[test]
    fn x_bounds_fill_from_the_left_then_scroll() {
        let (app, _) = app_with(&[], (None, None));
        let started = app.started;
        assert_eq!(app.x_bounds(started + 5.0), [started, started + 30.0]);
        assert_eq!(app.x_bounds(started + 50.0), [started + 20.0, started + 50.0]);
    }

    #[test]
    fn pausing_freezes_the_view() {
        let (mut app, _) = app_with(&[], (None, None));
        let started = app.started;
        app.paused_at = Some(started + 40.0);
        assert_eq!(app.x_bounds(started + 90.0), [started + 10.0, started + 40.0]);
    }

    #[test]
    fn smoothing_widens_at_once_and_narrows_gradually() {
        let (mut app, _) = app_with(&[], (None, None));
        assert_eq!(app.smoothed_y_bounds([0.0, 100.0], 0.1), [0.0, 100.0]);
        // A spike: the top moves straight up.
        assert_eq!(app.smoothed_y_bounds([0.0, 500.0], 0.1), [0.0, 500.0]);
        // It scrolls away: the top eases back down, not all the way in one frame.
        let [_, hi] = app.smoothed_y_bounds([0.0, 100.0], 0.1);
        assert!(hi < 500.0 && hi > 100.0, "{}", hi);
        let mut hi = hi;
        for _ in 0..100 {
            hi = app.smoothed_y_bounds([0.0, 100.0], 0.1)[1];
        }
        assert!((hi - 100.0).abs() < 0.01, "{}", hi);
    }

    #[test]
    fn pinned_bounds_are_not_smoothed() {
        let (mut app, _) = app_with(&[], (Some(0.0), Some(10_000.0)));
        app.smoothed_y_bounds([0.0, 10_000.0], 0.1);
        assert_eq!(app.smoothed_y_bounds([0.0, 10_000.0], 0.1), [0.0, 10_000.0]);
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
}

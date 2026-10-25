//! The screen: a lane per host, each with its trace, its loss rail and a card with the
//! current figures, under a title bar and over a time axis and a bar of keys.

mod card;
mod chrome;
mod draw;
mod help;
mod lane;
mod layout;

use crate::app::{App, EasedBounds, Readout, Status};
use crate::cli::{Args, RstBehaviour};
use crate::color_depth::ColorDepth;
use crate::stats::{percentile, Summary};
use crate::theme::Theme;
use chrono::{DateTime, Duration as ChronoDuration, Local};
use layout::Body;
use std::time::Duration;
use tui::crossterm::event::{KeyCode, KeyEvent};
use tui::layout::Rect;
use tui::Frame;

pub struct UiOptions {
    pub simple_graphics: bool,
    pub vertical_margin: u16,
    pub horizontal_margin: u16,
    /// Seconds between probes.
    pub interval: f64,
    /// How the hosts are probed, for the title bar: "ICMP", "TCP :443", "command".
    pub mode: String,
    pub family: Option<&'static str>,
    pub interface: Option<String>,
    pub depth: ColorDepth,
    /// Whether to paint gping's own background.
    pub paint: bool,
}

impl UiOptions {
    pub fn from_args(args: &Args, interval: Duration, depth: ColorDepth) -> UiOptions {
        let mode = if args.cmd {
            "command".to_string()
        } else if args.tcp {
            match args.tcp_rst {
                RstBehaviour::Pong => format!("TCP :{}", args.tcp_port),
                RstBehaviour::Drop => format!("TCP :{} rst=drop", args.tcp_port),
            }
        } else {
            "ICMP".to_string()
        };
        #[cfg(not(target_os = "windows"))]
        let interface = args.interface.clone();
        #[cfg(target_os = "windows")]
        let interface = None;
        UiOptions {
            simple_graphics: args.simple_graphics,
            vertical_margin: args.vertical_margin,
            horizontal_margin: args.horizontal_margin,
            interval: interval.as_secs_f64(),
            mode,
            family: if args.ipv4 {
                Some("IPv4")
            } else if args.ipv6 {
                Some("IPv6")
            } else {
                None
            },
            interface,
            depth,
            paint: std::env::var("GPING_BG").map_or(true, |v| v != "none"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Changed,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    /// Each lane fits its own host.
    PerHost,
    /// Every lane on one scale, for comparing hosts.
    Shared,
}

/// How long a status badge blinks for when a host starts losing pings.
const BLINK_SECONDS: f64 = 1.5;

/// Samples are kept for at least this long, whatever the window: a host's status looks back
/// ten seconds, and lost pings arrive seconds late, dated back to when they were sent.
const KEEP_SECONDS: f64 = 60.0;

pub struct Ui {
    options: UiOptions,
    theme: Theme,
    scale: ScaleMode,
    hidden: Vec<bool>,
    help: bool,
    eased: Vec<EasedBounds>,
    shared: EasedBounds,
    /// Each host's status, and when it took it.
    statuses: Vec<(Status, f64)>,
    last_draw: Option<f64>,
    /// Seconds per dot column of the plots, and the slot and second last drawn.
    slot: f64,
    drawn_slot: i64,
    drawn_second: i64,
    /// Something is moving (a blink, an easing axis) until then.
    busy_until: f64,
}

/// What the widgets need to know about the frame being drawn.
pub struct Scene<'a> {
    pub theme: &'a Theme,
    pub options: &'a UiOptions,
    pub app: &'a App,
    pub now: f64,
    pub x_bounds: [f64; 2],
    /// The final frame, left in the scrollback: the session, not the moment.
    pub closing: bool,
    pub scale: ScaleMode,
    pub wall_now: DateTime<Local>,
}

impl Scene<'_> {
    /// Wall-clock time for a time on gping's clock.
    pub fn wall(&self, t: f64) -> DateTime<Local> {
        self.wall_now - ChronoDuration::milliseconds(((self.now - t) * 1000.0) as i64)
    }

    pub fn paused(&self) -> bool {
        self.app.paused_at.is_some() && !self.closing
    }
}

/// Figures for one host, worked out once per frame.
pub struct HostView {
    pub index: usize,
    pub summary: Option<Summary>,
    pub status: Status,
    pub readout: Readout,
    pub baseline: Option<f64>,
    /// The status badge is drawn inverted in this frame (it blinks on bad news).
    pub blink: bool,
}

impl Ui {
    pub fn new(options: UiOptions) -> Ui {
        let theme = Theme::new(options.depth, options.paint);
        Ui {
            options,
            theme,
            scale: ScaleMode::PerHost,
            hidden: Vec::new(),
            help: false,
            eased: Vec::new(),
            shared: EasedBounds::default(),
            statuses: Vec::new(),
            last_draw: None,
            slot: 1.0,
            drawn_slot: i64::MIN,
            drawn_second: i64::MIN,
            busy_until: 0.0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, app: &mut App, now: f64) -> KeyOutcome {
        if self.help {
            // Any key closes the help; q, Esc and Ctrl-C still quit (run.rs sees them first).
            self.help = false;
            return KeyOutcome::Changed;
        }
        match key.code {
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                app.paused_at = match app.paused_at {
                    Some(_) => None,
                    None => Some(now),
                };
            }
            KeyCode::Char('s') => {
                self.scale = match self.scale {
                    ScaleMode::PerHost => ScaleMode::Shared,
                    ScaleMode::Shared => ScaleMode::PerHost,
                };
            }
            KeyCode::Char('0') => self.hidden.iter_mut().for_each(|h| *h = false),
            KeyCode::Char(c @ '1'..='9') => {
                let host = c as usize - '1' as usize;
                if host >= app.hosts.len() {
                    return KeyOutcome::Ignored;
                }
                self.hidden.resize(app.hosts.len(), false);
                let showing = self.hidden.iter().filter(|h| !**h).count();
                // The last host on show stays.
                if self.hidden[host] || showing > 1 {
                    self.hidden[host] = !self.hidden[host];
                }
            }
            KeyCode::Char('r') => app.hosts.iter_mut().for_each(|h| h.reset(now)),
            KeyCode::Char('?') => self.help = true,
            _ => return KeyOutcome::Ignored,
        }
        KeyOutcome::Changed
    }

    /// Whether time alone has changed what's on screen: the plot has scrolled by a dot, the
    /// clock has ticked, or something is animating.
    pub fn wants_redraw(&self, app: &App, now: f64) -> bool {
        let scrolled =
            app.paused_at.is_none() && (now / self.slot).floor() as i64 != self.drawn_slot;
        scrolled || now.floor() as i64 != self.drawn_second || now < self.busy_until
    }

    /// Draws the screen and returns the row to leave the cursor on afterwards.
    pub fn draw(&mut self, frame: &mut Frame, app: &mut App, now: f64, closing: bool) -> u16 {
        let full = frame.area();
        let (vm, hm) = (self.options.vertical_margin, self.options.horizontal_margin);
        let area = Rect {
            x: full.x + hm.min(full.width / 2),
            y: full.y + vm.min(full.height / 2),
            width: full.width.saturating_sub(hm.saturating_mul(2)),
            height: full.height.saturating_sub(vm.saturating_mul(2)),
        };
        let dt = self.last_draw.map_or(0.0, |last| (now - last).max(0.0));
        self.last_draw = Some(now);

        let x_bounds = app.x_bounds(now);
        for host in &mut app.hosts {
            host.trim(x_bounds[0].min(now - KEEP_SECONDS));
        }
        let n = app.hosts.len();
        self.hidden.resize(n, false);
        self.eased.resize(n, EasedBounds::default());
        self.statuses.resize(n, (Status::Waiting, now));

        let views: Vec<HostView> = (0..n)
            .map(|i| self.view(app, i, now, x_bounds, closing))
            .collect();
        // The final frame reports on every host, hidden or not.
        let visible: Vec<usize> = (0..n).filter(|&i| closing || !self.hidden[i]).collect();
        let plan = layout::plan(area, &visible);

        let buf = frame.buffer_mut();
        draw::fill(buf, area, self.theme.base);
        let scene = Scene {
            theme: &self.theme,
            options: &self.options,
            app,
            now,
            x_bounds,
            closing,
            scale: self.scale,
            wall_now: Local::now(),
        };

        if let Some(rect) = plan.masthead {
            chrome::masthead(buf, rect, &scene, &views, &visible);
        }
        let mut busy = false;
        match &plan.body {
            Body::Lanes(lanes) => {
                let shared_target = self.shared_target(app, &views, &visible, x_bounds);
                let pinned = (app.yrange.0.is_some(), app.yrange.1.is_some());
                // Not a closure: in edition 2018 it would borrow all of `self`.
                let shared = match shared_target {
                    Some(target) => Some(self.shared.update(target, dt, pinned)),
                    None => None,
                };
                for lane in lanes {
                    let view = &views[lane.host];
                    let bounds = match (self.scale, shared) {
                        (ScaleMode::Shared, Some(bounds)) => {
                            busy |= self.shared.settling(shared_target.unwrap_or(bounds));
                            bounds
                        }
                        _ => {
                            let host = &app.hosts[lane.host];
                            let typical =
                                view.baseline.or(view.summary.map(|s| s.p50)).unwrap_or(0.0);
                            let target =
                                app.with_span_floor(app.y_bounds_of([host], x_bounds), typical);
                            let eased = self.eased[lane.host].update(target, dt, pinned);
                            busy |= self.eased[lane.host].settling(target);
                            eased
                        }
                    };
                    let drawn = lane::draw(buf, lane, &scene, view, bounds);
                    if drawn.slot > 0.0 {
                        self.slot = drawn.slot;
                    }
                    card::draw(buf, lane.card, lane.kind, &scene, view);
                }
            }
            Body::Lines(lines) => {
                for &(host, rect) in lines {
                    chrome::host_line(buf, rect, &scene, &views[host]);
                }
            }
        }
        if let Some((rect, count)) = plan.more {
            chrome::more(buf, rect, &scene, count);
        }
        if let Some(rect) = plan.axis {
            chrome::time_axis(buf, rect, &scene);
        }
        if let Some(rect) = plan.footer {
            chrome::footer(buf, rect, &scene, &views);
        }
        if self.help && !closing {
            let room = match &plan.body {
                Body::Lanes(lanes) if !lanes.is_empty() => Rect {
                    width: lanes[0].card.x.saturating_sub(area.x),
                    ..area
                },
                _ => area,
            };
            help::draw(buf, area, room, &scene);
        }

        if views.iter().any(|v| v.blink)
            || views
                .iter()
                .any(|v| self.statuses[v.index].1 + BLINK_SECONDS > now)
        {
            busy = true;
        }
        self.busy_until = if busy { now + 0.1 } else { 0.0 };
        self.drawn_slot = (now / self.slot).floor() as i64;
        self.drawn_second = now.floor() as i64;

        // The cursor goes under the frame: on the blank margin row if there is one, otherwise
        // on the last row, which the caller then pushes up with a newline.
        if vm > 0 {
            area.bottom().min(full.bottom().saturating_sub(1))
        } else {
            full.bottom().saturating_sub(1)
        }
    }

    fn view(
        &mut self,
        app: &App,
        index: usize,
        now: f64,
        x_bounds: [f64; 2],
        closing: bool,
    ) -> HostView {
        let host = &app.hosts[index];
        let interval = self.options.interval;
        let status = host.status(now, interval);
        let (previous, since) = self.statuses[index];
        if status != previous {
            self.statuses[index] = (status, now);
        }
        let since = if status != previous { now } else { since };
        let alarming = matches!(status, Status::Lossy | Status::Down | Status::Failed);
        let age = now - since;
        let blink =
            alarming && !closing && age < BLINK_SECONDS && (age * 4.0).floor() as i64 % 2 == 0;
        HostView {
            index,
            summary: host.summary(x_bounds),
            status,
            readout: host.readout(now, interval),
            baseline: host.baseline(),
            blink,
        }
    }

    /// The bounds every lane shares in shared mode.
    fn shared_target(
        &self,
        app: &App,
        views: &[HostView],
        visible: &[usize],
        x_bounds: [f64; 2],
    ) -> Option<[f64; 2]> {
        if self.scale != ScaleMode::Shared {
            return None;
        }
        let mut typical: Vec<f64> = visible
            .iter()
            .filter_map(|&i| views[i].baseline.or(views[i].summary.map(|s| s.p50)))
            .filter(|v| v.is_finite())
            .collect();
        let typical = if typical.is_empty() {
            0.0
        } else {
            percentile(&mut typical, 0.5)
        };
        let hosts = visible.iter().map(|&i| &app.hosts[i]);
        Some(app.with_span_floor(app.y_bounds_of(hosts, x_bounds), typical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Host, HostState};
    use crate::theme::SERIES;
    use tui::backend::TestBackend;
    use tui::style::Color;
    use tui::Terminal;

    fn options(depth: ColorDepth) -> UiOptions {
        UiOptions {
            simple_graphics: false,
            vertical_margin: 1,
            horizontal_margin: 0,
            interval: 0.2,
            mode: "ICMP".into(),
            family: None,
            interface: None,
            depth,
            paint: true,
        }
    }

    /// An app with `n` hosts that have been pinged for 40 seconds: steady replies with a
    /// spike or two, a lost ping now and then and, for the third host, a dead spell.
    fn app(n: usize) -> App {
        let hosts = (0..n)
            .map(|i| {
                let c = SERIES[i % SERIES.len()];
                let mut host = Host::new(
                    format!("host-{i}.example"),
                    Some(format!("192.0.2.{i}")),
                    Color::Rgb(c.0, c.1, c.2),
                    c,
                );
                for k in 0..200 {
                    let at = k as f64 * 0.2;
                    let lost = k % 37 == 5 || (i == 2 && (120..150).contains(&k));
                    let ms = 10 + (i as u64 * 7) + (k % 5) + if k % 41 == 3 { 60 } else { 0 };
                    host.record(at, (!lost).then(|| Duration::from_millis(ms)));
                }
                host
            })
            .collect();
        App::new(hosts, 30.0, 0.0, (None, None))
    }

    fn render(ui: &mut Ui, app: &mut App, w: u16, h: u16, closing: bool) -> tui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|frame| {
                ui.draw(frame, app, 40.0, closing);
                ui.options.depth.adapt_buffer(frame.buffer_mut());
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buf: &tui::buffer::Buffer) -> String {
        let w = buf.area.width as usize;
        buf.content
            .chunks(w.max(1))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    // No size, host count or mode may panic: every widget clips to what it was given.
    #[test]
    fn every_size_renders() {
        for n in [1, 2, 3, 7] {
            let mut app = app(n);
            let mut ui = Ui::new(options(ColorDepth::TrueColor));
            for w in (0..=160).step_by(7) {
                for h in (0..=50).step_by(3) {
                    ui.help = (w + h) % 2 == 0;
                    render(&mut ui, &mut app, w, h, false);
                    render(&mut ui, &mut app, w, h, true);
                }
            }
        }
    }

    #[test]
    fn every_colour_depth_renders() {
        for depth in [
            ColorDepth::TrueColor,
            ColorDepth::Ansi256,
            ColorDepth::Ansi16,
            ColorDepth::Mono,
        ] {
            let mut app = app(3);
            let mut ui = Ui::new(options(depth));
            let buf = render(&mut ui, &mut app, 120, 34, false);
            if depth == ColorDepth::Mono {
                assert!(buf
                    .content
                    .iter()
                    .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
            }
            if depth == ColorDepth::Ansi16 {
                // Nothing a theme can make invisible.
                assert!(buf
                    .content
                    .iter()
                    .all(|c| c.fg != Color::Black && c.fg != Color::DarkGray));
            }
        }
    }

    #[test]
    fn the_main_screen_has_everything() {
        let mut app = app(3);
        let mut ui = Ui::new(options(ColorDepth::TrueColor));
        let screen = text(&render(&mut ui, &mut app, 120, 34, false));
        // Every host lost a ping in the last ten seconds, so all three are lossy.
        for expected in [
            "gping",
            "ICMP",
            "host-0.example",
            "host-2.example",
            "q quit",
            "now",
            "✕ 3 lossy",
            "session",
        ] {
            assert!(
                screen.contains(expected),
                "{} is missing from\n{}",
                expected,
                screen
            );
        }
        // Lost pings are marked on the rails.
        assert!(screen.contains('✕'));
    }

    fn screen(depth: ColorDepth, app: &mut App, w: u16, h: u16, closing: bool) -> String {
        text(&render(&mut Ui::new(options(depth)), app, w, h, closing))
    }

    fn row(screen: &str, y: usize) -> &str {
        screen.lines().nth(y).unwrap_or_default()
    }

    /// A host whose ping `k`, taken every 0.2s for 40 seconds, got the reply `reply(k)`.
    fn host(i: usize, name: &str, reply: impl Fn(usize) -> Option<u64>) -> Host {
        let c = SERIES[i % SERIES.len()];
        let mut host = Host::new(name.into(), None, Color::Rgb(c.0, c.1, c.2), c);
        for k in 0..200 {
            host.record(k as f64 * 0.2, reply(k).map(Duration::from_millis));
        }
        host
    }

    // However small the cards, the final frame reports on the session: host 2 lost 36 of its
    // 200 pings (18%), 34 of them in the last 30 seconds (23% of those).
    #[test]
    fn the_final_frame_reports_the_session_at_every_size() {
        for (w, h, expected) in [
            (120, 34, "lost 36 (18%)"),
            (80, 24, "lost 36 (18%)"),
            (60, 16, "18%"),
        ] {
            let report = screen(ColorDepth::TrueColor, &mut app(3), w, h, true);
            assert!(report.contains(expected), "{}x{}:\n{}", w, h, report);
            assert!(!report.contains("23%"), "{}x{}:\n{}", w, h, report);
        }
    }

    #[test]
    fn a_host_that_never_answered_is_reported_as_such() {
        let mut app = app(2);
        app.hosts[1] = host(1, "dead.example", |_| None);
        for (w, h) in [(120, 34), (80, 24), (60, 16)] {
            let report = screen(ColorDepth::TrueColor, &mut app, w, h, true);
            assert!(report.contains("no replies"), "{}x{}:\n{}", w, h, report);
            assert!(!report.contains("waiting"), "{}x{}:\n{}", w, h, report);
        }
    }

    // A failed pinger has no samples, but "waiting…" would be wrong: it says why it failed.
    #[test]
    fn a_failed_host_says_why() {
        let mut app = app(2);
        app.hosts[1] = Host::new("2606:4700:4700::1111".into(), None, Color::Reset, (0, 0, 0));
        app.hosts[1].state = HostState::Failed("ping6: UDP connect: No route to host".into());
        let live = screen(ColorDepth::TrueColor, &mut app, 120, 34, false);
        for expected in ["failed", "No route", "2606:4700:4700::1111"] {
            assert!(
                live.contains(expected),
                "{} is missing from\n{}",
                expected,
                live
            );
        }
        assert!(!live.contains("waiting"), "{}", live);
    }

    #[test]
    fn hidden_hosts_are_still_in_the_report() {
        let mut app = app(3);
        let mut ui = Ui::new(options(ColorDepth::TrueColor));
        ui.hidden = vec![false, true, false];
        let live = text(&render(&mut ui, &mut app, 120, 34, false));
        assert!(
            !live.contains("host-1.example") && live.contains("1 hidden"),
            "{}",
            live
        );
        let report = text(&render(&mut ui, &mut app, 120, 34, true));
        assert!(report.contains("host-1.example"), "{}", report);
    }

    // When the title bar runs out of room, a down host is the last thing to go.
    #[test]
    fn a_narrow_title_bar_keeps_the_bad_news() {
        let mut app = app(3);
        app.hosts[0] = host(0, "a", |k| (k < 150).then_some(10));
        app.hosts[1] = host(1, "b", |k| (k != 195).then_some(10));
        app.hosts[2] = host(2, "c", |_| Some(10));
        let mut lost_ok = false;
        for w in 30..=80 {
            let title = row(&screen(ColorDepth::TrueColor, &mut app, w, 34, false), 1).to_string();
            assert!(!title.contains("ok") || title.contains("down"), "{}", title);
            lost_ok |= title.contains("down") && !title.contains("ok");
        }
        assert!(lost_ok, "at some width only the bad news fits");
    }

    #[test]
    fn the_final_footer_drops_whole_pieces() {
        let report = screen(ColorDepth::TrueColor, &mut app(3), 40, 12, true);
        assert_eq!(
            row(&report, 10).trim(),
            "600 sent  ·  48 lost (8%)",
            "{}",
            report
        );
    }

    #[test]
    fn the_heat_legend_only_shows_where_heat_does() {
        let mut app = app(3);
        assert!(
            screen(ColorDepth::TrueColor, &mut app, 120, 34, false).contains("slower than usual")
        );
        assert!(screen(ColorDepth::TrueColor, &mut app, 80, 24, false).contains("━━━ slow"));
        assert!(!screen(ColorDepth::Mono, &mut app, 120, 34, false).contains("slower"));
    }

    // With too many hosts for lanes, each gets a line, and their figures line up whatever
    // each one's status.
    #[test]
    fn host_lines_keep_their_columns() {
        let mut app = app(10);
        app.hosts[3].state = HostState::Finished;
        let lines = screen(ColorDepth::TrueColor, &mut app, 80, 12, false);
        let columns: Vec<usize> = lines
            .lines()
            .filter_map(|line| line.chars().position(|c| c == '%'))
            .collect();
        assert!(columns.len() >= 5, "{}", lines);
        assert!(columns.windows(2).all(|w| w[0] == w[1]), "{}", lines);
    }

    #[test]
    fn huge_margins_leave_nothing_to_draw() {
        let mut opts = options(ColorDepth::TrueColor);
        opts.vertical_margin = 40_000;
        opts.horizontal_margin = 40_000;
        render(&mut Ui::new(opts), &mut app(2), 120, 34, false);
        render(
            &mut Ui::new(options(ColorDepth::TrueColor)),
            &mut app(2),
            120,
            34,
            true,
        );
    }

    // With a one-second window, a lost ping (dated back two seconds, to when it was sent) is
    // off the chart before it is known about, but the host is still lossy for ten seconds.
    #[test]
    fn a_short_window_still_knows_about_losses() {
        let mut app = app(1);
        app.window = 1.0;
        screen(ColorDepth::TrueColor, &mut app, 120, 34, false);
        assert_eq!(app.hosts[0].status(40.0, 0.2), Status::Lossy);
    }

    // `-s` draws a dot per cell, not braille's four, so the labels have to follow suit. On
    // 8 rows from 0 to 22 ms, braille puts 20 ms in the top row and a dot in the next.
    #[test]
    fn simple_graphics_labels_sit_on_their_gridlines() {
        let mut opts = options(ColorDepth::TrueColor);
        opts.simple_graphics = true;
        let mut app = app(1);
        app.yrange = (Some(0.0), Some(22_000.0));
        let lines = text(&render(&mut Ui::new(opts), &mut app, 120, 14, false));
        let mut labelled = 0;
        for line in lines.lines() {
            let cells: Vec<char> = line.chars().collect();
            if cells.get(5) == Some(&'┤') {
                labelled += 1;
                assert!(
                    cells[6..60].contains(&'·'),
                    "no gridline at {:?}\n{}",
                    line,
                    lines
                );
            }
        }
        assert!(labelled >= 3, "{}", lines);
    }

    #[test]
    fn the_final_frame_is_a_session_report() {
        let mut app = app(2);
        let mut ui = Ui::new(options(ColorDepth::TrueColor));
        let screen = text(&render(&mut ui, &mut app, 120, 34, true));
        assert!(screen.contains("stopped"));
        assert!(screen.contains("400 sent"), "{}", screen);
        assert!(
            !screen.contains("q quit"),
            "no key hints are left in the scrollback"
        );
    }
}

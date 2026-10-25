//! The screen. (Interim layout: the full design replaces the drawing below.)

use crate::app::App;
use crate::cli::Args;
use crate::format;
use crate::plot::{Marker, Plot, Scale, Series};
use std::time::Duration;
use tui::crossterm::event::{KeyCode, KeyEvent};
use tui::layout::Rect;
use tui::style::{Color, Style};
use tui::Frame;

pub struct UiOptions {
    pub simple_graphics: bool,
    pub vertical_margin: u16,
    pub horizontal_margin: u16,
}

impl UiOptions {
    pub fn from_args(args: &Args, _interval: Duration) -> UiOptions {
        UiOptions {
            simple_graphics: args.simple_graphics,
            vertical_margin: args.vertical_margin,
            horizontal_margin: args.horizontal_margin,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Changed,
    Ignored,
}

pub struct Ui {
    options: UiOptions,
    /// Seconds per dot column of the last plot drawn, and which slot it showed.
    slot: f64,
    drawn_slot: i64,
    drawn_second: i64,
}

impl Ui {
    pub fn new(options: UiOptions) -> Ui {
        Ui {
            options,
            slot: 1.0,
            drawn_slot: i64::MIN,
            drawn_second: i64::MIN,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, app: &mut App, now: f64) -> KeyOutcome {
        match key.code {
            KeyCode::Char(' ') => {
                app.paused_at = match app.paused_at {
                    Some(_) => None,
                    None => Some(now),
                };
                KeyOutcome::Changed
            }
            _ => KeyOutcome::Ignored,
        }
    }

    /// Whether time alone has changed what's on screen: the plot has scrolled by a dot, or
    /// the clock by a second.
    pub fn wants_redraw(&self, _app: &App, now: f64) -> bool {
        (now / self.slot).floor() as i64 != self.drawn_slot || now.floor() as i64 != self.drawn_second
    }

    /// Draws the screen and returns the row to leave the cursor on afterwards.
    pub fn draw(&mut self, frame: &mut Frame, app: &mut App, now: f64, _closing: bool) -> u16 {
        let full = frame.area();
        let area = Rect {
            x: full.x + self.options.horizontal_margin.min(full.width / 2),
            y: full.y + self.options.vertical_margin.min(full.height / 2),
            width: full.width.saturating_sub(2 * self.options.horizontal_margin),
            height: full.height.saturating_sub(2 * self.options.vertical_margin),
        };
        let buf = frame.buffer_mut();
        let x_bounds = app.x_bounds(now);
        for host in &mut app.hosts {
            host.trim(x_bounds[0] - app.window);
        }
        let mut y = area.y;
        for host in &app.hosts {
            if y >= area.bottom() {
                break;
            }
            let summary = host.summary(x_bounds);
            let line = match summary {
                Some(s) => format!(
                    "{}  last {}  avg {}  max {}  loss {}",
                    host.name,
                    format::latency(s.last),
                    format::latency(s.mean),
                    format::latency(s.max),
                    format::percent(s.loss())
                ),
                None => host.name.clone(),
            };
            buf.set_stringn(area.x, y, line, area.width as usize, Style::default().fg(host.color));
            y += 1;
        }
        let plot_area = Rect {
            y,
            height: area.bottom().saturating_sub(y),
            ..area
        };
        let y_bounds = app.y_bounds(x_bounds);
        let plot = Plot {
            series: app
                .hosts
                .iter()
                .map(|h| Series {
                    samples: &h.samples,
                    color: h.color,
                    heat: None,
                    glow: None,
                    head: None,
                })
                .collect(),
            x_bounds,
            y_bounds,
            scale: Scale::Linear,
            marker: if self.options.simple_graphics { Marker::Dot } else { Marker::Braille },
            grid: vec![],
            grid_color: Color::Gray,
            base: (0, 0, 0),
            curtain: Some((60, 10, 20)),
        };
        let drawn = plot.render(plot_area, buf);
        if drawn.slot > 0.0 {
            self.slot = drawn.slot;
        }
        self.drawn_slot = (now / self.slot).floor() as i64;
        self.drawn_second = now.floor() as i64;
        full.bottom().saturating_sub(1)
    }
}

//! One host's lane: its trace, with axis labels to the left, the loss rail underneath and a
//! histogram of where its latency usually sits.

use super::draw::put;
use super::layout::Lane;
use super::{HostView, Scene};
use crate::app::{grade, App};
use crate::format;
use crate::plot::{Drawn, Glow, Marker, Plot, Series};
use crate::theme::mix;
use tui::buffer::Buffer;
use tui::style::{Color, Modifier, Style};

/// The strongest tint under a line, and at the floor.
const FILL_TOP: f64 = 0.20;
const FILL_BOTTOM: f64 = 0.05;

pub fn draw(
    buf: &mut Buffer,
    lane: &Lane,
    scene: &Scene,
    view: &HostView,
    bounds: [f64; 2],
) -> Drawn {
    let t = scene.theme;
    let host = &scene.app.hosts[lane.host];
    let (ticks, step) = App::y_ticks(bounds, lane.plot.height);

    // Heat: the line warms through amber, orange and red where it's slower than the host
    // usually is. Not for a host whose colour was chosen with --color: that choice wins.
    let baseline = view.baseline.filter(|_| !host.chosen_color);
    let heat_colors = t.heat;
    let calm = host.color;
    let heat = move |value: f64| -> Option<Color> {
        let baseline = baseline?;
        match grade(value, baseline) {
            0 => None,
            g => Some(heat_colors[(g as usize - 1).min(2)]),
        }
    };
    let head = match host.color {
        Color::Rgb(..) => Some(Color::from(mix(host.tint, (255, 255, 255), 0.55))),
        other => Some(other),
    };
    let plot = Plot {
        series: vec![Series {
            samples: &host.samples,
            color: calm,
            heat: Some(&heat),
            glow: t.fills.then_some(Glow {
                tint: host.tint,
                top: FILL_TOP,
                bottom: FILL_BOTTOM,
            }),
            head: head.filter(|_| !scene.closing),
        }],
        x_bounds: scene.x_bounds,
        y_bounds: bounds,
        marker: if scene.options.simple_graphics {
            Marker::Dot
        } else {
            Marker::Braille
        },
        grid: ticks.clone(),
        grid_color: t.grid,
        base: t.base_rgb,
        curtain: t.curtain,
    };
    let drawn = plot.render(lane.plot, buf);

    gutter(buf, lane, scene, bounds, &ticks, step);
    rail(buf, lane, scene, &drawn);
    if let Some(rect) = lane.hist {
        histogram(buf, rect, scene, lane.host, bounds);
    }
    drawn
}

/// Tick labels down the left, on the rows their values sit on.
fn gutter(
    buf: &mut Buffer,
    lane: &Lane,
    scene: &Scene,
    [lo, hi]: [f64; 2],
    ticks: &[f64],
    step: f64,
) {
    let t = scene.theme;
    let rect = lane.gutter;
    let axis_x = rect.right().saturating_sub(1);
    let plot_rows = lane.plot.height;
    for y in rect.top()..rect.top() + plot_rows {
        put(
            buf,
            axis_x,
            y,
            "│",
            Style::default().fg(t.chrome),
            rect.right(),
        );
    }
    put(
        buf,
        axis_x,
        lane.rail.y,
        "└",
        Style::default().fg(t.chrome),
        rect.right(),
    );
    if plot_rows == 0 || hi <= lo {
        return;
    }
    let dots = plot_rows as f64 * 4.0 - 1.0;
    let mut last_row = None;
    for &tick in ticks.iter().rev() {
        let row = (((hi - tick) / (hi - lo) * dots).round() / 4.0).floor() as u16;
        if row >= plot_rows || last_row == Some(row) {
            continue;
        }
        last_row = Some(row);
        let y = rect.y + row;
        let label = format::tick(tick, step, hi);
        let x = axis_x
            .saturating_sub(label.chars().count() as u16)
            .max(rect.x);
        put(buf, x, y, &label, Style::default().fg(t.muted), axis_x);
        put(
            buf,
            axis_x,
            y,
            "┤",
            Style::default().fg(t.chrome),
            rect.right(),
        );
    }
}

/// The line under the plot, with a ✕ in each column that lost a ping and a caption on longer
/// outages.
fn rail(buf: &mut Buffer, lane: &Lane, scene: &Scene, drawn: &Drawn) {
    let t = scene.theme;
    let rect = lane.rail;
    let line = Style::default().fg(t.chrome);
    let mark = Style::default().fg(t.loss).add_modifier(Modifier::BOLD);
    let cross = if scene.options.simple_graphics {
        "×"
    } else {
        "✕"
    };
    // Dashed where gping wasn't running yet, so "no data" doesn't read as "all quiet".
    let [x0, x1] = scene.x_bounds;
    let started =
        rect.x as f64 + ((scene.app.started - x0) / (x1 - x0) * rect.width as f64).floor();
    for x in rect.left()..rect.right() {
        let glyph = if (x as f64) < started { "┄" } else { "─" };
        put(buf, x, rect.y, glyph, line, rect.right());
    }
    let lost: &[u16] = drawn.lost.first().map_or(&[], |l| l.as_slice());
    for &column in lost {
        let x = rect.x + column;
        put(buf, x, rect.y, cross, mark, rect.right());
        if let Some(curtain) = t.curtain {
            buf[(x, rect.y)].set_bg(curtain);
        }
    }

    // Outages of a second or more get a caption, where the line has room for it.
    let host = &scene.app.hosts[lane.host];
    let samples = host.window(scene.x_bounds);
    let interval = scene.options.interval;
    let mut run: Option<(f64, f64)> = None;
    let mut runs = Vec::new();
    for &(at, value) in samples {
        match (value.is_nan(), run) {
            (true, None) => run = Some((at, at)),
            (true, Some((start, _))) => run = Some((start, at)),
            (false, Some(r)) => {
                runs.push(r);
                run = None;
            }
            (false, None) => {}
        }
    }
    runs.extend(run);
    let column = |t: f64| ((t - x0) / (x1 - x0) * rect.width as f64).floor() as i64 + rect.x as i64;
    for (start, end) in runs {
        let seconds = end - start + interval;
        if seconds < 1.0 {
            continue;
        }
        // The caption starts just after the outage's first ✕ and may cover the rest of its
        // marks (the curtain still shows how long it was), but nothing past it.
        let caption = format!(" lost {} ", format::duration(seconds));
        let width = caption.chars().count() as i64;
        let first = column(start).max(rect.x as i64);
        let last = column(end).min(rect.right() as i64 - 1);
        let x = first + 1;
        if x + width > rect.right() as i64 {
            continue;
        }
        let free = (x..x + width).all(|cx| cx <= last || buf[(cx as u16, rect.y)].symbol() == "─");
        if free {
            put(buf, x as u16, rect.y, &caption, mark, rect.right());
        }
    }
}

/// How often the host's latency fell in each row's band, over the window: sideways bars,
/// square-root scaled so a rare slow tail still shows.
fn histogram(
    buf: &mut Buffer,
    rect: tui::layout::Rect,
    scene: &Scene,
    host: usize,
    [lo, hi]: [f64; 2],
) {
    let t = scene.theme;
    let host = &scene.app.hosts[host];
    let rows = rect.height as usize;
    if rows == 0 || hi <= lo {
        return;
    }
    let mut counts = vec![0u32; rows];
    let dots = rows as f64 * 4.0 - 1.0;
    for &(_, value) in host.window(scene.x_bounds) {
        if value.is_nan() || value < lo || value > hi {
            continue;
        }
        let row = (((hi - value) / (hi - lo) * dots).round() / 4.0).floor() as usize;
        counts[row.min(rows - 1)] += 1;
    }
    let most = counts.iter().copied().max().unwrap_or(0);
    if most == 0 {
        return;
    }
    const PARTS: [&str; 8] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
    let color = Color::from(mix(host.tint, t.panel_rgb, 0.25));
    let style = match host.color {
        Color::Rgb(..) => Style::default().fg(color),
        other => Style::default().fg(other),
    };
    for (row, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let eighths = ((count as f64 / most as f64).sqrt() * rect.width as f64 * 8.0)
            .round()
            .max(1.0) as usize;
        let (full, rest) = (eighths / 8, eighths % 8);
        let mut bar = "█".repeat(full.min(rect.width as usize));
        if rest > 0 && full < rect.width as usize {
            bar.push_str(PARTS[rest - 1]);
        }
        put(buf, rect.x, rect.y + row as u16, &bar, style, rect.right());
    }
}

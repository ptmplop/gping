//! Everything around the lanes: the title bar, the time axis, the key bar, and the
//! one-line host rows used when there's no room for lanes.

use super::draw::{put, shorten_middle, spans, spans_right, width};
use super::{HostView, ScaleMode, Scene};
use crate::app::{grade, HostState, Readout, Status};
use crate::format;
use crate::theme::SHAPES;
use tui::buffer::Buffer;
use tui::layout::Rect;
use tui::style::{Color, Modifier, Style};

fn fg(color: Color) -> Style {
    Style::default().fg(color)
}

fn bold(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

/// The word, glyph and colours for a status.
pub fn status_badge(scene: &Scene, status: Status) -> (&'static str, &'static str, (Color, Color)) {
    let t = scene.theme;
    match status {
        Status::Steady => ("steady", "●", t.good),
        Status::Slow => ("slow", "▲", t.warn),
        Status::Lossy => ("lossy", "✕", t.bad),
        Status::Down => ("down", "■", t.bad),
        Status::Waiting => ("waiting", "◌", t.idle),
        Status::Failed => ("failed", "✕", t.bad),
        Status::Finished => ("done", "■", t.idle),
    }
}

pub fn masthead(
    buf: &mut Buffer,
    rect: Rect,
    scene: &Scene,
    views: &[HostView],
    visible: &[usize],
) {
    let t = scene.theme;
    super::draw::fill(buf, rect, t.mast);
    let (y, right) = (rect.y, rect.right());

    // Right: how the hosts are doing, the clock, and how long gping has been running. Each
    // part has a weight: when they don't all fit beside the name, the lightest goes first.
    // That is the uptime, then the clock, then the counts of hosts doing fine before the
    // ones in trouble; PAUSED stays.
    let mut parts: Vec<(String, Style, u8)> = Vec::new();
    let mut counts: Vec<(Status, usize)> = Vec::new();
    for &i in visible {
        let status = views[i].status;
        match counts.iter_mut().find(|(s, _)| *s == status) {
            Some((_, n)) => *n += 1,
            None => counts.push((status, 1)),
        }
    }
    let order = [
        (Status::Down, 9),
        (Status::Failed, 8),
        (Status::Lossy, 7),
        (Status::Slow, 6),
        (Status::Steady, 4),
        (Status::Waiting, 5),
        (Status::Finished, 3),
    ];
    for (status, weight) in order {
        if let Some(&(_, n)) = counts.iter().find(|(s, _)| *s == status) {
            let (word, glyph, (color, _)) = status_badge(scene, status);
            let word = if status == Status::Steady { "ok" } else { word };
            parts.push((format!("{glyph} {n} {word}  "), bold(color), weight));
        }
    }
    let clock = scene.wall_now.format("%H:%M:%S").to_string();
    let ran = format::elapsed(scene.now.max(0.0) as u64);
    if scene.closing {
        parts.push(("■ stopped ".into(), bold(t.idle.0), 10));
        parts.push((clock, bold(t.text), 2));
        parts.push((format!("  ran {ran}"), fg(t.muted), 1));
    } else {
        if scene.paused() {
            parts.push((" PAUSED ".into(), bold(t.warn.0).bg(t.warn.1), 10));
            parts.push(("  ".into(), Style::default(), 10));
        }
        parts.push((clock, bold(t.text), 2));
        parts.push((format!("  up {ran}"), fg(t.muted), 1));
    }
    parts.push((" ".into(), Style::default(), 10));
    let mut left_end = rect.x + 9;
    while !parts.is_empty() {
        let shown: Vec<(&str, Style)> = parts.iter().map(|(s, st, _)| (s.as_str(), *st)).collect();
        if let Some(x) = spans_right(buf, right, y, &shown, rect.x + 10) {
            left_end = x;
            break;
        }
        let lightest = (0..parts.len()).min_by_key(|&i| parts[i].2).unwrap_or(0);
        parts.remove(lightest);
    }

    // Left: the name, then what is being measured and how, as much as fits.
    let x = spans(
        buf,
        rect.x,
        y,
        &[
            (" ◆ ", bold(t.accent)),
            ("gping", bold(t.text)),
            ("   ", Style::default()),
        ],
        left_end,
    );
    let n = scene.app.hosts.len();
    let mut facts: Vec<String> = vec![scene.options.mode.clone()];
    if let Some(family) = scene.options.family {
        facts.push(family.to_string());
    }
    if let Some(interface) = &scene.options.interface {
        facts.push(format!("via {interface}"));
    }
    if n > 1 {
        facts.push(format!("{n} hosts"));
    }
    let hidden = n - visible.len();
    if hidden > 0 {
        facts.push(format!("{hidden} hidden"));
    }
    facts.push(format!("every {}", seconds(scene.options.interval)));
    facts.push(format!("{} window", seconds(scene.app.window)));
    facts.push(scale_label(scene));
    let room = left_end.saturating_sub(x + 1);
    while !facts.is_empty() && width(&facts.join(" · ")) > room {
        // Shed the least important facts first: the host count, then interface details.
        let drop = facts
            .iter()
            .position(|f| f.ends_with("hosts"))
            .unwrap_or(facts.len() - 1);
        facts.remove(drop);
    }
    put(buf, x, y, &facts.join(" · "), fg(t.soft), left_end);
}

fn scale_label(scene: &Scene) -> String {
    match scene.app.yrange {
        (Some(lo), Some(hi)) => format!("pinned {:.0}–{:.0} ms", lo / 1_000.0, hi / 1_000.0),
        _ => match scene.scale {
            ScaleMode::PerHost => "per-host scale".to_string(),
            ScaleMode::Shared => "shared scale".to_string(),
        },
    }
}

/// Seconds as "0.2 s", "30 s", "5 min".
fn seconds(s: f64) -> String {
    if s >= 120.0 && s % 60.0 == 0.0 {
        format!("{} min", s / 60.0)
    } else if s >= 10.0 || s.fract() == 0.0 {
        format!("{s:.0} s")
    } else {
        let text = format!("{s:.3}");
        format!("{} s", text.trim_end_matches('0').trim_end_matches('.'))
    }
}

/// Time labels under the plots: seconds ago while live, the time of day when paused and in
/// the final frame, so moments can be pinned down.
pub fn time_axis(buf: &mut Buffer, rect: Rect, scene: &Scene) {
    let t = scene.theme;
    let [x0, x1] = scene.x_bounds;
    let window = x1 - x0;
    let cols = rect.width.max(1);
    let absolute = scene.paused() || scene.closing;
    let step = format::time_step(window, cols, if absolute { 12 } else { 8 });
    let column =
        |time: f64| -> i64 { rect.x as i64 + ((time - x0) / window * cols as f64).floor() as i64 };
    let mut free_until = rect.right();
    let mut k = 0u32;
    loop {
        let (time, label) = if absolute {
            // On whole multiples of the step, in wall-clock time (to the millisecond: whole
            // seconds would put every label up to a second late).
            let end = scene.wall(x1).timestamp_millis() as f64 / 1000.0;
            let first = (end / step).floor() * step;
            let at = first - k as f64 * step;
            let time = x1 - (end - at);
            (time, scene.wall(time).format("%H:%M:%S").to_string())
        } else {
            let ago = k as f64 * step;
            (x1 - ago, format::ago(ago))
        };
        if time < x0 {
            break;
        }
        let w = width(&label);
        let col = column(time);
        // "now" ends at the right edge; other labels are centred on their moment.
        let start = if k == 0 && !absolute {
            rect.right() as i64 - w as i64
        } else {
            col - w as i64 / 2
        };
        let start = start.min(rect.right() as i64 - w as i64).max(rect.x as i64);
        if start + (w as i64) < free_until as i64
            || (k == 0 && start + w as i64 <= rect.right() as i64)
        {
            put(buf, start as u16, rect.y, &label, fg(t.muted), rect.right());
            free_until = start as u16;
        }
        k += 1;
        if k > 1_000 {
            break;
        }
    }
}

pub fn footer(buf: &mut Buffer, rect: Rect, scene: &Scene, views: &[HostView]) {
    let t = scene.theme;
    let (y, right) = (rect.y, rect.right());
    if scene.closing {
        let app = scene.app;
        let failures: Vec<String> = app
            .hosts
            .iter()
            .filter_map(|h| match &h.state {
                crate::app::HostState::Failed(message) => Some(format!("{}: {}", h.name, message)),
                _ => None,
            })
            .collect();
        if !failures.is_empty() {
            let line = format!(" ✕ {}", failures.join("  ·  ").replace('\n', " "));
            put(buf, rect.x, y, &line, bold(t.loss), right);
            return;
        }
        let sent: u64 = app.hosts.iter().map(|h| h.session.sent).sum();
        let lost: u64 = app.hosts.iter().map(|h| h.session.lost).sum();
        let loss = if sent == 0 {
            0.0
        } else {
            lost as f64 / sent as f64
        };
        // Whole pieces go when it's narrow, the least telling first: the version, what the
        // chart covered, then the percentage.
        for (version, chart, percent) in [
            (true, true, true),
            (false, true, true),
            (false, false, true),
            (false, false, false),
        ] {
            let mut parts = vec![(" ".to_string(), Style::default())];
            if version {
                parts.push((format!("gping {}", env!("CARGO_PKG_VERSION")), fg(t.soft)));
                parts.push(("  ·  ".to_string(), fg(t.muted)));
            }
            parts.push((format!("{} sent", thousands(sent)), fg(t.soft)));
            parts.push(("  ·  ".to_string(), fg(t.muted)));
            parts.push(if lost == 0 {
                ("none lost".to_string(), fg(t.good.0))
            } else if percent {
                let text = format!("{} lost ({})", thousands(lost), format::percent(loss));
                (text, bold(t.loss))
            } else {
                (format!("{} lost", thousands(lost)), bold(t.loss))
            });
            if chart {
                parts.push(("  ·  ".to_string(), fg(t.muted)));
                parts.push((format!("chart: last {}", seconds(app.window)), fg(t.muted)));
            }
            let parts: Vec<(&str, Style)> = parts.iter().map(|(s, st)| (s.as_str(), *st)).collect();
            let fits = parts.iter().map(|(s, _)| width(s)).sum::<u16>() <= rect.width;
            if fits || !percent {
                spans(buf, rect.x, y, &parts, right);
                return;
            }
        }
        return;
    }

    let pause = if scene.paused() { "resume" } else { "pause" };
    let keys = [
        ("q", "quit"),
        ("space", pause),
        ("s", "scale"),
        ("1-9", "hide"),
        ("r", "reset"),
        ("?", "help"),
    ];
    // The legend on the right, if it fits beside at least the first few keys.
    let heat = &t.heat;
    let legend = [
        ("━", fg(heat[0])),
        ("━", fg(heat[1])),
        ("━", fg(heat[2])),
        (" slower than usual   ", fg(t.muted)),
        ("✕", bold(t.loss)),
        (" lost ", fg(t.muted)),
    ];
    let short = [
        ("━━━", fg(heat[0])),
        (" slow  ", fg(t.muted)),
        ("✕", bold(t.loss)),
        (" lost ", fg(t.muted)),
    ];
    // Heat only once a host has a baseline to be slower than, and never without colour.
    let has_heat = t.shows_heat && views.iter().any(|v| v.baseline.is_some());
    let limit = if rect.width >= 90 && has_heat {
        spans_right(buf, right, y, &legend, rect.x + 40).unwrap_or(right)
    } else if rect.width >= 60 && has_heat {
        spans_right(buf, right, y, &short, rect.x + 40).unwrap_or(right)
    } else if rect.width >= 60 {
        spans_right(buf, right, y, &legend[4..], rect.x + 40).unwrap_or(right)
    } else {
        right
    };
    let mut x = rect.x + 1;
    for (key, what) in keys {
        let piece = width(key) + 1 + width(what) + 3;
        if x + piece > limit {
            break;
        }
        x = spans(
            buf,
            x,
            y,
            &[
                (key, bold(t.text)),
                (" ", Style::default()),
                (what, fg(t.muted)),
            ],
            limit,
        );
        x += 3;
    }
}

/// A host on one line, for screens with no room for lanes.
pub fn host_line(buf: &mut Buffer, rect: Rect, scene: &Scene, view: &HostView) {
    let t = scene.theme;
    let host = &scene.app.hosts[view.index];
    let (y, right) = (rect.y, rect.right());
    let shape = SHAPES[view.index % SHAPES.len()];
    let (word, _, (color, _)) = status_badge(scene, view.status);
    let session = &host.session;
    // As on the cards: the session in the final frame, the moment before that.
    let value = if matches!(host.state, HostState::Failed(_)) {
        ("failed".to_string(), bold(t.loss))
    } else if scene.closing {
        match session.mean() {
            Some(mean) => (format!("avg {}", format::latency(mean)), bold(t.text)),
            None if session.sent > 0 => ("no replies".to_string(), bold(t.loss)),
            None => ("-".to_string(), fg(t.muted)),
        }
    } else {
        match view.readout {
            Readout::Value(us) => {
                let g = view.baseline.map_or(0, |b| grade(us, b));
                (format::latency(us), bold(t.graded(g, t.text)))
            }
            Readout::Lost => ("lost".to_string(), bold(t.loss)),
            Readout::NoReply(s) => (format!("down {}", format::elapsed(s as u64)), bold(t.loss)),
            Readout::Waiting => ("…".to_string(), fg(t.muted)),
        }
    };
    let loss = if scene.closing {
        session.loss()
    } else {
        view.summary.map_or(0.0, |s| s.loss())
    };
    // The status word is padded to the longest, so that the columns before it line up.
    let word = if scene.closing {
        String::new()
    } else {
        format!("  {word:<7}")
    };
    let tail = [
        (format!("{:>8}", value.0), value.1),
        ("  ".to_string(), Style::default()),
        (
            format!("{:>5}", format::percent(loss)),
            if loss > 0.0 {
                bold(t.loss)
            } else {
                fg(t.muted)
            },
        ),
        (word, fg(color)),
    ];
    let tail_parts: Vec<(&str, Style)> = tail.iter().map(|(s, st)| (s.as_str(), *st)).collect();
    // Drop the status word, then the loss, when it gets tight.
    let mut end = right;
    for keep in [4, 3, 1] {
        if let Some(x) = spans_right(buf, right, y, &tail_parts[..keep], rect.x + 6) {
            end = x;
            break;
        }
    }
    let name_room = end.saturating_sub(rect.x + 3);
    spans(
        buf,
        rect.x,
        y,
        &[
            (shape, fg(host.color)),
            (" ", Style::default()),
            (&shorten_middle(&host.name, name_room), bold(host.color)),
        ],
        end,
    );
}

pub fn more(buf: &mut Buffer, rect: Rect, scene: &Scene, count: usize) {
    let text = if count == 1 {
        "+1 more host".to_string()
    } else {
        format!("+{count} more hosts")
    };
    put(
        buf,
        rect.x,
        rect.y,
        &text,
        fg(scene.theme.muted),
        rect.right(),
    );
}

/// 1234567 as "1,234,567".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn seconds_read_naturally() {
        assert_eq!(seconds(0.2), "0.2 s");
        assert_eq!(seconds(0.25), "0.25 s");
        assert_eq!(seconds(1.0), "1 s");
        assert_eq!(seconds(30.0), "30 s");
        assert_eq!(seconds(300.0), "5 min");
    }
}

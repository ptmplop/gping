//! The card at the right of each lane, next to where the newest sample lands: the host, how
//! it's doing, and its numbers.

use super::chrome::{status_badge, thousands};
use super::draw::{fill, put, shorten_middle, spans, spans_right, truncate, width, wrap};
use super::layout::CardKind;
use super::{HostView, Scene};
use crate::app::{grade, Host, HostState, Readout};
use crate::bigdigits;
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

/// Where a card writes: from `x` up to (not including) `right`.
struct Pen<'a> {
    scene: &'a Scene<'a>,
    view: &'a HostView,
    host: &'a Host,
    x: u16,
    right: u16,
}

pub fn draw(buf: &mut Buffer, rect: Rect, kind: CardKind, scene: &Scene, view: &HostView) {
    let host = &scene.app.hosts[view.index];
    fill(buf, rect, scene.theme.panel);
    for y in rect.top()..rect.bottom() {
        put(buf, rect.x, y, "▌", fg(host.color), rect.right());
    }
    let pen = Pen {
        scene,
        view,
        host,
        x: rect.x + 2,
        right: rect.right().saturating_sub(1),
    };
    match kind {
        CardKind::Full => full(buf, rect, &pen),
        CardKind::Compact => compact(buf, rect, &pen),
        CardKind::Slim => slim(buf, rect, &pen),
    }
}

/// Rows of the full card, top to bottom; laid out with any spare rows split above and below.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Name,
    Detail,
    Blank,
    Digits,
    Stats,
    Spark,
    Totals,
}

fn full(buf: &mut Buffer, rect: Rect, pen: &Pen) {
    // The digits take three rows; the two blanks after them are their lower rows.
    let mut rows = vec![
        Row::Name,
        Row::Detail,
        Row::Blank,
        Row::Digits,
        Row::Blank,
        Row::Blank,
    ];
    if rect.height >= 9 {
        rows.push(Row::Blank);
    }
    rows.extend([Row::Stats, Row::Spark]);
    if rect.height >= 11 && !pen.scene.closing {
        rows.extend([Row::Blank, Row::Totals]);
    }
    let spare = rect.height.saturating_sub(rows.len() as u16);
    for (row, y) in rows.into_iter().zip(rect.y + spare / 3..rect.bottom()) {
        match row {
            Row::Name => name_row(buf, y, pen, true),
            Row::Detail => detail_row(buf, y, pen),
            Row::Digits if failure(pen).is_some() => {
                readout(buf, pen.x, y, pen.right, pen);
            }
            Row::Digits => {
                let end = readout(buf, pen.x, y, pen.right.saturating_sub(11), pen);
                stats_column(buf, end.max(pen.right.saturating_sub(10)), y, pen);
            }
            Row::Stats => stats_row(buf, y, pen),
            Row::Spark => spark_row(buf, y, pen),
            Row::Totals => totals_row(buf, y, pen),
            Row::Blank => {}
        }
    }
}

fn compact(buf: &mut Buffer, rect: Rect, pen: &Pen) {
    let t = pen.scene.theme;
    let mut y = rect.y;
    name_row(buf, y, pen, true);
    y += 1;
    if rect.height >= 6 {
        y += (rect.height - 6) / 3;
    }
    readout(buf, pen.x, y, pen.right, pen);
    y += 3;
    let s = pen.view.summary;
    let value =
        |f: fn(&crate::stats::Summary) -> f64| s.map_or("-".to_string(), |s| format::short(f(&s)));
    let lines: [Vec<(String, Style)>; 3] = if pen.scene.closing {
        session_lines(pen)
    } else {
        [
            vec![
                ("avg ".into(), fg(t.muted)),
                (value(|s| s.mean), fg(t.soft)),
                ("  p95 ".into(), fg(t.muted)),
                (value(|s| s.p95), fg(t.soft)),
            ],
            loss_parts(pen),
            vec![
                ("min ".into(), fg(t.muted)),
                (value(|s| s.min), fg(t.soft)),
                ("  max ".into(), fg(t.muted)),
                (value(|s| s.max), fg(t.soft)),
            ],
        ]
    };
    for line in lines.iter() {
        if y >= rect.bottom() {
            break;
        }
        let parts: Vec<(&str, Style)> = line.iter().map(|(s, st)| (s.as_str(), *st)).collect();
        spans(buf, pen.x, y, &parts, pen.right);
        y += 1;
    }
    if y < rect.bottom() {
        spark_row(buf, y, pen);
    }
}

fn slim(buf: &mut Buffer, rect: Rect, pen: &Pen) {
    let t = pen.scene.theme;
    let closing = pen.scene.closing;
    let session = &pen.host.session;
    name_row(buf, rect.y, pen, false);
    if rect.height >= 2 {
        let (text, style) = readout_text(pen);
        let loss = if closing {
            session.loss()
        } else {
            pen.view.summary.map_or(0.0, |s| s.loss())
        };
        let x = spans(buf, pen.x, rect.y + 1, &[(&text, style)], pen.right);
        if loss > 0.0 {
            spans_right(
                buf,
                pen.right,
                rect.y + 1,
                &[(&format::percent(loss), bold(t.loss))],
                x + 1,
            );
        }
    }
    if rect.height >= 3 {
        if let Some(reason) = failure(pen) {
            put(
                buf,
                pen.x,
                rect.y + 2,
                &truncate(reason, pen.right.saturating_sub(pen.x)),
                fg(t.soft),
                pen.right,
            );
        } else if closing {
            let sent = thousands(session.sent);
            spans(
                buf,
                pen.x,
                rect.y + 2,
                &[("sent ", fg(t.muted)), (&sent, fg(t.soft))],
                pen.right,
            );
        } else {
            let (word, _, (color, _)) = status_badge(pen.scene, pen.view.status);
            put(buf, pen.x, rect.y + 2, word, fg(color), pen.right);
        }
    }
    if rect.height >= 4 {
        let parts = if closing {
            session_lines(pen)[0].clone()
        } else {
            loss_parts(pen)
        };
        let parts: Vec<(&str, Style)> = parts.iter().map(|(s, st)| (s.as_str(), *st)).collect();
        spans(buf, pen.x, rect.y + 3, &parts, pen.right);
    }
}

/// The session in three short lines, for the final frame, most telling first (a short card
/// shows only the first two): what was lost, the spread of the replies, and what was sent.
/// The average is the big number.
fn session_lines(pen: &Pen) -> [Vec<(String, Style)>; 3] {
    let t = pen.scene.theme;
    let s = &pen.host.session;
    let replied = s.replies() > 0;
    let value = |v: Option<f64>| {
        v.filter(|v| v.is_finite())
            .map_or("-".to_string(), format::short)
    };
    let lost = if s.lost == 0 {
        vec![("none lost".to_string(), fg(t.soft))]
    } else {
        vec![
            ("lost ".to_string(), fg(t.muted)),
            (
                format!("{} ({})", thousands(s.lost), format::percent(s.loss())),
                bold(t.loss),
            ),
        ]
    };
    [
        lost,
        vec![
            ("min ".into(), fg(t.muted)),
            (value(replied.then_some(s.min)), fg(t.soft)),
            ("  max ".into(), fg(t.muted)),
            (value(replied.then_some(s.max)), fg(t.soft)),
        ],
        vec![
            ("jit ".into(), fg(t.muted)),
            (value(s.jitter()), fg(t.soft)),
            ("  sent ".into(), fg(t.muted)),
            (thousands(s.sent), fg(t.soft)),
        ],
    ]
}

/// Why the host's pinger failed, if it did.
fn failure<'a>(pen: &Pen<'a>) -> Option<&'a str> {
    match &pen.host.state {
        HostState::Failed(reason) => Some(reason),
        _ => None,
    }
}

/// The host's shape and name, with its status on the right.
fn name_row(buf: &mut Buffer, y: u16, pen: &Pen, badge: bool) {
    let t = pen.scene.theme;
    let (word, glyph, (color, background)) = status_badge(pen.scene, pen.view.status);
    let mut name_end = pen.right;
    if badge && !pen.scene.closing {
        let style = if pen.view.blink {
            bold(background).bg(color)
        } else {
            bold(color).bg(background)
        };
        // The word, unless the name would have to be cut for it: then just the glyph.
        let label = format!(" {word} ");
        let name_fits = pen.x + 2 + width(&pen.host.name) + 1 + width(&label) <= pen.right;
        let placed = if name_fits {
            spans_right(buf, pen.right, y, &[(&label, style)], pen.x + 6)
        } else {
            None
        };
        let placed =
            placed.or_else(|| spans_right(buf, pen.right, y, &[(glyph, bold(color))], pen.x + 4));
        if let Some(x) = placed {
            name_end = x.saturating_sub(1);
        }
    }
    let lost =
        matches!(pen.view.readout, Readout::Lost | Readout::NoReply(_)) || failure(pen).is_some();
    let shape = if lost {
        ("✕", bold(t.loss))
    } else {
        (SHAPES[pen.view.index % SHAPES.len()], fg(pen.host.color))
    };
    let room = name_end.saturating_sub(pen.x + 2);
    let name = shorten_middle(&pen.host.name, room);
    spans(
        buf,
        pen.x,
        y,
        &[
            shape,
            (" ", Style::default()),
            (&name, bold(pen.host.color)),
        ],
        name_end,
    );
}

/// Where the host is, and how it's probed.
fn detail_row(buf: &mut Buffer, y: u16, pen: &Pen) {
    let t = pen.scene.theme;
    let options = pen.scene.options;
    let mut parts: Vec<String> = Vec::new();
    if pen.scene.closing {
        let since = pen.host.session.since;
        let length = format::elapsed((pen.scene.now - since).max(0.0) as u64);
        parts.push(if since > 0.0 {
            format!("since reset · {length}")
        } else {
            format!("session · {length}")
        });
    } else {
        parts.extend(pen.host.address.clone());
        parts.push(options.mode.to_lowercase());
        if let Some(interface) = &options.interface {
            parts.push(interface.clone());
        }
    }
    let text = truncate(&parts.join(" · "), pen.right.saturating_sub(pen.x + 2));
    put(buf, pen.x + 2, y, &text, fg(t.muted), pen.right);
}

/// The current latency in big digits, or what's going on instead. Returns where it ended.
fn readout(buf: &mut Buffer, x: u16, y: u16, max_x: u16, pen: &Pen) -> u16 {
    let t = pen.scene.theme;
    if let Some(reason) = failure(pen) {
        put(buf, x, y, "failed", bold(t.loss), max_x);
        let room = max_x.saturating_sub(x);
        for (i, line) in wrap(reason, room, 2).iter().enumerate() {
            put(buf, x, y + 1 + i as u16, line, fg(t.soft), max_x);
        }
        return max_x;
    }
    let session = &pen.host.session;
    let (value, caption) = if pen.scene.closing {
        match session.mean() {
            Some(mean) => (Readout::Value(mean), "avg"),
            // Pings went out, and none came back.
            None if session.sent > 0 => {
                put(buf, x, y + 1, "no replies", bold(t.loss), max_x);
                return x + 10;
            }
            None => (Readout::Waiting, ""),
        }
    } else {
        (pen.view.readout, "")
    };
    match value {
        Readout::Value(us) => {
            let (number, unit) =
                format::latency_parts(us).unwrap_or_else(|| ("-".to_string(), "ms"));
            // The live reading warms with heat. A session's average isn't one reading, and
            // a few spikes are enough to lift it over the line, so it stays plain.
            let g = match pen.view.baseline {
                Some(baseline) if !pen.scene.closing => grade(us, baseline),
                _ => 0,
            };
            let color = t.graded(g, t.text);
            let w = bigdigits::width(&number);
            if x + w + 3 > max_x {
                let text = format!("{number} {unit}");
                return put(buf, x, y + 1, &text, bold(color), max_x);
            }
            bigdigits::render(&number, x, y, bold(color), t.light_digits, buf);
            put(buf, x + w + 1, y, unit, fg(t.muted), max_x);
            put(buf, x + w + 1, y + 2, caption, fg(t.muted), max_x);
            x + w + 1 + width(unit).max(width(caption))
        }
        Readout::Lost => {
            bigdigits::render("---", x, y, bold(t.loss), t.light_digits, buf);
            put(buf, x + 12, y + 1, "lost", bold(t.loss), max_x);
            x + 16
        }
        Readout::NoReply(seconds) => {
            put(buf, x, y, "no reply", bold(t.loss), max_x);
            let text = format!("for {}", format::elapsed(seconds.max(0.0) as u64));
            put(buf, x, y + 1, &text, fg(t.soft), max_x);
            x + 10
        }
        Readout::Waiting => {
            put(buf, x, y + 1, "waiting…", fg(t.muted), max_x);
            x + 8
        }
    }
}

/// The readout as plain text, for cards too small for big digits.
fn readout_text(pen: &Pen) -> (String, Style) {
    let t = pen.scene.theme;
    if failure(pen).is_some() {
        return ("failed".into(), bold(t.loss));
    }
    if pen.scene.closing {
        // The session's average; there is no room to say so beside its loss.
        let session = &pen.host.session;
        return match session.mean() {
            Some(mean) => (format::latency(mean), bold(t.text)),
            None if session.sent > 0 => ("no replies".into(), bold(t.loss)),
            None => ("-".into(), fg(t.muted)),
        };
    }
    match pen.view.readout {
        Readout::Value(us) => {
            let g = pen.view.baseline.map_or(0, |b| grade(us, b));
            (format::latency(us), bold(t.graded(g, t.text)))
        }
        Readout::Lost => ("lost".into(), bold(t.loss)),
        Readout::NoReply(s) => (format!("down {}", format::elapsed(s as u64)), bold(t.loss)),
        Readout::Waiting => ("waiting…".into(), fg(t.muted)),
    }
}

/// avg / p95 / max beside the digits (min / max / jitter for the session, at the end).
fn stats_column(buf: &mut Buffer, x: u16, y: u16, pen: &Pen) {
    let t = pen.scene.theme;
    let rows: [(&str, Option<f64>); 3] = if pen.scene.closing {
        let s = &pen.host.session;
        [
            ("min", (s.replies() > 0).then_some(s.min)),
            ("max", (s.replies() > 0).then_some(s.max)),
            ("jit", s.jitter()),
        ]
    } else {
        let s = pen.view.summary;
        [
            ("avg", s.map(|s| s.mean)),
            ("p95", s.map(|s| s.p95)),
            ("max", s.map(|s| s.max)),
        ]
    };
    for (i, (label, value)) in rows.iter().enumerate() {
        let value = value
            .filter(|v| v.is_finite())
            .map_or("-".to_string(), format::short);
        spans_right(
            buf,
            pen.right,
            y + i as u16,
            &[(label, fg(t.muted)), (&format!("{value:>6}"), fg(t.soft))],
            x,
        );
    }
}

fn loss_parts(pen: &Pen) -> Vec<(String, Style)> {
    let t = pen.scene.theme;
    let (lost, sent) = match pen.view.summary {
        Some(s) => (s.lost, s.sent()),
        None => (0, 0),
    };
    let ratio = if sent == 0 {
        0.0
    } else {
        lost as f64 / sent as f64
    };
    let style = if lost > 0 { bold(t.loss) } else { fg(t.soft) };
    vec![
        ("loss ".into(), fg(t.muted)),
        (format::percent(ratio), style),
    ]
}

/// min / jitter / loss over the window (sent / lost over the session, at the end).
fn stats_row(buf: &mut Buffer, y: u16, pen: &Pen) {
    let t = pen.scene.theme;
    if pen.scene.closing {
        let s = &pen.host.session;
        let lost_style = if s.lost > 0 { bold(t.loss) } else { fg(t.soft) };
        let parts = [
            ("sent ", fg(t.muted)),
            (&thousands(s.sent) as &str, fg(t.soft)),
            ("  lost ", fg(t.muted)),
            (
                &format!("{} ({})", thousands(s.lost), format::percent(s.loss())) as &str,
                lost_style,
            ),
        ];
        spans(buf, pen.x, y, &parts, pen.right);
        return;
    }
    let s = pen.view.summary;
    let min = s
        .map(|s| s.min)
        .filter(|v| v.is_finite())
        .map_or("-".to_string(), format::short);
    let jitter = s
        .map(|s| s.jitter)
        .filter(|v| v.is_finite())
        .map_or("-".to_string(), format::short);
    let loss = loss_parts(pen);
    let parts = [
        ("min ", fg(t.muted)),
        (&min as &str, fg(t.soft)),
        ("  jit ", fg(t.muted)),
        (&jitter as &str, fg(t.soft)),
        ("  ", Style::default()),
        (&loss[0].0 as &str, loss[0].1),
        (&loss[1].0 as &str, loss[1].1),
    ];
    spans(buf, pen.x, y, &parts, pen.right);
}

/// The whole session as a sparkline of each stretch's slowest reply, with lost pings in red.
fn spark_row(buf: &mut Buffer, y: u16, pen: &Pen) {
    let t = pen.scene.theme;
    let s = &pen.host.session;
    let tail = if pen.scene.closing {
        Vec::new()
    } else if s.lost > 0 {
        vec![
            (thousands(s.lost), bold(t.loss)),
            (format!("/{} lost", thousands(s.sent)), fg(t.muted)),
        ]
    } else {
        vec![(format!("{} sent", thousands(s.sent)), fg(t.muted))]
    };
    let tail: Vec<(&str, Style)> = tail.iter().map(|(s, st)| (s.as_str(), *st)).collect();
    let tail_x = spans_right(buf, pen.right, y, &tail, pen.x + 8).unwrap_or(pen.right);
    let x = spans(buf, pen.x, y, &[("session ", fg(t.muted))], tail_x);
    let columns = tail_x.saturating_sub(x + 1) as usize;
    if columns == 0 || pen.host.history.is_empty() {
        return;
    }
    const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let buckets = pen.host.history.columns(columns);
    let (lo, hi) = (s.min, s.max);
    // A stretch is marked red when it lost noticeably more than this host usually does;
    // as the session grows each column covers longer, and nearly all of them hold a loss.
    let notable = (3.0 * s.loss()).max(0.05);
    for (i, bucket) in buckets.iter().enumerate() {
        let cx = x + i as u16;
        let sent = bucket.replies + bucket.lost;
        if sent == 0 {
            continue;
        }
        if bucket.replies == 0 {
            put(buf, cx, y, "✕", bold(t.loss), tail_x);
            continue;
        }
        let level = if hi > lo {
            ((bucket.max - lo) / (hi - lo) * 7.0).round() as usize
        } else {
            0
        };
        let lossy = bucket.lost as f64 / sent as f64 >= notable;
        let style = if lossy {
            fg(t.loss)
        } else {
            fg(pen.host.color)
        };
        put(buf, cx, y, BARS[level.min(7)], style, tail_x);
    }
}

/// The whole session in one line, on tall cards, so window and session aren't confused.
fn totals_row(buf: &mut Buffer, y: u16, pen: &Pen) {
    let t = pen.scene.theme;
    let s = &pen.host.session;
    let mean = s.mean().map_or("-".to_string(), format::short);
    let max = if s.replies() > 0 {
        format::short(s.max)
    } else {
        "-".to_string()
    };
    let since = if s.since > 0.0 {
        "since reset "
    } else {
        "since start "
    };
    let parts = [
        (since, fg(t.muted)),
        ("avg ", fg(t.muted)),
        (&mean as &str, fg(t.soft)),
        ("  max ", fg(t.muted)),
        (&max as &str, fg(t.soft)),
    ];
    spans(buf, pen.x, y, &parts, pen.right);
}

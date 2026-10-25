//! The help panel: the keys, and what the marks on screen mean.

use super::draw::{clear, put, spans};
use super::Scene;
use tui::buffer::Buffer;
use tui::layout::Rect;
use tui::style::{Color, Modifier, Style};

/// Draws the panel over `area`, centred in `room` (the part not taken by the host cards) if
/// it fits there.
pub fn draw(buf: &mut Buffer, area: Rect, room: Rect, scene: &Scene) {
    let t = scene.theme;
    let fg = |c: Color| Style::default().fg(c);
    let key = |c: Color| fg(c).add_modifier(Modifier::BOLD);
    let heat = t.heat;
    let lost = if t.curtain.is_some() {
        "       a lost ping; the column behind it turns red"
    } else {
        "       a lost ping"
    };
    let mut lines: Vec<Vec<(&str, Style)>> = vec![
        vec![
            ("space", key(t.text)),
            ("   pause, and resume (probing carries on)", fg(t.soft)),
        ],
        vec![
            ("s", key(t.text)),
            ("       one scale per host, or one for all", fg(t.soft)),
        ],
        vec![
            ("1-9", key(t.text)),
            ("     hide or show a host; 0 shows them all", fg(t.soft)),
        ],
        vec![
            ("r", key(t.text)),
            ("       restart the session totals", fg(t.soft)),
        ],
        vec![
            ("q", key(t.text)),
            ("       quit (Esc and Ctrl-C too)", fg(t.soft)),
        ],
        vec![],
        vec![("✕", key(t.loss)), (lost, fg(t.soft))],
        vec![
            ("▏▍▋█", fg(t.soft)),
            ("    how often each latency came up", fg(t.soft)),
        ],
        vec![
            ("session", fg(t.muted)),
            (" every reply since start, slowest per stretch", fg(t.soft)),
        ],
    ];
    if t.shows_heat {
        lines.insert(
            6,
            vec![
                ("━", fg(heat[0])),
                ("━", fg(heat[1])),
                ("━", fg(heat[2])),
                ("     slower than this host usually is", fg(t.soft)),
            ],
        );
    }
    let inner_w = 56u16;
    let w = (inner_w + 4).min(area.width);
    let h = (lines.len() as u16 + 4).min(area.height);
    if w < 20 || h < 6 {
        return;
    }
    let y = area.y + (area.height - h) / 2;
    let rect = if room.width >= w {
        Rect::new(room.x + (room.width - w) / 2, y, w, h)
    } else {
        // Over the cards too: blank its rows right across, rather than leave scraps of
        // them either side.
        clear(buf, Rect::new(area.x, y, area.width, h), t.base);
        Rect::new(area.x + (area.width - w) / 2, y, w, h)
    };
    clear(buf, rect, t.panel);
    let border = fg(t.chrome);
    let right = rect.right() - 1;
    put(buf, rect.x, rect.y, "╭", border, rect.right());
    put(buf, right, rect.y, "╮", border, rect.right());
    put(buf, rect.x, rect.bottom() - 1, "╰", border, rect.right());
    put(buf, right, rect.bottom() - 1, "╯", border, rect.right());
    for x in rect.x + 1..right {
        put(buf, x, rect.y, "─", border, right);
        put(buf, x, rect.bottom() - 1, "─", border, right);
    }
    for y in rect.y + 1..rect.bottom() - 1 {
        put(buf, rect.x, y, "│", border, rect.right());
        put(buf, right, y, "│", border, rect.right());
    }
    spans(
        buf,
        rect.x + 2,
        rect.y,
        &[(" gping ", key(t.text)), ("keys and marks ", fg(t.muted))],
        right,
    );
    for (i, line) in lines.iter().enumerate() {
        let y = rect.y + 2 + i as u16;
        if y >= rect.bottom() - 1 {
            break;
        }
        spans(buf, rect.x + 3, y, line, right - 1);
    }
    put(
        buf,
        rect.x + 3,
        rect.bottom() - 1,
        " any key closes this ",
        fg(t.muted),
        right,
    );
}

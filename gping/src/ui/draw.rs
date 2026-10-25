//! Small drawing helpers that never write outside the buffer, whatever they're given.

use tui::buffer::Buffer;
use tui::layout::Rect;
use tui::style::{Color, Style};
use unicode_width::UnicodeWidthStr;

/// Writes `text` at `(x, y)`, stopping before `max_x` and the buffer's edge. Returns where it
/// stopped.
pub fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, max_x: u16) -> u16 {
    let area = buf.area;
    let limit = max_x.min(area.right());
    if y < area.top() || y >= area.bottom() || x < area.left() || x >= limit {
        return x;
    }
    buf.set_stringn(x, y, text, (limit - x) as usize, style).0
}

/// Writes styled pieces one after another.
pub fn spans(buf: &mut Buffer, mut x: u16, y: u16, parts: &[(&str, Style)], max_x: u16) -> u16 {
    for (text, style) in parts {
        x = put(buf, x, y, text, *style, max_x);
    }
    x
}

/// Writes styled pieces so that they end just before `right`, if they fit after `min_x`.
pub fn spans_right(
    buf: &mut Buffer,
    right: u16,
    y: u16,
    parts: &[(&str, Style)],
    min_x: u16,
) -> Option<u16> {
    let width: u16 = parts.iter().map(|(t, _)| width(t)).sum();
    let x = right.checked_sub(width)?;
    if x < min_x {
        return None;
    }
    spans(buf, x, y, parts, right);
    Some(x)
}

pub fn width(text: &str) -> u16 {
    text.width().min(u16::MAX as usize) as u16
}

/// Sets the background of every cell in `rect` (clipped to the buffer).
pub fn fill(buf: &mut Buffer, rect: Rect, bg: Color) {
    let rect = rect.intersection(buf.area);
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            buf[(x, y)].set_bg(bg);
        }
    }
}

/// Blanks every cell in `rect` (clipped to the buffer) and gives it the background `bg`, for
/// panels drawn over the rest of the screen.
pub fn clear(buf: &mut Buffer, rect: Rect, bg: Color) {
    let rect = rect.intersection(buf.area);
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            buf[(x, y)].reset();
            buf[(x, y)].set_bg(bg);
        }
    }
}

/// Cuts `text` to at most `max` cells, ending it with "…" if anything had to go.
pub fn truncate(text: &str, max: u16) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = leading(text.chars(), max - 1);
    out.push('…');
    out
}

/// Cuts `text` to at most `max` cells by taking out its middle, keeping both ends: a host
/// name or an address is told apart by either ("2606:4700…700::1111").
pub fn shorten_middle(text: &str, max: u16) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max < 3 {
        return truncate(text, max);
    }
    let tail_width = (max - 1) / 2;
    let head = leading(text.chars(), max - 1 - tail_width);
    let tail: String = leading(text.chars().rev(), tail_width)
        .chars()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

/// Breaks `text` into at most `lines` lines of at most `max` cells, between words where it
/// can, ending the last with "…" if the text doesn't fit.
pub fn wrap(text: &str, max: u16, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let joined = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if width(&joined) <= max {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        line = word.to_string();
        if out.len() == lines {
            break;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    let more = out.len() > lines;
    out.truncate(lines);
    if let Some(last) = out.last_mut() {
        if more || width(last) > max {
            let room = if more { max.saturating_sub(1) } else { max };
            *last = truncate(last, room);
            if more && !last.ends_with('…') {
                last.push('…');
            }
        }
    }
    out
}

/// As many of `chars` as fit in `max` cells.
fn leading(chars: impl Iterator<Item = char>, max: u16) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in chars {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if used + w > max {
            break;
        }
        used += w;
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_outside_the_buffer_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 2));
        assert_eq!(put(&mut buf, 0, 0, "hello", Style::default(), 100), 0);
        assert_eq!(put(&mut buf, 5, 9, "hello", Style::default(), 100), 5);
        assert_eq!(put(&mut buf, 12, 5, "hello", Style::default(), 100), 15);
        fill(&mut buf, Rect::new(0, 0, 100, 100), Color::Red);
        assert!(buf.content.iter().all(|c| c.bg == Color::Red));
    }

    #[test]
    fn right_alignment() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        assert_eq!(
            spans_right(
                &mut buf,
                10,
                0,
                &[("ab", Style::default()), ("cd", Style::default())],
                0
            ),
            Some(6)
        );
        assert_eq!(buf[(6, 0)].symbol(), "a");
        assert_eq!(
            spans_right(&mut buf, 10, 0, &[("too long for it", Style::default())], 0),
            None
        );
    }

    #[test]
    fn truncation_counts_cells() {
        assert_eq!(truncate("example.com", 11), "example.com");
        assert_eq!(truncate("example.com", 7), "exampl…");
        assert_eq!(truncate("日本語", 4), "日…");
        assert_eq!(truncate("example.com", 0), "");
    }

    #[test]
    fn wrapping_breaks_between_words() {
        let reason = "ping6: UDP connect: No route to host (exit status: 68)";
        assert_eq!(
            wrap(reason, 24, 2),
            ["ping6: UDP connect: No", "route to host (exit…"]
        );
        assert_eq!(wrap("short", 24, 2), ["short"]);
        assert_eq!(wrap("averyveryverylongword", 8, 1), ["averyve…"]);
        assert!(wrap("", 8, 2).is_empty());
    }

    // Cut silently, "2606:4700:4700::1111" became "2606:4700:4700::111": another address.
    #[test]
    fn names_keep_both_ends() {
        assert_eq!(
            shorten_middle("2606:4700:4700::1111", 19),
            "2606:4700…700::1111"
        );
        assert_eq!(shorten_middle("149.112.112.112", 12), "149.11…2.112");
        assert_eq!(shorten_middle("dns.example", 11), "dns.example");
        assert_eq!(shorten_middle("dns.example", 2), "d…");
    }
}

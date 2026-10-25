//! A three-row, seven-segment style font built from box-drawing characters, for the
//! latency readouts: large enough to read from across the room.
//!
//! Each glyph is three cells wide. Glyphs are separated by one blank column, and a decimal
//! point sits in that gap on the bottom row, so `7.65` is eleven cells wide:
//!
//! ```text
//! ╺━┓ ┏━╸ ┏━╸
//!   ┃ ┣━┓ ┗━┓
//!   ╹•┗━┛ ╺━┛
//! ```

use tui::buffer::Buffer;
use tui::style::Style;

/// Heavy strokes, for terminals that draw box-drawing characters themselves (most do).
const HEAVY: [[&str; 3]; 11] = [
    ["┏━┓", "┃ ┃", "┗━┛"],
    ["╺┓ ", " ┃ ", "╺┻╸"],
    ["╺━┓", "┏━┛", "┗━╸"],
    ["╺━┓", " ━┫", "╺━┛"],
    ["╻ ╻", "┗━┫", "  ╹"],
    ["┏━╸", "┗━┓", "╺━┛"],
    ["┏━╸", "┣━┓", "┗━┛"],
    ["╺━┓", "  ┃", "  ╹"],
    ["┏━┓", "┣━┫", "┗━┛"],
    ["┏━┓", "┗━┫", "╺━┛"],
    ["   ", "╺━╸", "   "],
];

/// Light strokes from the original IBM PC set, for consoles whose fonts lack heavy ones.
const LIGHT: [[&str; 3]; 11] = [
    ["┌─┐", "│ │", "└─┘"],
    [" ┐ ", " │ ", " ┴ "],
    ["╶─┐", "┌─┘", "└─╴"],
    ["╶─┐", " ─┤", "╶─┘"],
    ["╷ ╷", "└─┤", "  ╵"],
    ["┌─╴", "└─┐", "╶─┘"],
    ["┌─╴", "├─┐", "└─┘"],
    ["╶─┐", "  │", "  ╵"],
    ["┌─┐", "├─┤", "└─┘"],
    ["┌─┐", "└─┤", "╶─┘"],
    ["   ", "╶─╴", "   "],
];

pub const HEIGHT: u16 = 3;

/// Cells needed to draw `text` (digits, '-', and '.').
pub fn width(text: &str) -> u16 {
    let glyphs = text.chars().filter(|c| *c != '.').count() as u16;
    (glyphs * 4).saturating_sub(1)
}

/// Draws `text` with its top-left corner at `(x, y)`, clipped to the buffer. Characters
/// other than digits, '-' and '.' are skipped.
pub fn render(text: &str, x: u16, y: u16, style: Style, light: bool, buf: &mut Buffer) {
    let font = if light { &LIGHT } else { &HEAVY };
    let area = buf.area;
    // Coordinates are worked out in u32 so a reading that starts near the edge of the
    // u16 range can't overflow; anything outside the buffer is simply not drawn.
    let put = |cx: u32, cy: u32, symbol: &str, buf: &mut Buffer| {
        let inside = cx >= u32::from(area.left())
            && cx < u32::from(area.right())
            && cy >= u32::from(area.top())
            && cy < u32::from(area.bottom());
        if inside {
            if let Some(cell) = buf.cell_mut((cx as u16, cy as u16)) {
                cell.set_symbol(symbol).set_style(style);
            }
        }
    };
    let (x, y) = (u32::from(x), u32::from(y));
    let mut cx = x;
    for ch in text.chars() {
        let glyph = match ch {
            '0'..='9' => &font[ch as usize - '0' as usize],
            '-' => &font[10],
            '.' => {
                // The point goes in the gap after the previous glyph.
                if cx > x {
                    put(cx - 1, y + 2, "•", buf);
                }
                continue;
            }
            _ => continue,
        };
        for (row, line) in glyph.iter().enumerate() {
            for (col, symbol) in line.chars().enumerate() {
                if symbol != ' ' {
                    let mut utf8 = [0u8; 4];
                    put(cx + col as u32, y + row as u32, symbol.encode_utf8(&mut utf8), buf);
                }
            }
        }
        cx += 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::layout::Rect;

    fn draw(text: &str) -> Vec<String> {
        let mut buf = Buffer::empty(Rect::new(0, 0, width(text), HEIGHT));
        render(text, 0, 0, Style::default(), false, &mut buf);
        (0..HEIGHT)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    #[test]
    fn renders_a_reading() {
        assert_eq!(draw("7.65"), ["╺━┓ ┏━╸ ┏━╸", "  ┃ ┣━┓ ┗━┓", "  ╹•┗━┛ ╺━┛"]);
        assert_eq!(width("7.65"), 11);
        assert_eq!(width("105"), 11);
    }

    #[test]
    fn every_glyph_is_three_cells_wide() {
        for glyph in HEAVY.iter().chain(LIGHT.iter()) {
            for row in glyph {
                assert_eq!(row.chars().count(), 3, "{:?}", glyph);
            }
        }
    }

    #[test]
    fn drawing_is_clipped_to_the_buffer() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 2));
        render("88.8", 3, 1, Style::default(), false, &mut buf);
        render("88.8", u16::MAX - 2, 0, Style::default(), true, &mut buf);
    }
}

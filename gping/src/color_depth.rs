//! Terminal colour support.
//!
//! The UI is designed in 24-bit colour. Terminals that can't show it get every colour in the
//! frame mapped to the nearest one they do support, in a single pass over the buffer just
//! before it is flushed, so the widgets themselves never have to care.

use tui::buffer::Buffer;
use tui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Ansi256,
    Ansi16,
    /// No colour at all (`NO_COLOR`): shapes, gaps and words carry every meaning.
    Mono,
}

impl ColorDepth {
    /// Works out what the terminal supports from the environment. This errs on the side of
    /// 256 colours, which almost every terminal in use today understands. `GPING_COLOR`
    /// (truecolor, 256, 16 or none) overrides the guess, and `NO_COLOR` turns colour off.
    pub fn detect() -> ColorDepth {
        let var = |name: &str| std::env::var(name).unwrap_or_default().to_ascii_lowercase();
        if let Some(depth) = Self::from_name(&var("GPING_COLOR")) {
            return depth;
        }
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return ColorDepth::Mono;
        }
        Self::from_env(
            &var("COLORTERM"),
            &var("TERM"),
            &var("TERM_PROGRAM"),
            std::env::var_os("WT_SESSION").is_some(),
        )
    }

    fn from_name(name: &str) -> Option<ColorDepth> {
        match name {
            "truecolor" | "24bit" => Some(ColorDepth::TrueColor),
            "256" => Some(ColorDepth::Ansi256),
            "16" => Some(ColorDepth::Ansi16),
            "none" | "mono" => Some(ColorDepth::Mono),
            _ => None,
        }
    }

    fn from_env(
        colorterm: &str,
        term: &str,
        term_program: &str,
        windows_terminal: bool,
    ) -> ColorDepth {
        if colorterm == "truecolor" || colorterm == "24bit" {
            return ColorDepth::TrueColor;
        }
        if windows_terminal || cfg!(windows) {
            return ColorDepth::TrueColor;
        }
        if ["direct", "truecolor", "24bit"]
            .iter()
            .any(|t| term.contains(t))
        {
            return ColorDepth::TrueColor;
        }
        if [
            "iterm.app",
            "wezterm",
            "vscode",
            "ghostty",
            "hyper",
            "tabby",
            "rio",
        ]
        .contains(&term_program)
        {
            return ColorDepth::TrueColor;
        }
        if term.contains("256") {
            return ColorDepth::Ansi256;
        }
        if term == "linux" || term == "ansi" || term.starts_with("vt") || term.starts_with("cons") {
            return ColorDepth::Ansi16;
        }
        ColorDepth::Ansi256
    }

    /// Maps a foreground colour to one this terminal can show.
    pub fn adapt_fg(self, color: Color) -> Color {
        match (self, color) {
            (ColorDepth::Mono, _) => Color::Reset,
            (ColorDepth::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
            (ColorDepth::Ansi16, Color::Rgb(r, g, b)) => nearest_16(r, g, b),
            (ColorDepth::Ansi16, Color::Indexed(i)) if i >= 16 => {
                let (r, g, b) = indexed_to_rgb(i);
                nearest_16(r, g, b)
            }
            _ => color,
        }
    }

    /// Maps a background colour to one this terminal can show. With 16 colours there is no
    /// telling what the theme makes of them, so only the named ones the UI chose survive.
    pub fn adapt_bg(self, color: Color) -> Color {
        match (self, color) {
            (ColorDepth::Mono, _) => Color::Reset,
            (ColorDepth::Ansi16, Color::Rgb(..) | Color::Indexed(_)) => Color::Reset,
            _ => self.adapt_fg(color),
        }
    }

    /// Rewrites every colour in the buffer for this terminal.
    pub fn adapt_buffer(self, buf: &mut Buffer) {
        if self == ColorDepth::TrueColor {
            return;
        }
        for cell in buf.content.iter_mut() {
            cell.fg = self.adapt_fg(cell.fg);
            cell.bg = self.adapt_bg(cell.bg);
        }
    }
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    // Weighted towards green, where the eye is most sensitive ("redmean" approximation).
    let rmean = (a.0 as i32 + b.0 as i32) / 2;
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    (((512 + rmean) * dr * dr) >> 8) as u32
        + (4 * dg * dg) as u32
        + (((767 - rmean) * db * db) >> 8) as u32
}

fn nearest_cube_level(v: u8) -> usize {
    CUBE.iter()
        .enumerate()
        .min_by_key(|(_, &level)| (level as i32 - v as i32).abs())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// The closest entry of the xterm 256-colour palette (the 6x6x6 cube or the grey ramp;
/// the first 16 entries are left out because terminals theme them).
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (
        nearest_cube_level(r),
        nearest_cube_level(g),
        nearest_cube_level(b),
    );
    let cube = (16 + 36 * ri + 6 * gi + bi) as u8;
    let cube_rgb = (CUBE[ri], CUBE[gi], CUBE[bi]);

    let grey_level = ((r as u32 + g as u32 + b as u32) / 3) as i32;
    // Ramp entry i is 8 + 10i; round to the nearest one rather than the one below.
    let grey_index = ((grey_level - 3) / 10).clamp(0, 23) as u8;
    let grey_value = 8 + 10 * grey_index;
    let grey_rgb = (grey_value, grey_value, grey_value);

    if distance((r, g, b), grey_rgb) < distance((r, g, b), cube_rgb) {
        232 + grey_index
    } else {
        cube
    }
}

fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        16..=231 => {
            let i = i - 16;
            (
                CUBE[(i / 36) as usize],
                CUBE[((i / 6) % 6) as usize],
                CUBE[(i % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
        _ => (128, 128, 128),
    }
}

/// The closest of the 16 named ANSI colours, judged against their usual xterm values. Black
/// and dark grey are never chosen: themes make them anything from #000 to #333 (both are
/// #333333 on some), which no foreground can afford.
fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    const NAMED: [(Color, (u8, u8, u8)); 14] = [
        (Color::Red, (205, 0, 0)),
        (Color::Green, (0, 205, 0)),
        (Color::Yellow, (205, 205, 0)),
        (Color::Blue, (0, 0, 238)),
        (Color::Magenta, (205, 0, 205)),
        (Color::Cyan, (0, 205, 205)),
        (Color::Gray, (229, 229, 229)),
        (Color::LightRed, (255, 0, 0)),
        (Color::LightGreen, (0, 255, 0)),
        (Color::LightYellow, (255, 255, 0)),
        (Color::LightBlue, (92, 92, 255)),
        (Color::LightMagenta, (255, 0, 255)),
        (Color::LightCyan, (0, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    NAMED
        .iter()
        .min_by_key(|(_, rgb)| distance((r, g, b), *rgb))
        .map(|(c, _)| *c)
        .unwrap_or(Color::Reset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection() {
        use ColorDepth::*;
        assert_eq!(
            ColorDepth::from_env("truecolor", "xterm-256color", "", false),
            TrueColor
        );
        assert_eq!(
            ColorDepth::from_env("", "xterm-256color", "iterm.app", false),
            TrueColor
        );
        assert_eq!(
            ColorDepth::from_env("", "xterm-direct", "", false),
            TrueColor
        );
        if !cfg!(windows) {
            assert_eq!(
                ColorDepth::from_env("", "xterm-256color", "apple_terminal", false),
                Ansi256
            );
            assert_eq!(ColorDepth::from_env("", "screen", "", false), Ansi256);
            assert_eq!(ColorDepth::from_env("", "linux", "", false), Ansi16);
        }
        assert_eq!(ColorDepth::from_env("", "xterm", "", true), TrueColor);
    }

    #[test]
    fn exact_palette_entries_map_to_themselves() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(95, 135, 175), 16 + 36 + 2 * 6 + 3);
        assert_eq!(nearest_256(128, 128, 128), 244);
    }

    // Greys round to the nearest ramp entry: #111111 sits between 232 (#080808) and 233
    // (#121212), much closer to the latter.
    #[test]
    fn greys_round_to_the_nearest_ramp_entry() {
        assert_eq!(nearest_256(0x11, 0x11, 0x11), 233);
        assert_eq!(nearest_256(0x0e, 0x0e, 0x0e), 233);
        assert_eq!(nearest_256(0x0c, 0x0c, 0x0c), 232);
    }

    #[test]
    fn greys_prefer_the_grey_ramp() {
        // A dark grey sits between cube levels 0 and 95, and exactly on ramp entry 234.
        assert_eq!(nearest_256(0x1c, 0x1c, 0x1c), 234);
    }

    #[test]
    fn adapting_colours() {
        let c = Color::Rgb(1, 2, 3);
        assert_eq!(ColorDepth::TrueColor.adapt_fg(c), c);
        assert!(matches!(ColorDepth::Ansi256.adapt_fg(c), Color::Indexed(_)));
        assert_eq!(
            ColorDepth::Ansi16.adapt_fg(Color::Rgb(250, 10, 10)),
            Color::LightRed
        );
        // Reset and named colours pass through untouched.
        assert_eq!(ColorDepth::Ansi16.adapt_fg(Color::Reset), Color::Reset);
        assert_eq!(ColorDepth::Ansi256.adapt_fg(Color::Green), Color::Green);
        // With 16 colours, only the named backgrounds survive.
        assert_eq!(
            ColorDepth::Ansi16.adapt_bg(Color::Rgb(20, 20, 20)),
            Color::Reset
        );
        assert_eq!(ColorDepth::Ansi16.adapt_bg(Color::Red), Color::Red);
        // No colour means no colour.
        assert_eq!(ColorDepth::Mono.adapt_fg(Color::Red), Color::Reset);
        assert_eq!(ColorDepth::Mono.adapt_bg(Color::Red), Color::Reset);
    }

    // A dark foreground must not land on black or dark grey, which some themes make
    // indistinguishable from the background.
    #[test]
    fn sixteen_colours_never_pick_black_or_dark_grey() {
        for rgb in [(10, 10, 10), (60, 60, 60), (110, 120, 137), (127, 127, 127)] {
            let c = ColorDepth::Ansi16.adapt_fg(Color::Rgb(rgb.0, rgb.1, rgb.2));
            assert!(
                c != Color::Black && c != Color::DarkGray,
                "{:?} -> {:?}",
                rgb,
                c
            );
        }
    }

    #[test]
    fn overrides_by_name() {
        assert_eq!(ColorDepth::from_name("256"), Some(ColorDepth::Ansi256));
        assert_eq!(ColorDepth::from_name("none"), Some(ColorDepth::Mono));
        assert_eq!(ColorDepth::from_name("bogus"), None);
    }
}

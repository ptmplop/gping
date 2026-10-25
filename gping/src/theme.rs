//! Colours.
//!
//! The palette is designed for 24-bit colour on a dark background that gping paints itself,
//! so its contrast doesn't depend on the terminal's theme. Every foreground is at least 0.45
//! in perceived brightness (0.30 R + 0.59 G + 0.11 B) against it, which iTerm2's "minimum
//! contrast" setting then leaves alone; anything meant to look dim is a background tint
//! instead. Terminals with fewer colours get a [`Theme`] resolved for what they can show.

use crate::color_depth::ColorDepth;
use anyhow::{anyhow, Result};
use std::str::FromStr;
use tui::style::Color;

pub type Rgb = (u8, u8, u8);

/// Linear blend from `a` (t = 0) to `b` (t = 1).
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    let channel = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// Host colours, in the order they're handed out: all cool or neutral, since amber, orange
/// and red mean "slower than usual" and crimson means "lost".
pub const SERIES: [Rgb; 8] = [
    (0x38, 0xbd, 0xf8), // sky
    (0xc0, 0x84, 0xfc), // violet
    (0xa3, 0xe6, 0x35), // lime
    (0x2d, 0xd4, 0xbf), // teal
    (0xe8, 0x79, 0xf9), // fuchsia
    (0x81, 0x8c, 0xf8), // indigo
    (0x86, 0xef, 0xac), // mint
    (0xcb, 0xd5, 0xe1), // silver
];

/// The same, for terminals with 16 colours.
pub const SERIES_16: [Color; 8] = [
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightGreen,
    Color::LightCyan,
    Color::Magenta,
    Color::Blue,
    Color::Green,
    Color::White,
];

/// A shape per host, so hosts can be told apart without colour (and past the eighth one,
/// where colours repeat).
pub const SHAPES: [&str; 16] = [
    "●", "◆", "▲", "▼", "■", "○", "◇", "△", "◐", "◑", "◒", "◓", "◎", "◉", "□", "▽",
];

const BASE: Rgb = (0x0e, 0x10, 0x13);
const LOSS: Rgb = (0xff, 0x4d, 0x6a);

/// Every colour the UI uses, by role, for one colour depth.
pub struct Theme {
    /// Painted under the whole frame; `Reset` leaves the terminal's own background.
    pub base: Color,
    /// What tints are blended against.
    pub base_rgb: Rgb,
    /// Behind the host cards.
    pub panel: Color,
    pub panel_rgb: Rgb,
    /// Behind the title bar.
    pub mast: Color,
    /// Values and names.
    pub text: Color,
    /// Secondary values.
    pub soft: Color,
    /// Labels, units and axis text.
    pub muted: Color,
    /// Axis lines.
    pub chrome: Color,
    /// Guide dots.
    pub grid: Color,
    pub accent: Color,
    /// Status colours, as (foreground, background) for the status badges.
    pub good: (Color, Color),
    pub warn: (Color, Color),
    pub bad: (Color, Color),
    pub idle: (Color, Color),
    /// Lost pings: the ✕ marks and the words about them.
    pub loss: Color,
    /// Behind the columns that hold a lost ping.
    pub curtain: Option<Color>,
    /// Slower than usual: elevated, high, critical.
    pub heat: [Color; 3],
    /// Whether to tint the area under each line.
    pub fills: bool,
    /// Whether the big digits use the light line set, for console fonts.
    pub light_digits: bool,
}

impl Theme {
    /// `paint` says whether to paint the background (`GPING_BG=none` turns it off).
    pub fn new(depth: ColorDepth, paint: bool) -> Theme {
        let rgb = |c: Rgb| Color::Rgb(c.0, c.1, c.2);
        let hex = |v: u32| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let mut theme = Theme {
            base: rgb(BASE),
            base_rgb: BASE,
            panel: hex(0x151a22),
            panel_rgb: (0x15, 0x1a, 0x22),
            mast: hex(0x181e28),
            text: hex(0xe6ebf2),
            soft: hex(0xa3acb9),
            muted: hex(0x7d8796),
            chrome: hex(0x6e7889),
            grid: hex(0x747d88),
            accent: hex(0x38bdf8),
            good: (hex(0x34d399), hex(0x0f2e25)),
            warn: (hex(0xfbbf24), hex(0x33280a)),
            bad: (rgb(LOSS), hex(0x3a0d18)),
            idle: (hex(0x94a3b8), hex(0x1c2230)),
            loss: rgb(LOSS),
            curtain: Some(rgb(mix(BASE, LOSS, 0.16))),
            heat: [hex(0xffc145), hex(0xff8a3d), hex(0xff4d4d)],
            fills: true,
            light_digits: false,
        };
        match depth {
            ColorDepth::TrueColor => {}
            ColorDepth::Ansi256 => {
                // The nearest-colour mapping would collapse these greys into one another
                // and turn the curtain grey, so they get their own entries.
                theme.base = Color::Indexed(233);
                theme.panel = Color::Indexed(235);
                theme.mast = Color::Indexed(236);
                theme.curtain = Some(Color::Indexed(52));
                theme.fills = false;
            }
            ColorDepth::Ansi16 => {
                theme.base = Color::Reset;
                theme.panel = Color::Reset;
                theme.mast = Color::Reset;
                theme.text = Color::White;
                theme.soft = Color::Gray;
                theme.muted = Color::Gray;
                theme.chrome = Color::Gray;
                theme.grid = Color::Gray;
                theme.accent = Color::LightCyan;
                theme.good = (Color::LightGreen, Color::Reset);
                theme.warn = (Color::Yellow, Color::Reset);
                theme.bad = (Color::LightRed, Color::Reset);
                theme.idle = (Color::Gray, Color::Reset);
                theme.loss = Color::LightRed;
                theme.curtain = Some(Color::Red);
                theme.heat = [Color::Yellow, Color::LightRed, Color::Red];
                theme.fills = false;
                theme.light_digits = true;
            }
            ColorDepth::Mono => {
                theme.base = Color::Reset;
                theme.panel = Color::Reset;
                theme.mast = Color::Reset;
                theme.curtain = None;
                theme.fills = false;
                theme.light_digits = true;
            }
        }
        if !paint {
            theme.base = Color::Reset;
        }
        theme
    }

    /// The colour for a latency grade (see `app::grade`), given the host's own colour.
    pub fn graded(&self, grade: u8, calm: Color) -> Color {
        match grade {
            0 => calm,
            g => self.heat[(g as usize - 1).min(2)],
        }
    }
}

/// A colour for each of `count` hosts: the user's `--color` choices first, in order, then
/// the palette, skipping entries that would be hard to tell apart from a colour the user
/// chose. Each comes with an RGB approximation for blending; a named colour such as `red`
/// is still drawn as the terminal's own red, the approximation only shades fills.
pub fn series_colors(user: &[String], count: usize) -> Result<Vec<(Color, Rgb)>> {
    let mut colors = Vec::with_capacity(count);
    for name in user.iter().take(count) {
        let color =
            Color::from_str(name).map_err(|err| anyhow!("Invalid color code: `{name}` ({err})"))?;
        colors.push((color, approximate(color)));
    }
    let taken: Vec<Rgb> = colors.iter().map(|&(_, rgb)| rgb).collect();
    let mut palette = SERIES
        .iter()
        .copied()
        .filter(|&candidate| taken.iter().all(|&t| distance(candidate, t) > 60.0))
        .collect::<Vec<_>>();
    if palette.is_empty() {
        palette = SERIES.to_vec();
    }
    // Past the end of the palette colours repeat; the lanes and labels still tell hosts apart.
    let mut next = palette.iter().cycle();
    while colors.len() < count {
        let rgb = *next.next().expect("the palette is never empty");
        colors.push((Color::Rgb(rgb.0, rgb.1, rgb.2), rgb));
    }
    Ok(colors)
}

fn distance(a: Rgb, b: Rgb) -> f64 {
    let d = |x: u8, y: u8| (x as f64 - y as f64).powi(2);
    (d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)).sqrt()
}

/// The RGB a colour is usually shown as: the xterm defaults for the named and indexed ones.
pub fn approximate(color: Color) -> Rgb {
    const NAMED: [Rgb; 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let index = match color {
        Color::Rgb(r, g, b) => return (r, g, b),
        Color::Indexed(i) => i,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset => 7,
    };
    match index {
        0..=15 => NAMED[index as usize],
        16..=231 => {
            let i = index - 16;
            (
                CUBE[(i / 36) as usize],
                CUBE[((i / 6) % 6) as usize],
                CUBE[(i % 6) as usize],
            )
        }
        _ => {
            let v = 8 + 10 * (index - 232);
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing() {
        assert_eq!(mix((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(mix((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(mix((0, 100, 200), (100, 0, 200), 0.5), (50, 50, 200));
        assert_eq!(mix((10, 10, 10), (20, 20, 20), 7.0), (20, 20, 20));
        assert_eq!(mix((10, 10, 10), (20, 20, 20), f64::NAN), (10, 10, 10));
    }

    #[test]
    fn user_colours_come_first_and_keep_their_form() {
        let colors = series_colors(&["red".to_string(), "#102030".to_string()], 3).unwrap();
        assert_eq!(colors[0].0, Color::Red);
        assert_eq!(colors[1].0, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(
            colors[2].0,
            Color::Rgb(SERIES[0].0, SERIES[0].1, SERIES[0].2)
        );
    }

    #[test]
    fn invalid_colours_are_errors() {
        assert!(series_colors(&["not-a-colour".to_string()], 1).is_err());
    }

    // The old colour picker compared enum values, so `-c green` and the automatic
    // Indexed(2) it then handed to the next host were the same green on screen.
    #[test]
    fn automatic_colours_avoid_the_users_choices() {
        let sky = format!("#{:02x}{:02x}{:02x}", SERIES[0].0, SERIES[0].1, SERIES[0].2);
        let colors = series_colors(&[sky], 3).unwrap();
        assert!(colors[1..].iter().all(|&(_, rgb)| rgb != SERIES[0]));
    }

    // It used to walk Color::Indexed(2..) with an unchecked unwrap: hundreds of hosts
    // overflowed it, and the 257th hung the program.
    #[test]
    fn any_number_of_hosts_gets_a_colour() {
        let colors = series_colors(&[], 300).unwrap();
        assert_eq!(colors.len(), 300);
        assert_eq!(colors[8], colors[0]);
    }

    // iTerm2's minimum contrast (0.39 on the owner's profile) rewrites any foreground within
    // that much perceived brightness of the background. Every foreground role must clear it
    // so what's designed is what's shown.
    #[test]
    fn foregrounds_clear_minimum_contrast() {
        let brightness =
            |c: Rgb| (0.30 * c.0 as f64 + 0.59 * c.1 as f64 + 0.11 * c.2 as f64) / 255.0;
        let theme = Theme::new(ColorDepth::TrueColor, true);
        let mut roles = vec![
            theme.text,
            theme.soft,
            theme.muted,
            theme.chrome,
            theme.grid,
            theme.accent,
        ];
        roles.extend([
            theme.good.0,
            theme.warn.0,
            theme.bad.0,
            theme.idle.0,
            theme.loss,
        ]);
        roles.extend(theme.heat);
        roles.extend(SERIES.iter().map(|&(r, g, b)| Color::Rgb(r, g, b)));
        for role in roles {
            let Color::Rgb(r, g, b) = role else {
                panic!("{:?}", role)
            };
            let gap = brightness((r, g, b)) - brightness(BASE);
            assert!(
                gap >= 0.40,
                "{:?} is only {:.3} brighter than the background",
                role,
                gap
            );
        }
    }

    #[test]
    fn grades_pick_heat_colours() {
        let theme = Theme::new(ColorDepth::TrueColor, true);
        assert_eq!(theme.graded(0, Color::Blue), Color::Blue);
        assert_eq!(theme.graded(1, Color::Blue), theme.heat[0]);
        assert_eq!(theme.graded(3, Color::Blue), theme.heat[2]);
        assert_eq!(theme.graded(9, Color::Blue), theme.heat[2]);
    }

    #[test]
    fn approximations() {
        assert_eq!(approximate(Color::Red), (205, 0, 0));
        assert_eq!(approximate(Color::Indexed(196)), (255, 0, 0));
        assert_eq!(approximate(Color::Indexed(244)), (128, 128, 128));
    }
}
